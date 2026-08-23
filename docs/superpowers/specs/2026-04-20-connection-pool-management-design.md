# Design: Connection Pool Management (ColmenaEngine + PgPoolRegistry)

**Status:** Approved for planning
**Date:** 2026-04-20
**Author:** Daniel Garcia (brainstormed with Claude)
**Target component:** `colmena_dag_engine` lib + `adp/apps/service/ia/platform/worker/`

## Summary

Eliminate per-request Postgres pool creation in the Colmena DAG engine by introducing a single **`ColmenaEngine`** constructed once per process that owns a shared **`PgPoolRegistry`**. The registry is the sole source of truth for every Postgres pool: the Startti-internal DB (always one URL, pinned at startup) and every user-supplied `connection_url` (cached with LRU eviction). All current consumers — `PostgresDagStateRepository`, `PostgresSecureValueRepository`, `ConversationRepositoryFactory`, and the SQL node's `PgPoolAdapter` — are refactored to request pools by URL from the registry instead of opening their own.

## Motivation

The production worker at ``adp/apps/service/ia/platform/worker/src/main.rs`` (repo ADP) currently creates a fresh `sqlx::PgPool::connect(&db_url)` inside `process_job` (line 166) for every Redis job it processes, plus a new `ConversationRepositoryFactory::new()` (line 167) whose internal cache starts empty on every call. The `colmena_dag_engine` library itself replicates the pattern in [`api.rs`](../../../src/libs/colmena/src/dag_engine/api.rs) (lines 29 and 320) and [`main.rs`](../../../src/libs/colmena/src/dag_engine/main.rs) (line 73). The SQL node's [`PgPoolAdapter`](../../../src/libs/colmena/src/dag_engine/infrastructure/sql_pool_adapter.rs) owns its own `max_connections=5` pool per instance.

The observed effect in Cloud Run is Postgres connection-pool saturation: with sqlx's default `max_connections=10` and multiple concurrent jobs per instance across N Cloud Run replicas, the internal Cloud SQL instance sees hundreds of short-lived connections. The problem is worst when a single graph has both an LLM memory node and a SQL node, each opening independent pools against the same database.

## Goals

- One `PgPool` per unique `connection_url` per process, reused across all jobs and all consumers.
- Internal DB (`DATABASE_URL`) is pinned at startup: state repo, secure-value repo, LLM memory (if pointing to it), and any SQL node targeting it share one `Arc<PgPool>`.
- Bounded memory: LRU cap on the number of cached user-DB URLs per process.
- Automatic idle-connection reclamation via sqlx timeouts (`min_connections=0` + `idle_timeout`) so an idle cached pool holds zero TCP connections to the actual DB.
- Explicit async shutdown on SIGTERM so Cloud Run can drain pools before container kill.
- Tunable via env vars with sensible defaults — zero new required config for day-one deploy.
- Preserved contract: HTTP/Redis interfaces unchanged, SSE events unchanged, DB schemas unchanged.

## Non-goals

- **External connection pooler (Cloud SQL Managed Connection Pooling, PgBouncer, Supavisor)** — deferred. With `max_conn_per_url=2` and 10 Cloud Run instances, the internal Postgres sees ~20 connections. Phase 1 makes MCP unnecessary at current scale; see "Phase 2 triggers" below.
- **TTL-based eviction of idle pools** — deferred. Under `min_connections=0` an idle cached pool holds zero TCP connections; only memory (small). LRU alone bounds memory. Add TTL later if memory creep is observed.
- **Cross-instance pool sharing** (pool broker service) — overkill for current scale.
- **Centralized metrics backend integration** (Prometheus, OpenTelemetry) — out of scope; we expose metrics via a debug HTTP endpoint + tracing logs only.
- **Retrofitting Python bindings** — the `workflow_execution/` Python service was deleted; only the Rust worker and CLI remain.
- **Changing the DAG node contract** — nodes keep pulling pools via factories; no new trait methods exposed to user-written nodes.

## Architecture

