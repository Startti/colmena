//! Synthetic LLM tools for the v1 CRDT documents feature.
//!
//! Mirrors the `document_tools.rs` pattern: each tool is a thin adapter that
//! builds a JSON Schema (via `schemars`), parses LLM-provided args into a
//! typed struct, and calls a function on `crdt_documents::tool_executor::*`
//! against the registered `Doc`.
//!
//! Tools refuse to operate if `artifact_id` is not registered in the
//! runtime. The `artifact_id` itself is injected by the executor from the
//! `llm_call.config.crdt_documents` block — the LLM never sets it.

#[cfg(test)]
use crate::crdt_documents::{ArtifactId, CrdtDocumentsRuntime};
use crate::llm::domain::tools::ToolDefinition;
use crate::text;
use schemars::JsonSchema;
use serde::Deserialize;
#[cfg(test)]
use std::sync::Arc;

pub use super::crdt_doc_context::CrdtDocsContext;

pub const TOOL_LIST_SHEETS: &str = "crdt_doc_list_sheets";

// ── list_sheets ───────────────────────────────────────────────────────────────

/// `list_sheets` takes no parameters; the artifact is resolved server-side.
#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct ListSheetsArgs {}

/// Build the [`ToolDefinition`] for `crdt_doc_list_sheets`.
pub fn tool_list_sheets() -> ToolDefinition {
    super::build_synthetic_tool_with_summary::<ListSheetsArgs>(
        TOOL_LIST_SHEETS,
        text::tool_description(TOOL_LIST_SHEETS),
        text::tool_summary(TOOL_LIST_SHEETS),
    )
}

/// Execute `list_sheets` against the runtime. Returns
/// `{ "sheets": [ { "sheet_id": "…", "name": "…", "formula_count": N }, … ] }`
/// or `{ "error": "artifact_not_found" }` when the id is not registered.
///
/// `formula_count` is the number of cells in the sheet that carry a non-empty
/// `f` (formula) entry. Lets the agent decide whether `include_formulas=true`
/// on a subsequent `crdt_doc_read` is worth the payload cost.
pub fn execute_list_sheets(ctx: &CrdtDocsContext) -> serde_json::Value {
    let Some(doc) = ctx.doc() else {
        return serde_json::json!({ "error": "artifact_not_found" });
    };
    let proj = crate::crdt_documents::projection::project(&doc);
    let sheets: Vec<serde_json::Value> = proj["sheets"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|s| {
            let sheet_id = s["id"].as_str().unwrap_or("");
            let formula_count =
                crate::crdt_documents::projection::count_formulas_in_sheet(&doc, sheet_id);
            serde_json::json!({
                "sheet_id": s["id"],
                "name": s["name"],
                "formula_count": formula_count,
            })
        })
        .collect();
    serde_json::json!({ "sheets": sheets })
}

// ─────────────────────────────────────────────────────────────────────────────
// crdt_doc_list_sheets_of — peek at another artifact's sheets (F)
// ─────────────────────────────────────────────────────────────────────────────

pub const TOOL_LIST_SHEETS_OF: &str = "crdt_doc_list_sheets_of";

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ListSheetsOfArgs {
    /// ID of the artifact whose sheets we want to list. Can be any artifact
    /// in the registry — does NOT enforce session ownership (the agent must
    /// have legitimately obtained this ID via list_my_artifacts, an explicit
    /// prompt, or future workspace listing).
    pub artifact_id: String,
}

pub fn tool_list_sheets_of() -> ToolDefinition {
    super::build_synthetic_tool_with_summary::<ListSheetsOfArgs>(
        TOOL_LIST_SHEETS_OF,
        text::tool_description(TOOL_LIST_SHEETS_OF),
        text::tool_summary(TOOL_LIST_SHEETS_OF),
    )
}

/// Core projection used by both `execute_list_sheets_of` (Local mode) and
/// the REST `/documents/:id/sheets-with-counts` handler (WsPeer mode).
///
/// Looks up `aid` in `runtime.registry` and returns the same JSON shape the
/// tool dispatcher returns:
///   `{ artifact_id, name, sheets: [{sheet_id, name, n_rows, n_cols}, …] }`
/// or `{ "error": "artifact_not_found", "artifact_id": "…" }`.
pub fn list_sheets_of_runtime(
    runtime: &crate::crdt_documents::CrdtDocumentsRuntime,
    aid: &crate::crdt_documents::ArtifactId,
) -> serde_json::Value {
    let Some(entry) = runtime.registry.get(aid) else {
        return serde_json::json!({
            "error": "artifact_not_found",
            "artifact_id": aid.to_string(),
        });
    };
    // Project sheets directly from the Y.Doc — counts computed on-the-fly
    // from each sheet's cells Y.Map (no SQL needed). Mirrors the trait
    // imports used in `crdt_documents::projection::project`.
    use yrs::{Array, Map, ReadTxn, Transact};
    let txn = entry.doc.transact();
    let workbook = match txn.get_map("workbook") {
        Some(m) => m,
        None => {
            return serde_json::json!({
                "artifact_id": aid.to_string(),
                "name": entry.meta.name.clone(),
                "sheets": [],
            });
        }
    };
    let sheets_arr = match workbook.get(&txn, "sheets") {
        Some(yrs::Out::YArray(a)) => a,
        _ => {
            return serde_json::json!({
                "artifact_id": aid.to_string(),
                "name": entry.meta.name.clone(),
                "sheets": [],
            });
        }
    };
    let mut sheets_out = Vec::with_capacity(sheets_arr.len(&txn) as usize);
    for i in 0..sheets_arr.len(&txn) {
        let sheet_map = match sheets_arr.get(&txn, i) {
            Some(yrs::Out::YMap(m)) => m,
            _ => continue,
        };
        let sid = match sheet_map.get(&txn, "id") {
            Some(yrs::Out::Any(yrs::Any::String(s))) => s.to_string(),
            _ => continue,
        };
        let name = match sheet_map.get(&txn, "name") {
            Some(yrs::Out::Any(yrs::Any::String(s))) => s.to_string(),
            _ => String::new(),
        };
        // Compute n_rows / n_cols by walking cells addresses
        let (n_rows, n_cols) = match sheet_map.get(&txn, "cells") {
            Some(yrs::Out::YMap(cells_map)) => {
                let mut max_row = 0u32;
                let mut max_col = 0u32;
                let mut has_any = false;
                for (addr, _) in cells_map.iter(&txn) {
                    if let Some((r, c)) = parse_a1_to_rc(addr) {
                        if !has_any {
                            max_row = r;
                            max_col = c;
                            has_any = true;
                        } else {
                            if r > max_row {
                                max_row = r;
                            }
                            if c > max_col {
                                max_col = c;
                            }
                        }
                    }
                }
                if has_any {
                    (max_row + 1, max_col + 1) // 1-indexed inclusive counts
                } else {
                    (0, 0)
                }
            }
            _ => (0, 0),
        };
        sheets_out.push(serde_json::json!({
            "sheet_id": sid,
            "name": name,
            "n_rows": n_rows,
            "n_cols": n_cols,
        }));
    }
    serde_json::json!({
        "artifact_id": aid.to_string(),
        "name": entry.meta.name.clone(),
        "sheets": sheets_out,
    })
}

