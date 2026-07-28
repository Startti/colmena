# src/libs/colmena/src/crdt_documents/yjs_protocol.rs

**Layer:** infrastructure  **Purpose:** Implements Yjs sync v1 protocol bridge between WebSocket and yrs::Doc, handling message encoding/decoding and sync handshake without depending on y-sync's incompatible yrs version.

## Symbols

- `MSG_AWARENESS` (const, pub) — protocol message tag (1) for awareness updates; intentionally unused, kept for protocol documentation
- `MSG_AUTH` (const, pub) — protocol message tag (2) for authentication responses
- `MSG_QUERY_AWARENESS` (const, pub) — protocol message tag (3) for awareness queries
- `encode_sync_step1(sv: &StateVector) -> Vec<u8>` (fn, pub(super)) — encodes state vector into step1 frame: `[MSG_SYNC][MSG_SYNC_STEP_1][sv_bytes]`
- `encode_sync_step2(update: &[u8]) -> Vec<u8>` (fn, pub(super)) — encodes update payload into step2 frame: `[MSG_SYNC][MSG_SYNC_STEP_2][update_bytes]`
- `decode_sync_step1_sv(bytes: &[u8]) -> Option<Vec<u8>>` (fn, pub(super)) — decodes step1 frame and returns raw state-vector bytes (length-prefix stripped)
- `decode_sync_step2_update(bytes: &[u8]) -> Option<Vec<u8>>` (fn, pub(super)) — decodes step2 or update frame and returns raw update bytes; matches both MSG_SYNC_STEP_2 and MSG_SYNC_UPDATE sub-tags
- `encode_update(update: &[u8]) -> Vec<u8>` (fn, pub(super)) — encodes update payload into update frame: `[MSG_SYNC][MSG_SYNC_UPDATE][update_bytes]`
- `SyncMsg` (enum, pub(super)) — parsed sync message variant: `Step1 { sv_bytes }` or `Step2OrUpdate { update_bytes }`
- `parse_msgs(bytes: &[u8]) -> Result<Vec<SyncMsg>>` (fn, pub(super)) — parses zero or more sync messages from raw bytes, silently ignoring non-sync messages (awareness, auth, etc.)
- `handle_socket<F>(socket: WebSocket, doc: Arc<Doc>, post_update: Option<F>) -> Result<()>` (async fn, pub) — drives single WebSocket connection's sync loop: sends initial state vector, subscribes to updates, concurrently reads incoming frames and applies them; invokes optional `post_update` callback after each successful update
- `two_docs_converge_via_sync_messages()` (test) — verifies sync protocol at message level by feeding one doc's state vector and update to another and asserting value convergence
- `parses_query_awareness_with_no_payload()` (test) — regression test for y-websocket's MSG_QUERY_AWARENESS (no payload) handshake; asserts parser skips it and yields following sync_step1
- `parses_awareness_with_payload_then_sync()` (test) — verifies parser consumes MSG_AWARENESS (tag=1) with varint-length-prefixed payload and continues to parse subsequent sync_step1

## File-level notes

- **Module documentation (lines 1–20):** Explains why y-sync's `MessageReader` / `Awareness` cannot be used directly (yrs version clash: y-sync 0.4 pins yrs 0.17, crate uses yrs 0.26). Solution borrows only message-tag constants from y-sync and implements framing independently in yrs 0.26.
- **Well-scoped and complete:** All functions have robust error handling via `.map_err()` with contextual messages. No unfinished code (no `todo!()`, `unimplemented!()`, or `FIXME` comments).
- **Test coverage:** Three focused unit tests cover protocol parsing (awareness/auth/sync message mixing, state convergence) without requiring live WebSocket or network I/O.
- **Constants documentation (lines 33–39):** MSG_AWARENESS, MSG_AUTH, MSG_QUERY_AWARENESS kept alongside imported y-sync constants to document full protocol numbering; MSG_AWARENESS is marked `#[allow(dead_code)]` with justification.
- **Pub(super) visibility pattern:** All functions and types are `pub(super)`, indicating they are module-internal interfaces used by `tool_executor` and other callers in the `crdt_documents` module hierarchy.
