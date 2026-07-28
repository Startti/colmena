# src/libs/colmena/src/llm/infrastructure/files/signed_url_downloader.rs

**Layer:** infrastructure  
**Purpose:** HTTP adapter that streams GCS V4 signed URL responses without Authorization headers, which would invalidate the query-parameter-encoded signature.

## Symbols

- `SignedUrlDownloader` (struct, pub) — HTTP client wrapper for downloading signed URLs via GET without auth headers
- `SignedUrlDownloader::new()` (fn, pub) — Constructs downloader with default reqwest client (10s connect, 600s total timeout)
- `SignedUrlDownloader::default_client()` (fn, private) — Builds preconfigured reqwest::Client with timeouts and user-agent
- `SignedUrlDownloader::with_client()` (fn, pub) — Constructs downloader reusing caller's existing Client to share connection pool
- `SignedUrlDownloader::stream()` (fn, pub async) — Fetches URL and streams response body as BoxedByteStream; returns NetworkError on transport failure or SignedUrlFetchFailed on non-2xx HTTP status
- `Default for SignedUrlDownloader` (impl) — Delegates to `new()`
- `SignedUrlFetcher for SignedUrlDownloader` (impl) — Implements domain trait by delegating to `stream()`
- `tests::stream_returns_body_chunks_on_2xx()` (fn, test) — Validates 200 response body streams correctly via wiremock
- `tests::stream_errors_on_403()` (fn, test) — Validates SignedUrlFetchFailed error on 403 (expired/invalid signature)
- `tests::stream_errors_on_404()` (fn, test) — Validates SignedUrlFetchFailed error on 404 (missing resource)
- `tests::stream_does_not_send_authorization()` (fn, test) — Confirms no Authorization header is sent (would invalidate signature)

## File-level notes

- All public methods have doc comments explaining behavior and error cases
- Error handling is complete at the boundary: network failures map to `LlmError::NetworkError`, non-2xx to `LlmError::SignedUrlFetchFailed`
- No panic-on-error production path (only in `default_client()` which is initialization-time)
- Test coverage is comprehensive (success, auth-related failures, auth-header validation)
- Follows hexagonal pattern: implements domain trait `SignedUrlFetcher`, takes no other domain dependencies
