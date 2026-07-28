# src/libs/colmena/src/google_oauth/domain/errors.rs

**Layer:** domain  **Purpose:** Defines coarse-grained domain errors for the OAuth subsystem, mapping variants to operator runbook actions (revoked tokens, invalid creds, transient failures, missing config).

## Symbols

- `OAuthError` (pub enum) — Domain error type for OAuth subsystem with 4 variants, derives Debug/Error/PartialEq/Eq
- `OAuthError::RefreshTokenRevoked` (variant) — Refresh token revoked by user at Google account or rotated by Google; operator must re-run colmena_oauth_setup and update Secret Manager
- `OAuthError::ClientCredsInvalid(String)` (variant) — Client credentials (client_id or client_secret) are invalid, usually from misconfigured env vars or stale Secret Manager value
- `OAuthError::Transient(String)` (variant) — Transient network/5xx/timeout failure after internal retries exhausted; callers may retry on next request with fresh refresh
- `OAuthError::ConfigMissing(Vec<String>)` (variant) — One or more required env vars missing or empty at initialization; carries full list of missing variable names so operator sees all at once

## File-level notes

- Excellent documentation: each variant includes actionable remediation steps and references the OAuth runbook (docs/superpowers/specs/2026-06-10-oauth-user-scoped-design.md §8.4)
- Proper layer boundary: zero infrastructure dependencies; uses `thiserror` as appropriate for domain errors
- Test coverage: three tests verify error message content (runbook direction, variable listing, PartialEq behavior) — supports the test-assertion pattern shown in callers
- PartialEq/Eq derives are necessary and documented inline (test comment notes they are used heavily for `assert_eq!` patterns)
- No unused variants or unclear semantics; all four cases map cleanly to concrete operator actions per design
