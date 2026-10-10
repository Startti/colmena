//! `attachment_run_python` over a large file: the tool's answer for a call the
//! routing handed to the large path (dark behind `COLMENA_LARGE_TABULAR`).
//!
//! The answer carries `tables` (names, rows, column types) where the small path
//! carries `row_count` and `columns`. Every refusal is the typed error object of
//! [`RunRefusal`](crate::tabular_run::refusal::RunRefusal): a sentence and a code,
//! no key, no path, no adapter text.

use super::{truncate, AttachmentRunPythonArgs, OUTPUT_BYTE_CAP};
use crate::dag_engine::infrastructure::dag_tool_executor::LargeTarget;
use crate::llm::domain::large_tabular::LargeTool;
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

/// What `data_run_python` says about large files: the same helpers as
/// [`LARGE_FILES_TEXT`], reached through its own argument shape.
pub(crate) const DATA_RUN_LARGE_FILES_TEXT: &str = "\n\nLarge files (over 50 MiB): pass ONE binding that names the file, \
`bindings: [{\"var\": \"big\", \"attachment_id\": \"<id>\"}]` (`var` is not used and the file cannot be combined with other bindings). \
The records variable is NOT loaded. Use `tables`:\n\
- `tables.names`, `tables.schema(name)`: tables, columns, types, row counts.\n\
- `t = tables[name]` is a handle, not a DataFrame.\n\
- `t.read(columns=[...], filters=[...])`: load only the columns you need.\n\
- `for part in t.parts(columns=[...]):` up to 500,000 rows per part; aggregate\n  each part and combine. Use this for anything that touches every row.\n\
- `t.head()` to look at a few rows.\n\
A whole table cannot be loaded at once. Runs may take up to 5 minutes.\n\
Set `output`: it is your answer, and the call returns it as `result` (`result` is used only when `output` is not set), with `tables`, `stdout` and the files you returned.\n\
To return a file, call `emit_table(df_or_parts, \"name\", \"csv\" | \"parquet\")` (up to 8 files; parquet takes one DataFrame).\n\
`code_ref`, `output_tables`, `output_sheets` and `output_attachments` are not available over a large file.\n\
No charts or images: return aggregated numbers and build charts from them.\n\
The optional `tables` argument names the tables to make readable (default: all).";

/// The origin tag of the files the large path of `data_run_python` returns. It is its own
/// tag, not `data_run_python`'s: that one is also written by the small path's
/// `output_attachments` sink, which has no quota and takes no lock.
pub(crate) const DATA_RUN_LARGE_ORIGIN: &str = "data_run_python_large";

/// The origin the files a call through `tool` return carry.
fn output_origin(tool: LargeTool) -> &'static str {
    match tool {
        LargeTool::AttachmentRunPython => super::ATTACHMENT_RUN_PYTHON_TOOL_NAME,
        LargeTool::DataRunPython => DATA_RUN_LARGE_ORIGIN,
    }
}

/// The origins the large-path quota counts: ONE budget per conversation shared by both
/// tools. Files the small path of `data_run_python` registered are not counted, and do
/// not count against anyone: they have their own, older, behaviour.
const QUOTA_ORIGINS: [&str; 2] = [
    super::ATTACHMENT_RUN_PYTHON_TOOL_NAME,
    DATA_RUN_LARGE_ORIGIN,
];

/// The two clocks of a call: how often it shows it is alive, and how long it may take.
#[derive(Clone, Copy)]
pub(super) struct CallClock {
    pub every: std::time::Duration,
    pub budget: std::time::Duration,
}

