# src/libs/colmena/src/crdt_documents/ws_peer.rs

**Layer:** infrastructure  **Purpose:** WebSocket peer client for syncing a local Y.Doc CRDT replica with a remote documents server. Manages bidirectional sync via Yjs protocol v1, exposing a `Doc` reference for tool dispatchers while handling all network I/O and subscription management in a background thread.

## Symbols

- `WsPeerError` (enum, pub) — error variants for connection/sync/send failures; displayed as Connect/Sync/Send/Closed
- `WsPeerError::Connect` (enum variant, pub) — connection failure with context string
- `WsPeerError::Sync` (enum variant, pub) — sync handshake failure (step1/step2 decode, state apply, protocol errors)
- `WsPeerError::Send` (enum variant, pub) — WebSocket send failure
- `WsPeerError::Closed` (enum variant, pub) — WebSocket closed unexpectedly during operation
- `WsPeerArtifact` (struct, pub) — holds Arc<Doc> local replica, artifact_id, alive flag, and channels to background sync task
- `WsPeerArtifact::doc` (field, pub) — Arc-wrapped local Y.Doc replica shared with tool dispatchers
- `WsPeerArtifact::artifact_id` (field, pub) — identifier for the CRDT artifact on the server
- `WsPeerArtifact::alive` (field, pub) — Arc<AtomicBool> flag set to false when background sync task exits
- `WsPeerArtifact::shutdown_tx` (field, private) — optional oneshot sender to signal shutdown to background task
- `WsPeerArtifact::done_rx` (field, private) — optional oneshot receiver that completes when background task exits
- `WsPeerArtifact::connect` (fn, pub async) — establishes WS connection to `<server_url>/<artifact_id>`, performs Yjs sync v1 handshake (step1/step2), spawns background sync thread, returns Self or WsPeerError
- `WsPeerArtifact::is_alive` (fn, pub) — returns true if background sync task is running (Acquire semantics)
- `WsPeerArtifact::shutdown` (fn, pub async) — signals background task to flush pending updates and close socket, waits for task to confirm; idempotent
- `peer_mutation_propagates_to_server` (fn, test) — integration test: spins up local CrdtDocumentsRuntime server, connects peer, mutates peer doc, polls server replica for convergence
- `server_mutation_propagates_to_peer` (fn, test) — integration test: mutates server-side doc directly, polls peer replica for inbound update convergence

## File-level notes

- **Architecture**: Correctly separates infrastructure (network/threading) from domain (Doc mutations). Tool dispatchers hold `Arc<Doc>` and never see WS or threading details.
- **Thread safety**: Yjs `Subscription` is !Send, so local tokio runtime spins up in a std::thread. Subscription created and dropped inside the thread boundary. Arc<AtomicBool> `alive` flag allows callers to detect socket death without blocking. Well-documented in lines 171–178.
- **Protocol**: Yjs sync v1 handshake correctly sequenced: (1) receive server state vector, (2) send our state vector, (3) receive server diff, (4) apply to local doc. After handshake, background task maintains bidirectional sync via SyncMsg::Step1 (respond to state requests) and SyncMsg::Step2OrUpdate (apply incoming updates).
- **Graceful shutdown**: On shutdown signal, drain pending updates channel before closing socket (lines 221–226). Idempotent `shutdown()` method handles multiple calls via `.take()`.
- **Error propagation**: Connection and sync errors surface via Result; parsing/send errors in the background task silently break the loop and set `alive=false`, requiring callers to check `is_alive()` after tool calls (documented in module doc lines 36–42).
- **Dead code**: Line 187 captures `server_sv` into the background task closure but never uses it. Comment says "for completeness; unused now" — this is vestigial and can be removed.
