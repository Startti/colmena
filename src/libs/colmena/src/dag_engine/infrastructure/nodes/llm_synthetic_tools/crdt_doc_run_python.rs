//! LLM tool `crdt_doc_run_python` — runs sandboxed Python (pandas/numpy/
//! scipy.stats) against workbook data extracted from the current
//! `CrdtDocsContext`. See:
//!   - Spec: docs/superpowers/specs/2026-06-03-crdt-pandas-integration-design.md
//!   - Plan: docs/superpowers/plans/2026-06-03-crdt-pandas-integration.md (C-T5)

use crate::crdt_documents::{
    build_records_for_sheets, write_records_as_new_sheet, RecordsError,
};
use crate::dag_engine::infrastructure::nodes::python_node::execute_sandboxed_helper;
use crate::llm::domain::tools::ToolDefinition;
use schemars::JsonSchema;
use serde::Deserialize;

pub use super::crdt_doc_context::CrdtDocsContext;

pub const TOOL_RUN_PYTHON: &str = "crdt_doc_run_python";

/// Caps that apply to the response sent back to the LLM.
const OUTPUT_BYTE_CAP: usize = 10 * 1024;
const STDOUT_BYTE_CAP: usize = 10 * 1024;
const ERROR_BYTE_CAP: usize = 10 * 1024;
const PREVIEW_ROWS_IN_WROTE_SHEET: usize = 5;
/// Sandbox timeout for code execution (v1 hardcoded — see BACKLOG for
/// configurable path).
const CODE_TIMEOUT_SECS: u64 = 30;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RunPythonArgs {
    /// Sheets to load as pandas DataFrames. Available in the code as
    /// `dfs[<sheet_id>]`. At least one required.
    pub sheet_ids: Vec<String>,
    /// Python code to execute. Must define `output` (any JSON-serializable
    /// value) and/or `output_sheet` (a pandas DataFrame). Has access to
    /// `pandas as pd`, `numpy as np`, `scipy.stats as stats`.
    pub code: String,
    /// If set, `output_sheet` is written as a new sheet with this name.
    /// Name collisions append " (2)", " (3)" etc.
    #[serde(default)]
    pub write_to_sheet: Option<String>,
}

/// Build the [`ToolDefinition`] for `crdt_doc_run_python`.
pub fn tool_run_python() -> ToolDefinition {
    super::build_synthetic_tool::<RunPythonArgs>(
        TOOL_RUN_PYTHON,
        "Run sandboxed Python code (pandas, numpy, scipy.stats) against \
         workbook data. Loads requested sheets as `dfs[sheet_id]` pandas \
         DataFrames. The code must define `output` (returned to you) and/or \
         `output_sheet` (a DataFrame). If `write_to_sheet` is set, \
         `output_sheet` is persisted as a new sheet with that name. \
         Use this for analysis on large sheets — only the first ~10 rows \
         need to pass through your context (use crdt_doc_read for a sample); \
         the code runs server-side on the full data.",
    )
}

