# src/libs/colmena/src/gdocs/infrastructure/auth.rs

**Layer:** infrastructure  **Purpose:** Token acquisition and caching for the Google Docs REST client. Provides a production OAuth path delegating to the shared `google_oauth` subsystem and a test path with static pre-seeded tokens for wiremock tests without hitting real OAuth endpoints.

## Symbols

### Type Definitions
- `TOKEN_TTL` (const, test-only) — 50-minute duration for test token cache expiry
- `StaticCached` (struct, test-only, derives Debug + Clone) — Cached bearer token and expiry instant for test-mode caching
- `Inner` (enum, private) — Internal representation: either `OAuth(Arc<OAuthRefreshTokenProvider>)` for production or `Static { cache: Arc<Mutex<Option<StaticCached>>> }` for tests
- `TokenCache` (struct, pub, derives Clone) — Public token source wrapper; cheap to clone (inner state is Arc)

### Implementations
- `TokenCache::from_oauth_credentials(creds)` (pub fn) — Production constructor wrapping a shared `OAuthRefreshTokenProvider`
- `TokenCache::for_tests_static()` (pub fn, test-only) — Test constructor with a static cache that wiremock tests can pre-seed
- `TokenCache::get()` (pub async fn) — Return a non-expired bearer token; defers to OAuth provider in production, reads seeded value in tests
- `TokenCache::invalidate()` (pub async fn) — Force-invalidate cache; clears OAuth provider cache in production, no-op in tests (wiremock 401-refresh tests rely on seeded token surviving)
- `TokenCache::set_token_for_test()` (pub async fn, test-only, visibility pub(crate)) — Seed the cached token directly for wiremock-based tests
- `oauth_error_to_docs_error()` (fn, private) — Map OAuthError variants to DocsError types (revoked/invalid-creds/missing-config → NotConfigured; transient → AuthFailed)

### Tests
- `tests::cache_returns_seeded_token_within_ttl()` (async test) — Verify seeded token is returned within TTL
- `tests::invalidate_is_no_op_in_test_variant()` (async test) — Verify Static variant invalidate does NOT clear seeded token
- `tests::unseeded_static_cache_returns_auth_failed()` (async test) — Verify unseeded Static cache returns AuthFailed error
- `tests::token_cache_clone_shares_state()` (sync test) — Verify both clones see same backing storage (tested indirectly via seeding flow)

## File-level notes

- **Intentional `#[allow(dead_code)]` on `StaticCached.expires_at`**: Field mirrors production cache shape but tests don't enforce expiry at runtime; documented in line 27–28.
- **Parallel with `gsheets::infrastructure::auth`**: Deliberately kept distinct (mirrored structure) so both modules remain independently migratable, as noted in lines 8–9.
- **Intentional panic guard at line 125–128**: Programming error catch — `set_token_for_test` called on OAuth variant panics with clear message; not a stub.
- **All test-only code properly guarded with `#[cfg(test)]`**: No test infrastructure leaks into production.
- **Error mapping is exhaustive**: All `OAuthError` variants handled in `oauth_error_to_docs_error`.
- **No flagged issues**: Code is clean, well-structured, and intentionally designed. No dead symbols, incomplete implementations, or obvious improvements.
