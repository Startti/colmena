# src/libs/colmena/src/llm/application/llm_call_use_case.rs

**Layer:** Application  **Purpose:** Orchestrates LLM calls with transparent file resolution. Converts `FileSource::SignedUrl` entries to provider-uploaded files (Gemini Files API, Anthropic Files API, etc.), with caching, deduplication, and single-retry logic for provider file-not-found errors.

## Symbols

- `LlmCallUseCase` (struct, pub) — Main use case orchestrator; holds injected repository and optional file-resolution dependencies
- `LlmCallUseCase::new` (pub fn) — Constructor accepting only LLM repository; file resolution is opt-in via builder methods
- `LlmCallUseCase::with_file_cache` (pub fn) — Builder to inject file cache repository
- `LlmCallUseCase::with_file_provider_factory` (pub fn) — Builder to inject provider-specific file upload factory
- `LlmCallUseCase::with_signed_url_fetcher` (pub fn) — Builder to inject signed URL downloader
- `LlmCallUseCase::execute` (pub async fn) — Orchestrates one LLM call: resolves files, calls LLM, handles ProviderFileNotFound by invalidating cache and retrying with re-download
- `LlmCallUseCase::resolve_files_in_messages` (async fn, private) — Walks all messages and resolves files within each; no-op if file-resolution deps not injected
- `LlmCallUseCase::invalidate_provider_file_id` (async fn, private) — Clears cache entries for a specific provider_file_id after failure (best-effort)
- `LlmCallUseCase::snapshot_signed_urls` (fn, private) — Captures (document_id → signed_url) map before resolution to enable retry recovery
- `LlmCallUseCase::reset_uploaded_files_with_id` (fn, private) — Reverts FileSource::Uploaded back to SignedUrl using snapshot for retry re-download
- `LlmCallUseCase::resolve_files` (pub async fn) — Main file-resolution logic: per-file resilient, intra-request dedup, cache lookup, download→upload flow; returns AllFilesFailedToResolve if 100% failure
- `LlmCallUseCase::resolve_one` (async fn, private) — Resolves a single file across 3 branches (InlineBytes→upload or inline, Uploaded→passthrough, SignedUrl→cache-or-download-upload); handles dedup and retry snapshot management
- `tests` (mod, cfg(test)) — Unit tests for execute success, empty messages, and repository errors
- `resolve_files_tests` (mod, cfg(test)) — File resolution tests: StubCache and StubProvider mocks; cache hit/miss, intra-request dedup, inline byte upload, image URL passthrough
- `StubCache` (struct, cfg(test)) — Mock FileCacheRepository for testing; Mutex<Vec> storage
- `StubProvider` (struct, cfg(test)) — Mock FileProviderRepository for testing; increments upload counter
- `snapshot_and_reset_tests` (mod, cfg(test)) — Unit tests for snapshot capture and file reset during retry
- `retry_tests` (mod, cfg(test)) — E2E-like tests using wiremock to verify signed-URL redownload after ProviderFileNotFound

## File-level notes

- **Architecture:** Strict hexagonal — all external integrations (LLM, file cache, provider upload, signed URL fetch) are injected as trait objects; the use case knows only about domain types and ports
- **Retry logic:** Two-phase snapshot-and-reset pattern (lines 76–106): (1) snapshot original SignedUrls before first resolve, (2) if ProviderFileNotFound, invalidate cache for that file_id and reset matching Uploaded entries back to SignedUrl, then resolve again. Single retry only; a second LLM call is never attempted
- **Per-file resilience:** resolve_files (line 240) is intentionally non-atomic; one file's failure doesn't abort others (line 275–278). If all files fail, AllFilesFailedToResolve is raised; otherwise, only successful files are returned
- **File source hierarchy:** InlineBytes (text-like) → inline, no provider upload; Uploaded (caller-provided file_id) → passthrough; SignedUrl → download + upload (or direct passthrough for Anthropic/OpenAI images)
- **Intra-request dedup:** session_dedup HashMap (line 258) prevents uploading the same document_id twice within one resolve_files call; cross-request dedup is via cache lookup
- **Logging:** Extensive use of `crate::colmena_log!` throughout resolve_one (16 log statements) to aid debugging of file resolution paths
