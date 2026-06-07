//! LLM tool `crdt_doc_run_python` — runs sandboxed Python (pandas/numpy/
//! scipy.stats) against workbook data extracted from the current
//! `CrdtDocsContext`. See:
//!   - Spec: docs/superpowers/specs/2026-06-03-crdt-pandas-integration-design.md
//!   - Plan: docs/superpowers/plans/2026-06-03-crdt-pandas-integration.md (C-T5)

use crate::crdt_documents::{build_records_for_sheets, write_records_as_new_sheet, RecordsError};
use crate::dag_engine::infrastructure::nodes::python_node::execute_sandboxed_helper;
use crate::llm::domain::tools::ToolDefinition;
use crate::text;
use schemars::JsonSchema;
use serde::Deserialize;

use super::diff_writer::diff_records;
use super::sheet_collision::{build_sheet_exists_error, parse_policy, CollisionPolicy, TabMeta};

pub use super::crdt_doc_context::CrdtDocsContext;

pub const TOOL_RUN_PYTHON: &str = "crdt_doc_run_python";

const CRDT_PY_PRELUDE: &str =
    include_str!("../../../../../text/prompts/python_sandbox/crdt_doc_run_python_prelude.md");
const CRDT_PY_POSTLUDE: &str =
    include_str!("../../../../../text/prompts/python_sandbox/crdt_doc_run_python_postlude.md");

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
    /// Operator-supplied collision policy. Default `fail`. Wired via
    /// `fixed_config.on_existing_sheet`. Accepted: fail, auto_suffix, overwrite.
    #[serde(default, alias = "on_existing_sheet")]
    pub on_existing_sheet: Option<String>,
}

/// Build the [`ToolDefinition`] for `crdt_doc_run_python`.
pub fn tool_run_python() -> ToolDefinition {
    super::build_synthetic_tool_with_summary::<RunPythonArgs>(
        TOOL_RUN_PYTHON,
        text::tool_description(TOOL_RUN_PYTHON),
        text::tool_summary(TOOL_RUN_PYTHON),
    )
}

