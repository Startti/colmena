//! napi mirror of the PyO3 `colmena.documents` submodule. Zero-deps raw
//! surface; the polars DataFrame ergonomics live in the `@colmena-ai/documents`
//! companion package, analogous to the Python `colmena_documents` pandas wrapper.

use crate::crdt_documents::{ArtifactId, CrdtDocumentsRuntime};
use napi::bindgen_prelude::*;
use napi_derive::napi;
use once_cell::sync::OnceCell;
use serde_json::Value;
use std::sync::Arc;

static RUNTIME: OnceCell<Arc<CrdtDocumentsRuntime>> = OnceCell::new();

async fn runtime() -> Result<Arc<CrdtDocumentsRuntime>> {
    if let Some(rt) = RUNTIME.get() {
        return Ok(rt.clone());
    }
    let storage_root = std::env::var("COLMENA_CRDT_DOCUMENTS_STORAGE_ROOT")
        .unwrap_or_else(|_| ".colmena/crdt_documents".to_string());
    let cfg = serde_json::json!({ "storage_root": storage_root });
    let built = CrdtDocumentsRuntime::from_config(&cfg)
        .await
        .map_err(|e| Error::new(Status::GenericFailure, e.to_string()))?;
    let arc = Arc::new(built);
    let _ = RUNTIME.set(arc.clone());
    Ok(arc)
}

fn parse_id(s: &str) -> Result<ArtifactId> {
    s.parse::<ArtifactId>()
        .map_err(|e| Error::new(Status::InvalidArg, format!("invalid artifact_id: {e}")))
}

/// List all sheets in a CRDT artifact. Returns an array of `{ sheetId, name }` objects.
#[napi]
#[allow(deprecated)]
pub async fn documents_list_sheets(artifact_id: String) -> Result<Value> {
    let rt = runtime().await?;
    let id = parse_id(&artifact_id)?;
    let entry = rt
        .registry
        .get(&id)
        .ok_or_else(|| Error::new(Status::GenericFailure, "artifact not found"))?;
    let proj = crate::crdt_documents::projection::project(&entry.doc);
    let mut out = Vec::new();
    for s in proj["sheets"].as_array().cloned().unwrap_or_default() {
        out.push(serde_json::json!({
            "sheetId": s["id"].as_str().unwrap_or(""),
            "name": s["name"].as_str().unwrap_or(""),
        }));
    }
    Ok(Value::Array(out))
}

/// Read all cells in a specific sheet. Returns a cell-addressed map
/// (`{ "A1": "header", "B1": "other", ... }`), mirroring the Python binding.
#[napi]
#[allow(deprecated)]
pub async fn documents_read_sheet(artifact_id: String, sheet_id: String) -> Result<Value> {
    let rt = runtime().await?;
    let id = parse_id(&artifact_id)?;
    let entry = rt
        .registry
        .get(&id)
        .ok_or_else(|| Error::new(Status::GenericFailure, "artifact not found"))?;
    let proj = crate::crdt_documents::projection::project(&entry.doc);
    let sheet = proj["sheets"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|s| s["id"].as_str() == Some(sheet_id.as_str()))
        .ok_or_else(|| Error::new(Status::GenericFailure, "sheet not found"))?;
    Ok(sheet["cells"].clone())
}

/// Add a new sheet to a CRDT artifact (creating the artifact if needed).
/// Returns the new sheet's UUID.
#[napi]
#[allow(deprecated)]
pub async fn documents_add_sheet(artifact_id: String, name: String) -> Result<String> {
    let rt = runtime().await?;
    let id = parse_id(&artifact_id)?;
    let entry = rt.registry.get_or_create(&id, "(from node)");
    let sheet_id = crate::crdt_documents::tool_executor::apply_add_sheet(&entry.doc, &name);
    entry.mark_dirty();
    let msg = format!("added sheet '{name}'");
    rt.tracker.record(&id, None, "node", &msg).await;
    Ok(sheet_id)
}

/// Write column headers + row data to a sheet (mode: "replace" | "append").
#[napi]
#[allow(deprecated)]
pub async fn documents_write_sheet(
    artifact_id: String,
    sheet_id: String,
    columns: Vec<String>,
    rows: Vec<Vec<Value>>,
    mode: Option<String>,
) -> Result<()> {
    let rt = runtime().await?;
    let id = parse_id(&artifact_id)?;
    let entry = rt
        .registry
        .get(&id)
        .ok_or_else(|| Error::new(Status::GenericFailure, "artifact not found"))?;
    let mode = mode.unwrap_or_else(|| "replace".to_string());
    if !matches!(mode.as_str(), "replace" | "append") {
        return Err(Error::new(
            Status::InvalidArg,
            "mode must be 'replace' or 'append'",
        ));
    }
    // Write column headers to row 1.
    for (col_idx, col_name) in columns.iter().enumerate() {
        let addr = format!("{}{}", col_letter(col_idx as u32), 1);
        let _ = crate::crdt_documents::tool_executor::apply_set_cell_in_proc(
            &entry.doc,
            &sheet_id,
            &addr,
            &Value::String(col_name.clone()),
        );
    }
    // Write data rows starting at row 2.
    for (row_idx, row) in rows.iter().enumerate() {
        for (col_idx, val) in row.iter().enumerate() {
            let addr = format!("{}{}", col_letter(col_idx as u32), row_idx + 2);
            let _ = crate::crdt_documents::tool_executor::apply_set_cell_in_proc(
                &entry.doc, &sheet_id, &addr, val,
            );
        }
    }
    entry.mark_dirty();
    let msg = format!("wrote {} rows to {sheet_id}", rows.len());
    rt.tracker.record(&id, None, "node", &msg).await;
    Ok(())
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
