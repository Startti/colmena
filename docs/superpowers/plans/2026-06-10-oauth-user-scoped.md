# OAuth User-Scoped Auth — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the Service Account auth path in `gsheets` + `gdocs` with an OAuth user-scoped flow on `agents@startti.co`. Hard cutover (no dual mode). Activity logs and share dialogs will show `agents@startti.co` instead of the SA email.

**Architecture:** New shared module `src/libs/colmena/src/google_oauth/` for OAuth token plumbing. `gsheets/infrastructure/auth.rs` and `gdocs/infrastructure/auth.rs` swap their SA JWT path for OAuth bearer token retrieval. SA constructors retained under `#[cfg(test)]` so existing wiremock unit tests keep compiling.

**Tech stack:** Rust 1.95, `reqwest` (existing), `serde_json` (existing), `tokio::sync::Mutex` (existing), `chrono` (existing). New deps: `webbrowser = "1"` (only used by the CLI binary, ~150KB).

**Reference spec:** [`docs/superpowers/specs/2026-06-10-oauth-user-scoped-design.md`](../specs/2026-06-10-oauth-user-scoped-design.md)

---

## File Structure

### Create

| Path | Responsibility |
|---|---|
| `src/libs/colmena/src/google_oauth/mod.rs` | Module root; re-exports. |
| `src/libs/colmena/src/google_oauth/domain/mod.rs` | Domain index. |
| `src/libs/colmena/src/google_oauth/domain/types.rs` | `AccessToken`, `RefreshTokenSecret`, `CachedToken`. |
| `src/libs/colmena/src/google_oauth/domain/errors.rs` | `OAuthError` enum (RefreshTokenRevoked, ClientCredsInvalid, Transient, ConfigMissing). |
| `src/libs/colmena/src/google_oauth/domain/traits.rs` | `AuthTokenProvider` async trait. |
| `src/libs/colmena/src/google_oauth/infrastructure/mod.rs` | Infra index. |
| `src/libs/colmena/src/google_oauth/infrastructure/config.rs` | `OAuthCredentials::from_env()` — reads 3 env vars, returns all-missing list on failure. |
| `src/libs/colmena/src/google_oauth/infrastructure/refresh_client.rs` | `POST /token` with retry/backoff. |
| `src/libs/colmena/src/google_oauth/infrastructure/token_provider.rs` | `OAuthRefreshTokenProvider` implementing `AuthTokenProvider`. |
| `src/libs/colmena/src/bin/colmena_oauth_setup.rs` | CLI binary for the one-time consent flow. |
| `docs/developer_guide/47_google_oauth.md` | Dev guide section — setup, troubleshooting, runbook. |

### Modify

| Path | Change |
|---|---|
| `src/libs/colmena/Cargo.toml` | Add `webbrowser = "1"` (binary-only); declare new `[[bin]]`. |
| `src/libs/colmena/src/lib.rs` | `pub mod google_oauth;`. |
| `src/libs/colmena/src/gsheets/infrastructure/auth.rs` | Replace `TokenProvider` impl: production path uses `OAuthRefreshTokenProvider`. SA-JWT path becomes `#[cfg(test)]` `for_tests` constructor. |
| `src/libs/colmena/src/gsheets/infrastructure/config.rs` | Drop `credentials_path` field; add `share_email: String` from `COLMENA_GOOGLE_SHARE_EMAIL`. |
| `src/libs/colmena/src/gsheets/infrastructure/http_client.rs` | Drop `client_email` extraction (~10 lines around L46). `sa_email` field renames to `share_email` and reads from new config. |
| `src/libs/colmena/src/gdocs/infrastructure/auth.rs` | Same as gsheets. |
| `src/libs/colmena/src/gdocs/infrastructure/config.rs` | Same as gsheets. |
| `src/libs/colmena/src/gdocs/infrastructure/http_client.rs` | Same shape changes as gsheets. |
| `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/google_workspace_prelude.rs` | Extend `resolve_sa_email` (rename to `resolve_share_email`): honour `COLMENA_GOOGLE_SHARE_EMAIL` BEFORE the old SA chain. Old chain becomes the fallback for tests/local-dev. |
| `docs/CHANGELOG_2026-06.md` | Add §26 entry (after ship). |
| `CLAUDE.md` | Update §"SECURE_VALUES_KEY now required at startup" pattern with the new OAuth env-var requirements pattern for deploy_gcp.sh. |
| `docs/BACKLOG.md` | Mark "OAuth user-scoped flow" entry as SHIPPED. |

### Delete

No file deletions. SA constructors retained behind `#[cfg(test)]`.

---

## Phase 1 — google_oauth module skeleton (foundation)

