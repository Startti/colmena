# Agent Session ID Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Introduce a chat-scoped `agent_session_id` that lives above per-run `session_id`, enabling resume-by-conversation and cross-run LLM memory continuity, and fix the silent `llm_node_history` collision when multiple `llm_call` nodes share a run.

**Architecture:** Aditive schema columns (`agent_session_id`, `parent_session_id` on `dag_runs`; `agent_session_id`, `node_id` on `llm_node_history`). Engine injects `__colmena_agent_session_id` and `__colmena_node_id_path` into every node's inputs. LLM history reads by `(agent_session_id, node_id)` when present, falls back to `(session_id, node_id)` legacy. Subgraphs inherit the conversation handle and extend the path prefix. Lifecycle decision logic in `DagRunUseCase::execute_stream` covers three branches: direct resume by session_id, resume-by-agent (find leaf), or fresh start.

**Tech Stack:** Rust 1.x, sqlx (Postgres + SQLite), axum, tokio, async-trait, mockall (tests).

**Spec:** `docs/superpowers/specs/2026-04-28-agent-session-id-design.md`

---

## File structure

### Files created

- `src/libs/colmena/migrations/postgres/20260428000001_dag_runs_agent_session_id.sql` — PG migration: dag_runs columns
- `src/libs/colmena/migrations/postgres/20260428000002_llm_history_agent_and_node.sql` — PG migration: llm_node_history columns
- `src/libs/colmena/migrations/sqlite/20260428000001_llm_history_agent_and_node.sql` — SQLite migration (only `llm_node_history` exists in SQLite; `dag_runs` is PG-only)
- `tests/agent_session_id_lifecycle.rs` — end-to-end integration tests covering the three lifecycle branches

### Files modified

| Path | Responsibility |
|---|---|
| `src/libs/colmena/src/dag_engine/domain/graph.rs` | `Graph::validate()` rejects `/` in node IDs |
| `src/libs/colmena/src/dag_engine/domain/state.rs` | `DagRunState` adds `agent_session_id`, `parent_session_id` fields. `DagStateRepository` gains `find_suspended_leaf` |
| `src/libs/colmena/src/dag_engine/infrastructure/persistence/postgres_dag_state_repository.rs` | Read/write new columns, implement `find_suspended_leaf` |
| `src/libs/colmena/src/llm/domain/memory.rs` | `ConversationRepository` trait carries optional `agent_session_id` and required `node_id` |
| `src/libs/colmena/src/llm/infrastructure/persistence/postgres_conversation_repository.rs` | New keying strategy (read/write) |
| `src/libs/colmena/src/llm/infrastructure/persistence/sqlite_conversation_repository.rs` | New keying strategy (read/write) |
| `src/libs/colmena/src/llm/infrastructure/persistence/in_memory_conversation_repository.rs` | New keying strategy |
| `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs` | Read by `(agent_session_id, node_id_path)` when agent present |
| `src/libs/colmena/src/dag_engine/infrastructure/nodes/subgraph.rs` | UUID for child, propagate `agent_session_id`, write `parent_session_id`, extend path prefix |
| `src/libs/colmena/src/dag_engine/application/ports.rs` | `SubGraphExecutorPort` signatures |
| `src/libs/colmena/src/dag_engine/application/run_use_case.rs` | Lifecycle decision, injection of `__colmena_agent_session_id`, `__colmena_node_id_path`, `find_suspended_leaf` use, conflict check |
| `src/libs/colmena/src/dag_engine/engine.rs` | `execute_stream` signature gains `agent_session_id: Option<String>` and `path_prefix: Option<String>` |
| `src/libs/colmena/src/dag_engine/main.rs` | CLI flag `--agent-session-id` |
| `src/libs/colmena/src/dag_engine/api.rs` | HTTP `X-Agent-Session-Id` header / body field; `run_dag` signature accepts agent id |
| `src/libs/colmena/src/node_bindings/mod.rs` | Forward new arg to `run_dag` (or default `None`) |
| `src/libs/colmena/src/python_bindings/mod.rs` | Forward new arg to `run_dag` (or default `None`) |
| `docs/developer_guide/database_schema.md` | Document new columns and keying |
| `docs/developer_guide/15_memory_guide.md` | Explain `agent_session_id` and per-node history |
| `docs/developer_guide/19_nested_agents_and_subgraphs.md` | Note path-qualified node_id and parent_session_id |

---

## Task summary (sequence)

| # | Phase | Task |
|---|---|---|
| 1 | Foundation | Add `Graph::validate()` rejecting `/` in node IDs |
| 2 | Foundation | Wire validation into all graph load sites |
| 3 | Schema | Postgres migration: `dag_runs` columns |
| 4 | Schema | Postgres migration: `llm_node_history` columns |
| 5 | Schema | SQLite migration: `llm_node_history` columns |
| 6 | Domain | Extend `DagRunState` with `agent_session_id`, `parent_session_id` |
| 7 | Domain | Add `find_suspended_leaf` to `DagStateRepository` trait |
| 8 | Domain | Extend `ConversationRepository` trait with optional `agent_session_id` and required `node_id` |
| 9 | Persistence | Update `PostgresDagStateRepository` to read/write new columns |
| 10 | Persistence | Implement `find_suspended_leaf` in `PostgresDagStateRepository` |
| 11 | Persistence | Update `PostgresConversationRepository` with new keying |
| 12 | Persistence | Update `SqliteConversationRepository` with new keying |
| 13 | Persistence | Update `InMemoryConversationRepository` with new keying |
| 14 | Engine | Compute path-qualified `node_id` per node and inject `__colmena_node_id_path` |
| 15 | Engine | Inject `__colmena_agent_session_id` (always present, may be JSON null) |
| 16 | Engine | Extend `execute_stream` signature with `agent_session_id` and `path_prefix` |
| 17 | LLM node | `llm.rs` switches keying based on agent presence |
| 18 | Subgraph | Extend `SubGraphExecutorPort` with `parent_session_id`, `agent_session_id`, `path_prefix` |
| 19 | Subgraph | `SubGraphNode` uses UUID, writes `parent_session_id`, extends path prefix |
| 20 | Lifecycle | Implement branches 1/2/3 decision logic in `execute_stream` |
| 21 | Lifecycle | Add conflict check `(agent_session_id, session_id)` mismatch error |
| 22 | API | Update `main.rs` CLI flag `--agent-session-id` |
| 23 | API | Update `api.rs` HTTP `X-Agent-Session-Id` header + body field, extend `run_dag` |
| 24 | Bindings | Forward optional `agent_session_id` in node + python bindings |
| 25 | Tests | End-to-end integration tests covering lifecycle branches |
| 26 | Docs | Update `database_schema.md`, `15_memory_guide.md`, `19_nested_agents_and_subgraphs.md` |

---

## Phase 0 — Foundation: graph validation