### The single-registry principle

Both internal and user DBs share one `PgPoolRegistry`. The only difference is a **pinned flag**: the internal URL is marked pinned at startup, which makes it immune to LRU eviction and exempts it from the `max_entries` cap. Any JSON that happens to reference the internal URL in a `connection_url` field hits the same pinned pool automatically — no duplicate pool is opened.

### Component overview

```
Worker process (platform/worker/src/main.rs)
─────────────────────────────────────────────
 main() at boot:
   let engine = ColmenaEngine::new(EngineConfig::from_env()).await?;
   Router::new().route(...).with_state(AppState { engine, redis })

 process_job(job, state):
   state.engine.execute_stream(graph, session_id, ...)
     → yields DagExecutionEvents → worker XADDs to Redis stream

 On SIGTERM:
   axum graceful shutdown → engine.shutdown().await → registry.close_all()
```

```
ColmenaEngine (colmena_dag_engine::engine)
──────────────────────────────────────────
 registry: Arc<PgPoolRegistry>
 node_registry: Arc<HashMapNodeRegistry>  ← existing
 use_case: Arc<DagRunUseCase>              ← existing
 closed: AtomicBool

 At construction:
   1. Build registry with PoolConfig.
   2. registry.pin(internal_url) → gets Arc<PgPool>.
   3. PostgresDagStateRepository::new(internal_pool.clone())
      PostgresSecureValueRepository::new(internal_pool.clone())
      (plus migrate() for both)
   4. ConversationRepositoryFactory::new(registry.clone())
   5. SqlPortFactory::new(registry.clone())
   6. HashMapNodeRegistry::new_with_secure_values(
        repo_factory, sql_port_factory, state_repo, secure_value_service)
   7. DagRunUseCase::with_secure_values_and_service(...)
   8. Wire subgraph executor (registry.set_subgraph_executor).

 Public API:
   run_dag(...), execute_stream(...), shutdown()
```

```
PgPoolRegistry (infrastructure::pool_registry)
──────────────────────────────────────────────
 pools:   DashMap<UrlKey, Arc<PgPool>>
 lru:     Mutex<LruCache<UrlKey, ()>>
 pinned:  DashSet<UrlKey>
 config:  PoolConfig
 metrics: atomic counters (get_or_create, hits, evictions)

 get_or_create(url):
   let key = UrlKey::normalize(url)
   if let Some(pool) = pools.get(&key) {
       if not pinned: lru.promote(&key)
       cache_hits += 1
       return Ok(pool.clone())
   }
   if cached_pools >= max_entries: evict_lru()
   let pool = PgPoolOptions::new()
       .min_connections(config.min_conn_per_url)
       .max_connections(config.max_conn_per_url)
       .idle_timeout(config.idle_timeout)
       .max_lifetime(config.max_lifetime)
       .acquire_timeout(config.acquire_timeout)
       .connect(url).await?;
   pools.insert(key.clone(), Arc::new(pool));
   lru.put(key, ())
   Ok(pool)

 pin(url):
   create pool as above, insert, mark pinned, NOT inserted into LRU.
   Returns Arc<PgPool>.

 close_all():
   for (_, pool) in pools.drain():
     pool.close().await   // sqlx native; waits for in-flight queries
```

### How a pool reaches a DAG node

Nodes never see the registry directly. They see **factories** that the engine builds at startup:

| Call site | Who asks | Factory | Under the hood |
|---|---|---|---|
| State persistence (suspend/resume) | `DagRunUseCase` | n/a — `state_repo` holds pinned `Arc<PgPool>` | direct reference to pinned pool |
| Secure-value read/write | `SecureValueService` | n/a — `secure_value_repo` holds pinned `Arc<PgPool>` | direct reference to pinned pool |
| LLM `llm_call` with memory | the node's `execute()` | `ConversationRepositoryFactory.get_repository(url)` | resolves URL → `registry.get_or_create(url)` |
| SQL node (`nodes/sql.rs`) | the node's `execute()` | `SqlPortFactory.get_port(url, timeout_ms, work_mem_mb)` | resolves URL → `registry.get_or_create(url)`; `PgPoolAdapter` wraps the borrowed pool with per-query config |

