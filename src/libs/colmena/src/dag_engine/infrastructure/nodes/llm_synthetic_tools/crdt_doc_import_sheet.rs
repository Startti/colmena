//! LLM tool `crdt_doc_import_sheet` — clone a sheet from any artifact into
//! the current ctx artifact. The core of subsystem F (cross-sheet analysis).
//!
//! Snapshot only — later changes to the source do NOT propagate.
//! See: docs/superpowers/specs/2026-06-04-crdt-cross-sheet-analysis-design.md §3.2

use crate::crdt_documents::{
    df_writer::resolve_unique_sheet_name,
    tool_executor::{apply_add_sheet, apply_set_cell_in_proc},
    ArtifactId,
};
use crate::llm::domain::tools::ToolDefinition;
use schemars::JsonSchema;
use serde::Deserialize;
use yrs::{Array, Map, ReadTxn, Transact};

pub use super::crdt_doc_context::CrdtDocsContext;

pub const TOOL_IMPORT_SHEET: &str = "crdt_doc_import_sheet";

/// Cap matching `crdt_doc_run_python` (100 MB combined load). A single
/// import that would push the destination past this is rejected.
pub const MAX_IMPORT_BYTES: usize = 100 * 1024 * 1024;

/// Hard cap on sheets per artifact (defensive against agents that
/// import in a loop). 100 covers any plausible workflow; bump in v1.1
/// if real usage shows demand.
pub const MAX_SHEETS_PER_ARTIFACT: usize = 100;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ImportSheetArgs {
    /// Source artifact — where the sheet currently lives. Must be a different
    /// artifact than the current ctx (self-import is rejected).
    pub source_artifact_id: String,
    /// Sheet within the source artifact to clone.
    pub source_sheet_id: String,
    /// Optional new name for the cloned sheet in the destination. Default:
    /// `"<source_name> (from art_xxxx)"` where xxxx are the first 4 chars
    /// of the source ULID (after the "art_" prefix). Name collision auto-
    /// suffixes ` (2)`, ` (3)`, … (delegates to df_writer::resolve_unique_sheet_name).
    #[serde(default)]
    pub new_name: Option<String>,
}

pub fn tool_import_sheet() -> ToolDefinition {
    super::build_synthetic_tool::<ImportSheetArgs>(
        TOOL_IMPORT_SHEET,
        "Clone a sheet from another artifact into the CURRENT artifact \
         (the one your ctx is pinned to). The cloned sheet becomes a normal \
         sheet in the current workbook — you can read it, modify it, or pass \
         it together with original sheets to crdt_doc_run_python for \
         multi-sheet analysis (merge, compare, enrich, etc.). \n\
         IMPORTANT: snapshot only — later changes to the source do NOT \
         propagate. Re-import if you need fresh data. \n\
         Use AFTER crdt_doc_list_sheets_of (to discover the source_sheet_id). \
         See the crdt-doc-cross-sheet-analysis skill for the 6 canonical \
         pandas patterns this enables.",
    )
}

/// Extracted sheet payload (read from the source doc).
struct ExtractedSheet {
    name: String,
    /// `(A1-address, value, cell-type-string)` triples.
    cells: Vec<(String, serde_json::Value, String)>,
    bytes_estimate: usize,
}

