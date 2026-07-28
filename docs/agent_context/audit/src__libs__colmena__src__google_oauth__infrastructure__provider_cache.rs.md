# src/libs/colmena/src/google_oauth/infrastructure/provider_cache.rs

**Layer:** infrastructure  
**Purpose:** Process-wide cache of `OAuthRefreshTokenProvider` instances keyed by a SHA-256 fingerprint of credentials, ensuring that all nodes/tools with the same identity (token_url + client_id + refresh_token) reuse a single access-token cache and token mint.

## Symbols

- `OAuthProviderCache` (struct, pub) — Mutex-wrapped HashMap mapping credential fingerprint strings to Arc-wrapped providers; created once and injected into HttpNode at registry init.
- `OAuthProviderCache::new()` (fn, pub) — Returns an empty cache using Default.
- `OAuthProviderCache::fingerprint()` (fn, pub, static) — Computes SHA-256 hex digest of (token_url, client_id, client_secret, refresh_token) tuple with null-byte separators to prevent field concat collision; hashes refresh token so it never appears in plaintext in the key.
- `OAuthProviderCache::get_or_create()` (fn, pub) — Acquires the cache lock, looks up provider by fingerprint, creates and inserts a new OAuthRefreshTokenProvider on cache miss, and returns an Arc clone; guarantees identity-based deduplication.
- `tests::same_creds_return_same_provider_arc()` (test, private) — Verifies that two calls with identical credentials return the same Arc (pointer equality).
- `tests::different_creds_return_different_providers()` (test, private) — Verifies that different refresh tokens result in different Arc instances.
- `tests::fingerprint_does_not_contain_plaintext_refresh_token()` (test, private) — Confirms fingerprint output never includes the plain refresh_token string.

## File-level notes

- No flagged items. Code is clean, well-documented, and defensive. All symbols have clear purposes.
- The credential fingerprinting strategy (hashing vs. embedding) is explicit and well-tested; null-byte separators are safe from concat collision.
- Mutex poison handling via `.expect()` is idiomatic for infrastructure-level caches where a poisoned state indicates process-wide inconsistency.
- Tests adequately cover the three key behaviors: deduplication by identity, separation by difference, and token secrecy in the cache key.