pub async fn execute_list_sheets_of(
    ctx: &CrdtDocsContext,
    args: ListSheetsOfArgs,
) -> serde_json::Value {
    use crate::crdt_documents::ArtifactId;
    let aid: ArtifactId = match args.artifact_id.parse() {
        Ok(a) => a,
        Err(_) => {
            return serde_json::json!({
                "error": "invalid_artifact_id",
                "value": args.artifact_id,
            });
        }
    };
    match ctx {
        CrdtDocsContext::Local { runtime, .. } => list_sheets_of_runtime(runtime, &aid),
        CrdtDocsContext::WsPeer { backend, .. } => {
            // WsPeer mode: hit the server's `/documents/:id/sheets-with-counts`
            // endpoint. Downcast the backend to RestBackend to reuse its
            // client + base_url (same pattern used by execute_create_artifact).
            let rest_any = backend.as_ref() as &dyn std::any::Any;
            let Some(rest) = rest_any.downcast_ref::<crate::crdt_documents::RestBackend>() else {
                return serde_json::json!({
                    "error": "internal: wrong backend type for ws_peer mode"
                });
            };
            let url = format!("{}/documents/{}/sheets-with-counts", rest.base_url, aid);
            match rest.client.get(&url).send().await {
                Ok(resp) if resp.status().is_success() => {
                    match resp.json::<serde_json::Value>().await {
                        Ok(v) => v,
                        Err(e) => serde_json::json!({"error": format!("invalid_response: {e}")}),
                    }
                }
                Ok(resp) if resp.status() == reqwest::StatusCode::NOT_FOUND => {
                    serde_json::json!({
                        "error": "artifact_not_found",
                        "artifact_id": aid.to_string(),
                    })
                }
                Ok(resp) => serde_json::json!({
                    "error": format!("server_error_{}", resp.status()),
                }),
                Err(e) => serde_json::json!({"error": format!("http_error: {e}")}),
            }
        }
    }
}

pub async fn dispatch_crdt_doc_list_sheets_of(
    ctx: &CrdtDocsContext,
    args: serde_json::Value,
) -> serde_json::Value {
    match serde_json::from_value::<ListSheetsOfArgs>(args) {
        Ok(a) => execute_list_sheets_of(ctx, a).await,
        Err(e) => serde_json::json!({ "error": format!("invalid_args: {e}") }),
    }
}

// Internal helper — parses "A1", "AA12" into (row_idx0, col_idx0).
// Returns None if format is invalid.
pub(super) fn parse_a1_to_rc(addr: &str) -> Option<(u32, u32)> {
    let split = addr.find(|c: char| c.is_ascii_digit())?;
    if split == 0 {
        return None;
    }
    let col_part = &addr[..split];
    let row_part = &addr[split..];
    let row: u32 = row_part.parse().ok()?;
    let row = row.checked_sub(1)?;
    let mut col: u32 = 0;
    for ch in col_part.chars() {
        if !ch.is_ascii_alphabetic() {
            return None;
        }
        col = col
            .checked_mul(26)?
            .checked_add((ch.to_ascii_uppercase() as u32) - ('A' as u32) + 1)?;
    }
    Some((row, col.checked_sub(1)?))
}

// ── read ──────────────────────────────────────────────────────────────────────

pub const TOOL_READ: &str = "crdt_doc_read";
pub const TOOL_SET_CELL: &str = "crdt_doc_set_cell";
pub const TOOL_SET_RANGE: &str = "crdt_doc_set_range";
pub const TOOL_ADD_SHEET: &str = "crdt_doc_add_sheet";

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReadArgs {
    pub sheet_id: String,
    /// Optional A1-style range, e.g. "A1:D10". Omit for all cells.
    #[serde(default)]
    pub range: Option<String>,
    /// When true, each cell becomes `{v}` or `{v, f, fs}` (formula text +
    /// source tag) instead of a bare scalar. Default false keeps the
    /// pandas-friendly shape. Use for auditing or inspecting formulas.
    #[serde(default)]
    pub include_formulas: bool,
}

/// Build the [`ToolDefinition`] for `crdt_doc_read`.
pub fn tool_read() -> ToolDefinition {
    super::build_synthetic_tool_with_summary::<ReadArgs>(
        TOOL_READ,
        text::tool_description(TOOL_READ),
        text::tool_summary(TOOL_READ),
    )
}

/// Execute `read` against the runtime.
pub fn execute_read(ctx: &CrdtDocsContext, args: ReadArgs) -> serde_json::Value {
    let Some(doc) = ctx.doc() else {
        return serde_json::json!({ "error": "artifact_not_found" });
    };
    // Pre-parse range once so we can apply it to both branches uniformly
    // (and surface invalid_range before doing any projection work).
    // UX alias: a single A1 like "C1" auto-expands to "C1:C1" so the agent
    // doesn't waste a turn discovering the colon convention.
    let range_bounds = match args.range.as_deref() {
        None => None,
        Some(range) => {
            let normalized = if range.contains(':') {
                std::borrow::Cow::Borrowed(range)
            } else {
                std::borrow::Cow::Owned(format!("{range}:{range}"))
            };
            match parse_range(&normalized) {
                Some(b) => Some(b),
                None => return serde_json::json!({ "error": "invalid_range" }),
            }
        }
    };
    let in_range = |addr: &str| -> bool {
        match range_bounds {
            None => true,
            Some(((r0, c0), (r1, c1))) => match parse_a1(addr) {
                Some((r, c)) => r >= r0 && r <= r1 && c >= c0 && c <= c1,
                None => false,
            },
        }
    };

    if args.include_formulas {
        // D-T6: formula-aware shape. Verify the sheet exists by looking at
        // the projection's sheet ids — `project_sheet_cells_with_formulas`
        // returns an empty map for unknown sheets, which would otherwise
        // mask "sheet_not_found" as "empty sheet".
        let proj = crate::crdt_documents::projection::project(&doc);
        let sheet_exists = proj["sheets"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .any(|s| s["id"].as_str() == Some(args.sheet_id.as_str()))
            })
            .unwrap_or(false);
        if !sheet_exists {
            return serde_json::json!({ "error": "sheet_not_found" });
        }
        let all_cells = crate::crdt_documents::projection::project_sheet_cells_with_formulas(
            &doc,
            &args.sheet_id,
        );
        let filtered: serde_json::Map<String, serde_json::Value> = all_cells
            .into_iter()
            .filter(|(addr, _)| in_range(addr))
            .collect();
        return serde_json::json!({ "sheet_id": args.sheet_id, "cells": filtered });
    }

    // Default (back-compat): scalar values, pandas-friendly.
    let proj = crate::crdt_documents::projection::project(&doc);
    let sheets = proj["sheets"].as_array().cloned().unwrap_or_default();
    let Some(sheet) = sheets
        .into_iter()
        .find(|s| s["id"].as_str() == Some(args.sheet_id.as_str()))
    else {
        return serde_json::json!({ "error": "sheet_not_found" });
    };
    let cells = sheet["cells"].as_object().cloned().unwrap_or_default();
    let filtered: serde_json::Map<String, serde_json::Value> = cells
        .into_iter()
        .filter(|(addr, _)| in_range(addr))
        .collect();
    serde_json::json!({ "sheet_id": args.sheet_id, "cells": filtered })
}

