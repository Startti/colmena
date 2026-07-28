# src/libs/colmena/src/llm/infrastructure/files/anthropic_files_api.rs

**Layer:** infrastructure  **Purpose:** Adapter implementing the Anthropic Files API (beta) for streaming file uploads via multipart/form-data. Handles HTTP request construction, response parsing, and error classification without automatic TTL.

## Symbols

- `BETA_HEADER` (const) — String constant for the required Anthropic beta header value `"files-api-2025-04-14"`
- `AnthropicFilesApiAdapter` (struct, pub) — HTTP client wrapper holding reqwest Client, base URL, and API key for Anthropic Files API uploads
- `AnthropicFilesApiAdapter::new` (fn, pub) — Factory constructor pointing at production Anthropic API with default timeouts
- `AnthropicFilesApiAdapter::with_base_url` (fn, pub) — Factory constructor with custom base URL for testing; trims trailing slashes
- `AnthropicFilesApiAdapter::default_client` (fn, private) — Creates a reqwest Client with 10s connect timeout, 600s request timeout, and colmena user-agent
- `AnthropicFilesApiAdapter::base_url` (fn, pub) — Getter exposing the configured endpoint URL for test assertions
- `UploadResponse` (struct, private) — Deserialization struct capturing the `id` field from successful Anthropic file upload response
- `FileProviderRepository` (trait impl for AnthropicFilesApiAdapter) — Implements domain trait for file storage abstraction
- `upload_streaming` (async fn) — Wraps byte stream into multipart/form-data part, POSTs to Anthropic Files API endpoint, classifies invalid MIME as InvalidMimeType error, maps HTTP failures to FileApiUploadFailed
- `ttl` (fn) — Returns None because Anthropic Files API does not auto-expire uploaded files
- `provider` (fn) — Returns ProviderKind::Anthropic
- `tests::fake_stream` (fn, private) — Helper creating a boxed async stream from a byte slice for test payloads
- `tests::upload_succeeds_returns_file_id` (async test) — Verifies successful upload returns file ID with correct metadata and no expiry
- `tests::upload_fails_on_413` (async test) — Verifies HTTP 413 error is mapped to FileApiUploadFailed
- `tests::ttl_is_none` (test) — Verifies TTL always returns None
- `tests::upload_classifies_invalid_mime_as_invalid_mime_type` (async test) — Verifies malformed MIME string raises InvalidMimeType before HTTP traffic

## File-level notes

- Clean, focused implementation of a single external API adapter with no domain dependencies beyond trait contracts
- Error handling explicitly maps HTTP and parse failures to domain errors (`LlmError` variants) for proper retry semantics
- Uses wiremock for all integration tests; no real API calls required to verify behavior
- Spanish inline comments clarify the beta header requirement and error classification rationale
- No unused code, no incomplete implementations, no panics in production paths
