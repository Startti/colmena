//! `attachment_run_python` synthetic tool — execute pandas code against a
//! registered CSV/XLSX attachment without forcing the LLM to read every row.
//!
//! ## Why this exists
//!
//! After item 13 shipped (`sql_inspect_attachment` + `sql_bulk_insert_*`),
//! the LLM has three options for a tabular attachment:
//!
//! 1. **See the schema** — auto-summary in the catalog block (free, 0 calls).
//! 2. **Run a calculation** — this tool. Backend loads the file into a
//!    pandas DataFrame, the LLM provides Python code, only the result
//!    (stdout + optional `result` global) crosses the wire. ~80 tokens
//!    response regardless of file size.
//! 3. **Bulk-load into Postgres** — `sql_bulk_insert_from_attachment`.
//!
//! Compared to `load_attachment` (which dumps every row into the LLM
//! context, ~50K tokens for a 1487-row CSV), this tool is **~600× cheaper**
//! per query.
//!
//! ## Sandbox
//!
//! Reuses `execute_sandboxed_helper` from `nodes::python_node` in the same
//! `restricted` mode as `gsheets_run_python` and `crdt_doc_run_python`.
//! Imports are AST-validated:
//!   - **Allowed:** pandas, numpy, scipy.stats, math, datetime, decimal,
//!     json, re, statistics, string, collections, functools, itertools.
//!   - **Blocked:** os, sys, subprocess, socket, urllib, requests,
//!     importlib, builtins, ctypes (anything that could escape the sandbox).
//!
//! No filesystem access, no network access. The attachment bytes live in
//! Python memory only for the duration of the call.
//!
//! ## Both inline AND signed-URL attachments are supported
//!
//! Resolution happens through the shared `DagToolExecutor::fetch_attachment_bytes`
//! helper (Bulk T0, commit `479c321`). That path reads from
//! `OutputStorageRepository`, which is what the registry persists for both
//! source types — inline `data:` base64 uploads AND signed-URL pulls. The
//! dispatcher does not need to know the source type.

use crate::llm::domain::{LlmError, ToolCall, ToolResult};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Tool name surfaced to the LLM. Same convention as the other SQL bulk tools.
pub const ATTACHMENT_RUN_PYTHON_TOOL_NAME: &str = "attachment_run_python";

/// Wall-clock cap for one Python execution. Mirrors `gsheets_run_python`.
const CODE_TIMEOUT_SECS: u64 = 30;

/// Per-string cap for output strings (stdout, error). Strings beyond this
/// are truncated with a `[truncated]` marker before being returned to the LLM.
const OUTPUT_BYTE_CAP: usize = 50 * 1024;

/// Arguments accepted by the `attachment_run_python` tool.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AttachmentRunPythonArgs {
    /// `document_id` of an attachment registered in the conversation catalog
    /// (visible in the system message's catalog block). Resolves through
    /// `DagToolExecutor::fetch_attachment_bytes`, which supports inline and
    /// signed-URL sources uniformly.
    pub attachment_id: String,

    /// Python source to execute. A pandas DataFrame `df` is pre-loaded from
    /// the attachment before this code runs. Convenience modules also
    /// available: `pd` (pandas), `np` (numpy), `stats` (scipy.stats), and
    /// the standard library (math, datetime, decimal, json, re, statistics,
    /// string, collections, functools, itertools).
    ///
    /// Two ways to surface output:
    ///   - `print(...)` → captured into `stdout` in the response.
    ///   - Assign a value to a global `result` → returned in the response's
    ///     `result` field. `result` may be any JSON-serialisable value (dict,
    ///     list, number, string, None). Pandas DataFrames are converted via
    ///     `to_dict(orient="records")`; pandas Series via `to_list()`.
    pub code: String,

    /// CSV delimiter override. Default: auto-detect comma / semicolon / tab.
    /// Ignored for XLSX.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delimiter: Option<String>,

    /// XLSX sheet name. Default: first sheet. Ignored for CSV.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sheet_name: Option<String>,

    /// 1-indexed row where the headers live. Default 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header_row: Option<u32>,
}

/// Response shape returned by `attachment_run_python`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AttachmentRunPythonResponse {
    /// Captured stdout from `print()` calls. Truncated at
    /// `OUTPUT_BYTE_CAP` bytes.
    pub stdout: String,

    /// Value of the `result` global if the user code assigned one;
    /// `null` otherwise.
    #[serde(default)]
    pub result: serde_json::Value,

    /// Wall-clock execution time in milliseconds (DataFrame load + user code).
    pub duration_ms: u64,

    /// Number of rows loaded into `df`. Useful so the LLM knows the working
    /// set size without computing `len(df)`.
    pub row_count: u64,

    /// Column names loaded into `df`. Mirrors `loaded_columns` in
    /// `gsheets_run_python`.
    pub columns: Vec<String>,

    /// Optional sheet name (XLSX only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sheet_name: Option<String>,

    /// Error message if execution failed (sandbox violation, syntax error,
    /// runtime exception). When present, `stdout` may still hold partial
    /// output captured before the failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Build the `ToolDefinition` for the LLM. Schema is derived from
