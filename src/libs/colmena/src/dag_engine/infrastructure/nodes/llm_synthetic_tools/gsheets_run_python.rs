//! LLM tool `gsheets_run_python` — runs sandboxed Python
//! (pandas/numpy/scipy.stats) over data loaded directly from Google
//! Sheets via the existing [`SheetsClient`] adapter. Mirrors
//! [`crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::crdt_doc_run_python`]
//! one-for-one — same prelude/postlude shape, same caps, same error
//! envelope. The win over `gsheets_read` + `run_python` is that **rows
//! never pass through the LLM context**: the dispatcher fetches each
//! binding directly from Google (in parallel), pushes the records into
//! the sandbox as Python globals, and the LLM only sees the final
//! `output`.
//!
//! See E-T14 in the implementation plan.

use crate::dag_engine::infrastructure::nodes::python_node::execute_sandboxed_helper;
use crate::gsheets::domain::{ReadOptions, ReadResponse, SheetsClient, SpreadsheetId};
use crate::gsheets::infrastructure::config::GSheetsConfig;
use crate::gsheets::infrastructure::http_client::GoogleSheetsHttpClient;
use crate::llm::domain::tools::ToolDefinition;
use crate::text;
use futures::future::join_all;
use schemars::JsonSchema;
use serde::Deserialize;
use std::sync::Arc;

use super::gsheets_tools::error_to_json;

pub const TOOL_GSHEETS_RUN_PYTHON: &str = "gsheets_run_python";

const GSHEETS_PY_PRELUDE: &str =
    include_str!("../../../../../text/prompts/python_sandbox/gsheets_run_python_prelude.md");
const GSHEETS_PY_POSTLUDE: &str =
    include_str!("../../../../../text/prompts/python_sandbox/gsheets_run_python_postlude.md");

/// Caps that apply to the response sent back to the LLM.
const OUTPUT_BYTE_CAP: usize = 10 * 1024;
const STDOUT_BYTE_CAP: usize = 10 * 1024;
const ERROR_BYTE_CAP: usize = 10 * 1024;
/// Sandbox timeout for code execution (v1 hardcoded — mirrors the
/// `crdt_doc_run_python` cap).
const CODE_TIMEOUT_SECS: u64 = 30;

// ── Args ─────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GsheetsRunPythonArgs {
    /// Spreadsheet ranges to load. Each binding becomes a Python global
    /// (a list of `{col: val}` dicts) under the name given by `var`.
    /// At least one entry required. Bindings are fetched IN PARALLEL.
    pub bindings: Vec<GsheetsBinding>,
    /// Python code. Has access to `pandas as pd`, `numpy as np`,
    /// `scipy.stats as stats`, plus each binding's records list bound
    /// under its `var` name. Define `output` (any JSON-serializable
    /// value) — that is what the LLM sees.
    pub code: String,

    /// Optional target spreadsheet for `output_sheets` returned by the script.
    /// When set AND the script assigns `output_sheets = {name: DataFrame, ...}`,
    /// the dispatcher creates one new tab per entry and writes the DataFrame
    /// as a values matrix (header row + body). If the name already exists,
    /// the dispatcher appends " (2)", " (3)", etc. (capped at 10 attempts).
    /// When omitted, any `output_sheets` produced by the script is ignored.
    #[serde(default)]
    pub write_to_spreadsheet: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GsheetsBinding {
    /// Python variable name to bind the records to (e.g. "products",
    /// "sales"). Must be a non-empty identifier-shaped string.
    pub var: String,
    /// Spreadsheet ID (the chunk between `/d/` and `/edit` in the
    /// Google Sheets URL).
    pub spreadsheet_id: String,
    /// Tab name (e.g. "Sheet1", "Hoja 1").
    pub sheet: String,
    /// Optional A1 range (e.g. "A1:H5001"). Omit to read the entire tab.
    #[serde(default)]
    pub range: Option<String>,
}

// ── Tool builder ─────────────────────────────────────────────────────

pub fn tool_gsheets_run_python() -> ToolDefinition {
    super::build_synthetic_tool_with_summary::<GsheetsRunPythonArgs>(
        TOOL_GSHEETS_RUN_PYTHON,
        text::tool_description(TOOL_GSHEETS_RUN_PYTHON),
        text::tool_summary(TOOL_GSHEETS_RUN_PYTHON),
    )
}