URL resolution (`${DATABASE_URL}`, `secure_value://...`, `$DYNAMIC`) is performed by the existing variable resolver **before** the URL hits the factory. The registry only ever sees fully-resolved, normalized URLs.

### URL normalization

`UrlKey::normalize(url)` applies conservative transforms to avoid accidental cache misses:

- Lowercase scheme and host.
- Strip trailing slash on path.
- Preserve query parameters as-is (they can change behavior — `sslmode`, `options`, etc.).
- Do **not** strip credentials; two URLs with the same host/db but different users are distinct pools by design.

The key stored internally is the normalized `String`. External callers never see it.

## Components

### PoolConfig

```rust
pub struct PoolConfig {
    pub max_entries: usize,
    pub max_conn_per_url: u32,
    pub min_conn_per_url: u32,
    pub idle_timeout: Duration,
    pub max_lifetime: Duration,
    pub acquire_timeout: Duration,
}

impl PoolConfig {
    pub fn defaults() -> Self { /* values in §Config below */ }
    pub fn from_env() -> Result<Self, ConfigError>;
}
```

### EngineConfig

```rust
pub struct EngineConfig {
    pub internal_database_url: String,
    pub pool_config: PoolConfig,
}
impl EngineConfig { pub fn from_env() -> Result<Self, ConfigError>; }
```

### ColmenaEngine

```rust
pub struct ColmenaEngine {
    registry: Arc<PgPoolRegistry>,
    use_case: Arc<DagRunUseCase>,
    // state_repo, secure_value_service, node_registry, and the factories are
    // owned transitively by use_case / node_registry — no direct field needed.
    closed: AtomicBool,
}

impl ColmenaEngine {
    pub async fn new(config: EngineConfig) -> Result<Self, EngineError>;

    pub async fn run_dag(
        &self,
        graph: Graph,
        resume_session_id: Option<String>,
        resume_answer: Option<String>,
        include_extra_info: bool,
    ) -> Result<Value, EngineError>;

    pub fn execute_stream(
        &self,
        graph: Graph,
        resume_session_id: Option<String>,
        resume_answer: Option<String>,
        include_extra_info: bool,
    ) -> impl Stream<Item = Result<DagExecutionEvent, DagError>> + Send + '_;

    pub async fn shutdown(&self);          // idempotent via AtomicBool

    pub fn registry_metrics(&self) -> RegistryMetrics;  // for /debug/pools
}

impl Drop for ColmenaEngine {
    fn drop(&mut self) {
        if !self.closed.load(Ordering::SeqCst) {
            tracing::warn!("ColmenaEngine dropped without shutdown() — pools may leak");
        }
    }
}
```

### PgPoolRegistry

```rust
pub struct PgPoolRegistry {
    pools: DashMap<UrlKey, Arc<PgPool>>,
    lru: Mutex<LruCache<UrlKey, ()>>,
    pinned: DashSet<UrlKey>,
    config: PoolConfig,
    metrics: RegistryMetricsInner,  // AtomicU64 counters
}

impl PgPoolRegistry {
    pub fn new(config: PoolConfig) -> Self;
    pub async fn get_or_create(&self, url: &str) -> Result<Arc<PgPool>, RegistryError>;
    pub async fn pin(&self, url: &str) -> Result<Arc<PgPool>, RegistryError>;
    pub fn metrics(&self) -> RegistryMetrics;
    pub async fn close_all(&self);
}
```

### SqlPortFactory

```rust
pub struct SqlPortFactory {
    registry: Arc<PgPoolRegistry>,
}
impl SqlPortFactory {
    pub fn new(registry: Arc<PgPoolRegistry>) -> Self;
    pub async fn get_port(
        &self,
        url: &str,
        statement_timeout_ms: u64,
        work_mem_mb: u64,
    ) -> Result<Arc<dyn SqlConnectionPort>, SqlNodeError>;
}
```