/// `AttachmentRunPythonArgs` via `schemars`.
pub fn build_attachment_run_python_tool_definition() -> crate::llm::domain::tools::ToolDefinition {
    super::build_synthetic_tool_with_summary::<AttachmentRunPythonArgs>(
        ATTACHMENT_RUN_PYTHON_TOOL_NAME,
        crate::text::tool_description(ATTACHMENT_RUN_PYTHON_TOOL_NAME),
        crate::text::tool_summary(ATTACHMENT_RUN_PYTHON_TOOL_NAME),
    )
}

/// Wrap the user's Python code with a prelude that materialises the
/// DataFrame and a postlude that extracts the `result` global.
///
/// The wrapper uses indented blocks so the user's code can include
/// top-level imports of allowed modules (the sandbox validates the
/// surrounding wrapper too, so all imports MUST be in the allow-list).
fn wrap_user_code(code: &str) -> String {
    // Note: `pd.DataFrame(_attachment_records)` keeps every column as
    // `object` dtype (CSV values arrived as strings). The user can coerce
    // with `df = df.astype(...)` or `pd.to_numeric(df['col'], errors='coerce')`
    // for any column they care about.
    //
    // Output convention: the user assigns a value to a `result` global. The
    // postlude then aliases `result` → `output`, which is the global name
    // `execute_sandboxed_helper` reads out of locals. Pandas objects are
    // structurally serialised so the response stays JSON-clean.
    format!(
        r#"
import pandas as pd
import numpy as np
import scipy.stats as stats

df = pd.DataFrame(_attachment_records)
result = None

{code}

# === colmena auto-postlude ===
def __col_serialise(v):
    if v is None:
        return None
    if hasattr(v, 'to_dict') and callable(v.to_dict):
        try:
            return v.to_dict(orient='records')
        except TypeError:
            return v.to_dict()
    if hasattr(v, 'to_list') and callable(v.to_list):
        return v.to_list()
    try:
        # numpy scalar coercion — np.int64 / np.float64 / etc.
        import numpy as _np
        if isinstance(v, _np.generic):
            return v.item()
    except Exception:
        pass
    return v

output = __col_serialise(result)
"#,
        code = code
    )
}

/// Truncate a string to `cap` bytes, appending `[truncated]` if it was cut.
fn truncate(s: &str, cap: usize) -> String {
    if s.len() <= cap {
        return s.to_string();
    }
    let mut end = cap;
    while !s.is_char_boundary(end) && end > 0 {
        end -= 1;
    }
    format!("{}\n[truncated]", &s[..end])
}