// ── Dispatchers ──────────────────────────────────────────────────────

/// Build a client from process env (`GOOGLE_APPLICATION_CREDENTIALS` /
/// ADC) and dispatch. This is the production entry point used by the
/// router in `dag_tool_executor`.
pub async fn dispatch_gsheets_run_python(args: serde_json::Value) -> serde_json::Value {
    let client = match GoogleSheetsHttpClient::from_config(&GSheetsConfig::from_env()) {
        Ok(c) => c,
        Err(e) => return error_to_json(e),
    };
    dispatch_gsheets_run_python_with_client(Arc::new(client), args).await
}

/// Inner dispatcher that takes an injected [`SheetsClient`] — used by
/// tests with a wiremock-backed client. Bindings are fetched in
/// parallel via [`futures::future::join_all`].
pub async fn dispatch_gsheets_run_python_with_client(
    client: Arc<dyn SheetsClient>,
    args: serde_json::Value,
) -> serde_json::Value {
    // 1. Parse + validate args.
    let parsed: GsheetsRunPythonArgs = match serde_json::from_value(args) {
        Ok(a) => a,
        Err(e) => {
            return serde_json::json!({
                "error": "invalid_args",
                "message": e.to_string(),
            });
        }
    };
    if parsed.bindings.is_empty() {
        return serde_json::json!({
            "error": "invalid_args",
            "message": "bindings must be non-empty",
        });
    }
    for b in &parsed.bindings {
        if b.var.trim().is_empty() {
            return serde_json::json!({
                "error": "invalid_args",
                "message": "every binding must have a non-empty `var`",
            });
        }
    }
    // Reject duplicate `var` names — last one would silently shadow the
    // earlier reads otherwise.
    {
        let mut seen = std::collections::HashSet::new();
        for b in &parsed.bindings {
            if !seen.insert(b.var.as_str()) {
                return serde_json::json!({
                    "error": "invalid_args",
                    "message": format!("duplicate binding var: {}", b.var),
                });
            }
        }
    }

    // 2. Fetch every binding in parallel. `read_range` with
    //    `as_records=true` returns the per-tab records shape already
    //    keyed by header row — exactly what pandas wants.
    let fetches = parsed.bindings.iter().map(|b| {
        let client = client.clone();
        let opts = ReadOptions {
            value_render: crate::gsheets::domain::ValueRenderOption::UnformattedValue,
            as_records: true,
        };
        let ss = SpreadsheetId(b.spreadsheet_id.clone());
        let sheet = b.sheet.clone();
        let range = b.range.clone();
        let var = b.var.clone();
        async move {
            let res = client.read_range(&ss, &sheet, range.as_deref(), opts).await;
            (var, res)
        }
    });
    let results: Vec<(String, Result<ReadResponse, _>)> = join_all(fetches).await;

    // 3. Build the sandbox inputs map + a side-table of column names per
    //    binding (used in error responses so the LLM can self-correct
    //    without an extra round-trip).
    let mut inputs = serde_json::Map::new();
    let mut loaded_columns = serde_json::Map::new();
    for (var, res) in results {
        let resp = match res {
            Ok(r) => r,
            Err(e) => {
                let mut payload = error_to_json(e);
                if let serde_json::Value::Object(ref mut m) = payload {
                    m.insert("binding".to_string(), serde_json::Value::String(var));
                }
                return payload;
            }
        };
        let columns = extract_columns(&resp.values);
        loaded_columns.insert(
            var.clone(),
            serde_json::Value::Array(columns.into_iter().map(serde_json::Value::String).collect()),
        );
        // `values` is a JSON array of {col: val} objects when
        // as_records=true — push it under `var` unchanged.
        let records = match resp.values {
            serde_json::Value::Array(a) => serde_json::Value::Array(a),
            // Defensive: if Google returned an empty body, treat as
            // empty list rather than confusing the sandbox.
            serde_json::Value::Null => serde_json::Value::Array(vec![]),
            other => other,
        };
        inputs.insert(var, records);
    }
    let loaded_columns = serde_json::Value::Object(loaded_columns);
    // Make column metadata available to the sandbox too (mirrors
    // crdt_doc_run_python's `loaded_sheet_columns` helper).
    inputs.insert(
        "_gsheets_loaded_columns".to_string(),
        loaded_columns.clone(),
    );

    // 4. Wrap user code with prelude/postlude.
    let wrapped_code = wrap_user_code(&parsed.code);

    // 5. Run in spawn_blocking with timeout.
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
                "stdout": "",
                "error": truncate(&e, ERROR_BYTE_CAP),
                "loaded_columns": loaded_columns,
            });
        }
        Ok(Err(join_err)) => {
            return serde_json::json!({
                "error": format!("internal join error: {join_err}"),
                "loaded_columns": loaded_columns,
            });
        }
        Err(_) => {
            return serde_json::json!({
                "error": format!("code execution exceeded {CODE_TIMEOUT_SECS}s timeout"),
                "loaded_columns": loaded_columns,
            });
        }
    };

    // The postlude wraps all globals into {"user_output": ..., "output_sheets": ...}.
    let wrapped_output = helper_result.output.unwrap_or(serde_json::Value::Null);

    // Extract the user-facing `output` global (the "user_output" key in the
    // postlude-wrapped object).
    let user_output = wrapped_output
        .get("user_output")
        .cloned()
        .unwrap_or(serde_json::Value::Null);

    // Multi-sheet write-back: if the script set `output_sheets` AND the args
    // supplied `write_to_spreadsheet`, write each entry as a new tab.
    let output_sheets_value = wrapped_output.get("output_sheets");

    let has_target = parsed.write_to_spreadsheet.is_some();
    let has_sheets = output_sheets_value
        .map(|v| !v.is_null())
        .unwrap_or(false);

    let mut wrote_sheets_response = serde_json::Value::Null;
    if let (Some(target_id), Some(sheets_value)) = (
        parsed.write_to_spreadsheet.as_deref(),
        output_sheets_value.filter(|v| !v.is_null()),
    ) {
        let spreadsheet_id = crate::gsheets::domain::SpreadsheetId(target_id.to_string());
        let results = write_output_sheets(&client, &spreadsheet_id, sheets_value).await;
        wrote_sheets_response = serde_json::Value::Array(results);
    }

    // Warning when args + globals are misconfigured
    let warning: Option<&'static str> = match (has_target, has_sheets) {
        (true, false) => Some(
            "write_to_spreadsheet was set but the script did not assign \
             `output_sheets = {...}`; nothing was written.",
        ),
        (false, true) => Some(
            "Script assigned `output_sheets` but no `write_to_spreadsheet` arg \
             was provided; the result was discarded.",
        ),
        _ => None,
    };

    let (user_output_capped, output_truncated) = truncate_json(&user_output, OUTPUT_BYTE_CAP);
    let mut response = serde_json::json!({
        "output": user_output_capped,
        "stdout": truncate(&helper_result.stdout, STDOUT_BYTE_CAP),
        "error": serde_json::Value::Null,
        "wrote_sheets": wrote_sheets_response,
    });
    if output_truncated {
        response["_output_truncated"] = serde_json::json!(true);
    }
    if let Some(w) = warning {
        response["_warning"] = serde_json::Value::String(w.to_string());
    }
    response
}

