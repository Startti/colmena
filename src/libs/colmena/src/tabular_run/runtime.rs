//! One call of the large path, start to finish: make sure the file is prepared,
//! verify the prepared copy through the registry, run the model's code over it
//! on a mounted executor, and say what happened in terms the tool can show.
//!
//! This is what the routing hands a large file to. It never falls back: every
//! way the call cannot run is a typed [`RunRefusal`], and the original file is
//! never read.

use super::collect::RejectReason;
use super::mounted::{MountedCall, MountedError, MountedExecutor, OUT_MIB};
use super::outputs::{OutputGuard, StoreSink};
use super::prelude::{prelude_inputs, tables_summary, unwrap_emitted, wrap_large_code};
use super::refusal::{FailureReason, RunRefusal, Unavailable};
use super::stage::StageLimits;
use super::verify::{still_current, verify_prepared};
use crate::dag_engine::domain::python_executor::{PythonRunError, PythonRunRequest};
use crate::dag_engine::infrastructure::python_exec::protocol::CRASHED_MESSAGE;
use crate::storage::domain::OutputStorageRepository;
use crate::tabular_prepare::ports::{PrepareRequest, ProgressState};
use crate::tabular_prepare::registry::PreparationRegistry;
use crate::tabular_prepare::{EnsureOutcome, PrepareError, TabularPrepare};
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;

/// The phases of a large call and the time each may take. They add up to less than
/// the call's clock (`PHASE_SUM_MAX`, 900 s, the progress ticker's longest), with
/// the rest as slack for the waits between them: preparation 180 s, staging the
/// data 150 s, the code 300 s, reading back and storing the outputs 150 s.
pub const PREPARE_BUDGET: Duration = Duration::from_secs(180);
pub const STAGE_BUDGET: Duration = Duration::from_secs(150);
pub const RUN_BUDGET: Duration = Duration::from_secs(300);
pub const COLLECT_BUDGET: Duration = Duration::from_secs(150);

/// Which phase a call is in, for the answer that says where it was cut.
#[derive(Debug, Default)]
pub struct PhaseCell(std::sync::atomic::AtomicU8);

impl PhaseCell {
    fn set(&self, phase: u8) {
        self.0.store(phase, std::sync::atomic::Ordering::SeqCst);
    }

    /// Words for the model: where the call was.
    pub fn describe(&self) -> &'static str {
        match self.0.load(std::sync::atomic::Ordering::SeqCst) {
            1 => "while preparing the file",
            2 => "while staging the data, running the code or reading back its files",
            _ => "before it started",
        }
    }
}

/// What one call is allowed.
#[derive(Debug, Clone, Copy)]
pub struct RuntimeConfig {
    /// Longest a call waits for a preparation that is still running (240 s).
    pub prepare_wait: Duration,
    /// Deadline of the model's code on the large path only (`HEAVY_TIMEOUT_SECS`, 300 s).
    pub heavy_timeout: Duration,
    pub limits: StageLimits,
    pub out_mb: u64,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            prepare_wait: PREPARE_BUDGET,
            heavy_timeout: RUN_BUDGET,
            limits: StageLimits {
                total_time: STAGE_BUDGET,
                ..StageLimits::default()
            },
            out_mb: OUT_MIB,
        }
    }
}

/// What the tool knows about a large file and the code to run over it.
#[derive(Debug, Clone)]
pub struct LargeRunRequest {
    /// From the session's own catalog row, never from the model.
    pub source_key: String,
    pub mime_type: String,
    pub filename: String,
    pub size_bytes: u64,
    pub code: String,
    /// Tables to make readable; empty is all of them.
    pub tables: Vec<String>,
    /// Where generated files belong, as for any attachment the engine stores.
    pub session_id: Option<String>,
    pub agent_session_id: Option<String>,
    /// Updated as the call moves on, so a cut-off can say where it was.
    pub phase: std::sync::Arc<PhaseCell>,
}

