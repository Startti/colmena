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
    ctx.tracker()
        .record(
            ctx.artifact_id(),
            None,
            "agent:llm",
            &format!("set {}!{} = {}", args.sheet_id, args.addr, args.value),
        )
        .await;
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
    ctx.tracker()
        .record(
            ctx.artifact_id(),
            None,
            "agent:llm",
            &format!(
                "wrote {cells_written} cells starting at {}!{}",
                args.sheet_id, args.start_addr
            ),
        )
        .await;
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
    ctx.tracker()
        .record(
            ctx.artifact_id(),
            None,
            "agent:llm",
            &format!("added sheet '{}' (id={sheet_id})", args.name),
        )
        .await;
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
    /// Optional cursor — return only events after this id.
    #[serde(default)]
    pub since_event_id: Option<u64>,
}

/// Build the [`ToolDefinition`] for `crdt_doc_get_recent_changes`.
pub fn tool_get_recent_changes() -> ToolDefinition {
    super::build_synthetic_tool::<GetRecentChangesArgs>(
        TOOL_GET_RECENT_CHANGES,
        "Get a narration of recent peer changes to the document. \
         Optionally filter by since_event_id. Returns \
         { current_event_id, narration } where narration is a human-readable \
         summary of all events since the cursor.",
    )
}

/// Execute `get_recent_changes` against the runtime.
pub async fn execute_get_recent_changes(
    ctx: &CrdtDocsContext,
    args: GetRecentChangesArgs,
) -> serde_json::Value {
    let events = ctx
        .tracker()
        .since(ctx.artifact_id(), args.since_event_id, None, None, 100)
        .await;
    let current_event_id = events.iter().map(|e| e.event_id).max();
    let narration = if events.is_empty() {
        "No changes since last check.".to_string()
    } else {
        events
            .iter()
            .map(|e| format!("- [{}] ({}): {}", e.event_id, e.origin, e.summary))
            .collect::<Vec<_>>()
            .join("\n")
    };
    serde_json::json!({
        "current_event_id": current_event_id,
        "narration": narration,
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
        let ctx = CrdtDocsContext::new_local(rt, id);
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
        let ctx = CrdtDocsContext::new_local(rt, unknown);
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
        (CrdtDocsContext::new_local(rt, id), tmp)
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
        let v = execute_read(&ctx, ReadArgs { sheet_id, range: None });
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
                values_2d: vec![
                    vec![json!("a"), json!("b")],
                    vec![json!(1), json!(2)],
                ],
            },
        )
        .await;
        let v = execute_read(&ctx, ReadArgs { sheet_id, range: None });
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
        let v =
            execute_get_recent_changes(&ctx, GetRecentChangesArgs { since_event_id: None }).await;
        assert_eq!(v["narration"], "No changes since last check.");

        // Record via a mutation.
        let s = execute_add_sheet(&ctx, AddSheetArgs { name: "Sales".into() }).await;
        let _sheet_id = s["sheet_id"].as_str().unwrap();

        let v =
            execute_get_recent_changes(&ctx, GetRecentChangesArgs { since_event_id: None }).await;
        let n = v["narration"].as_str().unwrap();
        assert!(n.contains("added sheet"), "got: {n}");
        assert!(n.contains("Sales"), "got: {n}");
        let current = v["current_event_id"].as_u64().unwrap();

        // since_event_id filters: passing current returns empty narration.
        let v = execute_get_recent_changes(
            &ctx,
            GetRecentChangesArgs {
                since_event_id: Some(current),
            },
        )
        .await;
        assert_eq!(v["narration"], "No changes since last check.");

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
