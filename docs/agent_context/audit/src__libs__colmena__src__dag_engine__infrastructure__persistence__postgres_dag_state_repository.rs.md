# src/libs/colmena/src/dag_engine/infrastructure/persistence/postgres_dag_state_repository.rs

**Layer:** infrastructure  
**Purpose:** PostgreSQL adapter implementing domain-level persistence traits (`DagStateRepository`, `DagTaskMemoryRepository`). Handles idempotent schema migrations and full CRUD/query operations over DAG run state, task memory, and phase summaries via sqlx.

## Symbols

- `PostgresDagStateRepository` (struct, pub) — PostgreSQL adapter holding a connection pool for DAG state persistence
- `PostgresDagStateRepository::new` (fn, pub) — Constructor wrapping a PgPool in the repository
- `PostgresDagStateRepository::pool` (fn, pub) — Exposes the connection pool for integration test cleanup
- `PostgresDagStateRepository::migrate` (async fn, pub) — Applies idempotent schema migrations at startup (ALTER TABLE ADD COLUMN IF NOT EXISTS on dag_runs and dag_task_memory)
- `row_to_task` (fn, private) — Helper that deserializes a sqlx PgRow into a DagTask, extracting phase/parallel/context/is_bridge with defensive defaults
- `DagStateRepository::get_by_id` (async fn, trait impl) — Fetches one DAG run by session_id, deserializing all JSON state columns (all_outputs, active_queue, execution_history, global_calls, caller_specific_calls, global_shared_state)
- `DagStateRepository::save` (async fn, trait impl) — Upserts a DagRunState via INSERT...ON CONFLICT, serializing all JSON state and optional session IDs
- `DagStateRepository::find_resume_entry` (async fn, trait impl) — Finds the top-level SUSPENDED session for an agent_session_id; errors if >1 concurrent chain exists
- `DagStateRepository::find_suspended_child` (async fn, trait impl) — Finds the most recent SUSPENDED child run of a given parent_session_id
- `DagStateRepository::cancel_running_descendants` (async fn, trait impl) — Recursive CTE that flips all RUNNING descendants of a root session to CANCELLED in one atomic update
- `DagTaskMemoryRepository::add_task` (async fn, trait impl) — Inserts a DagTask into dag_task_memory with phase, parallel, context, and is_bridge columns
- `DagTaskMemoryRepository::update_task_result` (async fn, trait impl) — Marks a task completed=TRUE and persists its result JSON
- `DagTaskMemoryRepository::get_tasks_for_run` (async fn, trait impl) — Fetches all tasks for a session, ordered by phase ASC then created_at ASC
- `DagTaskMemoryRepository::get_first_uncompleted_task` (async fn, trait impl) — Returns the earliest uncompleted task in execution order (lowest phase, earliest created_at)
- `DagTaskMemoryRepository::delete_task` (async fn, trait impl) — Deletes a single task by UUID
- `DagTaskMemoryRepository::clear_tasks_for_run` (async fn, trait impl) — Deletes all tasks for a session (bulk cleanup)
- `DagTaskMemoryRepository::get_current_phase` (async fn, trait impl) — Returns MIN(phase) of uncompleted tasks; indicates the next phase to process
- `DagTaskMemoryRepository::get_uncompleted_tasks_for_phase` (async fn, trait impl) — Fetches all uncompleted tasks in a specific phase, ordered by created_at
- `DagTaskMemoryRepository::save_phase_summary` (async fn, trait impl) — Stores a phase summary string in dag_phase_summaries table
- `DagTaskMemoryRepository::get_phase_summaries` (async fn, trait impl) — Retrieves all phase summaries for a session, ordered by phase ASC

## File-level notes

- **Defensive deserialization strategy**: `row_to_task` uses `.try_get().unwrap_or()` on all optional columns (phase, parallel, context, is_bridge) to tolerate missing columns during migrations. This is intentional infrastructure-layer robustness, not a bug.
- **Silent status coercion in `get_by_id`**: Line 117–118 uses `.unwrap_or(DagRunStatus::Failed)` when parsing the status string. Any malformed status silently becomes Failed rather than propagating an error. This could mask data corruption but is symmetric with the defensive deserialization pattern.
- **Idempotent migration logic**: Lines 29–62 hardcode all schema mutations (5 ALTER TABLE on dag_runs, 4 on dag_task_memory). Migrations are run at every repository init. No external migration file; common in sqlx codebases but less maintainable than separate .sql files if schema grows.
- **Recursive CTE for descendant traversal**: `cancel_running_descendants` (lines 268–295) uses a recursive CTE to flatten the parent_session_id tree and update all RUNNING descendants in one atomic query. Correct but could benefit from inline comments explaining the CTE logic.
- **Find resume entry query complexity**: `find_resume_entry` (lines 218–247) uses a nested subquery to exclude suspended parents when filtering top-level chains. The query is correct but the logic could be clearer with comments explaining why parent_session_id NOT IN (...) is necessary.
- **All trait implementations are complete**: No unfinished methods, no todo!() or unimplemented!() markers. All error paths use `.map_err()` to wrap into DagError.
