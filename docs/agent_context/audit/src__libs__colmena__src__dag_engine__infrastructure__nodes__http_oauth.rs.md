# src/libs/colmena/src/dag_engine/infrastructure/nodes/http_oauth.rs

**Layer:** infrastructure  
**Purpose:** Implements OAuth2 (refresh_token grant) authentication for the http_request node, including parsing and validation of the `auth` config block and a 401-retry send helper with automatic token refresh.

## Symbols

- `OAuthAuthSpec` (struct, pub) — holds resolved OAuth2 credentials (token_url, client_id, client_secret, refresh_token); all `${ENV}` still unresolved
- `OAuthAuthSpec::fmt` (impl Debug, pub) — custom Debug that redacts client_secret and refresh_token fields to prevent accidental logging of secrets
- `parse_oauth_auth` (pub fn) — parses and validates the `auth` config block; returns Ok(None) if absent, Ok(Some(spec)) if valid, or Err with detailed message on validation failure (missing fields, wrong type, mutual exclusion with static auth, anti-exfiltration guard)
- `send_with_oauth_retry` (pub async fn) — sends an HTTP request with a fresh Bearer token from the OAuth provider; on 401 response, invalidates cached token, mints a new one, and retries exactly once; other status codes (403, 429, etc.) pass through unchanged

## File-level notes

- Comprehensive test coverage: 7 unit tests + 1 async integration test with wiremock verify parsing, validation, mutual exclusion rules, anti-exfiltration guard, and 401-retry logic
- Security posture is solid: Debug redaction prevents secret leakage via stray debug prints, anti-exfiltration guard blocks LLM from supplying base_url when auth is set, mutual exclusion prevents mixing OAuth with static bearer tokens
- Error messages are clear and actionable (e.g., "auth block missing required fields: <list>")
- Note: `send_with_oauth_retry` requires non-streaming request bodies (enforced via `try_clone()` check at line 120); this is documented in the error message and is an acceptable constraint for OAuth flows
- No FIXME/TODO/unimplemented markers; no dead code paths; all functions have clear callers (parse_oauth_auth is called during node initialization, send_with_oauth_retry wraps the HTTP send in the http_request node)
