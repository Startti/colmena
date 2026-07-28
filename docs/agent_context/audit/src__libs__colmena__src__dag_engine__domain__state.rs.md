# src/libs/colmena/src/dag_engine/domain/state.rs

**Layer:** domain  
**Purpose:** Defines the core state structures and persistence ports for DAG run execution: run status enums, run/task/phase-summary value objects, and repository traits for retrieving and persisting orchestration state.

## Symbols

- `DagRunStatus` (enum, pub) — Terminal and intermediate execution states: Running, Suspended, Completed, Failed, Cancelled (each with doc comment explaining purpose).
- `DagRunStatus::Display` impl — Formats status as uppercase string ("RUNNING", "SUSPENDED", etc.).
- `DagRunStatus::FromStr` impl — Parses uppercase string back to `DagRunStatus` variant or error.
- `DagRunState` (struct, pub) — Complete snapshot of a DAG run: session_id, agent_session_id, parent_session_id, graph_json, all_outputs, status, global_shared_state, active_queue, execution_history, global_calls, caller_specific_calls.
- `DagStateRepository` (trait, pub) — Async persistence port: get_by_id, save, find_resume_entry (locate top of suspended chain per agent), find_suspended_child (locate direct child), cancel_running_descendants (mark descendants cancelled on hard stop).
- `DagTask` (struct, pub) — Single unit of work: id, session_id, task_name, assigned_to, completed flag, result, phase (1-based), parallel flag, context (semantic purpose), is_bridge flag (phase prerequisite).
- `DagPhaseSummary` (struct, pub) — Reactor output per phase: session_id, phase number, summary string.
- `DagTaskMemoryRepository` (trait, pub) — Async persistence port for task lifecycle and phase awareness: add/update/delete/list tasks, phase routing (get_current_phase, get_uncompleted_tasks_for_phase), phase-summary save/retrieve.
- `status_tests` (mod, private) — 3 tests validating DagRunStatus Display/FromStr roundtrip and variant parsing.

## File-level notes

- Clean separation of concerns: value objects (DagRunState, DagTask, DagPhaseSummary) vs. abstract persistence ports (two traits).
- `cancel_running_descendants` has a default no-op implementation (`Ok(0)`) to allow in-memory/test repositories to opt out while persisted repositories provide full semantics.
- All complex methods (`find_resume_entry`, `find_suspended_child`, `cancel_running_descendants`) include detailed doc comments explaining chain semantics, single-leaf-at-a-time design, and cascade behavior.
- No infrastructure dependencies; all external integrations are via traits (hexagonal ports).
- Serde derives enable JSON serialization for graph snapshots and persistence.
