# src/libs/colmena/src/dag_engine/engine.rs

**Layer:** application  **Purpose:** Process-wide entry point for DAG execution. Owns the shared pool registry, pinned internal-DB pool, state/secure-value repositories, node registry, and DagRunUseCase. Exposes four execution paths (run_dag, execute_stream, execute_stream_cancellable, stream_sse_parts) and handles graceful shutdown.

## Symbols

- `EngineError` (enum, pub) — Error type combining config/registry/migration/other errors
  - `Config` (variant) — Wraps ConfigError
  - `Registry` (variant) — Wraps RegistryError
  - `Migration` (variant) — Migration failure with message
  - `Other` (variant) — Catchall for other errors

- `EngineConfig` (struct, pub) — Configuration bundle for ColmenaEngine initialization
  - `internal_database_url` (field) — Postgres URL for internal schema
  - `pool_config` (field) — Pool configuration (size, timeout, etc.)
  - `storage` (field) — Arc to output storage adapter (media generation persistence)
  - `attachment_registry` (field) — Optional override attachment registry for tests
  - `liveness` (field) — Heartbeat + idle-timeout settings for execution loop

- `parse_bool_env` (fn, private) — Parses truthy/falsy string env values to Option<bool>

- `EngineConfig::from_env` (method, pub async) — Builds config from environment variables with multi-tier storage adapter fallback (explicit local/prod via COLMENA_LOCAL, implicit via callback/local-dir/in-memory)

- `ColmenaEngine` (struct, pub) — Process-wide orchestrator owning registry, use case, and shutdown flag
  - `registry` (field) — Shared pool registry for all Postgres connections
  - `use_case` (field) — DAG execution use case
  - `closed` (field) — AtomicBool for idempotent shutdown tracking

- `ColmenaEngine::new` (method, pub async) — Async constructor: pins internal pool, runs migrations, creates repositories/factories/node registry, wires circular dependencies for subgraph/for_each execution

- `ColmenaEngine::run_dag` (method, pub async) — Single-turn synchronous execution: drains execute_stream and returns final output (legacy path now delegating to execute_stream)

- `ColmenaEngine::execute_stream` (method, pub) — Streams raw DagExecutionEvent without cancellation support (6-arg backward-compatible API)

- `ColmenaEngine::execute_stream_cancellable` (method, pub) — Streams raw DagExecutionEvent with hard-stop support via CancellationToken (stops between nodes, persists Cancelled state, marks subgraph descendants as CANCELLED)

- `ColmenaEngine::stream_sse_parts` (method, pub) — Streams Vercel-AI-SDK style SSE payloads via SseMapper; takes Arc<Self> for 'static lifetime; auto-calls shutdown and drops Arc on graph finish

- `ColmenaEngine::registry_metrics` (method, pub) — Snapshots pool registry metrics (cached_pools count, etc.)

- `ColmenaEngine::shutdown` (method, pub async) — Idempotent close of all cached pools using AtomicBool swap gate; logs pool count and duration

- `ColmenaEngine` Drop impl — Warns if engine dropped without explicit shutdown (safety net for resource cleanup)

- `env_guard_rail_tests` (mod, #[cfg(test)]) — Test module for storage adapter env selection logic
  - `clean_env` (fn, private) — Clears all COLMENA_LOCAL/COLMENA_STORAGE_* env vars before each test
  - `parse_bool_env_recognizes_truthy_values` (test) — Verifies truthy string parsing (true/1/yes/on, case-insensitive)
  - `parse_bool_env_recognizes_falsy_values` (test) — Verifies falsy string parsing (false/0/no/off)
  - `parse_bool_env_returns_none_for_unset` (test) — Verifies None for unset env var
  - `parse_bool_env_returns_none_for_garbage` (test) — Verifies None for invalid values

## File-level notes

- **Architecture**: Facade/entry-point pattern. Orchestrates domain traits, infrastructure adapters, and application use cases. Clean separation between public API (execution methods) and private internals (registry, use case).
- **Storage adapter selection**: Sophisticated multi-tier fallback in `from_env` (lines 103–205) handles explicit local/prod modes via COLMENA_LOCAL flag plus implicit fallbacks. Well-documented with a feature table; inherent complexity but reasonable for environment flexibility.
- **Circular dependency wiring** (lines 299–300): Uses `set_subgraph_executor` and `set_foreach_registry` to break initialization cycles after `Arc::clone()`. Pattern is necessary for architecture but not obvious from struct API; adequately commented.
- **Execution paths**: Four methods export different abstraction levels (raw events, SSE payloads, single-turn value) for different consumers (CLI, Python bindings, HTTP server). Each has a clear documented contract.
- **Idempotent shutdown**: Uses `AtomicBool::swap` for thread-safe once-only close; Drop impl warns if omitted. Good resource-safety practice for bound ports (LocalHttpStorageAdapter).
- **Test discipline**: Env-mutating tests properly use `#[serial]` and clean before/after. No flaky behavior expected.
- **No dead code**: All public methods and fields are used or reasonably present for API completeness. The `registry_metrics` method is likely consumed by monitoring/observability code not in this repo.