/// `data_run_python` as the model sees it when a large file is served: the usual
/// definition (its sources and gating unchanged), the text above and the `tables`
/// argument.
pub(crate) fn data_run_python_definition(
    usual: crate::llm::domain::tools::ToolDefinition,
) -> crate::llm::domain::tools::ToolDefinition {
    let mut def = usual;
    def.description.push_str(DATA_RUN_LARGE_FILES_TEXT);
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

/// The typed refusal of a `data_run_python` call the large route cannot serve: a sentence
/// the model can act on, `retryable: false`, and nothing was run.
fn refusal_value(message: &str) -> serde_json::Value {
    serde_json::json!({
        "error": message,
        "retryable": false,
        "source": "execution",
        "code": crate::llm::domain::large_tabular::LARGE_TABULAR_ERROR_CODE,
    })
}

/// A field the model filled in with nothing: absent, null, an empty or blank string, an
/// empty list or object, `false`. It asks for nothing, so it is not "set".
fn is_blank(v: &serde_json::Value) -> bool {
    match v {
        serde_json::Value::Null => true,
        serde_json::Value::Bool(b) => !b,
        serde_json::Value::String(s) => s.trim().is_empty(),
        serde_json::Value::Array(a) => a.is_empty(),
        serde_json::Value::Object(o) => o.is_empty(),
        serde_json::Value::Number(_) => false,
    }
}

fn text_set(v: &Option<String>) -> bool {
    v.as_deref().is_some_and(|s| !s.trim().is_empty())
}

/// What a call to `data_run_python` over a large file asks for that the large route
/// cannot do. Everything here is known before anything runs, so it is refused before.
fn unsupported_by_the_large_route(
    parsed: &super::super::data_run_python::DataRunPythonArgs,
    raw: &serde_json::Value,
) -> Option<String> {
    if parsed.bindings.len() != 1 {
        return Some(
            "a large file is analysed on its own: call `data_run_python` with only that file \
             in `bindings` and read it through `tables`"
                .into(),
        );
    }
    let b = &parsed.bindings[0];
    // 0 is the first row, the default: it asks for nothing.
    let extra = [
        ("spreadsheet_id", text_set(&b.spreadsheet_id)),
        ("sheet", text_set(&b.sheet)),
        ("range", text_set(&b.range)),
        ("query", text_set(&b.query)),
        ("data", b.data.as_ref().is_some_and(|d| !is_blank(d))),
        ("delimiter", text_set(&b.delimiter)),
        ("sheet_name", text_set(&b.sheet_name)),
        ("header_row", b.header_row.is_some_and(|n| n != 0)),
    ]
    .into_iter()
    .find_map(|(field, set)| set.then_some(field));
    if let Some(field) = extra {
        return Some(format!(
            "a binding for a large file takes only `var` and `attachment_id`: `{field}` is not \
             available over a large file (name the table with the `tables` argument)"
        ));
    }
    for field in ["write_to_spreadsheet", "on_existing_sheet"] {
        if raw.get(field).is_some_and(|v| !is_blank(v)) {
            return Some(format!(
                "`{field}` is not available over a large file: nothing can be written to a \
                 spreadsheet from it; return files with `emit_table`"
            ));
        }
    }
    let code = parsed.code.as_deref().is_some_and(|c| !c.trim().is_empty());
    let code_ref = parsed
        .code_ref
        .as_deref()
        .is_some_and(|c| !c.trim().is_empty());
    match (code, code_ref) {
        (_, true) => {
            Some("`code_ref` is not available over a large file: pass the Python in `code`".into())
        }
        (false, false) => Some("`code` is required: pass the Python to run in `code`".into()),
        (true, false) => None,
    }
}

/// A `data_run_python` call whose bindings name a large host-owned file runs over
/// the file's prepared tables, exactly as `attachment_run_python` does. `None` only when
/// no binding names such a file: the call is then handled as it always was, untouched.
/// Once this has decided the call is a large-file call it ALWAYS answers (a refusal
/// before anything runs, or the run's answer, or a typed internal error), so a run that
/// started is never followed by the small path running the code again.
pub(crate) async fn dispatch_from_data_run_python(
    executor: &crate::dag_engine::infrastructure::dag_tool_executor::DagToolExecutor,
    call_id: &str,
    args: &serde_json::Value,
) -> Option<serde_json::Value> {
    use crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::data_run_python::DataRunPythonArgs;
    // `var` names the Python variable of a normal binding and is not used over a large
    // file, so a binding for one that omits it is accepted (the small path would answer
    // "missing field `var`" and the call would never reach the large route).
    let mut args = args.clone();
    if let Some(bindings) = args.get_mut("bindings").and_then(|b| b.as_array_mut()) {
        for b in bindings {
            let Some(o) = b.as_object_mut() else { continue };
            let named = ["var", "binding_name", "name"]
                .iter()
                .any(|k| o.contains_key(*k));
            let large = o
                .get("attachment_id")
                .and_then(|v| v.as_str())
                .is_some_and(|id| {
                    executor
                        .large_target_for(LargeTool::DataRunPython, id)
                        .is_some()
                });
            if !named && large {
                o.insert("var".into(), "big".into());
            }
        }
    }
    let args = &args;
    let parsed: DataRunPythonArgs = serde_json::from_value(args.clone()).ok()?;
    let mut targets = parsed
        .bindings
        .iter()
        .filter_map(|b| b.attachment_id.as_deref())
        .filter_map(|id| {
            executor
                .large_target_for(LargeTool::DataRunPython, id)
                .map(|t| (id.to_string(), t))
        });
    let (attachment_id, target) = targets.next()?;
    // Two bindings are refused by the same rule, whatever they name.
    if let Some(why) = unsupported_by_the_large_route(&parsed, args) {
        return Some(refusal_value(&why));
    }
    let large_args = AttachmentRunPythonArgs {
        attachment_id,
        code: parsed.code.clone().unwrap_or_default(),
        delimiter: None,
        sheet_name: None,
        header_row: None,
    };
    let clock = CallClock {
        every: std::time::Duration::from_secs(
            crate::dag_engine::infrastructure::dag_tool_executor::TOOL_PROGRESS_INTERVAL_SECS,
        ),
        budget: executor.large_call_budget().unwrap_or(CALL_BUDGET),
    };
    let raw = args.to_string();
    let result = dispatch_bounded(
        executor,
        call_id,
        &large_args,
        &raw,
        target,
        clock,
        LargeTool::DataRunPython,
    )
    .await;
    Some(serde_json::from_str(&result.output).unwrap_or_else(|_| {
        serde_json::json!({
            "error": "the large-file analysis ended without a readable answer; its files, if any, \
                      were handled as usual. Do not repeat the call without changing it",
            "retryable": false,
            "source": "execution",
        })
    }))
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
    /// Sheets of the workbook that are not tables, and why; absent when none.
    #[serde(skip_serializing_if = "serde_json::Value::is_null")]
    skipped_sheets: serde_json::Value,
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
    let clock = CallClock {
        every,
        budget: executor.large_call_budget().unwrap_or(CALL_BUDGET),
    };
    dispatch_bounded(
        executor,
        call_id,
        args,
        raw_arguments,
        target,
        clock,
        LargeTool::AttachmentRunPython,
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
    origin: &str,
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
            Ok(_held) => register_all(executor, origin, files, &mut ledger).await.err(),
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
    origin: &str,
    files: &[crate::tabular_run::runtime::EmittedOutput],
    ledger: &mut crate::tabular_run::outputs::OutputLedger,
) -> Result<(), Stopped> {
    let usage = tokio::time::timeout(REGISTRY_STEP, executor.generated_usage(&QUOTA_ORIGINS)).await;
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
                origin,
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
    skipped: serde_json::Value,
    emitted: Vec<crate::tabular_run::runtime::EmittedOutput>,
    not_kept: Vec<String>,
    keeping: Keeping,
}

/// [`dispatch`] with the clocks given, so a test can run it in seconds. `tool` is the
/// tool the model called: it decides which convention the code may use for its answer
/// and which origin the returned files carry.
async fn dispatch_bounded(
    executor: &crate::dag_engine::infrastructure::dag_tool_executor::DagToolExecutor,
    call_id: &str,
    args: &AttachmentRunPythonArgs,
    raw_arguments: &str,
    target: LargeTarget,
    clock: CallClock,
    tool: LargeTool,
) -> ToolResult {
    let (every, budget) = (clock.every, clock.budget);
    let origin = output_origin(tool);
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
    // Keeping the returned files is part of the call, so it runs under the call's
    // clock; cut off, the ledger dropped inside undoes what it did.
    let work = async {
        // A conversation already at its limit is known before the code runs: the call
        // still runs, its files are not stored (nothing is uploaded to be deleted) and
        // the answer says so. A usage that cannot be read here is left to `keep_outputs`.
        let keep_files =
            match tokio::time::timeout(REGISTRY_STEP, executor.generated_usage(&QUOTA_ORIGINS))
                .await
            {
                Ok(Some((files, bytes))) => files < SESSION_MAX_FILES && bytes < SESSION_MAX_BYTES,
                _ => true,
            };
        let out = target
            .runtime
            .run(LargeRunRequest {
                source_key: target.source_key,
                mime_type: target.mime_type,
                filename: target.filename,
                size_bytes: target.size_bytes,
                code: args.code.clone(),
                tables: requested_tables(raw_arguments),
                session_id: target.session_id,
                agent_session_id: target.agent_session_id,
                phase: phase.clone(),
                keep_files,
                accept_output: tool == LargeTool::DataRunPython,
            })
            .await?;
        let crate::tabular_run::runtime::LargeRunOutput {
            stdout,
            result,
            tables,
            skipped,
            emitted,
            not_kept,
            guard,
        } = out;
        let ledger = guard.into_ledger(executor.registry_handle());
        let keeping = keep_outputs(executor, origin, &emitted, ledger).await;
        Ok(Finished {
            stdout,
            result,
            tables,
            skipped,
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
                    skipped_sheets: out.skipped,
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
                skipped_sheets: serde_json::Value::Null,
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
    use crate::llm::domain::large_tabular::LargeTool;
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

    /// Defence in depth below the agent loop: the executor serves the large path per
    /// tool, so a call that reaches it by another route, naming a tool the node does not
    /// offer, is not run. The sandbox runs for the tool the node serves and for no other.
    #[tokio::test]
    async fn the_large_path_runs_only_for_the_tool_the_node_serves() {
        use crate::llm::domain::large_tabular::LargeServed;
        let p = prepared(&[("sales", 1)], 4).await;
        let drp_args = json!({
            "bindings": [{"var": "big", "attachment_id": "doc-1"}], "code": "output = 1"
        });
        // Only data_run_python is served: attachment_run_python is not run.
        let exec = Recorder::ok(json!({"ran": true}));
        let ex = executor(true, Some(Arc::new(runtime(&p, exec.clone(), true))))
            .with_large_served(LargeServed::decide(true, true, false, true));
        let out = body(&ex, r#"{"attachment_id":"doc-1","code":"result = 1"}"#).await;
        assert_eq!(exec.calls(), 0, "{out}");
        assert!(super::dispatch_from_data_run_python(&ex, "c", &drp_args)
            .await
            .is_some());
        assert_eq!(exec.calls(), 1);
        // Only attachment_run_python is served: data_run_python is left to its own path.
        let exec = Recorder::ok(json!({"ran": true}));
        let ex = executor(true, Some(Arc::new(runtime(&p, exec.clone(), true))))
            .with_large_served(LargeServed::decide(true, true, true, false));
        assert!(super::dispatch_from_data_run_python(&ex, "c", &drp_args)
            .await
            .is_none());
        assert_eq!(exec.calls(), 0);
        let out = body(&ex, r#"{"attachment_id":"doc-1","code":"result = 1"}"#).await;
        assert_eq!(out["result"]["ran"], true, "{out}");
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
        let target = ex
            .large_target_for(
                crate::llm::domain::large_tabular::LargeTool::AttachmentRunPython,
                "doc-1",
            )
            .expect("routed");
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
            super::CallClock {
                every: std::time::Duration::from_secs(1),
                budget: std::time::Duration::from_secs(60),
            },
            LargeTool::AttachmentRunPython,
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
            super::CallClock {
                every: std::time::Duration::from_secs(1),
                budget: std::time::Duration::from_secs(2),
            },
            LargeTool::AttachmentRunPython,
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

    fn retagged(rows: Vec<ConversationAttachment>, tag: &str) -> Vec<ConversationAttachment> {
        rows.into_iter()
            .map(|r| ConversationAttachment {
                origin: Some(origin::generated_by(tag)),
                ..r
            })
            .collect()
    }

    /// An executor that serves `data_run_python` over a prepared file, and the calls it ran.
    async fn drp_executor(exec: &Arc<Recorder>) -> (DagToolExecutor, Prepared) {
        use crate::llm::domain::large_tabular::LargeServed;
        let p = prepared(&[("sales", 1)], 4).await;
        let ex = executor(true, Some(Arc::new(runtime(&p, exec.clone(), true))))
            .with_large_served(LargeServed::decide(true, true, false, true));
        (ex, p)
    }

    fn drp_args(extra: serde_json::Value) -> serde_json::Value {
        let mut args = json!({
            "bindings": [{"var": "big", "attachment_id": "doc-1"}],
            "code": "output = 1"
        });
        for (k, v) in extra.as_object().into_iter().flatten() {
            args[k] = v.clone();
        }
        args
    }

    /// The full executor route by name (`ToolExecutor::execute`), past the agent loop's
    /// "offered" check: the per-tool serving set is what stops a tool the node does not
    /// offer from reaching the sandbox. Nothing runs for the unserved tool, whichever tool
    /// it is; the served one runs.
    #[tokio::test]
    async fn the_executor_route_by_name_runs_only_the_served_tool() {
        use crate::llm::domain::large_tabular::LargeServed;
        use crate::llm::domain::ToolExecutor;
        async fn drp(ex: &DagToolExecutor) -> String {
            let call = ToolCall {
                function: FunctionCall::new(
                    "data_run_python".into(),
                    drp_args(json!({})).to_string(),
                ),
                ..call("{}")
            };
            ex.execute(&call).await.unwrap().output
        }
        async fn arp(ex: &DagToolExecutor) -> String {
            ex.execute(&call(r#"{"attachment_id":"doc-1","code":"result = 1"}"#))
                .await
                .unwrap()
                .output
        }
        let p = prepared(&[("sales", 1)], 4).await;
        // Only data_run_python is served: attachment_run_python by name does not run.
        let exec = Recorder::ok(json!({"ran": true}));
        let ex = executor(true, Some(Arc::new(runtime(&p, exec.clone(), true))))
            .with_large_served(LargeServed::decide(true, true, false, true));
        let refused = arp(&ex).await;
        assert_eq!(exec.calls(), 0, "{refused}");
        // ... and data_run_python by name does.
        let out = drp(&ex).await;
        assert!(out.contains("\"ran\":true"), "{out}");
        assert_eq!(exec.calls(), 1);
        // Only attachment_run_python is served: data_run_python by name does not run.
        let exec = Recorder::ok(json!({"ran": true}));
        let ex = executor(true, Some(Arc::new(runtime(&p, exec.clone(), true))))
            .with_large_served(LargeServed::decide(true, true, true, false));
        let _ = drp(&ex).await;
        assert_eq!(exec.calls(), 0);
    }

    /// The two builders give the same answer in either order: a node that serves nothing
    /// (or only one tool) is not made to serve `attachment_run_python` by the runtime
    /// being wired after, or before, it said so.
    #[tokio::test]
    async fn the_serving_set_does_not_depend_on_the_order_of_the_builders() {
        use crate::llm::domain::large_tabular::{LargeServed, LargeTool};
        let p = prepared(&[("sales", 1)], 4).await;
        let rt = Arc::new(runtime(&p, Recorder::ok(json!(1)), true));
        for served in [
            LargeServed::default(),
            LargeServed::decide(true, true, false, true),
            LargeServed::decide(true, true, true, false),
        ] {
            let plain = || {
                DagToolExecutor::new(Arc::new(NoNodes), Default::default())
                    .with_attachments(vec![row(true)])
            };
            let a = plain()
                .with_large_tabular(rt.clone())
                .with_large_served(served);
            let b = plain()
                .with_large_served(served)
                .with_large_tabular(rt.clone());
            for tool in [LargeTool::AttachmentRunPython, LargeTool::DataRunPython] {
                assert_eq!(
                    a.large_target_for(tool, "doc-1").is_some(),
                    b.large_target_for(tool, "doc-1").is_some(),
                    "{served:?} {tool:?}"
                );
                assert_eq!(
                    a.large_target_for(tool, "doc-1").is_some(),
                    served.serves(tool)
                );
            }
        }
        // Said nothing: a wired runtime serves the tool it was first wired for.
        let ex = DagToolExecutor::new(Arc::new(NoNodes), Default::default())
            .with_attachments(vec![row(true)])
            .with_large_tabular(rt);
        assert!(ex
            .large_target_for(LargeTool::AttachmentRunPython, "doc-1")
            .is_some());
        assert!(ex
            .large_target_for(LargeTool::DataRunPython, "doc-1")
            .is_none());
    }

    /// Fields filled in with nothing ask for nothing: they do not turn a good call into a
    /// refusal, and a binding without `var` is accepted (the docs say it is not used).
    #[tokio::test]
    async fn empty_values_and_a_missing_var_do_not_refuse_a_large_call() {
        let exec = Recorder::ok(json!(1));
        let (ex, _p) = drp_executor(&exec).await;
        let args = json!({
            "bindings": [{
                "attachment_id": "doc-1", "delimiter": "", "sheet": " ", "query": "",
                "data": [], "sheet_name": "", "header_row": 0, "range": null
            }],
            "code": "output = 1",
            "write_to_spreadsheet": "",
            "on_existing_sheet": "",
        });
        let out = super::dispatch_from_data_run_python(&ex, "c", &args)
            .await
            .unwrap();
        assert!(out.get("error").is_none(), "{out}");
        assert_eq!(exec.calls(), 1);
        // A real request next to the empty ones is still refused.
        let mut real = args.clone();
        real["bindings"][0]["query"] = json!("SELECT 1");
        let out = super::dispatch_from_data_run_python(&ex, "c", &real)
            .await
            .unwrap();
        assert!(out["error"].as_str().unwrap().contains("`query`"), "{out}");
        assert_eq!(exec.calls(), 1);
    }

    /// Everything the large route cannot do is refused BEFORE the code runs: a typed
    /// refusal, `retryable: false`, the code of every large-file refusal, nothing executed.
    #[tokio::test]
    async fn what_the_large_route_cannot_do_is_refused_before_anything_runs() {
        let exec = Recorder::ok(json!(1));
        let (ex, _p) = drp_executor(&exec).await;
        let with_binding = |field: &str, value: serde_json::Value| {
            let mut a = drp_args(json!({}));
            a["bindings"][0][field] = value;
            a
        };
        let cases = [
            (
                drp_args(json!({"write_to_spreadsheet": "sheet-id"})),
                "write_to_spreadsheet",
            ),
            (
                drp_args(json!({"on_existing_sheet": "overwrite"})),
                "on_existing_sheet",
            ),
            (with_binding("data", json!([{"a": 1}])), "`data`"),
            (with_binding("query", json!("SELECT 1")), "`query`"),
            (
                with_binding("spreadsheet_id", json!("s")),
                "`spreadsheet_id`",
            ),
            (with_binding("sheet", json!("tab")), "`sheet`"),
            (with_binding("range", json!("A1:B2")), "`range`"),
            (with_binding("sheet_name", json!("Sheet1")), "`sheet_name`"),
            (with_binding("delimiter", json!(";")), "`delimiter`"),
            (with_binding("header_row", json!(2)), "`header_row`"),
            (
                drp_args(json!({"code_ref": "k"})),
                "`code_ref` is not available",
            ),
            (
                json!({"bindings": [{"var": "big", "attachment_id": "doc-1"}]}),
                "`code` is required",
            ),
        ];
        for (args, expect) in cases {
            let out = super::dispatch_from_data_run_python(&ex, "c", &args)
                .await
                .expect("a large-file call is always answered");
            let text = out["error"].as_str().unwrap_or_default();
            assert!(text.contains(expect), "{expect}: {out}");
            assert_eq!(out["retryable"], false, "{expect}: {out}");
            assert_eq!(out["code"], "large_tabular_file", "{expect}: {out}");
        }
        assert_eq!(exec.calls(), 0, "nothing was executed");
        // The same call without those extras runs.
        assert!(
            super::dispatch_from_data_run_python(&ex, "c", &drp_args(json!({})))
                .await
                .is_some()
        );
        assert_eq!(exec.calls(), 1);
    }

    /// The write sinks are only knowable after the run: the answer says the code SET them
    /// and that nothing was written.
    #[tokio::test]
    async fn a_write_sink_the_code_set_is_reported_as_not_written() {
        let canned = json!({
            "__colmena_emitted": [],
            "__colmena_unwritten": ["output_sheets", "output_tables", "not_a_sink"],
            "result": 7
        });
        let exec = Recorder::ok(canned);
        let (ex, _p) = drp_executor(&exec).await;
        let out = super::dispatch_from_data_run_python(&ex, "c", &drp_args(json!({})))
            .await
            .unwrap();
        assert_eq!(out["result"], 7, "{out}");
        let why = out["not_kept"].to_string();
        assert!(
            why.contains("`output_sheets` was set, but nothing was written"),
            "{why}"
        );
        assert!(
            why.contains("`output_tables` was set, but nothing was written"),
            "{why}"
        );
        assert!(
            !why.contains("not_a_sink"),
            "only the three sinks are ever named: {why}"
        );
    }

    /// ONE budget per conversation, shared by both tools: files either tool returned count
    /// for the other.
    #[tokio::test]
    async fn the_quota_is_one_budget_shared_by_both_tools() {
        use crate::tabular_run::refusal::SESSION_MAX_FILES;
        let p = prepared(&[("sales", 1)], 4).await;
        for (held_by, called_through_drp) in [
            ("attachment_run_python", true),
            ("data_run_python_large", false),
        ] {
            let exec = Recorder::ok_with_files(json!(1), &[("out.csv", b"1")]);
            let rows = retagged(generated_rows(SESSION_MAX_FILES, 10), held_by);
            let ex = with_registry(rows, Arc::new(runtime(&p, exec.clone(), true)))
                .with_large_served(crate::llm::domain::large_tabular::LargeServed::decide(
                    true, true, true, true,
                ));
            let out = if called_through_drp {
                super::dispatch_from_data_run_python(&ex, "c", &drp_args(json!({})))
                    .await
                    .unwrap()
            } else {
                body(&ex, r#"{"attachment_id":"doc-1","code":"result = 1"}"#).await
            };
            assert!(out.get("emitted").is_none(), "{held_by}: {out}");
            assert!(
                out["not_kept"]
                    .to_string()
                    .contains("return results in the answer"),
                "{held_by}: {out}"
            );
        }
    }

    /// The other direction: what the small path of `data_run_python` registered neither
    /// counts against the large budget nor is counted by it.
    #[tokio::test]
    async fn files_of_the_small_sink_do_not_eat_the_large_budget() {
        use crate::tabular_run::refusal::SESSION_MAX_FILES;
        let exec = Recorder::ok_with_files(json!(1), &[("out.csv", b"1")]);
        let p = prepared(&[("sales", 1)], 4).await;
        let rows = retagged(generated_rows(SESSION_MAX_FILES, 10), "data_run_python");
        let ex = with_registry(rows, Arc::new(runtime(&p, exec.clone(), true))).with_large_served(
            crate::llm::domain::large_tabular::LargeServed::decide(true, true, false, true),
        );
        let out = super::dispatch_from_data_run_python(&ex, "c", &drp_args(json!({})))
            .await
            .unwrap();
        assert_eq!(out["emitted"].as_array().map(|a| a.len()), Some(1), "{out}");
    }

    /// A conversation that already holds the most this tool may return gets a
    /// typed refusal before anything runs.
    #[tokio::test]
    async fn a_conversation_at_its_file_quota_still_gets_its_result_but_keeps_no_file() {
        use crate::tabular_run::refusal::SESSION_MAX_FILES;
        let p = prepared(&[("sales", 1)], 4).await;
        // The conversation is at its limit: the call runs and returns its result, and
        // the answer says up front that no file of this call is kept.
        let exec = Recorder::ok(json!(1));
        let ex = with_registry(
            generated_rows(SESSION_MAX_FILES, 10),
            Arc::new(runtime(&p, exec.clone(), true)),
        );
        let out = body(&ex, r#"{"attachment_id":"doc-1","code":"pass"}"#).await;
        assert_eq!(out["result"], 1, "{out}");
        assert!(out.get("code").is_none(), "{out}");
        assert!(
            out["not_kept"]
                .to_string()
                .contains("return results in the answer"),
            "{out}"
        );
        assert_eq!(exec.calls(), 1);
        // Files the code returns are not even stored: nothing is uploaded to be deleted.
        let exec = Recorder::ok_with_files(json!(2), &[("out.csv", b"12345")]);
        let ex = with_registry(
            generated_rows(SESSION_MAX_FILES, 10),
            Arc::new(runtime(&p, exec, true)),
        );
        let out = body(&ex, r#"{"attachment_id":"doc-1","code":"pass"}"#).await;
        assert_eq!(out["result"], 2);
        assert!(out.get("emitted").is_none(), "{out}");
        assert!(p.storage.stored.lock().unwrap().is_empty());
        assert!(p.storage.deleted.lock().unwrap().is_empty());
    }

    /// A call that only CROSSES the limit is told it would exceed it, with the
    /// numbers; one already at the limit is told the limit is reached.
    #[tokio::test]
    async fn the_quota_message_says_would_exceed_when_only_this_call_crosses_it() {
        use crate::tabular_run::refusal::SESSION_MAX_FILES;
        let p = prepared(&[("sales", 1)], 4).await;
        let exec = Recorder::ok_with_files(json!(1), &[("a.csv", b"1"), ("b.csv", b"2")]);
        let ex = with_registry(
            generated_rows(SESSION_MAX_FILES - 1, 10),
            Arc::new(runtime(&p, exec, true)),
        );
        let out = body(&ex, r#"{"attachment_id":"doc-1","code":"pass"}"#).await;
        assert!(out.get("emitted").is_none(), "{out}");
        let why = out["not_kept"].to_string();
        assert!(
            why.contains("would exceed") && why.contains("files kept so far"),
            "{why}"
        );
        assert!(!why.contains("already holds"), "{why}");
    }

    /// The quota counts the files THIS tool returned (its origin tag) and nothing else:
    /// the small path registers no generated attachment, and other tools' files carry
    /// their own tags.
    #[tokio::test]
    async fn the_quota_counts_only_the_files_this_tool_returned() {
        use crate::tabular_run::refusal::SESSION_MAX_FILES;
        let p = prepared(&[("sales", 1)], 4).await;
        let mut rows = generated_rows(SESSION_MAX_FILES, 10);
        for r in &mut rows {
            r.origin = Some(origin::generated_by("data_run_python"));
        }
        let exec = Recorder::ok_with_files(json!(1), &[("out.csv", b"1")]);
        let ex = with_registry(rows, Arc::new(runtime(&p, exec, true)));
        let out = body(&ex, r#"{"attachment_id":"doc-1","code":"pass"}"#).await;
        assert_eq!(out["emitted"].as_array().map(|a| a.len()), Some(1), "{out}");
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
            super::CallClock {
                every: std::time::Duration::from_secs(1),
                budget: std::time::Duration::from_secs(2),
            },
            LargeTool::AttachmentRunPython,
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

    /// How the registry misbehaves in an interleaving test.
    #[derive(Clone, Copy, PartialEq)]
    enum Fault {
        /// The nth upsert commits, then reports an error.
        UpsertCommitsThenErrs(usize),
        /// The nth upsert commits, then never answers.
        UpsertCommitsThenHangs(usize),
        /// Every row delete commits, then reports an error.
        DeleteCommitsThenErrs,
        /// The first row delete never answers (the next ones work).
        FirstDeleteHangs,
        /// Every row delete never answers, and the row stays.
        DeleteHangs,
    }

    /// The book's registry, with one fault.
    struct Faulty {
        inner: Arc<dyn crate::llm::domain::AttachmentRegistry>,
        fault: Fault,
        upserts: std::sync::atomic::AtomicUsize,
        deletes: std::sync::atomic::AtomicUsize,
    }

    impl Faulty {
        fn over(book: &Book, p: &Prepared, fault: Fault) -> Arc<Self> {
            Arc::new(Self {
                inner: Arc::new(book.registry(p.storage.deleted_handle())),
                fault,
                upserts: Default::default(),
                deletes: Default::default(),
            })
        }
    }

    #[async_trait::async_trait]
    impl crate::llm::domain::AttachmentRegistry for Faulty {
        async fn upsert(
            &self,
            input: crate::llm::domain::attachments::UpsertAttachmentInput,
        ) -> Result<(), AttachmentError> {
            let n = self
                .upserts
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                + 1;
            self.inner.upsert(input).await?;
            match self.fault {
                Fault::UpsertCommitsThenErrs(k) if k == n => {
                    Err(AttachmentError::RepositoryFailed("timed out".into()))
                }
                Fault::UpsertCommitsThenHangs(k) if k == n => std::future::pending().await,
                _ => Ok(()),
            }
        }
        async fn upsert_checked(
            &self,
            input: crate::llm::domain::attachments::UpsertAttachmentInput,
        ) -> Result<crate::llm::domain::attachments::UpsertOutcome, AttachmentError> {
            self.inner.upsert_checked(input).await
        }
        async fn lookup(
            &self,
            a: &str,
            d: &str,
            p: ProviderKind,
        ) -> Result<Option<ConversationAttachment>, AttachmentError> {
            self.inner.lookup(a, d, p).await
        }
        async fn refresh_provider_file_id(
            &self,
            a: &str,
            d: &str,
            p: ProviderKind,
            f: &str,
        ) -> Result<(), AttachmentError> {
            self.inner.refresh_provider_file_id(a, d, p, f).await
        }
        async fn update_description(
            &self,
            a: &str,
            d: &str,
            p: ProviderKind,
            t: &str,
        ) -> Result<(), AttachmentError> {
            self.inner.update_description(a, d, p, t).await
        }
        async fn list_for_session(
            &self,
            a: &str,
        ) -> Result<Vec<ConversationAttachment>, AttachmentError> {
            self.inner.list_for_session(a).await
        }
        async fn lookup_by_document_id(
            &self,
            a: &str,
            d: &str,
        ) -> Result<Option<ConversationAttachment>, AttachmentError> {
            self.inner.lookup_by_document_id(a, d).await
        }
        async fn touch_last_used(&self, a: &str, d: &str) -> Result<(), AttachmentError> {
            self.inner.touch_last_used(a, d).await
        }
        async fn find_stale_attachments(
            &self,
            q: crate::llm::domain::attachments::StaleAttachmentQuery,
        ) -> Result<Vec<ConversationAttachment>, AttachmentError> {
            self.inner.find_stale_attachments(q).await
        }
        async fn delete_attachment(&self, a: &str, d: &str) -> Result<(), AttachmentError> {
            self.inner.delete_attachment(a, d).await
        }
        async fn delete_attachment_for_provider(
            &self,
            a: &str,
            d: &str,
            p: ProviderKind,
        ) -> Result<(), AttachmentError> {
            let n = self
                .deletes
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            match self.fault {
                Fault::DeleteHangs => std::future::pending().await,
                Fault::FirstDeleteHangs if n == 0 => std::future::pending().await,
                Fault::DeleteCommitsThenErrs => {
                    self.inner.delete_attachment_for_provider(a, d, p).await?;
                    Err(AttachmentError::RepositoryFailed("timed out".into()))
                }
                _ => self.inner.delete_attachment_for_provider(a, d, p).await,
            }
        }
    }

    fn with_faulty(faulty: Arc<Faulty>, p: &Prepared, exec: Arc<Recorder>) -> DagToolExecutor {
        executor(true, Some(Arc::new(runtime(p, exec, true))))
            .with_attachment_registry(faulty)
            .with_agent_session_id(Some("agent_1".into()))
    }

    const CALL: &str = r#"{"attachment_id":"doc-1","code":"pass"}"#;

    async fn two_files() -> (Prepared, Arc<Recorder>) {
        (
            prepared(&[("sales", 1)], 4).await,
            Recorder::ok_with_files(json!(1), &[("a.csv", b"1"), ("b.csv", b"2")]),
        )
    }

    fn nothing_left(book: &Book, p: &Prepared) {
        let mut deleted = p.storage.deleted.lock().unwrap().clone();
        deleted.sort();
        assert_eq!(deleted, ["generated/a.csv", "generated/b.csv"]);
        assert!(
            book.rows.lock().unwrap().is_empty(),
            "a row points at a deleted object"
        );
    }

    /// An upsert that COMMITTED but reported an error leaves no row pointing at a
    /// deleted object: the row was marked "may exist" before it was asked for.
    #[tokio::test]
    async fn an_upsert_that_committed_and_then_failed_is_undone() {
        let (p, exec) = two_files().await;
        let book = Book::default();
        let faulty = Faulty::over(&book, &p, Fault::UpsertCommitsThenErrs(2));
        let out = body(&with_faulty(faulty, &p, exec), CALL).await;
        assert!(out.get("emitted").is_none(), "{out}");
        assert!(
            out["not_kept"].to_string().contains("none was kept"),
            "{out}"
        );
        settle().await;
        nothing_left(&book, &p);
    }

    /// An upsert that committed and then never answered is cut by its own timeout
    /// and undone the same way.
    #[tokio::test]
    async fn an_upsert_that_committed_and_then_hung_is_undone() {
        let (p, exec) = two_files().await;
        let book = Book::default();
        let faulty = Faulty::over(&book, &p, Fault::UpsertCommitsThenHangs(2));
        let out = body(&with_faulty(faulty, &p, exec), CALL).await;
        assert!(out.get("emitted").is_none(), "{out}");
        settle().await;
        nothing_left(&book, &p);
    }

    /// A row delete whose outcome is unknown (it committed, then errored) is re-read:
    /// the row is gone, so the objects go and nothing is reported kept.
    #[tokio::test]
    async fn an_unknown_row_delete_is_reread_before_it_is_called_kept() {
        let (p, exec) = two_files().await;
        let book = Book::default();
        let faulty = Faulty::over(&book, &p, Fault::UpsertCommitsThenErrs(2));
        let faulty = Arc::new(Faulty {
            fault: Fault::DeleteCommitsThenErrs,
            ..Arc::try_unwrap(faulty).ok().unwrap()
        });
        // The second upsert must still fail: do it through the book's own switch.
        *book.fail_upsert_on.lock().unwrap() = Some(2);
        let out = body(&with_faulty(faulty, &p, exec), CALL).await;
        assert!(
            out.get("emitted").is_none(),
            "nothing is reported kept: {out}"
        );
        settle().await;
        nothing_left(&book, &p);
    }

    /// A cleanup cut off midway (the rollback's future dropped while a row delete
    /// hangs) is finished by the ledger's Drop: nothing is left.
    #[tokio::test]
    async fn a_rollback_cut_off_midway_is_finished_by_the_ledger() {
        use crate::tabular_run::outputs::StoreSink;
        let p = prepared(&[("sales", 1)], 4).await;
        let book = Book::default();
        let faulty = Faulty::over(&book, &p, Fault::FirstDeleteHangs);
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
        let mut ledger = guard.into_ledger(Some((faulty, "agent_1".into())));
        for k in ["generated/a.csv", "generated/b.csv"] {
            book.rows.lock().unwrap().push(ConversationAttachment {
                document_id: k.into(),
                ..row(false)
            });
            ledger.row_may_exist(k);
        }
        // Dropped while the first delete hangs.
        let cut =
            tokio::time::timeout(std::time::Duration::from_millis(100), ledger.rollback()).await;
        assert!(cut.is_err(), "the rollback was still waiting");
        tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
        nothing_left(&book, &p);
    }

    /// The conversation's lock is free while a failed registration's rows are being
    /// taken back: a hanging delete does not hold other calls up.
    #[tokio::test]
    async fn the_lock_is_not_held_across_the_rollback() {
        let (p, exec) = two_files().await;
        let book = Book::default();
        *book.fail_upsert_on.lock().unwrap() = Some(2);
        let faulty = Faulty::over(&book, &p, Fault::DeleteHangs);
        let watched = faulty.clone();
        // Its own conversation: the lock is per conversation and other tests share "agent_1".
        let ex =
            with_faulty(faulty, &p, exec).with_agent_session_id(Some("agent_lock_probe".into()));
        let probe = async move {
            // The rollback is running once a row delete has begun (it then hangs).
            while watched.deletes.load(std::sync::atomic::Ordering::SeqCst) == 0 {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            super::session_lock("agent_lock_probe").try_lock().is_ok()
        };
        let (_, free) = tokio::join!(body(&ex, CALL), probe);
        assert!(free, "the lock was held while rows were being removed");
    }

    /// Keeping the files is inside the call's clock: a registry that never answers is
    /// cut by the clock too, and the ledger inside undoes what it made.
    #[tokio::test]
    async fn keeping_the_files_runs_under_the_calls_clock() {
        let (p, exec) = two_files().await;
        let book = Book::default();
        let faulty = Faulty::over(&book, &p, Fault::UpsertCommitsThenHangs(1));
        let ex = with_faulty(faulty, &p, exec);
        let target = ex
            .large_target_for(
                crate::llm::domain::large_tabular::LargeTool::AttachmentRunPython,
                "doc-1",
            )
            .expect("routed");
        let result = super::dispatch_bounded(
            &ex,
            "c1",
            &args(),
            "{}",
            target,
            super::CallClock {
                every: std::time::Duration::from_secs(1),
                budget: std::time::Duration::from_millis(120),
            },
            LargeTool::AttachmentRunPython,
        )
        .await;
        assert!(
            result.output.contains("did not finish"),
            "{}",
            result.output
        );
        tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
        nothing_left(&book, &p);
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