/// Execute `run_python` against the runtime: extract records from the
/// requested sheets, run the user's wrapped code in the sandbox, and
/// optionally persist `output_sheet` as a new sheet.
pub async fn execute_run_python(ctx: &CrdtDocsContext, args: RunPythonArgs) -> serde_json::Value {
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
    // Capture column names per sheet — surfaced on errors so the LLM can
    // self-correct without an extra round trip (e.g. when its pandas code
    // hits a KeyError because the header row in the Y.Doc was a single
    // title cell instead of the real column names).
    let mut loaded_sheet_columns = serde_json::Map::new();
    for (sid, recs) in &records_by_sheet {
        let array: Vec<serde_json::Value> = recs
            .records
            .iter()
            .cloned()
            .map(serde_json::Value::Object)
            .collect();
        dfs_raw_json.insert(sid.clone(), serde_json::Value::Array(array));
        loaded_sheet_columns.insert(
            sid.clone(),
            serde_json::Value::Array(
                recs.columns
                    .iter()
                    .map(|c| serde_json::Value::String(c.clone()))
                    .collect(),
            ),
        );
    }
    let loaded_sheet_columns = serde_json::Value::Object(loaded_sheet_columns);
    let mut inputs = serde_json::Map::new();
    inputs.insert(
        "_dfs_raw".to_string(),
        serde_json::Value::Object(dfs_raw_json),
    );

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
                "loaded_sheet_columns": loaded_sheet_columns,
            });
        }
        Ok(Err(join_err)) => {
            return serde_json::json!({
                "error": format!("internal join error: {join_err}"),
                "loaded_sheet_columns": loaded_sheet_columns,
            });
        }
        Err(_) => {
            return serde_json::json!({
                "error": format!("code execution exceeded {CODE_TIMEOUT_SECS}s timeout"),
                "loaded_sheet_columns": loaded_sheet_columns,
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
                        // D-T8: surface the recalc count so the agent can
                        // see whether dependent formulas refreshed as a
                        // side effect of the write (always 0 in the
                        // new-sheet path since the sheet was just created,
                        // but the field is part of the contract).
                        "cells_recalculated": wr.cells_recalculated,
                        // D-T8: per-cell warnings (NeedsBrowser, EvalError,
                        // Cycle, ParseError) bubbled up from
                        // `apply_set_cell_in_proc`. Always empty for the
                        // new-sheet path today, but the field is part of
                        // the contract so future in-place writers route
                        // through the same response shape.
                        "warnings": serde_json::to_value(&wr.warnings)
                            .unwrap_or(serde_json::Value::Array(vec![])),
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
                            origin: origin.clone(),
                            summary: format!(
                                "wrote {} rows via run_python to new sheet '{}'",
                                wr.n_rows, wr.resolved_name
                            ),
                        })
                        .await
                        .unwrap_or(0);
                    ctx.record_event_id(event_id);

                    // D-T8: emit `formula_replaced_by_literal` events for any
                    // cells whose formula was overwritten with a literal. In
                    // the new-sheet path `formula_replacements` is always
                    // empty (the sheet was just created and had no prior
                    // formulas), so this loop is a no-op today. Kept here so
                    // that future in-place writers exercising
                    // `apply_records_to_doc` route through the same event-
                    // emission path with no caller changes.
                    for repl in &wr.formula_replacements {
                        let _ = ctx
                            .backend()
                            .record_event(crate::crdt_documents::change_tracker_store::NewEvent {
                                artifact_id: ctx.artifact_id().clone(),
                                sheet_id: Some(repl.sheet.clone()),
                                origin: origin.clone(),
                                summary: format!(
                                    "formula_replaced_by_literal: {}!{} (was '{}')",
                                    repl.sheet, repl.addr, repl.prior_formula
                                ),
                            })
                            .await;
                    }
                }
                Err(e) => {
                    return serde_json::json!({
                        "output": user_output,
                        "wrote_sheet": serde_json::Value::Null,
                        "stdout": truncate(&helper_result.stdout, STDOUT_BYTE_CAP),
                        "error": format!("write_to_sheet failed: {e}"),
                        "loaded_sheet_columns": loaded_sheet_columns,
                    });
                }
            }
        }
    }

    // 8. Multi-sheet path with mode dispatch (replace / overwrite / update_in_place).
    let mut wrote_sheets_response = serde_json::Value::Null;
    if let Some(sheets_value) = wrapped_output.get("output_sheets") {
        if !sheets_value.is_null() {
            let policy: CollisionPolicy = args
                .on_existing_sheet
                .as_deref()
                .map(parse_policy)
                .transpose()
                .ok()
                .flatten()
                .unwrap_or_default();
            let map = sheets_value.as_object().cloned().unwrap_or_default();
            let mut results: Vec<serde_json::Value> = Vec::new();
            for (raw_name, entry) in map {
                if let Some(err) = entry.get("_postlude_error").and_then(|v| v.as_str()) {
                    results.push(serde_json::json!({"name": raw_name, "error": err}));
                    continue;
                }
                let mode = entry
                    .get("mode")
                    .and_then(|v| v.as_str())
                    .unwrap_or("replace");
                let result = match mode {
                    "update_in_place" => crdt_update_in_place(&doc, ctx, &raw_name, &entry).await,
                    "overwrite" => crdt_overwrite(&doc, ctx, &raw_name, &entry).await,
                    "replace" => crdt_replace(&doc, ctx, &raw_name, &entry, policy).await,
                    other => serde_json::json!({
                        "name": raw_name,
                        "error": format!("unknown mode '{other}'; valid: replace, update_in_place, overwrite"),
                    }),
                };
                results.push(result);
            }
            wrote_sheets_response = serde_json::Value::Array(results);
        }
    }

    // 9. Truncate user output JSON if too large.
    let (user_output_capped, output_truncated) = truncate_json(&user_output, OUTPUT_BYTE_CAP);

    let mut response = serde_json::json!({
        "output": user_output_capped,
        "wrote_sheet": wrote_sheet_response,
        "wrote_sheets": wrote_sheets_response,
        "stdout": truncate(&helper_result.stdout, STDOUT_BYTE_CAP),
        "error": serde_json::Value::Null,
    });
    if output_truncated {
        response["_output_truncated"] = serde_json::json!(true);
    }
    // Surface a warning if BOTH single + multi paths produced writes.
    if !wrote_sheet_response.is_null() && !wrote_sheets_response.is_null() {
        response["_warning"] = serde_json::json!(
            "Both 'output_sheet' (legacy) and 'output_sheets' were set; both \
             were written. Prefer 'output_sheets' for new code."
        );
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

// ── crdt helpers for the modes ───────────────────────────────────────────

/// Look up tab metadata in the CRDT projection. Returns:
/// - (sheet_id, TabMeta) when the named sheet exists with non-empty cells.
/// - (sheet_id, TabMeta { columns: [], n_rows: 0, n_cols: 0 }) when the sheet
///   exists but has no cells yet.
/// - None when no sheet with that name exists.
fn crdt_lookup_tab_by_name(doc: &yrs::Doc, name: &str) -> Option<(String, TabMeta)> {
    use crate::crdt_documents::build_sheet_records;
    let proj = crate::crdt_documents::projection::project(doc);
    let sheets = proj["sheets"].as_array().cloned().unwrap_or_default();
    let sid = sheets
        .into_iter()
        .find(|s| s["name"].as_str() == Some(name))
        .and_then(|s| s["id"].as_str().map(String::from))?;
    let recs = build_sheet_records(doc, &sid).ok()?;
    let meta = TabMeta {
        n_rows: recs.records.len() as u64,
        n_cols: recs.columns.len() as u64,
        columns: recs.columns,
    };
    Some((sid, meta))
}

async fn crdt_replace(
    doc: &yrs::Doc,
    ctx: &CrdtDocsContext,
    raw_name: &str,
    entry: &serde_json::Value,
    policy: CollisionPolicy,
) -> serde_json::Value {
    if let Some((_sid, meta)) = crdt_lookup_tab_by_name(doc, raw_name) {
        match policy {
            CollisionPolicy::Fail => {
                return build_sheet_exists_error(raw_name, None, &meta);
            }
            CollisionPolicy::Overwrite => {
                return crdt_write_full(doc, ctx, raw_name, raw_name, entry).await;
            }
            CollisionPolicy::AutoSuffix => {
                // Start at 2: raw_name itself is known to collide (we just
                // hit `Some(meta)` above). First candidate is "raw_name (2)".
                for attempt in 2i32..=10 {
                    let candidate = format!("{raw_name} ({attempt})");
                    if crdt_lookup_tab_by_name(doc, &candidate).is_none() {
                        return crdt_write_full(doc, ctx, raw_name, &candidate, entry).await;
                    }
                }
                return serde_json::json!({
                    "name": raw_name,
                    "error": "auto_suffix: all 10 name attempts already exist",
                });
            }
        }
    }
    crdt_write_full(doc, ctx, raw_name, raw_name, entry).await
}

async fn crdt_overwrite(
    doc: &yrs::Doc,
    ctx: &CrdtDocsContext,
    raw_name: &str,
    entry: &serde_json::Value,
) -> serde_json::Value {
    if let Some((_sid, meta)) = crdt_lookup_tab_by_name(doc, raw_name) {
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
        let mismatch = {
            use std::collections::HashSet;
            let sa: HashSet<&String> = meta.columns.iter().collect();
            let sb: HashSet<&String> = input_cols.iter().collect();
            sa != sb
        };
        if mismatch && !allow {
            return serde_json::json!({
                "name": raw_name,
                "error": "SchemaChange",
                "current_columns": meta.columns,
                "input_columns": input_cols,
                "message": format!(
                    "Overwriting '{raw_name}' would change its schema: current {:?} → new {:?}. \
                     RECOMMENDED: use a different tab name. To proceed anyway, add \
                     'allow_schema_change: true' to the spec dict.",
                    meta.columns, input_cols
                ),
            });
        }
    }
    crdt_write_full(doc, ctx, raw_name, raw_name, entry).await
}

async fn crdt_update_in_place(
    doc: &yrs::Doc,
    ctx: &CrdtDocsContext,
    raw_name: &str,
    entry: &serde_json::Value,
) -> serde_json::Value {
    use crate::crdt_documents::build_sheet_records;
    use crate::crdt_documents::tool_executor::apply_set_cell_in_proc;
    let Some(key) = entry.get("key").and_then(|v| v.as_str()) else {
        return serde_json::json!({"tab": raw_name, "error": "update_in_place requires `key` field"});
    };
    let Some((sheet_id, _meta)) = crdt_lookup_tab_by_name(doc, raw_name) else {
        return serde_json::json!({
            "name": raw_name,
            "error": "UpdateRequiresExistingTab",
            "message": format!("update_in_place needs sheet '{raw_name}' to exist; use mode=replace to create it."),
        });
    };
    let recs = match build_sheet_records(doc, &sheet_id) {
        Ok(r) => r,
        Err(e) => return serde_json::json!({"tab": raw_name, "error": format!("read failed: {e}")}),
    };
    let current_records = recs.records;
    let header_cols = recs.columns;
    let new_records: Vec<serde_json::Map<String, serde_json::Value>> = entry
        .get("df_records")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|r| r.as_object().cloned()).collect())
        .unwrap_or_default();
    let restrict: Option<Vec<String>> = entry.get("columns").and_then(|v| v.as_array()).map(|a| {
        a.iter()
            .filter_map(|c| c.as_str().map(String::from))
            .collect()
    });
    let strict = entry
        .get("strict_match")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
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
    // Build index: key_value (string) → 1-based row in the Y.Doc cells map.
    // build_sheet_records emits records in row order, so row N (1-based, header is row 1)
    // for record index i is i + 2.
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
    let mut cells_written = 0usize;
    for chg in &diff.changes {
        let (Some(&row), Some(&col_idx)) = (
            key_to_row.get(&chg.key_value),
            col_to_index.get(&chg.column),
        ) else {
            continue;
        };
        let addr = {
            let mut col = String::new();
            let mut n = col_idx;
            loop {
                col.insert(0, (b'A' + (n % 26) as u8) as char);
                n /= 26;
                if n == 0 {
                    break;
                }
                n -= 1;
            }
            format!("{col}{row}")
        };
        let _ = apply_set_cell_in_proc(doc, &sheet_id, &addr, &chg.new_value);
        cells_written += 1;
    }
    if cells_written > 0 {
        ctx.mark_dirty();
        let origin = ctx
            .session_id()
            .map(|s| format!("agent:{s}"))
            .unwrap_or_else(|| "agent:llm".to_string());
        let event_id = ctx
            .backend()
            .record_event(crate::crdt_documents::change_tracker_store::NewEvent {
                artifact_id: ctx.artifact_id().clone(),
                sheet_id: Some(sheet_id.clone()),
                origin,
                summary: format!(
                    "update_in_place: patched {} cell(s) across {} row(s) in '{raw_name}'",
                    cells_written, diff.rows_changed
                ),
            })
            .await
            .unwrap_or(0);
        ctx.record_event_id(event_id);
    }
    serde_json::json!({
        "tab": raw_name,
        "sheet_id": sheet_id,
        "mode": "update_in_place",
        "changes": {
            "rows": diff.rows_changed,
            "cells": cells_written,
            "columns": diff.columns_touched,
        },
        "unchanged": {"rows": diff.rows_unchanged},
        "skipped": {
            "rows_not_in_target": diff.rows_skipped_not_in_target,
            "rows_null_key": diff.rows_skipped_null_key,
        },
    })
}

