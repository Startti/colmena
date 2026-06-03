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
//! or our own Rust `tool_executor`) sync with our server.

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

// Non-sync outer message tag constants (y-sync doesn't re-export these).
// MSG_AWARENESS is caught by the `_` arm in parse_msgs but kept here to
// document the protocol numbering alongside MSG_AUTH and MSG_QUERY_AWARENESS.
#[allow(dead_code)]
const MSG_AWARENESS: u8 = 1;
const MSG_AUTH: u8 = 2;
const MSG_QUERY_AWARENESS: u8 = 3;

// ─── helpers ────────────────────────────────────────────────────────────────

/// Encode a sync_step1 message: `[MSG_SYNC][MSG_SYNC_STEP_1][sv_bytes]`.
///
/// Exposed as `pub(super)` so `tool_executor` can send client-side step1 frames
/// during the full sync handshake.
pub(super) fn encode_sync_step1(sv: &StateVector) -> Vec<u8> {
    let mut enc = EncoderV1::new();
    enc.write_var(MSG_SYNC);
    enc.write_var(MSG_SYNC_STEP_1);
    enc.write_buf(sv.encode_v1());
    enc.to_vec()
}

/// Encode a sync_step2 message: `[MSG_SYNC][MSG_SYNC_STEP_2][update_bytes]`.
///
/// Exposed as `pub(super)` so `tool_executor` can build client-side frames
/// without duplicating the framing logic.
pub(super) fn encode_sync_step2(update: &[u8]) -> Vec<u8> {
    let mut enc = EncoderV1::new();
    enc.write_var(MSG_SYNC);
    enc.write_var(MSG_SYNC_STEP_2);
    enc.write_buf(update);
    enc.to_vec()
}

/// Decode the server's sync_step1 message and return the raw state-vector
/// bytes (already unwrapped from the length-prefix).
///
/// Returns `None` if `bytes` is not a well-formed step1 frame.
/// Exposed as `pub(super)` for use by `tool_executor`.
pub(super) fn decode_sync_step1_sv(bytes: &[u8]) -> Option<Vec<u8>> {
    use yrs::encoding::read::{Cursor, Read};
    let mut cur = Cursor::new(bytes);
    let outer: u8 = cur.read_var().ok()?;
    if outer != MSG_SYNC {
        return None;
    }
    let sub: u8 = cur.read_var().ok()?;
    if sub != MSG_SYNC_STEP_1 {
        return None;
    }
    let sv_bytes = cur.read_buf().ok()?;
    Some(sv_bytes.to_vec())
}

/// Decode a sync_step2 (or update) message and return the raw update bytes.
///
/// Matches both `MSG_SYNC_STEP_2` and `MSG_SYNC_UPDATE` sub-tags, since both
/// carry an update payload. Returns `None` for non-matching frames.
/// Exposed as `pub(super)` so `tool_executor` can wait for the server's step2
/// during the full sync handshake.
pub(super) fn decode_sync_step2_update(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut cur = Cursor::new(bytes);
    let outer: u8 = cur.read_var().ok()?;
    if outer != MSG_SYNC {
        return None;
    }
    let sub: u8 = cur.read_var().ok()?;
    if sub != MSG_SYNC_STEP_2 && sub != MSG_SYNC_UPDATE {
        return None;
    }
    let update_bytes = cur.read_buf().ok()?;
    Some(update_bytes.to_vec())
}

/// Encode an update message: `[MSG_SYNC][MSG_SYNC_UPDATE][update_bytes]`.
pub(super) fn encode_update(update: &[u8]) -> Vec<u8> {
    let mut enc = EncoderV1::new();
    enc.write_var(MSG_SYNC);
    enc.write_var(MSG_SYNC_UPDATE);
    enc.write_buf(update);
    enc.to_vec()
}

// ─── message parser ──────────────────────────────────────────────────────────

/// All Yjs sync-v1 message variants we care about.
pub(super) enum SyncMsg {
    /// Client's state vector; we reply with step2 (missing updates).
    Step1 { sv_bytes: Vec<u8> },
    /// Client's missing-updates payload; we apply it.
    Step2OrUpdate { update_bytes: Vec<u8> },
}