fn parse_a1(addr: &str) -> Option<(u32, u32)> {
    let split = addr.find(|c: char| c.is_ascii_digit())?;
    let col_part = &addr[..split];
    let row_part = &addr[split..];
    let row: u32 = row_part.parse().ok()?;
    let row = row.checked_sub(1)?;
    let mut col: u32 = 0;
    for ch in col_part.chars() {
        if !ch.is_ascii_uppercase() {
            return None;
        }
        col = col * 26 + (ch as u32 - 'A' as u32 + 1);
    }
    Some((row, col.checked_sub(1)?))
}

fn parse_range(range: &str) -> Option<((u32, u32), (u32, u32))> {
    let (lhs, rhs) = range.split_once(':')?;
    Some((parse_a1(lhs)?, parse_a1(rhs)?))
}

fn col_letter(mut col: u32) -> String {
    let mut s = String::new();
    loop {
        s.insert(0, (b'A' + (col % 26) as u8) as char);
        if col < 26 {
            break;
        }
        col = col / 26 - 1;
    }
    s
}

// ── set_cell ──────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SetCellArgs {
    pub sheet_id: String,
    /// A1-style cell address (e.g. "A1"). Also accepts the alias `address`
    /// for ergonomics — the agent often guesses that name even though the
    /// canonical schema documents `addr`.
    #[serde(alias = "address")]
    pub addr: String,
    pub value: serde_json::Value,
}

/// Build the [`ToolDefinition`] for `crdt_doc_set_cell`.
pub fn tool_set_cell() -> ToolDefinition {
    super::build_synthetic_tool_with_summary::<SetCellArgs>(
        TOOL_SET_CELL,
        text::tool_description(TOOL_SET_CELL),
        text::tool_summary(TOOL_SET_CELL),
    )
}

/// Execute `set_cell` against the runtime.
pub async fn execute_set_cell(ctx: &CrdtDocsContext, args: SetCellArgs) -> serde_json::Value {
    let Some(doc) = ctx.doc() else {
        return serde_json::json!({ "error": "artifact_not_found" });
    };
    let outcome = crate::crdt_documents::tool_executor::apply_set_cell_in_proc(
        &doc,
        &args.sheet_id,
        &args.addr,
        &args.value,
    );
    ctx.mark_dirty();
    let origin = ctx
        .session_id()
        .map(|s| format!("agent:{s}"))
        .unwrap_or_else(|| "agent:llm".to_string());
    let event_id = ctx
        .backend()
        .record_event(crate::crdt_documents::change_tracker_store::NewEvent {
            artifact_id: ctx.artifact_id().clone(),
            sheet_id: Some(args.sheet_id.clone()),
            origin,
            summary: format!("set {}!{} = {}", args.sheet_id, args.addr, args.value),
        })
        .await
        .unwrap_or(0);
    ctx.record_event_id(event_id);
    // D-T5: surface SetCellOutcome to the agent so it can react to
    // unsupported functions (NeedsBrowser), eval errors, cycles, or
    // parse errors. `cells_recalculated` lets the agent see the
    // dependent cascade size; `warnings` is `[]` when everything was
    // clean (literal write or simple formula with no issues).
    serde_json::json!({
        "ok": true,
        "cells_recalculated": outcome.cells_recalculated,
        "warnings": outcome.warnings,
    })
}

// ── set_range ─────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SetRangeArgs {
    pub sheet_id: String,
    /// A1-style top-left cell of the range. Accepts `start` as alias.
    #[serde(alias = "start")]
    pub start_addr: String,
    /// Row-major 2D array of cell values. Accepts `values` as alias.
    #[serde(alias = "values")]
    pub values_2d: Vec<Vec<serde_json::Value>>,
}

/// Build the [`ToolDefinition`] for `crdt_doc_set_range`.
pub fn tool_set_range() -> ToolDefinition {
    super::build_synthetic_tool_with_summary::<SetRangeArgs>(
        TOOL_SET_RANGE,
        text::tool_description(TOOL_SET_RANGE),
        text::tool_summary(TOOL_SET_RANGE),
    )
}

