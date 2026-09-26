# src/libs/colmena/src/llm/infrastructure/files/signed_url_downloader.rs

**Layer:** infrastructure  
**Purpose:** The one guarded HTTP client for attachment URLs (`files[].url`): http(s) only, global unicast addresses only (DNS resolver + IP-literal check on the URL and each redirect hop), no proxy, connect/total timeouts and a byte cap; no Authorization header (a signed URL carries its signature in the query). CHANGELOG 2026-09 §137.

## Symbols

- `SignedUrlDownloader` (struct, pub) — HTTP client wrapper for downloading signed URLs via GET without auth headers
- `SignedUrlDownloader::new()` (fn, pub) — The guarded client; `COLMENA_ATTACHMENT_ALLOW_PRIVATE_HOSTS` / `COLMENA_ATTACHMENT_MAX_BYTES` (default 100 MiB) read once per process, with one shared `reqwest::Client` (10s connect, 600s total)
- `GuardedResolver`, `literal_refused`, `guarded_client`, `cap` (private) — DNS check, IP-literal check (URL and redirect hops), client build, byte cap on the stream
- `SignedUrlDownloader::stream()` (fn, pub async) — Streams the body; `AttachmentUrlRefused` (nothing dialled), `AttachmentTooLarge`, `NetworkError`, `SignedUrlFetchFailed`
- `Default for SignedUrlDownloader` (impl) — Delegates to `new()`
- `SignedUrlFetcher for SignedUrlDownloader` (impl) — Implements domain trait by delegating to `stream()`
- `tests::stream_returns_body_chunks_on_2xx()` (fn, test) — Validates 200 response body streams correctly via wiremock
- `tests::stream_errors_on_403()` (fn, test) — Validates SignedUrlFetchFailed error on 403 (expired/invalid signature)
- `tests::stream_errors_on_404()` (fn, test) — Validates SignedUrlFetchFailed error on 404 (missing resource)
- `tests::stream_does_not_send_authorization()` (fn, test) — Confirms no Authorization header is sent (would invalidate signature)
- `tests::a_non_public_address_is_never_dialed`, `a_redirect_to_a_non_public_address_is_not_followed`, `only_http_urls_are_fetched`, `a_body_past_the_byte_cap_is_not_read`, `a_streamed_body_past_the_byte_cap_ends_in_an_error`, `a_transport_error_does_not_carry_the_url` (tests) — The guard's refusals; polarity tests use the test-only `public_only()`

## File-level notes

- All public methods have doc comments explaining behavior and error cases
- Error handling is complete at the boundary: network failures map to `LlmError::NetworkError`, non-2xx to `LlmError::SignedUrlFetchFailed`
- No panic-on-error production path (only the `expect` in `guarded_client()`, at client build)
- Test coverage is comprehensive (success, auth-related failures, auth-header validation)
- Follows hexagonal pattern: implements domain trait `SignedUrlFetcher`, takes no other domain dependencies