/// Dispatcher entry point used by `DagToolExecutor::execute`.
pub async fn dispatch_attachment_run_python_via_executor(
    executor: &crate::dag_engine::infrastructure::dag_tool_executor::DagToolExecutor,
    tool_call: &ToolCall,
) -> Result<ToolResult, LlmError> {
    let call_id = &tool_call.id;

    // 1. Parse args from the LLM.
    let args: AttachmentRunPythonArgs = match serde_json::from_str(&tool_call.function.arguments) {
        Ok(a) => a,
        Err(e) => {
            return Ok(err_envelope(
                call_id,
                format!("failed to parse attachment_run_python args: {e}"),
            ))
        }
    };

    if args.code.trim().is_empty() {
        return Ok(err_envelope(call_id, "code must be non-empty".to_string()));
    }

    // 2. Fetch attachment bytes via the shared plumbing (Bulk T0).
    let stored = match executor.fetch_attachment_bytes(&args.attachment_id).await {
        Ok(b) => b,
        Err(e) => return Ok(err_envelope(call_id, e)),
    };

    // 3. Prefer catalog mime/filename (authoritative) over storage's reported
    //    values — local storage strips both. Same pattern as sql_inspect.
    let (effective_mime, effective_filename) = executor
        .lookup_attachment_meta(&args.attachment_id)
        .unwrap_or_else(|| (stored.mime_type.clone(), stored.filename.clone()));

    // 4. Parse CSV/XLSX → records (pure).
    let (columns, records) = match super::sql_bulk_tools::parse_attachment_to_records(
        &stored.bytes,
        &effective_mime,
        &effective_filename,
        args.delimiter.as_deref(),
        args.sheet_name.as_deref(),
        args.header_row,
    ) {
        Ok(t) => t,
        Err(e) => return Ok(err_envelope(call_id, e)),
    };
    let row_count = records.len() as u64;

    // 5. Build the sandbox inputs map and wrap the user code.
    let mut inputs = serde_json::Map::new();
    let records_value =
        serde_json::Value::Array(records.into_iter().map(serde_json::Value::Object).collect());
    inputs.insert("_attachment_records".to_string(), records_value);
    let wrapped = wrap_user_code(&args.code);

    // 6. Run in `spawn_blocking` with a wall-clock timeout. Same pattern as
    //    `gsheets_run_python`.
    let started = std::time::Instant::now();
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(CODE_TIMEOUT_SECS),
        tokio::task::spawn_blocking(move || {
            crate::dag_engine::infrastructure::nodes::python_node::execute_sandboxed_helper(
                &wrapped,
                "restricted",
                CODE_TIMEOUT_SECS,
                &inputs,
            )
        }),
    )
    .await;

    let duration_ms = started.elapsed().as_millis() as u64;

    let helper_result = match result {
        Ok(Ok(Ok(r))) => r,
        Ok(Ok(Err(e))) => {
            let resp = AttachmentRunPythonResponse {
                stdout: String::new(),
                result: serde_json::Value::Null,
                duration_ms,
                row_count,
                columns,
                sheet_name: args.sheet_name.clone(),
                error: Some(truncate(&e, OUTPUT_BYTE_CAP)),
            };
            return Ok(success_envelope(call_id, &resp));
        }
        Ok(Err(join_err)) => {
            return Ok(err_envelope(
                call_id,
                format!("internal join error: {join_err}"),
            ));
        }
        Err(_) => {
            return Ok(err_envelope(
                call_id,
                format!("code execution exceeded {CODE_TIMEOUT_SECS}s timeout"),
            ));
        }
    };

    // 7. Extract stdout + `result` global.
    let stdout = truncate(&helper_result.stdout, OUTPUT_BYTE_CAP);
    // `execute_sandboxed_helper` returns the value bound to `output` (its
    // convention). For us the user assigns `result`, which is bound in
    // `locals` after the postlude runs. We surface it via the same path —
    // see python_node::execute_sandboxed_helper. Inputs map carries it back.
    let result_value = helper_result.output.unwrap_or(serde_json::Value::Null);

    let resp = AttachmentRunPythonResponse {
        stdout,
        result: result_value,
        duration_ms,
        row_count,
        columns,
        sheet_name: args.sheet_name.clone(),
        error: None,
    };
    Ok(success_envelope(call_id, &resp))
}

/// Wrap a successful response in `ToolResult::success`.
fn success_envelope(call_id: &str, resp: &AttachmentRunPythonResponse) -> ToolResult {
    let v = serde_json::to_value(resp).unwrap_or(serde_json::Value::Null);
    ToolResult::success(call_id.to_string(), v.to_string())
}

/// Wrap a structured error message into the tool error envelope the LLM
/// expects. Same convention as `sql_bulk_tools::err_envelope`.
fn err_envelope(call_id: &str, msg: String) -> ToolResult {
    let v = serde_json::json!({ "error": msg, "source": "execution" });
    ToolResult::success(call_id.to_string(), v.to_string())
}

// Marker to silence unused warnings in case the `HashMap` import drifts
// during refactors. Removed at any cleanup pass.
#[allow(dead_code)]
fn _hashmap_keep_alive() -> HashMap<String, String> {
    HashMap::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_minimal_deserialize() {
        let body = r#"{"attachment_id":"doc-x","code":"print(df.shape)"}"#;
        let args: AttachmentRunPythonArgs = serde_json::from_str(body).unwrap();
        assert_eq!(args.attachment_id, "doc-x");
        assert!(args.code.contains("print"));
        assert!(args.delimiter.is_none());
    }

    #[test]
    fn args_optional_fields_deserialize() {
        let body = r#"{
            "attachment_id": "doc-x",
            "code": "result = df['price'].max()",
            "sheet_name": "Inventory",
            "header_row": 2,
            "delimiter": ";"
        }"#;
        let args: AttachmentRunPythonArgs = serde_json::from_str(body).unwrap();
        assert_eq!(args.sheet_name.as_deref(), Some("Inventory"));
        assert_eq!(args.header_row, Some(2));
        assert_eq!(args.delimiter.as_deref(), Some(";"));
    }

    #[test]
    fn wrap_user_code_preloads_df() {
        let user = "result = df['price'].mean()";
        let wrapped = wrap_user_code(user);
        assert!(wrapped.contains("df = pd.DataFrame(_attachment_records)"));
        assert!(wrapped.contains(user));
        assert!(wrapped.contains("import pandas as pd"));
    }

    #[test]
    fn truncate_under_cap_unchanged() {
        let s = "short";
        assert_eq!(truncate(s, 100), s);
    }

    #[test]
    fn truncate_over_cap_appends_marker() {
        let long: String = "x".repeat(200);
        let out = truncate(&long, 50);
        assert!(out.starts_with("xxx"));
        assert!(out.contains("[truncated]"));
    }
}
