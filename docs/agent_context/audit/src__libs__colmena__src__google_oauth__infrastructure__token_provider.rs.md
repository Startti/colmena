# src/libs/colmena/src/google_oauth/infrastructure/token_provider.rs

**Layer:** infrastructure  **Purpose:** Production implementation of OAuth token refresh with concurrent access coalescing, cache-based reuse, and token-rotation detection via HTTP refresh client.

## Symbols

- `EXPIRY_MARGIN_SECONDS` (const, private) — Safety margin in seconds subtracted from token expiry when deciding cache validity; prevents request-in-flight race with server-side expiry
- `OAuthRefreshTokenProvider` (struct, public) — Concrete `AuthTokenProvider` maintaining credentials, HTTP refresh client, and `Arc<Mutex<Option<CachedToken>>>` for lock-based cache and concurrency coalescing
- `OAuthRefreshTokenProvider::new` (fn, public) — Constructs provider from `OAuthCredentials` with default refresh client targeting Google's OAuth endpoint
- `OAuthRefreshTokenProvider::with_endpoint` (fn, public) — Constructs provider for custom token endpoints used by http_request native OAuth flow
- `OAuthRefreshTokenProvider::with_refresh_client` (fn, public, #[cfg(test)]) — Test-only constructor accepting custom `RefreshClient` for wiremock control
- `OAuthRefreshTokenProvider::invalidate_cache` (async fn, public) — Clears cached access token to force refresh on next call; used by HTTP retry logic after 401 errors
- `impl AuthTokenProvider for OAuthRefreshTokenProvider` (trait impl) — Provides `get_bearer_token` async method with cache-hit fast path and lock-protected refresh logic
- `get_bearer_token` (async fn, public) — Returns cached token if valid (>60s margin), else refreshes via HTTP, logs rotation events, updates cache under lock for thundering-herd coalescing
- `build_cached_token` (fn, private) — Constructs `CachedToken` from `RefreshResponse` by anchoring expiry to current time plus server-reported `expires_in`
- `log_token_rotation` (fn, private) — Emits structured tracing::warn event for token rotation without logging the sensitive rotated value itself
- `mod tests` (module, private, #[cfg(test)]) — Test suite with 6 tokio/wiremock tests covering cache behavior, concurrency, expiry, rotation, and failure cases

## File-level notes

- **Design intent:** The file implements two key patterns:
  1. **Lock-based thundering-herd coalescing:** Mutex acquisition order ensures one task refreshes while others wait on the lock and observe the cached result (cheap because critical section is tiny).
  2. **Token rotation observability:** When Google rotates `refresh_token` in the OAuth response, a structured log event is emitted per contract; the new value is NOT persisted (Secret Manager write out of scope) and the old token remains valid until it fails.
  
- **Error handling:** Failed refreshes (e.g., revoked refresh token) propagate `OAuthError` and leave cache empty, ensuring next attempt retries from scratch rather than serving stale token.

- **Test coverage:** Comprehensive: fast-path cache reuse, near-expiry refresh trigger, concurrent coalescing (10-task load test with wiremock `expect(1)`), token rotation handling, and failed-refresh cache cleanup.

- **Naming & clarity:** Well-documented module-level comment explains mutex roles and rotation behavior. Unused parameter `_rotated_refresh_token` in `log_token_rotation` is intentional (security: token value is not logged).