/// Execute `run_python` against the runtime: extract records from the
/// requested sheets, run the user's wrapped code in the sandbox, and
/// optionally persist `output_sheet` as a new sheet.
pub async fn execute_run_python(
    ctx: &CrdtDocsContext,
    args: RunPythonArgs,
) -> serde_json::Value {
    // 1. Validate args.
    if args.sheet_ids.is_empty() {
        return serde_json::json!({ "error": "sheet_ids must be non-empty" });
    }

    // 2. Build records from the Y.Doc.
    let Some(doc) = ctx.doc() else {
        return serde_json::json!({ "error": "artifact_not_found" });
    };
    let records_by_sheet = match build_records_for_sheets(&doc, &args.sheet_ids) {
        Ok(m) => m,
        Err(RecordsError::SheetNotFound(id)) => {
            return serde_json::json!({ "error": format!("sheet_not_found: {id}") });
        }
        Err(RecordsError::SizeCapExceeded { actual, limit }) => {
            return serde_json::json!({
                "error": "load_size_exceeded",
                "actual_bytes": actual,
                "limit_bytes": limit,
            });
        }
    };

    // 3. Build inputs for the python helper: `_dfs_raw` is a dict whose
    //    values are lists of dicts (records). The auto-prelude builds
    //    pandas DataFrames from these.
    let mut dfs_raw_json = serde_json::Map::new();
    for (sid, recs) in &records_by_sheet {
        let array: Vec<serde_json::Value> = recs
            .records
            .iter()
            .cloned()
            .map(serde_json::Value::Object)
            .collect();
        dfs_raw_json.insert(sid.clone(), serde_json::Value::Array(array));
    }
    let mut inputs = serde_json::Map::new();
    inputs.insert("_dfs_raw".to_string(), serde_json::Value::Object(dfs_raw_json));

    // 4. Wrap user code with prelude (build dfs) and postlude (package output).
    let wrapped_code = wrap_user_code(&args.code);

    // 5. Run in spawn_blocking with a timeout.
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(CODE_TIMEOUT_SECS),
        tokio::task::spawn_blocking(move || {
            execute_sandboxed_helper(&wrapped_code, "restricted", CODE_TIMEOUT_SECS, &inputs)
        }),
    )
    .await;

    let helper_result = match result {
        Ok(Ok(Ok(r))) => r,
        Ok(Ok(Err(e))) => {
            return serde_json::json!({
                "output": serde_json::Value::Null,
                "wrote_sheet": serde_json::Value::Null,
                "stdout": "",
                "error": truncate(&e, ERROR_BYTE_CAP),
            });
        }
        Ok(Err(join_err)) => {
            return serde_json::json!({ "error": format!("internal join error: {join_err}") });
        }
        Err(_) => {
            return serde_json::json!({
                "error": format!("code execution exceeded {CODE_TIMEOUT_SECS}s timeout"),
            });
        }
    };

    // 6. Unpack the wrapped output: postlude packs three fields into `output`.
    let wrapped_output = helper_result.output.unwrap_or(serde_json::Value::Null);
    let user_output = wrapped_output
        .get("user_output")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let sheet_records = wrapped_output
        .get("sheet_records")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let sheet_cols = wrapped_output
        .get("sheet_cols")
        .cloned()
        .unwrap_or(serde_json::Value::Null);

    // 7. If `write_to_sheet` was set AND `output_sheet` was a DataFrame,
    //    write to Y.Doc.
    let mut wrote_sheet_response = serde_json::Value::Null;
    if let Some(target_name) = args.write_to_sheet.as_deref() {
        if let (Some(records_arr), Some(cols_arr)) =
            (sheet_records.as_array(), sheet_cols.as_array())
        {
            let cols: Vec<String> = cols_arr
                .iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect();
            let records: Vec<serde_json::Map<String, serde_json::Value>> = records_arr
                .iter()
                .filter_map(|v| v.as_object().cloned())
                .collect();
            match write_records_as_new_sheet(&doc, target_name, &cols, &records) {
                Ok(wr) => {
                    let preview: Vec<&serde_json::Map<String, serde_json::Value>> =
                        records.iter().take(PREVIEW_ROWS_IN_WROTE_SHEET).collect();
                    wrote_sheet_response = serde_json::json!({
                        "sheet_id": wr.sheet_id,
                        "name": wr.resolved_name,
                        "n_rows": wr.n_rows,
                        "n_cols": wr.n_cols,
                        "preview": preview,
                        "truncated_at": wr.truncated_at,
                    });
                    ctx.mark_dirty();
                    let origin = ctx
                        .session_id()
                        .map(|s| format!("agent:{s}"))
                        .unwrap_or_else(|| "agent:llm".to_string());
                    let event_id = ctx
                        .backend()
                        .record_event(crate::crdt_documents::change_tracker_store::NewEvent {
                            artifact_id: ctx.artifact_id().clone(),
                            sheet_id: Some(wr.sheet_id.clone()),
                            origin,
                            summary: format!(
                                "wrote {} rows via run_python to new sheet '{}'",
                                wr.n_rows, wr.resolved_name
                            ),
                        })
                        .await
                        .unwrap_or(0);
                    ctx.record_event_id(event_id);
                }
                Err(e) => {
                    return serde_json::json!({
                        "output": user_output,
                        "wrote_sheet": serde_json::Value::Null,
                        "stdout": truncate(&helper_result.stdout, STDOUT_BYTE_CAP),
                        "error": format!("write_to_sheet failed: {e}"),
                    });
                }
            }
        }
    }

    // 8. Truncate user output JSON if too large.
    let (user_output_capped, output_truncated) = truncate_json(&user_output, OUTPUT_BYTE_CAP);

    let mut response = serde_json::json!({
        "output": user_output_capped,
        "wrote_sheet": wrote_sheet_response,
        "stdout": truncate(&helper_result.stdout, STDOUT_BYTE_CAP),
        "error": serde_json::Value::Null,
    });
    if output_truncated {
        response["_output_truncated"] = serde_json::json!(true);
    }
    response
}

/// Dispatch wrapper called from `dag_tool_executor`.
pub async fn dispatch_crdt_doc_run_python(
    ctx: &CrdtDocsContext,
    args: serde_json::Value,
) -> serde_json::Value {
    match serde_json::from_value::<RunPythonArgs>(args) {
        Ok(a) => execute_run_python(ctx, a).await,
        Err(e) => serde_json::json!({ "error": format!("invalid_args: {e}") }),
    }
}

// ── helpers ──────────────────────────────────────────────────────────────

fn wrap_user_code(user_code: &str) -> String {
    let prelude = r#"# === colmena auto-prelude (do not modify) ===
import pandas as pd
import numpy as np
from scipy import stats

dfs = {k: pd.DataFrame(v) for k, v in _dfs_raw.items()}
del _dfs_raw

# === user code starts here ===
"#;
    let postlude = r#"
# === user code ends ===

# === colmena auto-postlude ===
__col_user_output = output if 'output' in dir() else None
__col_sheet_records = None
__col_sheet_cols = None
if 'output_sheet' in dir() and output_sheet is not None:
    import pandas as _pd
    if isinstance(output_sheet, _pd.DataFrame):
        __col_sheet_records = output_sheet.to_dict('records')
        __col_sheet_cols = list(output_sheet.columns)

output = {
    'user_output': __col_user_output,
    'sheet_records': __col_sheet_records,
    'sheet_cols': __col_sheet_cols,
}
"#;
    format!("{prelude}{user_code}{postlude}")
}

fn truncate(s: &str, cap: usize) -> String {
    if s.len() <= cap {
        s.to_string()
    } else {
        // Byte-safe truncate (find nearest char boundary).
        let mut end = cap;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}…[truncated at {} bytes]", &s[..end], cap)
    }
}

fn truncate_json(v: &serde_json::Value, cap: usize) -> (serde_json::Value, bool) {
    let serialized = serde_json::to_string(v).unwrap_or_default();
    if serialized.len() <= cap {
        (v.clone(), false)
    } else {
        let mut end = cap;
        while end > 0 && !serialized.is_char_boundary(end) {
            end -= 1;
        }
        (
            serde_json::Value::String(format!(
                "{}…[truncated at {} bytes]",
                &serialized[..end],
                cap
            )),
            true,
        )
    }
}
