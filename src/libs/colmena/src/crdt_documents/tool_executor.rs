//! Tool-executor mutations: in-proc helpers that mutate a `yrs::Doc` directly
//! (used by `POST /spike/agent-op` and the multi-sheet LLM tool layer).
//! WS-client variant (`apply_set_cell_via_ws`) stays here for symmetry.

use anyhow::{anyhow, Result};
use serde_json::Value;
use yrs::{Any, Array, ArrayPrelim, Doc, Map, MapPrelim, ReadTxn, Transact, WriteTxn};

/// Sanity-check route: mutates the doc directly without going through WS.
///
/// Creates the workbook/sheets/cells structure on demand. Idempotent on
/// the sheet entry (looks up by `sheet_id`; creates if missing).
pub fn apply_set_cell_in_proc(doc: &Doc, sheet_id: &str, addr: &str, value: &Value) {
    let mut txn = doc.transact_mut();
    let workbook = txn.get_or_insert_map("workbook");
    let sheets_arr = match workbook.get(&txn, "sheets") {
        Some(yrs::Out::YArray(a)) => a,
        _ => workbook.insert(&mut txn, "sheets", ArrayPrelim::default()),
    };

    // Find sheet by id, or push a new one.
    let mut sheet_idx: Option<u32> = None;
    for i in 0..sheets_arr.len(&txn) {
        if let Some(yrs::Out::YMap(m)) = sheets_arr.get(&txn, i) {
            if let Some(yrs::Out::Any(yrs::Any::String(s))) = m.get(&txn, "id") {
                if s.as_ref() == sheet_id {
                    sheet_idx = Some(i);
                    break;
                }
            }
        }
    }
    let sheet = match sheet_idx {
        Some(i) => match sheets_arr.get(&txn, i).unwrap() {
            yrs::Out::YMap(m) => m,
            _ => unreachable!(),
        },
        None => {
            let new_sheet = sheets_arr.push_back(&mut txn, MapPrelim::default());
            new_sheet.insert(&mut txn, "id", sheet_id);
            new_sheet.insert(&mut txn, "name", sheet_id);
            new_sheet.insert(&mut txn, "cells", MapPrelim::default());
            new_sheet
        }
    };

    let cells = match sheet.get(&txn, "cells") {
        Some(yrs::Out::YMap(m)) => m,
        _ => sheet.insert(&mut txn, "cells", MapPrelim::default()),
    };

    let cell = cells.insert(&mut txn, addr, MapPrelim::default());
    let (any, type_tag) = json_to_any(value);
    cell.insert(&mut txn, "v", any);
    cell.insert(&mut txn, "t", type_tag);
}