Internally, `get_port` calls `registry.get_or_create(url)` and wraps the `Arc<PgPool>` in a `PgPoolAdapter` that holds the per-query config. Multiple adapters can share a pool; `SET LOCAL statement_timeout` + `SET LOCAL work_mem` applied inside each transaction keep the configs isolated per query.

### Refactored consumers

- [`ConversationRepositoryFactory`](../../../src/libs/colmena/src/llm/infrastructure/persistence/repository_factory.rs) — constructor changes from `new()` to `new(registry: Arc<PgPoolRegistry>)`; `get_repository(url)` no longer calls `PgPoolOptions::new()`; instead calls `registry.get_or_create(url)`. The inner `HashMap<Url, Arc<dyn ConversationRepository>>` stays — it caches the repo *wrapper*, not the pool.
- [`PgPoolAdapter`](../../../src/libs/colmena/src/dag_engine/infrastructure/sql_pool_adapter.rs) — no longer owns a pool. Constructor becomes `new(pool: Arc<PgPool>, statement_timeout_ms, work_mem_mb)`. `connect()` method on the `SqlConnectionPort` trait is removed (the pool is always present). The RLS helper methods still take `&self` and use the injected pool.
- [`nodes/sql.rs`](../../../src/libs/colmena/src/dag_engine/infrastructure/nodes/sql.rs) — the `HashMapNodeRegistry` (already passed to every `ExecutableNode::execute` via the existing execution context) gains a `sql_port_factory: Arc<SqlPortFactory>` field alongside the existing `conversation_factory`. The node reads this factory from the context and calls `get_port(url, timeout_ms, work_mem_mb)`. The node never constructs a `PgPoolAdapter` directly.
- [`api.rs`](../../../src/libs/colmena/src/dag_engine/api.rs) (`run_dag`, `serve_dag`) — refactored to instantiate `ColmenaEngine::new()` once and reuse it. For `run_dag` (used by CLI single-shot), the engine is created, used once, then `shutdown().await` is called before return.
- [`main.rs`](../../../src/libs/colmena/src/dag_engine/main.rs) (CLI) — same pattern as `api.rs`.
- ``platform/worker/src/main.rs`` (repo ADP) — `main()` builds the engine, passes it in `AppState`; `process_job` receives `&Engine` and calls `engine.execute_stream(...)`. Lines 164-189 (pool + repos + registry + use_case construction) are deleted from `process_job`.

## Shutdown semantics

Rust's `Drop` is synchronous and cannot `.await`. The engine exposes an **explicit async `shutdown()`** that:

1. Swaps a `closed: AtomicBool` to prevent double-close.
2. Calls `registry.close_all().await`, which iterates the `DashMap`, calls `pool.close().await` on each (sqlx waits for in-flight queries to drain).
3. Logs `engine_shutdown{pools_closed, duration_ms}`.

The worker's `main()` awaits graceful HTTP shutdown first, then calls `engine.shutdown().await` before returning. A defensive `Drop` logs a warning if shutdown was skipped, but does not attempt any async work.

Cloud Run grants a 10s grace period after SIGTERM. `pool.close()` in sqlx typically completes in sub-second even with several connections. If profiling shows the close phase approaching the grace window, parallelize with `futures::future::join_all` (noted as a follow-up optimization, not baseline).

## Config

All env vars are optional except `DATABASE_URL` and `REDIS_URL` (which already exist).

