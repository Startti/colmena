# src/libs/colmena/src/gdocs/domain/errors.rs

**Layer:** domain  **Purpose:** Defines the `DocsError` enum with 14 variants representing all recoverable errors the gdocs subsystem can surface. Each variant maps 1:1 to a JSON envelope the LLM sees via dispatcher serialization.

## Symbols

- `DocsError` (pub enum) — Umbrella error type; 14 variants cover auth (NotConfigured, AuthFailed, DocumentNotFound, PermissionDenied), content-addressing (AmbiguousMatch, TextNotFound, ConfirmManyMatches, ScopeCrossesBoundary), conflict detection (HumanChangesPending), tab operations (TabNotFound, TabExists, NoParentFolder), rate limiting (RateLimit, Conflict), HTTP failures (Http), and validation (InvalidArgs, Internal).
  - `NotConfigured(String)` — No SA JSON or ADC available; operator has not configured auth.
  - `AuthFailed(String)` — Token refresh failed after one retry.
  - `DocumentNotFound(String)` — 404 on documents.get; documentId is wrong or SA can't see it.
  - `TabNotFound(String)` — tab_id argument doesn't resolve to a tab in the document.
  - `PermissionDenied(String)` — 403; operator should share document/folder with the SA email.
  - `NoParentFolder` — create_* was called but no parent folder is configured (env var or per-call argument).
  - `AmbiguousMatch { find, matches }` — find returned multiple matches without occurrence/anchor to disambiguate.
  - `TextNotFound { find, fuzzy_suggestions }` — find returned zero matches; carries Levenshtein distance suggestions for LLM recovery.
  - `ConfirmManyMatches { find, count, preview }` — find returned ≥5 matches and confirm_many was not set; carries preview.
  - `HumanChangesPending { since, changes_overlapping_scope, changes_outside_scope }` — Human edits overlap intended agent scope; partitions changes by overlap.
  - `ScopeCrossesBoundary(String)` — find text would cross paragraph/table/structural boundary; v1 rejects these.
  - `TabExists(String)` — add_tab would create a tab whose title already exists and on_existing_tab policy is fail (default).
  - `RateLimit(u32)` — 429 from Google after dispatcher exhausted retry budget.
  - `Conflict` — writeControl.requiredRevisionId was rejected; doc changed concurrently with our write.
  - `Http(String)` — 5xx / network / generic HTTP failure.
  - `InvalidArgs(String)` — Caller passed bad input caught before network call (bad enum, malformed scope).
  - `Internal(String)` — Catch-all for assertion failures inside dispatcher and use cases.

- `display_strings_render` (test fn) — Validates Display impl for AmbiguousMatch and HumanChangesPending error messages.

- `errors_are_clone_and_send_sync` (test fn) — Verifies DocsError trait bounds (Clone, Send, Sync for async/serialization safety).

## File-level notes

- All 14 error variants are used as public API surface across the gdocs domain and application layers; no dead variants.
- Test at line 133 includes intentional unused-import canary (`HumanChangeKind::Insert`) with explicit comment — not a flag.
- Derives `Debug, Clone, Error` via thiserror; all Display messages map to stable JSON shapes in dispatcher.
- No todos, unfinished stubs, or simplification opportunities detected.