/// Connects to `url` as a Yjs WS client, applies `set_cell(sheet_id, addr,
/// value)` to a local `Doc`, and ships the resulting update diff back to the
/// server so all peers converge.
///
/// Full Yjs sync v1 protocol (both sides exchange step1+step2 for proper merge):
///   1. Server → client: `[MSG_SYNC][STEP_1][server_sv]`
///   2. Client → server: `[MSG_SYNC][STEP_1][client_empty_sv]`
///   3. Server → client: `[MSG_SYNC][STEP_2][server_state_for_client]`
///   4. Client imports server state, then applies its mutation.
///   5. Client → server: `[MSG_SYNC][STEP_2][diff_against_server_sv]`
///   6. Client closes after a brief flush pause.
///
/// Steps 2–4 are critical: without importing the server's current CRDT state
/// first, the client's fresh `Doc` creates new conflicting CRDT object IDs for
/// the same top-level structure (workbook, sheets, cells maps). The CRDT merge
/// then resolves the key conflict by last-write-wins, silently discarding the
/// other agent's data.
pub async fn apply_set_cell_via_ws(
    url: &str,
    sheet_id: &str,
    addr: &str,
    value: &Value,
) -> Result<()> {
    use crate::crdt_documents::yjs_protocol::{
        decode_sync_step1_sv, decode_sync_step2_update, encode_sync_step1, encode_sync_step2,
    };
    use futures::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message as TMsg;
    use yrs::updates::decoder::Decode;
    use yrs::{ReadTxn, StateVector};

    let (mut ws, _resp) = tokio_tungstenite::connect_async(url)
        .await
        .map_err(|e| anyhow!("connect: {e}"))?;

    let local = Doc::new();

    // Step 1: Wait for the server's sync_step1 and capture the server SV.
    let server_sv: StateVector = loop {
        match ws.next().await {
            Some(Ok(TMsg::Binary(bytes))) => {
                if let Some(sv_bytes) = decode_sync_step1_sv(&bytes) {
                    break StateVector::decode_v1(&sv_bytes)
                        .map_err(|e| anyhow!("decode SV: {e:?}"))?;
                }
                // Not a step1 frame (e.g. awareness) — keep reading.
                continue;
            }
            Some(Ok(_)) => continue, // text / ping / pong frames
            Some(Err(e)) => return Err(anyhow!("ws recv: {e}")),
            None => return Err(anyhow!("ws closed before sync_step1")),
        }
    };

    // Step 2: Send our own sync_step1 so the server knows our (empty) state and
    // sends us its full state via sync_step2.
    let our_sv = local.transact().state_vector();
    ws.send(TMsg::Binary(encode_sync_step1(&our_sv).into()))
        .await
        .map_err(|e| anyhow!("send step1: {e}"))?;

    // Step 3: Wait for the server's sync_step2 (the server's current state).
    // We may receive other frames (awareness, update broadcasts) — skip them.
    // When the server's doc is empty, it will send an empty update; we still
    // need to receive it to complete the handshake.
    let server_state_bytes: Vec<u8> = loop {
        match ws.next().await {
            Some(Ok(TMsg::Binary(ref bytes))) => {
                if let Some(state) = decode_sync_step2_update(bytes) {
                    break state;
                }
                continue;
            }
            Some(Ok(_)) => continue,
            Some(Err(e)) => return Err(anyhow!("ws recv step2: {e}")),
            None => return Err(anyhow!("ws closed before sync_step2")),
        }
    };

    // Step 4: Import the server's state into our local doc, then apply mutation.
    // This ensures our doc shares the same CRDT object IDs as the server.
    {
        let update = yrs::Update::decode_v1(&server_state_bytes)
            .map_err(|e| anyhow!("decode server state: {e:?}"))?;
        local
            .transact_mut()
            .apply_update(update)
            .map_err(|e| anyhow!("apply server state: {e:?}"))?;
    }
    apply_set_cell_in_proc(&local, sheet_id, addr, value);

    // Step 5: Compute diff of our new mutation against the server's known SV,
    // then send it.
    let diff = local.transact().encode_diff_v1(&server_sv);
    let frame = encode_sync_step2(&diff);
    ws.send(TMsg::Binary(frame.into()))
        .await
        .map_err(|e| anyhow!("send step2: {e}"))?;

    // Give the server a moment to apply the update, then close.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    ws.send(TMsg::Close(None)).await.ok();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crdt_documents::projection;

    #[test]
    fn set_cell_then_project_reflects_value() {
        let doc = Doc::new();
        apply_set_cell_in_proc(&doc, "s1", "A1", &Value::String("Hola".into()));
        let v = projection::project(&doc);
        eprintln!("projection: {v}");
        assert_eq!(
            v["sheets"][0]["cells"]["A1"],
            serde_json::Value::String("Hola".into())
        );
    }
}

fn json_to_any(v: &Value) -> (Any, &'static str) {
    match v {
        Value::String(s) => (Any::String(s.clone().into()), "s"),
        Value::Number(n) => (
            n.as_f64().map(Any::Number).unwrap_or(Any::Null),
            "n",
        ),
        Value::Bool(b) => (Any::Bool(*b), "b"),
        _ => (Any::Null, "s"),
    }
}

// ─── multi-sheet helpers ─────────────────────────────────────────────────────

/// Append a new sheet to the workbook. Returns the generated sheet id
/// (format: `sh_<ULID>`).
pub fn apply_add_sheet(doc: &Doc, name: &str) -> String {
    let mut txn = doc.transact_mut();
    let wb = txn.get_or_insert_map("workbook");
    let sheets = match wb.get(&txn, "sheets") {
        Some(yrs::Out::YArray(a)) => a,
        _ => wb.insert(&mut txn, "sheets", ArrayPrelim::default()),
    };
    let sheet_id = format!("sh_{}", ulid::Ulid::new());
    let sheet = sheets.push_back(&mut txn, MapPrelim::default());
    sheet.insert(&mut txn, "id", sheet_id.as_str());
    sheet.insert(&mut txn, "name", name);
    sheet.insert(&mut txn, "cells", MapPrelim::default());
    sheet_id
}