/// Execute `set_range` against the runtime.
pub async fn execute_set_range(ctx: &CrdtDocsContext, args: SetRangeArgs) -> serde_json::Value {
    let Some(doc) = ctx.doc() else {
        return serde_json::json!({ "error": "artifact_not_found" });
    };
    let Some((r0, c0)) = parse_a1(&args.start_addr) else {
        return serde_json::json!({ "error": "invalid_start_addr" });
    };
    let mut cells_written = 0_usize;
    // D-T5: accumulate recalc count + warnings across the whole batch so
    // the agent sees one aggregate result (per-cell warnings carry their
    // own `addr` field, so they remain individually attributable).
    let mut total_cells_recalculated = 0_usize;
    let mut all_warnings: Vec<crate::crdt_documents::tool_executor::SetCellWarning> = Vec::new();
    for (dr, row) in args.values_2d.iter().enumerate() {
        for (dc, value) in row.iter().enumerate() {
            let r = r0 + dr as u32;
            let c = c0 + dc as u32;
            let addr = format!("{}{}", col_letter(c), r + 1);
            let outcome = crate::crdt_documents::tool_executor::apply_set_cell_in_proc(
                &doc,
                &args.sheet_id,
                &addr,
                value,
            );
            total_cells_recalculated += outcome.cells_recalculated;
            all_warnings.extend(outcome.warnings);
            cells_written += 1;
        }
    }
    ctx.mark_dirty();
    let origin = ctx
        .session_id()
        .map(|s| format!("agent:{s}"))
        .unwrap_or_else(|| "agent:llm".to_string());
    let event_id = ctx
        .backend()
        .record_event(crate::crdt_documents::change_tracker_store::NewEvent {
            artifact_id: ctx.artifact_id().clone(),
            sheet_id: Some(args.sheet_id.clone()),
            origin,
            summary: format!(
                "wrote {cells_written} cells starting at {}!{}",
                args.sheet_id, args.start_addr
            ),
        })
        .await
        .unwrap_or(0);
    ctx.record_event_id(event_id);
    serde_json::json!({
        "ok": true,
        "cells_written": cells_written,
        "total_cells_recalculated": total_cells_recalculated,
        "warnings": all_warnings,
    })
}

// ── add_sheet ─────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AddSheetArgs {
    pub name: String,
}

/// Build the [`ToolDefinition`] for `crdt_doc_add_sheet`.
pub fn tool_add_sheet() -> ToolDefinition {
    super::build_synthetic_tool_with_summary::<AddSheetArgs>(
        TOOL_ADD_SHEET,
        text::tool_description(TOOL_ADD_SHEET),
        text::tool_summary(TOOL_ADD_SHEET),
    )
}

/// Execute `add_sheet` against the runtime.
pub async fn execute_add_sheet(ctx: &CrdtDocsContext, args: AddSheetArgs) -> serde_json::Value {
    let Some(doc) = ctx.doc() else {
        return serde_json::json!({ "error": "artifact_not_found" });
    };
    let sheet_id = crate::crdt_documents::tool_executor::apply_add_sheet(&doc, &args.name);
    ctx.mark_dirty();
    let origin = ctx
        .session_id()
        .map(|s| format!("agent:{s}"))
        .unwrap_or_else(|| "agent:llm".to_string());
    let event_id = ctx
        .backend()
        .record_event(crate::crdt_documents::change_tracker_store::NewEvent {
            artifact_id: ctx.artifact_id().clone(),
            sheet_id: None,
            origin,
            summary: format!("added sheet '{}' (id={sheet_id})", args.name),
        })
        .await
        .unwrap_or(0);
    ctx.record_event_id(event_id);
    serde_json::json!({ "sheet_id": sheet_id })
}

// ── Dispatch wrappers (async — called from dag_tool_executor) ────────────────

pub async fn dispatch_crdt_doc_list_sheets(
    ctx: &CrdtDocsContext,
    _args: serde_json::Value,
) -> serde_json::Value {
    execute_list_sheets(ctx)
}

pub async fn dispatch_crdt_doc_read(
    ctx: &CrdtDocsContext,
    args: serde_json::Value,
) -> serde_json::Value {
    match serde_json::from_value::<ReadArgs>(args) {
        Ok(a) => execute_read(ctx, a),
        Err(e) => serde_json::json!({ "error": format!("invalid_args: {e}") }),
    }
}

pub async fn dispatch_crdt_doc_set_cell(
    ctx: &CrdtDocsContext,
    args: serde_json::Value,
) -> serde_json::Value {
    match serde_json::from_value::<SetCellArgs>(args) {
        Ok(a) => execute_set_cell(ctx, a).await,
        Err(e) => serde_json::json!({ "error": format!("invalid_args: {e}") }),
    }
}

pub async fn dispatch_crdt_doc_set_range(
    ctx: &CrdtDocsContext,
    args: serde_json::Value,
) -> serde_json::Value {
    match serde_json::from_value::<SetRangeArgs>(args) {
        Ok(a) => execute_set_range(ctx, a).await,
        Err(e) => serde_json::json!({ "error": format!("invalid_args: {e}") }),
    }
}

pub async fn dispatch_crdt_doc_add_sheet(
    ctx: &CrdtDocsContext,
    args: serde_json::Value,
) -> serde_json::Value {
    match serde_json::from_value::<AddSheetArgs>(args) {
        Ok(a) => execute_add_sheet(ctx, a).await,
        Err(e) => serde_json::json!({ "error": format!("invalid_args: {e}") }),
    }
}

// ── get_recent_changes ────────────────────────────────────────────────────────

pub const TOOL_GET_RECENT_CHANGES: &str = "crdt_doc_get_recent_changes";

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct GetRecentChangesArgs {
    /// Cursor — only events after this id. Default: agent's own cursor
    /// (looked up via backend.cursor_for).
    #[serde(default)]
    pub since_event_id: Option<u64>,
    /// Filter to one sheet. Default: all sheets.
    #[serde(default)]
    pub sheet_id: Option<String>,
    /// Cap result count. Default: 50.
    #[serde(default)]
    pub limit: Option<u32>,
    /// NEW in F (subsystem F): if provided, audits THIS artifact instead of
    /// the ctx's pinned artifact. Enables cross-artifact inspection without
    /// rebinding ctx. Default: ctx.artifact_id() (B behaviour unchanged).
    #[serde(default)]
    pub artifact_id: Option<String>,
}

/// Build the [`ToolDefinition`] for `crdt_doc_get_recent_changes`.
pub fn tool_get_recent_changes() -> ToolDefinition {
    super::build_synthetic_tool_with_summary::<GetRecentChangesArgs>(
        TOOL_GET_RECENT_CHANGES,
        text::tool_description(TOOL_GET_RECENT_CHANGES),
        text::tool_summary(TOOL_GET_RECENT_CHANGES),
    )
}