async fn crdt_write_full(
    doc: &yrs::Doc,
    ctx: &CrdtDocsContext,
    raw_name: &str,
    write_name: &str,
    entry: &serde_json::Value,
) -> serde_json::Value {
    use crate::crdt_documents::write_records_as_new_sheet;
    let records: Vec<serde_json::Map<String, serde_json::Value>> = entry
        .get("df_records")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|r| r.as_object().cloned()).collect())
        .unwrap_or_default();
    let cols: Vec<String> = entry
        .get("df_cols")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|c| c.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    match write_records_as_new_sheet(doc, write_name, &cols, &records) {
        Ok(wr) => {
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
                        "wrote {} rows via run_python to sheet '{}'",
                        wr.n_rows, wr.resolved_name
                    ),
                })
                .await
                .unwrap_or(0);
            ctx.record_event_id(event_id);
            serde_json::json!({
                "name": raw_name,
                "resolved_name": wr.resolved_name,
                "sheet_id": wr.sheet_id,
                "n_rows": wr.n_rows,
                "n_cols": wr.n_cols,
            })
        }
        Err(e) => serde_json::json!({
            "name": raw_name,
            "error": format!("write_records_as_new_sheet failed: {e}"),
        }),
    }
}

// ── helpers ──────────────────────────────────────────────────────────────

