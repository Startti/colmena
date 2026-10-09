//! `attachment_run_python` over a large file: the tool's answer for a call the
//! routing handed to the large path (dark behind `COLMENA_LARGE_TABULAR`).
//!
//! The answer carries `tables` (names, rows, column types) where the small path
//! carries `row_count` and `columns`. Every refusal is the typed error object of
//! [`RunRefusal`](crate::tabular_run::refusal::RunRefusal): a sentence and a code,
//! no key, no path, no adapter text.

use super::{err_envelope, truncate, AttachmentRunPythonArgs, OUTPUT_BYTE_CAP};
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
    // The conversation's quota is checked before anything runs.
    let usage = executor
        .generated_usage(super::ATTACHMENT_RUN_PYTHON_TOOL_NAME)
        .await;
    if let Some((files, bytes)) = usage {
        if files >= SESSION_MAX_FILES || bytes >= SESSION_MAX_BYTES {
            return ToolResult::success(
                call_id.to_string(),
                RunRefusal::SessionQuota.to_tool_error().to_string(),
            );
        }
    }
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
    let outcome = match executor.with_progress_ticker(tick, run).await {
        Ok(outcome) => outcome,
        // The call's future was dropped: the request is closed, the child killed
        // and the volume given back by their own drop guards.
        Err(_) => {
            return err_envelope(
                call_id,
                format!(
                "the large-file call did not finish within {}s and was stopped; nothing was kept",
                budget.as_secs()
            ),
            )
        }
    };
    let duration_ms = started.elapsed().as_millis() as u64;
    match outcome {
        Ok(out) => {
            let (result, result_note) = cap_result(out.result);
            let mut not_kept = out.not_kept;
            let mut emitted = vec![];
            let (used_files, used_bytes) = usage.unwrap_or((0, 0));
            let new_bytes: u64 = out.emitted.iter().map(|f| f.size_bytes).sum();
            let over_quota = used_files + out.emitted.len() > SESSION_MAX_FILES
                || used_bytes + new_bytes > SESSION_MAX_BYTES;
            if over_quota {
                // All or nothing: the guard deletes what was stored.
                not_kept.push(format!(
                    "none of the {} returned file(s) was kept: {}",
                    out.emitted.len(),
                    RunRefusal::SessionQuota.message()
                ));
            } else {
                // Each returned file becomes an attachment of the session, through
                // the path every generated file takes (engine-owned, not a host
                // reference). Nothing is reported as kept unless it is stored AND
                // registered; one failure takes back the others.
                let mut registered: Vec<&str> = vec![];
                let mut failed = false;
                for f in &out.emitted {
                    let done = executor
                        .register_stored_attachment(
                            &f.storage_key,
                            &f.mime_type,
                            &f.name,
                            f.size_bytes,
                            super::ATTACHMENT_RUN_PYTHON_TOOL_NAME,
                        )
                        .await;
                    if done.is_err() {
                        failed = true;
                        break;
                    }
                    registered.push(&f.storage_key);
                }
                if failed {
                    for key in registered {
                        executor.unregister_stored_attachment(key).await;
                    }
                    not_kept.push(
                        "the returned files could not be saved to this conversation, so none was kept".to_string(),
                    );
                } else {
                    for f in out.emitted {
                        emitted.push(EmittedFile {
                            name: f.name,
                            mime_type: f.mime_type,
                            size_bytes: f.size_bytes,
                            document_id: f.storage_key,
                            rows_reported_by_code: f.rows,
                            dtypes_reported_by_code: f.dtypes,
                        });
                    }
                    out.guard.commit();
                }
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
        Err(LargeRunError::Timeout { secs }) => retryable_error(
            call_id,
            format!("code execution exceeded {secs}s timeout"),
            true,
        ),
        Err(LargeRunError::Internal) => retryable_error(
            call_id,
            "the large-file executor failed; try again, and if it keeps failing the file cannot be analysed here".to_string(),
            true,
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
    async fn a_conversation_at_its_file_quota_is_refused_before_anything_runs() {
        use crate::tabular_run::refusal::SESSION_MAX_FILES;
        let p = prepared(&[("sales", 1)], 4).await;
        let exec = Recorder::ok(json!(1));
        let ex = with_registry(
            generated_rows(SESSION_MAX_FILES, 10),
            Arc::new(runtime(&p, exec.clone(), true)),
        );
        let out = body(&ex, r#"{"attachment_id":"doc-1","code":"pass"}"#).await;
        assert_eq!(out["code"], "large_tabular_quota");
        assert_eq!(out["retryable"], false);
        assert_eq!(exec.calls(), 0);
        // One under the quota still runs.
        let exec = Recorder::ok(json!(1));
        let ex = with_registry(
            generated_rows(SESSION_MAX_FILES - 1, 10),
            Arc::new(runtime(&p, exec.clone(), true)),
        );
        let out = body(&ex, r#"{"attachment_id":"doc-1","code":"pass"}"#).await;
        assert_eq!(out["result"], 1);
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
}
