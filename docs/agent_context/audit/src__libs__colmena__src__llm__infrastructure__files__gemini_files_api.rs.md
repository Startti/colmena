# src/libs/colmena/src/llm/infrastructure/files/gemini_files_api.rs

**Layer:** infrastructure  
**Purpose:** Implements resumable upload adapter for Gemini Files API, handling 8MB chunked uploads with 48-hour file expiration.

## Symbols

- `CHUNK_SIZE` (const, private) — 8 MB chunk size constant for resumable uploads
- `GeminiFilesApiAdapter` (struct, pub) — Adapter implementing FileProviderRepository for Gemini's resumable upload protocol
- `GeminiFilesApiAdapter::new` (fn, pub) — Creates adapter configured for production Gemini API
- `GeminiFilesApiAdapter::with_base_url` (fn, pub) — Creates adapter with custom base_url (for test mocking)
- `GeminiFilesApiAdapter::default_client` (fn, private) — Builds reqwest Client with 10s connect timeout, 600s overall timeout, and colmena user-agent
- `GeminiFilesApiAdapter::base_url` (fn, pub) — Getter exposing configured endpoint for testing and diagnostics
- `GeminiFilesApiAdapter::start_session` (async fn, private) — Initiates resumable upload session via POST, returns upload URL from X-Goog-Upload-URL header
- `GeminiFilesApiAdapter::put_chunk` (async fn, private) — Uploads a chunk via PUT with offset and command headers, optionally finalizes and parses response
- `UploadFinalizeResponse` (struct, private) — Deserializes finalize PUT response containing file metadata
- `GeminiFile` (struct, private) — Deserializes file metadata from Gemini API (name field)
- `FileProviderRepository::upload_streaming` (async fn) — Main upload method managing stream-to-chunks buffering, multi-PUT protocol, 48-hour TTL metadata
- `FileProviderRepository::ttl` (fn) — Returns 48-hour TTL for Gemini files
- `FileProviderRepository::provider` (fn) — Returns ProviderKind::Google
- `tests::fake_stream` (fn, private) — Test helper creating a BoxedByteStream from byte slice
- `tests::small_file_one_chunk_uploads` (async test) — Verifies single-chunk upload flow with mocked session init and finalize responses
- `tests::session_start_failure_propagates` (async test) — Verifies HTTP 403 session-start failure is propagated as FileApiUploadFailed
- `tests::ttl_is_48h` (test) — Verifies TTL getter returns 48-hour duration

## File-level notes

- **Well-structured infrastructure adapter** implementing hexagonal pattern cleanly: domain trait (`FileProviderRepository`) via port/adapter separation.
- **Resumable upload logic sound** — correctly manages multi-PUT protocol with CHUNK_SIZE chunking, buffer split strategies, and finalize detection; nested loop structure is necessary and clearly commented.
- **Error handling defensive** — HTTP failures map to domain `LlmError::FileApiUploadFailed` with context; response-body capture uses `unwrap_or_default()` to suppress parse errors when reporting other errors (minor but acceptable for error logging).
- **Test coverage adequate** — happy path (single chunk), error propagation, and TTL verified with wiremock mocking; tests correctly use `with_base_url` for isolation.
- **Code comments bilingual** — module doc and most inline comments in Spanish (consistent with project convention for infrastructure code). English user-facing doc comments present on public API.
- **No clippy warnings** — code follows Rust idioms; `expect()` on Client::builder is justified (should never panic in practice).

No flags. Code is complete, well-tested, and ready for production use.