/// A file the code returned, stored and described.
#[derive(Debug, Clone, PartialEq)]
pub struct EmittedOutput {
    pub name: String,
    pub mime_type: String,
    pub size_bytes: u64,
    /// The engine's own storage handle, as for every generated attachment.
    pub storage_key: String,
    /// What the code said about it, when it matches a kept file (untrusted, cleaned).
    pub rows: Option<u64>,
    pub dtypes: Vec<(String, String)>,
}

/// A call whose code ran to the end. The files it returned are in storage and
/// stay there only if the holder commits `guard` (after registering them): dropped
/// without it, they are deleted.
#[derive(Debug)]
pub struct LargeRunOutput {
    pub stdout: String,
    pub result: Value,
    /// Names, rows and column types of the tables the code could read.
    pub tables: Value,
    /// The files the code returned, already in storage.
    pub emitted: Vec<EmittedOutput>,
    /// What was written to the output volume and not kept, and why, as text
    /// for the model (a name only when it passed the charset).
    pub not_kept: Vec<String>,
    pub guard: OutputGuard,
}

/// Why a call produced no output.
#[derive(Debug, Clone, PartialEq)]
pub enum LargeRunError {
    /// Refused before any code ran.
    Refused(RunRefusal),
    /// The code ran and failed: the text the model needs to fix it.
    Python(String),
    Timeout {
        secs: u64,
    },
    /// The executor failed for a reason that is not the code's. The detail is
    /// logged, never returned: it can carry a socket path of the host. `retryable`
    /// is false when the failure reads as a setup problem, which no wait fixes.
    Internal {
        retryable: bool,
    },
}

/// What the model is told when the child ended without a result.
const MEMORY_TEXT: &str = "the run ended without returning a result; it probably ran out of \
    memory. Select fewer columns with `read(columns=[...])`, or loop \
    `for part in tables[name].parts(columns=[...])` and combine per-part results";

/// Whether an executor failure's text reads as a setup problem (no wait fixes it).
/// A heuristic on the executor's own fixed texts; unknown text is a moment.
fn reads_as_setup_problem(text: &str) -> bool {
    let lower = text.to_lowercase();
    [
        "misconfigured",
        "is not configured",
        "no staging directory",
        "requires linux",
    ]
    .iter()
    .any(|k| lower.contains(k))
}

/// Why an output was not kept, in words for the model.
fn reason_text(reason: RejectReason) -> &'static str {
    match reason {
        RejectReason::BadName => "the name is not allowed (letters, digits, '.', '_' and '-', ending in .csv or .parquet)",
        RejectReason::NotARegularFile => "not a regular file",
        RejectReason::HardLinked => "more than one name for the file",
        RejectReason::TooLarge => "over the size limit for one file",
        RejectReason::NameCollision => "another file has the same name ignoring case; neither was kept",
        RejectReason::OverFileCount => "over the number of files allowed",
        RejectReason::OverTotal => "over the size limit for all files together",
        RejectReason::Unreadable => "could not be read",
    }
}

pub struct LargeTabularRuntime {
    prepare: TabularPrepare,
    registry: Arc<dyn PreparationRegistry>,
    storage: Arc<dyn OutputStorageRepository>,
    executor: Arc<dyn MountedExecutor>,
    config: RuntimeConfig,
}

impl LargeTabularRuntime {
    pub fn new(
        prepare: TabularPrepare,
        registry: Arc<dyn PreparationRegistry>,
        storage: Arc<dyn OutputStorageRepository>,
        executor: Arc<dyn MountedExecutor>,
    ) -> Self {
        Self {
            prepare,
            registry,
            storage,
            executor,
            config: RuntimeConfig::default(),
        }
    }

    pub fn with_config(mut self, config: RuntimeConfig) -> Self {
        self.config = config;
        self
    }

