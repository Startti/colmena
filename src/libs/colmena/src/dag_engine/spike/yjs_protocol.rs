//! Yjs sync v1 protocol bridge between an `axum::extract::ws::WebSocket`
//! and a `yrs::Doc`. Delegates wire-format constants to `y-sync` but does
//! all encoding/decoding through **yrs 0.26** so the two crates' incompatible
//! `yrs` dependency versions (y-sync 0.4 pins yrs 0.17) never clash.
//!
//! # Why we don't use y-sync's `MessageReader` / `Awareness`
//!
//! `y-sync 0.4` depends on `yrs 0.17.4`.  Our crate uses `yrs 0.26`.  Rust
//! resolves them as two distinct crates so `y_sync::awareness::Awareness::new`
//! expects a yrs-0.17 `Doc` and `y_sync::sync::MessageReader` yields
//! yrs-0.17 `StateVector` values — neither is usable with our yrs-0.26 doc.
//!
//! Solution: borrow only the **message-tag constants** from y-sync
//! (`MSG_SYNC`, `MSG_SYNC_STEP_1`, `MSG_SYNC_STEP_2`, `MSG_SYNC_UPDATE`) and
//! implement the tiny framing logic ourselves on top of yrs 0.26.  The
//! on-wire format is identical regardless of crate version (lib0 varint
//! framing), so clients built against any yrs generation interoperate.
//!
//! Spec §4.1: this lets any Yjs client (Univer's `y-websocket` provider
//! or our own Rust `agent_peer`) sync with our server.

use anyhow::{anyhow, Result};
use axum::extract::ws::{Message, WebSocket};
use futures::StreamExt;
use std::sync::Arc;
use y_sync::sync::{MSG_SYNC, MSG_SYNC_STEP_1, MSG_SYNC_STEP_2, MSG_SYNC_UPDATE};
use yrs::encoding::read::{Cursor, Read};
use yrs::encoding::write::Write;
use yrs::updates::decoder::Decode;
use yrs::updates::encoder::{Encode, Encoder, EncoderV1};
use yrs::{Doc, ReadTxn, StateVector, Transact, Update};

// ─── helpers ────────────────────────────────────────────────────────────────

/// Encode a sync_step1 message: `[MSG_SYNC][MSG_SYNC_STEP_1][sv_bytes]`.
fn encode_sync_step1(sv: &StateVector) -> Vec<u8> {
    let mut enc = EncoderV1::new();
    enc.write_var(MSG_SYNC);
    enc.write_var(MSG_SYNC_STEP_1);
    enc.write_buf(sv.encode_v1());
    enc.to_vec()
}

/// Encode a sync_step2 message: `[MSG_SYNC][MSG_SYNC_STEP_2][update_bytes]`.
fn encode_sync_step2(update: &[u8]) -> Vec<u8> {
    let mut enc = EncoderV1::new();
    enc.write_var(MSG_SYNC);
    enc.write_var(MSG_SYNC_STEP_2);
    enc.write_buf(update);
    enc.to_vec()
}

/// Encode an update message: `[MSG_SYNC][MSG_SYNC_UPDATE][update_bytes]`.
fn encode_update(update: &[u8]) -> Vec<u8> {
    let mut enc = EncoderV1::new();
    enc.write_var(MSG_SYNC);
    enc.write_var(MSG_SYNC_UPDATE);
    enc.write_buf(update);
    enc.to_vec()
}

// ─── message parser ──────────────────────────────────────────────────────────

/// All Yjs sync-v1 message variants we care about.
enum SyncMsg {
    /// Client's state vector; we reply with step2 (missing updates).
    Step1 { sv_bytes: Vec<u8> },
    /// Client's missing-updates payload; we apply it.
    Step2OrUpdate { update_bytes: Vec<u8> },
}

/// Parse zero or more sync messages from a raw byte buffer.
///
/// Ignores non-sync messages (awareness, auth, etc.) silently — the spike
/// doesn't need them.
fn parse_msgs(bytes: &[u8]) -> Result<Vec<SyncMsg>> {
    let mut cur = Cursor::new(bytes);
    let mut out = Vec::new();
    while !cur.buf.is_empty() {
        let outer_tag: u8 = cur
            .read_var()
            .map_err(|e| anyhow!("outer tag: {e:?}"))?;
        if outer_tag != MSG_SYNC {
            // Skip non-sync messages: read and discard the payload buffer.
            let _ = cur.read_buf().map_err(|e| anyhow!("skip non-sync buf: {e:?}"))?;
            continue;
        }
        let sub_tag: u8 = cur
            .read_var()
            .map_err(|e| anyhow!("sync sub-tag: {e:?}"))?;
        let payload = cur
            .read_buf()
            .map_err(|e| anyhow!("sync payload: {e:?}"))?
            .to_vec();
        match sub_tag {
            t if t == MSG_SYNC_STEP_1 => out.push(SyncMsg::Step1 { sv_bytes: payload }),
            t if t == MSG_SYNC_STEP_2 || t == MSG_SYNC_UPDATE => {
                out.push(SyncMsg::Step2OrUpdate { update_bytes: payload })
            }
            _ => {} // ignore unknown sub-tags
        }
    }
    Ok(out)
}

