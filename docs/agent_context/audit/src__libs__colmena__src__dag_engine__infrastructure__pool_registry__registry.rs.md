# src/libs/colmena/src/dag_engine/infrastructure/pool_registry/registry.rs

**Layer:** infrastructure  
**Purpose:** Implements `PgPoolRegistry`, the centralized registry for managing Postgres connection pools with LRU eviction, pinning, and safe concurrent access. Enforces one-pool-per-URL invariant via per-URL creation locks.

## Symbols

- `PgPoolRegistry` (struct) — Thread-safe registry managing multiple Postgres pools with LRU eviction tracking, pinned set, last-used timestamps, and creation locks
- `impl PgPoolRegistry` — Implementation block
- `new(config: PoolConfig) -> Self` — Constructor initializing empty registry with 10× oversized LRU cache to prevent premature drops
- `build_pool_options(&self) -> PgPoolOptions` — Constructs sqlx pool options from registry config (min/max connections, timeouts)
- `hash_url(key: &UrlKey) -> String` — Private static method computing SHA256 digest of URL for log redaction (no raw URLs in traces)
- `pin(&self, url: &str) -> Result<Arc<PgPool>, RegistryError>` — Creates pool if absent, marks pinned (never LRU-evicted), enforces one-pool-per-URL via creation lock
- `try_cached(&self, url: &str) -> Option<Arc<PgPool>>` — Returns cached pool if exists, promotes unpinned entries in LRU, records cache hit metric
- `get_or_create(&self, url: &str) -> Result<Arc<PgPool>, RegistryError>` — Gets cached pool or creates new one, evicting LRU entry if non-pinned count ≥ max_entries
- `evict_if_needed(&self)` — Private async method evicting least-recently-used unpinned entry when non-pinned count exceeds max_entries; clears from pools/last_used/creation_locks
- `snapshot_metrics(&self) -> RegistryMetrics` — Collects per-URL pool metrics (size, idle, pinned status, last_used_at) and aggregate counters into immutable snapshot
- `close_all(&self)` — Idempotent graceful close: marks registry closed, awaits sqlx close on all pools, clears all tracking structures
- `insert_for_test(&self, url: &str, pool: Arc<PgPool>, pinned: bool)` — Test-only helper injecting mock pools with pinned flag (bypasses LRU eviction logic for bookkeeping tests)
- `tests` (mod) — Unit and integration test suite
- `fake_pool() -> Arc<PgPool>` — Test helper creating lazy-connect pool (never actually connects, safe for offline bookkeeping tests)
- `tiny_config(max_entries: usize) -> PoolConfig` — Test helper creating minimal config with given max_entries and defaults
- `pinned_url_is_in_pinned_set()` — Verifies inserted pinned pool appears in pinned set
- `cache_hit_for_pinned_pool_does_not_touch_lru()` — Confirms pinned pools bypass LRU even under repeated hits
- `cache_hit_for_unpinned_pool_promotes_lru()` — Confirms unpinned cache hits promote in LRU order
- `lru_evicts_when_over_capacity()` — Confirms LRU eviction triggers when non-pinned count exceeds max_entries
- `pinned_entries_are_exempt_from_eviction()` — Confirms pinned pools are never evicted despite LRU pressure
- `close_all_drains_every_entry()` — Confirms close_all clears pools, pinned, last_used, and creation_locks
- `metrics_snapshot_reflects_state()` — Confirms snapshot_metrics accurately counts entries and masks URLs in hashes
- `concurrent_get_or_create_produces_single_pool()` — Stress test with 50 concurrent try_cached calls verifies single Arc identity under high contention (no race on pools/lru/last_used)
- `close_all_then_get_or_create_returns_closed()` — Confirms get_or_create fails immediately after close_all without attempting connection

## File-level notes

- **Concurrency model**: Pools map (DashMap, lock-free reads), LRU cache (Mutex, guarded), per-URL creation locks (serializes slow path), closed flag (AtomicBool). All interactions safe under concurrent access.
- **One-pool-per-URL invariant**: Enforced by per-URL Mutex in creation_locks; double-check pattern re-verifies pool presence after lock acquired, preventing duplicate creation under concurrent load.
- **LRU overprovision**: LRU capacity set to 10× max_entries (min 1024) to prevent `lru.put()` from silently evicting before registry's own `evict_if_needed()` logic runs; allows registry full control over eviction order.
- **Pinning design**: Pinned pools bypass LRU entirely; last_used still tracked for metrics; useful for system-internal dbs that must remain available.
- **Cleanup on close**: `close_all()` sets closed flag first (gates new creation), then iterates snapshot of keys to avoid concurrent modification; idempotent (safe to call multiple times).
- **Creation lock cleanup**: Locks are removed after pool insertion to avoid unbounded map growth; necessary because DashMap::entry().or_insert_with() doesn't support remove-after-use patterns.
- **Test isolation**: fake_pool() uses lazy connect to avoid network I/O; all bookkeeping tests bypass actual sqlx pool lifecycle, making suite fast and offline-safe.
- No identified bugs, edge cases, or unfinished work; code is production-ready.