/// Read the source sheet's name + cells into owned values. Returns
/// `Ok(None)` when the sheet id doesn't exist in the source doc.
fn extract_source_sheet(src_doc: &yrs::Doc, source_sheet_id: &str) -> Option<ExtractedSheet> {
    let txn = src_doc.transact();
    let workbook = txn.get_map("workbook")?;
    let sheets_arr = match workbook.get(&txn, "sheets") {
        Some(yrs::Out::YArray(a)) => a,
        _ => return None,
    };
    for i in 0..sheets_arr.len(&txn) {
        let sheet_map = match sheets_arr.get(&txn, i) {
            Some(yrs::Out::YMap(m)) => m,
            _ => continue,
        };
        let sid = match sheet_map.get(&txn, "id") {
            Some(yrs::Out::Any(yrs::Any::String(s))) => s.to_string(),
            _ => continue,
        };
        if sid != source_sheet_id {
            continue;
        }
        let name = match sheet_map.get(&txn, "name") {
            Some(yrs::Out::Any(yrs::Any::String(s))) => s.to_string(),
            _ => String::new(),
        };
        let mut cells = Vec::<(String, serde_json::Value, String)>::new();
        let mut bytes: usize = 0;
        if let Some(yrs::Out::YMap(cells_map)) = sheet_map.get(&txn, "cells") {
            for (addr, cell_out) in cells_map.iter(&txn) {
                if let yrs::Out::YMap(cell_map) = cell_out {
                    let v_json = match cell_map.get(&txn, "v") {
                        Some(yrs::Out::Any(any)) => any_to_json(&any),
                        _ => serde_json::Value::Null,
                    };
                    let t = match cell_map.get(&txn, "t") {
                        Some(yrs::Out::Any(yrs::Any::String(s))) => s.to_string(),
                        _ => "s".to_string(),
                    };
                    bytes += addr.len() + v_json.to_string().len() + t.len() + 8;
                    cells.push((addr.to_string(), v_json, t));
                }
            }
        }
        return Some(ExtractedSheet {
            name,
            cells,
            bytes_estimate: bytes,
        });
    }
    None
}

/// Count of sheets in `doc`'s workbook (defensive — returns 0 if the
/// workbook hasn't been materialized yet).
fn count_sheets(doc: &yrs::Doc) -> usize {
    let txn = doc.transact();
    match txn
        .get_map("workbook")
        .and_then(|wb| wb.get(&txn, "sheets"))
    {
        Some(yrs::Out::YArray(arr)) => arr.len(&txn) as usize,
        _ => 0,
    }
}