    /// Makes sure the file is prepared and returns once it is, or why not.
    async fn ensure(&self, req: &LargeRunRequest) -> Result<(), RunRefusal> {
        let request = PrepareRequest {
            source_key: req.source_key.clone(),
            mime_type: req.mime_type.clone(),
            filename: req.filename.clone(),
            size_bytes: req.size_bytes,
        };
        match self
            .prepare
            .ensure_prepared(&request, self.config.prepare_wait)
            .await
        {
            Ok(EnsureOutcome::Ready(_)) => Ok(()),
            Ok(EnsureOutcome::NotEnabled) => Err(RunRefusal::NotEnabled),
            Ok(EnsureOutcome::StillPreparing { progress }) => {
                let percent = progress.and_then(|p| match (p.state, p.total) {
                    (ProgressState::Running, Some(total)) if total > 0 => {
                        Some((p.done.saturating_mul(100) / total).min(100))
                    }
                    _ => None,
                });
                Err(RunRefusal::StillPreparing { percent })
            }
            Ok(EnsureOutcome::Failed {
                error_code,
                final_failure,
                ..
            }) => Err(RunRefusal::PreparationFailed {
                reason: FailureReason::from_code(&error_code),
                final_failure,
            }),
            Err(PrepareError::Registry(_)) => Err(RunRefusal::Unavailable(Unavailable::Registry)),
            // The host's trigger answers an error for a file it will never prepare:
            // terminal, and said so, not a retry-later.
            Err(PrepareError::Trigger(_)) => Err(RunRefusal::NeverPrepared),
        }
    }

