# src/libs/colmena/src/gdocs/application/co_edit_guard.rs

**Layer:** application  
**Purpose:** Co-edit safety pipeline for Google Docs. Runs before every edit to detect human-authored paragraph changes since the agent's last write and either blocks (if overlap with intended scope) or warns (if outside scope).

## Symbols

- `GuardOk` (struct, pub) — Result type returned when guard allows an edit to proceed; wraps fresh snapshot, resolved scope, and outside-scope human changes as informational soft-warnings.
- `GuardOk::snapshot` (field, pub) — Fresh document snapshot post-guard, cached or freshly fetched, for use-case consumption.
- `GuardOk::resolved_scope` (field, pub) — Concrete paragraph range (resolved from the requested scope against the snapshot), used for overlap detection.
- `GuardOk::soft_warnings` (field, pub) — Vec of human edits detected outside the requested scope, surfaced to the agent for awareness.
- `GuardContext<'a>` (struct, pub) — Dependency injection container for guard operations, carrying HTTP client, snapshot cache, revision store, session ID, and optional service-account email.
- `GuardContext::client` (field, pub) — Production HTTP client (DocsClient trait object) or mock in tests.
- `GuardContext::cache` (field, pub) — Per-(session, doc) snapshot cache with 5-second TTL.
- `GuardContext::revisions` (field, pub) — Postgres-backed revision store tracking (agent_session_id, doc_id) → last_revision_id mappings.
- `GuardContext::session_id` (field, pub) — Stable identifier of the current chat session (required for cache/revision keying).
- `GuardContext::sa_email` (field, pub) — Optional service-account email to identify and exclude self-authored revisions from human-change detection. [FLAG: unfinished — field declared with documented intent to filter self-writes, but never used in filtering logic of run_guard or run_guard_non_blocking.]
- `run_guard` (async fn, pub) → Result<GuardOk, DocsError> — Main co-edit safety pipeline: fetches snapshot (cache or fresh), resolves scope, reads prior revision+snapshot from store, diffs if drift detected, partitions changes by overlap, and errors with HumanChangesPending if overlap found or falls back to v1 conservative block if snapshot unavailable.
- `run_guard_non_blocking` (async fn, pub) → Result<GuardOk, DocsError> — Non-blocking variant for table-cell edits where scope cannot be expressed as a paragraph range; detects human changes since agent's cursor and returns all as soft-warnings (never blocks).
- `partition_by_scope` (fn, private) → (Vec<HumanChange>, Vec<HumanChange>) — Helper that partitions a list of human changes into overlapping-with-scope and outside-scope buckets using ResolvedScope::contains_paragraph.
- `guard_tests` (mod, test-gated cfg) — Comprehensive unit-test suite with 7 test cases covering first-contact, no-drift, revision-drift v1 fallback, cache hits, v1.1 drift+snapshot with overlap (blocks with details), drift outside scope (proceeds with soft-warnings), and non-blocking guard behavior.

## File-level notes

- The module correctly implements the v1.1 co-edit pipeline documented in `docs/superpowers/specs/2026-06-09-gdocs-paragraph-diff-design.md`.
- The `sa_email` field in `GuardContext` is prepared for filtering self-authored revisions but the filtering logic is not implemented; the pipeline currently treats all revisions conservatively as potential human changes. This is documented as the intended conservative fallback when `sa_email` is None, but even when Some, the field is unused.
- All test cases are isolated, use proper mocking (MockDocsClient, TestRig), and validate the three key paths: first-contact, no-drift, and drift with/without prior snapshot.
- The `paragraph_diff` and `scope_resolver` dependencies are correctly delegated to their respective modules.
- No dead-code or typo issues detected; error handling is complete (? propagation for client/store errors, explicit match on revision equality and prior-snapshot availability).