/// Rename a sheet by id. Returns `false` if no sheet with that id is found.
pub fn apply_rename_sheet(doc: &Doc, sheet_id: &str, new_name: &str) -> bool {
    let mut txn = doc.transact_mut();
    // Find the index first (read phase), then mutate.
    let idx = find_sheet_index_in_txn(&txn, sheet_id);
    let Some(i) = idx else { return false; };
    let wb = txn.get_or_insert_map("workbook");
    let sheets = match wb.get(&txn, "sheets") {
        Some(yrs::Out::YArray(a)) => a,
        _ => return false,
    };
    if let Some(yrs::Out::YMap(m)) = sheets.get(&txn, i) {
        m.insert(&mut txn, "name", new_name);
        true
    } else {
        false
    }
}

/// Delete a sheet by id. Returns `false` if not found.
pub fn apply_delete_sheet(doc: &Doc, sheet_id: &str) -> bool {
    let mut txn = doc.transact_mut();
    let idx = find_sheet_index_in_txn(&txn, sheet_id);
    let Some(i) = idx else { return false; };
    let wb = txn.get_or_insert_map("workbook");
    let sheets = match wb.get(&txn, "sheets") {
        Some(yrs::Out::YArray(a)) => a,
        _ => return false,
    };
    sheets.remove(&mut txn, i);
    true
}

/// Reorder sheets. `new_order` must be a permutation of the existing sheet
/// ids (same set, different order). Returns `false` on any mismatch.
///
/// Implementation: snapshot-and-restore, because `yrs::Array` has no
/// in-place move operation.
pub fn apply_reorder_sheets(doc: &Doc, new_order: &[String]) -> bool {
    // Phase 1: snapshot (read-only txn).
    let snapshots: Vec<serde_json::Value> = {
        let txn = doc.transact();
        let wb = match txn.get_map("workbook") {
            Some(m) => m,
            None => return false,
        };
        let sheets = match wb.get(&txn, "sheets") {
            Some(yrs::Out::YArray(a)) => a,
            _ => return false,
        };
        let len = sheets.len(&txn);
        if len as usize != new_order.len() {
            return false;
        }
        (0..len)
            .filter_map(|i| match sheets.get(&txn, i) {
                Some(yrs::Out::YMap(m)) => Some(snapshot_sheet_inline(&txn, &m)),
                _ => None,
            })
            .collect()
    };

    // Validate that new_order is a permutation of existing ids.
    let mut existing_ids: Vec<String> = snapshots
        .iter()
        .filter_map(|s| s["id"].as_str().map(str::to_string))
        .collect();
    if existing_ids.len() != new_order.len() {
        return false;
    }
    let mut requested = new_order.to_vec();
    existing_ids.sort();
    requested.sort();
    if existing_ids != requested {
        return false;
    }

    // Phase 2: clear and re-insert in new order (write txn).
    let mut txn = doc.transact_mut();
    let wb = txn.get_or_insert_map("workbook");
    let sheets = match wb.get(&txn, "sheets") {
        Some(yrs::Out::YArray(a)) => a,
        _ => return false,
    };
    let current_len = sheets.len(&txn);
    for i in (0..current_len).rev() {
        sheets.remove(&mut txn, i);
    }
    for desired_id in new_order {
        let snap = match snapshots.iter().find(|s| s["id"].as_str() == Some(desired_id)) {
            Some(s) => s,
            None => return false,
        };
        let new_sheet = sheets.push_back(&mut txn, MapPrelim::default());
        new_sheet.insert(&mut txn, "id", desired_id.as_str());
        let name = snap["name"].as_str().unwrap_or("");
        new_sheet.insert(&mut txn, "name", name);
        let cells_map = new_sheet.insert(&mut txn, "cells", MapPrelim::default());
        if let Some(obj) = snap["cells"].as_object() {
            for (addr, v) in obj {
                let cell = cells_map.insert(&mut txn, addr.as_str(), MapPrelim::default());
                let (any, t) = json_to_any(v);
                cell.insert(&mut txn, "v", any);
                cell.insert(&mut txn, "t", t);
            }
        }
    }
    true
}

// ─── internal utilities ──────────────────────────────────────────────────────