// ── Helpers ──────────────────────────────────────────────────────────

/// Write each `(name, DataFrame)` entry from `output_sheets` as a new tab
/// in the target spreadsheet. Returns one metadata entry per write (success
/// or error). On name collision, retries with " (2)" ... " (10)".
async fn write_output_sheets(
    client: &Arc<dyn SheetsClient>,
    spreadsheet_id: &SpreadsheetId,
    output_sheets: &serde_json::Value,
) -> Vec<serde_json::Value> {
    use crate::gsheets::domain::CellValue;
    use crate::gsheets::domain::SheetsError;

    let Some(map) = output_sheets.as_object() else {
        return Vec::new();
    };
    let mut results: Vec<serde_json::Value> = Vec::new();
    for (raw_name, entry) in map {
        let records = entry.get("records").and_then(|v| v.as_array());
        let cols = entry.get("cols").and_then(|v| v.as_array());
        let (Some(records), Some(cols)) = (records, cols) else {
            results.push(serde_json::json!({
                "name": raw_name,
                "error": "entry missing records or cols",
            }));
            continue;
        };

        // Resolve a non-conflicting name: try raw_name, then "name (2)" ... "name (10)".
        let mut resolved_name: Option<String> = None;
        let mut sheet_id: Option<i64> = None;
        for attempt in 1i32..=10 {
            let candidate = if attempt == 1 {
                raw_name.clone()
            } else {
                format!("{raw_name} ({attempt})")
            };
            match client.add_sheet(spreadsheet_id, &candidate).await {
                Ok(meta) => {
                    resolved_name = Some(candidate);
                    sheet_id = Some(meta.sheet_id.0);
                    break;
                }
                Err(SheetsError::Http(msg)) if msg.to_lowercase().contains("already exists") => {
                    continue;
                }
                Err(e) => {
                    results.push(serde_json::json!({
                        "name": raw_name,
                        "error": format!("add_sheet failed: {e}"),
                    }));
                    break;
                }
            }
        }
        let (Some(resolved_name), Some(sheet_id)) = (resolved_name, sheet_id) else {
            // All 10 attempts collided or an error broke the loop early (already pushed).
            if !results.iter().any(|r| r.get("name") == Some(&serde_json::Value::String(raw_name.clone()))) {
                results.push(serde_json::json!({
                    "name": raw_name,
                    "error": "could not create tab: all 10 name attempts already exist",
                }));
            }
            continue;
        };

        // Build matrix: header row + body rows.
        let header_row: Vec<CellValue> = cols
            .iter()
            .map(|c| CellValue::String(c.as_str().unwrap_or("").to_string()))
            .collect();
        let mut matrix: Vec<Vec<CellValue>> = vec![header_row];
        for rec in records {
            let Some(obj) = rec.as_object() else {
                continue;
            };
            let row: Vec<CellValue> = cols
                .iter()
                .map(|c| {
                    let key = c.as_str().unwrap_or("");
                    match obj.get(key) {
                        Some(serde_json::Value::Null) | None => CellValue::Null,
                        Some(serde_json::Value::Bool(b)) => CellValue::Bool(*b),
                        Some(serde_json::Value::Number(n)) => n
                            .as_f64()
                            .map(CellValue::Number)
                            .unwrap_or(CellValue::Null),
                        Some(serde_json::Value::String(s)) => CellValue::String(s.clone()),
                        Some(other) => CellValue::String(other.to_string()),
                    }
                })
                .collect();
            matrix.push(row);
        }
        let n_rows = matrix.len();
        let n_cols = cols.len();

        match client
            .set_range(spreadsheet_id, &resolved_name, "A1", matrix)
            .await
        {
            Ok(_) => {
                results.push(serde_json::json!({
                    "name": raw_name,
                    "resolved_name": resolved_name,
                    "sheet_id": sheet_id,
                    "n_rows": n_rows,
                    "n_cols": n_cols,
                }));
            }
            Err(e) => {
                results.push(serde_json::json!({
                    "name": raw_name,
                    "resolved_name": resolved_name,
                    "error": format!("set_range failed: {e}"),
                }));
            }
        }
    }
    results
}

