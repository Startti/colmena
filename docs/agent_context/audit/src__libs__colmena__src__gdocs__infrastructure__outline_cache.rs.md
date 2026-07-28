# src/libs/colmena/src/gdocs/infrastructure/outline_cache.rs

**Layer:** infrastructure  **Purpose:** Per-(session, doc) in-memory cache of DocumentSnapshot with TTL. Absorbs tool-call bursts within a configurable window to avoid repeated Docs API `documents.get` calls.

## Symbols

- `Entry` (struct, private) — holds a DocumentSnapshot and fetched_at timestamp for TTL age validation
- `OutlineCache` (pub struct) — in-process cache keyed on (agent_session_id, DocumentId) with mutex-protected HashMap and configurable TTL
- `OutlineCache::new` (pub fn) — constructs an empty cache with the given TTL duration
- `OutlineCache::get_fresh` (pub fn) — returns cached snapshot if younger than TTL; None triggers fresh fetch
- `OutlineCache::put` (pub fn) — inserts or replaces cached snapshot with current timestamp; called after fetch or successful batch_update
- `OutlineCache::invalidate` (pub fn) — removes cached entry; called after writes to force fresh data on next read
- `fake_snapshot` (fn, test) — test helper that constructs a minimal DocumentSnapshot
- `put_and_get_within_ttl` (test) — verifies cache hit within TTL window
- `miss_when_session_or_doc_differs` (test) — verifies cache keys on session_id and doc_id independently
- `expires_after_ttl` (test) — verifies TTL expiration forces cache miss
- `invalidate_removes_entry` (test) — verifies explicit invalidation removes cached entry

## File-level notes

- No unused symbols; all public methods are integration points for the dispatcher.
- No incomplete work (no `todo!`, `unimplemented!`, or stub impls).
- Defensive `.expect("outline cache mutex poisoned")` on all lock acquisitions is appropriate for this shared-state pattern.
- Tests are comprehensive: hit/miss, TTL expiration, multi-key isolation, invalidation all covered.
- Code quality is high: straightforward caching logic with clear intent via doc comments.
