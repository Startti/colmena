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
/// Protocol (Yjs sync v1):
///   1. Server → client: `[MSG_SYNC][STEP_1][state_vector]`
///   2. Client applies its mutation locally.
///   3. Client → server: `[MSG_SYNC][STEP_2][diff_against_server_sv]`
///   4. Client closes after a brief flush pause.
pub async fn apply_set_cell_via_ws(
    url: &str,
    sheet_id: &str,
    addr: &str,
    value: &Value,
) -> Result<()> {
    use crate::dag_engine::spike::yjs_protocol::{decode_sync_step1_sv, encode_sync_step2};
    use futures::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message as TMsg;
    use yrs::updates::decoder::Decode;
    use yrs::{ReadTxn, StateVector};

    let (mut ws, _resp) = tokio_tungstenite::connect_async(url)
        .await
        .map_err(|e| anyhow!("connect: {e}"))?;

    let local = Doc::new();

    // Wait for the server's sync_step1 message and extract the state vector.
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

    // Apply the mutation to our local doc.
    apply_set_cell_in_proc(&local, sheet_id, addr, value);

    // Compute the diff between our state and the server's known state.
    let diff = local.transact().encode_diff_v1(&server_sv);

    // Send sync_step2 carrying that diff.
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
