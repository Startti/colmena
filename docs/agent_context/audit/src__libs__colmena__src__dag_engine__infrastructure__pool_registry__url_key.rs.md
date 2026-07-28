# src/libs/colmena/src/dag_engine/infrastructure/pool_registry/url_key.rs

**Layer:** infrastructure  
**Purpose:** Provides a normalized representation of Postgres connection URLs for use as registry keys. Normalizes scheme and host to lowercase, strips single trailing slashes, and preserves credentials (case-sensitive) and query parameters.

## Symbols

- `UrlKey` (struct, pub) — Newtype wrapper around String for normalized Postgres connection URLs; derives Debug, Clone, PartialEq, Eq, Hash for use as map keys
- `UrlKey::normalize()` (fn, pub) — Parses raw URL string, normalizes scheme and host (lowercase), preserves credentials and query parameters, strips single trailing slash, returns UrlKey without external crate dependency
- `UrlKey::as_str()` (fn, pub) — Returns normalized URL as borrowed &str slice
- `Display for UrlKey` (impl, pub) — Implements Display trait by delegating to inner String
- `fmt()` (fn) — Writes the normalized URL string to the formatter

## Tests

- `lowercases_scheme_and_host()` — Verifies scheme and host are normalized to lowercase regardless of input case
- `preserves_credentials_case_sensitive()` — Ensures credentials are not normalized (case-sensitive preservation)
- `strips_single_trailing_slash()` — Confirms trailing slash is removed from path without query string
- `preserves_query_parameters()` — Verifies query parameters are preserved and affect key equality
- `distinct_users_are_distinct_keys()` — Ensures different usernames produce different keys
- `handles_url_without_path()` — Tests edge case of URL with no path component

## File-level notes

- Intentionally avoids the `url` crate to keep dependencies minimal and normalization behavior predictable
- Conservative normalization strategy: only lowercase scheme/host, preserve everything else to avoid false collisions
- Well-commented parsing logic handles all URL component variants (credentials, port, path, query)
- Tests provide comprehensive coverage of edge cases and the contract (case handling, query preservation, trailing slash)
- No error handling needed; code is intentionally lenient for malformed URLs (falls through with minimal processing)
