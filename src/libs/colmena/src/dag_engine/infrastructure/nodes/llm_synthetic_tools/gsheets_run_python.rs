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

use super::diff_writer::diff_records;
use super::gsheets_tools::error_to_json;
use super::sheet_collision::{build_sheet_exists_error, parse_policy, CollisionPolicy, TabMeta};

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
    #[serde(deserialize_with = "deserialize_bindings_flexible")]
    pub bindings: Vec<GsheetsBinding>,
    /// Python code. Has access to `pandas as pd`, `numpy as np`,
    /// `scipy.stats as stats`, plus each binding's records list bound
    /// under its `var` name. Define `output` (any JSON-serializable
    /// value) — that is what the LLM sees.
    pub code: String,

    /// Optional target spreadsheet for `output_sheets` returned by the script.
    /// When set AND the script assigns `output_sheets = {name: DataFrame, ...}`,
    /// the dispatcher creates one new tab per entry and writes the DataFrame
    /// as a values matrix (header row + body). Collision behavior on existing
    /// tabs is controlled by `on_existing_sheet` (default: `fail` — returns a
    /// structured `SheetExists` error; set to `auto_suffix` for the legacy
    /// " (N)" behavior, or `overwrite` to clobber). When `write_to_spreadsheet`
    /// is omitted, any `output_sheets` produced by the script is ignored.
    #[serde(default)]
    pub write_to_spreadsheet: Option<String>,

    /// LLM-tolerance shim: some models pass `output_sheets` here, but
    /// `output_sheets` is actually a Python-global variable the code
    /// assigns. We accept any value here and discard it, emitting a
    /// `_warning` in the response so the LLM learns the correct usage.
    #[serde(default)]
    pub output_sheets: Option<serde_json::Value>,

    /// Operator-supplied collision policy. Default `fail`. Wired via
    /// `fixed_config.on_existing_sheet`. Accepted values: `fail`,
    /// `auto_suffix`, `overwrite`.
    #[serde(default, alias = "on_existing_sheet")]
    pub on_existing_sheet: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GsheetsBinding {
    /// Python variable name to bind the records to (e.g. "products",
    /// "sales"). Must be a non-empty identifier-shaped string.
    /// Accepts UX aliases `binding_name` and `name` because LLMs (especially
    /// Gemini-2.5-pro) often hallucinate these alternate field names instead
    /// of `var`.
    #[serde(alias = "binding_name", alias = "name")]
    pub var: String,
    /// Spreadsheet ID (the chunk between `/d/` and `/edit` in the
    /// Google Sheets URL).
    pub spreadsheet_id: String,
    /// Tab name (e.g. "Sheet1", "Hoja 1"). Accepts UX alias `sheet_name`.
    #[serde(alias = "sheet_name")]
    pub sheet: String,
    /// Optional A1 range (e.g. "A1:H5001"). Omit to read the entire tab.
    #[serde(default)]
    pub range: Option<String>,
}

// ── Flexible bindings deserializer ───────────────────────────────────