fn wrap_user_code(user_code: &str) -> String {
    format!("{CRDT_PY_PRELUDE}{user_code}{CRDT_PY_POSTLUDE}")
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

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crdt_documents::{ArtifactId, CrdtDocumentsRuntime};
    use crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::crdt_doc_tools::{
        execute_add_sheet, execute_list_sheets, execute_set_range, AddSheetArgs, SetRangeArgs,
    };
    use serde_json::json;
    use std::sync::Arc;

    async fn make_runtime() -> Arc<CrdtDocumentsRuntime> {
        let tmp = std::env::temp_dir().join(format!("crdt_py_{}", ulid::Ulid::new()));
        let cfg = json!({ "storage_root": tmp.to_str().unwrap() });
        Arc::new(CrdtDocumentsRuntime::from_config(&cfg).await.unwrap())
    }

    #[tokio::test]
    async fn multi_sheet_output_sheets_writes_three_new_tabs() {
        // Ensure the Python interpreter is ready (same pattern as python_node tests).
        pyo3::prepare_freethreaded_python();

        // Build in-memory runtime + artifact with one sheet "src" (3 rows).
        let rt = make_runtime().await;
        let id = ArtifactId::new();
        let _ = rt.registry.get_or_create(&id, "workbook");
        let ctx = CrdtDocsContext::new_local(rt, id, Some("test_session".to_string()));

        // Add the "src" sheet and seed header + 2 data rows.
        let add_result = execute_add_sheet(&ctx, AddSheetArgs { name: "src".into() }).await;
        let sheet_id = add_result["sheet_id"].as_str().unwrap().to_string();
        execute_set_range(
            &ctx,
            SetRangeArgs {
                sheet_id: sheet_id.clone(),
                start_addr: "A1".into(),
                values_2d: vec![
                    vec![json!("k"), json!("v")],
                    vec![json!("a"), json!("1")],
                    vec![json!("b"), json!("2")],
                ],
            },
        )
        .await;

        let args = RunPythonArgs {
            sheet_ids: vec![sheet_id.clone()],
            code: format!(
                r#"
import pandas as pd
df = pd.DataFrame(dfs["{sid}"])
output_sheets = {{
    "tab_one":   df.head(1),
    "tab_two":   df.tail(1),
    "tab_three": df,
}}
output = {{"done": True}}
"#,
                sid = sheet_id
            ),
            write_to_sheet: None,
            on_existing_sheet: None,
        };
        let result = execute_run_python(&ctx, args).await;

        // Skip gracefully if pandas is not installed in this test environment.
        if let Some(err) = result.get("error").and_then(|v| v.as_str()) {
            if err.contains("No module named 'pandas'") {
                eprintln!("SKIPPED (no pandas in test env): {err}");
                return;
            }
        }

        // The response must include 3 wrote_sheets entries.
        let wrote = result
            .get("wrote_sheets")
            .and_then(|v| v.as_array())
            .unwrap_or_else(|| {
                panic!(
                    "wrote_sheets should be an array; full result: {}",
                    serde_json::to_string_pretty(&result).unwrap_or_default()
                )
            });
        assert_eq!(
            wrote.len(),
            3,
            "expected 3 wrote_sheets entries, got {}; result: {}",
            wrote.len(),
            serde_json::to_string_pretty(&result).unwrap_or_default()
        );

        // Verify the 3 new sheets actually exist in the runtime.
        let list_result = execute_list_sheets(&ctx);
        let sheets = list_result["sheets"].as_array().expect("sheets array");
        let names: std::collections::HashSet<String> = sheets
            .iter()
            .filter_map(|s| s["name"].as_str().map(String::from))
            .collect();
        for needle in &["tab_one", "tab_two", "tab_three"] {
            assert!(
                names.contains(*needle),
                "missing sheet '{}'; sheets present: {:?}",
                needle,
                names
            );
        }
    }

    #[tokio::test]
    async fn update_in_place_patches_only_changed_cells_in_crdt() {
        pyo3::prepare_freethreaded_python();
        let rt = make_runtime().await;
        let id = ArtifactId::new();
        let _ = rt.registry.get_or_create(&id, "workbook");
        let ctx = CrdtDocsContext::new_local(rt, id, Some("test_session".to_string()));

        let add = execute_add_sheet(
            &ctx,
            AddSheetArgs {
                name: "Sales".into(),
            },
        )
        .await;
        let sheet_id = add["sheet_id"].as_str().unwrap().to_string();
        execute_set_range(
            &ctx,
            SetRangeArgs {
                sheet_id: sheet_id.clone(),
                start_addr: "A1".into(),
                values_2d: vec![
                    vec![json!("id"), json!("price")],
                    vec![json!("a"), json!(10)],
                    vec![json!("b"), json!(20)],
                    vec![json!("c"), json!(30)],
                ],
            },
        )
        .await;

        let args = RunPythonArgs {
            sheet_ids: vec![sheet_id.clone()],
            code: format!(
                r#"
import pandas as pd
df = pd.DataFrame(dfs["{sid}"])
df.loc[df['id'] == 'b', 'price'] = 99
output_sheets = {{
    'Sales': {{'mode': 'update_in_place', 'df': df, 'key': 'id'}},
}}
output = {{}}
"#,
                sid = sheet_id
            ),
            write_to_sheet: None,
            on_existing_sheet: None,
        };
        let result = execute_run_python(&ctx, args).await;

        if let Some(err) = result.get("error").and_then(|v| v.as_str()) {
            if err.contains("No module named 'pandas'") {
                eprintln!("SKIPPED (no pandas): {err}");
                return;
            }
        }
        let wrote = result["wrote_sheets"]
            .as_array()
            .expect("wrote_sheets array");
        assert_eq!(wrote.len(), 1);
        let entry = &wrote[0];
        assert_eq!(entry["mode"], "update_in_place");
        // Only the 'b' row changed → 1 cell.
        assert_eq!(entry["changes"]["cells"], 1);
    }
}
