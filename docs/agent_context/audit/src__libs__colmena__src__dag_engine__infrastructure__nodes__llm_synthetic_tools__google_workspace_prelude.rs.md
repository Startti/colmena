# src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/google_workspace_prelude.rs

**Layer:** infrastructure  
**Purpose:** Resolves Google Workspace share email from environment variables (OAuth or legacy SA flow) and builds auto-injected system-message prelude text that instructs LLM agents how to use Google Docs/Sheets tools correctly, including workflow guidance for sheets and table editing.

## Symbols

- `resolve_share_email()` (pub fn) — Resolves the Google Workspace share email via priority chain: OAuth var (`COLMENA_GOOGLE_SHARE_EMAIL`) → legacy SA var (`COLMENA_GOOGLE_SA_EMAIL`) → client_email from JSON at `GOOGLE_APPLICATION_CREDENTIALS` → None
- `resolve_sa_email()` (pub fn, deprecated) — Backward-compatible alias for `resolve_share_email()`; kept for external callers during OAuth migration
- `read_env_nonempty()` (private fn) — Reads an environment variable and returns `None` if unset or empty/whitespace-only
- `read_client_email_from_json()` (private fn) — Parses a JSON file (typically Google service account credentials) and extracts the `client_email` field
- `SHEET_WORKFLOW_PRELUDE` (const str) — Spanish-language prelude text explaining how to read sheet structure first before operating (handles non-standard headers, merged cells, column binding for `gsheets_run_python`)
- `TABLE_WORKFLOW_PRELUDE` (const str) — Spanish-language prelude text explaining how to read tables before editing via `gdocs_read_tables` and `gdocs_set_table_cell`, including merged-cell master-only rule and last-row/column deletion constraints
- `build_google_workspace_prelude()` (pub fn) — Constructs the full system-message prelude string; when `sa_email` is `Some`, embeds the email with two explicit reminders (share + doc ID); when `None`, provides degraded variant telling user to consult operator for email; always appends sheet and table workflow guidance
- `has_google_workspace_tools()` (pub fn, generic) — Returns true if the tool name iterator contains any name starting with `gsheets_` or `gdocs_` (underscore required, prefix exact)

### Test module (lines 219–493)
- `prelude_with_email_includes_address_and_share_instruction()` — Pins that email-variant prelude contains address, Editor keyword, and doc-ID mention; no operator-degraded fallback text
- `prelude_prefers_share_over_create_in_both_variants()` — Regression test: both variants (with/without email) must mention sharing existing docs as default over creating new ones; names the create tools
- `prelude_includes_understand_sheet_workflow_in_both_variants()` — Regression test: both variants must reference `gsheets_read`, warn that header is not always row 1, name `gsheets_run_python` for code-based comparison, and document merged-cell behavior
- `prelude_with_email_repeats_email_for_mandatory_first_turn_instructions()` — Regression pin: email must appear ≥2 times (intentional duplication to prevent low-temperature models from compressing the share instruction); asserts "DOS COSAS" anti-compression language present
- `prelude_without_email_falls_back_gracefully()` — Email-free variant contains operator-directed language and no `@` symbols
- `prelude_includes_table_workflow_in_both_variants()` — Regression test: both variants must reference `gdocs_read_tables`, name `gdocs_set_table_cell`, explain master-only merged-cell rule, and warn that last row/column cannot be deleted
- `has_google_workspace_tools_detects_gsheets_prefix()` — Prefix detection covers `gsheets_` and `gdocs_` with underscore; rejects `gsheetsraw` (no underscore); handles empty list
- `resolve_sa_email_env_var_wins_over_json()` — `COLMENA_GOOGLE_SA_EMAIL` precedence test vs JSON fallback; uses `#[serial]` to isolate env mutations
- `resolve_share_email_oauth_var_wins_over_everything()` — OAuth canonical var (`COLMENA_GOOGLE_SHARE_EMAIL`) must win when all three sources are set; uses `#[serial]`
- `resolve_share_email_falls_back_to_legacy_sa_var()` — Backward-compat pin: legacy SA env var must still resolve when OAuth var is unset; uses `#[serial]`
- `resolve_share_email_treats_empty_share_email_as_unset()` — Whitespace-only env values must NOT win; fallback chain continues; uses `#[serial]`
- `read_client_email_from_json_returns_field()` — Extracts `client_email` from valid service account JSON
- `read_client_email_from_json_missing_field_yields_none()` — JSON without `client_email` field returns None
- `read_client_email_from_json_invalid_json_yields_none()` — Malformed JSON returns None gracefully
- `read_client_email_from_json_missing_file_yields_none()` — Missing file path returns None (no panic)

## File-level notes

- **Module-level documentation (lines 1–29):** Clearly explains two responsibilities: (1) demand doc ID from LLM before operating; (2) surface the share email so LLM can instruct user to grant access. Explicitly documents the OAuth migration (2026-06-10) and the resolution priority chain.
- **Intentional repetition in prelude:** Lines 95–99 document why the email address appears twice in the email-bearing variant — observed behavior in low-temperature GPT-4o-mini that would compress the two bullets into a single "give me the ID" reply, dropping the critical share instruction. The test at line 303 pins this regression.
- **Spanish language throughout:** All prelude text is Spanish (per project convention). Prelude guidance is domain-specific: sheets require understanding structure before reading; tables require reading coordinates first before editing.
- **Product requirement (2026-06-11):** Prelude must steer LLMs toward preferring sharing existing docs over creating new ones, because created docs live in the agent account, not the user's Drive.
- **Comprehensive test coverage:** 17 tests cover all branches (with/without email, env-var precedence, JSON parsing, prefix detection, edge cases like empty strings, whitespace, missing files). Tests using `#[serial]` correctly isolate env-var mutations to prevent race conditions.
- **No dead code, stubs, or TODOs:** Implementation is complete. The deprecation of `resolve_sa_email()` is intentional and properly marked.
- **Error handling:** All I/O operations (env var read, file read, JSON parse) use `ok()?` chaining with `Option<T>` — clean error handling matching Rust idioms.
