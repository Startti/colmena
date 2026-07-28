# src/libs/colmena/src/dag_engine/infrastructure/nodes/task_memory_writer.rs

**Layer:** infrastructure  
**Purpose:** DAG node that persists task results and applies Critic-driven mutations (add/delete tasks, suspend) to PostgreSQL task memory, then returns the current task state.

## Symbols

- `TaskMemoryWriterNode` (struct, pub) — stores an optional DagTaskMemoryRepository to manage task persistence
- `TaskMemoryWriterNode::new` (fn, pub) — constructs a node with an optional task memory repository
- `impl ExecutableNode for TaskMemoryWriterNode` — trait impl enabling the node to run in DAG execution
- `execute` (async fn, trait) — processes node inputs/config to update task results, apply add/delete/suspend mutations, fetch and return all current tasks; returns JSON with all_tasks and suspension status
- `description` (fn, trait) — returns descriptive string: "Updates the PostgreSQL Task Memory with task results and applies Critic mutations (add/delete/suspend)."
- `default_output` (fn, trait) — specifies "result" as the canonical output field
- `schema` (fn, trait) — returns JSON schema documenting inputs: task_id, result, add_tasks (array), delete_tasks (array), suspend (bool)

## File-level notes

- **Error handling boundary (line 91)**: delete_task errors are silently discarded (`let _ = ...`), unlike other repository operations which propagate via `?`. This is inconsistent — deletion failures are swallowed, which could hide data inconsistencies. Document or audit whether non-fatal deletion is intentional.
- **Session-id extraction**: session_id is pulled from _state with fallback to "unknown_run", allowing graceful handling of missing state.
- **Repo presence guard**: entire execute logic is gated by `if let Some(repo)` check (line 36); returns error if repository is None, which is the correct fail-closed behavior.
- **Schema well-defined**: node_schema() documents all supported inputs and behavior clearly.