- [ ] **T1.** `cargo new --lib`-equivalent: create `src/libs/colmena/src/google_oauth/` with `mod.rs`, `domain/`, `infrastructure/` directories.
- [ ] **T2.** Add `pub mod google_oauth;` to `src/libs/colmena/src/lib.rs`.
- [ ] **T3.** Create `domain/types.rs` with:
  - `AccessToken(String)` — newtype wrapping the bearer string.
  - `RefreshTokenSecret(String)` — Display impl redacts.
  - `CachedToken { access_token: AccessToken, expires_at: DateTime<Utc> }`.
- [ ] **T4.** Create `domain/errors.rs`:
  ```rust
  pub enum OAuthError {
      RefreshTokenRevoked,
      ClientCredsInvalid(String),
      Transient(String),
      ConfigMissing(Vec<String>),
  }
  ```
  Implement `thiserror::Error`.
- [ ] **T5.** Create `domain/traits.rs` with `AuthTokenProvider` async trait:
  ```rust
  #[async_trait::async_trait]
  pub trait AuthTokenProvider: Send + Sync {
      async fn get_bearer_token(&self) -> Result<AccessToken, OAuthError>;
  }
  ```
- [ ] **T6.** Confirm `cargo check` passes.

## Phase 2 — Config + refresh client (HTTP layer)

- [ ] **T7.** Create `infrastructure/config.rs` with `OAuthCredentials::from_env()`:
  - Reads `COLMENA_GOOGLE_OAUTH_CLIENT_ID`, `..._CLIENT_SECRET`, `..._REFRESH_TOKEN`.
  - Returns `Err(OAuthError::ConfigMissing(missing_names))` listing ALL missing vars.
- [ ] **T8.** Unit tests for `from_env()` using `std::env::set_var` with `#[serial_test::serial]`.
- [ ] **T9.** Create `infrastructure/refresh_client.rs` with `RefreshClient::refresh(&self, creds: &OAuthCredentials) -> Result<RefreshResponse, OAuthError>`:
  - POST to `https://oauth2.googleapis.com/token`.
  - Form-encoded body with `grant_type=refresh_token` + 3 credentials.
  - Parse JSON response into `RefreshResponse { access_token, expires_in, refresh_token: Option<String> }`.
  - Map errors per §5.3: 400 invalid_grant → `RefreshTokenRevoked`, 400 invalid_client → `ClientCredsInvalid`, 5xx/network → `Transient` with retry (1s/2s, max 2 attempts).
- [ ] **T10.** Wiremock unit tests for `RefreshClient`:
  - Happy path: 200 with access_token + expires_in.
  - 400 invalid_grant → `RefreshTokenRevoked`.
  - 400 invalid_client → `ClientCredsInvalid`.
  - 503 then 200 on retry → success after backoff.
  - 503 × 3 → `Transient`.
  - Response includes rotated `refresh_token` → returned in `RefreshResponse.refresh_token` (so the provider can log it).
- [ ] **T11.** `cargo test --lib google_oauth::infrastructure::refresh_client` green.

## Phase 3 — Token provider with cache

- [ ] **T12.** Create `infrastructure/token_provider.rs`:
  ```rust
  pub struct OAuthRefreshTokenProvider {
      creds: OAuthCredentials,
      refresh: RefreshClient,
      cache: tokio::sync::Mutex<Option<CachedToken>>,
  }
  ```
- [ ] **T13.** Implement `AuthTokenProvider`:
  - Lock the mutex.
  - If cache has token AND `Utc::now() < expires_at - Duration::seconds(60)` → clone + return.
  - Else refresh: call `RefreshClient::refresh`, build `CachedToken`, store, return.
  - On rotated `refresh_token`: log structured WARN `oauth.refresh_token_rotated` with `tracing::warn!`. Do NOT persist.
- [ ] **T14.** Unit tests:
  - First call refreshes, second within 1h returns cached.
  - Cache near-expiry triggers fresh refresh.
  - Concurrent calls (spawn 10 tasks) share one refresh due to mutex.
  - Rotated refresh_token logs warn (capture with `tracing-test`).
- [ ] **T15.** `cargo test --lib google_oauth` green.

## Phase 4 — Wire OAuth into gsheets

- [ ] **T16.** Read `src/libs/colmena/src/gsheets/infrastructure/auth.rs` end-to-end. Identify the current `TokenProvider` shape and how `GoogleSheetsHttpClient` calls it.
- [ ] **T17.** Refactor `auth.rs`:
  - The existing `TokenProvider` struct gets a new constructor `from_oauth_credentials(creds: OAuthCredentials)` that wraps `OAuthRefreshTokenProvider`.
  - The existing SA-JWT constructor becomes `#[cfg(test)] pub fn for_tests_with_static_token(token: &str) -> Self`.
  - All production callsites switch to `from_oauth_credentials`.