### Task 1: Add `Graph::validate()` rejecting `/` in node IDs

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/domain/graph.rs`

- [ ] **Step 1: Write the failing test**

Append at the bottom of `graph.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn graph_with_node_id(id: &str) -> Graph {
        let json = json!({
            "nodes": {
                id: { "type": "math", "config": {} }
            },
            "edges": []
        });
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn validate_rejects_slash_in_node_id() {
        let g = graph_with_node_id("router/inner");
        let err = g.validate().unwrap_err();
        assert!(err.contains("router/inner"), "error should name the offending id, got: {}", err);
        assert!(err.contains("'/'"), "error should mention the forbidden char, got: {}", err);
    }

    #[test]
    fn validate_accepts_clean_node_id() {
        let g = graph_with_node_id("router_inner");
        assert!(g.validate().is_ok());
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib --package colmena_dag_engine dag_engine::domain::graph::tests`
Expected: FAIL with "no method named `validate` found".

- [ ] **Step 3: Implement `validate`**

Add at the bottom of `Graph` (before the `tests` mod):

```rust
impl Graph {
    /// Validates structural invariants the engine depends on.
    ///
    /// Currently rejects:
    /// - Node IDs containing `/` — the engine uses `/` to separate path-qualified
    ///   `node_id`s in subgraph hierarchies (`subgraph_node/inner_node`). Allowing
    ///   `/` in user-defined IDs would make the resulting paths ambiguous.
    pub fn validate(&self) -> Result<(), String> {
        for node_id in self.nodes.keys() {
            if node_id.contains('/') {
                return Err(format!(
                    "Invalid node id '{}': character '/' is reserved for subgraph path qualifiers",
                    node_id
                ));
            }
        }
        Ok(())
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --lib --package colmena_dag_engine dag_engine::domain::graph::tests`
Expected: PASS (2 passed).

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/dag_engine/domain/graph.rs
git commit -m "feat(graph): add validate() rejecting '/' in node ids

Reserves '/' as the path separator for subgraph-qualified node ids,
which the upcoming agent_session_id work uses to disambiguate llm_call
nodes that share an id across subgraphs."
```

---

### Task 2: Wire `validate()` into all graph load sites

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/main.rs:80-82`
- Modify: `src/libs/colmena/src/dag_engine/api.rs:32`
- Modify: `src/libs/colmena/src/dag_engine/api.rs` (server graph load — top-level `serve_dag`)
- Modify: `src/libs/colmena/src/dag_engine/application/run_use_case.rs:796-797` (subgraph entry)

- [ ] **Step 1: Write a failing integration-style test for the CLI run path**

Create `tests/graph_validation.rs`:

```rust
//! Smoke test: feeding a graph with an invalid node id is rejected before execution.

use colmena::dag_engine::domain::graph::Graph;

#[test]
fn graph_with_slash_in_node_id_fails_validation() {
    let raw = serde_json::json!({
        "nodes": {
            "bad/id": { "type": "math", "config": {} }
        },
        "edges": []
    });
    let g: Graph = serde_json::from_value(raw).unwrap();
    let err = g.validate().expect_err("validation must fail");
    assert!(err.contains("bad/id"));
}
```

- [ ] **Step 2: Run it (expected to pass on its own — Task 1 already added validate())**

Run: `cargo test --test graph_validation`
Expected: PASS.

- [ ] **Step 3: Wire validation into main.rs `Run` arm**

In `src/libs/colmena/src/dag_engine/main.rs` after the `serde_json::from_str` call (around line 80–82) add:

```rust
let graph: colmena::dag_engine::domain::graph::Graph =
    serde_json::from_str(&file_content)?;
graph
    .validate()
    .map_err(|e| anyhow::anyhow!("Invalid graph: {}", e))?;
```

- [ ] **Step 4: Wire validation into api.rs `run_dag` and `serve_dag`**

In `src/libs/colmena/src/dag_engine/api.rs:32`, immediately after `let mut graph: Graph = serde_json::from_str(&file_content)?;` add:

```rust
graph
    .validate()
    .map_err(|e| Box::<dyn std::error::Error>::from(format!("Invalid graph: {}", e)))?;
```

In `serve_dag` (where the graph is loaded once at startup, search for `serde_json::from_str` near `pub async fn serve_dag`), add the same validation and propagate the error via `Box<dyn Error>`.

- [ ] **Step 5: Wire validation into subgraph spawning**

In `src/libs/colmena/src/dag_engine/application/run_use_case.rs` `run_subgraph` (around line 796–797), after `let graph: Graph = serde_json::from_value(graph_json.clone())?`, add:

```rust
graph
    .validate()
    .map_err(|e| DagError::NodeExecution(format!("Invalid sub-graph: {}", e)))?;
```

- [ ] **Step 6: Compile**

Run: `cargo build`
Expected: success.

- [ ] **Step 7: Run all existing tests to confirm no regression**

Run: `cargo test --lib --package colmena_dag_engine`
Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add src/libs/colmena/src/dag_engine/main.rs \
        src/libs/colmena/src/dag_engine/api.rs \
        src/libs/colmena/src/dag_engine/application/run_use_case.rs \
        tests/graph_validation.rs
git commit -m "feat(graph): validate node ids at every graph load site"
```

---

## Phase 1 — Schema migrations

### Task 3: Postgres migration — `dag_runs` columns

**Files:**
- Create: `src/libs/colmena/migrations/postgres/20260428000001_dag_runs_agent_session_id.sql`

- [ ] **Step 1: Create the migration file**

```sql
-- Adds chat-scoped identifier and explicit parent linkage to dag_runs.
-- See docs/superpowers/specs/2026-04-28-agent-session-id-design.md §3.1.

ALTER TABLE dag_runs
    ADD COLUMN IF NOT EXISTS agent_session_id VARCHAR(255),
    ADD COLUMN IF NOT EXISTS parent_session_id VARCHAR(255);

CREATE INDEX IF NOT EXISTS idx_dag_runs_agent_session_id
    ON dag_runs(agent_session_id);

CREATE INDEX IF NOT EXISTS idx_dag_runs_parent_session_id
    ON dag_runs(parent_session_id);

CREATE INDEX IF NOT EXISTS idx_dag_runs_agent_status
    ON dag_runs(agent_session_id, status);
```

- [ ] **Step 2: Compile (forces migration discovery)**

Run: `cargo build`
Expected: success.

- [ ] **Step 3: Run a dummy graph to apply the migration**

Pick any small graph that boots the engine, e.g.:
```bash
source .env
cargo run --bin dag_engine -- run tests/graphs/basic/trigger.json
```

Expected: starts (the migration runs at engine boot via `sqlx::migrate!`). Run terminates normally.

- [ ] **Step 4: Verify columns exist**

```bash
psql "$DATABASE_URL" -c "\d dag_runs" | grep -E "agent_session_id|parent_session_id"
```
Expected: both columns shown.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/migrations/postgres/20260428000001_dag_runs_agent_session_id.sql
git commit -m "feat(db): add agent_session_id and parent_session_id to dag_runs"
```

---

### Task 4: Postgres migration — `llm_node_history` columns

**Files:**
- Create: `src/libs/colmena/migrations/postgres/20260428000002_llm_history_agent_and_node.sql`

- [ ] **Step 1: Create the migration file**

```sql
-- Adds chat-scoped identifier and per-node identifier to llm_node_history.
-- Pre-existing rows have node_id = NULL and are excluded from new reads.
-- See docs/superpowers/specs/2026-04-28-agent-session-id-design.md §3.2.

ALTER TABLE llm_node_history
    ADD COLUMN IF NOT EXISTS agent_session_id TEXT,
    ADD COLUMN IF NOT EXISTS node_id TEXT;

CREATE INDEX IF NOT EXISTS idx_llm_history_agent_node
    ON llm_node_history(agent_session_id, node_id, created_at);

CREATE INDEX IF NOT EXISTS idx_llm_history_session_node
    ON llm_node_history(session_id, node_id, created_at);
```

- [ ] **Step 2: Compile and apply via dummy run**

Run: `cargo run --bin dag_engine -- run tests/graphs/basic/trigger.json`
Expected: success.

- [ ] **Step 3: Verify**

```bash
psql "$DATABASE_URL" -c "\d llm_node_history" | grep -E "agent_session_id|node_id"
```
Expected: both columns shown.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/migrations/postgres/20260428000002_llm_history_agent_and_node.sql
git commit -m "feat(db): add agent_session_id and node_id to llm_node_history"
```

---

### Task 5: SQLite migration — `llm_node_history` columns

**Files:**
- Create: `src/libs/colmena/migrations/sqlite/20260428000001_llm_history_agent_and_node.sql`

- [ ] **Step 1: Create the migration**

```sql
-- SQLite mirror of the Postgres llm_history migration (§3.2 of the spec).

ALTER TABLE llm_node_history ADD COLUMN agent_session_id TEXT;
ALTER TABLE llm_node_history ADD COLUMN node_id TEXT;

CREATE INDEX IF NOT EXISTS idx_llm_history_agent_node
    ON llm_node_history(agent_session_id, node_id, created_at);

CREATE INDEX IF NOT EXISTS idx_llm_history_session_node
    ON llm_node_history(session_id, node_id, created_at);
```

> Note: SQLite has no `dag_runs` table (Postgres-only feature). No SQLite mirror needed for the dag_runs migration.

- [ ] **Step 2: Run a graph that uses SQLite memory**

```bash
cargo run --bin dag_engine -- run tests/graphs/memory/memory_sqlite_example.json
```
Expected: success.

- [ ] **Step 3: Verify columns**

```bash
sqlite3 colmena_memory.db ".schema llm_node_history"
```
Expected: schema includes `agent_session_id` and `node_id`.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/migrations/sqlite/20260428000001_llm_history_agent_and_node.sql
git commit -m "feat(db): SQLite mirror of llm_history agent+node columns"
```

---

## Phase 2 — Domain types

### Task 6: Extend `DagRunState` with `agent_session_id`, `parent_session_id`

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/domain/state.rs`

- [ ] **Step 1: Add the fields**

In `state.rs:42`, replace the `DagRunState` struct with:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DagRunState {
    pub session_id: String,

    /// Chat / conversation handle. NULL for legacy runs that never opted in.
    #[serde(default)]
    pub agent_session_id: Option<String>,

    /// session_id of the immediate parent run when this row is a subgraph.
    /// NULL for root runs.
    #[serde(default)]
    pub parent_session_id: Option<String>,

    pub graph_json: Value,
    pub all_outputs: HashMap<String, Value>,
    pub status: DagRunStatus,

    /// Global shared state acting as a persistent whiteboard for all nodes
    #[serde(default)]
    pub global_shared_state: Value,

    /// The current execution queue. When suspending, this captures what is left to run.
    #[serde(default)]
    pub active_queue: std::collections::VecDeque<String>,

    /// Sequence of executed nodes as (CallerId, TargetId)
    #[serde(default)]
    pub execution_history: Vec<(String, String)>,

    /// Total execution count per node
    #[serde(default)]
    pub global_calls: HashMap<String, u32>,

    /// Caller-specific execution count matrix: caller_id -> target_id -> count
    #[serde(default)]
    pub caller_specific_calls: HashMap<String, HashMap<String, u32>>,
}
```

- [ ] **Step 2: Compile**

Run: `cargo build`
Expected: failures in every site that constructs `DagRunState` literally (run_use_case.rs, postgres_dag_state_repository.rs, tests). They will be addressed in subsequent tasks; for now, fix only by adding `agent_session_id: None, parent_session_id: None` to **every** literal construction (search the workspace).

```bash
grep -rn "DagRunState {" src/libs/colmena/src/ tests/
```

For each match, add the two fields. Re-run `cargo build` until clean.

- [ ] **Step 3: Run the existing test suite to confirm no regression**

Run: `cargo test --lib --package colmena_dag_engine`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add -p src/libs/colmena/src/ tests/
git commit -m "feat(state): add agent_session_id and parent_session_id to DagRunState"
```

---

### Task 7: Add `find_suspended_leaf` to `DagStateRepository` trait

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/domain/state.rs:70-74`

- [ ] **Step 1: Add the method to the trait**

Replace the `DagStateRepository` block:

```rust
#[async_trait]
pub trait DagStateRepository: Send + Sync {
    async fn get_by_id(&self, session_id: &str) -> Result<Option<DagRunState>, DagError>;
    async fn save(&self, state: &DagRunState) -> Result<(), DagError>;

    /// Returns the `session_id` of the deepest SUSPENDED run for a given chat —
    /// the row that is SUSPENDED and is NOT the parent of any other SUSPENDED row.
    /// Returns `None` if no run is currently suspended for that chat.
    /// Returns `Err` if more than one leaf exists (concurrent suspended branches —
    /// out of scope for this design; defensive check).
    async fn find_suspended_leaf(
        &self,
        agent_session_id: &str,
    ) -> Result<Option<String>, DagError>;
}
```

- [ ] **Step 2: Compile and watch for missing impls**

Run: `cargo build`
Expected: compile errors at all `impl DagStateRepository` sites — these are addressed in Tasks 9–10.

- [ ] **Step 3: Add a temporary stub at every impl to keep the tree compiling for the next tasks**

For every impl found by `grep -rn "impl DagStateRepository" src/libs/colmena/src/`, add:

```rust
async fn find_suspended_leaf(
    &self,
    _agent_session_id: &str,
) -> Result<Option<String>, DagError> {
    Ok(None)
}
```

- [ ] **Step 4: Compile**

Run: `cargo build`
Expected: success.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/dag_engine/domain/state.rs \
        src/libs/colmena/src/dag_engine/infrastructure/persistence/
git commit -m "feat(state): add find_suspended_leaf to DagStateRepository trait

Stubbed in all impls; PostgresDagStateRepository will get a real
implementation in a later task."
```

---

### Task 8: Extend `ConversationRepository` trait

**Files:**
- Modify: `src/libs/colmena/src/llm/domain/memory.rs`

- [ ] **Step 1: Add a context struct and update the trait**

Replace `memory.rs` with:

```rust
use crate::llm::domain::{LlmError, LlmMessage};
use async_trait::async_trait;

/// Value Object that identifies the run scope of a single message.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SessionId(pub String);

/// Value Object that identifies the conversation a message belongs to.
/// `None` means the message belongs only to a single run (legacy mode).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AgentSessionId(pub String);

/// Path-qualified node identifier (e.g., "router" or "ventas/responder").
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NodeIdPath(pub String);

/// Identifies a single LLM thread to read/write history for.
#[derive(Debug, Clone)]
pub struct ConversationKey {
    pub session_id: SessionId,
    pub agent_session_id: Option<AgentSessionId>,
    pub node_id: NodeIdPath,
}

#[derive(Debug, Clone)]
pub struct Conversation {
    pub key: ConversationKey,
    pub messages: Vec<LlmMessage>,
}

#[async_trait]
pub trait ConversationRepository: Send + Sync {
    /// Loads all messages for the given thread.
    /// When `key.agent_session_id` is `Some`, filters by `(agent_session_id, node_id)`.
    /// When `None`, falls back to `(session_id, node_id)`.
    async fn get_by_id(&self, key: &ConversationKey) -> Result<Conversation, LlmError>;

    /// Appends a single message to the thread.
    /// Always writes `session_id` and `node_id`; `agent_session_id` is written
    /// when present.
    async fn add_message(
        &self,
        key: &ConversationKey,
        message: LlmMessage,
    ) -> Result<(), LlmError>;

    /// Deletes all messages for the given thread (matches the same filter as `get_by_id`).
    async fn delete(&self, key: &ConversationKey) -> Result<(), LlmError>;
}
```

- [ ] **Step 2: Update the `mod.rs` re-exports**

In `src/libs/colmena/src/llm/domain/mod.rs`, ensure these are exported:

```rust
pub use memory::{
    AgentSessionId, Conversation, ConversationKey, ConversationRepository, NodeIdPath, SessionId,
};
```

(Adapt to the existing `pub use memory::...` style — add the missing names without removing what's already there.)

- [ ] **Step 3: Compile and discover all callers that need updates**

Run: `cargo build`
Expected: errors at all 3 conversation repository impls + every site that calls `get_by_id`/`add_message`/`delete`. These are addressed in Tasks 11–13 and 17.

- [ ] **Step 4: Defer commit — keep this change uncommitted until Task 13 is green**

Tasks 11–13 land impls of the new trait shape. Until then the tree does not compile. **Do NOT commit yet.** The commit happens at the end of Task 13 (or earlier if a single squash commit is preferred).

Skip ahead to Task 9 with the trait change uncommitted in your working tree.

---

## Phase 3 — Persistence

### Task 9: Update `PostgresDagStateRepository` (read/write new columns)

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/persistence/postgres_dag_state_repository.rs`

- [ ] **Step 1: Update `get_by_id` SELECT**

In the `get_by_id` impl (around line 95), change the SQL and the row mapping:

```rust
async fn get_by_id(&self, session_id: &str) -> Result<Option<DagRunState>, DagError> {
    let row_opt = sqlx::query(
        "SELECT session_id, agent_session_id, parent_session_id, graph_json, all_outputs, status, \
                active_queue, execution_history, global_calls, caller_specific_calls, global_shared_state \
         FROM dag_runs WHERE session_id = $1"
    )
    .bind(session_id)
    .fetch_optional(&self.pool)
    .await
    .map_err(|e| DagError::StateError(format!("Database error on get: {}", e)))?;

    match row_opt {
        Some(row) => {
            let status_str: String = row.get("status");
            let status = status_str
                .parse::<DagRunStatus>()
                .unwrap_or(DagRunStatus::Failed);

            let all_outputs_json: serde_json::Value = row.get("all_outputs");
            let all_outputs: HashMap<String, Value> =
                serde_json::from_value(all_outputs_json).unwrap_or_default();

            let active_queue_json: serde_json::Value = row.get("active_queue");
            let active_queue = serde_json::from_value(active_queue_json).unwrap_or_default();

            let execution_history_json: serde_json::Value = row.get("execution_history");
            let execution_history =
                serde_json::from_value(execution_history_json).unwrap_or_default();

            let global_calls_json: serde_json::Value = row.get("global_calls");
            let global_calls = serde_json::from_value(global_calls_json).unwrap_or_default();

            let caller_specific_calls_json: serde_json::Value =
                row.get("caller_specific_calls");
            let caller_specific_calls =
                serde_json::from_value(caller_specific_calls_json).unwrap_or_default();

            let global_shared_state: serde_json::Value = row.get("global_shared_state");

            Ok(Some(DagRunState {
                session_id: row.get("session_id"),
                agent_session_id: row.try_get("agent_session_id").ok().flatten(),
                parent_session_id: row.try_get("parent_session_id").ok().flatten(),
                graph_json: row.get("graph_json"),
                all_outputs,
                status,
                global_shared_state,
                active_queue,
                execution_history,
                global_calls,
                caller_specific_calls,
            }))
        }
        None => Ok(None),
    }
}
```

(`row.try_get(...).ok().flatten()` returns `Option<Option<T>>` flattened to `Option<T>` — handles both NULL and pre-migration absence cleanly.)

- [ ] **Step 2: Update `save` UPSERT**

Replace the `sqlx::query` block of `save`:

```rust
sqlx::query(
    r#"INSERT INTO dag_runs (
        session_id, agent_session_id, parent_session_id,
        graph_json, all_outputs, status,
        active_queue, execution_history, global_calls, caller_specific_calls, global_shared_state,
        updated_at
       )
       VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, NOW())
       ON CONFLICT (session_id) DO UPDATE SET
         agent_session_id = EXCLUDED.agent_session_id,
         parent_session_id = EXCLUDED.parent_session_id,
         graph_json = EXCLUDED.graph_json,
         all_outputs = EXCLUDED.all_outputs,
         status = EXCLUDED.status,
         active_queue = EXCLUDED.active_queue,
         execution_history = EXCLUDED.execution_history,
         global_calls = EXCLUDED.global_calls,
         caller_specific_calls = EXCLUDED.caller_specific_calls,
         global_shared_state = EXCLUDED.global_shared_state,
         updated_at = NOW()"#
)
.bind(&state.session_id)
.bind(state.agent_session_id.as_deref())
.bind(state.parent_session_id.as_deref())
.bind(&state.graph_json)
.bind(&all_outputs_json)
.bind(&status_str)
.bind(&active_queue_json)
.bind(&execution_history_json)
.bind(&global_calls_json)
.bind(&caller_specific_calls_json)
.bind(&state.global_shared_state)
.execute(&self.pool)
.await
.map_err(|e| DagError::StateError(format!("Database error on save: {}", e)))?;
```

- [ ] **Step 3: Compile**

Run: `cargo build`
Expected: success.

- [ ] **Step 4: Run a graph and verify columns are populated**

```bash
cargo run --bin dag_engine -- run tests/graphs/basic/trigger.json
psql "$DATABASE_URL" -c "SELECT session_id, agent_session_id, parent_session_id FROM dag_runs ORDER BY updated_at DESC LIMIT 1;"
```
Expected: row exists; `agent_session_id` and `parent_session_id` are NULL (no agent passed).

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/persistence/postgres_dag_state_repository.rs
git commit -m "feat(state): persist agent_session_id and parent_session_id"
```

---

### Task 10: Implement `find_suspended_leaf` in Postgres repo

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/persistence/postgres_dag_state_repository.rs`

- [ ] **Step 1: Implement the method**

Replace the stub from Task 7 with:

```rust
async fn find_suspended_leaf(
    &self,
    agent_session_id: &str,
) -> Result<Option<String>, DagError> {
    let rows = sqlx::query(
        "SELECT session_id FROM dag_runs \
         WHERE agent_session_id = $1 \
           AND status = 'SUSPENDED' \
           AND session_id NOT IN ( \
               SELECT parent_session_id FROM dag_runs \
                WHERE agent_session_id = $1 AND parent_session_id IS NOT NULL \
                  AND status = 'SUSPENDED' \
           )"
    )
    .bind(agent_session_id)
    .fetch_all(&self.pool)
    .await
    .map_err(|e| DagError::StateError(format!("Database error on find_suspended_leaf: {}", e)))?;

    match rows.len() {
        0 => Ok(None),
        1 => {
            let sid: String = rows[0].get("session_id");
            Ok(Some(sid))
        }
        n => Err(DagError::StateError(format!(
            "Found {} concurrent suspended leaves for agent_session_id {} — concurrent leaves are not supported in this design",
            n, agent_session_id
        ))),
    }
}
```

- [ ] **Step 2: Compile**

Run: `cargo build`
Expected: success.

- [ ] **Step 3: Add an integration test**

Create `tests/find_suspended_leaf.rs`:

```rust
//! Smoke test: find_suspended_leaf returns the deepest SUSPENDED run for an agent.

use colmena::dag_engine::domain::state::{DagRunState, DagRunStatus, DagStateRepository};
use colmena::dag_engine::infrastructure::persistence::PostgresDagStateRepository;
use serde_json::json;
use std::collections::{HashMap, VecDeque};

fn fake_state(
    session_id: &str,
    agent: Option<&str>,
    parent: Option<&str>,
    status: DagRunStatus,
) -> DagRunState {
    DagRunState {
        session_id: session_id.to_string(),
        agent_session_id: agent.map(|s| s.to_string()),
        parent_session_id: parent.map(|s| s.to_string()),
        graph_json: json!({"nodes": {}, "edges": []}),
        all_outputs: HashMap::new(),
        status,
        global_shared_state: json!({}),
        active_queue: VecDeque::new(),
        execution_history: Vec::new(),
        global_calls: HashMap::new(),
        caller_specific_calls: HashMap::new(),
    }
}

#[tokio::test]
async fn finds_leaf_in_three_level_tree() {
    dotenvy::dotenv().ok();
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    let repo = PostgresDagStateRepository::new(pool);

    // Build root + sub + subsub all SUSPENDED for one chat.
    let chat = "test_chat_leaf_three";
    let root = format!("{}_root", chat);
    let sub = format!("{}_sub", chat);
    let subsub = format!("{}_subsub", chat);

    repo.save(&fake_state(&root, Some(chat), None, DagRunStatus::Suspended)).await.unwrap();
    repo.save(&fake_state(&sub,  Some(chat), Some(&root),    DagRunStatus::Suspended)).await.unwrap();
    repo.save(&fake_state(&subsub, Some(chat), Some(&sub),  DagRunStatus::Suspended)).await.unwrap();

    let leaf = repo.find_suspended_leaf(chat).await.unwrap();
    assert_eq!(leaf, Some(subsub.clone()));

    // Cleanup
    sqlx::query("DELETE FROM dag_runs WHERE agent_session_id = $1")
        .bind(chat).execute(&PostgresDagStateRepository::new(sqlx::PgPool::connect(&url).await.unwrap()).pool_for_test())
        .await.ok();
}
```

> If `pool_for_test()` doesn't exist on the repo, expose `pub(crate) fn pool(&self) -> &PgPool { &self.pool }` and use that. Add it as a tiny test helper.

- [ ] **Step 4: Add the test helper**

In `postgres_dag_state_repository.rs`, just below `pub fn new`, add:

```rust
#[cfg(test)]
pub(crate) fn pool_for_test(&self) -> &sqlx::PgPool {
    &self.pool
}
```

Adjust the test to call `repo.pool_for_test()` directly instead of constructing a second repo.

- [ ] **Step 5: Run the test**

Run: `cargo test --test find_suspended_leaf -- --test-threads=1`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/persistence/postgres_dag_state_repository.rs \
        tests/find_suspended_leaf.rs
git commit -m "feat(state): implement find_suspended_leaf with deepest-first semantics"
```

---

### Task 11: Update `PostgresConversationRepository` for new keying

**Files:**
- Modify: `src/libs/colmena/src/llm/infrastructure/persistence/postgres_conversation_repository.rs`

- [ ] **Step 1: Replace the implementation**

Replace the entire `impl ConversationRepository for PostgresConversationRepository` block:

```rust
#[async_trait]
impl ConversationRepository for PostgresConversationRepository {
    async fn get_by_id(&self, key: &ConversationKey) -> Result<Conversation, LlmError> {
        let rows = if let Some(agent) = &key.agent_session_id {
            sqlx::query(
                "SELECT role, content, tool_call_id, tool_calls, created_at \
                 FROM llm_node_history \
                 WHERE agent_session_id = $1 AND node_id = $2 \
                 ORDER BY created_at ASC"
            )
            .bind(&agent.0)
            .bind(&key.node_id.0)
            .fetch_all(&self.pool)
            .await
        } else {
            sqlx::query(
                "SELECT role, content, tool_call_id, tool_calls, created_at \
                 FROM llm_node_history \
                 WHERE session_id = $1 AND node_id = $2 \
                 ORDER BY created_at ASC"
            )
            .bind(&key.session_id.0)
            .bind(&key.node_id.0)
            .fetch_all(&self.pool)
            .await
        }
        .map_err(|e| LlmError::RequestFailed { message: format!("Database error: {}", e) })?;

        let messages = rows
            .into_iter()
            .map(|row| {
                let role_str: String = row.get("role");
                let content: String = row.get("content");
                let tool_call_id: Option<String> = row.get("tool_call_id");
                let tool_calls_json: Option<serde_json::Value> = row.get("tool_calls");
                let _created_at: DateTime<Utc> = row.get("created_at");

                let role = match role_str.as_str() {
                    "system" => MessageRole::System,
                    "user" => MessageRole::User,
                    "assistant" => MessageRole::Assistant,
                    "tool" => MessageRole::Tool,
                    _ => MessageRole::User,
                };

                match role {
                    MessageRole::System => LlmMessage::system(content).unwrap(),
                    MessageRole::User => LlmMessage::user(content).unwrap(),
                    MessageRole::Assistant => {
                        if let Some(tc_json) = tool_calls_json {
                            let tool_calls: Vec<crate::llm::domain::ToolCall> =
                                serde_json::from_value(tc_json).unwrap_or_default();
                            LlmMessage::assistant_with_tool_calls(content, tool_calls).unwrap()
                        } else {
                            LlmMessage::assistant(content).unwrap()
                        }
                    }
                    MessageRole::Tool => LlmMessage::tool(
                        tool_call_id.unwrap_or_else(|| "unknown".to_string()),
                        content,
                    )
                    .unwrap(),
                }
            })
            .collect();

        Ok(Conversation {
            key: key.clone(),
            messages,
        })
    }

    async fn add_message(
        &self,
        key: &ConversationKey,
        message: LlmMessage,
    ) -> Result<(), LlmError> {
        let role_str = match message.role() {
            MessageRole::System => "system",
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
            MessageRole::Tool => "tool",
        };

        let tool_calls_json = message
            .tool_calls()
            .and_then(|tc| serde_json::to_value(tc).ok());

        sqlx::query(
            "INSERT INTO llm_node_history (\
                session_id, agent_session_id, node_id, \
                role, content, tool_call_id, tool_calls, created_at\
             ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)"
        )
        .bind(&key.session_id.0)
        .bind(key.agent_session_id.as_ref().map(|a| a.0.clone()))
        .bind(&key.node_id.0)
        .bind(role_str)
        .bind(message.content())
        .bind(message.tool_call_id())
        .bind(tool_calls_json)
        .bind(Utc::now())
        .execute(&self.pool)
        .await
        .map_err(|e| LlmError::RequestFailed { message: format!("Database error: {}", e) })?;

        Ok(())
    }

    async fn delete(&self, key: &ConversationKey) -> Result<(), LlmError> {
        let res = if let Some(agent) = &key.agent_session_id {
            sqlx::query("DELETE FROM llm_node_history WHERE agent_session_id = $1 AND node_id = $2")
                .bind(&agent.0)
                .bind(&key.node_id.0)
                .execute(&self.pool)
                .await
        } else {
            sqlx::query("DELETE FROM llm_node_history WHERE session_id = $1 AND node_id = $2")
                .bind(&key.session_id.0)
                .bind(&key.node_id.0)
                .execute(&self.pool)
                .await
        };

        res.map_err(|e| LlmError::RequestFailed {
            message: format!("Database error: {}", e),
        })?;
        Ok(())
    }
}
```

Make sure the `use` block at the top imports `ConversationKey`, `AgentSessionId`, `NodeIdPath`:

```rust
use crate::llm::domain::{
    Conversation, ConversationKey, ConversationRepository, LlmError, LlmMessage, MessageRole,
};
```

- [ ] **Step 2: Compile**

Run: `cargo build`
Expected: success (one repo migrated; the other two will follow).

- [ ] **Step 3: Defer commit — Tasks 12 and 13 still need to update SQLite and in-memory impls; tree is broken until all three are migrated**

Move on to Task 12 with the working tree dirty.

---

### Task 12: Update `SqliteConversationRepository` for new keying

**Files:**
- Modify: `src/libs/colmena/src/llm/infrastructure/persistence/sqlite_conversation_repository.rs`

- [ ] **Step 1: Apply the same shape as Task 11 but with SQLite types**

Replace the impl, mirroring Task 11. Key differences:
- `?` placeholders instead of `$1`, `$2`, ...
- `tool_calls` column is `TEXT` (JSON-encoded string), so serialize/deserialize via `serde_json::to_string` and `from_str`.
- `created_at` is stored as `TEXT` (ISO-8601), so write `Utc::now().to_rfc3339()`.

```rust
use crate::llm::domain::{
    Conversation, ConversationKey, ConversationRepository, LlmError, LlmMessage, MessageRole,
};

use async_trait::async_trait;
use chrono::Utc;
use sqlx::{Row, SqlitePool};

pub struct SqliteConversationRepository {
    pool: SqlitePool,
}

impl SqliteConversationRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl ConversationRepository for SqliteConversationRepository {
    async fn get_by_id(&self, key: &ConversationKey) -> Result<Conversation, LlmError> {
        let rows = if let Some(agent) = &key.agent_session_id {
            sqlx::query(
                "SELECT role, content, tool_call_id, tool_calls, created_at \
                 FROM llm_node_history \
                 WHERE agent_session_id = ? AND node_id = ? \
                 ORDER BY created_at ASC"
            )
            .bind(&agent.0)
            .bind(&key.node_id.0)
            .fetch_all(&self.pool)
            .await
        } else {
            sqlx::query(
                "SELECT role, content, tool_call_id, tool_calls, created_at \
                 FROM llm_node_history \
                 WHERE session_id = ? AND node_id = ? \
                 ORDER BY created_at ASC"
            )
            .bind(&key.session_id.0)
            .bind(&key.node_id.0)
            .fetch_all(&self.pool)
            .await
        }
        .map_err(|e| LlmError::RequestFailed { message: format!("Database error: {}", e) })?;

        let messages = rows
            .into_iter()
            .map(|row| {
                let role_str: String = row.get("role");
                let content: String = row.get("content");
                let tool_call_id: Option<String> = row.get("tool_call_id");
                let tool_calls_str: Option<String> = row.get("tool_calls");
                let _created_at_str: String = row.get("created_at");

                let role = match role_str.as_str() {
                    "system" => MessageRole::System,
                    "user" => MessageRole::User,
                    "assistant" => MessageRole::Assistant,
                    "tool" => MessageRole::Tool,
                    _ => MessageRole::User,
                };

                match role {
                    MessageRole::System => LlmMessage::system(content).unwrap(),
                    MessageRole::User => LlmMessage::user(content).unwrap(),
                    MessageRole::Assistant => {
                        if let Some(tc_str) = tool_calls_str {
                            let tool_calls: Vec<crate::llm::domain::ToolCall> =
                                serde_json::from_str(&tc_str).unwrap_or_default();
                            LlmMessage::assistant_with_tool_calls(content, tool_calls).unwrap()
                        } else {
                            LlmMessage::assistant(content).unwrap()
                        }
                    }
                    MessageRole::Tool => LlmMessage::tool(
                        tool_call_id.unwrap_or_else(|| "unknown".to_string()),
                        content,
                    )
                    .unwrap(),
                }
            })
            .collect();

        Ok(Conversation {
            key: key.clone(),
            messages,
        })
    }

    async fn add_message(
        &self,
        key: &ConversationKey,
        message: LlmMessage,
    ) -> Result<(), LlmError> {
        let role_str = match message.role() {
            MessageRole::System => "system",
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
            MessageRole::Tool => "tool",
        };

        let tool_calls_str = message
            .tool_calls()
            .and_then(|tc| serde_json::to_string(tc).ok());

        let id = uuid::Uuid::new_v4().to_string();

        sqlx::query(
            "INSERT INTO llm_node_history (\
                id, session_id, agent_session_id, node_id, \
                role, content, tool_call_id, tool_calls, created_at\
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(&id)
        .bind(&key.session_id.0)
        .bind(key.agent_session_id.as_ref().map(|a| a.0.clone()))
        .bind(&key.node_id.0)
        .bind(role_str)
        .bind(message.content())
        .bind(message.tool_call_id())
        .bind(tool_calls_str)
        .bind(Utc::now().to_rfc3339())
        .execute(&self.pool)
        .await
        .map_err(|e| LlmError::RequestFailed { message: format!("Database error: {}", e) })?;

        Ok(())
    }

    async fn delete(&self, key: &ConversationKey) -> Result<(), LlmError> {
        let res = if let Some(agent) = &key.agent_session_id {
            sqlx::query("DELETE FROM llm_node_history WHERE agent_session_id = ? AND node_id = ?")
                .bind(&agent.0)
                .bind(&key.node_id.0)
                .execute(&self.pool)
                .await
        } else {
            sqlx::query("DELETE FROM llm_node_history WHERE session_id = ? AND node_id = ?")
                .bind(&key.session_id.0)
                .bind(&key.node_id.0)
                .execute(&self.pool)
                .await
        };

        res.map_err(|e| LlmError::RequestFailed {
            message: format!("Database error: {}", e),
        })?;
        Ok(())
    }
}
```

- [ ] **Step 2: Compile**

Run: `cargo build`
Expected: success.

- [ ] **Step 3: Defer commit — Task 13 will land in-memory repo; commit happens at the end of Task 13 covering all three impls**

Move on to Task 13.

---

### Task 13: Update `InMemoryConversationRepository` for new keying

**Files:**
- Modify: `src/libs/colmena/src/llm/infrastructure/persistence/in_memory_conversation_repository.rs`

- [ ] **Step 1: Read the existing file first** (it's small).

Run: `wc -l src/libs/colmena/src/llm/infrastructure/persistence/in_memory_conversation_repository.rs`

- [ ] **Step 2: Replace the storage shape**

The in-memory repo today keys by `SessionId`. Switch to keying by a tuple `(Option<String>, String, String)` representing `(agent_session_id, session_id, node_id_path)`. The lookup logic mirrors the SQL path: prefer agent_session_id when present.

```rust
use crate::llm::domain::{
    AgentSessionId, Conversation, ConversationKey, ConversationRepository, LlmError, LlmMessage,
    NodeIdPath, SessionId,
};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Default)]
pub struct InMemoryConversationRepository {
    /// Storage indexed by (agent_session_id_or_session_id, node_id) — the
    /// effective key under which messages were appended.
    inner: Mutex<HashMap<(String, String), Vec<LlmMessage>>>,
}

impl InMemoryConversationRepository {
    pub fn new() -> Self {
        Self::default()
    }

    fn lookup_key(key: &ConversationKey) -> (String, String) {
        let id = match &key.agent_session_id {
            Some(a) => a.0.clone(),
            None => key.session_id.0.clone(),
        };
        (id, key.node_id.0.clone())
    }
}

#[async_trait]
impl ConversationRepository for InMemoryConversationRepository {
    async fn get_by_id(&self, key: &ConversationKey) -> Result<Conversation, LlmError> {
        let map = self.inner.lock().unwrap();
        let messages = map.get(&Self::lookup_key(key)).cloned().unwrap_or_default();
        Ok(Conversation {
            key: key.clone(),
            messages,
        })
    }

    async fn add_message(
        &self,
        key: &ConversationKey,
        message: LlmMessage,
    ) -> Result<(), LlmError> {
        let mut map = self.inner.lock().unwrap();
        map.entry(Self::lookup_key(key)).or_default().push(message);
        Ok(())
    }

    async fn delete(&self, key: &ConversationKey) -> Result<(), LlmError> {
        let mut map = self.inner.lock().unwrap();
        map.remove(&Self::lookup_key(key));
        Ok(())
    }
}
```

- [ ] **Step 3: Add a unit test in the same file**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::domain::{AgentSessionId, NodeIdPath, SessionId};

    fn k(agent: Option<&str>, session: &str, node: &str) -> ConversationKey {
        ConversationKey {
            session_id: SessionId(session.to_string()),
            agent_session_id: agent.map(|a| AgentSessionId(a.to_string())),
            node_id: NodeIdPath(node.to_string()),
        }
    }

    #[tokio::test]
    async fn agent_keying_isolates_two_runs_under_same_chat() {
        let repo = InMemoryConversationRepository::new();
        let k1 = k(Some("chat_x"), "run_1", "router");
        let k2 = k(Some("chat_x"), "run_2", "router");

        repo.add_message(&k1, LlmMessage::user("hi from run 1".into()).unwrap()).await.unwrap();
        let conv = repo.get_by_id(&k2).await.unwrap();
        assert_eq!(conv.messages.len(), 1, "agent keying should let run 2 see run 1's history");
    }

    #[tokio::test]
    async fn legacy_keying_does_not_cross_runs() {
        let repo = InMemoryConversationRepository::new();
        let k1 = k(None, "run_1", "router");
        let k2 = k(None, "run_2", "router");

        repo.add_message(&k1, LlmMessage::user("hi from run 1".into()).unwrap()).await.unwrap();
        let conv = repo.get_by_id(&k2).await.unwrap();
        assert!(conv.messages.is_empty(), "legacy keying must not leak across runs");
    }

    #[tokio::test]
    async fn node_id_isolates_two_llm_calls_in_same_run() {
        let repo = InMemoryConversationRepository::new();
        let router = k(Some("chat_x"), "run_1", "router");
        let responder = k(Some("chat_x"), "run_1", "responder");

        repo.add_message(&router, LlmMessage::user("router only".into()).unwrap()).await.unwrap();
        let conv = repo.get_by_id(&responder).await.unwrap();
        assert!(conv.messages.is_empty(), "responder must not see router's history");
    }
}
```

- [ ] **Step 4: Run the unit tests**

Run: `cargo test --lib --package colmena_dag_engine in_memory_conversation_repository`
Expected: 3 PASS.

- [ ] **Step 5: Compile to confirm the tree is green again**

Run: `cargo build`
Expected: success.

- [ ] **Step 6: Commit Tasks 8 + 11 + 12 + 13 together (the trait-and-impls migration)**

```bash
git add src/libs/colmena/src/llm/domain/ \
        src/libs/colmena/src/llm/infrastructure/persistence/postgres_conversation_repository.rs \
        src/libs/colmena/src/llm/infrastructure/persistence/sqlite_conversation_repository.rs \
        src/libs/colmena/src/llm/infrastructure/persistence/in_memory_conversation_repository.rs
git commit -m "feat(llm): migrate ConversationRepository to ConversationKey

The trait now takes a ConversationKey carrying an optional agent_session_id
and a required node_id, replacing the bare SessionId. All three impls
(Postgres, SQLite, in-memory) honor the new keying:

- Reads filter by (agent_session_id, node_id) when an agent is present;
  fall back to (session_id, node_id) for legacy callers.
- Writes always include session_id, node_id, and agent_session_id when set.

In-memory repo gains unit tests for cross-run continuity by agent,
legacy isolation by session_id, and per-node isolation."
```

---

## Phase 4 — Engine internals (paths + injections)

### Task 14: Compute path-qualified `node_id` and inject `__colmena_node_id_path`

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/application/run_use_case.rs:184-352`
- Modify: `src/libs/colmena/src/dag_engine/engine.rs:142-155`

- [ ] **Step 1: Extend `execute_stream` signature with `path_prefix`**

In `run_use_case.rs:184`:

```rust
pub fn execute_stream(
    self,
    graph: Graph,
    resume_session_id: Option<String>,
    resume_answer: Option<String>,
    include_extra_info: bool,
    /// Path prefix injected by the parent subgraph node, if any.
    /// For root runs this is `None` and node_id_path = node_id.
    path_prefix: Option<String>,
    /// Conversation handle. `None` for legacy runs.
    agent_session_id: Option<String>,
) -> impl futures::Stream<...>
```

- [ ] **Step 2: Compute `node_id_path` per node and inject it**

In `run_use_case.rs:351-352`, where `__colmena_session_id` is injected, add:

```rust
let node_id_path = match &path_prefix {
    Some(prefix) => format!("{}/{}", prefix, node_id),
    None => node_id.clone(),
};

inputs.insert("__colmena_session_id".to_string(), Value::String(session_id.clone()));
inputs.insert("__node_id".to_string(), Value::String(node_id.clone()));
inputs.insert("__colmena_node_id_path".to_string(), Value::String(node_id_path.clone()));
```

- [ ] **Step 3: Update `engine.rs::execute_stream` to forward both new args**

```rust
pub fn execute_stream(
    &self,
    graph: Graph,
    resume_session_id: Option<String>,
    resume_answer: Option<String>,
    include_extra_info: bool,
    path_prefix: Option<String>,
    agent_session_id: Option<String>,
) -> impl Stream<Item = Result<DagExecutionEvent, DagError>> + Send + '_ {
    (*self.use_case).clone().execute_stream(
        graph,
        resume_session_id,
        resume_answer,
        include_extra_info,
        path_prefix,
        agent_session_id,
    )
}
```

- [ ] **Step 4: Fix the four call sites that break**

```bash
grep -rn "execute_stream(" src/libs/colmena/src/ tests/ | grep -v "fn execute_stream"
```

For every call, default the new args to `None, None`:
- `main.rs:87` — pass `None, session_id.clone()` for path/agent (CLI not yet hooked; agent stays None for now)
- `api.rs:74-79` and `api.rs:282-283` and `api.rs:585-590` — `None, None`
- `run_use_case.rs:818-820` and `run_use_case.rs:865-870` (subgraph paths) — `None, None` for now (Task 19 will pass the real values)

- [ ] **Step 5: Compile**

Run: `cargo build`
Expected: success.

- [ ] **Step 6: Run an existing graph to confirm no regression**

```bash
source .env
cargo run --bin dag_engine -- run tests/graphs/basic/trigger.json
```
Expected: completes normally.

- [ ] **Step 7: Commit**

```bash
git add src/libs/colmena/src/dag_engine/
git commit -m "feat(engine): inject path-qualified node_id and accept agent_session_id

Adds two parameters to execute_stream: path_prefix (for subgraph nesting)
and agent_session_id (chat handle). Both default to None so callers that
don't care about the new feature pass None and behavior is unchanged."
```

---

### Task 15: Inject `__colmena_agent_session_id`

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/application/run_use_case.rs` (around the same place as Task 14)

- [ ] **Step 1: Add the injection alongside the others**

After the line that inserts `__colmena_node_id_path`:

```rust
inputs.insert(
    "__colmena_agent_session_id".to_string(),
    match &agent_session_id {
        Some(a) => Value::String(a.clone()),
        None => Value::Null,
    },
);
```

> Always inserting the key (even as Null) gives nodes a stable shape: `inputs.get("__colmena_agent_session_id")` always returns `Some(_)`, but `as_str()` returns `None` when no chat handle was passed.

- [ ] **Step 2: Compile**

Run: `cargo build`
Expected: success.

- [ ] **Step 3: Run an existing graph and confirm no regression**

```bash
cargo run --bin dag_engine -- run tests/graphs/basic/trigger.json
```
Expected: completes normally.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/dag_engine/application/run_use_case.rs
git commit -m "feat(engine): inject __colmena_agent_session_id into every node"
```

---

### Task 16: (folded into Tasks 14–15 — no separate task)

Tasks 14 and 15 together extended the `execute_stream` signature. This row in the summary table is intentionally folded; no additional work required.

---

## Phase 5 — LLM node consumes new keys

### Task 17: `llm.rs` switches keying based on `agent_session_id`

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs:395-401`

- [ ] **Step 1: Replace the session resolution**

In `llm.rs` find the block that resolves `session_id` (around line 395). Replace with:

```rust
// Conversation handle (NEW): provided by the engine; may be JSON null.
let agent_session_id_str: Option<String> = inputs
    .get("__colmena_agent_session_id")
    .and_then(|v| v.as_str())
    .filter(|s| !s.is_empty())
    .map(|s| s.to_string());

// Run-scoped session id (always present).
let session_id_str = inputs
    .get("__colmena_session_id")
    .and_then(|v| v.as_str())
    .ok_or("missing __colmena_session_id")?
    .to_string();

// Path-qualified node id (engine-injected).
let node_id_path_str = inputs
    .get("__colmena_node_id_path")
    .and_then(|v| v.as_str())
    .ok_or("missing __colmena_node_id_path")?
    .to_string();

// Effective conversation key for memory operations. Bound to the lifetime
// of `agent_session_id_str` / `session_id_str` / `node_id_path_str` above.
let conversation_key = crate::llm::domain::ConversationKey {
    session_id: crate::llm::domain::SessionId(session_id_str.clone()),
    agent_session_id: agent_session_id_str
        .as_ref()
        .map(|a| crate::llm::domain::AgentSessionId(a.clone())),
    node_id: crate::llm::domain::NodeIdPath(node_id_path_str.clone()),
};

// Connection URL (Optional - for Memory Backend) — unchanged from before.
let connection_url_raw = inputs
    .get("connection_url")
    .and_then(|v| v.as_str())
    .or_else(|| config.get("connection_url").and_then(|v| v.as_str()));
```

- [ ] **Step 2: Update every site that called `repo.get_by_id(&SessionId(...))` or `repo.add_message(&SessionId(...), ...)`**

Search for those call sites within `llm.rs`:

```bash
grep -n "get_by_id\|add_message\|delete" src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs
```

For each, swap the `&SessionId(...)` argument for `&conversation_key`. The trait now takes `&ConversationKey`.

- [ ] **Step 3: Update `agent_service.rs` and any other internal LLM users**

```bash
grep -rn "get_by_id\|add_message\|ConversationRepository" src/libs/colmena/src/llm/application/
```

Adapt every caller to construct a `ConversationKey`. For internal helpers not tied to a specific DAG node (e.g. `agent_service.rs`), use a stable string identifier reflecting the caller, such as:

```rust
ConversationKey {
    session_id: SessionId(session_id.to_string()),
    agent_session_id: None,                              // internal callers don't carry chat handles
    node_id: NodeIdPath("agent_service".to_string()),    // stable per internal caller
}
```

Pick a different string per caller if multiple internal helpers touch the same `session_id`, so their histories don't interleave.

- [ ] **Step 4: Compile**

Run: `cargo build`
Expected: success.

- [ ] **Step 5: Run a graph that uses memory and confirm history reads/writes work**

```bash
source .env
cargo run --bin dag_engine -- run tests/graphs/memory/memory_sqlite_example.json
```
Expected: completes normally; history persists.

- [ ] **Step 6: Run unit + integration tests**

Run: `cargo test --lib --package colmena_dag_engine`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs \
        src/libs/colmena/src/llm/application/
git commit -m "feat(llm): keying by ConversationKey

llm_call now reads memory by (agent_session_id, node_id) when an agent
session is present; falls back to (session_id, node_id) otherwise.
This eliminates the silent collision when multiple llm_call nodes share
a run and enables cross-run memory continuity for chats."
```

---

## Phase 6 — Subgraph propagation

### Task 18: Extend `SubGraphExecutorPort`

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/application/ports.rs:34-51`

- [ ] **Step 1: Add three new args to both methods**

```rust
#[async_trait::async_trait]
pub trait SubGraphExecutorPort: Send + Sync {
    /// Ejecuta un subgrafo desde cero.
    async fn run_subgraph(
        &self,
        session_id: &str,
        graph_json: Value,
        global_state: Value,
        observer: Option<Arc<dyn crate::dag_engine::domain::observer::ExecutionObserver>>,
        parent_session_id: Option<String>,
        agent_session_id: Option<String>,
        path_prefix: Option<String>,
    ) -> Result<Value, DagError>;

    /// Reanuda un subgrafo suspendido tras un Human-in-the-Loop.
    async fn resume_subgraph(
        &self,
        session_id: &str,
        answer: String,
        observer: Option<Arc<dyn crate::dag_engine::domain::observer::ExecutionObserver>>,
        agent_session_id: Option<String>,
        path_prefix: Option<String>,
    ) -> Result<Value, DagError>;
}
```

- [ ] **Step 2: Update the impl in `run_use_case.rs:787-895`**

Use the new args:
- In `run_subgraph`: when constructing the initial `DagRunState`, set `agent_session_id`, `parent_session_id`. Forward `path_prefix` and `agent_session_id` to `execute_stream`.
- In `resume_subgraph`: forward `agent_session_id` and `path_prefix` to `execute_stream`.

```rust
async fn run_subgraph(
    &self,
    session_id: &str,
    graph_json: Value,
    global_state: Value,
    observer: Option<Arc<dyn crate::dag_engine::domain::observer::ExecutionObserver>>,
    parent_session_id: Option<String>,
    agent_session_id: Option<String>,
    path_prefix: Option<String>,
) -> Result<Value, DagError> {
    let graph: Graph = serde_json::from_value(graph_json.clone())
        .map_err(|e| DagError::NodeExecution(format!("Invalid sub-graph JSON: {}", e)))?;
    graph
        .validate()
        .map_err(|e| DagError::NodeExecution(format!("Invalid sub-graph: {}", e)))?;

    if let Some(repo) = &self.state_repository {
        let initial_state = DagRunState {
            session_id: session_id.to_string(),
            agent_session_id: agent_session_id.clone(),
            parent_session_id: parent_session_id.clone(),
            graph_json,
            all_outputs: HashMap::new(),
            global_shared_state: global_state,
            execution_history: Vec::new(),
            global_calls: HashMap::new(),
            caller_specific_calls: HashMap::new(),
            active_queue: VecDeque::new(),
            status: DagRunStatus::Running,
        };
        repo.save(&initial_state).await?;
    }

    use futures::StreamExt;
    let mut stream = Box::pin(self.clone().execute_stream(
        graph,
        Some(session_id.to_string()),
        None,
        true,
        path_prefix,
        agent_session_id,
    ));

    // ... (rest unchanged, drain the stream into `final_out`)
```

For `resume_subgraph`:

```rust
async fn resume_subgraph(
    &self,
    session_id: &str,
    answer: String,
    observer: Option<Arc<dyn crate::dag_engine::domain::observer::ExecutionObserver>>,
    agent_session_id: Option<String>,
    path_prefix: Option<String>,
) -> Result<Value, DagError> {
    let state = if let Some(repo) = &self.state_repository {
        repo.get_by_id(session_id).await?.ok_or_else(|| {
            DagError::NodeExecution(format!("Child session {} not found for resume", session_id))
        })?
    } else {
        return Err(DagError::NodeExecution("State repository missing for resume".to_string()));
    };

    let graph: Graph = serde_json::from_value(state.graph_json)
        .map_err(|e| DagError::NodeExecution(format!("Invalid sub-graph state JSON: {}", e)))?;

    use futures::StreamExt;
    let mut stream = Box::pin(self.clone().execute_stream(
        graph,
        Some(session_id.to_string()),
        Some(answer),
        true,
        path_prefix,
        agent_session_id.or(state.agent_session_id),
    ));
    // ... (rest unchanged)
```

- [ ] **Step 3: Compile**

Run: `cargo build`
Expected: callsite errors in `subgraph.rs` (Task 19) — leave them for the next task.

- [ ] **Step 4: Defer commit until Task 19 is green**

The tree does not compile until `subgraph.rs` (Task 19) consumes the new port surface. **Do NOT commit yet.** The Task 18 + Task 19 changes commit together at the end of Task 19.

---

### Task 19: `SubGraphNode` uses UUID, propagates parent + agent + path

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/subgraph.rs:38-185`

- [ ] **Step 1: Replace child id derivation and propagation**

In `subgraph.rs` `execute`:

```rust
async fn execute(
    &self,
    inputs: &NodeInputs,
    config: &Value,
    _global_state: &mut Value,
    _observer: Option<Arc<dyn ExecutionObserver>>,
) -> Result<Value, Box<dyn Error + Send + Sync>> {
    let parent_session_id = inputs
        .get("__colmena_session_id")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown_parent")
        .to_string();

    let agent_session_id = inputs
        .get("__colmena_agent_session_id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());

    let parent_path = inputs
        .get("__colmena_node_id_path")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    // The subgraph node's *own* path is what its children must inherit.
    // (parent_path is already the qualified id of THIS subgraph node.)
    let child_path_prefix = if parent_path.is_empty() {
        None
    } else {
        Some(parent_path.clone())
    };

    // RESUME PATH — find the existing child run.
    if let Some(resume_answer) = inputs
        .get("__colmena_resume_answer")
        .and_then(|v| v.as_str())
    {
        // We need the child_session_id stored from the previous run. With UUIDs
        // we can't derive it; we read it from a state field we wrote on first run.
        // Convention: we stash the child id under config["__child_session_id"]
        // at first invocation (set by the engine in all_outputs). Look it up via
        // the parent session's all_outputs.
        //
        // ALTERNATIVE (chosen here to keep this self-contained): query the
        // dag_runs table for the SUSPENDED child whose parent_session_id equals
        // our parent_session_id and whose path qualifier matches ours.
        let executor = self
            .executor
            .get()
            .ok_or("SubGraphExecutorPort not initialized in SubGraphNode")?;

        // The find-by-parent path requires repository access from the executor.
        // We extend the port in step below.
        let child_session_id = executor
            .find_child_session_id_for_resume(&parent_session_id, &parent_path)
            .await?
            .ok_or_else(|| format!("No suspended child found under parent {} / path {}", parent_session_id, parent_path))?;

        colmena_log!(
            "▶️ [SubGraphNode] Resuming child graph {} (path={}) with answer...",
            child_session_id, parent_path
        );
        let result = executor
            .resume_subgraph(
                &child_session_id,
                resume_answer.to_string(),
                _observer.clone(),
                agent_session_id.clone(),
                child_path_prefix.clone(),
            )
            .await?;

        if result.get("__colmena_status").and_then(|v| v.as_str()) == Some("SUSPENDED") {
            return Ok(result);
        }
        return Ok(result);
    }

    // FRESH RUN — generate a new UUID for the child session.
    let child_session_id = uuid::Uuid::new_v4().to_string();

    let graph_json = if let Some(inline) = config.get("child_graph_inline") {
        inline.clone()
    } else if let Some(path_val) = config.get("child_graph_path").and_then(|v| v.as_str()) {
        let path = std::path::Path::new(path_val);
        if !path.exists() {
            return Err(format!("child_graph_path not found: {}", path_val).into());
        }
        let contents = fs::read_to_string(path).await?;
        serde_json::from_str(&contents)?
    } else {
        return Err(
            "SubGraphNode requires 'child_graph_inline' or 'child_graph_path' in config".into(),
        );
    };

    let agent_name = config
        .get("__agent_name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let mut child_state_obj = serde_json::Map::new();
    for (k, v) in inputs {
        if !k.starts_with("__colmena_") && k != "__node_id" {
            child_state_obj.insert(k.clone(), v.clone());
        }
    }
    let child_state = Value::Object(child_state_obj);

    if let (Some(ref name), Some(ref obs)) = (&agent_name, &_observer) {
        let start_event = DagExecutionEvent::NodeStart {
            node_id: name.clone(),
            node_type: "subgraph".to_string(),
            inputs: Value::Object(Default::default()),
            config: Value::Object(Default::default()),
        };
        if let Ok(raw) = serde_json::to_value(&start_event) {
            obs.on_event(NodeEvent::SubgraphChildEvent(raw));
        }
    }

    colmena_log!(
        "🔄 [SubGraphNode] Running SubGraph in isolated session: {} (path_prefix={:?})",
        child_session_id, child_path_prefix
    );

    let result = self
        .executor
        .get()
        .ok_or("SubGraphExecutorPort not initialized in SubGraphNode")?
        .run_subgraph(
            &child_session_id,
            graph_json,
            child_state,
            _observer.clone(),
            Some(parent_session_id.clone()),
            agent_session_id.clone(),
            child_path_prefix.clone(),
        )
        .await?;

    if result.get("__colmena_status").and_then(|v| v.as_str()) == Some("SUSPENDED") {
        colmena_log!("⏸️ [SubGraphNode] Child graph suspended! Bubbling up to parent...");
        return Ok(result);
    }

    let final_output = if let Some(obj) = result.as_object() {
        obj.values()
            .find(|v| {
                v.get("extra_info")
                    .and_then(|ei| ei.get("__colmena_is_output_node"))
                    .and_then(|f| f.as_bool())
                    .unwrap_or(false)
            })
            .cloned()
            .unwrap_or(result.clone())
    } else {
        result.clone()
    };

    if let (Some(ref name), Some(ref obs)) = (&agent_name, &_observer) {
        let finish_event = DagExecutionEvent::SubgraphNodeFinish {
            node_id: name.clone(),
            output: final_output.clone(),
        };
        if let Ok(raw) = serde_json::to_value(&finish_event) {
            obs.on_event(NodeEvent::SubgraphChildEvent(raw));
        }
    }

    Ok(final_output)
}
```

- [ ] **Step 2: Add `find_child_session_id_for_resume` to `SubGraphExecutorPort`**

In `ports.rs`:

```rust
/// Finds the SUSPENDED child run whose parent_session_id matches and whose
/// node_id_path begins with `parent_node_path`. Returns the child's session_id.
async fn find_child_session_id_for_resume(
    &self,
    parent_session_id: &str,
    parent_node_path: &str,
) -> Result<Option<String>, DagError>;
```

- [ ] **Step 3: Implement it in `run_use_case.rs`**

Add a new method on `DagStateRepository` first (state.rs):

```rust
/// Returns the session_id of a child SUSPENDED run whose parent_session_id
/// matches the input. There must be at most one such child per parent path
/// in the chosen design (single-leaf-at-a-time).
async fn find_suspended_child(
    &self,
    parent_session_id: &str,
) -> Result<Option<String>, DagError>;
```

Implement in `PostgresDagStateRepository`:

```rust
async fn find_suspended_child(
    &self,
    parent_session_id: &str,
) -> Result<Option<String>, DagError> {
    let row_opt = sqlx::query(
        "SELECT session_id FROM dag_runs \
         WHERE parent_session_id = $1 AND status = 'SUSPENDED' \
         ORDER BY updated_at DESC LIMIT 1"
    )
    .bind(parent_session_id)
    .fetch_optional(&self.pool)
    .await
    .map_err(|e| DagError::StateError(format!("Database error on find_suspended_child: {}", e)))?;

    Ok(row_opt.map(|r| r.get::<String, _>("session_id")))
}
```

Now in `DagRunUseCase` (impl `SubGraphExecutorPort`):

```rust
async fn find_child_session_id_for_resume(
    &self,
    parent_session_id: &str,
    _parent_node_path: &str,
) -> Result<Option<String>, DagError> {
    if let Some(repo) = &self.state_repository {
        repo.find_suspended_child(parent_session_id).await
    } else {
        Ok(None)
    }
}
```

> The `_parent_node_path` arg is reserved for future use (when subgraphs may have multiple suspended children — currently single-leaf scope means parent_session_id is enough).

- [ ] **Step 4: Add the trait stub to other DagStateRepository impls**

```bash
grep -rn "impl DagStateRepository" src/libs/colmena/src/
```

Add stubs returning `Ok(None)` to keep them compiling.

- [ ] **Step 5: Compile and run a memory graph**

Run: `cargo build && cargo run --bin dag_engine -- run tests/graphs/memory/memory_sqlite_example.json`
Expected: success.

- [ ] **Step 6: Run a subgraph graph**

Search for an existing subgraph test:
```bash
ls tests/graphs/agents/ | grep -i subgraph
```
Run one of them. Expected: success.

- [ ] **Step 7: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/subgraph.rs \
        src/libs/colmena/src/dag_engine/application/ports.rs \
        src/libs/colmena/src/dag_engine/application/run_use_case.rs \
        src/libs/colmena/src/dag_engine/domain/state.rs \
        src/libs/colmena/src/dag_engine/infrastructure/persistence/postgres_dag_state_repository.rs
git commit -m "feat(subgraph): UUID children, parent_session_id linkage, path propagation

Subgraph children get a fresh UUID instead of {parent}_sub_{node} and
write parent_session_id explicitly. The subgraph node propagates
agent_session_id and the path prefix down to the child execution stream.
Resume now looks up the child by parent_session_id (single-leaf design)."
```

---

## Phase 7 — Lifecycle decision logic

### Task 20: Implement the three-branch lifecycle in `execute_stream`

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/application/run_use_case.rs:193-225`

- [ ] **Step 1: Replace the context loader with the new decision tree**

Find the block that begins with `let mut session_id = uuid::Uuid::new_v4().to_string();` and the `Context loader` comment. Replace through the end of that load block:

```rust
let mut session_id = uuid::Uuid::new_v4().to_string();
let mut active_agent_session_id: Option<String> = agent_session_id.clone();
let mut parent_session_id_for_save: Option<String> = None;

// ── Lifecycle decision (spec §4.1) ─────────────────────────────────────
//
// Branch 1: explicit session_id provided → direct resume.
// Branch 2: only agent_session_id provided → search for SUSPENDED leaf,
//           else fresh root run under that chat.
// Branch 3: neither → legacy fresh-run path.
//
match (&resume_session_id, &agent_session_id) {
    (Some(id), maybe_agent) => {
        // Branch 1: direct resume by run UUID. (Sub-branch when both args
        // present and conflict — see conflict check below.)
        if let Some(repo) = &self.state_repository {
            if let Some(state) = repo.get_by_id(id).await? {
                // Conflict check: if caller passed an agent_session_id and the
                // stored row has a different one, fail.
                if let (Some(passed), Some(stored)) =
                    (maybe_agent, &state.agent_session_id)
                {
                    if passed != stored {
                        Err(DagError::NodeExecution(format!(
                            "session_id {} belongs to agent_session_id {} but caller passed {}",
                            id, stored, passed
                        )))?;
                    }
                }

                all_outputs = state.all_outputs;
                active_queue = state.active_queue;
                session_id = state.session_id;
                execution_history = state.execution_history;
                global_calls = state.global_calls;
                caller_specific_calls = state.caller_specific_calls;
                global_shared_state = state.global_shared_state;
                active_agent_session_id = state.agent_session_id;
                parent_session_id_for_save = state.parent_session_id;
            } else {
                // Row not found — caller knows the id but it's not in the table.
                // Treat as fresh start with that id.
                session_id = id.clone();
                active_agent_session_id = maybe_agent.clone();
            }
        } else {
            session_id = id.clone();
            active_agent_session_id = maybe_agent.clone();
        }
    }
    (None, Some(agent)) => {
        // Branch 2: resolve by chat handle.
        if let Some(repo) = &self.state_repository {
            match repo.find_suspended_leaf(agent).await? {
                Some(leaf_id) => {
                    // Resume the leaf.
                    if let Some(state) = repo.get_by_id(&leaf_id).await? {
                        all_outputs = state.all_outputs;
                        active_queue = state.active_queue;
                        session_id = state.session_id;
                        execution_history = state.execution_history;
                        global_calls = state.global_calls;
                        caller_specific_calls = state.caller_specific_calls;
                        global_shared_state = state.global_shared_state;
                        active_agent_session_id = state.agent_session_id;
                        parent_session_id_for_save = state.parent_session_id;
                    }
                }
                None => {
                    // No suspended leaf — fresh root run under this chat.
                    // session_id stays as a new UUID; agent_session_id propagates.
                    active_agent_session_id = Some(agent.clone());
                }
            }
        } else {
            active_agent_session_id = Some(agent.clone());
        }
    }
    (None, None) => {
        // Branch 3: pure legacy. session_id stays as a new UUID;
        // agent_session_id remains None.
    }
}
```

- [ ] **Step 2: Update every `repo.save(&state)` call inside `execute_stream` to set the new fields**

Search for `DagRunState {` constructions inside `execute_stream`. Each must include:

```rust
agent_session_id: active_agent_session_id.clone(),
parent_session_id: parent_session_id_for_save.clone(),
```

- [ ] **Step 3: Pass the resolved agent_session_id into the per-node injection**

Replace the line from Task 15 (`inputs.insert("__colmena_agent_session_id"...)`) so it reads from `active_agent_session_id` (the resolved value) instead of the unresolved `agent_session_id` parameter.

- [ ] **Step 4: Compile**

Run: `cargo build`
Expected: success.

- [ ] **Step 5: Run all existing memory tests to confirm no regression**

```bash
source .env
cargo run --bin dag_engine -- run tests/graphs/memory/memory_sqlite_example.json
cargo test --lib --package colmena_dag_engine
```
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src/libs/colmena/src/dag_engine/application/run_use_case.rs
git commit -m "feat(engine): lifecycle decision branches for agent_session_id

Implements the three-branch logic from spec §4.1:
- session_id given → direct resume (with agent conflict check)
- agent_session_id only → find SUSPENDED leaf, else fresh root run
- neither → legacy fresh run"
```

---

### Task 21: (folded into Task 20 — conflict check is included)

The conflict check between an explicit `session_id` and a non-matching `agent_session_id` was implemented as part of Task 20 Branch 1. No separate task needed.

---

## Phase 8 — API surface

### Task 22: CLI flag `--agent-session-id`

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/main.rs:14-25, 51-87`

- [ ] **Step 1: Add the flag to the `Run` enum variant**

```rust
Run {
    file_path: String,
    #[arg(long, alias = "resume-id")]
    session_id: Option<String>,
    #[arg(long)]
    agent_session_id: Option<String>,
    #[arg(long)]
    answer: Option<String>,
    #[arg(long, default_value_t = false)]
    include_extra_info: bool,
    #[arg(long, default_value_t = false)]
    verbose: bool,
},
```

- [ ] **Step 2: Forward it into `execute_stream`**

In the `Run` arm match, replace the `execute_stream` call with:

```rust
let s = engine.execute_stream(
    graph,
    session_id.clone(),
    answer,
    include_extra_info,
    None,                   // path_prefix (root run)
    agent_session_id.clone(),
);
```

And update the variant destructure to include `agent_session_id`.

- [ ] **Step 3: Compile and try**

```bash
cargo build
cargo run --bin dag_engine -- run tests/graphs/basic/trigger.json --agent-session-id chat_test
psql "$DATABASE_URL" -c "SELECT session_id, agent_session_id FROM dag_runs WHERE agent_session_id = 'chat_test';"
```
Expected: row exists with `agent_session_id = 'chat_test'`.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/dag_engine/main.rs
git commit -m "feat(cli): add --agent-session-id flag"
```

---

### Task 23: HTTP `X-Agent-Session-Id` header / body field

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/api.rs:14-19, 200-553, 555-652`

- [ ] **Step 1: Update `run_dag` signature**

```rust
pub async fn run_dag(
    file_path: String,
    resume_id: Option<String>,
    resume_answer: Option<String>,
    inject_payload: Option<Value>,
    include_extra_info: bool,
    agent_session_id: Option<String>,
) -> Result<Value, Box<dyn std::error::Error>> { ... }
```

Inside, replace the `engine.execute_stream(...)` call (line ~74) with:

```rust
let internal_stream = engine.execute_stream(
    graph,
    resume_id.clone(),
    resume_answer.clone(),
    include_extra_info,
    None,
    agent_session_id.clone(),
);
```

- [ ] **Step 2: Read the header in `handler_webhook`**

In `handler_webhook` (after the SSE detection block, before `engine.execute_stream`):

```rust
let agent_session_id_header = headers
    .get("x-agent-session-id")
    .and_then(|v| v.to_str().ok())
    .map(|s| s.to_string());

let agent_session_id_body = payload
    .as_object()
    .and_then(|o| o.get("agent_session_id"))
    .and_then(|v| v.as_str())
    .map(|s| s.to_string());

let agent_session_id = agent_session_id_header.or(agent_session_id_body);
```

Forward it into all `engine.execute_stream(...)` calls inside this handler — there are 2 (the SSE branch and the buffered branch). Replace `None` for `agent_session_id` with `agent_session_id.clone()`.

- [ ] **Step 3: Read the header in `handler_resume`**

```rust
let agent_session_id = headers
    .get("x-agent-session-id")
    .and_then(|v| v.to_str().ok())
    .map(|s| s.to_string());
```

Forward into the `engine.execute_stream(...)` calls (SSE + buffered).

- [ ] **Step 4: Compile**

Run: `cargo build`
Expected: success.

- [ ] **Step 5: Manual smoke test**

```bash
cargo run --bin dag_engine -- serve tests/graphs/agents/llm_call.json &
sleep 2
curl -H "X-Agent-Session-Id: chat_http_test" \
     -H "Content-Type: application/json" \
     -d '{"prompt": "hello"}' \
     http://localhost:3000/webhook
psql "$DATABASE_URL" -c "SELECT session_id, agent_session_id FROM dag_runs WHERE agent_session_id = 'chat_http_test';"
kill %1
```
Expected: row exists.

- [ ] **Step 6: Commit**

```bash
git add src/libs/colmena/src/dag_engine/api.rs
git commit -m "feat(api): support X-Agent-Session-Id header and agent_session_id body field"
```

---

### Task 24: Bindings — forward optional agent_session_id

**Files:**
- Modify: `src/libs/colmena/src/node_bindings/mod.rs:129-145`
- Modify: `src/libs/colmena/src/python_bindings/mod.rs:275-300`

- [ ] **Step 1: Node bindings**

In `node_bindings/mod.rs:129`:

```rust
#[napi]
pub async fn run_dag(
    file_path: String,
    resume_id: Option<String>,
    resume_answer: Option<String>,
    inject_payload: Option<serde_json::Value>,
    include_extra_info: Option<bool>,
    agent_session_id: Option<String>,
) -> napi::Result<serde_json::Value> {
    let result = crate::dag_engine::api::run_dag(
        file_path,
        resume_id,
        resume_answer,
        inject_payload,
        include_extra_info.unwrap_or(false),
        agent_session_id,
    )
    .await
    .map_err(|e| napi::Error::from_reason(e.to_string()))?;
    Ok(result)
}
```

- [ ] **Step 2: Python bindings**

In `src/libs/colmena/src/python_bindings/mod.rs:273-309`, update both the `#[pyo3(signature = ...)]` attribute and the function body to add `agent_session_id`:

```rust
#[pyfunction]
#[pyo3(signature = (file_path, resume_id=None, resume_answer=None, inject_payload=None, include_extra_info=false, agent_session_id=None))]
fn run_dag(
    py: Python,
    file_path: String,
    resume_id: Option<String>,
    resume_answer: Option<String>,
    inject_payload: Option<pyo3::Bound<'_, pyo3::PyAny>>,
    include_extra_info: bool,
    agent_session_id: Option<String>,
) -> PyResult<String> {
    let inject_payload_val: Option<serde_json::Value> = match inject_payload {
        Some(obj) => Some(
            pythonize::depythonize_bound(obj).map_err(|e| DagException::new_err(e.to_string()))?,
        ),
        None => None,
    };
    py.allow_threads(move || {
        let rt =
            tokio::runtime::Runtime::new().map_err(|e| DagException::new_err(e.to_string()))?;

        rt.block_on(async {
            match crate::dag_engine::api::run_dag(
                file_path,
                resume_id,
                resume_answer,
                inject_payload_val,
                include_extra_info,
                agent_session_id,
            )
            .await
            {
                Ok(result) => serde_json::to_string_pretty(&result)
                    .map_err(|e| DagException::new_err(e.to_string())),
                Err(e) => Err(DagException::new_err(e.to_string())),
            }
        })
    })
}
```

- [ ] **Step 3: Compile each feature**

```bash
cargo check --features python
cargo check --features node
cargo check
```
Expected: all three succeed.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/node_bindings/mod.rs \
        src/libs/colmena/src/python_bindings/mod.rs
git commit -m "feat(bindings): forward agent_session_id from Node and Python bindings"
```

---

## Phase 9 — Tests

### Task 25: End-to-end integration tests

**Files:**
- Create: `tests/agent_session_id_lifecycle.rs`

- [ ] **Step 1: Write the test scenarios**

```rust
//! End-to-end tests for the agent_session_id lifecycle (spec §4.1).
//!
//! Requires `DATABASE_URL` to be set and reachable. Each test cleans up
//! its own `dag_runs` rows.

use colmena::dag_engine::engine::{ColmenaEngine, EngineConfig};
use colmena::dag_engine::domain::graph::Graph;
use futures::StreamExt;
use serde_json::json;

async fn engine() -> ColmenaEngine {
    dotenvy::dotenv().ok();
    let cfg = EngineConfig::from_env().unwrap();
    ColmenaEngine::new(cfg).await.unwrap()
}

fn trivial_graph() -> Graph {
    let raw = json!({
        "nodes": {
            "log": { "type": "log", "config": { "message": "hello" } }
        },
        "edges": []
    });
    serde_json::from_value(raw).unwrap()
}

async fn cleanup(chat: &str) {
    let url = std::env::var("DATABASE_URL").unwrap();
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    sqlx::query("DELETE FROM dag_runs WHERE agent_session_id = $1")
        .bind(chat).execute(&pool).await.ok();
}

#[tokio::test]
async fn first_run_under_new_chat_creates_root_with_agent_id() {
    let chat = "test_first_run";
    cleanup(chat).await;

    let eng = engine().await;
    let mut s = Box::pin(eng.execute_stream(
        trivial_graph(), None, None, false, None, Some(chat.into()),
    ));
    while s.next().await.is_some() {}

    let url = std::env::var("DATABASE_URL").unwrap();
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    let row: (String, Option<String>, Option<String>) = sqlx::query_as(
        "SELECT session_id, agent_session_id, parent_session_id \
         FROM dag_runs WHERE agent_session_id = $1"
    ).bind(chat).fetch_one(&pool).await.unwrap();
    assert_eq!(row.1, Some(chat.into()));
    assert_eq!(row.2, None);

    cleanup(chat).await;
    eng.shutdown().await;
}

#[tokio::test]
async fn second_run_same_chat_completes_creates_new_run_keeps_chat() {
    let chat = "test_second_run";
    cleanup(chat).await;

    let eng = engine().await;

    // First run.
    let mut s1 = Box::pin(eng.execute_stream(
        trivial_graph(), None, None, false, None, Some(chat.into()),
    ));
    while s1.next().await.is_some() {}

    // Second run (no SUSPENDED state, so a fresh root run).
    let mut s2 = Box::pin(eng.execute_stream(
        trivial_graph(), None, None, false, None, Some(chat.into()),
    ));
    while s2.next().await.is_some() {}

    let url = std::env::var("DATABASE_URL").unwrap();
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    let count: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM dag_runs WHERE agent_session_id = $1"
    ).bind(chat).fetch_one(&pool).await.unwrap();
    assert_eq!(count.0, 2, "two distinct runs should exist");

    cleanup(chat).await;
    eng.shutdown().await;
}

#[tokio::test]
async fn conflict_between_session_id_and_agent_session_id_errors() {
    let chat_a = "test_conflict_chat_a";
    let chat_b = "test_conflict_chat_b";
    cleanup(chat_a).await;
    cleanup(chat_b).await;

    let eng = engine().await;

    // Create a run under chat_a.
    let mut s = Box::pin(eng.execute_stream(
        trivial_graph(), None, None, false, None, Some(chat_a.into()),
    ));
    while s.next().await.is_some() {}

    // Read its session_id.
    let url = std::env::var("DATABASE_URL").unwrap();
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    let (sid,): (String,) = sqlx::query_as(
        "SELECT session_id FROM dag_runs WHERE agent_session_id = $1 LIMIT 1"
    ).bind(chat_a).fetch_one(&pool).await.unwrap();

    // Now resume that session_id while passing chat_b — should error.
    let mut s2 = Box::pin(eng.execute_stream(
        trivial_graph(), Some(sid), None, false, None, Some(chat_b.into()),
    ));
    let mut got_error = false;
    while let Some(item) = s2.next().await {
        if item.is_err() { got_error = true; }
    }
    assert!(got_error, "must surface the conflict as a stream error");

    cleanup(chat_a).await;
    cleanup(chat_b).await;
    eng.shutdown().await;
}
```

- [ ] **Step 2: Run them**

```bash
source .env
cargo test --test agent_session_id_lifecycle -- --test-threads=1
```
Expected: 3 PASS.

- [ ] **Step 3: Commit**

```bash
git add tests/agent_session_id_lifecycle.rs
git commit -m "test(engine): lifecycle integration tests for agent_session_id"
```

---

## Phase 10 — Documentation

### Task 26: Update developer docs

**Files:**
- Modify: `docs/developer_guide/database_schema.md`
- Modify: `docs/developer_guide/15_memory_guide.md`
- Modify: `docs/developer_guide/19_nested_agents_and_subgraphs.md`

- [ ] **Step 1: `database_schema.md` — document new columns**

Find the `dag_runs` section. Add two rows to the column table for `agent_session_id` (VARCHAR(255), YES, "Chat handle…") and `parent_session_id` (VARCHAR(255), YES, "Immediate parent run…"). Add new indexes to the indexes list.

Find the `llm_node_history` section. Add `agent_session_id` (TEXT, YES) and `node_id` (TEXT, YES) to the column table. Add a paragraph explaining the read-key semantics:

> Reads filter by `(agent_session_id, node_id)` when the run carries an `agent_session_id`; otherwise by `(session_id, node_id)`. Pre-migration rows where `node_id IS NULL` are excluded from new reads.

Add the new migration files to the migrations table at the top of the doc.

- [ ] **Step 2: `15_memory_guide.md` — explain agent_session_id**

Add a new section titled **"Agent Session ID — memoria a través de runs"** before "Tips" that explains:
- How to pass `--agent-session-id` from CLI
- How to pass it via the HTTP header
- Difference between `session_id` (run-scoped) and `agent_session_id` (chat-scoped)
- That memory persists across multiple runs of the same chat
- Backward compatibility: graphs that don't pass it work exactly as before

Also fix the existing inconsistency: the doc currently calls the field `thread_id` but the code reads it as `session_id` from config. Either rename the doc references or add a footnote pointing to the actual code path.

- [ ] **Step 3: `19_nested_agents_and_subgraphs.md` — note new propagation**

Add or update a section explaining:
- Subgraph children get a fresh UUID instead of `{parent}_sub_{node_id}`.
- `parent_session_id` is now an explicit column linking child to parent.
- LLM history inside subgraphs is keyed by `(agent_session_id, "<parent_path>/<inner_node>")` where `parent_path` is the path-qualified id of the subgraph node.

- [ ] **Step 4: Commit**

```bash
git add docs/developer_guide/database_schema.md \
        docs/developer_guide/15_memory_guide.md \
        docs/developer_guide/19_nested_agents_and_subgraphs.md
git commit -m "docs: document agent_session_id, path-qualified node_id, parent_session_id"
```

---

## Final verification

- [ ] **Step 1: Full test suite**

```bash
source .env
cargo build
cargo test --lib --package colmena_dag_engine
cargo test --test '*' -- --test-threads=1
```
Expected: all PASS.

- [ ] **Step 2: Smoke test the four lifecycle paths**

```bash
# Branch 3 — legacy
cargo run --bin dag_engine -- run tests/graphs/basic/trigger.json

# Branch 2 — chat first run
cargo run --bin dag_engine -- run tests/graphs/basic/trigger.json --agent-session-id smoke_chat

# Branch 2 — chat second run, completed run reuses chat
cargo run --bin dag_engine -- run tests/graphs/basic/trigger.json --agent-session-id smoke_chat

# Branch 1 — direct resume by session_id (use one printed from a suspend graph)
cargo run --bin dag_engine -- run tests/graphs/basic/suspend.json
# Note the session_id from output, then:
# cargo run --bin dag_engine -- run tests/graphs/basic/suspend.json --session-id <UUID> --answer "yes"
```
Expected: all complete or suspend cleanly.

- [ ] **Step 3: Verify a chat-style flow with memory**

Pick a graph that uses `llm_call` with memory. Run it twice with the same `--agent-session-id` and confirm the second run sees the first run's history. Concrete check:

```bash
psql "$DATABASE_URL" -c "
  SELECT session_id, agent_session_id, node_id, role, LEFT(content, 40)
    FROM llm_node_history
   WHERE agent_session_id = 'smoke_chat'
   ORDER BY created_at;
"
```
Expected: two distinct `session_id` values share the same `agent_session_id`, history is chronological.

---

## Self-review notes

This plan implements every section of the spec:

| Spec section | Plan task |
|---|---|
| §3.1 dag_runs columns | Tasks 3, 6, 9 |
| §3.2 llm_node_history columns | Tasks 4, 5, 8, 11–13 |
| §3.3 migration files | Tasks 3, 4, 5 |
| §4.1 lifecycle decision | Task 20 |
| §4.2 leaf resolution query | Task 10 |
| §4.4 subgraph spawn | Tasks 18, 19 |
| §5.1 CLI | Task 22 |
| §5.2 HTTP | Task 23 |
| §5.3 internal propagation | Tasks 14, 15 |
| §6.1 LLM read priority | Task 17 |
| §6.2 path-qualified node_id | Tasks 1, 2, 14, 19 |
| §6.3 cross-run continuity | Tasks 11, 13, 17, 25 |
| §7 subgraph behavior | Tasks 18, 19 |
| §8 backward compatibility | Built into every task (`Option<String>` defaults to `None`) |
| §9 migration plan order | Reflected in task ordering |
| §9.3 test matrix | Tasks 10, 13, 25 |