/// Wrap user code with the standard prelude (imports) and postlude
/// (collect `output`). Unlike `crdt_doc_run_python` we do NOT auto-build
/// `pd.DataFrame(...)` for each binding — the LLM picks the shape it
/// wants (DataFrame, dict-of-lists, raw list of dicts).
fn wrap_user_code(user_code: &str) -> String {
    format!("{GSHEETS_PY_PRELUDE}{user_code}{GSHEETS_PY_POSTLUDE}")
}

/// Pull column names off the first record when `as_records=true`. If
/// Google returned an empty body, fall back to an empty list — the
/// surface stays consistent for the error envelope.
fn extract_columns(values: &serde_json::Value) -> Vec<String> {
    match values {
        serde_json::Value::Array(arr) => arr
            .first()
            .and_then(|v| v.as_object())
            .map(|obj| obj.keys().cloned().collect())
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

fn truncate(s: &str, cap: usize) -> String {
    if s.len() <= cap {
        s.to_string()
    } else {
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

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path_regex};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    async fn mock_with_two_sheets() -> (MockServer, Arc<GoogleSheetsHttpClient>) {
        let server = MockServer::start().await;
        let client = GoogleSheetsHttpClient::for_tests(&server.uri(), &server.uri(), &server.uri());
        client.token_test_seed("fake-bearer-token").await;

        // Products!A1:C3 → records: [{sku, qty}, ...] (2 rows + header).
        Mock::given(method("GET"))
            .and(path_regex(r"/sp_p/values/Products"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "range": "Products!A1:C3",
                "majorDimension": "ROWS",
                "values": [
                    ["sku",   "qty"],
                    ["a-1",   10],
                    ["a-2",   20],
                ],
            })))
            .mount(&server)
            .await;

        // Sales!A1:C3 → records: [{sku, sold}, ...].
        Mock::given(method("GET"))
            .and(path_regex(r"/sp_s/values/Sales"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "range": "Sales!A1:C3",
                "majorDimension": "ROWS",
                "values": [
                    ["sku",   "sold"],
                    ["a-1",   3],
                    ["a-2",   7],
                ],
            })))
            .mount(&server)
            .await;

        (server, Arc::new(client))
    }

    #[tokio::test]
    async fn dispatch_runs_pandas_over_two_bindings_in_parallel() {
        pyo3::prepare_freethreaded_python();
        let (_server, client) = mock_with_two_sheets().await;
        let args = serde_json::json!({
            "bindings": [
                {"var": "products", "spreadsheet_id": "sp_p", "sheet": "Products"},
                {"var": "sales",    "spreadsheet_id": "sp_s", "sheet": "Sales"},
            ],
            // Intentionally NOT calling pd.DataFrame() so the test works in
            // environments where pandas isn't installed (CI Rust-only).
            // The auto-prelude still imports pandas — if pandas is missing
            // the wrapped code fails before reaching user code. We guard
            // against that by short-circuiting the assertion below when
            // the error is the well-known "No module named 'pandas'".
            "code": "output = {'rows_p': len(products), 'rows_s': len(sales)}",
        });
        let res =
            dispatch_gsheets_run_python_with_client(client as Arc<dyn SheetsClient>, args).await;
        // Skip the assertion if pandas isn't installed in the test
        // environment — the dispatcher wiring is what we're verifying;
        // pandas-vs-no-pandas is a sandbox-layer concern covered by
        // python_node's own tests.
        if let Some(err) = res.get("error").and_then(|v| v.as_str()) {
            if err.contains("No module named 'pandas'") {
                eprintln!("SKIPPED (no pandas in test env): {err}");
                return;
            }
            panic!("expected null error, got: {res}");
        }
        assert_eq!(res["output"]["rows_p"], 2);
        assert_eq!(res["output"]["rows_s"], 2);
    }

    #[tokio::test]
    async fn python_keyerror_returns_loaded_columns_for_self_correction() {
        pyo3::prepare_freethreaded_python();
        let (_server, client) = mock_with_two_sheets().await;
        let args = serde_json::json!({
            "bindings": [
                {"var": "products", "spreadsheet_id": "sp_p", "sheet": "Products"},
                {"var": "sales",    "spreadsheet_id": "sp_s", "sheet": "Sales"},
            ],
            // Wrong column name on purpose — pandas should raise KeyError,
            // and the response must include loaded_columns so the LLM can
            // see the real schema.
            "code": "df = pd.DataFrame(products); _ = df['THIS_COLUMN_DOES_NOT_EXIST']",
        });
        let res =
            dispatch_gsheets_run_python_with_client(client as Arc<dyn SheetsClient>, args).await;
        let err = res["error"].as_str().unwrap_or_default();
        assert!(!err.is_empty(), "expected an error string, got: {res}");
        let loaded = &res["loaded_columns"];
        assert!(
            loaded.is_object(),
            "expected loaded_columns object, got: {res}"
        );
        // The KeyError vs. ModuleNotFoundError distinction depends on
        // whether pandas is installed — either way the response MUST
        // carry the `loaded_columns` self-correction hint, which is the
        // contract under test.
        let prods = loaded["products"].as_array().unwrap();
        assert!(
            prods.iter().any(|v| v == "sku"),
            "expected 'sku' in loaded_columns.products, got: {loaded}"
        );
        let sales = loaded["sales"].as_array().unwrap();
        assert!(
            sales.iter().any(|v| v == "sold"),
            "expected 'sold' in loaded_columns.sales, got: {loaded}"
        );
    }

    #[tokio::test]
    async fn empty_bindings_returns_invalid_args() {
        let (_server, client) = mock_with_two_sheets().await;
        let args = serde_json::json!({ "bindings": [], "code": "output = 1" });
        let res =
            dispatch_gsheets_run_python_with_client(client as Arc<dyn SheetsClient>, args).await;
        assert_eq!(res["error"], "invalid_args");
    }

    #[tokio::test]
    async fn duplicate_binding_var_is_rejected() {
        let (_server, client) = mock_with_two_sheets().await;
        let args = serde_json::json!({
            "bindings": [
                {"var": "x", "spreadsheet_id": "sp_p", "sheet": "Products"},
                {"var": "x", "spreadsheet_id": "sp_s", "sheet": "Sales"},
            ],
            "code": "output = 1",
        });
        let res =
            dispatch_gsheets_run_python_with_client(client as Arc<dyn SheetsClient>, args).await;
        assert_eq!(res["error"], "invalid_args");
        assert!(res["message"]
            .as_str()
            .unwrap_or_default()
            .contains("duplicate"));
    }

    #[test]
    fn tool_def_has_summary() {
        let td = tool_gsheets_run_python();
        assert!(
            td.summary.is_some(),
            "gsheets_run_python must declare a summary for lazy_tool_loading"
        );
        let s = td.summary.unwrap();
        assert!(
            s.len() >= 10 && s.len() <= 200,
            "summary length out of bounds: {}",
            s.len()
        );
    }

    // ── multi-sheet write-back tests ──────────────────────────────────────

    /// Happy path: script sets `output_sheets` with 3 DataFrames and
    /// `write_to_spreadsheet` is supplied → 3 add_sheet + 3 set_range
    /// calls, `wrote_sheets` has 3 entries, no `error`.
    #[tokio::test]
    async fn multi_sheet_write_back_three_tabs() {
        pyo3::prepare_freethreaded_python();

        let server = MockServer::start().await;
        let client =
            GoogleSheetsHttpClient::for_tests(&server.uri(), &server.uri(), &server.uri());
        client.token_test_seed("fake-bearer-token").await;

        // Binding: Products tab on spreadsheet "ws_w".
        Mock::given(method("GET"))
            .and(path_regex(r"/ws_w/values/Products"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "range": "Products!A1:B3",
                "majorDimension": "ROWS",
                "values": [
                    ["sku", "qty"],
                    ["a-1", 10],
                    ["a-2", 20],
                ],
            })))
            .mount(&server)
            .await;

        // add_sheet (batchUpdate POST) calls — one per output tab.
        // No .expect() because the pandas-skip path exits before these
        // mocks are exercised; the assertion below checks the count instead.
        Mock::given(method("POST"))
            .and(path_regex(r"/ws_w:batchUpdate"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "replies": [{"addSheet": {"properties": {"sheetId": 1, "title": "Top10"}}}]
            })))
            .mount(&server)
            .await;

        // set_range (values PUT) calls — one per output tab.
        Mock::given(method("PUT"))
            .and(path_regex(r"/ws_w/values/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "updatedRange": "Top10!A1:B2",
                "updatedCells": 4,
            })))
            .mount(&server)
            .await;

        let args = serde_json::json!({
            "bindings": [{"var": "products", "spreadsheet_id": "ws_w", "sheet": "Products"}],
            "code": "\
import pandas as pd\n\
df = pd.DataFrame(products)\n\
output_sheets = {\n\
    'Top10':   df.head(1),\n\
    'Bottom5': df.tail(1),\n\
    'All':     df,\n\
}\n\
output = {'tabs_written': 3}\n\
",
            "write_to_spreadsheet": "ws_w"
        });

        let client: Arc<dyn SheetsClient> = Arc::new(client);
        let res =
            dispatch_gsheets_run_python_with_client(client, args).await;

        // Skip if pandas not installed in this test environment.
        if let Some(err) = res.get("error").and_then(|v| v.as_str()) {
            if err.contains("No module named 'pandas'") {
                eprintln!("SKIPPED (no pandas in test env): {err}");
                return;
            }
            panic!("unexpected error: {res}");
        }

        let wrote = res
            .get("wrote_sheets")
            .and_then(|v| v.as_array())
            .expect("wrote_sheets should be an array");
        assert_eq!(
            wrote.len(),
            3,
            "expected 3 wrote_sheets entries, got {}; full response: {res}",
            wrote.len()
        );
        // Each entry must have name + resolved_name (not an error entry).
        for entry in wrote {
            assert!(
                entry.get("resolved_name").is_some(),
                "entry missing resolved_name: {entry}"
            );
            assert!(
                entry.get("error").is_none(),
                "unexpected error in wrote_sheets entry: {entry}"
            );
        }
    }

    /// Collision retry: first add_sheet call returns 400 "already exists";
    /// dispatcher retries with " (2)" suffix and succeeds. The
    /// `wrote_sheets` entry must carry both `name` and `resolved_name`.
    #[tokio::test]
    async fn multi_sheet_collision_retries_with_suffix() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use wiremock::Respond;

        pyo3::prepare_freethreaded_python();

        let server = MockServer::start().await;
        let client =
            GoogleSheetsHttpClient::for_tests(&server.uri(), &server.uri(), &server.uri());
        client.token_test_seed("fake-bearer-token").await;

        // Binding: Whatever tab on spreadsheet "ws_c".
        Mock::given(method("GET"))
            .and(path_regex(r"/ws_c/values/Whatever"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "range": "Whatever!A1:B2",
                "majorDimension": "ROWS",
                "values": [
                    ["id", "val"],
                    [1, 99],
                ],
            })))
            .mount(&server)
            .await;

        // First POST returns 400 "already exists"; second returns 200.
        struct AddSheetResponder(AtomicUsize);
        impl Respond for AddSheetResponder {
            fn respond(&self, _req: &wiremock::Request) -> wiremock::ResponseTemplate {
                let attempt = self.0.fetch_add(1, Ordering::SeqCst);
                if attempt == 0 {
                    wiremock::ResponseTemplate::new(400).set_body_string(
                        r#"{"error":{"code":400,"message":"A sheet with the name \"Existing\" already exists. Please enter another name."}}"#,
                    )
                } else {
                    wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                        "replies": [{"addSheet": {"properties": {"sheetId": 7777, "title": "Existing (2)"}}}]
                    }))
                }
            }
        }

        // No .expect() — pandas-skip path exits before these mocks fire;
        // the assertions below verify the collision-retry contract instead.
        Mock::given(method("POST"))
            .and(path_regex(r"/ws_c:batchUpdate"))
            .respond_with(AddSheetResponder(AtomicUsize::new(0)))
            .mount(&server)
            .await;

        Mock::given(method("PUT"))
            .and(path_regex(r"/ws_c/values/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "updatedRange": "Existing (2)!A1:B2",
                "updatedCells": 4,
            })))
            .mount(&server)
            .await;

        let args = serde_json::json!({
            "bindings": [{"var": "x", "spreadsheet_id": "ws_c", "sheet": "Whatever"}],
            "code": "\
import pandas as pd\n\
df = pd.DataFrame(x)\n\
output_sheets = {'Existing': df}\n\
output = {}\n\
",
            "write_to_spreadsheet": "ws_c"
        });

        let client: Arc<dyn SheetsClient> = Arc::new(client);
        let res =
            dispatch_gsheets_run_python_with_client(client, args).await;

        // Skip if pandas not installed in this test environment.
        if let Some(err) = res.get("error").and_then(|v| v.as_str()) {
            if err.contains("No module named 'pandas'") {
                eprintln!("SKIPPED (no pandas in test env): {err}");
                return;
            }
            panic!("unexpected error: {res}");
        }

        let wrote = res
            .get("wrote_sheets")
            .and_then(|v| v.as_array())
            .expect("wrote_sheets should be an array");
        assert_eq!(wrote.len(), 1, "expected 1 wrote_sheets entry; full response: {res}");

        let entry = &wrote[0];
        assert_eq!(entry["name"], "Existing", "name mismatch: {entry}");
        assert_eq!(
            entry["resolved_name"],
            "Existing (2)",
            "resolved_name mismatch: {entry}"
        );
        assert!(
            entry.get("error").is_none(),
            "unexpected error in wrote_sheets entry: {entry}"
        );
    }
}
