# src/libs/colmena/src/crdt_documents/server.rs

**Layer:** infrastructure  **Purpose:** Axum HTTP server router for CRDT documents, exposing REST CRUD endpoints, WebSocket Yjs sync protocol, and cross-artifact operations (sheets, import/export).

## Symbols

- `router` (fn, pub) — Creates Axum Router with all REST/WS routes bound to CrdtDocumentsRuntime state
- `CreateRequest` (struct) — Deserialized POST body with artifact name and optional agent_session_id
- `CreateResponse` (struct) — Serialized response with generated artifact_id and created_at timestamp
- `create_handler` (fn, async) — POST /documents handler; creates artifact and optionally touches change-tracker store
- `ListResponse` (struct) — Serialized wrapper for list of artifact metadata
- `list_handler` (fn, async) — GET /documents handler; returns all in-memory artifacts from registry
- `delete_handler` (fn, async) — DELETE /documents/:id handler; removes artifact, stops writer, deletes storage
- `changes_handler` (fn, async) — GET /documents/:id/changes handler; queries change-tracker store with since/limit/sheet_id/exclude_origin filters
- `RecordEventBody` (struct) — Deserialized POST body with sheet_id, origin, summary for manual event recording
- `record_event_handler` (fn, async) — POST /documents/:id/events handler; inserts event into change-tracker store
- `CursorBody` (struct) — Deserialized POST body with agent_session_id and last_event_id
- `set_cursor_handler` (fn, async) — POST /documents/:id/cursor handler; upserts cursor position for agent session
- `get_cursor_handler` (fn, async) — GET /documents/:id/cursor handler; retrieves last_event_id cursor for agent session
- `by_session_handler` (fn, async) — GET /documents/by-session/:sid handler; returns artifacts associated with session (with limit)
- `sheets_with_counts_handler` (fn, async) — GET /documents/:id/sheets-with-counts handler; delegates to LLM tool helper and translates artifact_not_found error to 404
- `ImportSheetBody` (struct) — Deserialized POST body with source artifact/sheet, optional new_name and dest_session_id for attribution
- `import_sheet_handler` (fn, async) — POST /documents/:id/import-sheet handler; validates and delegates to import_sheet_runtime helper
- `INDEX_HTML` (const) — Static HTML bytes for GET / (Univer demo page)
- `MINIMAL_HTML` (const) — Static HTML bytes for GET /minimal (diagnostic page without Univer)
- `index` (fn, async) — GET / handler; returns static INDEX_HTML
- `minimal` (fn, async) — GET /minimal handler; returns static MINIMAL_HTML
- `fixture_xlsx` (fn, async) — GET /spike.xlsx handler; reads fixture spreadsheet from file (default spike/fixtures/test.xlsx or COLMENA_SPIKE_FIXTURE_XLSX env var)
- `ws_handler` (fn, async) — WebSocket handler for Yjs sync protocol at /documents/:id/yjs and /yjs/:id (alias); auto-creates artifact on first hit; spawns dedicated thread for socket I/O (yrs::Subscription is !Send); records peer updates via change-tracker
- `projection_handler` (fn, async) — GET /documents/:id/projection.json handler; strips .json suffix and returns current Yrs→IR projection from registry
- `ImportResponse` (struct) — Serialized response with sheets_imported and cells_imported counts
- `import_handler` (fn, async) — POST /documents/:id/import handler; imports XLSX bytes into artifact doc; signals dirty and notifies snapshot writer
- `export_handler` (fn, async) — GET /documents/:id/export.xlsx handler; strips .xlsx suffix and exports doc to XLSX bytes
- `tests` (mod) — Test module with 3 integration tests
- `fresh_runtime` (fn, async) — Test helper; creates temp directory and CrdtDocumentsRuntime from JSON config
- `projection_returns_404_for_unknown_artifact` (test) — Verifies 404 for unknown artifact ID on projection endpoint
- `projection_returns_empty_for_registered_artifact` (test) — Verifies 200 with empty sheets array for newly registered artifact
- `projection_rejects_invalid_id` (test) — Verifies 400 BAD_REQUEST for malformed artifact ID

## File-level notes

- **Unfinished (v1.1 TODO)** at lines 435–439: `post_update` callback uses coarse summary (byte count only) and loses per-cell diff information. Comment notes two v1.1 approaches: (a) capture pre-update doc state in handle_socket and pass it in, or (b) refactor handle_socket to invoke post_update with pre-state clone. Currently implemented as coarse summary and documented as acceptable for v1 when change narration is informational only.
- **Thread safety pattern** at ws_handler (lines 420–428): WebSocket socket driven on dedicated thread with single-threaded tokio runtime because yrs::Subscription is !Send. Uses tokio::sync::oneshot channel for coordination.
- **Peer attribution** at ws_handler (lines 410–451): Captures peer_type and session_id from URL query params to distinguish agent vs browser updates and tag them as `agent:<session_id>` or `peer:browser` in change-tracker origin field.
- **Fire-and-forget tracking** at ws_handler (lines 456–464): change-tracker record call spawned async (best-effort) to avoid blocking WS reader; trade-off accepted for v1 (events may land out-of-order or be lost under high load).
- **Best-effort store writes** at create_handler (line 90): touch_artifact store failure intentionally ignored to avoid failing artifact creation on change-tracker unavailability.
- **Repetitive ID parsing**: All handlers parse `id_str: Path<String>` via `ArtifactId::from_str()` with consistent 400 BAD_REQUEST error path; no extraction to middleware.
- **Alias routes**: `/yjs/:id` alias for `/documents/:id/yjs` accommodates y-websocket client library's URL construction without requiring URL-encoded slashes.
- **Legacy fixture path**: fixture_xlsx defaults to `spike/fixtures/test.xlsx` for diagnostic use; kept for backward compatibility with demo HTML even though Task 9 API is canonical.
