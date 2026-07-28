# src/libs/colmena/src/crdt_documents/crdt_backend.rs

**Layer:** infrastructure  
**Purpose:** Abstraction layer for CRDT document storage backends. Provides a `CrdtBackend` trait with two implementations: `DirectBackend` (direct ChangeTrackerStore access for local/shared mode) and `RestBackend` (HTTP calls to remote CRDT service for ws_peer mode).

## Symbols

### Types & Traits
- `BackendError` (pub enum) — Error wrapper combining Store, Http, and Decode variants via thiserror; implements Display and From<StoreError>.
- `CrdtBackend` (pub trait, async) — Port defining six async operations: record_event, events_since (with filters), upsert_cursor, cursor_for, artifacts_for_session, and touch_artifact.

### DirectBackend
- `DirectBackend` (pub struct) — Holds `Arc<dyn ChangeTrackerStore>` for local backend access.
- `impl CrdtBackend for DirectBackend` — Six methods that transparently delegate to the wrapped store and convert StoreError into BackendError.

### RestBackend
- `RestBackend` (pub struct) — Holds `reqwest::Client` and `base_url` (String) for remote CRDT service calls.
- `RestBackend::new()` (pub fn) — Constructor that creates a fresh Client and trims trailing slash from base_url.
- `impl CrdtBackend for RestBackend` — Six methods that construct HTTP URLs, send requests, and parse JSON responses:
  - `record_event()` — POST to `/documents/{id}/events`; extracts event `id` from response.
  - `events_since()` — GET from `/documents/{id}/changes?since=...&limit=...` with optional sheet_id/exclude_origin filters; parses `events` array into `Vec<StoredEvent>`.
  - `upsert_cursor()` — POST to `/documents/{id}/cursor` with agent_session_id and last_event_id.
  - `cursor_for()` — GET from `/documents/{id}/cursor?agent_session_id=...`; returns None on NOT_FOUND, otherwise extracts last_event_id.
  - `artifacts_for_session()` — GET from `/documents/by-session/{id}?limit=...`; parses `artifacts` array into `Vec<StoredArtifact>`.
  - `touch_artifact()` — No-op; server handles touch via POST body on document creation.

### Tests
- `direct_backend_records_and_queries()` (async test) — Verifies DirectBackend can insert and query a single event via InMemoryChangeTrackerStore.

## File-level notes

- **Missing error context in JSON parsing (RestBackend):** All methods use `.unwrap_or()` or silent fallbacks for JSON field extraction (e.g., line 171: `as_u64().unwrap_or(0)`, lines 208–213: multiple `as_str().unwrap_or("")`). These hide parse failures and return default values instead of propagating decode errors. Suggests robust handling should distinguish between "field missing" and "field has wrong type" and report both to the caller.
  
- **Inconsistent fallback patterns:** Different methods use different default values when JSON parsing fails (0 for u64, empty string for str, None for Option<u64> at line 265). Consider standardizing or making defaults explicit in a DecodeError variant.

- **No status-code verification in events_since/artifacts_for_session:** RestBackend methods `events_since()` (line 192) and `artifacts_for_session()` (line 277) do not check HTTP response status before parsing JSON, unlike `record_event()` (line 164) and `upsert_cursor()` (line 237). Could fail silently with 5xx errors if the server returns error JSON.

- **DirectBackend implementation is transparent:** All methods are passthrough `?` conversions from StoreError to BackendError; no logic, no duplication. Clean adapter pattern.

- **RestBackend::touch_artifact is intentionally no-op:** Comment correctly documents that ws_peer mode (server-driven) handles touch at ingest time, so the client method is a stub by design.

- **Single test:** Only covers DirectBackend with in-memory store; RestBackend has no tests (would require HTTP mocking or integration infrastructure). Consider adding wiremock or similar for RestBackend path validation.
