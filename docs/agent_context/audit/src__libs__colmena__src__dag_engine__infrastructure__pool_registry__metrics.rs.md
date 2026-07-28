# src/libs/colmena/src/dag_engine/infrastructure/pool_registry/metrics.rs

**Layer:** infrastructure  **Purpose:** Provides observability metrics for the pool registry with atomic counters tracking pool operations (get_or_create, pin, cache hits, evictions) and serializable snapshot structures for JSON export in debugging endpoints.

## Symbols

- `RegistryMetricsInner` (struct, pub(crate)) — Inner holder of atomic counters for pool registry events
- `RegistryMetricsInner::get_or_create_total` (field, pub) — AtomicU64 counter for total get_or_create operations
- `RegistryMetricsInner::pin_total` (field, pub) — AtomicU64 counter for total pin operations
- `RegistryMetricsInner::cache_hits_total` (field, pub) — AtomicU64 counter for total cache hits
- `RegistryMetricsInner::evictions_total` (field, pub) — AtomicU64 counter for total evictions
- `RegistryMetricsInner::record_get_or_create` (fn, pub) — Increments get_or_create_total counter by 1 with Relaxed ordering
- `RegistryMetricsInner::record_pin` (fn, pub) — Increments pin_total counter by 1 with Relaxed ordering
- `RegistryMetricsInner::record_cache_hit` (fn, pub) — Increments cache_hits_total counter by 1 with Relaxed ordering
- `RegistryMetricsInner::record_eviction` (fn, pub) — Increments evictions_total counter by 1 with Relaxed ordering
- `RegistryMetrics` (struct, pub) — Serializable snapshot of pool registry metrics exported via ColmenaEngine::registry_metrics()
- `RegistryMetrics::cached_pools` (field, pub) — Number of pools currently cached
- `RegistryMetrics::pinned_pools` (field, pub) — Number of pools currently pinned
- `RegistryMetrics::evictions_total` (field, pub) — Cumulative eviction count
- `RegistryMetrics::get_or_create_total` (field, pub) — Cumulative get_or_create call count
- `RegistryMetrics::pin_total` (field, pub) — Cumulative pin call count
- `RegistryMetrics::cache_hits_total` (field, pub) — Cumulative cache hit count
- `RegistryMetrics::per_url` (field, pub) — Per-URL pool metrics breakdown
- `PoolMetrics` (struct, pub) — Metrics snapshot for an individual pool
- `PoolMetrics::url_hash` (field, pub) — SHA-256 hex hash of normalized URL (redacts credentials for security)
- `PoolMetrics::size` (field, pub) — Current connection count in pool
- `PoolMetrics::idle` (field, pub) — Number of idle connections available in pool
- `PoolMetrics::pinned` (field, pub) — Whether this pool is pinned (protected from eviction)
- `PoolMetrics::last_used_at` (field, pub) — Last access timestamp (skipped from JSON serialization)
- `tests::counters_increment` (test, private) — Verifies atomic counter methods increment correctly
- `tests::snapshot_serializes_to_json` (test, private) — Verifies RegistryMetrics serializes to valid JSON with expected field values

## File-level notes

- **Atomic ordering discipline:** All counter increments use `Ordering::Relaxed` (appropriate for event counters with no synchronization requirements)
- **Security consideration:** `url_hash` explicitly uses SHA-256 to avoid exposing raw credentials in snapshots
- **Serialization strategy:** `SystemTime` in `PoolMetrics::last_used_at` is skipped from JSON to avoid custom serializer boilerplate; value remains available for internal eviction logic
- **Test coverage:** Both core functionalities (counter increment and JSON export) are tested; counters are tested via `load(Ordering::Relaxed)` to match increment behavior
- **No dependency on external types:** Uses only `serde`, `std::sync::atomic`, and `std::time`; zero infrastructure complexity