| Env var | Default | Valid range | Purpose |
|---|---|---|---|
| `COLMENA_POOL_MAX_ENTRIES` | `100` | 1 – 10000 | Max distinct URLs cached. Beyond this, LRU evicts. |
| `COLMENA_POOL_MAX_CONN_PER_URL` | `2` | 1 – 50 | `sqlx::PgPoolOptions::max_connections`. |
| `COLMENA_POOL_MIN_CONN_PER_URL` | `0` | 0 – `max_conn_per_url` | Keep at 0 so idle pools hold 0 TCP conns. |
| `COLMENA_POOL_IDLE_TIMEOUT_SEC` | `30` | 10 – 3600 | Closes idle TCP connections within a pool. |
| `COLMENA_POOL_MAX_LIFETIME_SEC` | `600` | 60 – 86400 | Recycles connections to survive LB idle-kills. |
| `COLMENA_POOL_ACQUIRE_TIMEOUT_SEC` | `10` | 1 – 60 | Max wait for a free connection before query fails. |

`PoolConfig::from_env()` validates ranges and returns `ConfigError` with a descriptive message if any are out of bounds. The worker fails fast at startup rather than run with silent misconfiguration.

## Observability

### Metrics

```rust
pub struct RegistryMetrics {
    pub cached_pools: usize,
    pub pinned_pools: usize,
    pub evictions_total: u64,
    pub get_or_create_total: u64,
    pub cache_hits_total: u64,
    pub per_url: Vec<PoolMetrics>,
}

pub struct PoolMetrics {
    pub url_hash: String,         // SHA-256 hex of normalized URL — never log raw URLs (credentials)
    pub size: u32,                // sqlx pool size (active + idle)
    pub idle: u32,
    pub last_used_at: SystemTime,
    pub pinned: bool,
}
```

### Debug endpoint (worker)

`GET /debug/pools` returns `RegistryMetrics` as JSON. Useful in staging; in prod, protect behind a simple header check or Cloud Run IAM allowlist.

### Tracing events

- `engine_started { pinned_pool_count, max_entries, idle_timeout_sec }` — info, once at boot.
- `pool_created { url_hash, pinned }` — info, on new pool in registry.
- `pool_evicted { url_hash, reason }` — warn, on LRU evict.
- `engine_shutdown { pools_closed, duration_ms }` — info, on clean close.
- `engine_dropped_without_shutdown` — warn, from defensive `Drop`.

**Never log raw connection URLs** — they contain credentials. Use `url_hash` or a masked form (`postgres://***@host:port/db`).

## Migration plan

Atomic in a single PR. No two-pool transitional state. Commits within the PR:

1. **`pool_registry.rs` module** — `PgPoolRegistry`, `PoolConfig`, `UrlKey`, `RegistryMetrics`. Unit tests: URL normalization, pin/unpin, LRU eviction, concurrent `get_or_create`, `close_all`.
2. **`engine.rs` + factory refactors** — `ColmenaEngine`, `EngineConfig`; refactor `ConversationRepositoryFactory` and `PgPoolAdapter` signatures. Existing unit tests re-run against new signatures.
3. **`SqlPortFactory` + `nodes/sql.rs` refactor** — wire factory through node context. SQL node integration tests re-run.
4. **`api.rs`, CLI `main.rs`, worker `main.rs` refactor** — switch to `ColmenaEngine`. Worker adds `shutdown_signal` handling and explicit `engine.shutdown().await` after axum graceful stop.
5. **Observability** — `/debug/pools` endpoint in the worker, tracing events wired. Post-deploy validation script documented in the PR.

Rollback is a binary revert: no schema changes, no Redis key changes, no HTTP contract changes.

## Rollout

**Staging:**

1. Deploy to staging Cloud Run (`min_instances=1, max_instances=3`).
2. Exercise representative jobs for ~30 min.
3. Validate `pg_stat_activity` on Cloud SQL: stable connection count (expected ~2–6 for internal DB under light load).
4. Validate `/debug/pools`: `cached_pools` stable, `evictions_total` ≈ 0 under normal traffic.
5. Test suspend/resume: confirm state persistence still works through pinned pool.
6. Test a DAG with a SQL node pointing to an **external** DB: confirm a second registry entry appears and is reused on a second invocation.

**Production:**

1. Deploy during an off-peak window with rollback plan prepared.
2. Observe for 24h: Cloud SQL connection count, `/debug/pools`, worker error rate, job latency p50/p95/p99.
3. Abort and rollback if Cloud SQL connections exceed 50% of `max_connections`, or worker error rate increases > 0.5% over baseline.