/// Parse zero or more sync messages from a raw byte buffer.
///
/// Ignores non-sync messages (awareness, auth, etc.) silently — the spike
/// doesn't need them.
pub(super) fn parse_msgs(bytes: &[u8]) -> Result<Vec<SyncMsg>> {
    let mut cur = Cursor::new(bytes);
    let mut out = Vec::new();
    while cur.has_content() {
        let outer_tag: u8 = cur
            .read_var()
            .map_err(|e| anyhow!("outer tag: {e:?}"))?;
        if outer_tag != MSG_SYNC {
            match outer_tag {
                MSG_QUERY_AWARENESS => {
                    // tag=3: no payload bytes — nothing to consume.
                }
                MSG_AUTH => {
                    // tag=2: a varint permission code (0=denied, 1=granted),
                    // optionally followed by a varint-length-prefixed reason
                    // string when permission is denied.
                    let perm: u64 = cur
                        .read_var()
                        .map_err(|e| anyhow!("skip auth perm: {e:?}"))?;
                    if perm == 0 && !cur.buf.is_empty() {
                        let _ = cur
                            .read_buf()
                            .map_err(|e| anyhow!("skip auth reason: {e:?}"))?;
                    }
                }
                _ => {
                    // MSG_AWARENESS (1) and any unknown tag with a
                    // varint-length-prefixed buffer payload.
                    let _ = cur
                        .read_buf()
                        .map_err(|e| anyhow!("skip non-sync buf: {e:?}"))?;
                }
            }
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
///
/// `post_update` is an optional callback invoked with the raw update bytes
/// after each successful `apply_update`. Use it to mark a snapshot dirty
/// or feed a `ChangeTracker`.
pub async fn handle_socket<F>(
    mut socket: WebSocket,
    doc: Arc<Doc>,
    post_update: Option<F>,
) -> Result<()>
where
    F: Fn(&[u8]) + Send + Sync + 'static,
{
    // 1. Send our initial sync_step1 (state vector).
    let sv = doc.transact().state_vector();
    socket
        .send(Message::Binary(encode_sync_step1(&sv)))
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
                        if socket.send(Message::Binary(bytes)).await.is_err() {
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
                                .send(Message::Binary(encode_sync_step2(&diff)))
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
                            if let Some(cb) = &post_update {
                                cb(&update_bytes);
                            }
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

    /// Regression for the R1.1 blocker: y-websocket sends MSG_QUERY_AWARENESS
    /// with no payload as part of its handshake. The parser must skip it
    /// silently and continue processing subsequent messages in the same frame.
    #[test]
    fn parses_query_awareness_with_no_payload() {
        use super::{MSG_QUERY_AWARENESS, MSG_SYNC};
        use y_sync::sync::MSG_SYNC_STEP_1;
        use yrs::encoding::write::Write;
        use yrs::updates::encoder::{Encoder, EncoderV1};

        let mut enc = EncoderV1::new();
        // outer tag 3 (query_awareness) — no payload
        enc.write_var(MSG_QUERY_AWARENESS as u64);
        // then MSG_SYNC + sub-tag 0 (sync_step1) + empty state vector buffer
        enc.write_var(MSG_SYNC as u64);
        enc.write_var(MSG_SYNC_STEP_1 as u64);
        enc.write_buf([]);
        let bytes = enc.to_vec();

        let msgs = super::parse_msgs(&bytes).expect("parse must succeed");
        assert_eq!(msgs.len(), 1, "should yield exactly one sync_step1");
        assert!(
            matches!(msgs[0], super::SyncMsg::Step1 { .. }),
            "expected Step1"
        );
    }

    /// MSG_AWARENESS (tag=1) has a varint-length-prefixed payload.
    /// Parser must consume it cleanly and still yield the following sync message.
    #[test]
    fn parses_awareness_with_payload_then_sync() {
        use super::MSG_SYNC;
        use y_sync::sync::MSG_SYNC_STEP_1;
        use yrs::encoding::write::Write;
        use yrs::updates::encoder::{Encoder, EncoderV1};

        let mut enc = EncoderV1::new();
        // MSG_AWARENESS = 1 with a 4-byte payload
        enc.write_var(1u64);
        enc.write_buf([0xDE, 0xAD, 0xBE, 0xEF]);
        // then a normal sync_step1 with an empty SV
        enc.write_var(MSG_SYNC as u64);
        enc.write_var(MSG_SYNC_STEP_1 as u64);
        enc.write_buf([]);
        let bytes = enc.to_vec();

        let msgs = super::parse_msgs(&bytes).expect("parse must succeed");
        assert_eq!(msgs.len(), 1, "should yield exactly one sync_step1");
        assert!(
            matches!(msgs[0], super::SyncMsg::Step1 { .. }),
            "expected Step1"
        );
    }
}