/// Execute `get_recent_changes` against the runtime.
pub async fn execute_get_recent_changes(
    ctx: &CrdtDocsContext,
    args: GetRecentChangesArgs,
) -> serde_json::Value {
    // F-T3: if args.artifact_id is provided, audit that artifact instead
    // of the ctx's pinned one (cross-artifact inspection).
    let target_aid: crate::crdt_documents::ArtifactId = match args.artifact_id.as_deref() {
        Some(s) => match s.parse() {
            Ok(a) => a,
            Err(_) => {
                return serde_json::json!({
                    "error": "invalid_artifact_id",
                    "value": s,
                });
            }
        },
        None => ctx.artifact_id().clone(),
    };
    let since = match args.since_event_id {
        Some(s) => s,
        None => match ctx.session_id() {
            Some(sid) => ctx
                .backend()
                .cursor_for(sid, &target_aid)
                .await
                .ok()
                .flatten()
                .unwrap_or(0),
            None => 0,
        },
    };
    let limit = args.limit.unwrap_or(50);
    let own_origin = ctx.session_id().map(|s| format!("agent:{s}"));
    let events = ctx
        .backend()
        .events_since(
            &target_aid,
            since,
            args.sheet_id.as_deref(),
            own_origin.as_deref(),
            limit,
        )
        .await
        .unwrap_or_default();
    let current_event_id = events.iter().map(|e| e.id).max();
    let truncated = (events.len() as u32) >= limit;
    serde_json::json!({
        "current_event_id": current_event_id,
        "events": events.iter().map(|e| serde_json::json!({
            "id": e.id,
            "origin": e.origin,
            "sheet_id": e.sheet_id,
            "summary": e.summary,
            "created_at": e.created_at,
        })).collect::<Vec<_>>(),
        "truncated": truncated,
    })
}

pub async fn dispatch_crdt_doc_get_recent_changes(
    ctx: &CrdtDocsContext,
    args: serde_json::Value,
) -> serde_json::Value {
    match serde_json::from_value::<GetRecentChangesArgs>(args) {
        Ok(a) => execute_get_recent_changes(ctx, a).await,
        Err(e) => serde_json::json!({ "error": format!("invalid_args: {e}") }),
    }
}

// ── list_my_artifacts ─────────────────────────────────────────────────────────

pub const TOOL_LIST_MY_ARTIFACTS: &str = "crdt_doc_list_my_artifacts";

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct ListMyArtifactsArgs {
    #[serde(default)]
    pub limit: Option<u32>,
}

/// Build the [`ToolDefinition`] for `crdt_doc_list_my_artifacts`.
pub fn tool_list_my_artifacts() -> ToolDefinition {
    super::build_synthetic_tool_with_summary::<ListMyArtifactsArgs>(
        TOOL_LIST_MY_ARTIFACTS,
        text::tool_description(TOOL_LIST_MY_ARTIFACTS),
        text::tool_summary(TOOL_LIST_MY_ARTIFACTS),
    )
}

/// Execute `list_my_artifacts` against the backend.
pub async fn execute_list_my_artifacts(
    ctx: &CrdtDocsContext,
    args: ListMyArtifactsArgs,
) -> serde_json::Value {
    let Some(sid) = ctx.session_id() else {
        return serde_json::json!({ "error": "session_required" });
    };
    let limit = args.limit.unwrap_or(50);
    let arts = ctx
        .backend()
        .artifacts_for_session(sid, limit)
        .await
        .unwrap_or_default();
    serde_json::json!({
        "artifacts": arts.iter().map(|a| serde_json::json!({
            "artifact_id": a.artifact_id,
            "name": a.name,
            "created_at": a.created_at,
            "last_accessed_at": a.last_accessed_at,
        })).collect::<Vec<_>>(),
    })
}

pub async fn dispatch_crdt_doc_list_my_artifacts(
    ctx: &CrdtDocsContext,
    args: serde_json::Value,
) -> serde_json::Value {
    match serde_json::from_value::<ListMyArtifactsArgs>(args) {
        Ok(a) => execute_list_my_artifacts(ctx, a).await,
        Err(e) => serde_json::json!({ "error": format!("invalid_args: {e}") }),
    }
}

// ── create_artifact ───────────────────────────────────────────────────────────

pub const TOOL_CREATE_ARTIFACT: &str = "crdt_doc_create_artifact";

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CreateArtifactArgs {
    pub name: String,
}

/// Build the [`ToolDefinition`] for `crdt_doc_create_artifact`.
pub fn tool_create_artifact() -> ToolDefinition {
    super::build_synthetic_tool_with_summary::<CreateArtifactArgs>(
        TOOL_CREATE_ARTIFACT,
        text::tool_description(TOOL_CREATE_ARTIFACT),
        text::tool_summary(TOOL_CREATE_ARTIFACT),
    )
}

/// Execute `create_artifact`. In local mode this creates a doc in the
/// shared `DocRegistry` and records a touch row in the backend. In
/// ws_peer mode it POSTs to the CRDT documents server's `/documents`
/// endpoint so the doc lives where the server can serve WS subscribers.
pub async fn execute_create_artifact(
    ctx: &CrdtDocsContext,
    args: CreateArtifactArgs,
) -> serde_json::Value {
    let Some(sid) = ctx.session_id() else {
        return serde_json::json!({ "error": "session_required" });
    };

    match ctx {
        CrdtDocsContext::Local { runtime, .. } => {
            let new_id = crate::crdt_documents::ArtifactId::new();
            let _ = runtime.registry.get_or_create(&new_id, &args.name);
            let _ = ctx
                .backend()
                .touch_artifact(sid, &new_id, Some(&args.name))
                .await;
            serde_json::json!({
                "artifact_id": new_id.to_string(),
                "name": args.name,
            })
        }
        CrdtDocsContext::WsPeer { backend, .. } => {
            // ws_peer mode: POST /documents to the server. Downcast the
            // backend trait object to RestBackend to reuse its
            // client + base_url. Requires `CrdtBackend: Any` (B-T5).
            let rest_any = backend.as_ref() as &dyn std::any::Any;
            let Some(rest) = rest_any.downcast_ref::<crate::crdt_documents::RestBackend>() else {
                return serde_json::json!({
                    "error": "internal: wrong backend type for ws_peer mode"
                });
            };
            let url = format!("{}/documents", rest.base_url);
            let body = serde_json::json!({
                "name": args.name,
                "agent_session_id": sid,
            });
            match rest.client.post(&url).json(&body).send().await {
                Ok(resp) if resp.status().is_success() => {
                    match resp.json::<serde_json::Value>().await {
                        Ok(j) => j,
                        Err(e) => serde_json::json!({ "error": format!("decode: {e}") }),
                    }
                }
                Ok(resp) => {
                    serde_json::json!({ "error": format!("status {}", resp.status()) })
                }
                Err(e) => serde_json::json!({ "error": format!("http: {e}") }),
            }
        }
    }
}

pub async fn dispatch_crdt_doc_create_artifact(
    ctx: &CrdtDocsContext,
    args: serde_json::Value,
) -> serde_json::Value {
    match serde_json::from_value::<CreateArtifactArgs>(args) {
        Ok(a) => execute_create_artifact(ctx, a).await,
        Err(e) => serde_json::json!({ "error": format!("invalid_args: {e}") }),
    }
}