/// Custom deserializer for `bindings` that accepts either:
///
/// 1. The canonical array form:
///    `[{"var":"products","spreadsheet_id":"abc","sheet":"products"}, ...]`
///
/// 2. An object whose VALUES are binding-shaped objects (treat the
///    key as `var`):
///    `{"products": {"spreadsheet_id":"abc","sheet_name":"products"}, ...}`
///
/// LLMs (Gemini-2.5-pro in particular) frequently emit form 2 despite
/// the schema declaring `Vec<GsheetsBinding>`. This deserializer
/// tolerates both shapes so the demo doesn't burn 10+ turns
/// retrying. Form 3 (values are bare strings — ambiguous sheet name)
/// is rejected with a clear "specify sheet too" error.
fn deserialize_bindings_flexible<'de, D>(deserializer: D) -> Result<Vec<GsheetsBinding>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::{self, MapAccess, SeqAccess, Visitor};
    use std::fmt;

    struct BindingsVisitor;

    impl<'de> Visitor<'de> for BindingsVisitor {
        type Value = Vec<GsheetsBinding>;

        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("an array of bindings or an object whose values are bindings")
        }

        // Canonical array form
        fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
        where
            A: SeqAccess<'de>,
        {
            let mut out: Vec<GsheetsBinding> = Vec::new();
            while let Some(elem) = seq.next_element::<GsheetsBinding>()? {
                out.push(elem);
            }
            Ok(out)
        }

        // LLM-hallucinated dict form
        fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
        where
            A: MapAccess<'de>,
        {
            let mut out: Vec<GsheetsBinding> = Vec::new();
            while let Some((key, value)) = map.next_entry::<String, serde_json::Value>()? {
                match value {
                    // Form 2: value is binding-shaped object. Merge in `var` from key.
                    serde_json::Value::Object(mut obj) => {
                        obj.insert("var".to_string(), serde_json::Value::String(key.clone()));
                        let binding: GsheetsBinding =
                            serde_json::from_value(serde_json::Value::Object(obj))
                                .map_err(de::Error::custom)?;
                        out.push(binding);
                    }
                    // Form 3: value is just a string. Ambiguous — reject.
                    serde_json::Value::String(_) => {
                        return Err(de::Error::custom(format!(
                            "`bindings` entry '{key}' is a bare string; \
                             cannot determine sheet name. Use the array form: \
                             [{{\"var\":\"{key}\",\"spreadsheet_id\":\"...\",\"sheet\":\"...\"}}]"
                        )));
                    }
                    _ => {
                        return Err(de::Error::custom(format!(
                            "`bindings` entry '{key}' has unexpected value type; \
                             expected an object with spreadsheet_id + sheet"
                        )));
                    }
                }
            }
            Ok(out)
        }
    }

    deserializer.deserialize_any(BindingsVisitor)
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
    // Capture early whether the LLM mistakenly passed `output_sheets` as a
    // tool arg (it is a Python-global the code assigns, not a tool param).
    let output_sheets_arg_provided = parsed.output_sheets.is_some();
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
    let has_sheets = output_sheets_value.map(|v| !v.is_null()).unwrap_or(false);

    let mut wrote_sheets_response = serde_json::Value::Null;
    if let (Some(target_id), Some(sheets_value)) = (
        parsed.write_to_spreadsheet.as_deref(),
        output_sheets_value.filter(|v| !v.is_null()),
    ) {
        let spreadsheet_id = crate::gsheets::domain::SpreadsheetId(target_id.to_string());
        let policy = parsed
            .on_existing_sheet
            .as_deref()
            .map(parse_policy)
            .transpose()
            .unwrap_or_else(|e| {
                eprintln!("[gsheets] invalid on_existing_sheet, defaulting to fail: {e}");
                None
            })
            .unwrap_or_default();
        let results = write_output_sheets(&client, &spreadsheet_id, sheets_value, policy).await;
        wrote_sheets_response = serde_json::Value::Array(results);
    }

    // Warning when args + globals are misconfigured
    let config_warning: Option<&'static str> = match (has_target, has_sheets) {
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

    // Warn if the LLM mistakenly passed `output_sheets` as a tool arg.
    let arg_warning: Option<&'static str> = if output_sheets_arg_provided {
        Some(
            "You passed `output_sheets` as a tool argument; it was ignored. \
             `output_sheets` is a Python global your CODE assigns (e.g. \
             `output_sheets = {'tab1': df1}` inside the code string).",
        )
    } else {
        None
    };

    // Merge both warnings into one string when both are present.
    let final_warning: Option<String> = match (config_warning, arg_warning) {
        (Some(a), Some(b)) => Some(format!("{a} | {b}")),
        (Some(a), None) => Some(a.to_string()),
        (None, Some(b)) => Some(b.to_string()),
        (None, None) => None,
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
    if let Some(w) = final_warning {
        response["_warning"] = serde_json::Value::String(w);
    }
    response
}

// ── Helpers ──────────────────────────────────────────────────────────

/// Dispatch each normalized `output_sheets` entry by mode. Returns one
/// metadata entry per attempted write.
async fn write_output_sheets(
    client: &Arc<dyn SheetsClient>,
    spreadsheet_id: &SpreadsheetId,
    output_sheets: &serde_json::Value,
    policy: CollisionPolicy,
) -> Vec<serde_json::Value> {
    let Some(map) = output_sheets.as_object() else {
        return Vec::new();
    };
    let mut results: Vec<serde_json::Value> = Vec::new();
    for (raw_name, entry) in map {
        // Surface postlude-side errors as-is.
        if let Some(err) = entry.get("_postlude_error").and_then(|v| v.as_str()) {
            results.push(serde_json::json!({"name": raw_name, "error": err}));
            continue;
        }
        let mode = entry
            .get("mode")
            .and_then(|v| v.as_str())
            .unwrap_or("replace");
        match mode {
            "update_in_place" => {
                results.push(do_update_in_place(client, spreadsheet_id, raw_name, entry).await);
            }
            "overwrite" => {
                results.push(do_overwrite(client, spreadsheet_id, raw_name, entry).await);
            }
            "replace" => {
                results.push(do_replace(client, spreadsheet_id, raw_name, entry, policy).await);
            }
            other => {
                results.push(serde_json::json!({
                    "name": raw_name,
                    "error": format!("unknown mode '{other}'; valid: replace, update_in_place, overwrite"),
                }));
            }
        }
    }
    results
}