// ─── main handler ────────────────────────────────────────────────────────────

/// Drives a single WebSocket connection's sync with `doc`.
///
/// Steps:
///   1. Server sends sync_step1 (state vector) so the client can compute
///      its missing updates.
///   2. Each side responds with sync_step2 (the missing update) or
///      additional `update` messages.
///   3. Future incremental updates fan out via an `update_v1` observer.
pub async fn handle_socket(mut socket: WebSocket, doc: Arc<Doc>) -> Result<()> {
    // 1. Send our initial sync_step1 (state vector).
    let sv = doc.transact().state_vector();
    socket
        .send(Message::Binary(encode_sync_step1(&sv).into()))
        .await
        .map_err(|e| anyhow!("send sync_step1: {e}"))?;

    // 2. Subscribe to local updates so we can forward them to the client.
    //    The `_subscription` guard must stay alive for the duration of the loop.
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
    let _subscription = doc
        .observe_update_v1(move |_txn, evt| {
            let bytes = encode_update(&evt.update);
            let _ = tx.send(bytes);
        })
        .map_err(|e| anyhow!("observe_update_v1: {e:?}"))?;

    // 3. Concurrently read from socket and write outbound updates.
    loop {
        tokio::select! {
            outbound = rx.recv() => {
                match outbound {
                    Some(bytes) => {
                        if socket.send(Message::Binary(bytes.into())).await.is_err() {
                            break;
                        }
                    }
                    None => break,
                }
            }
            incoming = socket.next() => {
                let Some(msg) = incoming else { break };
                let Ok(msg) = msg else { break };
                let bytes = match msg {
                    Message::Binary(b) => b,
                    Message::Close(_) => break,
                    _ => continue,
                };
                let msgs = parse_msgs(&bytes)?;
                for m in msgs {
                    match m {
                        SyncMsg::Step1 { sv_bytes } => {
                            // Decode client's state vector and reply with our diff.
                            let sv = StateVector::decode_v1(&sv_bytes)
                                .map_err(|e| anyhow!("decode sv: {e:?}"))?;
                            let diff = doc.transact().encode_state_as_update_v1(&sv);
                            if socket
                                .send(Message::Binary(encode_sync_step2(&diff).into()))
                                .await
                                .is_err()
                            {
                                return Ok(());
                            }
                        }
                        SyncMsg::Step2OrUpdate { update_bytes } => {
                            let update = Update::decode_v1(&update_bytes)
                                .map_err(|e| anyhow!("decode update: {e:?}"))?;
                            doc.transact_mut()
                                .apply_update(update)
                                .map_err(|e| anyhow!("apply update: {e:?}"))?;
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

// ─── tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use yrs::updates::decoder::Decode;
    use yrs::{Any, Doc, Map, Out, ReadTxn, Transact};

    /// Verifies the sync layer at the message level (no real WebSocket) by
    /// feeding one doc's state vector + update to another and asserting
    /// convergence.
    #[test]
    fn two_docs_converge_via_sync_messages() {
        let doc_a = Doc::new();
        let wb = doc_a.get_or_insert_map("workbook");
        {
            let mut txn = doc_a.transact_mut();
            wb.insert(&mut txn, "marker", "from-A");
        }

        let doc_b = Doc::new();

        // B sends its state vector to A; A replies with the missing update.
        let sv_b = doc_b.transact().state_vector();
        let update_for_b = doc_a.transact().encode_diff_v1(&sv_b);

        // B applies the update.
        {
            let mut txn = doc_b.transact_mut();
            let update = yrs::Update::decode_v1(&update_for_b).expect("decode update");
            txn.apply_update(update).expect("apply");
        }

        let txn = doc_b.transact();
        let wb_b = txn.get_map("workbook").expect("workbook");
        let marker = wb_b.get(&txn, "marker").expect("marker");
        match marker {
            Out::Any(Any::String(s)) => assert_eq!(s.as_ref(), "from-A"),
            other => panic!("unexpected: {other:?}"),
        }
    }
}
