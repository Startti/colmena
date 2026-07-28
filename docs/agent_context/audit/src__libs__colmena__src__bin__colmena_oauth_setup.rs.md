# src/libs/colmena/src/bin/colmena_oauth_setup.rs

**Layer:** infrastructure  **Purpose:** One-time CLI tool for operators to obtain OAuth refresh_token credentials for production Google API access (Docs, Sheets, Drive). Spins up a local redirect server, opens the consent flow in the user's browser, and extracts the refresh_token after authorization.

## Symbols

- `OAUTH_AUTHORIZE_URL` (const, private) — Hard-coded URL for Google's OAuth 2.0 authorization endpoint
- `OAUTH_TOKEN_URL` (const, private) — Hard-coded URL for Google's OAuth token exchange endpoint
- `REQUESTED_SCOPES` (const, private) — Array of three scopes: drive.file, documents (Docs API), spreadsheets (Sheets API)
- `Args` (struct, private) — CLI argument parser with `client_secret_path` (required) and `port` (default 8080)
- `ClientSecretFile` (struct, private) — Serde deserializer for downloaded OAuth client secret JSON with optional `installed` or `web` blocks
- `ClientSecretFields` (struct, private) — Extracted credentials: `client_id` and `client_secret`
- `ClientSecretFile::into_fields` (fn, private) — Converts ClientSecretFile to ClientSecretFields, choosing `installed` block first, then `web`, errors if both absent
- `AuthRedirectQuery` (struct, private) — Serde deserializer for Google's OAuth redirect query params: `code`, `error`, `error_description`
- `AppState` (struct, private) — Arc-wrapped state shared with axum handler; holds Mutex-wrapped oneshot sender for auth-code signaling
- `main` (async fn, private) — Entry point: parses CLI args, calls `run()`, prints refresh_token with instructions or error message
- `run` (async fn, private) — Orchestrates the flow: loads credentials, builds auth URL, spawns axum server, opens browser, waits for redirect, exchanges code for refresh_token
- `build_auth_url` (fn, private) — Constructs Google authorization URL with response_type, client_id, redirect_uri, scopes, access_type=offline, prompt=consent, with proper URL encoding
- `handle_redirect` (async fn, private) — Axum handler that receives Google's redirect; extracts auth code or error from query params and sends via oneshot channel; returns success or failure HTML
- `SUCCESS_HTML` (const, private) — Static HTML template shown in browser upon successful consent
- `FAILURE_HTML` (const, private) — Static HTML template shown in browser upon consent denial or error
- `TokenResponse` (struct, private) — Serde deserializer for Google's token exchange POST response with `access_token`, `refresh_token`, `error`, `error_description`
- `exchange_code_for_refresh_token` (async fn, private) — POSTs auth code to Google's token endpoint; parses and validates response; returns refresh_token or error message
- `tests` (mod, private) — Test module under `#[cfg(test)]`
- `tests::auth_url_contains_required_params` (fn, private) — Unit test verifying auth URL contains required parameters and proper URL encoding
- `tests::parses_installed_client_secret_shape` (fn, private) — Unit test for parsing `installed` block from client secret JSON
- `tests::parses_web_client_secret_shape` (fn, private) — Unit test for parsing `web` block from client secret JSON
- `tests::rejects_client_secret_with_neither_block` (fn, private) — Unit test verifying error when JSON has neither `installed` nor `web` block

## File-level notes

- Well-structured single-purpose CLI tool with clear error handling and user-friendly output formatting.
- Proper use of Rust async/await with tokio, axum for the HTTP server, and oneshot channels for signaling.
- All significant symbols are called or tested; no dead code detected.
- No todo!(), unimplemented!(), or FIXME comments; code appears production-ready.
- URL encoding correctly applied via `urlencoding` crate.
- Scopes are hard-coded as documented in the codebase (drive.file, documents, spreadsheets); operators can edit if needed.
- Test coverage includes both JSON parsing variants and URL construction.
