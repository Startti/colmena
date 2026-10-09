//! `attachment_run_python` over a large file: the tool's answer for a call the
//! routing handed to the large path (dark behind `COLMENA_LARGE_TABULAR`).
//!
//! The answer carries `tables` (names, rows, column types) where the small path
//! carries `row_count` and `columns`. Every refusal is the typed error object of
//! [`RunRefusal`](crate::tabular_run::refusal::RunRefusal): a sentence and a code,
//! no key, no path, no adapter text.

use super::{truncate, AttachmentRunPythonArgs, OUTPUT_BYTE_CAP};
use crate::dag_engine::infrastructure::dag_tool_executor::LargeTarget;
use crate::llm::domain::ToolResult;
use crate::tabular_run::refusal::{RunRefusal, SESSION_MAX_BYTES, SESSION_MAX_FILES};
use crate::tabular_run::runtime::{LargeRunError, LargeRunRequest};
use serde::Serialize;

/// What the tool description says about large files, appended to the usual
/// description only while a runtime that can serve them is wired.
pub(super) const LARGE_FILES_TEXT: &str = "\n\nLarge files (over 50 MiB): `df` is NOT loaded. Use `tables`:\n\
- `tables.names`, `tables.schema(name)`: tables, columns, types, row counts.\n\
- `t = tables[name]` is a handle, not a DataFrame.\n\
- `t.read(columns=[...], filters=[...])`: load only the columns you need.\n\
- `for part in t.parts(columns=[...]):` up to 500,000 rows per part; aggregate\n  each part and combine. Use this for anything that touches every row.\n\
- `t.head()` to look at a few rows.\n\
A whole table cannot be loaded at once. Runs may take up to 5 minutes.\n\
To return a file, call `emit_table(df_or_parts, \"name\", \"csv\" | \"parquet\")` (up to 8 files; parquet takes one DataFrame).\n\
No charts or images: return aggregated numbers and build charts from them.\n\
The optional `tables` argument names the tables to make readable (default: all).";

/// The tool as the model sees it when large files are served: the usual
/// definition, the text above, and the `tables` argument.
pub(super) fn tool_definition() -> crate::llm::domain::tools::ToolDefinition {
    let mut def = super::build_attachment_run_python_tool_definition();
    def.description.push_str(LARGE_FILES_TEXT);
    if let Some(properties) = def
        .input_schema_override
        .as_mut()
        .and_then(|schema| schema.get_mut("properties"))
        .and_then(|p| p.as_object_mut())
    {
        properties.insert(
            "tables".to_string(),
            serde_json::json!({
                "type": "array",
                "items": {"type": "string"},
                "description": "Large files only: the tables to make readable (default: all)."
            }),
        );
    }
    def
}

/// The tool's response for a call that ran over a large file.
#[derive(Debug, Serialize)]
struct LargeResponse {
    stdout: String,
    result: serde_json::Value,
    duration_ms: u64,
    /// What the code could read: `[{name, rows, columns: [{name, type}]}]`.
    #[serde(skip_serializing_if = "serde_json::Value::is_null")]
    tables: serde_json::Value,
    /// Set when `result` was cut: what to do instead.
    #[serde(skip_serializing_if = "Option::is_none")]
    result_note: Option<&'static str>,
    /// The files the code returned, already attachments of the session.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    emitted: Vec<EmittedFile>,
    /// What was written and not kept, and why.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    not_kept: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

/// A returned file, as the model sees it: its id is the engine's own handle for
/// a generated attachment, never a host key.
#[derive(Debug, Serialize)]
struct EmittedFile {
    name: String,
    mime_type: String,
    size_bytes: u64,
    document_id: String,
    /// What the code SAID about the file (untrusted: it wrote the file).
    #[serde(skip_serializing_if = "Option::is_none")]
    rows_reported_by_code: Option<u64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    dtypes_reported_by_code: Vec<(String, String)>,
}

/// An error answer that says whether asking again can work.
fn retryable_error(call_id: &str, message: String, retryable: bool) -> ToolResult {
    let v = serde_json::json!({"error": message, "retryable": retryable, "source": "execution"});
    ToolResult::success(call_id.to_string(), v.to_string())
}

/// The result, or a cut of it with the note that says what to do.
fn cap_result(result: serde_json::Value) -> (serde_json::Value, Option<&'static str>) {
    let text = result.to_string();
    if text.len() <= OUTPUT_BYTE_CAP {
        return (result, None);
    }
    (
        serde_json::Value::String(truncate(&text, OUTPUT_BYTE_CAP)),
        Some("result was too large to return in full: aggregate it first, or write it with emit_table(...) and return the file"),
    )
}

fn answer(call_id: &str, response: &LargeResponse) -> ToolResult {
    let v = serde_json::to_value(response).unwrap_or(serde_json::Value::Null);
    ToolResult::success(call_id.to_string(), v.to_string())
}

/// The whole large call may take this long, tool-progress events keeping the run
/// loop's idle watchdog (300 s by default) from cutting it: the longest the
/// ticker accepts. The parts inside it have their own limits (preparation wait
/// 240 s, run 300 s, each transfer 240 s).
const CALL_BUDGET: std::time::Duration = std::time::Duration::from_secs(
    crate::dag_engine::infrastructure::dag_tool_executor::TOOL_PROGRESS_MAX_BOUND_SECS,
);

/// The `tables` the model named, read leniently from the raw arguments and only
/// here, on the large path: null, a missing key, a single string or a list with
/// odd members never fail the call (and never touch any other path). Not names
/// (non-strings) are ignored; at most 64 are kept.
fn requested_tables(raw_arguments: &str) -> Vec<String> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw_arguments) else {
        return vec![];
    };
    match value.get("tables") {
        Some(serde_json::Value::String(one)) => vec![one.clone()],
        Some(serde_json::Value::Array(items)) => items
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .take(64)
            .collect(),
        _ => vec![],
    }
}

pub(super) async fn dispatch(
    executor: &crate::dag_engine::infrastructure::dag_tool_executor::DagToolExecutor,
    call_id: &str,
    args: &AttachmentRunPythonArgs,
    raw_arguments: &str,
    target: LargeTarget,
) -> ToolResult {
    let every = std::time::Duration::from_secs(
        crate::dag_engine::infrastructure::dag_tool_executor::TOOL_PROGRESS_INTERVAL_SECS,
    );
    dispatch_bounded(
        executor,
        call_id,
        args,
        raw_arguments,
        target,
        every,
        CALL_BUDGET,
    )
    .await
}

