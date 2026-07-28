# src/libs/colmena/src/google_oauth/infrastructure/refresh_client.rs

**Layer:** infrastructure  **Purpose:** Stateless HTTP transport layer for Google OAuth token refresh. POSTs to Google's token endpoint to exchange a refresh_token for a fresh access_token; implements retry logic for transient failures and error classification (revoked vs invalid credentials vs network).

## Symbols

- `DEFAULT_TOKEN_ENDPOINT` (const, private) — Google OAuth 2.0 token endpoint URL; overridable in tests for wiremock
- `PRODUCTION_RETRY_DELAYS` (const, private) — Exponential backoff schedule: 1s then 2s delays before giving up
- `RefreshResponse` (pub struct) — Successful token response with access_token, expires_in, and optional rotated_refresh_token
- `RefreshClient` (pub struct) — Stateless HTTP client holding reqwest::Client, endpoint URL, and retry configuration
- `RefreshClient::new()` (pub fn) — Production constructor with default Google endpoint and production retry delays
- `RefreshClient::with_endpoint()` (pub fn) — Production constructor accepting custom OAuth2 token endpoint (supports non-Google providers)
- `RefreshClient::for_tests()` (pub fn, cfg(test)) — Test constructor pointing at wiremock URL with fast 20/40ms retry delays for quick test execution
- `RefreshClient::refresh()` (pub async fn) — Main entry point; refreshes token with retry loop on transient errors, short-circuits on non-transient (4xx misconfig/revocation)
- `RefreshClient::refresh_once()` (async fn, private) — Single HTTP POST attempt; builds form payload, sends, parses JSON response, classifies errors by HTTP status and error code
- `impl Default for RefreshClient` — Delegates to `RefreshClient::new()`
- `is_transient()` (fn, private) — Helper predicate; returns true iff error is `OAuthError::Transient`
- `transient_inner_msg()` (fn, private) — Helper; extracts message from Transient variant, falls back to debug format
- `TokenSuccessBody` (struct, private) — Serde deserialization target for successful response JSON; mirrors Google's schema with optional refresh_token field
- `TokenErrorBody` (struct, private) — Serde deserialization target for error response JSON; parses error code and optional description
- `tests` (mod, cfg(test)) — 8 test cases covering happy path, rotated tokens, retry logic, error classification (invalid_grant → RefreshTokenRevoked, invalid_client → ClientCredsInvalid), retry exhaustion, and no-retry-on-4xx assertion

## File-level notes

- **Error handling:** Comprehensive classification: `invalid_grant` (400) → `RefreshTokenRevoked`; `invalid_client` (400) → `ClientCredsInvalid`; 5xx → `Transient` with retry; 401/403/other 4xx → `ClientCredsInvalid` (non-transient). Network errors (timeout, connect) are tagged as `Transient`.
- **Defensive parsing:** Response body parsing is fail-safe; if JSON parse fails on success path, wrapped as `Transient`; if JSON parse fails on error path, gracefully handles with `.ok()` and falls through to generic status-code handler.
- **Test isolation:** All tests use wiremock MockServer; config credentials via `OAuthCredentials::for_tests()` helper.
- **No caching or concurrency control:** By design, this layer is stateless; the wrapper `OAuthRefreshTokenProvider` in `token_provider.rs` adds the cache and mutex.