// ── all tools ─────────────────────────────────────────────────────────────────

/// All CRDT document tool definitions.
pub fn build_all_crdt_doc_tools() -> Vec<ToolDefinition> {
    vec![
        tool_list_sheets(),
        tool_read(),
        tool_set_cell(),
        tool_set_range(),
        tool_add_sheet(),
        tool_get_recent_changes(),
        tool_list_my_artifacts(),
        tool_create_artifact(),
        tool_list_sheets_of(),
        super::crdt_doc_import_sheet::tool_import_sheet(),
        super::tool_run_python(),
    ]
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crdt_documents::tool_executor::apply_add_sheet;
    use serde_json::json;

    async fn make_runtime() -> Arc<CrdtDocumentsRuntime> {
        let tmp = std::env::temp_dir().join(format!("crdt_ls_{}", ulid::Ulid::new()));
        let cfg = json!({ "storage_root": tmp.to_str().unwrap() });
        Arc::new(CrdtDocumentsRuntime::from_config(&cfg).await.unwrap())
    }

    #[tokio::test]
    async fn lists_two_sheets() {
        let rt = make_runtime().await;
        let id = ArtifactId::new();
        let entry = rt.registry.get_or_create(&id, "workbook");
        apply_add_sheet(&entry.doc, "Sales");
        apply_add_sheet(&entry.doc, "Summary");
        let ctx = CrdtDocsContext::new_local(rt, id, Some("test_session".to_string()));
        let v = execute_list_sheets(&ctx);
        let sheets = v["sheets"].as_array().unwrap();
        assert_eq!(sheets.len(), 2);
        assert_eq!(sheets[0]["name"], "Sales");
        assert_eq!(sheets[1]["name"], "Summary");
        // sheet_id must be a non-empty string (sh_<ULID>)
        assert!(sheets[0]["sheet_id"].as_str().unwrap().starts_with("sh_"));
    }

    #[tokio::test]
    async fn returns_error_for_unknown_artifact() {
        let rt = make_runtime().await;
        let unknown = ArtifactId::new(); // never registered
        let ctx = CrdtDocsContext::new_local(rt, unknown, Some("test_session".to_string()));
        let v = execute_list_sheets(&ctx);
        assert_eq!(v["error"], "artifact_not_found");
    }

    #[tokio::test]
    async fn list_sheets_reports_formula_count() {
        // D-T7: per-sheet `formula_count` lets the agent decide whether
        // `crdt_doc_read(include_formulas=true)` is worth the payload cost.
        use crate::crdt_documents::tool_executor::{apply_add_sheet, apply_set_cell_in_proc};
        let rt = make_runtime().await;
        let id = ArtifactId::new();
        let entry = rt.registry.get_or_create(&id, "workbook");

        // Sheet1: A1=1 (literal), B1="=A1+1" (formula), C1="=A1*2" (formula)
        let s1 = apply_add_sheet(&entry.doc, "Sheet1");
        let _ = apply_set_cell_in_proc(&entry.doc, &s1, "A1", &json!(1));
        let _ = apply_set_cell_in_proc(&entry.doc, &s1, "B1", &json!("=A1+1"));
        let _ = apply_set_cell_in_proc(&entry.doc, &s1, "C1", &json!("=A1*2"));

        // Sheet2: A1=10 literal only (no formulas).
        let s2 = apply_add_sheet(&entry.doc, "Sheet2");
        let _ = apply_set_cell_in_proc(&entry.doc, &s2, "A1", &json!(10));

        let ctx = CrdtDocsContext::new_local(rt, id, Some("test_session".to_string()));
        let v = execute_list_sheets(&ctx);
        let sheets = v["sheets"].as_array().unwrap();
        assert_eq!(sheets.len(), 2);

        let by_name: std::collections::HashMap<&str, &serde_json::Value> = sheets
            .iter()
            .map(|s| (s["name"].as_str().unwrap(), s))
            .collect();
        assert_eq!(by_name["Sheet1"]["formula_count"], json!(2));
        assert_eq!(by_name["Sheet2"]["formula_count"], json!(0));
    }

    #[test]
    fn list_sheets_tool_def_has_correct_name() {
        let def = tool_list_sheets();
        assert_eq!(def.name, TOOL_LIST_SHEETS);
        assert!(!def.description.is_empty());
    }

    #[test]
    fn list_sheets_schema_has_no_visible_params() {
        let def = tool_list_sheets();
        let schema_str = def
            .input_schema_override
            .as_ref()
            .expect("must have input_schema_override")
            .to_string();
        // Empty args struct → schema should NOT expose artifact_id to the LLM
        assert!(!schema_str.contains("artifact_id"));
    }

    // ── helpers for new tool tests ────────────────────────────────────────────

    async fn fresh_ctx() -> (CrdtDocsContext, std::path::PathBuf) {
        let tmp = std::env::temp_dir().join(format!("t_{}", ulid::Ulid::new()));
        let cfg = json!({ "storage_root": tmp.to_str().unwrap() });
        let rt = Arc::new(CrdtDocumentsRuntime::from_config(&cfg).await.unwrap());
        let id = ArtifactId::new();
        let _ = rt.registry.get_or_create(&id, "t");
        (
            CrdtDocsContext::new_local(rt, id, Some("test_session".to_string())),
            tmp,
        )
    }

    /// Variant of `fresh_ctx` that uses a caller-supplied session id, so
    /// tests can verify session-scoped backend queries (artifacts_for_session).
    async fn fresh_ctx_with_session(session_id: &str) -> (CrdtDocsContext, std::path::PathBuf) {
        let tmp = std::env::temp_dir().join(format!("t_{}", ulid::Ulid::new()));
        let cfg = json!({ "storage_root": tmp.to_str().unwrap() });
        let rt = Arc::new(CrdtDocumentsRuntime::from_config(&cfg).await.unwrap());
        let id = ArtifactId::new();
        let _ = rt.registry.get_or_create(&id, "t");
        (
            CrdtDocsContext::new_local(rt, id, Some(session_id.to_string())),
            tmp,
        )
    }

    #[tokio::test]
    async fn set_cell_then_read_returns_value() {
        let (ctx, tmp) = fresh_ctx().await;
        let s = execute_add_sheet(&ctx, AddSheetArgs { name: "X".into() }).await;
        let sheet_id = s["sheet_id"].as_str().unwrap().to_string();
        execute_set_cell(
            &ctx,
            SetCellArgs {
                sheet_id: sheet_id.clone(),
                addr: "A1".into(),
                value: json!("hello"),
            },
        )
        .await;
        let v = execute_read(
            &ctx,
            ReadArgs {
                sheet_id,
                range: None,
                include_formulas: false,
            },
        );
        assert_eq!(v["cells"]["A1"], "hello");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn set_range_writes_2d_block() {
        let (ctx, tmp) = fresh_ctx().await;
        let s = execute_add_sheet(&ctx, AddSheetArgs { name: "X".into() }).await;
        let sheet_id = s["sheet_id"].as_str().unwrap().to_string();
        execute_set_range(
            &ctx,
            SetRangeArgs {
                sheet_id: sheet_id.clone(),
                start_addr: "B2".into(),
                values_2d: vec![vec![json!("a"), json!("b")], vec![json!(1), json!(2)]],
            },
        )
        .await;
        let v = execute_read(
            &ctx,
            ReadArgs {
                sheet_id,
                range: None,
                include_formulas: false,
            },
        );
        assert_eq!(v["cells"]["B2"], "a");
        assert_eq!(v["cells"]["C2"], "b");
        assert_eq!(v["cells"]["B3"], json!(1.0));
        assert_eq!(v["cells"]["C3"], json!(2.0));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn execute_set_cell_surfaces_cells_recalculated_and_warnings() {
        // Seed: A1=1, B1="=A1+1", then set A1 to "=BOGUSFN(1)" so the
        // new write emits a NeedsBrowser warning. The dispatcher must
        // pass both `cells_recalculated` and `warnings` through to the
        // agent.
        let (ctx, tmp) = fresh_ctx().await;
        let s = execute_add_sheet(&ctx, AddSheetArgs { name: "X".into() }).await;
        let sheet_id = s["sheet_id"].as_str().unwrap().to_string();
        execute_set_cell(
            &ctx,
            SetCellArgs {
                sheet_id: sheet_id.clone(),
                addr: "A1".into(),
                value: json!(1),
            },
        )
        .await;
        execute_set_cell(
            &ctx,
            SetCellArgs {
                sheet_id: sheet_id.clone(),
                addr: "B1".into(),
                value: json!("=A1+1"),
            },
        )
        .await;

        // The NeedsBrowser write — A1 becomes a needs_browser placeholder.
        let v = execute_set_cell(
            &ctx,
            SetCellArgs {
                sheet_id: sheet_id.clone(),
                addr: "A1".into(),
                value: json!("=BOGUSFN(1)"),
            },
        )
        .await;
        assert_eq!(v["ok"], true);
        assert!(
            v.get("cells_recalculated").is_some(),
            "tool result must carry cells_recalculated; got {v}"
        );
        let warnings = v["warnings"].as_array().expect("warnings must be an array");
        assert_eq!(
            warnings.len(),
            1,
            "expected one NeedsBrowser warning; got {v}"
        );
        assert_eq!(warnings[0]["kind"], "needs_browser");
        assert_eq!(warnings[0]["addr"], "A1");

        // A clean write should still produce an empty warnings array (not
        // missing, not null) so the agent never has to special-case shape.
        let v_clean = execute_set_cell(
            &ctx,
            SetCellArgs {
                sheet_id: sheet_id.clone(),
                addr: "C1".into(),
                value: json!(42),
            },
        )
        .await;
        assert_eq!(v_clean["ok"], true);
        assert_eq!(v_clean["warnings"], json!([]));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn execute_set_range_aggregates_recalc_and_warnings() {
        // Range write where one cell triggers a NeedsBrowser warning.
        // Result must aggregate: total_cells_recalculated is a number,
        // warnings is a flat list across the batch.
        let (ctx, tmp) = fresh_ctx().await;
        let s = execute_add_sheet(&ctx, AddSheetArgs { name: "X".into() }).await;
        let sheet_id = s["sheet_id"].as_str().unwrap().to_string();
        let v = execute_set_range(
            &ctx,
            SetRangeArgs {
                sheet_id: sheet_id.clone(),
                start_addr: "A1".into(),
                values_2d: vec![vec![json!(1), json!("=BOGUSFN(1)"), json!("hello")]],
            },
        )
        .await;
        assert_eq!(v["ok"], true);
        assert_eq!(v["cells_written"], 3);
        assert!(v.get("total_cells_recalculated").is_some());
        let warnings = v["warnings"].as_array().expect("warnings array");
        assert_eq!(
            warnings.len(),
            1,
            "expected one warning across the batch; got {v}"
        );
        assert_eq!(warnings[0]["kind"], "needs_browser");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn read_with_include_formulas_returns_v_f_fs() {
        // D-T6: include_formulas=true → cells become {v}/{v,f,fs} objects.
        let (ctx, tmp) = fresh_ctx().await;
        let s = execute_add_sheet(&ctx, AddSheetArgs { name: "X".into() }).await;
        let sheet_id = s["sheet_id"].as_str().unwrap().to_string();

        // Literal cell.
        execute_set_cell(
            &ctx,
            SetCellArgs {
                sheet_id: sheet_id.clone(),
                addr: "A1".into(),
                value: json!(5),
            },
        )
        .await;
        // Formula cell — should be backend-evaluated (fs="be") with v=10.
        execute_set_cell(
            &ctx,
            SetCellArgs {
                sheet_id: sheet_id.clone(),
                addr: "B1".into(),
                value: json!("=A1*2"),
            },
        )
        .await;

        // Default read (back-compat): scalars.
        let v_scalar = execute_read(
            &ctx,
            ReadArgs {
                sheet_id: sheet_id.clone(),
                range: None,
                include_formulas: false,
            },
        );
        assert_eq!(v_scalar["cells"]["A1"], json!(5.0));
        assert_eq!(v_scalar["cells"]["B1"], json!(10.0));

        // Formula-aware read.
        let v = execute_read(
            &ctx,
            ReadArgs {
                sheet_id: sheet_id.clone(),
                range: None,
                include_formulas: true,
            },
        );
        let a1 = v["cells"]["A1"].as_object().expect("A1 is object");
        assert_eq!(a1["v"], json!(5.0));
        assert!(a1.get("f").is_none());
        assert!(a1.get("fs").is_none());

        let b1 = v["cells"]["B1"].as_object().expect("B1 is object");
        assert_eq!(b1["v"], json!(10.0));
        assert_eq!(b1["f"], json!("=A1*2"));
        assert_eq!(b1["fs"], json!("be"));

        // Sheet-not-found still surfaces with include_formulas=true.
        let err = execute_read(
            &ctx,
            ReadArgs {
                sheet_id: "sh_nope".into(),
                range: None,
                include_formulas: true,
            },
        );
        assert_eq!(err["error"], "sheet_not_found");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn read_with_range_filters() {
        let (ctx, tmp) = fresh_ctx().await;
        let s = execute_add_sheet(&ctx, AddSheetArgs { name: "X".into() }).await;
        let sheet_id = s["sheet_id"].as_str().unwrap().to_string();
        execute_set_cell(
            &ctx,
            SetCellArgs {
                sheet_id: sheet_id.clone(),
                addr: "A1".into(),
                value: json!(1),
            },
        )
        .await;
        execute_set_cell(
            &ctx,
            SetCellArgs {
                sheet_id: sheet_id.clone(),
                addr: "Z99".into(),
                value: json!(2),
            },
        )
        .await;
        let v = execute_read(
            &ctx,
            ReadArgs {
                sheet_id,
                range: Some("A1:B2".into()),
                include_formulas: false,
            },
        );
        assert_eq!(v["cells"].as_object().unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    #[ignore = "requires isolated DB — fails against a shared DATABASE_URL due to cross-test state; run with `cargo test -- --ignored`"]
    async fn get_recent_changes_empty_then_populated() {
        let (ctx, tmp) = fresh_ctx().await;

        // Initially: no changes.
        let v = execute_get_recent_changes(
            &ctx,
            GetRecentChangesArgs {
                since_event_id: None,
                sheet_id: None,
                limit: None,
                artifact_id: None,
            },
        )
        .await;
        let events = v["events"].as_array().unwrap();
        assert!(events.is_empty());
        assert_eq!(v["truncated"], false);

        // Seed a peer event (different origin so the own-origin filter
        // doesn't hide it).
        let peer_event_id = ctx
            .backend()
            .record_event(crate::crdt_documents::change_tracker_store::NewEvent {
                artifact_id: ctx.artifact_id().clone(),
                sheet_id: None,
                origin: "agent:peer_session".to_string(),
                summary: "added sheet 'Sales' (id=sh_peer)".to_string(),
            })
            .await
            .unwrap();
        assert!(peer_event_id > 0);

        // Also do an own-session mutation — it should NOT appear (own-origin
        // filter excludes it).
        let _ = execute_add_sheet(&ctx, AddSheetArgs { name: "Own".into() }).await;

        let v = execute_get_recent_changes(
            &ctx,
            GetRecentChangesArgs {
                since_event_id: None,
                sheet_id: None,
                limit: None,
                artifact_id: None,
            },
        )
        .await;
        let events = v["events"].as_array().unwrap();
        assert_eq!(events.len(), 1, "only the peer event should appear");
        assert_eq!(events[0]["origin"], "agent:peer_session");
        let s = events[0]["summary"].as_str().unwrap();
        assert!(s.contains("added sheet"), "got: {s}");
        assert!(s.contains("Sales"), "got: {s}");
        let current = v["current_event_id"].as_u64().unwrap();

        // since_event_id filters: passing current returns empty events.
        let v = execute_get_recent_changes(
            &ctx,
            GetRecentChangesArgs {
                since_event_id: Some(current),
                sheet_id: None,
                limit: None,
                artifact_id: None,
            },
        )
        .await;
        assert!(v["events"].as_array().unwrap().is_empty());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    #[ignore = "requires isolated DB — fails against a shared DATABASE_URL due to cross-test state; run with `cargo test -- --ignored`"]
    async fn list_my_artifacts_returns_session_artifacts() {
        let (ctx, tmp) = fresh_ctx_with_session("s_list").await;
        let aid1 = ArtifactId::new();
        let aid2 = ArtifactId::new();
        ctx.backend()
            .touch_artifact("s_list", &aid1, Some("First"))
            .await
            .unwrap();
        ctx.backend()
            .touch_artifact("s_list", &aid2, Some("Second"))
            .await
            .unwrap();
        let v = execute_list_my_artifacts(&ctx, ListMyArtifactsArgs { limit: None }).await;
        assert_eq!(v["artifacts"].as_array().unwrap().len(), 2);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn create_artifact_returns_new_id_local_mode() {
        let (ctx, tmp) = fresh_ctx_with_session("s_create").await;
        let v = execute_create_artifact(
            &ctx,
            CreateArtifactArgs {
                name: "Inventory Q3".into(),
            },
        )
        .await;
        assert!(v["artifact_id"].as_str().unwrap().starts_with("art_"));
        assert_eq!(v["name"].as_str().unwrap(), "Inventory Q3");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    // ── D-T16 UX alias coverage ───────────────────────────────────────────

    #[test]
    fn set_cell_args_accept_address_alias() {
        let v = json!({
            "sheet_id": "Sheet1",
            "address": "A1",
            "value": 42,
        });
        let parsed: SetCellArgs = serde_json::from_value(v).unwrap();
        assert_eq!(parsed.addr, "A1");
    }

    #[test]
    fn set_cell_args_canonical_addr_still_works() {
        let v = json!({
            "sheet_id": "Sheet1",
            "addr": "B2",
            "value": "x",
        });
        let parsed: SetCellArgs = serde_json::from_value(v).unwrap();
        assert_eq!(parsed.addr, "B2");
    }

    #[test]
    fn set_range_args_accept_start_and_values_aliases() {
        let v = json!({
            "sheet_id": "Sheet1",
            "start": "A1",
            "values": [[1, 2], [3, 4]],
        });
        let parsed: SetRangeArgs = serde_json::from_value(v).unwrap();
        assert_eq!(parsed.start_addr, "A1");
        assert_eq!(parsed.values_2d.len(), 2);
        assert_eq!(parsed.values_2d[1][1], json!(4));
    }

    #[tokio::test]
    async fn read_accepts_single_a1_range() {
        // D-T16: passing `range: "C1"` (no colon) should auto-expand to
        // `C1:C1` instead of returning `invalid_range`.
        let (ctx, tmp) = fresh_ctx().await;
        let s = execute_add_sheet(&ctx, AddSheetArgs { name: "X".into() }).await;
        let sheet_id = s["sheet_id"].as_str().unwrap().to_string();
        execute_set_cell(
            &ctx,
            SetCellArgs {
                sheet_id: sheet_id.clone(),
                addr: "C1".into(),
                value: json!("hi"),
            },
        )
        .await;
        let v = execute_read(
            &ctx,
            ReadArgs {
                sheet_id: sheet_id.clone(),
                range: Some("C1".into()),
                include_formulas: false,
            },
        );
        assert_eq!(v["cells"]["C1"], "hi");
        // Nothing outside C1 leaked in.
        assert_eq!(v["cells"].as_object().unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