/// Mode `replace`: create tab and write full DataFrame. Apply policy on collision.
async fn do_replace(
    client: &Arc<dyn SheetsClient>,
    spreadsheet_id: &SpreadsheetId,
    raw_name: &str,
    entry: &serde_json::Value,
    policy: CollisionPolicy,
) -> serde_json::Value {
    let exists = match fetch_tab_meta(client, spreadsheet_id, raw_name).await {
        Ok(opt) => opt,
        Err(e) => {
            return serde_json::json!({
                "name": raw_name,
                "error": format!("metadata fetch failed: {e}"),
            });
        }
    };
    if let Some(meta) = exists {
        match policy {
            CollisionPolicy::Fail => {
                return build_sheet_exists_error(raw_name, Some(&spreadsheet_id.0), &meta);
            }
            CollisionPolicy::Overwrite => {
                // Use the existing tab name as-is (Sheets API set_range on
                // existing tab replaces contents from A1 down).
                return write_full_df(client, spreadsheet_id, raw_name, raw_name, entry).await;
            }
            CollisionPolicy::AutoSuffix => {
                // Start at 2: raw_name itself is already known to collide (we just
                // hit `Some(meta)` above). First candidate is "raw_name (2)".
                for attempt in 2i32..=10 {
                    let candidate = format!("{raw_name} ({attempt})");
                    match client.add_sheet(spreadsheet_id, &candidate).await {
                        Ok(_) => {
                            return write_full_df(
                                client,
                                spreadsheet_id,
                                raw_name,
                                &candidate,
                                entry,
                            )
                            .await;
                        }
                        Err(crate::gsheets::domain::SheetsError::Http(msg))
                            if msg.to_lowercase().contains("already exists") =>
                        {
                            continue;
                        }
                        Err(e) => {
                            return serde_json::json!({
                                "name": raw_name,
                                "error": format!("add_sheet failed: {e}"),
                            });
                        }
                    }
                }
                return serde_json::json!({
                    "name": raw_name,
                    "error": "auto_suffix: all 10 name attempts already exist",
                });
            }
        }
    }
    // Doesn't exist — create + write.
    match client.add_sheet(spreadsheet_id, raw_name).await {
        Ok(_) => write_full_df(client, spreadsheet_id, raw_name, raw_name, entry).await,
        Err(e) => serde_json::json!({
            "name": raw_name,
            "error": format!("add_sheet failed: {e}"),
        }),
    }
}

/// Mode `overwrite`: replace contents of existing tab. Creates if absent.
async fn do_overwrite(
    client: &Arc<dyn SheetsClient>,
    spreadsheet_id: &SpreadsheetId,
    raw_name: &str,
    entry: &serde_json::Value,
) -> serde_json::Value {
    // Schema-change guard: if the tab exists and its columns differ from
    // the input's columns, reject unless allow_schema_change=true.
    match fetch_tab_meta(client, spreadsheet_id, raw_name).await {
        Err(e) => {
            return serde_json::json!({
                "name": raw_name,
                "error": format!("metadata fetch failed: {e}"),
            });
        }
        Ok(None) => {
            // Tab doesn't exist — fall through to write_full_df below.
        }
        Ok(Some(meta)) => {
            let input_cols: Vec<String> = entry
                .get("df_cols")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|c| c.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            let allow = entry
                .get("allow_schema_change")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let mismatch = !columns_match(&meta.columns, &input_cols);
            if mismatch && !allow {
                return serde_json::json!({
                    "name": raw_name,
                    "error": "SchemaChange",
                    "current_columns": meta.columns,
                    "input_columns": input_cols,
                    "message": format!(
                        "Overwriting '{raw_name}' would change its schema: current {:?} → new {:?}. \
                         This is likely a mistake. RECOMMENDED: use a different tab name. To proceed \
                         anyway, add 'allow_schema_change: true' to the spec dict.",
                        meta.columns, input_cols
                    ),
                });
            }
        }
    }
    write_full_df(client, spreadsheet_id, raw_name, raw_name, entry).await
}