- [ ] **T18.** Update `gsheets/infrastructure/config.rs`:
  - Drop `credentials_path` from production reads.
  - Add `share_email: String` read from `COLMENA_GOOGLE_SHARE_EMAIL` env var.
  - `GSheetsConfig::from_env()` now ALSO calls `OAuthCredentials::from_env()` and stores it.
- [ ] **T19.** Update `gsheets/infrastructure/http_client.rs`:
  - Delete the `client_email` extraction block (~L43-56 of current file).
  - Rename `sa_email` field to `share_email`.
  - Constructor reads `share_email` directly from `GSheetsConfig`.
- [ ] **T20.** Update affected tests in `gsheets/`:
  - Wiremock tests that construct via `for_tests` keep working (no auth).
  - Integration tests need either `OAuthCredentials::for_tests()` helper that constructs from inline values, or `#[ignore]` gating.
- [ ] **T21.** `cargo check --lib` clean.
- [ ] **T22.** `cargo test --lib gsheets` green.

## Phase 5 — Wire OAuth into gdocs

- [ ] **T23.** Read `src/libs/colmena/src/gdocs/infrastructure/auth.rs` end-to-end.
- [ ] **T24.** Apply T17-equivalent refactor to gdocs `auth.rs`.
- [ ] **T25.** Apply T18/T19-equivalent changes to gdocs config + http_client.
- [ ] **T26.** Update affected gdocs tests.
- [ ] **T27.** `cargo check --lib` clean.
- [ ] **T28.** `cargo test --lib gdocs` green.

## Phase 6 — Update the workspace prelude

- [ ] **T29.** Read current `google_workspace_prelude.rs`. Plan: rename `resolve_sa_email` → `resolve_share_email`. New chain:
  1. `COLMENA_GOOGLE_SHARE_EMAIL` env var.
  2. (legacy/fallback for tests) `COLMENA_GOOGLE_SA_EMAIL` env var.
  3. (legacy/fallback for tests) `GOOGLE_APPLICATION_CREDENTIALS` JSON `client_email`.
  4. `None` → degraded prelude.
- [ ] **T30.** Update existing 8 prelude tests to cover the new env var:
  - `resolve_share_email_uses_new_env_var_first`.
  - `resolve_share_email_falls_back_to_legacy_sa_env_var`.
  - Existing tests for the SA JSON fallback keep passing.
- [ ] **T31.** Update callsite in `llm.rs` to use the renamed function.
- [ ] **T32.** `cargo test --lib google_workspace_prelude` green.

## Phase 7 — CLI binary `colmena_oauth_setup`

- [ ] **T33.** Add `webbrowser = "1"` to `Cargo.toml` (under `[dependencies]` since the binary lives in `src/bin/`).
- [ ] **T34.** Declare the binary in `Cargo.toml`:
  ```toml
  [[bin]]
  name = "colmena_oauth_setup"
  path = "src/bin/colmena_oauth_setup.rs"
  ```
- [ ] **T35.** Implement `src/bin/colmena_oauth_setup.rs` (~150 LOC):
  - CLI args: `--client-secret <path>` (required), `--port <u16>` (default 8080).
  - Read `client_secret.json`; extract `client_id` + `client_secret`.
  - Build authorization URL with `response_type=code`, scopes:
    - `https://www.googleapis.com/auth/drive.file`
    - `https://www.googleapis.com/auth/documents`
    - `https://www.googleapis.com/auth/spreadsheets`
    - `access_type=offline` (required to receive a refresh_token)
    - `prompt=consent` (force consent screen even on re-runs)
    - `redirect_uri=http://localhost:<port>`
  - Open browser with `webbrowser::open(url)`.
  - Spawn `axum` server on `localhost:<port>` with one route capturing `?code=...`.
  - On receipt: POST to `https://oauth2.googleapis.com/token` with `grant_type=authorization_code` + 4 fields.
  - On success: print refresh_token to stdout + brief HTML "you can close this tab" to the browser.
  - On error: print to stderr + exit code 1.
- [ ] **T36.** Smoke-test the binary locally against a throwaway OAuth client.
- [ ] **T37.** Document the binary in the dev guide.

## Phase 8 — Documentation

- [ ] **T38.** Create `docs/developer_guide/47_google_oauth.md`:
  - §1: Why OAuth (link spec).
  - §2: Setup steps (link to spec §7).
  - §3: Required env vars (link to spec §6).
  - §4: Runbook for refresh_token revocation (link to spec §8.4).
  - §5: Local dev (how to share creds across the team).
  - §6: Troubleshooting (the 4 error variants → operator action).
- [ ] **T39.** Add §47 link to `docs/DEVELOPER_GUIDE.md` index.
- [ ] **T40.** Update `CLAUDE.md` "Configuración requerida en operador" pattern under the gsheets/gdocs sections.
- [ ] **T41.** Mark BACKLOG "OAuth user-scoped flow" entry as SHIPPED with link to changelog §26.