/// What happened to the files a call returned.
enum Keeping {
    /// These keys are kept (all of them, or, when a row could not be taken back,
    /// the ones that stayed).
    Kept(Vec<String>),
    /// None was kept; the sentence says why, in words the model can act on.
    NotKept(String),
}

/// One lock per conversation: two calls of the same conversation count and register
/// their files one after the other, so the quota is exact within a process. (Across
/// processes it is not: each instance can add at most one call's files, 8, past it.)
fn session_lock(session: &str) -> std::sync::Arc<tokio::sync::Mutex<()>> {
    static LOCKS: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<String, std::sync::Arc<tokio::sync::Mutex<()>>>>,
    > = std::sync::OnceLock::new();
    let mut locks = LOCKS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if locks.len() > 4096 {
        locks.retain(|_, l| std::sync::Arc::strong_count(l) > 1);
    }
    locks.entry(session.to_string()).or_default().clone()
}

/// Longest wait for the conversation's lock, and for each registry call made under it.
const LOCK_WAIT: std::time::Duration = if cfg!(test) {
    std::time::Duration::from_millis(500)
} else {
    std::time::Duration::from_secs(15)
};
const REGISTRY_STEP: std::time::Duration = if cfg!(test) {
    std::time::Duration::from_millis(300)
} else {
    std::time::Duration::from_secs(10)
};

/// Why registering the returned files stopped.
enum Stopped {
    /// Nothing wrong with the registry, but the files cannot be kept.
    Refused(String),
    /// A registration failed or its outcome is unknown.
    RegistrationFailed,
}

/// Checks the conversation's quota and registers every returned file, or none. The
/// conversation's lock is held only for the usage read and the registrations, each
/// bounded; the cleanup of a failure runs after it is released. A usage that cannot
/// be read fails closed. Each row is marked "may exist" in the ledger BEFORE it is
/// asked of the registry, so an upsert that committed but failed is still undone.
async fn keep_outputs(
    executor: &crate::dag_engine::infrastructure::dag_tool_executor::DagToolExecutor,
    files: &[crate::tabular_run::runtime::EmittedOutput],
    mut ledger: crate::tabular_run::outputs::OutputLedger,
) -> Keeping {
    if files.is_empty() {
        ledger.commit();
        return Keeping::Kept(vec![]);
    }
    let Some(session) = executor.session_for_outputs() else {
        let _ = ledger.rollback().await;
        return Keeping::NotKept(
            "the returned files could not be saved to this conversation (no session to save them in), so none was kept".into(),
        );
    };
    let stopped = {
        let lock = session_lock(&session);
        let held = tokio::time::timeout(LOCK_WAIT, lock.lock()).await;
        let stopped = match held {
            Err(_) => Some(Stopped::Refused(
                "the conversation was busy saving other returned files, so none was kept; try again".into(),
            )),
            Ok(_held) => register_all(executor, files, &mut ledger).await.err(),
        };
        stopped
    };
    match stopped {
        None => {
            ledger.commit();
            Keeping::Kept(files.iter().map(|f| f.storage_key.clone()).collect())
        }
        Some(Stopped::Refused(why)) => {
            let _ = ledger.rollback().await;
            Keeping::NotKept(why)
        }
        Some(Stopped::RegistrationFailed) => {
            // Rows first, objects second; a pair whose row is not confirmed gone stays
            // whole and is reported as kept, because it may be.
            let stuck = ledger.rollback().await;
            match stuck.is_empty() {
                true => Keeping::NotKept(
                    "the returned files could not be saved to this conversation, so none was kept"
                        .into(),
                ),
                false => Keeping::Kept(stuck),
            }
        }
    }
}

/// The part of [`keep_outputs`] that runs under the conversation's lock.
async fn register_all(
    executor: &crate::dag_engine::infrastructure::dag_tool_executor::DagToolExecutor,
    files: &[crate::tabular_run::runtime::EmittedOutput],
    ledger: &mut crate::tabular_run::outputs::OutputLedger,
) -> Result<(), Stopped> {
    let usage = tokio::time::timeout(
        REGISTRY_STEP,
        executor.generated_usage(super::ATTACHMENT_RUN_PYTHON_TOOL_NAME),
    )
    .await;
    let Ok(Some((used_files, used_bytes))) = usage else {
        return Err(Stopped::Refused(
            "this conversation's limit on returned files could not be checked, so none of the returned files was kept; try again".into(),
        ));
    };
    let new_bytes: u64 = files.iter().map(|f| f.size_bytes).sum();
    if used_files + files.len() > SESSION_MAX_FILES || used_bytes + new_bytes > SESSION_MAX_BYTES {
        return Err(Stopped::Refused(quota_text(
            files.len(),
            used_files,
            used_bytes,
        )));
    }
    for f in files {
        ledger.row_may_exist(&f.storage_key);
        let made = tokio::time::timeout(
            REGISTRY_STEP,
            executor.register_stored_attachment(
                &f.storage_key,
                &f.mime_type,
                &f.name,
                f.size_bytes,
                super::ATTACHMENT_RUN_PYTHON_TOOL_NAME,
            ),
        )
        .await;
        if !matches!(made, Ok(Ok(()))) {
            return Err(Stopped::RegistrationFailed);
        }
    }
    Ok(())
}

/// What to tell the model when the files do not fit: whether the conversation was
/// already at its limit or this call's files would take it past it.
fn quota_text(returned: usize, used_files: usize, used_bytes: u64) -> String {
    let at_limit = used_files >= SESSION_MAX_FILES || used_bytes >= SESSION_MAX_BYTES;
    match at_limit {
        true => format!(
            "none of the {returned} returned file(s) was kept: {}",
            RunRefusal::SessionQuota.message()
        ),
        false => format!(
            "none of the {returned} returned file(s) was kept: keeping them would exceed this conversation's limit of {SESSION_MAX_FILES} files or {} MiB of returned files ({used_files} files kept so far). Return fewer or smaller files, or return results in the answer instead",
            SESSION_MAX_BYTES / (1024 * 1024)
        ),
    }
}

/// Everything a finished call has to say, with the files already kept or not.
struct Finished {
    stdout: String,
    result: serde_json::Value,
    tables: serde_json::Value,
    emitted: Vec<crate::tabular_run::runtime::EmittedOutput>,
    not_kept: Vec<String>,
    keeping: Keeping,
}