## Phase 2 triggers (future)

Revisit **Cloud SQL Managed Connection Pooling** (port 6432, Enterprise Plus edition) only if one of the following holds *after* Phase 1 is in production:

- **T1**: `pg_stat_activity` sustains > 60% of Cloud SQL `max_connections` for > 10 minutes.
- **T2**: Cloud SQL upgrade to Enterprise Plus is justified by an unrelated requirement.
- **T3**: Cloud Run sustains > 50 concurrent instances, making cross-instance multiplexing attractive.

Phase 2 is a **config change only**: switch `DATABASE_URL` port from `5432` to `6432`, enable MCP on the Cloud SQL instance, set `max_prepared_statements > 0` to accommodate sqlx's prepared-statement usage, and verify that the SQL node's `SET LOCAL statement_timeout` / `SET LOCAL work_mem` / `SELECT set_config(..., true)` inside transactions behave correctly under transaction-mode pooling (they should — all three are transaction-scoped). No engine code changes required.

## Testing strategy

- **Unit (registry):** URL normalization edge cases; pin is exempt from LRU and from `max_entries`; `get_or_create` concurrent calls for the same URL return the same `Arc<PgPool>`; `close_all` drains all pools and marks them closed.
- **Unit (factories):** `ConversationRepositoryFactory` and `SqlPortFactory` share the same pool when given the same URL.
- **Integration (engine boot):** `ColmenaEngine::new` successfully migrates `state` and `secure_values` schemas on the pinned pool.
- **Integration (worker):** Run the platform worker against a local Postgres + Redis fixture; submit 100 concurrent jobs against the internal DB only; assert that connections to the internal DB in `pg_stat_activity` stay ≤ `max_conn_per_url` (the pinned pool cap) throughout, modulo the brief transient overlap during `max_lifetime` rotation.
- **Integration (shutdown):** Send SIGTERM mid-execution; verify all in-flight queries complete or time out cleanly and no connections remain open in `pg_stat_activity` after shutdown.
- **Regression (graphs):** Re-run the existing `tests/graphs/` suite end-to-end, with special focus on `memory/`, `agents/` (LLM memory), and any SQL-node graph.

## Open risks

- **`PgPoolAdapter` shared between nodes.** Today each `PgPoolAdapter` holds `Arc<RwLock<u64>>` for `statement_timeout_ms` and `work_mem_mb`. Once multiple nodes can share a pool (and construct their own adapters wrapping it), each adapter keeps its own per-query config — confirmed safe because the values are applied via `SET LOCAL` inside transactions. **Mitigation:** add an integration test with two SQL nodes in the same graph, same URL, different timeouts.
- **URL normalization collisions.** Two URLs that differ only in unused query parameters will be cached separately. Acceptable — conservative normalization avoids false positives that could corrupt pool config.
- **Eviction of a pool with in-flight queries.** `PgPool::close()` on an evicted `Arc<PgPool>` waits for in-flight queries to finish. But the evicted pool is no longer reachable via the registry, so a new `get_or_create(same_url)` during eviction would create a second pool transiently. **Mitigation:** evictions are rare (only when cache > `max_entries`, which requires > 100 distinct URLs in flight). If this becomes a real pattern, add a generation counter; not baseline.
- **sqlx prepared-statement cache under high URL churn.** Each pool keeps its own prepared-statement cache. LRU eviction discards it. Minor perf hit only. Non-blocking.

## Future optimizations (not baseline)

- TTL-based eviction of idle pools (time since `last_used_at` > threshold) to reclaim Rust memory.
- Parallel `pool.close()` during shutdown with `futures::future::join_all` if the close phase ever approaches the Cloud Run 10s grace window.
- Prometheus/OpenTelemetry exporter for `RegistryMetrics`.
- Retry-on-transient-error wrapper around `registry.get_or_create` for network flakes during pool creation.
