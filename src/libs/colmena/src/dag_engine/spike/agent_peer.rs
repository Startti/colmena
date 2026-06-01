//! Agent-peer mutations. WS-client variant added in Task 9; in-proc
//! variant here (used by `POST /spike/agent-op`).

use anyhow::{anyhow, Result};
use serde_json::Value;
use yrs::{Any, Array, ArrayPrelim, Doc, Map, MapPrelim, Transact, WriteTxn};

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
    use crate::dag_engine::spike::yjs_protocol::{
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
    use crate::dag_engine::spike::projection;

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