/// [`dispatch`] with the ticker's interval and the call's budget given, so a test
/// can run it in seconds.
async fn dispatch_bounded(
    executor: &crate::dag_engine::infrastructure::dag_tool_executor::DagToolExecutor,
    call_id: &str,
    args: &AttachmentRunPythonArgs,
    raw_arguments: &str,
    target: LargeTarget,
    every: std::time::Duration,
    budget: std::time::Duration,
) -> ToolResult {
    use crate::dag_engine::domain::observer::ToolProgressStage;
    use crate::dag_engine::infrastructure::dag_tool_executor::ProgressTick;
    let started = std::time::Instant::now();
    // Progress events keep the run loop's idle watchdog from cutting a run that
    // is silent for minutes, and bound the whole call. `tool_id` is the id of the
    // model's tool call, which is the id the client finds the row by.
    let phase = std::sync::Arc::new(crate::tabular_run::runtime::PhaseCell::default());
    let tick = ProgressTick {
        tool_id: call_id,
        stage: ToolProgressStage::Running,
        call_started: tokio::time::Instant::now(),
        interval: every,
        call_budget: budget,
    };
    let run = target.runtime.run(LargeRunRequest {
        source_key: target.source_key,
        mime_type: target.mime_type,
        filename: target.filename,
        size_bytes: target.size_bytes,
        code: args.code.clone(),
        tables: requested_tables(raw_arguments),
        session_id: target.session_id,
        agent_session_id: target.agent_session_id,
        phase: phase.clone(),
    });
    // Keeping the returned files is part of the call, so it runs under the call's
    // clock; cut off, the ledger dropped inside undoes what it did.
    let work = async {
        let out = run.await?;
        let crate::tabular_run::runtime::LargeRunOutput {
            stdout,
            result,
            tables,
            emitted,
            not_kept,
            guard,
        } = out;
        let ledger = guard.into_ledger(executor.registry_handle());
        let keeping = keep_outputs(executor, &emitted, ledger).await;
        Ok(Finished {
            stdout,
            result,
            tables,
            emitted,
            not_kept,
            keeping,
        })
    };
    let outcome = match executor.with_progress_ticker(tick, work).await {
        Ok(outcome) => outcome,
        // The call's future was dropped: the request is closed, the child killed
        // and the volume given back by their own drop guards.
        Err(_) => {
            // Cut by the call's clock, in the phase the runtime had reached. The same
            // code repeats the same wait, so this is not retryable as it is.
            return retryable_error(
                call_id,
                format!(
                    "the large-file call did not finish within {}s and was stopped {}; nothing was kept. \
                     Reduce the work: aggregate, read fewer columns, or iterate parts() and combine",
                    budget.as_secs(),
                    phase.describe()
                ),
                false,
            );
        }
    };
    let duration_ms = started.elapsed().as_millis() as u64;
    match outcome {
        Ok(out) => {
            use Keeping::*;
            let (result, result_note) = cap_result(out.result);
            let mut not_kept = out.not_kept;
            let mut emitted = vec![];
            match out.keeping {
                Kept(kept) => {
                    for f in out.emitted.into_iter().filter(|f| kept.contains(&f.storage_key)) {
                        emitted.push(EmittedFile {
                            name: f.name,
                            mime_type: f.mime_type,
                            size_bytes: f.size_bytes,
                            document_id: f.storage_key,
                            rows_reported_by_code: f.rows,
                            dtypes_reported_by_code: f.dtypes,
                        });
                    }
                }
                NotKept(why) => not_kept.push(why),
            }
            answer(
                call_id,
                &LargeResponse {
                    stdout: truncate(&out.stdout, OUTPUT_BYTE_CAP),
                    result,
                    result_note,
                    duration_ms,
                    tables: out.tables,
                    emitted,
                    not_kept,
                    error: None,
                },
            )
        }
        Err(LargeRunError::Refused(refusal)) => {
            ToolResult::success(call_id.to_string(), refusal.to_tool_error().to_string())
        }
        Err(LargeRunError::Python(text)) => answer(
            call_id,
            &LargeResponse {
                stdout: String::new(),
                result: serde_json::Value::Null,
                result_note: None,
                duration_ms,
                tables: serde_json::Value::Null,
                emitted: vec![],
                not_kept: vec![],
                error: Some(truncate(&text, OUTPUT_BYTE_CAP)),
            },
        ),
        // The same code takes the same time again: it is not retryable as it is.
        Err(LargeRunError::Timeout { secs }) => retryable_error(
            call_id,
            format!(
                "code execution exceeded {secs}s timeout. Reduce the work: aggregate, read fewer columns, or iterate parts() and combine"
            ),
            false,
        ),
        Err(LargeRunError::Internal { retryable }) => retryable_error(
            call_id,
            match retryable {
                true => "the large-file executor failed; try again, and if it keeps failing the file cannot be analysed here".to_string(),
                false => "the large-file executor is not set up to run this; it cannot be analysed here".to_string(),
            },
            retryable,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::super::{
        build_attachment_run_python_tool_definition, dispatch_attachment_run_python_via_executor,
    };
    use crate::dag_engine::application::ports::NodeRegistryPort;
    use crate::dag_engine::domain::node::ExecutableNode;
    use crate::dag_engine::infrastructure::dag_tool_executor::DagToolExecutor;
    use crate::llm::domain::attachments::{origin, AttachmentSource};
    use crate::llm::domain::{ConversationAttachment, FunctionCall, ProviderKind, ToolCall};
    use crate::storage::domain::{MockOutputStorageRepository, StoredBytes};
    use crate::tabular_run::mounted::MountedError;
    use crate::tabular_run::refusal::Budget;
    use crate::tabular_run::refusal::RunRefusal;
    use crate::tabular_run::runtime::LargeTabularRuntime;
    use crate::tabular_run::testkit::*;
    use serde_json::{json, Value};
    use std::collections::HashMap;
    use std::sync::Arc;

    struct NoNodes;
    impl NodeRegistryPort for NoNodes {
        fn get_node(&self, _: &str) -> Option<Arc<dyn ExecutableNode>> {
            None
        }
        fn get_all_nodes(&self) -> HashMap<String, Arc<dyn ExecutableNode>> {
            HashMap::new()
        }
    }

    const MIB: u64 = 1024 * 1024;

    fn row(host: bool) -> ConversationAttachment {
        ConversationAttachment {
            agent_session_id: "agent_1".into(),
            document_id: "doc-1".into(),
            provider: ProviderKind::OpenAi,
            provider_file_id: if host { String::new() } else { "pf-1".into() },
            mime_type: "text/csv".into(),
            filename: "sales.csv".into(),
            size_bytes: Some(60 * MIB),
            label: None,
            description: None,
            source: AttachmentSource::Path(SOURCE.into()),
            registered_at: chrono::Utc::now(),
            refreshed_at: chrono::Utc::now(),
            storage_key: Some(SOURCE.into()),
            origin: Some(
                if host {
                    origin::HOST_STORAGE_REF
                } else {
                    origin::USER_UPLOAD
                }
                .into(),
            ),
            last_used_at: None,
        }
    }

    fn call(args: &str) -> ToolCall {
        ToolCall {
            id: "c1".into(),
            call_type: "function".into(),
            function: FunctionCall::new("attachment_run_python".into(), args.into()),
            response: None,
            provider_signature: None,
            scope_index: None,
        }
    }

    async fn body(ex: &DagToolExecutor, args: &str) -> Value {
        // The small path runs the code in this process.
        pyo3::Python::initialize();
        let result = dispatch_attachment_run_python_via_executor(ex, &call(args))
            .await
            .unwrap();
        serde_json::from_str(&result.output).unwrap()
    }

    fn executor(host: bool, runtime: Option<Arc<LargeTabularRuntime>>) -> DagToolExecutor {
        let mut storage = MockOutputStorageRepository::new();
        storage.expect_read().returning(|_| {
            Ok(StoredBytes {
                bytes: b"a,b\n1,2\n".to_vec(),
                mime_type: "text/csv".into(),
                filename: "sales.csv".into(),
            })
        });
        let ex = DagToolExecutor::new(Arc::new(NoNodes), Default::default())
            .with_attachments(vec![row(host)])
            .with_attachment_storage(Arc::new(storage));
        match runtime {
            Some(rt) => ex.with_large_tabular(rt),
            None => ex,
        }
    }

    #[tokio::test]
    async fn a_host_owned_file_runs_over_its_tables_and_answers_with_them() {
        let p = prepared(&[("sales", 2), ("stores", 1)], 4).await;
        let exec = Recorder::ok(json!({"total": 7}));
        let ex = executor(true, Some(Arc::new(runtime(&p, exec.clone(), true))));
        let out = body(
            &ex,
            r#"{"attachment_id":"doc-1","code":"result = 1","tables":["stores"]}"#,
        )
        .await;
        assert_eq!(out["result"], json!({"total": 7}));
        assert_eq!(out["stdout"], "hi\n");
        assert_eq!(out["tables"][0]["name"], "stores");
        assert!(
            out.get("row_count").is_none() && out.get("columns").is_none(),
            "{out}"
        );
        assert!(out.get("error").is_none(), "{out}");
        let seen = exec.seen.lock().unwrap();
        assert_eq!(seen[0].1, vec![1], "the `tables` argument was honoured");
        assert!(seen[0].0.code.contains("result = 1"));
    }

    /// Not wired (the switch is off, or the host gave the engine nothing): a
    /// host-owned file keeps the refusal it has always had, and a small file its
    /// usual path.
    #[tokio::test]
    async fn without_a_runtime_nothing_is_routed() {
        let out = body(
            &executor(true, None),
            r#"{"attachment_id":"doc-1","code":"print(1)"}"#,
        )
        .await;
        assert_eq!(out["code"], "large_tabular_file", "{out}");
        let out = body(
            &executor(false, None),
            r#"{"attachment_id":"doc-1","code":"print(1)"}"#,
        )
        .await;
        assert!(
            out.get("row_count").is_some(),
            "the small path answered: {out}"
        );
    }

    /// A small file, with the large path wired: it is not routed, the mounted
    /// executor is never asked, and the answer is the small path's.
    #[tokio::test]
    async fn a_small_file_keeps_its_path_even_with_the_runtime_wired() {
        let p = prepared(&[("sales", 1)], 4).await;
        let exec = Recorder::ok(Value::Null);
        let ex = executor(false, Some(Arc::new(runtime(&p, exec.clone(), true))));
        let out = body(&ex, r#"{"attachment_id":"doc-1","code":"print(1)"}"#).await;
        assert!(out.get("row_count").is_some(), "{out}");
        assert_eq!(exec.calls(), 0);
        assert!(p.storage.reads.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_refusal_is_the_typed_error_object_and_nothing_else() {
        let p = prepared(&[("sales", 1)], 4).await;
        let exec = Recorder::answering(Err(MountedError::Refused(RunRefusal::OverBudget(
            Budget::Volumes,
        ))));
        let ex = executor(true, Some(Arc::new(runtime(&p, exec, true))));
        let out = body(&ex, r#"{"attachment_id":"doc-1","code":"pass"}"#).await;
        assert_eq!(out["code"], "large_tabular_over_budget");
        assert_eq!(out["source"], "execution");
        assert_eq!(out.as_object().unwrap().len(), 4, "{out}");
        assert_eq!(out["retryable"], true);
        assert!(!out.to_string().contains(SOURCE));
    }

    #[tokio::test]
    async fn a_switch_that_is_off_in_the_runtime_refuses_and_runs_nothing() {
        let p = prepared(&[("sales", 1)], 4).await;
        let exec = Recorder::ok(Value::Null);
        let ex = executor(true, Some(Arc::new(runtime(&p, exec.clone(), false))));
        let out = body(&ex, r#"{"attachment_id":"doc-1","code":"pass"}"#).await;
        assert_eq!(out["code"], "large_tabular_disabled");
        assert_eq!(exec.calls(), 0);
    }

    /// The schema the model sees is the one it always saw: the `tables`
    /// argument is not in it.
    #[test]
    fn the_tool_schema_is_unchanged_by_the_tables_argument() {
        let def = serde_json::to_string(&build_attachment_run_python_tool_definition()).unwrap();
        assert!(!def.contains("tables"), "{def}");
    }

    /// With large files served the tool gains the text and the argument, and
    /// keeps everything else it had.
    #[test]
    fn the_large_file_definition_adds_the_text_and_the_argument_only() {
        let usual = build_attachment_run_python_tool_definition();
        let large = super::tool_definition();
        assert_eq!(large.name, usual.name);
        assert_eq!(large.summary, usual.summary);
        assert_eq!(
            large.description,
            format!("{}{}", usual.description, super::LARGE_FILES_TEXT)
        );
        let schema = |d: &crate::llm::domain::tools::ToolDefinition| {
            d.input_schema_override.clone().unwrap()
        };
        let (mut a, b) = (schema(&usual), schema(&large));
        assert!(b["properties"]["tables"]["items"]["type"] == "string");
        a["properties"]["tables"] = b["properties"]["tables"].clone();
        assert_eq!(a, b, "nothing else in the schema changed");
        assert!(super::LARGE_FILES_TEXT.contains("`df` is NOT loaded"));
        assert!(super::LARGE_FILES_TEXT.contains("up to 5 minutes"));
    }

    /// Collects the tool-progress events a call emitted.
    #[derive(Default)]
    struct Events(std::sync::Mutex<Vec<(String, u64)>>);
    impl crate::dag_engine::domain::observer::ExecutionObserver for Events {
        fn on_event(&self, event: crate::dag_engine::domain::observer::NodeEvent) {
            if let crate::dag_engine::domain::observer::NodeEvent::ToolProgress {
                tool_id,
                elapsed_ms,
                ..
            } = event
            {
                self.0.lock().unwrap().push((tool_id, elapsed_ms));
            }
        }
    }

    async fn slow_target(
        p: &Prepared,
        exec: Arc<Recorder>,
    ) -> (DagToolExecutor, super::LargeTarget, Arc<Events>) {
        let events = Arc::new(Events::default());
        let ex = executor(true, Some(Arc::new(runtime(p, exec, true))))
            .with_observer(Some(events.clone()));
        let target = ex.large_target("doc-1").expect("routed");
        (ex, target, events)
    }

    fn args() -> super::super::AttachmentRunPythonArgs {
        serde_json::from_str(r#"{"attachment_id":"doc-1","code":"pass"}"#).unwrap()
    }

    /// A run that is silent for seconds still speaks: one event per interval,
    /// with the tool call's own id and a growing elapsed time, and none after it ends.
    #[tokio::test]
    async fn a_long_call_reports_progress_under_the_tool_calls_id() {
        let p = prepared(&[("sales", 1)], 4).await;
        let exec = Recorder::ok(json!(1));
        *exec.delay.lock().unwrap() = Some(std::time::Duration::from_millis(3500));
        let (ex, target, events) = slow_target(&p, exec).await;
        let result = super::dispatch_bounded(
            &ex,
            "call-9",
            &args(),
            "{}",
            target,
            std::time::Duration::from_secs(1),
            std::time::Duration::from_secs(60),
        )
        .await;
        assert!(result.output.contains("\"result\":1"), "{}", result.output);
        let seen = events.0.lock().unwrap().clone();
        assert!(seen.len() >= 3, "{seen:?}");
        assert!(seen.iter().all(|(id, _)| id == "call-9"));
        assert!(
            seen.windows(2).all(|w| w[0].1 < w[1].1),
            "elapsed grows: {seen:?}"
        );
        let count = seen.len();
        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
        assert_eq!(
            events.0.lock().unwrap().len(),
            count,
            "none after the call ended"
        );
    }

    /// The call has a total clock: past it the step is dropped and the answer
    /// says so without a key or an adapter's text.
    #[tokio::test]
    async fn a_call_past_its_budget_is_stopped_with_a_clear_error() {
        let p = prepared(&[("sales", 1)], 4).await;
        let exec = Recorder::ok(json!(1));
        *exec.delay.lock().unwrap() = Some(std::time::Duration::from_secs(30));
        let (ex, target, _events) = slow_target(&p, exec).await;
        let result = super::dispatch_bounded(
            &ex,
            "c1",
            &args(),
            "{}",
            target,
            std::time::Duration::from_secs(1),
            std::time::Duration::from_secs(2),
        )
        .await;
        let out: Value = serde_json::from_str(&result.output).unwrap();
        assert!(
            out["error"]
                .as_str()
                .unwrap()
                .contains("did not finish within 2s"),
            "{out}"
        );
    }

    #[test]
    fn the_calls_budget_is_the_tickers_longest() {
        assert_eq!(super::CALL_BUDGET.as_secs(), 900);
    }

    /// `tables` is read leniently and only on this path: null, a string, odd
    /// members and a missing key never fail the call.
    #[test]
    fn the_tables_argument_is_read_leniently() {
        use super::requested_tables as t;
        assert_eq!(t(r#"{"tables":["a","b"]}"#), ["a", "b"]);
        assert_eq!(t(r#"{"tables":"a"}"#), ["a"]);
        assert_eq!(t(r#"{"tables":["a",3,null,{"x":1}]}"#), ["a"]);
        for odd in [
            r#"{"tables":null}"#,
            r#"{"tables":7}"#,
            r#"{}"#,
            "not json",
            r#"{"tables":{"a":1}}"#,
        ] {
            assert!(t(odd).is_empty(), "{odd}");
        }
        assert_eq!(
            t(&format!(
                r#"{{"tables":{}}}"#,
                serde_json::json!(vec!["x"; 100])
            ))
            .len(),
            64
        );
    }

    /// The args struct no longer has the field: a wrong `tables` cannot fail parsing
    /// on any path, the switch-off one included.
    #[test]
    fn a_wrong_typed_tables_does_not_fail_argument_parsing() {
        for body in [
            r#"{"attachment_id":"d","code":"x","tables":null}"#,
            r#"{"attachment_id":"d","code":"x","tables":5}"#,
        ] {
            assert!(
                serde_json::from_str::<super::super::AttachmentRunPythonArgs>(body).is_ok(),
                "{body}"
            );
        }
    }

    /// A returned DataFrame can be hundreds of MB: the result is cut like stdout
    /// and the model is told what to do instead.
    #[test]
    fn a_huge_result_is_cut_with_a_note_and_a_small_one_is_untouched() {
        let (small, note) = super::cap_result(json!({"a": [1, 2, 3]}));
        assert_eq!((small, note), (json!({"a": [1, 2, 3]}), None));
        let big = json!({"rows": vec!["x".repeat(100); 2000]});
        let (cut, note) = super::cap_result(big);
        assert!(
            cut.as_str().unwrap().len() < 60 * 1024
                && cut.as_str().unwrap().ends_with("[truncated]")
        );
        assert!(note.unwrap().contains("emit_table"));
    }

    /// The executor's text (it can name the host's socket path) never reaches the model.
    #[tokio::test]
    async fn an_executor_failure_reaches_the_model_as_fixed_text_marked_retryable() {
        let p = prepared(&[("sales", 1)], 4).await;
        let exec = Recorder::answering(Err(MountedError::Run(
            crate::dag_engine::domain::python_executor::PythonRunError::Internal(
                "PythonExecutorError: cannot connect to /run/colmena-python-executor-1234/sock"
                    .into(),
            ),
        )));
        let ex = executor(true, Some(Arc::new(runtime(&p, exec, true))));
        let out = body(&ex, r#"{"attachment_id":"doc-1","code":"pass"}"#).await;
        assert_eq!(out["retryable"], true);
        assert!(
            !out.to_string().contains("/run/") && !out.to_string().contains("sock"),
            "{out}"
        );
    }

    /// The whole-object tools say what is true about the large-file tool.
    #[tokio::test]
    async fn the_fetch_refusal_follows_whether_the_runtime_is_wired() {
        use crate::llm::domain::large_tabular::refusal_text_for;
        let p = prepared(&[("sales", 1)], 4).await;
        let wired = executor(
            true,
            Some(Arc::new(runtime(&p, Recorder::ok(Value::Null), true))),
        );
        assert_eq!(
            wired.fetch_attachment_bytes("doc-1").await.unwrap_err(),
            refusal_text_for(true)
        );
        let bare = executor(true, None);
        assert_eq!(
            bare.fetch_attachment_bytes("doc-1").await.unwrap_err(),
            refusal_text_for(false)
        );
    }

    async fn settle() {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }

    /// Nothing is reported as kept unless it is stored AND registered: with no
    /// registry to register in, the stored file is deleted and the answer says so.
    #[tokio::test]
    async fn a_file_that_cannot_be_registered_is_deleted_and_not_reported() {
        let p = prepared(&[("sales", 1)], 4).await;
        let exec = Recorder::ok_with_files(json!(7), &[("out.csv", b"a\n1\n")]);
        let ex = executor(true, Some(Arc::new(runtime(&p, exec, true))));
        let out = body(&ex, r#"{"attachment_id":"doc-1","code":"pass"}"#).await;
        assert_eq!(out["result"], 7);
        assert!(out.get("emitted").is_none(), "{out}");
        assert!(
            out["not_kept"].to_string().contains("none was kept"),
            "{out}"
        );
        settle().await;
        assert_eq!(*p.storage.deleted.lock().unwrap(), ["generated/out.csv"]);
    }

    fn generated_rows(n: usize, each: u64) -> Vec<ConversationAttachment> {
        (0..n)
            .map(|i| ConversationAttachment {
                document_id: format!("generated/old-{i}.csv"),
                origin: Some(origin::generated_by("attachment_run_python")),
                size_bytes: Some(each),
                provider: ProviderKind::Generated,
                ..row(false)
            })
            .collect()
    }

    fn with_registry(
        rows: Vec<ConversationAttachment>,
        rt: Arc<LargeTabularRuntime>,
    ) -> DagToolExecutor {
        use crate::llm::domain::attachments::attachment_registry::MockAttachmentRegistry;
        let mut reg = MockAttachmentRegistry::new();
        reg.expect_list_for_session()
            .returning(move |_| Ok(rows.clone()));
        reg.expect_upsert().returning(|_| Ok(()));
        reg.expect_delete_attachment_for_provider()
            .returning(|_, _, _| Ok(()));
        executor(true, Some(rt))
            .with_attachment_registry(Arc::new(reg))
            .with_agent_session_id(Some("agent_1".into()))
    }

    /// A conversation that already holds the most this tool may return gets a
    /// typed refusal before anything runs.
    #[tokio::test]
    async fn a_conversation_at_its_file_quota_still_gets_its_result_but_keeps_no_file() {
        use crate::tabular_run::refusal::SESSION_MAX_FILES;
        let p = prepared(&[("sales", 1)], 4).await;
        // Nothing to keep: the call runs and answers exactly as it would otherwise.
        let exec = Recorder::ok(json!(1));
        let ex = with_registry(
            generated_rows(SESSION_MAX_FILES, 10),
            Arc::new(runtime(&p, exec.clone(), true)),
        );
        let out = body(&ex, r#"{"attachment_id":"doc-1","code":"pass"}"#).await;
        assert_eq!(out["result"], 1, "{out}");
        assert!(
            out.get("code").is_none() && out.get("not_kept").is_none(),
            "{out}"
        );
        assert_eq!(exec.calls(), 1);
        // Files it would add are refused (and deleted), with a sentence the model can act on.
        let exec = Recorder::ok_with_files(json!(2), &[("out.csv", b"12345")]);
        let ex = with_registry(
            generated_rows(SESSION_MAX_FILES, 10),
            Arc::new(runtime(&p, exec, true)),
        );
        let out = body(&ex, r#"{"attachment_id":"doc-1","code":"pass"}"#).await;
        assert_eq!(out["result"], 2);
        assert!(out.get("emitted").is_none(), "{out}");
        let why = out["not_kept"].to_string();
        assert!(
            why.contains("none of the 1 returned file(s) was kept")
                && why.contains("return results in the answer"),
            "{why}"
        );
        settle().await;
        assert_eq!(*p.storage.deleted.lock().unwrap(), ["generated/out.csv"]);
    }

    /// The call that would cross the quota keeps none of its files, says so, and
    /// still returns the result.
    #[tokio::test]
    async fn a_call_that_would_cross_the_quota_keeps_none_of_its_files() {
        use crate::tabular_run::refusal::{MIB, SESSION_MAX_BYTES};
        let p = prepared(&[("sales", 1)], 4).await;
        let exec = Recorder::ok_with_files(json!(3), &[("out.csv", b"12345")]);
        let ex = with_registry(
            generated_rows(1, SESSION_MAX_BYTES - 2),
            Arc::new(runtime(&p, exec, true)),
        );
        let out = body(&ex, r#"{"attachment_id":"doc-1","code":"pass"}"#).await;
        assert_eq!(out["result"], 3);
        assert!(out.get("emitted").is_none(), "{out}");
        assert!(
            out["not_kept"]
                .to_string()
                .contains("none of the 1 returned"),
            "{out}"
        );
        settle().await;
        assert_eq!(*p.storage.deleted.lock().unwrap(), ["generated/out.csv"]);
        let _ = MIB;
    }

    /// Registered and stored: kept, reported, and not deleted.
    #[tokio::test]
    async fn a_file_stored_and_registered_is_kept_and_reported() {
        let p = prepared(&[("sales", 1)], 4).await;
        let exec = Recorder::ok_with_files(json!(1), &[("out.csv", b"12345")]);
        let ex = with_registry(vec![], Arc::new(runtime(&p, exec, true)));
        let out = body(&ex, r#"{"attachment_id":"doc-1","code":"pass"}"#).await;
        assert_eq!(out["emitted"][0]["document_id"], "generated/out.csv");
        settle().await;
        assert!(p.storage.deleted.lock().unwrap().is_empty());
    }

    /// The cut-off answer says where the call was and is not retryable as it is; so
    /// are a timeout of the code and a setup failure of the executor.
    #[tokio::test]
    async fn the_cut_off_and_timeout_answers_carry_retryable_and_the_phase() {
        let p = prepared(&[("sales", 1)], 4).await;
        let exec = Recorder::ok(json!(1));
        *exec.delay.lock().unwrap() = Some(std::time::Duration::from_secs(30));
        let (ex, target, _e) = slow_target(&p, exec).await;
        let result = super::dispatch_bounded(
            &ex,
            "c1",
            &args(),
            "{}",
            target,
            std::time::Duration::from_secs(1),
            std::time::Duration::from_secs(2),
        )
        .await;
        let out: Value = serde_json::from_str(&result.output).unwrap();
        assert_eq!(out["retryable"], false);
        let text = out["error"].as_str().unwrap();
        assert!(
            text.contains("while staging the data, running the code"),
            "{text}"
        );
        assert!(text.contains("parts()"), "{text}");
        for (error, retryable) in [
            (
                crate::dag_engine::domain::python_executor::PythonRunError::Timeout,
                false,
            ),
            (
                crate::dag_engine::domain::python_executor::PythonRunError::Internal(
                    "PythonExecutorError: this executor has no staging directory configured".into(),
                ),
                false,
            ),
            (
                crate::dag_engine::domain::python_executor::PythonRunError::Internal(
                    "PythonExecutorError: connection reset".into(),
                ),
                true,
            ),
        ] {
            let p = prepared(&[("sales", 1)], 4).await;
            let ex = executor(
                true,
                Some(Arc::new(runtime(
                    &p,
                    Recorder::answering(Err(MountedError::Run(error))),
                    true,
                ))),
            );
            let out = body(&ex, r#"{"attachment_id":"doc-1","code":"pass"}"#).await;
            assert_eq!(out["retryable"], retryable, "{out}");
        }
    }

    // ---- the ledger: rows first, objects second, never an object whose row stays ----

    use crate::llm::domain::attachments::attachment_registry::MockAttachmentRegistry;
    use crate::llm::domain::attachments::AttachmentError;

    /// A registry that remembers upserts and deletes and can be told to fail either.
    #[derive(Clone, Default)]
    struct Book {
        rows: Arc<std::sync::Mutex<Vec<ConversationAttachment>>>,
        /// "row-removed:<key>" and "object-deleted-after-row:<bool>" in order.
        log: Arc<std::sync::Mutex<Vec<String>>>,
        fail_upsert_on: Arc<std::sync::Mutex<Option<usize>>>,
        fail_delete: Arc<std::sync::atomic::AtomicBool>,
        upserts: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl Book {
        fn registry(
            &self,
            deleted_objects: Arc<std::sync::Mutex<Vec<String>>>,
        ) -> MockAttachmentRegistry {
            let mut reg = MockAttachmentRegistry::new();
            let b = self.clone();
            reg.expect_list_for_session().returning(move |_| {
                let rows = b.rows.lock().unwrap().clone();
                Ok(rows)
            });
            let b = self.clone();
            reg.expect_upsert().returning(move |input| {
                let n = b.upserts.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                if *b.fail_upsert_on.lock().unwrap() == Some(n) {
                    return Err(AttachmentError::RepositoryFailed("down".into()));
                }
                b.rows.lock().unwrap().push(ConversationAttachment {
                    document_id: input.document_id,
                    origin: input.origin,
                    size_bytes: input.size_bytes,
                    provider: ProviderKind::Generated,
                    ..row(false)
                });
                Ok(())
            });
            let b = self.clone();
            reg.expect_lookup().returning(move |_, key, _| {
                Ok(b.rows
                    .lock()
                    .unwrap()
                    .iter()
                    .find(|r| r.document_id == key)
                    .cloned())
            });
            let b = self.clone();
            reg.expect_delete_attachment_for_provider()
                .returning(move |_, key, _| {
                    if b.fail_delete.load(std::sync::atomic::Ordering::SeqCst) {
                        return Err(AttachmentError::RepositoryFailed("down".into()));
                    }
                    // The object must still be there when its row goes: rows first.
                    let object_already_gone =
                        deleted_objects.lock().unwrap().iter().any(|k| k == key);
                    b.log.lock().unwrap().push(format!(
                        "row-removed:{key}:object-gone={object_already_gone}"
                    ));
                    b.rows.lock().unwrap().retain(|r| r.document_id != key);
                    Ok(())
                });
            reg
        }
    }

    fn with_book(book: &Book, p: &Prepared, exec: Arc<Recorder>) -> DagToolExecutor {
        executor(true, Some(Arc::new(runtime(p, exec, true))))
            .with_attachment_registry(Arc::new(book.registry(p.storage.deleted_handle())))
            .with_agent_session_id(Some("agent_1".into()))
    }

    /// The second registration fails: the first row is taken back FIRST, then both
    /// objects are deleted, and the answer keeps nothing.
    #[tokio::test]
    async fn a_failing_second_registration_removes_rows_first_then_objects() {
        let p = prepared(&[("sales", 1)], 4).await;
        let book = Book::default();
        *book.fail_upsert_on.lock().unwrap() = Some(2);
        let exec = Recorder::ok_with_files(json!(1), &[("a.csv", b"1"), ("b.csv", b"2")]);
        let out = body(
            &with_book(&book, &p, exec),
            r#"{"attachment_id":"doc-1","code":"pass"}"#,
        )
        .await;
        assert!(out.get("emitted").is_none(), "{out}");
        assert!(
            out["not_kept"].to_string().contains("none was kept"),
            "{out}"
        );
        settle().await;
        // b's upsert failed, but it may have committed: its row is removed too (a no-op
        // here), before either object.
        assert_eq!(
            *book.log.lock().unwrap(),
            [
                "row-removed:generated/b.csv:object-gone=false",
                "row-removed:generated/a.csv:object-gone=false"
            ]
        );
        let mut deleted = p.storage.deleted.lock().unwrap().clone();
        deleted.sort();
        assert_eq!(deleted, ["generated/a.csv", "generated/b.csv"]);
        assert!(book.rows.lock().unwrap().is_empty());
    }

    /// A row that cannot be taken back keeps its object: the pair stays whole and the
    /// answer reports it as kept, because it is.
    #[tokio::test]
    async fn a_row_that_cannot_be_removed_keeps_its_object_and_is_reported_kept() {
        let p = prepared(&[("sales", 1)], 4).await;
        let book = Book::default();
        *book.fail_upsert_on.lock().unwrap() = Some(2);
        book.fail_delete
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let exec = Recorder::ok_with_files(json!(1), &[("a.csv", b"1"), ("b.csv", b"2")]);
        let out = body(
            &with_book(&book, &p, exec),
            r#"{"attachment_id":"doc-1","code":"pass"}"#,
        )
        .await;
        settle().await;
        // a.csv: registered, row would not go -> row and object both stay, reported kept.
        assert_eq!(out["emitted"].as_array().unwrap().len(), 1, "{out}");
        assert_eq!(out["emitted"][0]["document_id"], "generated/a.csv");
        assert_eq!(
            *p.storage.deleted.lock().unwrap(),
            ["generated/b.csv"],
            "only the unregistered object goes"
        );
        assert_eq!(book.rows.lock().unwrap().len(), 1);
    }

    /// The tool future is dropped while the files are being registered: the ledger's
    /// Drop removes the row already made and then both objects, the same order.
    #[tokio::test]
    async fn dropping_the_ledger_between_registrations_undoes_rows_then_objects() {
        use crate::tabular_run::outputs::{OutputLedger, StoreSink};
        let p = prepared(&[("sales", 1)], 4).await;
        let book = Book::default();
        let deleted = p.storage.deleted_handle();
        let reg: Arc<dyn crate::llm::domain::AttachmentRegistry> = Arc::new(book.registry(deleted));
        // Two objects stored through the sink, then a ledger that has registered the first.
        let sink = StoreSink::new(p.storage.clone(), None, None);
        let dir = tempfile::tempdir().unwrap();
        for (n, b) in [("a.csv", b"1"), ("b.csv", b"2")] {
            std::fs::write(dir.path().join(n), b).unwrap();
        }
        let found =
            crate::tabular_run::collect::collect_out(dir.path(), Default::default()).unwrap();
        for f in found.files {
            crate::tabular_run::mounted::OutputSink::accept(&sink, f)
                .await
                .unwrap();
        }
        let (_, guard) = sink.take_guarded();
        let mut ledger: OutputLedger = guard.into_ledger(Some((reg, "agent_1".into())));
        book.rows.lock().unwrap().push(ConversationAttachment {
            document_id: "generated/a.csv".into(),
            ..row(false)
        });
        ledger.row_may_exist("generated/a.csv");
        drop(ledger);
        settle().await;
        assert_eq!(
            *book.log.lock().unwrap(),
            ["row-removed:generated/a.csv:object-gone=false"]
        );
        let mut deleted = p.storage.deleted.lock().unwrap().clone();
        deleted.sort();
        assert_eq!(deleted, ["generated/a.csv", "generated/b.csv"]);
        assert!(book.rows.lock().unwrap().is_empty());
    }

    /// A usage that cannot be read is not zero: the files are not kept.
    #[tokio::test]
    async fn an_unreadable_usage_keeps_no_file_and_still_returns_the_result() {
        let p = prepared(&[("sales", 1)], 4).await;
        let mut reg = MockAttachmentRegistry::new();
        reg.expect_list_for_session()
            .returning(|_| Err(AttachmentError::RepositoryFailed("down".into())));
        reg.expect_upsert().returning(|_| Ok(()));
        reg.expect_delete_attachment_for_provider()
            .returning(|_, _, _| Ok(()));
        let exec = Recorder::ok_with_files(json!(9), &[("out.csv", b"12345")]);
        let ex = executor(true, Some(Arc::new(runtime(&p, exec, true))))
            .with_attachment_registry(Arc::new(reg))
            .with_agent_session_id(Some("agent_1".into()));
        let out = body(&ex, r#"{"attachment_id":"doc-1","code":"pass"}"#).await;
        assert_eq!(out["result"], 9);
        assert!(out.get("emitted").is_none(), "{out}");
        assert!(
            out["not_kept"].to_string().contains("could not be checked"),
            "{out}"
        );
        settle().await;
        assert_eq!(*p.storage.deleted.lock().unwrap(), ["generated/out.csv"]);
    }

    /// Two calls of one conversation with room for ONE more file: exactly one keeps
    /// its file (the quota is counted and registered under the conversation's lock).
    #[tokio::test]
    async fn two_concurrent_calls_cannot_both_pass_the_quota() {
        use crate::tabular_run::refusal::SESSION_MAX_FILES;
        let p = prepared(&[("sales", 1)], 4).await;
        let book = Book::default();
        *book.rows.lock().unwrap() = generated_rows(SESSION_MAX_FILES - 1, 10);
        let mk = |tag: &str| {
            let exec = Recorder::ok_with_files(json!(1), &[(tag, b"x")]);
            with_book(&book, &p, exec)
        };
        let (a, b) = (mk("a.csv"), mk("b.csv"));
        let (ra, rb) = tokio::join!(
            body(&a, r#"{"attachment_id":"doc-1","code":"pass"}"#),
            body(&b, r#"{"attachment_id":"doc-1","code":"pass"}"#),
        );
        let kept = [&ra, &rb]
            .iter()
            .filter(|o| o.get("emitted").is_some())
            .count();
        assert_eq!(kept, 1, "{ra} / {rb}");
        assert_eq!(book.rows.lock().unwrap().len(), SESSION_MAX_FILES);
    }
}
