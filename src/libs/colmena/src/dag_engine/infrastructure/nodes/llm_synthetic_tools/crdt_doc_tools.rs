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
    super::build_synthetic_tool::<ListSheetsArgs>(
        TOOL_LIST_SHEETS,
        "List the sheets in the current CRDT document. Returns id + name for each sheet.",
    )
}

/// Execute `list_sheets` against the runtime. Returns
/// `{ "sheets": [ { "sheet_id": "…", "name": "…" }, … ] }` or
/// `{ "error": "artifact_not_found" }` when the id is not registered.
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
            serde_json::json!({
                "sheet_id": s["id"],
                "name": s["name"],
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
    /// ID del artifact cuyo listado de sheets queremos. Puede ser cualquier
    /// artifact del registry — NO enforce session ownership (el agente debe
    /// haber obtenido el ID legítimamente vía list_my_artifacts, prompt
    /// explícito, o futuro workspace listing).
    pub artifact_id: String,
}

pub fn tool_list_sheets_of() -> ToolDefinition {
    super::build_synthetic_tool::<ListSheetsOfArgs>(
        TOOL_LIST_SHEETS_OF,
        "List the sheets of a different artifact (not the current one). \
         Use this to peek at what's inside another workbook BEFORE deciding \
         to clone a sheet from it via crdt_doc_import_sheet. Returns \
         {artifact_id, name, sheets:[{sheet_id, name, n_rows, n_cols}]}. \
         The agent must already know the target artifact_id (from \
         crdt_doc_list_my_artifacts or from the user's prompt).",
    )
}

pub fn execute_list_sheets_of(ctx: &CrdtDocsContext, args: ListSheetsOfArgs) -> serde_json::Value {
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
    // Cross-artifact peek requires registry access — only available in
    // Local mode. WsPeer mode would need a server-side endpoint (deferred
    // to a follow-up task).
    let runtime = match ctx {
        CrdtDocsContext::Local { runtime, .. } => runtime,
        CrdtDocsContext::WsPeer { .. } => {
            return serde_json::json!({
                "error": "unsupported_in_ws_peer_mode",
                "message": "crdt_doc_list_sheets_of is only available in local mode",
            });
        }
    };
    let Some(entry) = runtime.registry.get(&aid) else {
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

pub async fn dispatch_crdt_doc_list_sheets_of(
    ctx: &CrdtDocsContext,
    args: serde_json::Value,
) -> serde_json::Value {
    match serde_json::from_value::<ListSheetsOfArgs>(args) {
        Ok(a) => execute_list_sheets_of(ctx, a),
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
}

/// Build the [`ToolDefinition`] for `crdt_doc_read`.
pub fn tool_read() -> ToolDefinition {
    super::build_synthetic_tool::<ReadArgs>(
        TOOL_READ,
        "Read cell values from a sheet. Returns a flat map of A1 addresses \
         to values. Optionally restricts to a range.",
    )
}

/// Execute `read` against the runtime.
pub fn execute_read(ctx: &CrdtDocsContext, args: ReadArgs) -> serde_json::Value {
    let Some(doc) = ctx.doc() else {
        return serde_json::json!({ "error": "artifact_not_found" });
    };
    let proj = crate::crdt_documents::projection::project(&doc);
    let sheets = proj["sheets"].as_array().cloned().unwrap_or_default();
    let Some(sheet) = sheets
        .into_iter()
        .find(|s| s["id"].as_str() == Some(args.sheet_id.as_str()))
    else {
        return serde_json::json!({ "error": "sheet_not_found" });
    };
    let cells = sheet["cells"].as_object().cloned().unwrap_or_default();
    let filtered: serde_json::Map<String, serde_json::Value> = match args.range {
        None => cells.into_iter().collect(),
        Some(range) => {
            let Some(((r0, c0), (r1, c1))) = parse_range(&range) else {
                return serde_json::json!({ "error": "invalid_range" });
            };
            cells
                .into_iter()
                .filter(|(addr, _)| match parse_a1(addr) {
                    Some((r, c)) => r >= r0 && r <= r1 && c >= c0 && c <= c1,
                    None => false,
                })
                .collect()
        }
    };
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
    pub addr: String,
    pub value: serde_json::Value,
}

/// Build the [`ToolDefinition`] for `crdt_doc_set_cell`.
pub fn tool_set_cell() -> ToolDefinition {
    super::build_synthetic_tool::<SetCellArgs>(
        TOOL_SET_CELL,
        "Set a single cell. Value may be string, number, boolean, or null \
         (null deletes).",
    )
}

/// Execute `set_cell` against the runtime.
pub async fn execute_set_cell(ctx: &CrdtDocsContext, args: SetCellArgs) -> serde_json::Value {
    let Some(doc) = ctx.doc() else {
        return serde_json::json!({ "error": "artifact_not_found" });
    };
    crate::crdt_documents::tool_executor::apply_set_cell_in_proc(
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
    serde_json::json!({ "ok": true })
}

// ── set_range ─────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SetRangeArgs {
    pub sheet_id: String,
    pub start_addr: String,
    /// Row-major 2D array of cell values.
    pub values_2d: Vec<Vec<serde_json::Value>>,
}

/// Build the [`ToolDefinition`] for `crdt_doc_set_range`.
pub fn tool_set_range() -> ToolDefinition {
    super::build_synthetic_tool::<SetRangeArgs>(
        TOOL_SET_RANGE,
        "Bulk set a rectangular range starting at start_addr. values_2d is \
         a row-major 2D array.",
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
    for (dr, row) in args.values_2d.iter().enumerate() {
        for (dc, value) in row.iter().enumerate() {
            let r = r0 + dr as u32;
            let c = c0 + dc as u32;
            let addr = format!("{}{}", col_letter(c), r + 1);
            crate::crdt_documents::tool_executor::apply_set_cell_in_proc(
                &doc,
                &args.sheet_id,
                &addr,
                value,
            );
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
    serde_json::json!({ "ok": true, "cells_written": cells_written })
}

// ── add_sheet ─────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AddSheetArgs {
    pub name: String,
}

/// Build the [`ToolDefinition`] for `crdt_doc_add_sheet`.
pub fn tool_add_sheet() -> ToolDefinition {
    super::build_synthetic_tool::<AddSheetArgs>(
        TOOL_ADD_SHEET,
        "Append a new sheet with the given name. Returns the generated sheet_id.",
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
}

/// Build the [`ToolDefinition`] for `crdt_doc_get_recent_changes`.
pub fn tool_get_recent_changes() -> ToolDefinition {
    super::build_synthetic_tool::<GetRecentChangesArgs>(
        TOOL_GET_RECENT_CHANGES,
        "Get recent peer changes to the document (events from other \
         sessions; the agent's own mutations are excluded). Optionally \
         filter by since_event_id, sheet_id, and limit. Returns \
         { current_event_id, events: [...], truncated } where events is \
         an array of { id, origin, sheet_id, summary, created_at }.",
    )
}

/// Execute `get_recent_changes` against the runtime.
pub async fn execute_get_recent_changes(
    ctx: &CrdtDocsContext,
    args: GetRecentChangesArgs,
) -> serde_json::Value {
    let since = match args.since_event_id {
        Some(s) => s,
        None => match ctx.session_id() {
            Some(sid) => ctx
                .backend()
                .cursor_for(sid, ctx.artifact_id())
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
            ctx.artifact_id(),
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
    super::build_synthetic_tool::<ListMyArtifactsArgs>(
        TOOL_LIST_MY_ARTIFACTS,
        "List CRDT workbooks accessible to the current agent session. \
         Returns id, name, created_at, last_accessed_at for each.",
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
    super::build_synthetic_tool::<CreateArtifactArgs>(
        TOOL_CREATE_ARTIFACT,
        "Create a new CRDT workbook for this session. Returns the new \
         artifact_id. To mutate it you'll need a follow-up turn whose \
         config pins this artifact_id (current limitation; multi-artifact \
         write access is subsystem F).",
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
            },
        );
        assert_eq!(v["cells"]["B2"], "a");
        assert_eq!(v["cells"]["C2"], "b");
        assert_eq!(v["cells"]["B3"], json!(1.0));
        assert_eq!(v["cells"]["C3"], json!(2.0));
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
            },
        );
        assert_eq!(v["cells"].as_object().unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn get_recent_changes_empty_then_populated() {
        let (ctx, tmp) = fresh_ctx().await;

        // Initially: no changes.
        let v = execute_get_recent_changes(
            &ctx,
            GetRecentChangesArgs {
                since_event_id: None,
                sheet_id: None,
                limit: None,
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
            },
        )
        .await;
        assert!(v["events"].as_array().unwrap().is_empty());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
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
}
