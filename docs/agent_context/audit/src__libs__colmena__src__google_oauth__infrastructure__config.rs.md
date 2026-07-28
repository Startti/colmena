# src/libs/colmena/src/google_oauth/infrastructure/config.rs

**Layer:** infrastructure  **Purpose:** Loads OAuth credentials (client_id, client_secret, refresh_token) from environment variables, with comprehensive error reporting and whitespace tolerance for Cloud Run secret mounts.

## Symbols

- `OAuthCredentials` (struct, pub) — Holds the three-tuple of credentials needed to mint access tokens; derives Debug and Clone for cheap passing to stateless clients.
- `OAuthCredentials::from_env()` (fn, pub) — Reads credentials from `COLMENA_GOOGLE_OAUTH_*` env vars, collecting ALL missing/empty vars in one error for complete operator visibility during migration.
- `OAuthCredentials::new()` (fn, pub) — Direct constructor from strings used by http_request node's native OAuth (where creds come from graph config, not env).
- `OAuthCredentials::for_tests()` (fn, pub, test-only) — Wiremock-friendly test constructor bypassing env reads.
- `read_env()` (fn, private) — Helper that reads an env var, trims it, and returns None if empty or whitespace-only (catches Cloud Run secret mount misconfigs).
- `tests` (mod, test-only) — Comprehensive test suite verifying env-var collection, empty/whitespace handling, trimming, and all-vars-missing reporting via serial_test locks.

## File-level notes

- All expect() calls justified by inline comments proving the code path (missing-var check above) guarantees Some.
- Test functions properly isolated with clear_all() and serial markers to prevent process-env races.
- Comprehensive design: treats empty and whitespace-only vars identically (both fail-closed), trims leading/trailing whitespace, and lists all missing vars in a single error instead of stopping at the first — this "list every missing" contract is explicitly documented and improves operator UX during migration.
- No infrastructure dependencies beyond domain error type; clean separation of concerns.
- No flags: code is well-documented, tested, and contains no dead code, TODOs, or unfinished patterns.