pub async fn execute_import_sheet(
    ctx: &CrdtDocsContext,
    args: ImportSheetArgs,
) -> serde_json::Value {
    // 1. Parse + validate source ID.
    let src_aid: ArtifactId = match args.source_artifact_id.parse() {
        Ok(a) => a,
        Err(_) => {
            return serde_json::json!({
                "error": "invalid_artifact_id",
                "value": args.source_artifact_id,
            })
        }
    };

    // 2. Forbid self-import (catches loops + makes intent explicit).
    if &src_aid == ctx.artifact_id() {
        return serde_json::json!({
            "error": "self_import_forbidden",
            "artifact_id": src_aid.to_string(),
        });
    }

    // 3. Resolve source artifact via the registry. Cross-artifact reads
    //    only work in Local mode (mirrors execute_list_sheets_of in F-T1).
    let src_entry = match ctx {
        CrdtDocsContext::Local { runtime, .. } => match runtime.registry.get(&src_aid) {
            Some(e) => e,
            None => {
                return serde_json::json!({
                    "error": "source_artifact_not_found",
                    "artifact_id": src_aid.to_string(),
                })
            }
        },
        CrdtDocsContext::WsPeer { .. } => {
            return serde_json::json!({
                "error": "unsupported_in_ws_peer_mode",
                "tool": TOOL_IMPORT_SHEET,
            })
        }
    };

    // 4. Extract the source sheet's cells + name.
    let extracted = match extract_source_sheet(&src_entry.doc, &args.source_sheet_id) {
        Some(e) => e,
        None => {
            return serde_json::json!({
                "error": "source_sheet_not_found",
                "artifact_id": src_aid.to_string(),
                "sheet_id": args.source_sheet_id,
            })
        }
    };

    // 5. Enforce size cap.
    if extracted.bytes_estimate > MAX_IMPORT_BYTES {
        return serde_json::json!({
            "error": "load_size_exceeded",
            "actual_bytes": extracted.bytes_estimate,
            "limit_bytes": MAX_IMPORT_BYTES,
        });
    }

    // 6. Resolve destination doc.
    let Some(dest_doc) = ctx.doc() else {
        return serde_json::json!({ "error": "artifact_not_found" });
    };

    // 7. Enforce max-sheets-per-artifact on destination.
    let n_existing = count_sheets(&dest_doc);
    if n_existing >= MAX_SHEETS_PER_ARTIFACT {
        return serde_json::json!({
            "error": "max_sheets_in_artifact_exceeded",
            "current": n_existing,
            "limit": MAX_SHEETS_PER_ARTIFACT,
        });
    }

    // 8. Compute the destination sheet name (collision-aware).
    let src_aid_str = src_aid.to_string();
    let proposed_name = args
        .new_name
        .unwrap_or_else(|| format!("{} (from art_{})", extracted.name, &src_aid_str[4..8]));
    let final_name = resolve_unique_sheet_name(&dest_doc, &proposed_name);

    // 9. Write into destination — one sheet creation + per-cell writes.
    //    Reuses the same helpers as every other write path in the codebase.
    let new_sheet_id = apply_add_sheet(&dest_doc, &final_name);
    let mut max_row = 0u32;
    let mut max_col = 0u32;
    let mut has_any = false;
    for (addr, v_json, _t) in &extracted.cells {
        // We deliberately ignore the source `t` annotation: the writer
        // re-derives it from the JSON value via `apply_set_cell_in_proc`,
        // which matches the policy applied to any other tool-driven write.
        apply_set_cell_in_proc(&dest_doc, &new_sheet_id, addr, v_json);
        has_any = true;
        if let Some((r, c)) = super::crdt_doc_tools::parse_a1_to_rc(addr) {
            if r > max_row {
                max_row = r;
            }
            if c > max_col {
                max_col = c;
            }
        }
    }
    let n_rows = if has_any { max_row + 1 } else { 0 };
    let n_cols = if has_any { max_col + 1 } else { 0 };

    // 10. Side-effects: dirty flag + audit event.
    ctx.mark_dirty();
    let origin = ctx
        .session_id()
        .map(|s| format!("agent:{s}"))
        .unwrap_or_else(|| "agent:llm".to_string());
    let event_id = ctx
        .backend()
        .record_event(crate::crdt_documents::change_tracker_store::NewEvent {
            artifact_id: ctx.artifact_id().clone(),
            sheet_id: Some(new_sheet_id.clone()),
            origin,
            summary: format!(
                "imported sheet '{}' ({} rows × {} cols) from artifact art_{}",
                extracted.name,
                n_rows,
                n_cols,
                &src_aid_str[4..8]
            ),
        })
        .await
        .unwrap_or(0);
    ctx.record_event_id(event_id);

    serde_json::json!({
        "sheet_id": new_sheet_id,
        "name": final_name,
        "n_rows": n_rows,
        "n_cols": n_cols,
        "source": {
            "artifact_id": src_aid.to_string(),
            "sheet_id": args.source_sheet_id,
            "name": extracted.name,
        },
    })
}

pub async fn dispatch_crdt_doc_import_sheet(
    ctx: &CrdtDocsContext,
    args: serde_json::Value,
) -> serde_json::Value {
    match serde_json::from_value::<ImportSheetArgs>(args) {
        Ok(a) => execute_import_sheet(ctx, a).await,
        Err(e) => serde_json::json!({ "error": format!("invalid_args: {e}") }),
    }
}

// ── helpers ──────────────────────────────────────────────────────────────

fn any_to_json(a: &yrs::Any) -> serde_json::Value {
    match a {
        yrs::Any::Null | yrs::Any::Undefined => serde_json::Value::Null,
        yrs::Any::Bool(b) => serde_json::Value::Bool(*b),
        yrs::Any::Number(n) => serde_json::json!(n),
        yrs::Any::BigInt(i) => serde_json::json!(i),
        yrs::Any::String(s) => serde_json::Value::String(s.to_string()),
        _ => serde_json::Value::Null,
    }
}