/// Mode `update_in_place`: diff and apply only changed cells.
async fn do_update_in_place(
    client: &Arc<dyn SheetsClient>,
    spreadsheet_id: &SpreadsheetId,
    raw_name: &str,
    entry: &serde_json::Value,
) -> serde_json::Value {
    let Some(key) = entry.get("key").and_then(|v| v.as_str()) else {
        return serde_json::json!({
            "tab": raw_name,
            "error": "update_in_place requires `key` field in the spec dict",
        });
    };
    let restrict: Option<Vec<String>> = entry.get("columns").and_then(|v| v.as_array()).map(|a| {
        a.iter()
            .filter_map(|c| c.as_str().map(String::from))
            .collect()
    });
    let strict = entry
        .get("strict_match")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let new_records: Vec<serde_json::Map<String, serde_json::Value>> = entry
        .get("df_records")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|r| r.as_object().cloned()).collect())
        .unwrap_or_default();

    // Fetch current rows AND header column order from the tab.
    let read = match client
        .read_range(
            spreadsheet_id,
            raw_name,
            None,
            ReadOptions {
                value_render: crate::gsheets::domain::ValueRenderOption::UnformattedValue,
                as_records: true,
            },
        )
        .await
    {
        Ok(r) => r,
        Err(crate::gsheets::domain::SheetsError::SheetNotFound(_)) => {
            return serde_json::json!({
                "name": raw_name,
                "error": "UpdateRequiresExistingTab",
                "message": format!(
                    "update_in_place needs the tab '{raw_name}' to exist. Use mode=replace to create it."
                ),
            });
        }
        Err(e) => {
            return serde_json::json!({
                "tab": raw_name,
                "error": format!("read failed: {e}"),
            });
        }
    };
    let current_records: Vec<serde_json::Map<String, serde_json::Value>> = match read.values {
        serde_json::Value::Array(a) => a
            .into_iter()
            .filter_map(|v| match v {
                serde_json::Value::Object(o) => Some(o),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    };
    // Header order (for A1 mapping) — re-read header row directly so column order is preserved.
    let header_read = match client
        .read_range(
            spreadsheet_id,
            raw_name,
            Some("A1:Z1"),
            ReadOptions {
                value_render: crate::gsheets::domain::ValueRenderOption::UnformattedValue,
                as_records: false,
            },
        )
        .await
    {
        Ok(r) => r,
        Err(e) => {
            return serde_json::json!({
                "tab": raw_name,
                "error": format!("header read failed: {e}"),
            });
        }
    };
    let header_cols: Vec<String> = match header_read.values {
        serde_json::Value::Array(rows) => rows
            .first()
            .and_then(|r| r.as_array())
            .map(|cells| {
                cells
                    .iter()
                    .map(|c| c.as_str().unwrap_or("").to_string())
                    .collect()
            })
            .unwrap_or_default(),
        _ => Vec::new(),
    };

    let diff = match diff_records(
        &current_records,
        &new_records,
        key,
        restrict.as_deref(),
        strict,
        raw_name,
    ) {
        Ok(d) => d,
        Err(e) => return e.to_json(),
    };

    // Map key_value → row index (1-based, row 1 is header → first data is row 2).
    let mut key_to_row: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for (i, r) in current_records.iter().enumerate() {
        if let Some(k) = r.get(key).and_then(|v| match v {
            serde_json::Value::Null => None,
            serde_json::Value::String(s) => Some(s.clone()),
            serde_json::Value::Number(n) => Some(n.to_string()),
            serde_json::Value::Bool(b) => Some(b.to_string()),
            _ => None,
        }) {
            key_to_row.insert(k, i + 2);
        }
    }
    let mut col_to_index: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    for (i, c) in header_cols.iter().enumerate() {
        col_to_index.insert(c.clone(), i);
    }

    let mut cell_updates: Vec<(String, crate::gsheets::domain::CellValue)> = Vec::new();
    for chg in &diff.changes {
        let (Some(row), Some(col_idx)) = (
            key_to_row.get(&chg.key_value),
            col_to_index.get(&chg.column),
        ) else {
            continue;
        };
        let addr = a1_addr(*col_idx, *row);
        cell_updates.push((
            addr,
            crate::gsheets::domain::CellValue::from_json(&chg.new_value),
        ));
    }

    let cells_to_write = cell_updates.len();
    if cells_to_write == 0 {
        return serde_json::json!({
            "tab": raw_name,
            "mode": "update_in_place",
            "changes": {"rows": 0, "cells": 0, "columns": diff.columns_touched},
            "unchanged": {"rows": diff.rows_unchanged},
            "skipped": {
                "rows_not_in_target": diff.rows_skipped_not_in_target,
                "rows_null_key": diff.rows_skipped_null_key,
            },
        });
    }

    match client
        .batch_update_cells(spreadsheet_id, raw_name, cell_updates)
        .await
    {
        Ok(_) => serde_json::json!({
            "tab": raw_name,
            "mode": "update_in_place",
            "changes": {
                "rows": diff.rows_changed,
                "cells": cells_to_write,
                "columns": diff.columns_touched,
            },
            "unchanged": {"rows": diff.rows_unchanged},
            "skipped": {
                "rows_not_in_target": diff.rows_skipped_not_in_target,
                "rows_null_key": diff.rows_skipped_null_key,
            },
        }),
        Err(e) => serde_json::json!({
            "tab": raw_name,
            "error": format!("batch_update_cells failed: {e}"),
        }),
    }
}

/// Common DataFrame write — used by `replace`, `overwrite`, and the
/// auto_suffix paths. `name` is what gets written to. `raw_name` is what
/// the LLM asked for (for response labeling).
async fn write_full_df(
    client: &Arc<dyn SheetsClient>,
    spreadsheet_id: &SpreadsheetId,
    raw_name: &str,
    name: &str,
    entry: &serde_json::Value,
) -> serde_json::Value {
    use crate::gsheets::domain::CellValue;
    let records = entry.get("df_records").and_then(|v| v.as_array());
    let cols = entry.get("df_cols").and_then(|v| v.as_array());
    let (Some(records), Some(cols)) = (records, cols) else {
        return serde_json::json!({
            "name": raw_name,
            "error": "entry missing df_records or df_cols",
        });
    };
    let header_row: Vec<CellValue> = cols
        .iter()
        .map(|c| CellValue::String(c.as_str().unwrap_or("").to_string()))
        .collect();
    let mut matrix: Vec<Vec<CellValue>> = vec![header_row];
    for rec in records {
        let Some(obj) = rec.as_object() else { continue };
        let row: Vec<CellValue> = cols
            .iter()
            .map(|c| {
                let key = c.as_str().unwrap_or("");
                CellValue::from_json(obj.get(key).unwrap_or(&serde_json::Value::Null))
            })
            .collect();
        matrix.push(row);
    }
    let n_rows = matrix.len();
    let n_cols = cols.len();
    match client.set_range(spreadsheet_id, name, "A1", matrix).await {
        Ok(_) => serde_json::json!({
            "name": raw_name,
            "resolved_name": name,
            "n_rows": n_rows,
            "n_cols": n_cols,
        }),
        Err(e) => serde_json::json!({
            "name": raw_name,
            "resolved_name": name,
            "error": format!("set_range failed: {e}"),
        }),
    }
}

/// Returns true if two column lists name the same set. Order-insensitive
/// (Sheets column order is not part of the schema contract) and
/// duplicate-insensitive (HashSet semantics). Used by the overwrite
/// schema-change guard.
fn columns_match(a: &[String], b: &[String]) -> bool {
    let sa: std::collections::HashSet<&String> = a.iter().collect();
    let sb: std::collections::HashSet<&String> = b.iter().collect();
    sa == sb
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

/// Fetch lightweight metadata for the target tab: row/col count from
/// list_sheets + column names from the header row.
async fn fetch_tab_meta(
    client: &Arc<dyn SheetsClient>,
    spreadsheet_id: &SpreadsheetId,
    tab: &str,
) -> Result<Option<TabMeta>, crate::gsheets::domain::SheetsError> {
    let sheets = client.list_sheets(spreadsheet_id).await?;
    let Some(meta) = sheets.into_iter().find(|s| s.title == tab) else {
        return Ok(None);
    };
    // Header row (A1:Z1) — small read, gives us real column names.
    let read = client
        .read_range(
            spreadsheet_id,
            tab,
            Some("A1:Z1"),
            ReadOptions {
                value_render: crate::gsheets::domain::ValueRenderOption::UnformattedValue,
                as_records: false,
            },
        )
        .await?;
    let columns: Vec<String> = match read.values {
        serde_json::Value::Array(rows) => rows
            .first()
            .and_then(|r| r.as_array())
            .map(|cells| {
                cells
                    .iter()
                    .map(|c| c.as_str().unwrap_or("").to_string())
                    .collect()
            })
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    Ok(Some(TabMeta {
        n_rows: meta.row_count as u64,
        n_cols: meta.col_count as u64,
        columns,
    }))
}

/// Convert column letter+row index to A1 string for `batch_update_cells`.
/// `row_index` is 1-based, where row 1 is the header.
fn a1_addr(col_index: usize, row_index: usize) -> String {
    let mut col = String::new();
    let mut n = col_index;
    loop {
        col.insert(0, (b'A' + (n % 26) as u8) as char);
        n /= 26;
        if n == 0 {
            break;
        }
        n -= 1;
    }
    format!("{col}{row_index}")
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

    // ── UX alias tests (E-T22) ────────────────────────────────────────────

    #[test]
    fn binding_accepts_binding_name_alias() {
        let json = serde_json::json!({
            "binding_name": "products",
            "spreadsheet_id": "abc",
            "sheet_name": "products"
        });
        let parsed: GsheetsBinding = serde_json::from_value(json).expect("must parse with aliases");
        assert_eq!(parsed.var, "products");
        assert_eq!(parsed.sheet, "products");
    }

    #[test]
    fn binding_accepts_name_alias() {
        let json = serde_json::json!({
            "name": "sales",
            "spreadsheet_id": "xyz",
            "sheet": "Sales"
        });
        let parsed: GsheetsBinding =
            serde_json::from_value(json).expect("must parse with `name` alias");
        assert_eq!(parsed.var, "sales");
    }

    #[test]
    fn binding_canonical_var_still_works() {
        let json = serde_json::json!({
            "var": "x",
            "spreadsheet_id": "abc",
            "sheet": "Sheet1"
        });
        let parsed: GsheetsBinding =
            serde_json::from_value(json).expect("canonical names still work");
        assert_eq!(parsed.var, "x");
        assert_eq!(parsed.sheet, "Sheet1");
    }

    #[test]
    fn args_accept_output_sheets_as_tool_arg_silently() {
        // The LLM mistakenly passes `output_sheets` as a tool arg. The args
        // struct must deserialize without erroring; the dispatcher response
        // surfaces a warning (verified by a separate dispatcher integration
        // test in the wiremock suite if desired).
        let json = serde_json::json!({
            "bindings": [{"var": "x", "spreadsheet_id": "a", "sheet": "S"}],
            "code": "output = {}",
            "output_sheets": {"source": "LAST_EXPRESSION"}
        });
        let parsed: GsheetsRunPythonArgs = serde_json::from_value(json).expect("args must parse");
        assert!(parsed.output_sheets.is_some());
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

    // ── E-T22b: flexible bindings deserializer tests ──────────────────────

    #[test]
    fn bindings_canonical_array_form_works() {
        let json = serde_json::json!({
            "bindings": [
                {"var": "products", "spreadsheet_id": "abc", "sheet": "products"},
                {"var": "sales", "spreadsheet_id": "xyz", "sheet": "sales"}
            ],
            "code": "output = {}"
        });
        let parsed: GsheetsRunPythonArgs = serde_json::from_value(json).unwrap();
        assert_eq!(parsed.bindings.len(), 2);
        assert_eq!(parsed.bindings[0].var, "products");
        assert_eq!(parsed.bindings[1].var, "sales");
    }

    #[test]
    fn bindings_dict_form_accepted_with_objects() {
        let json = serde_json::json!({
            "bindings": {
                "products": {"spreadsheet_id": "abc", "sheet": "products"},
                "sales":    {"spreadsheet_id": "xyz", "sheet_name": "sales"}
            },
            "code": "output = {}"
        });
        let parsed: GsheetsRunPythonArgs =
            serde_json::from_value(json).expect("dict form must parse");
        assert_eq!(parsed.bindings.len(), 2);
        let names: std::collections::HashSet<String> =
            parsed.bindings.iter().map(|b| b.var.clone()).collect();
        assert!(names.contains("products"));
        assert!(names.contains("sales"));
    }

    #[test]
    fn bindings_dict_with_string_value_rejected() {
        let json = serde_json::json!({
            "bindings": {
                "products": "abc"
            },
            "code": "output = {}"
        });
        let err = serde_json::from_value::<GsheetsRunPythonArgs>(json).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("bare string") || msg.contains("cannot determine sheet"),
            "unexpected error message: {msg}"
        );
    }

    #[test]
    fn bindings_dict_form_works_with_binding_name_alias() {
        // Combined test: dict form + the `binding_name` alias inside each value
        let json = serde_json::json!({
            "bindings": {
                "products": {"spreadsheet_id": "abc", "sheet_name": "products"}
            },
            "code": "output = {}"
        });
        let parsed: GsheetsRunPythonArgs = serde_json::from_value(json).expect("must parse");
        assert_eq!(parsed.bindings.len(), 1);
        assert_eq!(parsed.bindings[0].var, "products");
        assert_eq!(parsed.bindings[0].sheet, "products");
        assert_eq!(parsed.bindings[0].spreadsheet_id, "abc");
    }

    // ── multi-sheet write-back tests ──────────────────────────────────────

    /// Happy path: script sets `output_sheets` with 3 DataFrames and
    /// `write_to_spreadsheet` is supplied → 3 add_sheet + 3 set_range
    /// calls, `wrote_sheets` has 3 entries, no `error`.
    #[tokio::test]
    async fn multi_sheet_write_back_three_tabs() {
        pyo3::prepare_freethreaded_python();

        let server = MockServer::start().await;
        let client = GoogleSheetsHttpClient::for_tests(&server.uri(), &server.uri(), &server.uri());
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
        let res = dispatch_gsheets_run_python_with_client(client, args).await;

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
        let client = GoogleSheetsHttpClient::for_tests(&server.uri(), &server.uri(), &server.uri());
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
        let res = dispatch_gsheets_run_python_with_client(client, args).await;

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
            1,
            "expected 1 wrote_sheets entry; full response: {res}"
        );

        let entry = &wrote[0];
        assert_eq!(entry["name"], "Existing", "name mismatch: {entry}");
        assert_eq!(
            entry["resolved_name"], "Existing (2)",
            "resolved_name mismatch: {entry}"
        );
        assert!(
            entry.get("error").is_none(),
            "unexpected error in wrote_sheets entry: {entry}"
        );
    }

    // ── update_in_place mode tests ────────────────────────────────────

    #[tokio::test]
    async fn update_in_place_writes_only_changed_cells() {
        pyo3::prepare_freethreaded_python();

        let server = wiremock::MockServer::start().await;
        let client = GoogleSheetsHttpClient::for_tests(&server.uri(), &server.uri(), &server.uri());
        client.token_test_seed("fake-token").await;

        // (a) read_range mock — returns current "Sales" with 3 rows.
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path_regex(r"/ss_u/values/Sales"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "range": "Sales!A1:C4",
                    "majorDimension": "ROWS",
                    "values": [
                        ["id", "price"],
                        ["a",  10],
                        ["b",  20],
                        ["c",  30],
                    ],
                })),
            )
            .mount(&server)
            .await;

        // (b) batchUpdate (cells) mock — one call with 2 cell updates
        //     (rows b & c, column price). No .expect() because the
        //     pandas-skip path exits before this mock is exercised.
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path_regex(r"/ss_u/values:batchUpdate"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "totalUpdatedCells": 2,
                })),
            )
            .mount(&server)
            .await;

        let args = serde_json::json!({
            "bindings": [{"var": "sales", "spreadsheet_id": "ss_u", "sheet": "Sales"}],
            "code": "\
        import pandas as pd\n\
        df = pd.DataFrame(sales)\n\
        df.loc[df['id'].isin(['b','c']), 'price'] = 99\n\
        output_sheets = {'Sales': {'mode': 'update_in_place', 'df': df, 'key': 'id'}}\n\
        output = {}\n\
        ",
            "write_to_spreadsheet": "ss_u"
        });

        let client: Arc<dyn SheetsClient> = Arc::new(client);
        let res = dispatch_gsheets_run_python_with_client(client, args).await;

        if let Some(err) = res.get("error").and_then(|v| v.as_str()) {
            if err.contains("No module named 'pandas'") {
                eprintln!("SKIPPED (no pandas): {err}");
                return;
            }
            panic!("unexpected error: {res}");
        }
        let wrote = res
            .get("wrote_sheets")
            .and_then(|v| v.as_array())
            .expect("wrote_sheets must be array");
        assert_eq!(wrote.len(), 1, "expected 1 wrote_sheets entry; got: {res}");
        let entry = &wrote[0];
        assert_eq!(entry["mode"], "update_in_place");
        assert_eq!(entry["changes"]["rows"], 2);
        assert_eq!(entry["changes"]["cells"], 2);
        assert_eq!(entry["unchanged"]["rows"], 1);
    }

    #[tokio::test]
    async fn replace_mode_default_fail_returns_sheet_exists() {
        pyo3::prepare_freethreaded_python();

        let server = wiremock::MockServer::start().await;
        let client = GoogleSheetsHttpClient::for_tests(&server.uri(), &server.uri(), &server.uri());
        client.token_test_seed("fake-token").await;

        // list_sheets — "Existing" already there.
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path_regex(r"/ss_f\?"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "sheets": [{
                        "properties": {
                            "sheetId": 7, "title": "Existing", "index": 0,
                            "gridProperties": {"rowCount": 100, "columnCount": 3}
                        }
                    }]
                })),
            )
            .mount(&server)
            .await;

        // read_range (for header columns) — used by fail-error builder.
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path_regex(r"/ss_f/values/Existing"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "range": "Existing!A1:Z1",
                    "majorDimension": "ROWS",
                    "values": [["a","b","c"]],
                })),
            )
            .mount(&server)
            .await;

        // NO batchUpdate, NO add_sheet, NO set_range calls expected.
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(wiremock::ResponseTemplate::new(500))
            .expect(0)
            .mount(&server)
            .await;

        let args = serde_json::json!({
            "bindings": [{"var": "x", "spreadsheet_id": "ss_f", "sheet": "Existing"}],
            "code": "\
        import pandas as pd\n\
        df = pd.DataFrame(x)\n\
        output_sheets = {'Existing': df}\n\
        output = {}\n\
        ",
            "write_to_spreadsheet": "ss_f"
        });

        let client: Arc<dyn SheetsClient> = Arc::new(client);
        let res = dispatch_gsheets_run_python_with_client(client, args).await;

        if let Some(err) = res.get("error").and_then(|v| v.as_str()) {
            if err.contains("No module named 'pandas'") {
                eprintln!("SKIPPED (no pandas): {err}");
                return;
            }
        }
        let wrote = res
            .get("wrote_sheets")
            .and_then(|v| v.as_array())
            .expect("wrote_sheets must be array");
        assert_eq!(wrote.len(), 1);
        assert_eq!(wrote[0]["error"], "SheetExists");
        let advice = wrote[0]["advice"].as_str().unwrap();
        assert!(advice.contains("Existing"));
    }

    #[tokio::test]
    async fn auto_suffix_policy_preserves_old_behavior() {
        // With fixed_config.on_existing_sheet="auto_suffix", colliding
        // "Existing" should write to "Existing (2)" silently — same as
        // pre-spec behavior.
        pyo3::prepare_freethreaded_python();

        let server = wiremock::MockServer::start().await;
        let client = GoogleSheetsHttpClient::for_tests(&server.uri(), &server.uri(), &server.uri());
        client.token_test_seed("fake-token").await;

        // Binding read — provides the Python var `x` to the sandbox.
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path_regex(r"/ss_a/values/Existing"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "range": "Existing!A1:B2",
                    "majorDimension": "ROWS",
                    "values": [
                        ["id", "val"],
                        [1, 99],
                    ],
                })),
            )
            .mount(&server)
            .await;

        // list_sheets — "Existing" already there (triggers collision check).
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path_regex(r"/ss_a\?"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "sheets": [{"properties": {"sheetId": 1, "title": "Existing", "index": 0,
                        "gridProperties": {"rowCount": 1, "columnCount": 1}}}]
                })),
            )
            .mount(&server)
            .await;

        // Header read (A1:Z1) — needed by fetch_tab_meta to get column names.
        // The path_regex for /ss_a/values/Existing above matches both the
        // binding read and the header read; wiremock uses the first matching
        // mock so this is OK — both return the same rows/header shape.

        // add_sheet for "Existing (2)".
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path_regex(r"/ss_a:batchUpdate"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "replies": [{"addSheet": {"properties": {"sheetId": 9, "title": "Existing (2)"}}}]
            })))
            .mount(&server)
            .await;

        // set_range write to "Existing (2)".
        wiremock::Mock::given(wiremock::matchers::method("PUT"))
            .and(wiremock::matchers::path_regex(r"/ss_a/values/"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "updatedRange": "Existing (2)!A1:B2",
                    "updatedCells": 4,
                })),
            )
            .mount(&server)
            .await;

        let args = serde_json::json!({
            "bindings": [{"var": "x", "spreadsheet_id": "ss_a", "sheet": "Existing"}],
            "code": "\
        import pandas as pd\n\
        df = pd.DataFrame(x)\n\
        output_sheets = {'Existing': df}\n\
        output = {}\n\
        ",
            "write_to_spreadsheet": "ss_a",
            "on_existing_sheet": "auto_suffix"
        });

        let client: Arc<dyn SheetsClient> = Arc::new(client);
        let res = dispatch_gsheets_run_python_with_client(client, args).await;

        if let Some(err) = res.get("error").and_then(|v| v.as_str()) {
            if err.contains("No module named 'pandas'") {
                eprintln!("SKIPPED (no pandas): {err}");
                return;
            }
            panic!("unexpected error: {res}");
        }
        let wrote = res["wrote_sheets"].as_array().expect("array");
        assert_eq!(wrote[0]["resolved_name"], "Existing (2)");
    }
}
