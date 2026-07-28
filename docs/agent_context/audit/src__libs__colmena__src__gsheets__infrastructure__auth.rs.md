# src/libs/colmena/src/gsheets/infrastructure/auth.rs

**Layer:** infrastructure  **Purpose:** Token acquisition and caching for Google Sheets REST client with dual production/test paths — production delegates to shared `google_oauth` subsystem; test uses static pre-seeded bearer tokens for wiremock HTTP tests.

## Symbols

- `StaticCached` (struct, test-only) — in-process token cache with token string and expiration time (mirrored from legacy for backward-compat via `set_token_for_test`)
- `Inner` (enum, private) — enum holding either `OAuth(Arc<OAuthRefreshTokenProvider>)` for production or test `Static` variant with cache + sticky token
- `TokenProvider` (struct, pub) — cheap-to-clone token source wrapper holding inner variant; public interface for token provisioning
- `TokenProvider::from_oauth_credentials` (pub fn) — production constructor wrapping `OAuthRefreshTokenProvider` built from credentials; reads credentials once at startup
- `TokenProvider::for_tests_static` (pub fn, test-only) — test constructor with static cache; token starts unset until `set_token_for_test` seeds it
- `TokenProvider::token` (pub async fn) — returns fresh bearer token; delegates to OAuth provider for production or reads seeded value for tests
- `TokenProvider::invalidate` (pub async fn) — force-invalidate cache (production clears OAuth cache; test path re-seeds from sticky value for 401-refresh wiremock loop)
- `TokenProvider::set_token_for_test` (pub async fn, test-only) — seed both cache and sticky value with known token; panics if called on OAuth variant
- `token_error_to_sheets_error` (private fn) — maps `crate::google_oauth::domain::OAuthError` variants to `SheetsError` domain vocabulary

## File-level notes

- Well-scoped infrastructure layer: exports only `TokenProvider` public API; internal `Inner` enum cleanly separates production vs test paths without exposing variance to callers
- `StaticCached::expires_at` carries `#[allow(dead_code)]` intentionally — comment documents it mirrors legacy shape for test-helper backward-compat, never enforced in test path
- Sequential Mutex locks in `invalidate()` and `set_token_for_test()` are clear and safe (distinct mutexes, no deadlock risk)
- Error mapping in `token_error_to_sheets_error` covers all `OAuthError` variants; `RefreshTokenRevoked`, `ClientCredsInvalid`, `ConfigMissing` → `NotConfigured`; `Transient(msg)` → `AuthFailed`
- Test module `tests` covers seeded token, invalidate-with-reseed, missing seed error, and clone cost — good coverage for dual-path logic
