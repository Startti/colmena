# src/libs/colmena/src/gdocs/infrastructure/config.rs

**Layer:** infrastructure  **Purpose:** Operator-facing configuration for the Google Docs subsystem, built from environment variables following the OAuth migration pattern (2026-06-10). Mirrors the GSheetsConfig design with auth credentials delegated to the shared `google_oauth` subsystem.

## Symbols

- `GDocsConfig` (struct, pub) — Operator-controlled runtime configuration for gdocs with OAuth scopes, parent folder, request timeout, max retries, revision cache TTL, and workspace share email
- `GDocsConfig::scopes` (field, pub) — OAuth scopes consented at colmena_oauth_setup; documentation-only at runtime as the Google token endpoint inherits whatever scopes the refresh_token was issued with
- `GDocsConfig::default_parent_folder` (field, pub) — Optional Drive folder ID that create_* calls drop new docs into; folder must be shared as Editor with the share_email
- `GDocsConfig::request_timeout` (field, pub) — Per-request HTTP timeout duration
- `GDocsConfig::max_retries` (field, pub) — Total retry count on 429 or 5xx errors before surfacing the error
- `GDocsConfig::revision_cache_ttl` (field, pub) — TTL for the in-memory snapshot cache used by the co-edit guard
- `GDocsConfig::share_email` (field, pub) — Workspace user email the agent acts as; read from COLMENA_GOOGLE_SHARE_EMAIL, empty string if unset
- `GDocsConfig::from_env` (fn, pub) — Build config from environment variables (COLMENA_GOOGLE_SHARE_EMAIL, COLMENA_GDOCS_SCOPES, COLMENA_GDOCS_DEFAULT_PARENT_FOLDER_ID, COLMENA_GDOCS_REVISION_CACHE_SECS) with hardcoded defaults (30s timeout, 3 retries, 5s cache TTL)
- `tests::default_struct_values` (fn, test) — Documents the default shape of GDocsConfig via assertions; note: does NOT call from_env() so values are manually hardcoded

## File-level notes

- **Inconsistency between `from_env()` defaults and test documentation:** The `from_env()` method defaults scopes to `["documents", "drive"]` (lines 60–62, justified by comment at lines 50–59 for realistic v1 flow where users share existing docs). However, the `default_struct_values` test hardcodes scopes as `["documents", "drive.file"]` (line 96). Since the test is meant to "document the default shape," this documents the wrong default and could mislead future developers. Either the test should match `from_env()` or be refactored to call `from_env()` to avoid drift.
- The config struct is well-structured with clear field documentation explaining the OAuth migration and scope trade-offs.
- No env-var safety concerns; all `std::env::var()` calls are defensive (.ok()) with sensible fallbacks.
