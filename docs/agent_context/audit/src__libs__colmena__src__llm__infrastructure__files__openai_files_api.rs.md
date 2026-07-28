# src/libs/colmena/src/llm/infrastructure/files/openai_files_api.rs

**Layer:** infrastructure  
**Purpose:** Implements OpenAI Files API adapter for streaming file uploads via multipart/form-data with purpose=user_data, producing file_ids for use in chat.completions/responses content parts.

## Symbols

- `OpenAiFilesApiAdapter` (struct, pub) — Adapter wrapping reqwest Client, base_url, and API key for OpenAI Files API integration
- `OpenAiFilesApiAdapter::new` (fn, pub) — Creates adapter pointing to production OpenAI API (`https://api.openai.com`)
- `OpenAiFilesApiAdapter::with_base_url` (fn, pub) — Creates adapter with custom base_url (typically wiremock server in tests), defensively trims trailing `/`
- `OpenAiFilesApiAdapter::default_client` (fn, private) — Builds reqwest Client with 10s connect timeout, 600s request timeout, colmena user agent
- `OpenAiFilesApiAdapter::base_url` (fn, pub) — Accessor exposing configured endpoint for test assertions on env-var overrides
- `UploadResponse` (struct, private) — Deserialization struct for OpenAI `/v1/files` POST response, carrying `id` field
- `FileProviderRepository impl for OpenAiFilesApiAdapter::upload_streaming` (async fn) — Wraps byte stream in multipart form with `purpose=user_data`, POSTs to `/v1/files`, parses response into ProviderFileRef; maps MIME validation and HTTP errors to LlmError variants
- `FileProviderRepository impl for OpenAiFilesApiAdapter::ttl` (fn) — Returns None; OpenAI files do not expire automatically
- `FileProviderRepository impl for OpenAiFilesApiAdapter::provider` (fn) — Returns ProviderKind::OpenAi
- `tests::fake_stream` (fn, private) — Test helper creating BoxedByteStream from byte slice
- `tests::upload_succeeds_with_bearer_and_purpose` (async fn, test) — Verifies successful upload with Bearer auth and correct purpose field, confirms file_id in response
- `tests::upload_errors_on_400` (async fn, test) — Verifies FileApiUploadFailed error on HTTP 400
- `tests::ttl_is_none` (fn, test) — Verifies ttl() returns None
- `tests::upload_classifies_invalid_mime_as_invalid_mime_type` (async fn, test) — Verifies invalid MIME string produces InvalidMimeType error with mime echoed in error

## File-level notes

- Clean infrastructure adapter with comprehensive error handling: MIME validation (line 80), HTTP status checks (line 104), and JSON parse failures (line 114–119) all mapped to domain errors
- Tests use wiremock server for isolation; no external API calls
- Cross-references similar reasoning in AnthropicFilesApiAdapter (line 76–77 comment), suggesting consistent pattern across provider adapters
- All public methods have clear doc comments explaining behavior and edge cases