/// Find the array index of the sheet with the given id using a read txn.
/// Returns `None` if the sheet is not found or the workbook has no sheets.
fn find_sheet_index_in_txn<T: yrs::ReadTxn>(txn: &T, sheet_id: &str) -> Option<u32> {
    let wb = txn.get_map("workbook")?;
    let sheets = match wb.get(txn, "sheets") {
        Some(yrs::Out::YArray(a)) => a,
        _ => return None,
    };
    for i in 0..sheets.len(txn) {
        if let Some(yrs::Out::YMap(m)) = sheets.get(txn, i) {
            if let Some(yrs::Out::Any(yrs::Any::String(s))) = m.get(txn, "id") {
                if s.as_ref() == sheet_id {
                    return Some(i);
                }
            }
        }
    }
    None
}

/// Inline snapshot of a single sheet map into JSON (used by `apply_reorder_sheets`).
fn snapshot_sheet_inline<T: yrs::ReadTxn>(txn: &T, sheet_map: &yrs::MapRef) -> serde_json::Value {
    let id = sheet_map
        .get(txn, "id")
        .and_then(|v| match v {
            yrs::Out::Any(yrs::Any::String(s)) => Some(s.to_string()),
            _ => None,
        })
        .unwrap_or_default();
    let name = sheet_map
        .get(txn, "name")
        .and_then(|v| match v {
            yrs::Out::Any(yrs::Any::String(s)) => Some(s.to_string()),
            _ => None,
        })
        .unwrap_or_default();
    let mut cells_out = serde_json::Map::new();
    if let Some(yrs::Out::YMap(cells_map)) = sheet_map.get(txn, "cells") {
        for (addr, cell_val) in cells_map.iter(txn) {
            if let yrs::Out::YMap(cell_map) = cell_val {
                if let Some(yrs::Out::Any(any)) = cell_map.get(txn, "v") {
                    let v = match any {
                        yrs::Any::Null | yrs::Any::Undefined => serde_json::Value::Null,
                        yrs::Any::Bool(b) => serde_json::Value::Bool(b),
                        yrs::Any::Number(n) => serde_json::json!(n),
                        yrs::Any::BigInt(n) => serde_json::json!(n),
                        yrs::Any::String(s) => serde_json::Value::String(s.to_string()),
                        _ => serde_json::Value::Null,
                    };
                    cells_out.insert(addr.to_string(), v);
                }
            }
        }
    }
    serde_json::json!({ "id": id, "name": name, "cells": cells_out })
}

#[cfg(test)]
mod multi_sheet_tests {
    use super::*;
    use crate::crdt_documents::projection::project;
    use yrs::Doc;

    #[test]
    fn add_sheet_appends_and_returns_unique_id() {
        let doc = Doc::new();
        let id1 = apply_add_sheet(&doc, "Sales");
        let id2 = apply_add_sheet(&doc, "Summary");
        assert_ne!(id1, id2);
        let v = project(&doc);
        assert_eq!(v["sheets"].as_array().unwrap().len(), 2);
        assert_eq!(v["sheets"][0]["name"], "Sales");
        assert_eq!(v["sheets"][1]["name"], "Summary");
    }

    #[test]
    fn rename_sheet_changes_name_only() {
        let doc = Doc::new();
        let id = apply_add_sheet(&doc, "Old");
        apply_set_cell_in_proc(&doc, &id, "A1", &serde_json::json!("kept"));
        assert!(apply_rename_sheet(&doc, &id, "New"));
        let v = project(&doc);
        assert_eq!(v["sheets"][0]["name"], "New");
        assert_eq!(v["sheets"][0]["cells"]["A1"], "kept");
    }

    #[test]
    fn delete_sheet_removes_it() {
        let doc = Doc::new();
        let a = apply_add_sheet(&doc, "A");
        let b = apply_add_sheet(&doc, "B");
        assert!(apply_delete_sheet(&doc, &a));
        let v = project(&doc);
        assert_eq!(v["sheets"].as_array().unwrap().len(), 1);
        assert_eq!(v["sheets"][0]["id"], b);
    }

    #[test]
    fn reorder_sheets_swaps() {
        let doc = Doc::new();
        let a = apply_add_sheet(&doc, "A");
        let b = apply_add_sheet(&doc, "B");
        assert!(apply_reorder_sheets(&doc, &[b.clone(), a.clone()]));
        let v = project(&doc);
        assert_eq!(v["sheets"][0]["id"], b);
        assert_eq!(v["sheets"][1]["id"], a);
    }
}
