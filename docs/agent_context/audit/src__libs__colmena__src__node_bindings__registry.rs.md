# src/libs/colmena/src/node_bindings/registry.rs

**Layer:** bindings  **Purpose:** Provides TypeScript/Node.js read-only access to the node registry via napi for introspection (listing node types, querying toolkit sub-tools).

## Symbols

- `Registry` (pub struct) — napi binding to read-only `HashMapNodeRegistry`; inspection-only handle with no database operations.
- `Registry::node_types` (pub fn) — Returns all registered node type names sorted alphabetically; calls `get_all_nodes()` on the inner registry and collects/sorts keys.
- `Registry::toolkit_catalog` (pub fn) — Returns JSON array of sub-tool entries (name, description, required) for a toolkit node given its node_type and static config; throws `InvalidArg` if node_type is not a toolkit.
- `SmokeTaskMemory` (struct, private) — Stub implementation of `DagTaskMemoryRepository` trait; all methods are no-ops returning empty results or success; used only by `default_registry()` to avoid requiring a real database for introspection-only registries.
- `SmokeTaskMemory::add_task` (async method, via trait impl) — No-op; returns `Ok(())`.
- `SmokeTaskMemory::update_task_result` (async method, via trait impl) — No-op; returns `Ok(())`.
- `SmokeTaskMemory::get_tasks_for_run` (async method, via trait impl) — No-op; returns `Ok(vec![])`.
- `SmokeTaskMemory::get_first_uncompleted_task` (async method, via trait impl) — No-op; returns `Ok(None)`.
- `SmokeTaskMemory::delete_task` (async method, via trait impl) — No-op; returns `Ok(())`.
- `SmokeTaskMemory::clear_tasks_for_run` (async method, via trait impl) — No-op; returns `Ok(())`.
- `SmokeTaskMemory::get_current_phase` (async method, via trait impl) — No-op; returns `Ok(None)`.
- `SmokeTaskMemory::get_uncompleted_tasks_for_phase` (async method, via trait impl) — No-op; returns `Ok(vec![])`.
- `SmokeTaskMemory::save_phase_summary` (async method, via trait impl) — No-op; returns `Ok(())`.
- `SmokeTaskMemory::get_phase_summaries` (async method, via trait impl) — No-op; returns `Ok(vec![])`.
- `default_registry` (pub fn) — Constructs and returns an inspection-only `Registry` with in-memory pools, conversation and SQL factories, and a stub task-memory repo; designed for non-execution introspection use cases.

## File-level notes

- File is a thin napi binding layer; all computation delegated to `dag_engine::infrastructure::registry` and factory types.
- `SmokeTaskMemory` is intentionally a complete no-op implementation; this is required by the trait but designed to be unused in practice (inspection-only context, never executes graphs).
- No externally visible errors or panics; napi errors are mapped cleanly via `Error::new(Status::InvalidArg, ...)`.
- The `default_registry` function creates pools via `PgPoolRegistry::new()` but does not instantiate connections; pools lazily initialize on first use (per documented design).
- No unused imports or dead code detected.
