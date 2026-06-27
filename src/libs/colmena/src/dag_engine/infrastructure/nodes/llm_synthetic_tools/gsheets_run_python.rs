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
use crate::gsheets::infrastructure::http_client::{rectangle_to_records, GoogleSheetsHttpClient};
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
    /// Python variable name to bind the records to (e.g. "products").
    /// Accepts UX aliases `binding_name` and `name`.
    #[serde(alias = "binding_name", alias = "name")]
    pub var: String,
    /// Spreadsheet ID. Required for a SHEET binding; omit for an inline `data`
    /// binding.
    #[serde(default)]
    pub spreadsheet_id: Option<String>,
    /// Tab name. Required for a SHEET binding; omit for an inline `data` binding.
    #[serde(default, alias = "sheet_name")]
    pub sheet: Option<String>,
    /// Optional A1 range (sheet bindings only). Omit to read the entire tab.
    #[serde(default)]
    pub range: Option<String>,
    /// Inline table data (an array of `{col: val}` objects, OR a 2-D array
    /// whose first row is the header). When present, this binding is NOT
    /// fetched from Google — the data is used directly. Use this to pass a
    /// table the model produced (e.g. extracted from an image) as an operand.
    #[serde(default)]
    pub data: Option<serde_json::Value>,
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

    // Each binding must have EXACTLY ONE source: a sheet (spreadsheet_id + sheet)
    // OR inline `data`. Reject both/neither so the LLM gets a clear signal.
    for b in &parsed.bindings {
        let has_sheet = b.spreadsheet_id.is_some() && b.sheet.is_some();
        let has_data = b.data.is_some();
        if has_sheet == has_data {
            return serde_json::json!({
                "error": "invalid_args",
                "message": format!(
                    "binding '{}' must have exactly one source: either \
                     `spreadsheet_id` + `sheet`, or inline `data`",
                    b.var
                ),
            });
        }
        if let Some(d) = &b.data {
            if !d.is_array() {
                return serde_json::json!({
                    "error": "invalid_args",
                    "message": format!("binding '{}' inline `data` must be a JSON array", b.var),
                });
            }
        }
    }

    // 2. Fetch only the SHEET bindings in parallel. Inline (`data`) bindings
    //    are resolved directly below — no network.
    let fetches = parsed
        .bindings
        .iter()
        .filter(|b| b.data.is_none())
        .map(|b| {
            let client = client.clone();
            let opts = ReadOptions {
                value_render: crate::gsheets::domain::ValueRenderOption::UnformattedValue,
                as_records: true,
            };
            // safe: validation guarantees sheet bindings have these
            let ss = SpreadsheetId(b.spreadsheet_id.clone().unwrap());
            let sheet = b.sheet.clone().unwrap();
            let range = b.range.clone();
            let var = b.var.clone();
            async move {
                let res = client.read_range(&ss, &sheet, range.as_deref(), opts).await;
                (var, res)
            }
        });
    let results: Vec<(String, Result<ReadResponse, _>)> = join_all(fetches).await;

    // `var` → (sheet name, had_range) for SHEET bindings — used to key the
    // per-sheet load snapshot retained for `update_by_position`.
    let var_sheet: std::collections::HashMap<String, (String, bool)> = parsed
        .bindings
        .iter()
        .filter(|b| b.data.is_none())
        .filter_map(|b| {
            b.sheet
                .clone()
                .map(|s| (b.var.clone(), (s, b.range.is_some())))
        })
        .collect();

    // 3. Build the sandbox inputs map + a side-table of column names per
    //    binding (used in error responses so the LLM can self-correct
    //    without an extra round-trip).
    let mut inputs = serde_json::Map::new();
    let mut loaded_columns = serde_json::Map::new();
    // Per-sheet load snapshot (the records the code starts from), keyed by sheet
    // name. `update_by_position` diffs the returned df against this to write only
    // the cells the code changed — so it never reverts untouched concurrent edits.
    let mut loaded_snapshots: std::collections::HashMap<String, LoadedSnapshot> =
        std::collections::HashMap::new();
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
        // Retain the snapshot for this sheet (clone the records before `records`
        // is moved into the sandbox inputs). A sheet bound more than once is
        // flagged ambiguous — positional write-back can't pick a mapping.
        if let Some((sheet, had_range)) = var_sheet.get(&var) {
            let recs: Vec<serde_json::Map<String, serde_json::Value>> = records
                .as_array()
                .map(|a| a.iter().filter_map(|v| v.as_object().cloned()).collect())
                .unwrap_or_default();
            loaded_snapshots
                .entry(sheet.clone())
                .and_modify(|s| s.ambiguous = true)
                .or_insert(LoadedSnapshot {
                    records: recs,
                    had_range: *had_range,
                    ambiguous: false,
                });
        }
        inputs.insert(var, records);
    }

    // Inline `data` bindings: normalize to records and inject like sheet bindings.
    for b in parsed.bindings.iter().filter(|b| b.data.is_some()) {
        let raw = b.data.clone().unwrap(); // validated to be an array
                                           // Accept records (array of objects) as-is; convert a 2-D array
                                           // (first row = header) to records to match the sheet shape.
        let records = match raw.as_array().and_then(|a| a.first()) {
            Some(serde_json::Value::Array(_)) => rectangle_to_records(&raw),
            _ => raw, // array of objects, or empty array
        };
        let columns = extract_columns(&records);
        loaded_columns.insert(
            b.var.clone(),
            serde_json::Value::Array(columns.into_iter().map(serde_json::Value::String).collect()),
        );
        inputs.insert(b.var.clone(), records);
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
        let results = write_output_sheets(
            &client,
            &spreadsheet_id,
            sheets_value,
            policy,
            &loaded_snapshots,
        )
        .await;
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
/// Snapshot of a sheet binding loaded this run. `update_by_position` diffs the
/// returned df against `records` (what the code started from) and maps each
/// changed cell back to the sheet by position, so the agent never computes an
/// A1 address. `ambiguous` is set when more than one binding loaded the same
/// sheet (no single position mapping). `had_range` blocks the mode (a range
/// subset shifts the header/row mapping).
struct LoadedSnapshot {
    records: Vec<serde_json::Map<String, serde_json::Value>>,
    had_range: bool,
    ambiguous: bool,
}

async fn write_output_sheets(
    client: &Arc<dyn SheetsClient>,
    spreadsheet_id: &SpreadsheetId,
    output_sheets: &serde_json::Value,
    policy: CollisionPolicy,
    loaded: &std::collections::HashMap<String, LoadedSnapshot>,
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
            "update_by_position" => {
                results.push(
                    do_update_by_position(client, spreadsheet_id, raw_name, entry, loaded).await,
                );
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
                    "error": format!("unknown mode '{other}'; valid: replace, update_in_place, update_by_position, overwrite"),
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
    // Header order (for A1 mapping) — re-read the WHOLE header row ("1:1", not a
    // capped "A1:Z1") so columns past Z map correctly. The cap silently dropped
    // edits to columns beyond Z; "1:1" returns the full row like update_by_position.
    let header_read = match client
        .read_range(
            spreadsheet_id,
            raw_name,
            Some("1:1"),
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
    // Formula `{{Column}}` refs resolve only against UNIQUELY-named columns
    // (empty/duplicate names are ambiguous and excluded).
    let resolvable_cols = addressable_columns(&header_cols);

    let mut cell_updates: Vec<(String, crate::gsheets::domain::CellValue)> = Vec::new();
    let mut formula_log = FormulaCellLog::default();
    for chg in &diff.changes {
        let (Some(row), Some(col_idx)) = (
            key_to_row.get(&chg.key_value),
            col_to_index.get(&chg.column),
        ) else {
            continue;
        };
        let addr = a1_addr(*col_idx, *row);
        let resolved = match resolve_formula_placeholders(&chg.new_value, &resolvable_cols, *row) {
            Ok(v) => v,
            Err(e) => return e.to_json(raw_name),
        };
        formula_log.record(&addr, &resolved);
        cell_updates.push((
            addr,
            crate::gsheets::domain::CellValue::from_json(&resolved),
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
        Ok(_) => {
            let mut resp = serde_json::json!({
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
            });
            formula_log.attach(&mut resp);
            resp
        }
        Err(e) => serde_json::json!({
            "tab": raw_name,
            "error": format!("batch_update_cells failed: {e}"),
        }),
    }
}

/// Validate that the returned df index is exactly the set `{0..n-1}` — i.e. the
/// model returned the WHOLE bound df (modified in place), not a filtered subset.
/// This is what makes positional write-back safe: a subset / `reset_index` /
/// `concat` fails here loudly instead of silently writing the wrong rows.
fn validate_full_index(df_index: &[serde_json::Value], n: usize) -> Result<(), String> {
    if df_index.len() != n {
        return Err(format!(
            "update_by_position needs the FULL df ({n} rows), got {}. Return the WHOLE df \
             modified in place — do NOT filter/subset the rows you return.",
            df_index.len()
        ));
    }
    let mut seen = vec![false; n];
    for v in df_index {
        let Some(idx) = v.as_i64().filter(|i| *i >= 0).map(|i| i as usize) else {
            return Err("the df index must be the original 0..N-1 integer row labels. Modify the \
                 bound df IN PLACE and return it whole — do NOT reset_index / sort+reset_index / concat."
                .to_string());
        };
        if idx >= n {
            return Err(format!(
                "row index {idx} is outside the loaded range 0..{n}. Return the whole bound df \
                 modified in place — do NOT add rows or reset_index."
            ));
        }
        if seen[idx] {
            return Err(
                "duplicate row index — the df index must be the original 0..N-1 labels \
                 (no concat/duplicates). Modify the bound df in place and return it whole."
                    .to_string(),
            );
        }
        seen[idx] = true;
    }
    Ok(())
}

/// Map each UNIQUELY-named, non-empty header column to its 0-based position.
/// Empty or duplicate header names are excluded — they can't be addressed by
/// name; the caller reports them as `skipped_columns`.
fn addressable_columns(header_cols: &[String]) -> std::collections::HashMap<String, usize> {
    let mut counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for c in header_cols {
        *counts.entry(c.as_str()).or_insert(0) += 1;
    }
    header_cols
        .iter()
        .enumerate()
        .filter(|(_, c)| !c.is_empty() && counts.get(c.as_str()) == Some(&1))
        .map(|(i, c)| (c.clone(), i))
        .collect()
}

/// A single cell a new column needs: a header cell (row 1) or a body value.
/// `raw` is the value as it came from the df — formula resolution happens in the
/// caller, where the full column→index map (existing + new) is available.
#[derive(Debug, PartialEq)]
struct PlannedCell {
    col_idx: usize,
    /// 1-based; row 1 is the header.
    row: usize,
    raw: serde_json::Value,
}

/// What `update_by_position` should write for columns present in the returned df
/// but absent from the sheet header.
#[derive(Debug, Default, PartialEq)]
struct NewColumnPlan {
    /// (column name, 0-based column index) for each added column, in df order.
    added: Vec<(String, usize)>,
    /// Header cells (row 1) + body cells, in write order.
    cells: Vec<PlannedCell>,
}

/// For each df column whose name is NOT in `header_cols`, assign the next free
/// column index (appended after the last header column, in df-column order) and
/// emit its header cell plus one body cell per record with a non-null value.
/// A column whose body is entirely null is skipped (no orphan header). The sheet
/// row for record `i` is `df_index[i] + 2` (header is row 1).
fn plan_new_columns(
    header_cols: &[String],
    new_records: &[serde_json::Map<String, serde_json::Value>],
    df_index: &[serde_json::Value],
) -> NewColumnPlan {
    use std::collections::HashSet;
    let existing: HashSet<&str> = header_cols.iter().map(String::as_str).collect();

    // Discover new column names in first-seen df order, excluding the synthetic
    // index and any name already present in the header.
    let mut new_names: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for r in new_records {
        for k in r.keys() {
            if k == "__index__" || existing.contains(k.as_str()) {
                continue;
            }
            if seen.insert(k.clone()) {
                new_names.push(k.clone());
            }
        }
    }

    let mut plan = NewColumnPlan::default();
    let mut next_idx = header_cols.len();
    for name in new_names {
        let mut body: Vec<PlannedCell> = Vec::new();
        for (i, r) in new_records.iter().enumerate() {
            let Some(v) = r.get(&name) else { continue };
            if v.is_null() {
                continue;
            }
            let Some(row_idx) = df_index.get(i).and_then(serde_json::Value::as_u64) else {
                continue;
            };
            body.push(PlannedCell {
                col_idx: next_idx,
                row: row_idx as usize + 2,
                raw: v.clone(),
            });
        }
        if body.is_empty() {
            continue; // entirely-null column → don't create an orphan header.
        }
        plan.cells.push(PlannedCell {
            col_idx: next_idx,
            row: 1,
            raw: serde_json::Value::String(name.clone()),
        });
        plan.cells.extend(body);
        plan.added.push((name, next_idx));
        next_idx += 1;
    }
    plan
}

/// Positional / index-based write-back. The model modifies the bound df IN
/// PLACE and returns the WHOLE df under `mode:'update_by_position'` (no key).
/// The dispatcher diffs it against the load snapshot by row index and writes
/// only the changed cells — no agent-computed A1 address, no unique key needed.
async fn do_update_by_position(
    client: &Arc<dyn SheetsClient>,
    spreadsheet_id: &SpreadsheetId,
    raw_name: &str,
    entry: &serde_json::Value,
    loaded: &std::collections::HashMap<String, LoadedSnapshot>,
) -> serde_json::Value {
    // 1. The tab must have been BOUND whole-sheet (unambiguously) this run.
    let snap = match loaded.get(raw_name) {
        None => {
            return serde_json::json!({
                "tab": raw_name,
                "error": "UpdateByPositionRequiresBinding",
                "message": format!(
                    "update_by_position needs the tab '{raw_name}' to be BOUND in this same \
                     gsheets_run_python call (whole sheet, no `range`)."
                ),
            })
        }
        Some(s) if s.ambiguous => {
            return serde_json::json!({
                "tab": raw_name,
                "error": format!("the tab '{raw_name}' was bound more than once — can't pick a positional mapping."),
            })
        }
        Some(s) if s.had_range => {
            return serde_json::json!({
                "tab": raw_name,
                "error": format!("update_by_position needs '{raw_name}' bound WITHOUT a `range` (whole sheet)."),
            })
        }
        Some(s) => s,
    };
    let n = snap.records.len();

    let new_records: Vec<serde_json::Map<String, serde_json::Value>> = entry
        .get("df_records")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|r| r.as_object().cloned()).collect())
        .unwrap_or_default();
    let df_index: Vec<serde_json::Value> = entry
        .get("df_index")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    if df_index.len() != new_records.len() {
        return serde_json::json!({
            "tab": raw_name,
            "error": "internal: df_index/df_records length mismatch",
        });
    }

    // 2. Require the full {0..N-1} index (catches subset / reset_index / concat).
    if let Err(msg) = validate_full_index(&df_index, n) {
        return serde_json::json!({"tab": raw_name, "error": "InvalidIndex", "message": msg});
    }

    // 3. Positional header → addressable columns (skip empty/duplicate names).
    let header_read = match client
        .read_range(
            spreadsheet_id,
            raw_name,
            Some("1:1"),
            ReadOptions {
                value_render: crate::gsheets::domain::ValueRenderOption::UnformattedValue,
                as_records: false,
            },
        )
        .await
    {
        Ok(r) => r,
        Err(e) => {
            return serde_json::json!({"tab": raw_name, "error": format!("header read failed: {e}")})
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
    let col_to_index = addressable_columns(&header_cols);

    // 4. Comparable columns = snapshot columns that are addressable.
    let snap_cols: Vec<String> = snap
        .records
        .first()
        .map(|r| r.keys().cloned().collect())
        .unwrap_or_default();
    let comparable: Vec<String> = snap_cols
        .iter()
        .filter(|c| col_to_index.contains_key(*c))
        .cloned()
        .collect();
    let skipped_columns: Vec<String> = snap_cols
        .iter()
        .filter(|c| !col_to_index.contains_key(*c))
        .cloned()
        .collect();

    // 5. Inject a synthetic `__index__` key into both sides (snapshot = position,
    //    new = df_index), projecting `new` to the comparable columns so a
    //    model-added column never trips the diff's column-mismatch check.
    const IDX: &str = "__index__";
    let cur: Vec<serde_json::Map<String, serde_json::Value>> = snap
        .records
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let mut m = r.clone();
            m.insert(IDX.to_string(), serde_json::json!(i));
            m
        })
        .collect();
    let nw: Vec<serde_json::Map<String, serde_json::Value>> = new_records
        .iter()
        .zip(df_index.iter())
        .map(|(r, idx)| {
            let mut m = serde_json::Map::new();
            for c in &comparable {
                if let Some(v) = r.get(c) {
                    m.insert(c.clone(), v.clone());
                }
            }
            m.insert(IDX.to_string(), idx.clone());
            m
        })
        .collect();

    // 6. Reuse the existing cell-diff, keyed on the synthetic index.
    let diff = match diff_records(&cur, &nw, IDX, Some(&comparable), false, raw_name) {
        Ok(d) => d,
        Err(e) => return e.to_json(),
    };

    // 7. Map each change → A1 cell. The `__index__` value IS the original row
    //    position, so `sheet_row = index + 2` (header is row 1); the column
    //    comes from the positional header. No agent arithmetic anywhere.
    let mut cell_updates: Vec<(String, crate::gsheets::domain::CellValue)> = Vec::new();
    let mut formula_log = FormulaCellLog::default();
    for chg in &diff.changes {
        let (Ok(idx), Some(&col_idx)) = (
            chg.key_value.parse::<usize>(),
            col_to_index.get(&chg.column),
        ) else {
            continue;
        };
        let target_row = idx + 2;
        // Resolve any `{{Column}}` refs in a formula using the SAME positional
        // header + row we use to place the cell — so the model never computes A1.
        let resolved = match resolve_formula_placeholders(&chg.new_value, &col_to_index, target_row)
        {
            Ok(v) => v,
            Err(e) => return e.to_json(raw_name),
        };
        let addr = a1_addr(col_idx, target_row);
        formula_log.record(&addr, &resolved);
        cell_updates.push((
            addr,
            crate::gsheets::domain::CellValue::from_json(&resolved),
        ));
    }

    // Append columns present in the returned df but absent from the sheet header.
    // They get the next free column indices; their formulas may reference other
    // new columns, so resolve against existing + new positions.
    let new_plan = plan_new_columns(&header_cols, &new_records, &df_index);
    let added_columns: Vec<serde_json::Value> = new_plan
        .added
        .iter()
        .map(|(name, i)| serde_json::json!({ "name": name, "column": col_letter(*i) }))
        .collect();
    let new_column_names: Vec<String> = new_plan
        .added
        .iter()
        .map(|(name, _)| name.clone())
        .collect();
    if !new_plan.cells.is_empty() {
        let mut resolvable = col_to_index.clone();
        for (name, i) in &new_plan.added {
            resolvable.insert(name.clone(), *i);
        }
        for pc in &new_plan.cells {
            let resolved = match resolve_formula_placeholders(&pc.raw, &resolvable, pc.row) {
                Ok(v) => v,
                Err(e) => return e.to_json(raw_name),
            };
            let addr = a1_addr(pc.col_idx, pc.row);
            formula_log.record(&addr, &resolved);
            cell_updates.push((
                addr,
                crate::gsheets::domain::CellValue::from_json(&resolved),
            ));
        }
    }

    let cells = cell_updates.len();
    if cells == 0 {
        return serde_json::json!({
            "tab": raw_name, "mode": "update_by_position",
            "changes": {"rows": 0, "cells": 0},
            "skipped_columns": skipped_columns,
        });
    }
    match client
        .batch_update_cells(spreadsheet_id, raw_name, cell_updates)
        .await
    {
        Ok(_) => {
            let mut columns_touched = diff.columns_touched.clone();
            columns_touched.extend(new_column_names.iter().cloned());
            let mut resp = serde_json::json!({
                "tab": raw_name, "mode": "update_by_position",
                "changes": {"rows": diff.rows_changed, "cells": cells, "columns": columns_touched},
                "skipped_columns": skipped_columns,
            });
            if !added_columns.is_empty() {
                resp["added_columns"] = serde_json::Value::Array(added_columns.clone());
            }
            formula_log.attach(&mut resp);
            resp
        }
        Err(e) => serde_json::json!({
            "tab": raw_name, "error": format!("batch_update_cells failed: {e}"),
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
    let col_names: Vec<String> = cols
        .iter()
        .map(|c| c.as_str().unwrap_or("").to_string())
        .collect();
    // Resolve `{{Column}}` formula placeholders here too, so creating a tab WITH
    // a formula column (replace/overwrite) works like the diff-write modes.
    let resolvable = addressable_columns(&col_names);
    let header_row: Vec<CellValue> = col_names.iter().cloned().map(CellValue::String).collect();
    let mut matrix: Vec<Vec<CellValue>> = vec![header_row];
    let mut formula_log = FormulaCellLog::default();
    for rec in records {
        let Some(obj) = rec.as_object() else { continue };
        // The row about to be pushed lands at sheet row `matrix.len() + 1`
        // (matrix[0] is the header → sheet row 1).
        let target_row = matrix.len() + 1;
        let mut row: Vec<CellValue> = Vec::with_capacity(col_names.len());
        for (j, key) in col_names.iter().enumerate() {
            let raw = obj.get(key).unwrap_or(&serde_json::Value::Null);
            let resolved = match resolve_formula_placeholders(raw, &resolvable, target_row) {
                Ok(v) => v,
                Err(e) => return e.to_json(raw_name),
            };
            formula_log.record(&a1_addr(j, target_row), &resolved);
            row.push(CellValue::from_json(&resolved));
        }
        matrix.push(row);
    }
    let n_rows = matrix.len();
    let n_cols = cols.len();
    match client.set_range(spreadsheet_id, name, "A1", matrix).await {
        Ok(_) => {
            let mut resp = serde_json::json!({
                "name": raw_name,
                "resolved_name": name,
                "n_rows": n_rows,
                "n_cols": n_cols,
            });
            formula_log.attach(&mut resp);
            resp
        }
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
    // Header row — span the full width reported by `col_count` instead of a
    // hardcoded `A1:Z1`, so sheets with >26 columns surface every header
    // (otherwise `current_state.columns` truncated at column Z and the LLM
    // decided collisions with incomplete info).
    let last_col = meta.col_count.max(1) as usize - 1;
    let header_range = format!("A1:{}", a1_addr(last_col, 1));
    let read = client
        .read_range(
            spreadsheet_id,
            tab,
            Some(&header_range),
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
    // Best-effort Drive `modifiedTime` — a collision envelope is more useful
    // when the LLM can tell fresh data from last year's. Never fail the whole
    // meta fetch over it.
    let last_modified = client
        .get_modified_time(spreadsheet_id)
        .await
        .ok()
        .flatten();
    Ok(Some(TabMeta {
        n_rows: meta.row_count as u64,
        n_cols: meta.col_count as u64,
        columns,
        last_modified,
    }))
}

/// Convert a 0-based column index to its A1 column letter(s):
/// `0 → "A"`, `25 → "Z"`, `26 → "AA"`, `27 → "AB"`. Shared by [`a1_addr`] and
/// the formula-placeholder resolver so a cell address and a `{{Column}}`
/// reference are computed by exactly the same rule.
fn col_letter(col_index: usize) -> String {
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
    col
}

/// Convert column index + row index to an A1 string for `batch_update_cells`.
/// `row_index` is 1-based, where row 1 is the header.
fn a1_addr(col_index: usize, row_index: usize) -> String {
    format!("{}{}", col_letter(col_index), row_index)
}

/// A `{{Column}}` placeholder named a column that isn't addressable (unknown,
/// empty, or duplicate header name). Surfaced as a structured tool error so the
/// model can self-correct — far better than the silent `#VALUE!` you get when a
/// model hand-computes the wrong column letter inside a formula.
#[derive(Debug)]
struct FormulaResolveError {
    unknown: String,
    valid: Vec<String>,
}

impl FormulaResolveError {
    fn to_json(&self, tab: &str) -> serde_json::Value {
        serde_json::json!({
            "tab": tab,
            "error": "FormulaUnknownColumn",
            "unknown_column": self.unknown,
            "valid_columns": self.valid,
            "message": format!(
                "A formula references column {} which is not an addressable column in \
                 '{}'. Reference columns by their EXACT name (case-sensitive) using \
                 {{{{Name}}}} — it resolves to that column in the SAME row. Never compute \
                 column letters by hand. Valid columns: {:?}.",
                format!("{{{{{}}}}}", self.unknown),
                tab,
                self.valid,
            ),
        })
    }
}

/// Resolve `{{ColumnName}}` placeholders inside a formula string into real A1
/// refs for `target_row`, using `resolvable` (addressable header column name →
/// 0-based index). Only a String that starts with `=` is processed — numbers,
/// plain strings, and null are returned unchanged. Each `{{Name}}` resolves to
/// that column's cell in the SAME row (`<letter><target_row>`); a name that
/// isn't addressable returns `Err` so the caller aborts the write (no partial
/// writes) instead of producing a broken formula.
fn resolve_formula_placeholders(
    value: &serde_json::Value,
    resolvable: &std::collections::HashMap<String, usize>,
    target_row: usize,
) -> Result<serde_json::Value, FormulaResolveError> {
    let Some(s) = value.as_str() else {
        return Ok(value.clone());
    };
    // Only formulas carry column references; `{{` is meaningless elsewhere.
    if !s.starts_with('=') || !s.contains("{{") {
        return Ok(value.clone());
    }
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        // A doubled `{{` opens a placeholder; a single `{` (Sheets array
        // literal, e.g. `={1,2;3,4}`) is copied through untouched.
        if bytes[i] == b'{' && i + 1 < s.len() && bytes[i + 1] == b'{' {
            if let Some(rel) = s[i + 2..].find("}}") {
                let name = s[i + 2..i + 2 + rel].trim();
                match resolvable.get(name) {
                    Some(&idx) => {
                        out.push_str(&col_letter(idx));
                        out.push_str(&target_row.to_string());
                        i = i + 2 + rel + 2;
                        continue;
                    }
                    None => {
                        let mut valid: Vec<String> = resolvable.keys().cloned().collect();
                        valid.sort();
                        return Err(FormulaResolveError {
                            unknown: name.to_string(),
                            valid,
                        });
                    }
                }
            }
        }
        // Copy one full char (keeps us on UTF-8 boundaries).
        let ch = s[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    Ok(serde_json::Value::String(out))
}

/// Max resolved-formula cells echoed back in a tool result. The model uses these
/// to report what it wrote truthfully (instead of recomputing A1 by hand, which
/// it gets wrong); a small sample is enough for a confirmation message. When the
/// write exceeds this (e.g. a whole-column fill), the response also carries
/// `formula_cells_total` + `formula_cells_truncated` so the model reports the
/// real count instead of mistaking the 50-cell sample for everything.
const FORMULA_CELLS_SAMPLE_CAP: usize = 50;

/// Accumulates the formulas a diff-write lands: a bounded `sample` (real A1 →
/// formula text) the model can quote verbatim, plus the `total` count so it
/// knows when the sample is partial.
#[derive(Default)]
struct FormulaCellLog {
    sample: serde_json::Map<String, serde_json::Value>,
    total: usize,
}

impl FormulaCellLog {
    /// Record `resolved` if it is a formula (`=…`). Counts every formula; only
    /// the first [`FORMULA_CELLS_SAMPLE_CAP`] land in the echoed sample.
    fn record(&mut self, addr: &str, resolved: &serde_json::Value) {
        if let Some(s) = resolved.as_str() {
            if s.starts_with('=') {
                self.total += 1;
                if self.sample.len() < FORMULA_CELLS_SAMPLE_CAP {
                    self.sample
                        .insert(addr.to_string(), serde_json::Value::String(s.to_string()));
                }
            }
        }
    }

    /// Attach `formula_cells` (+ `formula_cells_total`/`_truncated` when the
    /// sample is partial) to a success response. No-op when nothing was a formula.
    fn attach(self, resp: &mut serde_json::Value) {
        if self.total == 0 {
            return;
        }
        let Some(obj) = resp.as_object_mut() else {
            return;
        };
        let truncated = self.total > self.sample.len();
        obj.insert(
            "formula_cells".to_string(),
            serde_json::Value::Object(self.sample),
        );
        if truncated {
            obj.insert(
                "formula_cells_total".to_string(),
                serde_json::json!(self.total),
            );
            obj.insert(
                "formula_cells_truncated".to_string(),
                serde_json::json!(true),
            );
        }
    }
}

#[cfg(test)]
mod position_tests {
    use super::*;
    use serde_json::json;

    fn idx(v: &[i64]) -> Vec<serde_json::Value> {
        v.iter().map(|i| json!(i)).collect()
    }

    #[test]
    fn full_index_accepts_any_permutation_of_0_to_n() {
        assert!(validate_full_index(&idx(&[0, 1, 2, 3]), 4).is_ok());
        // sort() reorders labels but the set is still {0..N-1} → fine.
        assert!(validate_full_index(&idx(&[3, 0, 2, 1]), 4).is_ok());
    }

    #[test]
    fn full_index_rejects_subset() {
        // df.loc[mask] returning 2 of 4 rows → wrong-row footgun → rejected.
        let e = validate_full_index(&idx(&[1, 2]), 4).unwrap_err();
        assert!(e.contains("FULL df"));
    }

    #[test]
    fn full_index_rejects_out_of_range_and_duplicates() {
        assert!(validate_full_index(&idx(&[0, 1, 9]), 3).is_err()); // 9 >= N
        assert!(validate_full_index(&idx(&[0, 0, 1]), 3).is_err()); // duplicate (concat)
                                                                    // non-integer label (e.g. a string index)
        assert!(validate_full_index(&[json!("a"), json!("b")], 2).is_err());
    }

    #[test]
    fn addressable_skips_empty_and_duplicate_header_names() {
        // Mirrors the real "Hoja 16": two empty-named columns + dup.
        let header = vec![
            "CLIENT ID".to_string(),
            "".to_string(),
            "Cantidad".to_string(),
            "Tarifa".to_string(),
            "Importe".to_string(),
            "".to_string(),
            "Tarifa".to_string(), // duplicate name
        ];
        let map = addressable_columns(&header);
        assert_eq!(map.get("CLIENT ID"), Some(&0));
        assert_eq!(map.get("Cantidad"), Some(&2));
        assert_eq!(map.get("Importe"), Some(&4));
        // empty + duplicate names are NOT addressable.
        assert!(!map.contains_key(""));
        assert!(!map.contains_key("Tarifa"));
    }

    #[test]
    fn a1_addr_maps_position_to_letter() {
        // col 0→A, 18→S, 20→U, 21→V; row passed 1-based.
        assert_eq!(a1_addr(0, 2), "A2");
        assert_eq!(a1_addr(21, 20), "V20"); // Importe at sheet row 20
        assert_eq!(a1_addr(20, 20), "U20"); // Tarifa — the column pro wrongly hit
    }

    #[test]
    fn col_letter_handles_columns_past_z() {
        assert_eq!(col_letter(0), "A");
        assert_eq!(col_letter(18), "S"); // Cantidad
        assert_eq!(col_letter(25), "Z");
        assert_eq!(col_letter(26), "AA");
        assert_eq!(col_letter(27), "AB");
    }

    fn cols(pairs: &[(&str, usize)]) -> std::collections::HashMap<String, usize> {
        pairs.iter().map(|(n, i)| (n.to_string(), *i)).collect()
    }

    #[test]
    fn resolve_per_row_formula_to_real_a1() {
        // The headline case: the model writes column NAMES; the dispatcher emits
        // real A1 for the SAME row — `=S5*U5`, not the model's off-by-one `=R5*T5`.
        let map = cols(&[("Cantidad", 18), ("Tarifa", 20)]);
        let out =
            resolve_formula_placeholders(&json!("={{Cantidad}}*{{Tarifa}}"), &map, 5).unwrap();
        assert_eq!(out, json!("=S5*U5"));
    }

    #[test]
    fn resolve_column_name_with_space() {
        let map = cols(&[("CLIENT ID", 0)]);
        let out = resolve_formula_placeholders(&json!("={{CLIENT ID}}"), &map, 3).unwrap();
        assert_eq!(out, json!("=A3"));
    }

    #[test]
    fn resolve_unknown_column_errors_with_valid_list() {
        let map = cols(&[("Cantidad", 18), ("Tarifa", 20)]);
        let err = resolve_formula_placeholders(&json!("={{Cantdad}}*2"), &map, 5).unwrap_err();
        assert_eq!(err.unknown, "Cantdad");
        assert!(err.valid.contains(&"Cantidad".to_string()));
    }

    #[test]
    fn resolve_leaves_non_formula_and_scalars_untouched() {
        let map = cols(&[("x", 0)]);
        // braces but no leading '=' → not a formula → untouched
        assert_eq!(
            resolve_formula_placeholders(&json!("hola {{x}}"), &map, 2).unwrap(),
            json!("hola {{x}}")
        );
        assert_eq!(
            resolve_formula_placeholders(&json!(5), &map, 2).unwrap(),
            json!(5)
        );
        assert_eq!(
            resolve_formula_placeholders(&json!(null), &map, 2).unwrap(),
            json!(null)
        );
    }

    #[test]
    fn resolve_leaves_single_brace_array_literal_untouched() {
        // `={1,2;3,4}` is a real Sheets array literal — single braces must survive.
        let map = cols(&[("A", 0), ("B", 1)]);
        let out = resolve_formula_placeholders(&json!("={1,2;3,4}"), &map, 7).unwrap();
        assert_eq!(out, json!("={1,2;3,4}"));
    }

    #[test]
    fn resolve_multiple_tokens_with_surrounding_text() {
        let map = cols(&[("A", 2), ("B", 3)]); // C, D
        let out = resolve_formula_placeholders(&json!("=IF({{A}}>0,{{B}},0)"), &map, 3).unwrap();
        assert_eq!(out, json!("=IF(C3>0,D3,0)"));
    }

    #[test]
    fn formula_cells_echo_only_formulas_for_truthful_reporting() {
        let mut log = FormulaCellLog::default();
        log.record("V5", &json!("=S5*U5")); // formula → recorded
        log.record("B2", &json!(42)); // number → ignored
        log.record("C2", &json!("plain text")); // non-formula → ignored
        assert_eq!(log.total, 1);
        assert_eq!(log.sample.get("V5"), Some(&json!("=S5*U5")));

        let mut resp = json!({"tab": "Hoja 16"});
        log.attach(&mut resp);
        assert_eq!(resp["formula_cells"]["V5"], json!("=S5*U5"));
        // small write → no truncation signal
        assert!(resp.get("formula_cells_total").is_none());

        // empty set → field omitted entirely (no noise on pure-value writes)
        let mut resp2 = json!({"tab": "Hoja 16"});
        FormulaCellLog::default().attach(&mut resp2);
        assert!(resp2.get("formula_cells").is_none());
    }

    #[test]
    fn formula_cells_signal_truncation_past_cap() {
        // A whole-column fill (e.g. 130 rows) exceeds the 50-cell sample → the
        // response must announce the real total so the model reports it honestly.
        let mut log = FormulaCellLog::default();
        for r in 2..132 {
            log.record(&format!("V{r}"), &json!(format!("=S{r}*U{r}")));
        }
        assert_eq!(log.total, 130);
        let mut resp = json!({"tab": "Ventas"});
        log.attach(&mut resp);
        assert_eq!(
            resp["formula_cells"].as_object().unwrap().len(),
            FORMULA_CELLS_SAMPLE_CAP
        );
        assert_eq!(resp["formula_cells_total"], json!(130));
        assert_eq!(resp["formula_cells_truncated"], json!(true));
    }

    fn rec(pairs: &[(&str, serde_json::Value)]) -> serde_json::Map<String, serde_json::Value> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    #[test]
    fn plan_new_columns_appends_after_last_header() {
        let header = vec!["a".to_string(), "b".to_string()]; // A, B occupied → new at C (idx 2)
        let new = vec![
            rec(&[("a", json!(1)), ("b", json!(2)), ("Margen", json!("=C-D"))]),
            rec(&[("a", json!(3)), ("b", json!(4)), ("Margen", json!("=C-D"))]),
        ];
        let plan = plan_new_columns(&header, &new, &idx(&[0, 1]));
        assert_eq!(plan.added, vec![("Margen".to_string(), 2)]);
        assert_eq!(
            plan.cells,
            vec![
                PlannedCell {
                    col_idx: 2,
                    row: 1,
                    raw: json!("Margen")
                },
                PlannedCell {
                    col_idx: 2,
                    row: 2,
                    raw: json!("=C-D")
                },
                PlannedCell {
                    col_idx: 2,
                    row: 3,
                    raw: json!("=C-D")
                },
            ]
        );
    }

    #[test]
    fn plan_new_columns_multiple_get_consecutive_indices() {
        let header = vec!["a".to_string()]; // A occupied → new at B(1), C(2)
        let new = vec![rec(&[("a", json!(1)), ("x", json!(10)), ("y", json!(20))])];
        let plan = plan_new_columns(&header, &new, &idx(&[0]));
        assert_eq!(plan.added, vec![("x".to_string(), 1), ("y".to_string(), 2)]);
    }

    #[test]
    fn plan_new_columns_ignores_existing_header_names() {
        let header = vec!["a".to_string(), "b".to_string()];
        let new = vec![rec(&[("a", json!(1)), ("b", json!(9))])]; // both already in header
        let plan = plan_new_columns(&header, &new, &idx(&[0]));
        assert!(plan.added.is_empty());
        assert!(plan.cells.is_empty());
    }

    #[test]
    fn plan_new_columns_skips_all_null_column() {
        let header = vec!["a".to_string()];
        let new = vec![
            rec(&[("a", json!(1)), ("Empty", serde_json::Value::Null)]),
            rec(&[("a", json!(2)), ("Empty", serde_json::Value::Null)]),
        ];
        let plan = plan_new_columns(&header, &new, &idx(&[0, 1]));
        assert!(
            plan.added.is_empty(),
            "all-null column must not create an orphan header"
        );
        assert!(plan.cells.is_empty());
    }

    #[test]
    fn plan_new_columns_uses_df_index_for_row_mapping() {
        let header = vec!["a".to_string()]; // new col at B(1)
                                            // df_index out of natural order: record 0 → sheet row 3, record 1 → row 2.
        let new = vec![
            rec(&[("a", json!(1)), ("m", json!("r3"))]),
            rec(&[("a", json!(2)), ("m", json!("r2"))]),
        ];
        let plan = plan_new_columns(&header, &new, &idx(&[1, 0]));
        assert_eq!(
            plan.cells,
            vec![
                PlannedCell {
                    col_idx: 1,
                    row: 1,
                    raw: json!("m")
                },
                PlannedCell {
                    col_idx: 1,
                    row: 3,
                    raw: json!("r3")
                },
                PlannedCell {
                    col_idx: 1,
                    row: 2,
                    raw: json!("r2")
                },
            ]
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path_regex, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    async fn mock_with_two_sheets() -> (MockServer, Arc<GoogleSheetsHttpClient>) {
        let server = MockServer::start().await;
        let client = GoogleSheetsHttpClient::for_tests(&server.uri(), &server.uri(), &server.uri());
        client.token_test_seed("fake-bearer-token").await;

        // Products — Approach B: spreadsheets.get with includeGridData.
        Mock::given(method("GET"))
            .and(path_regex(r"/sp_p$"))
            .and(query_param("includeGridData", "true"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "sheets": [{"data": [{"startRow": 0, "startColumn": 0, "rowData": [
                    {"values": [
                        {"effectiveValue": {"stringValue": "sku"}},
                        {"effectiveValue": {"stringValue": "qty"}}
                    ]},
                    {"values": [
                        {"effectiveValue": {"stringValue": "a-1"}},
                        {"effectiveValue": {"numberValue": 10.0}}
                    ]},
                    {"values": [
                        {"effectiveValue": {"stringValue": "a-2"}},
                        {"effectiveValue": {"numberValue": 20.0}}
                    ]}
                ]}], "merges": []}]
            })))
            .mount(&server)
            .await;

        // Sales — Approach B: spreadsheets.get with includeGridData.
        Mock::given(method("GET"))
            .and(path_regex(r"/sp_s$"))
            .and(query_param("includeGridData", "true"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "sheets": [{"data": [{"startRow": 0, "startColumn": 0, "rowData": [
                    {"values": [
                        {"effectiveValue": {"stringValue": "sku"}},
                        {"effectiveValue": {"stringValue": "sold"}}
                    ]},
                    {"values": [
                        {"effectiveValue": {"stringValue": "a-1"}},
                        {"effectiveValue": {"numberValue": 3.0}}
                    ]},
                    {"values": [
                        {"effectiveValue": {"stringValue": "a-2"}},
                        {"effectiveValue": {"numberValue": 7.0}}
                    ]}
                ]}], "merges": []}]
            })))
            .mount(&server)
            .await;

        (server, Arc::new(client))
    }

    #[tokio::test]
    async fn dispatch_runs_pandas_over_two_bindings_in_parallel() {
        pyo3::Python::initialize();
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
        pyo3::Python::initialize();
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
        assert_eq!(parsed.sheet.as_deref(), Some("products"));
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
        assert_eq!(parsed.sheet.as_deref(), Some("Sheet1"));
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
        assert_eq!(parsed.bindings[0].sheet.as_deref(), Some("products"));
        assert_eq!(parsed.bindings[0].spreadsheet_id.as_deref(), Some("abc"));
    }

    // ── multi-sheet write-back tests ──────────────────────────────────────

    /// Happy path: script sets `output_sheets` with 3 DataFrames and
    /// `write_to_spreadsheet` is supplied → 3 add_sheet + 3 set_range
    /// calls, `wrote_sheets` has 3 entries, no `error`.
    #[tokio::test]
    async fn multi_sheet_write_back_three_tabs() {
        pyo3::Python::initialize();

        let server = MockServer::start().await;
        let client = GoogleSheetsHttpClient::for_tests(&server.uri(), &server.uri(), &server.uri());
        client.token_test_seed("fake-bearer-token").await;

        // Binding: Products tab on spreadsheet "ws_w" — Approach B.
        Mock::given(method("GET"))
            .and(path_regex(r"/ws_w$"))
            .and(query_param("includeGridData", "true"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "sheets": [{"data": [{"startRow": 0, "startColumn": 0, "rowData": [
                    {"values": [
                        {"effectiveValue": {"stringValue": "sku"}},
                        {"effectiveValue": {"stringValue": "qty"}}
                    ]},
                    {"values": [
                        {"effectiveValue": {"stringValue": "a-1"}},
                        {"effectiveValue": {"numberValue": 10.0}}
                    ]},
                    {"values": [
                        {"effectiveValue": {"stringValue": "a-2"}},
                        {"effectiveValue": {"numberValue": 20.0}}
                    ]}
                ]}], "merges": []}]
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

        pyo3::Python::initialize();

        let server = MockServer::start().await;
        let client = GoogleSheetsHttpClient::for_tests(&server.uri(), &server.uri(), &server.uri());
        client.token_test_seed("fake-bearer-token").await;

        // Binding: Whatever tab on spreadsheet "ws_c" — Approach B.
        Mock::given(method("GET"))
            .and(path_regex(r"/ws_c$"))
            .and(query_param("includeGridData", "true"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "sheets": [{"data": [{"startRow": 0, "startColumn": 0, "rowData": [
                    {"values": [
                        {"effectiveValue": {"stringValue": "id"}},
                        {"effectiveValue": {"stringValue": "val"}}
                    ]},
                    {"values": [
                        {"effectiveValue": {"numberValue": 1.0}},
                        {"effectiveValue": {"numberValue": 99.0}}
                    ]}
                ]}], "merges": []}]
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
        pyo3::Python::initialize();

        let server = wiremock::MockServer::start().await;
        let client = GoogleSheetsHttpClient::for_tests(&server.uri(), &server.uri(), &server.uri());
        client.token_test_seed("fake-token").await;

        // (a) read_range mock — Approach B: returns current "Sales" with 3 rows.
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path_regex(r"/ss_u$"))
            .and(query_param("includeGridData", "true"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "sheets": [{"data": [{"startRow": 0, "startColumn": 0, "rowData": [
                        {"values": [
                            {"effectiveValue": {"stringValue": "id"}},
                            {"effectiveValue": {"stringValue": "price"}}
                        ]},
                        {"values": [
                            {"effectiveValue": {"stringValue": "a"}},
                            {"effectiveValue": {"numberValue": 10.0}}
                        ]},
                        {"values": [
                            {"effectiveValue": {"stringValue": "b"}},
                            {"effectiveValue": {"numberValue": 20.0}}
                        ]},
                        {"values": [
                            {"effectiveValue": {"stringValue": "c"}},
                            {"effectiveValue": {"numberValue": 30.0}}
                        ]}
                    ]}], "merges": []}]
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
        pyo3::Python::initialize();

        let server = wiremock::MockServer::start().await;
        let client = GoogleSheetsHttpClient::for_tests(&server.uri(), &server.uri(), &server.uri());
        client.token_test_seed("fake-token").await;

        // list_sheets — "Existing" already there (no includeGridData param).
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path_regex(r"/ss_f$"))
            .and(wiremock::matchers::query_param_is_missing(
                "includeGridData",
            ))
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

        // read_range (binding + header columns) — Approach B (includeGridData=true).
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path_regex(r"/ss_f$"))
            .and(query_param("includeGridData", "true"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "sheets": [{"data": [{"startRow": 0, "startColumn": 0, "rowData": [
                        {"values": [
                            {"effectiveValue": {"stringValue": "a"}},
                            {"effectiveValue": {"stringValue": "b"}},
                            {"effectiveValue": {"stringValue": "c"}}
                        ]}
                    ]}], "merges": []}]
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

    /// Build a minimal wiremock-backed client that will never receive any
    /// sheet-fetch requests (used by inline-data tests that exercise
    /// pre-fetch validation only).
    async fn test_client_empty() -> Arc<dyn SheetsClient> {
        let server = MockServer::start().await;
        let client = GoogleSheetsHttpClient::for_tests(&server.uri(), &server.uri(), &server.uri());
        client.token_test_seed("fake-bearer-token").await;
        // The _server is intentionally dropped here — no mocks are mounted, and
        // inline tests never reach the fetch loop, so the mock server isn't needed.
        Arc::new(client)
    }

    #[test]
    fn binding_inline_data_parses() {
        let v = serde_json::json!({ "var": "t", "data": [{"a": 1}] });
        let b: GsheetsBinding = serde_json::from_value(v).unwrap();
        assert_eq!(b.var, "t");
        assert!(b.data.is_some());
        assert!(b.spreadsheet_id.is_none());
    }

    #[tokio::test]
    async fn dispatch_rejects_binding_with_both_sources() {
        let client = test_client_empty().await; // any mock; validation happens pre-fetch
        let args = serde_json::json!({
            "bindings": [{ "var": "t", "spreadsheet_id": "x", "sheet": "S", "data": [{"a":1}] }],
            "code": "output = 1"
        });
        let out = dispatch_gsheets_run_python_with_client(client, args).await;
        assert_eq!(out["error"], "invalid_args");
        assert!(out["message"].as_str().unwrap().contains("exactly one"));
    }

    #[tokio::test]
    async fn dispatch_rejects_binding_with_no_source() {
        let client = test_client_empty().await;
        let args = serde_json::json!({
            "bindings": [{ "var": "t" }],
            "code": "output = 1"
        });
        let out = dispatch_gsheets_run_python_with_client(client, args).await;
        assert_eq!(out["error"], "invalid_args");
        assert!(out["message"].as_str().unwrap().contains("exactly one"));
    }

    #[tokio::test]
    async fn dispatch_inline_data_binding_reaches_sandbox() {
        pyo3::Python::initialize();
        // No sheet fetch needed; client can be any mock.
        let client = test_client_empty().await;
        let args = serde_json::json!({
            "bindings": [{ "var": "img", "data": [{"nutrient": "Protein", "g": 12}, {"nutrient": "Fat", "g": 3}] }],
            "code": "output = {'rows': len(img), 'first': img[0]['nutrient']}"
        });
        let out = dispatch_gsheets_run_python_with_client(client, args).await;
        // loaded_columns is populated for inline bindings even when pandas fails.
        // Assert this BEFORE the pandas-skip guard so missing inline-loop is caught.
        assert!(
            out.get("loaded_columns")
                .and_then(|lc| lc.get("img"))
                .and_then(|v| v.as_array())
                .map(|a| a.contains(&serde_json::json!("nutrient")))
                .unwrap_or(false),
            "loaded_columns.img must contain 'nutrient'; got: {out}"
        );
        if let Some(err) = out.get("error").and_then(|v| v.as_str()) {
            if err.contains("No module named 'pandas'") {
                eprintln!("SKIPPED (no pandas in test env): {err}");
                return;
            }
            panic!("unexpected error: {out}");
        }
        assert_eq!(out["output"]["rows"], 2);
        assert_eq!(out["output"]["first"], "Protein");
    }

    #[tokio::test]
    async fn dispatch_inline_2d_array_is_converted_to_records() {
        pyo3::Python::initialize();
        let client = test_client_empty().await;
        let args = serde_json::json!({
            "bindings": [{ "var": "t", "data": [["nutrient", "g"], ["Protein", 12]] }],
            "code": "output = {'rows': len(t), 'g': t[0]['g']}"
        });
        let out = dispatch_gsheets_run_python_with_client(client, args).await;
        // loaded_columns must contain "g" from the 2-D header row.
        assert!(
            out.get("loaded_columns")
                .and_then(|lc| lc.get("t"))
                .and_then(|v| v.as_array())
                .map(|a| a.contains(&serde_json::json!("g")))
                .unwrap_or(false),
            "loaded_columns.t must contain 'g'; got: {out}"
        );
        if let Some(err) = out.get("error").and_then(|v| v.as_str()) {
            if err.contains("No module named 'pandas'") {
                eprintln!("SKIPPED (no pandas in test env): {err}");
                return;
            }
            panic!("unexpected error: {out}");
        }
        assert_eq!(out["output"]["rows"], 1);
        assert_eq!(out["output"]["g"], 12);
    }

    /// `a1_addr` is what computes the header read range in `fetch_tab_meta`.
    /// Boundary check that wide sheets span past column Z (the old `A1:Z1`
    /// hardcode truncated them).
    #[test]
    fn header_range_spans_past_column_z() {
        assert_eq!(a1_addr(25, 1), "Z1"); // 26th col
        assert_eq!(a1_addr(26, 1), "AA1"); // 27th col — past the old cap
        assert_eq!(a1_addr(29, 1), "AD1"); // 30th col
                                           // What fetch_tab_meta builds for a 30-column sheet:
        let last_col = 30usize - 1;
        assert_eq!(format!("A1:{}", a1_addr(last_col, 1)), "A1:AD1");
    }

    /// Wide sheet (>26 cols) surfaces ALL headers in the `SheetExists`
    /// envelope, and `last_modified` is populated from Drive `modifiedTime`.
    /// One mock serves both the `list_sheets` and `get_modified_time` reads
    /// because both hit `/{id}?...` and the body carries `sheets` +
    /// `modifiedTime`.
    #[tokio::test]
    async fn fail_envelope_has_wide_columns_and_last_modified() {
        pyo3::Python::initialize();

        let server = wiremock::MockServer::start().await;
        let client = GoogleSheetsHttpClient::for_tests(&server.uri(), &server.uri(), &server.uri());
        client.token_test_seed("fake-token").await;

        // 30-column header — proves the range went past Z.
        let header: Vec<String> = (0..30).map(|i| format!("c{i}")).collect();

        // list_sheets — no includeGridData (returns sheet metadata only).
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path_regex(r"/ss_w$"))
            .and(wiremock::matchers::query_param_is_missing(
                "includeGridData",
            ))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "sheets": [{
                        "properties": {
                            "sheetId": 3, "title": "Wide", "index": 0,
                            "gridProperties": {"rowCount": 10, "columnCount": 30}
                        }
                    }],
                    "modifiedTime": "2026-06-04T10:23:00Z",
                })),
            )
            .mount(&server)
            .await;

        // read_range (binding + header fetch) — Approach B: includeGridData=true.
        // 30-column header row; the binding data rows are omitted (empty DataFrame
        // is sufficient — the test only cares that the collision envelope has all
        // 30 column names).
        let header_values: Vec<serde_json::Value> = header
            .iter()
            .map(|h| serde_json::json!({"effectiveValue": {"stringValue": h}}))
            .collect();
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path_regex(r"/ss_w$"))
            .and(query_param("includeGridData", "true"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "sheets": [{"data": [{"startRow": 0, "startColumn": 0, "rowData": [
                        {"values": header_values}
                    ]}], "merges": []}]
                })),
            )
            .mount(&server)
            .await;

        let args = serde_json::json!({
            "bindings": [{"var": "x", "spreadsheet_id": "ss_w", "sheet": "Wide"}],
            "code": "\
        import pandas as pd\n\
        df = pd.DataFrame(x)\n\
        output_sheets = {'Wide': df}\n\
        output = {}\n\
        ",
            "write_to_spreadsheet": "ss_w"
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
        assert_eq!(wrote[0]["error"], "SheetExists");
        let cols = wrote[0]["current_state"]["columns"].as_array().unwrap();
        assert_eq!(cols.len(), 30, "all 30 headers must surface, not just A:Z");
        assert_eq!(cols[29], "c29");
        assert_eq!(
            wrote[0]["current_state"]["last_modified"],
            "2026-06-04T10:23:00Z"
        );
    }

    #[tokio::test]
    async fn auto_suffix_policy_preserves_old_behavior() {
        // With fixed_config.on_existing_sheet="auto_suffix", colliding
        // "Existing" should write to "Existing (2)" silently — same as
        // pre-spec behavior.
        pyo3::Python::initialize();

        let server = wiremock::MockServer::start().await;
        let client = GoogleSheetsHttpClient::for_tests(&server.uri(), &server.uri(), &server.uri());
        client.token_test_seed("fake-token").await;

        // list_sheets — "Existing" already there (no includeGridData param).
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path_regex(r"/ss_a$"))
            .and(wiremock::matchers::query_param_is_missing(
                "includeGridData",
            ))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "sheets": [{"properties": {"sheetId": 1, "title": "Existing", "index": 0,
                        "gridProperties": {"rowCount": 2, "columnCount": 2}}}]
                })),
            )
            .mount(&server)
            .await;

        // Binding read + header read — Approach B (includeGridData=true).
        // Both reads (binding var `x` and fetch_tab_meta header) share this mock.
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path_regex(r"/ss_a$"))
            .and(query_param("includeGridData", "true"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "sheets": [{"data": [{"startRow": 0, "startColumn": 0, "rowData": [
                        {"values": [
                            {"effectiveValue": {"stringValue": "id"}},
                            {"effectiveValue": {"stringValue": "val"}}
                        ]},
                        {"values": [
                            {"effectiveValue": {"numberValue": 1.0}},
                            {"effectiveValue": {"numberValue": 99.0}}
                        ]}
                    ]}], "merges": []}]
                })),
            )
            .mount(&server)
            .await;

        // Header read (A1:A1) — already served by the includeGridData mock above.

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
