# src/libs/colmena/src/google_oauth/domain/types.rs

**Layer:** domain  **Purpose:** Defines OAuth credential newtypes (AccessToken, RefreshTokenSecret, CachedToken) with intentional redaction and type safety to prevent accidental disclosure and token confusion.

## Symbols

- `AccessToken` (struct, pub) — Short-lived OAuth 2.0 bearer token newtype; Display is NOT redacted since tokens appear in HTTP headers anyway
- `AccessToken::as_str` (method, pub) — Borrows the token value as a string slice  [FLAG: dead_candidate — no in-file usage or test coverage; external callers unknown]
- `AccessToken::into_string` (method, pub) — Consumes the token and returns the owned String
- `fmt::Display for AccessToken` (impl) — Displays the token unhidden for debugging auth issues
- `RefreshTokenSecret` (struct, pub) — Long-lived refresh token newtype with redaction in Debug and Display
- `RefreshTokenSecret::new` (method, pub) — Constructor that converts any Into<String> value
- `RefreshTokenSecret::expose` (method, pub) — Exposes the raw value only when needed for refresh requests; callers warned never to log the result
- `fmt::Debug for RefreshTokenSecret` (impl) — Custom Debug that emits `<redacted>` instead of the token value
- `fmt::Display for RefreshTokenSecret` (impl) — Custom Display that emits `<redacted>` to prevent accidental leaks in panics and logs
- `CachedToken` (struct, pub) — Holds an access token and its UTC expiry timestamp; provider uses 60-second margin to avoid races
- `tests::refresh_token_redacts_in_debug_format` (test) — Verifies Debug impl does not leak refresh token value
- `tests::refresh_token_redacts_in_display_format` (test) — Verifies Display impl does not leak refresh token value
- `tests::refresh_token_expose_returns_raw_value` (test) — Verifies expose() method returns the unredacted token
- `tests::access_token_displays_normally` (test) — Verifies access tokens are NOT redacted in Display (by design)
- `tests::access_token_into_string_consumes` (test) — Verifies into_string() consumes and transfers ownership

## File-level notes

- Intentional design choice: two different redaction strategies (AccessToken visible, RefreshTokenSecret redacted) justified by credential lifetime and exposure surface
- Comprehensive test coverage for both redaction invariants and value extraction
- Well-documented module-level comment explains the dual purpose of newtypes: type safety and security
- No unfinished stubs, TODOs, panic-based placeholders, or other unfinished markers
- `AccessToken::as_str` lacks both in-file usage and test coverage; requires blast-radius check to determine if externally used
