# src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_context.rs

**Layer:** infrastructure  **Purpose:** Provides a unified execution context for CRDT document tools, abstracting between local in-process documents and remote WebSocket-backed peers. Decouples tool dispatchers from the underlying document source and manages change event tracking across both modes.

## Symbols

- `CrdtDocsContext` (enum, pub) — Represents the execution context for CRDT document tools with two variants: Local (in-process runtime) and WsPeer (WebSocket-backed remote).
- `CrdtDocsContext::Local` (variant, pub) — Local, in-process CRDT document mode with runtime registry, session ID, backend for change tracking, and atomic max event ID.
- `CrdtDocsContext::WsPeer` (variant, pub) — Remote WebSocket-backed CRDT document mode with local replica, alive flag, session ID, REST backend, and atomic max event ID.
- `new_local` (fn, pub) — Constructor that builds a context for local execution delegated to a CrdtDocumentsRuntime with DirectBackend wrapping the runtime's ChangeTrackerStore.
- `new_ws_peer` (fn, pub) — Constructor that builds a context for WebSocket peer connection, accepting WsPeerArtifact and server base URL, using RestBackend for HTTP event recording.
- `artifact_id` (fn, pub) — Returns a reference to the artifact ID this context is bound to, pattern-matched across both Local and WsPeer variants.
- `session_id` (fn, pub) — Returns the optional agent_session_id captured from llm_call inputs for event attribution, as a deref'd string slice.
- `backend` (fn, pub) — Returns a reference to the CrdtBackend trait object used for recording and querying change events (DirectBackend for Local, RestBackend for WsPeer).
- `doc` (fn, pub) — Fetches the Y.Doc to operate on, retrieving from the registry in Local mode or returning cloned arc in WsPeer mode if alive, else None.
- `mark_dirty` (fn, pub) — Marks the artifact dirty so the snapshot writer flushes it; delegates to runtime in Local mode, no-op in WsPeer mode where the service handles persistence.
- `record_event_id` (fn, pub) — Tracks the highest event_id observed during the turn using atomic compare-and-swap to safely update the max across concurrent calls.
- `max_event_id_observed` (fn, pub) — Returns the highest event ID observed via record_event_id during this turn, or 0 if none recorded yet.
- `is_alive` (fn, pub) — Returns true if the context can still serve mutations; always true for Local, depends on alive flag for WsPeer.

## File-level notes

- All public API is well-documented with comprehensive doc comments explaining the purpose and behavior of each constructor and accessor method.
- The implementation correctly uses atomic operations (Acquire/Release ordering) in `record_event_id` and `max_event_id_observed` to safely track the highest event ID across concurrent tool dispatcher calls.
- No error handling needed at the public API level; accessor methods appropriately return `Option` for uncertain results (doc lookup, alive status).
- The two-mode design (Local/WsPeer) is cleanly separated with consistent method signatures across variants via pattern matching.
- No unused code, dead code patterns, or unfinished implementations detected.
- Comment references to "B-T12" suggest external specification tracking, but this does not impact code quality.
