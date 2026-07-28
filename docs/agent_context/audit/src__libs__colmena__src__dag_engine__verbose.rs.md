# src/libs/colmena/src/dag_engine/verbose.rs

**Layer:** infrastructure  **Purpose:** Provides a global thread-safe atomic boolean flag to enable/disable verbose logging at runtime. Used to gate `colmena_log!` macro calls across the DAG engine and CLI.

## Symbols

- `VERBOSE` (static, private) — global AtomicBool holding the verbose flag state, initialized to false
- `set_verbose(v: bool)` (pub fn) — enables or disables verbose output at runtime using relaxed atomic store
- `is_verbose()` (pub fn) — returns true if verbose output is currently enabled via relaxed atomic load; marked #[inline]

## File-level notes

- Module doc comment mentions `COLMENA_VERBOSE=1` environment variable integration, but env-var reading is not implemented here — must be done by caller (likely CLI main or server startup that then calls `set_verbose()`)
- Uses `Ordering::Relaxed` for both load/store, which is correct for a flag (no synchronization with other atomics needed)
- Very focused, minimal module with single responsibility — no cross-cutting concerns or coupling
- No unused or incomplete code detected