## Phase 9 — Verification + ship

- [ ] **T42.** Local end-to-end smoke:
  - Set 3 OAuth env vars + `COLMENA_GOOGLE_SHARE_EMAIL` in shell.
  - Run a graph that uses `gsheets_list_sheets` against a doc shared with `agents@startti.co`.
  - Verify success.
  - Look at the doc's activity log → confirm "edited by agents@startti.co".
- [ ] **T43.** Run `cargo test --verbose` (full CI-equivalent). All green.
- [ ] **T44.** Run `cargo clippy --lib`. No new warnings.
- [ ] **T45.** Commit + push develop. Track for ADP cutover.
- [ ] **T46.** Append CHANGELOG_2026-06.md §26 with: scope chosen, storage chosen, breaking-change notice for ADP deploy_gcp.sh.

## Phase 10 — ADP deploy_gcp.sh changes (operator-driven, separate PR)

NOT executed by this plan — flagged for handoff. The operator (or a follow-up plan in the ADP repo) needs to:

- [ ] **T47.** (ADP) Add 3 secrets to Secret Manager: `colmena-oauth-client-id`, `colmena-oauth-client-secret`, `colmena-oauth-refresh-token`.
- [ ] **T48.** (ADP) IAM bind `roles/secretmanager.secretAccessor` to the worker SA on each of the 3 secrets.
- [ ] **T49.** (ADP) Update `apps/service/ia/platform/deploy_gcp.sh` to:
  - Add `--update-secrets` for the 3 OAuth secrets.
  - Add `--update-env-vars=COLMENA_GOOGLE_SHARE_EMAIL=agents@startti.co`.
  - Remove `--update-secrets=GOOGLE_APPLICATION_CREDENTIALS=...`.
  - Remove `--update-env-vars=COLMENA_GOOGLE_SA_EMAIL=...` if present.
- [ ] **T50.** (ADP) Deploy to dev, smoke test, deploy to prod (or wait for prod readiness).

---

## Risks & mitigations

| Risk | Mitigation |
|---|---|
| Refresh_token leaks during the consent flow on the operator's laptop | The CLI prints the token to stdout once. Operator immediately uploads to Secret Manager and clears terminal history. Documented in dev guide. |
| Boot-time failure when ADP deploys with new colmena but old env vars | `OAuthCredentials::from_env()` lists ALL missing vars in one error, so operator sees the full migration list immediately. |
| Tests break because they expect the SA path | All in-tree tests use `for_tests` constructors that bypass auth. We audit and update those that don't. |
| Google rotates refresh_tokens mid-deploy | The library logs `oauth.refresh_token_rotated` WARN events. Operator monitors. If a real rotation invalidates the old token, the runbook in §8.4 covers re-consent (~10 min). |
| Local dev becomes harder | The dev guide documents two paths: (1) shared team refresh_token for testing against a shared dev doc, (2) personal consent for testing against the dev's own Google account. Each dev picks. |
| The CLI binary depends on `webbrowser` which adds binary size | The crate is ~150KB and only pulled in for the binary target. The main library is unaffected. |

---

## Test plan

| Layer | Tests | Where |
|---|---|---|
| `OAuthCredentials::from_env()` | 4 unit tests (all-present, one-missing, all-missing, env-var-trimming) | `google_oauth::infrastructure::config::tests` |
| `RefreshClient` | 5 wiremock tests (happy, invalid_grant, invalid_client, retry-success, retry-exhausted) | `google_oauth::infrastructure::refresh_client::tests` |
| `OAuthRefreshTokenProvider` | 5 unit tests (first-call refreshes, cache hit, near-expiry refresh, concurrent share, rotation log) | `google_oauth::infrastructure::token_provider::tests` |
| `gsheets` auth integration | Existing wiremock tests stay green | `gsheets::infrastructure::http_client::tests` |
| `gdocs` auth integration | Existing wiremock tests stay green | `gdocs::infrastructure::http_client::tests` |
| Prelude | New env-var-precedence test + existing 8 keep passing | `google_workspace_prelude::tests` |
| End-to-end smoke (operator-run) | Run a graph against a real doc; verify activity log | T42 above |

---

## Completion criteria

- All Phase 1-9 checkboxes ticked.
- `cargo test --verbose` green.
- `cargo clippy --lib` clean.
- Spec §10 (open questions) still empty (no last-minute surprises).
- Operator has verified Phase 9 smoke test against `agents@startti.co`.
- Develop pushed.
- BACKLOG item marked SHIPPED.
- CHANGELOG §26 written.

Phase 10 (ADP deploy_gcp.sh) is the handoff. Once that ships in ADP, the cutover is complete.