    /// Runs the model's code over the prepared tables of `req`.
    pub async fn run(&self, req: LargeRunRequest) -> Result<LargeRunOutput, LargeRunError> {
        let refused = LargeRunError::Refused;
        req.phase.set(1);
        self.ensure(&req).await.map_err(refused)?;
        let plan = verify_prepared(&*self.registry, &*self.storage, &req.source_key)
            .await
            .map_err(refused)?;
        let chosen = plan.select(&req.tables).map_err(refused)?;
        let timeout = self.config.heavy_timeout;
        req.phase.set(2);
        let sink = StoreSink::new(
            self.storage.clone(),
            req.session_id.clone(),
            req.agent_session_id.clone(),
        );
        let call = PythonRunRequest {
            code: wrap_large_code(&req.code),
            mode: "restricted".to_string(),
            timeout: Some(timeout),
            inputs: prelude_inputs(plan.manifest(), &chosen),
        };
        let mounted = MountedCall {
            storage: &*self.storage,
            plan: &plan,
            tables: &chosen,
            limits: self.config.limits,
            out_mb: self.config.out_mb,
            sink: Some(&sink),
        };
        match self.executor.run_with_mounts(call, mounted).await {
            Ok(done) => {
                // The file may have been prepared again, or its copy removed, while
                // the call ran: what the code read could mix two generations, and
                // the row is only read at the start and the end. A changed copy is
                // not answered (the sink drops, deleting the files it stored). A
                // registry that cannot be read says nothing either way.
                if still_current(&*self.registry, &req.source_key, plan.generation()).await
                    == Some(false)
                {
                    return Err(refused(RunRefusal::CopyChanged));
                }
                let (result, reports) = unwrap_emitted(done.result.output.unwrap_or(Value::Null));
                let (stored, guard) = sink.take_guarded();
                let emitted = stored
                    .into_iter()
                    .map(|stored| {
                        let report = reports.iter().find(|r| r.name == stored.name);
                        EmittedOutput {
                            rows: report.and_then(|r| r.rows),
                            dtypes: report.map(|r| r.dtypes.clone()).unwrap_or_default(),
                            name: stored.name,
                            mime_type: stored.mime_type,
                            size_bytes: stored.size_bytes,
                            storage_key: stored.storage_key,
                        }
                    })
                    .collect();
                let mut not_kept: Vec<String> = done
                    .rejected
                    .iter()
                    .map(|r| match &r.name {
                        Some(n) => format!("{n}: {}", reason_text(r.reason)),
                        None => format!("a file with an invalid name: {}", reason_text(r.reason)),
                    })
                    .collect();
                if done.too_many_entries {
                    not_kept.push(
                        "the output folder held too many entries, so nothing was kept".into(),
                    );
                }
                Ok(LargeRunOutput {
                    stdout: done.result.stdout,
                    result,
                    tables: tables_summary(plan.manifest(), &chosen),
                    emitted,
                    not_kept,
                    guard,
                })
            }
            Err(MountedError::Refused(RunRefusal::CopyDamaged)) => {
                // Make the next claim prepare it again (the row must still be exactly
                // what was observed); best effort, and the answer is the same.
                if let Ok(Some(row)) = self.registry.get(&req.source_key).await {
                    let _ = self
                        .registry
                        .mark_manifest_missing(&row, chrono::Utc::now())
                        .await;
                }
                Err(refused(RunRefusal::CopyDamaged))
            }
            Err(MountedError::Refused(r)) => Err(refused(r)),
            Err(MountedError::Run(PythonRunError::Python(text))) => {
                if text == CRASHED_MESSAGE || text.contains("MemoryError") {
                    Err(LargeRunError::Python(MEMORY_TEXT.to_string()))
                } else {
                    Err(LargeRunError::Python(text))
                }
            }
            Err(MountedError::Run(PythonRunError::Timeout)) => Err(LargeRunError::Timeout {
                secs: timeout.as_secs(),
            }),
            Err(MountedError::Run(PythonRunError::Internal(text))) => {
                tracing::warn!(target: "colmena::tabular_run", detail = %text, "large-file call: executor failure");
                Err(LargeRunError::Internal {
                    retryable: !reads_as_setup_problem(&text),
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::refusal::Budget;
    use super::super::testkit::*;
    use super::*;
    use crate::tabular_prepare::ports::PrepareConfig;
    use serde_json::json;

    fn request(code: &str, tables: &[&str]) -> LargeRunRequest {
        LargeRunRequest {
            source_key: SOURCE.into(),
            mime_type: "text/csv".into(),
            filename: "sales.csv".into(),
            size_bytes: 60_000_000,
            code: code.into(),
            tables: tables.iter().map(|s| s.to_string()).collect(),
            session_id: Some("s1".into()),
            agent_session_id: Some("a1".into()),
            phase: Default::default(),
        }
    }

    #[tokio::test]
    async fn a_ready_file_runs_with_the_wrapped_code_the_heavy_deadline_and_its_tables() {
        let p = prepared(&[("sales", 2), ("stores", 1)], 4).await;
        let exec = Recorder::ok(json!(42));
        let out = runtime(&p, exec.clone(), true)
            .run(request("result = 42", &["stores"]))
            .await
            .unwrap();
        assert_eq!(out.result, json!(42));
        assert_eq!(out.stdout, "hi\n");
        assert_eq!(out.tables[0]["name"], "stores");
        assert_eq!(out.tables.as_array().unwrap().len(), 1);
        let seen = exec.seen.lock().unwrap();
        let (req, chosen, out_mb) = &seen[0];
        assert_eq!(req.mode, "restricted");
        assert_eq!(req.timeout, Some(Duration::from_secs(300)));
        assert!(req.code.contains("result = 42") && req.code.contains("tables = _Tables"));
        assert_eq!(chosen, &vec![1]);
        assert_eq!(*out_mb, OUT_MIB);
        assert_eq!(req.inputs["_ct_tables"][0]["index"], 1);
    }

    #[tokio::test]
    async fn no_tables_named_means_all_of_them() {
        let p = prepared(&[("sales", 1), ("stores", 1)], 4).await;
        let exec = Recorder::ok(Value::Null);
        runtime(&p, exec.clone(), true)
            .run(request("pass", &[]))
            .await
            .unwrap();
        assert_eq!(exec.seen.lock().unwrap()[0].1, vec![0, 1]);
    }

    /// Every way the call cannot run is a refusal and the executor is never
    /// asked: no code runs over data that is not vouched for.
    #[tokio::test]
    async fn a_table_that_is_not_there_is_refused_before_the_executor() {
        let p = prepared(&[("sales", 1)], 4).await;
        let exec = Recorder::ok(Value::Null);
        let err = runtime(&p, exec.clone(), true)
            .run(request("pass", &["nope"]))
            .await
            .unwrap_err();
        assert_eq!(
            err,
            LargeRunError::Refused(RunRefusal::NoSuchTable {
                name: "nope".into()
            })
        );
        assert_eq!(exec.calls(), 0);
    }

    #[tokio::test]
    async fn a_switch_that_is_off_runs_nothing() {
        let p = prepared(&[("sales", 1)], 4).await;
        let exec = Recorder::ok(Value::Null);
        let err = runtime(&p, exec.clone(), false)
            .run(request("pass", &[]))
            .await
            .unwrap_err();
        assert_eq!(err, LargeRunError::Refused(RunRefusal::NotEnabled));
        assert_eq!(exec.calls(), 0);
        assert!(p.storage.reads.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_file_that_is_still_preparing_is_refused_and_nothing_is_read() {
        let (registry, _dir) = sqlite_registry().await;
        claim(&registry, crate::tabular_prepare::registry::FORMAT_VERSION).await;
        let storage = FakeStorage::new();
        let exec = Recorder::ok(Value::Null);
        let config = PrepareConfig {
            large_tabular: true,
            ..PrepareConfig::default()
        };
        let rt = LargeTabularRuntime::new(
            TabularPrepare::new(config, registry.clone()),
            registry,
            storage.clone(),
            exec.clone(),
        )
        .with_config(RuntimeConfig {
            prepare_wait: Duration::from_millis(50),
            ..RuntimeConfig::default()
        });
        let err = rt.run(request("pass", &[])).await.unwrap_err();
        assert_eq!(
            err,
            LargeRunError::Refused(RunRefusal::StillPreparing { percent: None })
        );
        assert_eq!(exec.calls(), 0);
        assert!(storage.reads.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_final_preparation_failure_is_refused_with_its_reason() {
        let (registry, _dir) = sqlite_registry().await;
        let now = chrono::Utc::now();
        for attempt in 0..crate::tabular_prepare::registry::MAX_ATTEMPTS {
            let owner = format!("job-{attempt}");
            registry
                .claim(crate::tabular_prepare::registry::ClaimRequest {
                    source_key: SOURCE.into(),
                    source_bytes: 1,
                    format_version: crate::tabular_prepare::registry::FORMAT_VERSION,
                    owner: owner.clone(),
                    lease: chrono::Duration::minutes(5),
                    now,
                })
                .await
                .unwrap()
                .expect("claim won");
            registry
                .fail(SOURCE, &owner, "time", "gs://secret/key", now)
                .await
                .unwrap();
        }
        let exec = Recorder::ok(Value::Null);
        let config = PrepareConfig {
            large_tabular: true,
            ..PrepareConfig::default()
        };
        let rt = LargeTabularRuntime::new(
            TabularPrepare::new(config, registry.clone()),
            registry,
            FakeStorage::new(),
            exec.clone(),
        );
        let err = rt.run(request("pass", &[])).await.unwrap_err();
        let LargeRunError::Refused(refusal) = &err else {
            panic!("{err:?}")
        };
        assert_eq!(
            refusal,
            &RunRefusal::PreparationFailed {
                reason: FailureReason::Time,
                final_failure: true
            }
        );
        assert!(!refusal.message().contains("secret"));
        assert_eq!(exec.calls(), 0);
    }

    #[tokio::test]
    async fn the_executors_refusal_passes_through_typed() {
        let p = prepared(&[("sales", 1)], 4).await;
        let exec = Recorder::answering(Err(MountedError::Refused(RunRefusal::OverBudget(
            Budget::Volumes,
        ))));
        let err = runtime(&p, exec, true)
            .run(request("pass", &[]))
            .await
            .unwrap_err();
        assert_eq!(
            err,
            LargeRunError::Refused(RunRefusal::OverBudget(Budget::Volumes))
        );
    }

    #[tokio::test]
    async fn the_codes_failures_are_reported_as_the_code_s_and_memory_is_named() {
        let cases = [
            (
                PythonRunError::Python("NameError: name 'x' is not defined".into()),
                LargeRunError::Python("NameError: name 'x' is not defined".into()),
            ),
            (
                PythonRunError::Python(CRASHED_MESSAGE.into()),
                LargeRunError::Python(MEMORY_TEXT.into()),
            ),
            (
                PythonRunError::Python("Traceback ...\nMemoryError".into()),
                LargeRunError::Python(MEMORY_TEXT.into()),
            ),
            (
                PythonRunError::Timeout,
                LargeRunError::Timeout { secs: 300 },
            ),
            (
                PythonRunError::Internal("PythonExecutorError: boom".into()),
                LargeRunError::Internal { retryable: true },
            ),
        ];
        for (run_error, expected) in cases {
            let p = prepared(&[("sales", 1)], 4).await;
            let exec = Recorder::answering(Err(MountedError::Run(run_error)));
            let err = runtime(&p, exec, true)
                .run(request("pass", &[]))
                .await
                .unwrap_err();
            assert_eq!(err, expected);
        }
    }

    /// The trigger refusing a file (it will never be prepared) is a clear,
    /// terminal refusal naming no key and no adapter text, and nothing runs.
    #[tokio::test]
    async fn a_trigger_that_refuses_the_file_is_a_terminal_refusal() {
        use crate::tabular_prepare::ports::{PrepareRequest, PrepareTrigger, PrepareTriggerError};
        struct Refusing;
        #[async_trait::async_trait]
        impl PrepareTrigger for Refusing {
            async fn request(&self, _: PrepareRequest) -> Result<(), PrepareTriggerError> {
                Err(PrepareTriggerError::Unavailable(
                    "gs://secret/key: unsupported".into(),
                ))
            }
        }
        let (registry, _dir) = sqlite_registry().await;
        let exec = Recorder::ok(Value::Null);
        let config = PrepareConfig {
            large_tabular: true,
            trigger: Arc::new(Refusing),
            ..PrepareConfig::default()
        };
        let rt = LargeTabularRuntime::new(
            TabularPrepare::new(config, registry.clone()),
            registry,
            FakeStorage::new(),
            exec.clone(),
        );
        let err = rt.run(request("pass", &[])).await.unwrap_err();
        assert_eq!(err, LargeRunError::Refused(RunRefusal::NeverPrepared));
        let LargeRunError::Refused(r) = err else {
            unreachable!()
        };
        assert_eq!(r.code(), "large_tabular_failed");
        assert!(r.message().contains("will not be retried"));
        assert!(!r.message().contains("secret"));
        assert_eq!(exec.calls(), 0);
    }

    /// The files the code returned are in storage, described with what the code
    /// said about them (matched by name, cleaned); what was not kept is told in
    /// words, and the result is the code's alone.
    #[tokio::test]
    async fn the_files_the_code_returned_are_stored_and_described() {
        let p = prepared(&[("sales", 1)], 4).await;
        let answer = json!({
            "__colmena_emitted": [
                {"name": "out.csv", "rows": 2, "dtypes": {"a": "int64"}, "size": 8},
                {"name": "ghost.csv", "rows": 99}
            ],
            "result": {"total": 3}
        });
        let exec = Recorder::ok_with_files(
            answer,
            &[
                ("out.csv", b"a\n1\n2\n"),
                ("bad name.csv", b"x"),
                ("note.txt", b"x"),
            ],
        );
        let out = runtime(&p, exec, true)
            .run(request("pass", &[]))
            .await
            .unwrap();
        assert_eq!(out.result, json!({"total": 3}));
        assert_eq!(out.emitted.len(), 1);
        let e = &out.emitted[0];
        assert_eq!(
            (e.name.as_str(), e.rows, e.size_bytes),
            ("out.csv", Some(2), 6)
        );
        assert_eq!(e.dtypes, [("a".to_string(), "int64".to_string())]);
        assert_eq!(e.mime_type, "text/csv");
        assert_eq!(e.storage_key, "generated/out.csv");
        assert_eq!(
            *p.storage.stored.lock().unwrap(),
            [("out.csv".to_string(), b"a\n1\n2\n".to_vec())]
        );
        assert_eq!(out.not_kept.len(), 2, "{:?}", out.not_kept);
        assert!(out
            .not_kept
            .iter()
            .all(|t| t.starts_with("a file with an invalid name")));
        assert!(!out.not_kept.join("|").contains("bad name"));
    }

    #[tokio::test]
    async fn a_call_that_returns_no_files_has_none_stored() {
        let p = prepared(&[("sales", 1)], 4).await;
        let out = runtime(&p, Recorder::ok(json!(1)), true)
            .run(request("pass", &[]))
            .await
            .unwrap();
        assert!(out.emitted.is_empty() && out.not_kept.is_empty());
        assert!(p.storage.stored.lock().unwrap().is_empty());
    }

    #[test]
    fn the_defaults_are_the_designs_waits() {
        let c = RuntimeConfig::default();
        assert_eq!(c.prepare_wait, Duration::from_secs(180));
        assert_eq!(c.heavy_timeout, Duration::from_secs(300));
        assert_eq!(c.out_mb, OUT_MIB);
    }

    async fn settle() {
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    /// A later output that fails to store takes the earlier ones back: nothing is
    /// kept, nothing is reported as kept, and the refusal hides the adapter's text.
    #[tokio::test]
    async fn a_failing_store_deletes_the_outputs_stored_before_it() {
        let p = prepared(&[("sales", 1)], 4).await;
        *p.storage.store_fail_on.lock().unwrap() = Some(2);
        let exec = Recorder::ok_with_files(json!(1), &[("a.csv", b"1"), ("b.csv", b"2")]);
        let err = runtime(&p, exec, true)
            .run(request("pass", &[]))
            .await
            .unwrap_err();
        assert_eq!(err, LargeRunError::Refused(RunRefusal::Storage));
        settle().await;
        assert_eq!(*p.storage.deleted.lock().unwrap(), ["generated/a.csv"]);
    }

    /// The call's clock drops the future in the middle of collecting: what was
    /// already stored is deleted by the guard, not left behind.
    #[tokio::test]
    async fn a_call_dropped_while_collecting_deletes_what_it_stored() {
        let p = prepared(&[("sales", 1)], 4).await;
        *p.storage.store_stall_on.lock().unwrap() = Some((2, Duration::from_secs(30)));
        let exec = Recorder::ok_with_files(json!(1), &[("a.csv", b"1"), ("b.csv", b"2")]);
        let rt = runtime(&p, exec, true);
        let cut =
            tokio::time::timeout(Duration::from_millis(400), rt.run(request("pass", &[]))).await;
        assert!(cut.is_err(), "still collecting when it was cut");
        settle().await;
        assert_eq!(*p.storage.deleted.lock().unwrap(), ["generated/a.csv"]);
    }

    /// The holder of a finished call that never commits (its own future was dropped
    /// before registration, or registration failed) deletes the files by dropping.
    #[tokio::test]
    async fn outputs_not_committed_are_deleted_and_committed_ones_are_kept() {
        let p = prepared(&[("sales", 1)], 4).await;
        let exec = Recorder::ok_with_files(json!(1), &[("a.csv", b"1")]);
        let out = runtime(&p, exec, true)
            .run(request("pass", &[]))
            .await
            .unwrap();
        assert_eq!(out.emitted.len(), 1);
        drop(out);
        settle().await;
        assert_eq!(*p.storage.deleted.lock().unwrap(), ["generated/a.csv"]);
        let exec = Recorder::ok_with_files(json!(1), &[("b.csv", b"2")]);
        let out = runtime(&p, exec, true)
            .run(request("pass", &[]))
            .await
            .unwrap();
        out.guard.commit();
        settle().await;
        assert_eq!(
            *p.storage.deleted.lock().unwrap(),
            ["generated/a.csv"],
            "b.csv stays"
        );
    }

    /// The file is prepared again (here: its row removed) while the code runs: the
    /// answer is not given, it is retryable, and the files the code returned are
    /// deleted.
    #[tokio::test]
    async fn a_copy_that_changes_while_the_call_runs_is_not_answered() {
        let p = prepared(&[("sales", 1)], 4).await;
        let exec = Recorder::ok_with_files(json!(1), &[("a.csv", b"1")]);
        *exec.delay.lock().unwrap() = Some(Duration::from_millis(400));
        let registry = p.registry.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            registry.delete(SOURCE).await.unwrap();
        });
        let err = runtime(&p, exec, true)
            .run(request("pass", &[]))
            .await
            .unwrap_err();
        assert_eq!(err, LargeRunError::Refused(RunRefusal::CopyChanged));
        assert!(RunRefusal::CopyChanged.retryable());
        settle().await;
        assert_eq!(*p.storage.deleted.lock().unwrap(), ["generated/a.csv"]);
    }

    /// The phases fit under the call's clock, with room for the waits between them.
    #[test]
    fn the_phase_budgets_add_up_under_the_calls_clock() {
        let sum = PREPARE_BUDGET + STAGE_BUDGET + RUN_BUDGET + COLLECT_BUDGET;
        assert_eq!(sum, Duration::from_secs(780));
        assert!(
            sum < Duration::from_secs(900),
            "{sum:?} against the ticker's longest bound"
        );
        let c = RuntimeConfig::default();
        assert_eq!(
            (c.prepare_wait, c.heavy_timeout, c.limits.total_time),
            (PREPARE_BUDGET, RUN_BUDGET, STAGE_BUDGET)
        );
    }

    /// A damaged copy is reported as such (not retryable as it is) and its row is
    /// demoted so the next claim prepares it again.
    #[tokio::test]
    async fn a_damaged_copy_is_demoted_so_it_is_prepared_again() {
        use crate::tabular_prepare::registry::PrepareStatus;
        let p = prepared(&[("sales", 1)], 4).await;
        let exec = Recorder::answering(Err(MountedError::Refused(RunRefusal::CopyDamaged)));
        let err = runtime(&p, exec, true)
            .run(request("pass", &[]))
            .await
            .unwrap_err();
        assert_eq!(err, LargeRunError::Refused(RunRefusal::CopyDamaged));
        let row = p.registry.get(SOURCE).await.unwrap().unwrap();
        assert_eq!(row.status, PrepareStatus::Failed);
        assert_eq!(row.error_code.as_deref(), Some("manifest_missing"));
    }

    /// What storage is given is unique per call: the same file name from two calls
    /// reaches it under two names.
    #[tokio::test]
    async fn the_same_returned_name_reaches_storage_under_a_name_unique_to_the_call() {
        let p = prepared(&[("sales", 1)], 4).await;
        for _ in 0..2 {
            let exec = Recorder::ok_with_files(json!(1), &[("out.csv", b"1")]);
            let out = runtime(&p, exec, true)
                .run(request("pass", &[]))
                .await
                .unwrap();
            out.guard.commit();
        }
        let names = p.storage.stored_filenames.lock().unwrap().clone();
        assert_eq!(names.len(), 2);
        assert_ne!(names[0], names[1]);
        assert!(names.iter().all(|n| n.ends_with("-out.csv")));
    }
}
