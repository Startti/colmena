# Colmena — Database Schema Reference

All tables are created by versioned migrations in
`src/libs/colmena/migrations/postgres/` and applied automatically at engine
startup via `sqlx::migrate!()`. The migration runner uses the `_sqlx_migrations`
system table to track which migrations have been applied; it is idempotent and
safe to run on an already-initialized database.

## Migration files

| File | Purpose |
|------|---------|
| `20240101000000_initial_schema.sql` | Creates `llm_node_history`, `dag_runs` (base columns), `dag_task_memory`, `dag_phase_summaries` |
| `20260425000001_dag_runs_state_columns.sql` | Adds 5 JSONB execution-state columns to `dag_runs` |
| `20260425000002_secure_value_mappings.sql` | Enables `pgcrypto`, creates `secure_value_mappings` |
| `20260428000001_dag_runs_agent_session_id.sql` | Adds `agent_session_id`, `parent_session_id` columns and indices to `dag_runs` |
| `20260428000002_llm_history_agent_and_node.sql` | Adds `agent_session_id`, `node_id` columns and indices to `llm_node_history` |

*SQLite tiene su propia migración espejo `20260428000001_llm_history_agent_and_node.sql` para las columnas de `llm_node_history`. SQLite no tiene tabla `dag_runs` — esa funcionalidad es exclusiva de PostgreSQL.*

---

## Tables

### `llm_node_history`

Stores every message exchanged in an LLM conversation. One row per message.
A conversation is identified by `session_id` (the `thread_id` configured on an
`llm_call` node). Rows are ordered by `created_at` and loaded in full when
the node needs to send conversation history to a provider.

| Column | Type | Nullable | Default | Description |
|--------|------|----------|---------|-------------|
| `id` | `UUID` | NO | `gen_random_uuid()` | Unique message identifier |
| `session_id` | `TEXT` | NO | — | Conversation thread identifier (maps to `thread_id`/`session_id` in node config) |
| `agent_session_id` | `TEXT` | YES | — | Chat handle. Set when the engine run carries an `agent_session_id`; NULL otherwise. |
| `node_id` | `TEXT` | YES | — | Path-qualified identifier of the `llm_call` node that wrote this row (e.g. `"router"` or `"ventas/responder"`). NULL for pre-migration rows. |
| `role` | `TEXT` | NO | — | Message author: `system`, `user`, `assistant`, or `tool` |
| `content` | `TEXT` | NO | — | Message text body |
| `tool_call_id` | `TEXT` | YES | — | Provider-assigned ID linking a `tool` message back to the `assistant` tool call that requested it |
| `tool_calls` | `JSONB` | YES | — | Array of `ToolCall` objects serialized from an assistant response (present when role = `assistant` and the model requested tool execution) |
| `created_at` | `TIMESTAMPTZ` | NO | `NOW()` | Wall-clock time when the message was appended |

**Indexes**
- `idx_llm_node_history_session_id` on `(session_id)` — fast conversation load
- `idx_llm_node_history_created_at` on `(created_at)` — chronological ordering
- `idx_llm_history_agent_node` on `(agent_session_id, node_id, created_at)` — primary read path when an agent_session_id is present
- `idx_llm_history_session_node` on `(session_id, node_id, created_at)` — fallback for legacy reads

**Read semantics:**

- When the run carries an `agent_session_id`, reads filter by `(agent_session_id, node_id)` —
  history persists across multiple runs of the same chat.
- When `agent_session_id IS NULL` (legacy mode), reads fall back to `(session_id, node_id)` —
  history is scoped to a single run.
- Pre-migration rows where `node_id IS NULL` are excluded from new reads.

Writes always include `session_id`, `node_id`, and `agent_session_id` (when set) so the
row is fully attributed.

---

### `dag_runs`

Stores the complete execution state of one DAG run. One row per `session_id`.
The row is upserted on every state transition (node start, suspend, complete).
Used by the suspend/resume mechanism to reconstruct execution state after a
HITL (Human-in-the-Loop) pause or process restart.

| Column | Type | Nullable | Default | Description |
|--------|------|----------|---------|-------------|
| `session_id` | `VARCHAR(255)` | NO | — | **Primary key.** Unique run identifier passed to the engine at startup |
| `agent_session_id` | `VARCHAR(255)` | YES | — | Chat / conversation handle. Groups multiple runs (and their subgraph children) under the same external chat session. NULL for legacy runs that did not opt in. |
| `parent_session_id` | `VARCHAR(255)` | YES | — | When this row is a subgraph child, the parent run's `session_id`. NULL for root runs. |
| `graph_json` | `JSONB` | NO | — | Complete DAG graph definition as submitted to the engine |
| `all_outputs` | `JSONB` | NO | — | `HashMap<node_id, output_value>` — accumulated outputs from every node that has run |
| `status` | `VARCHAR(50)` | NO | — | Run lifecycle state: `RUNNING`, `SUSPENDED`, `COMPLETED`, or `FAILED` |
| `active_queue` | `JSONB` | NO | `[]` | `VecDeque<node_id>` — nodes still waiting to execute (serialized as a JSON array) |
| `execution_history` | `JSONB` | NO | `[]` | `Vec<[caller_id, target_id]>` — ordered log of every node invocation |
| `global_calls` | `JSONB` | NO | `{}` | `HashMap<node_id, count>` — total number of times each node has been called, used for global call-limit checks |
| `caller_specific_calls` | `JSONB` | NO | `{}` | `HashMap<caller_id, HashMap<target_id, count>>` — per-caller invocation counts, used for caller-scoped call limits |
| `global_shared_state` | `JSONB` | NO | `{}` | Persistent whiteboard object readable and writable by all nodes in the run |
| `created_at` | `TIMESTAMPTZ` | YES | `CURRENT_TIMESTAMP` | When the run row was first inserted |
| `updated_at` | `TIMESTAMPTZ` | YES | `CURRENT_TIMESTAMP` | Timestamp of the most recent state save |

**Indexes**
- `idx_dag_runs_agent_session_id` on `(agent_session_id)`
- `idx_dag_runs_parent_session_id` on `(parent_session_id)`
- `idx_dag_runs_agent_status` on `(agent_session_id, status)` — fast leaf lookup

**Tree linkage:** When `parent_session_id IS NOT NULL`, the row represents a subgraph
child. All rows in a conversation tree share the same `agent_session_id`. The deepest
SUSPENDED row (the one not referenced as a parent by any other SUSPENDED row) is the
"leaf" — the run currently awaiting user input.

---

### `dag_task_memory`

Tracks individual tasks within a multi-phase DAG loop (planner → agent →
reactor pattern). One row per task. The planner node inserts tasks; agent
nodes claim and complete them; the reactor node reads results and may create
tasks for the next phase.

| Column | Type | Nullable | Default | Description |
|--------|------|----------|---------|-------------|
| `id` | `UUID` | NO | — | Unique task identifier (generated by the application) |
| `session_id` | `VARCHAR(255)` | NO | — | Links the task to its DAG run |
| `task_name` | `TEXT` | NO | — | Human-readable task label assigned by the planner |
| `assigned_to` | `VARCHAR(255)` | NO | — | Node ID or agent name responsible for executing this task |
| `completed` | `BOOLEAN` | NO | `FALSE` | `TRUE` once the agent has written a result |
| `result` | `JSONB` | YES | — | Task output written by the agent node upon completion |
| `phase` | `INT` | NO | `1` | Execution phase (1-based). Tasks with the same phase number may run in parallel; phase N+1 only starts after all phase-N tasks complete |
| `parallel` | `BOOLEAN` | NO | `FALSE` | When `TRUE`, this task should run concurrently with other tasks in the same phase |
| `context` | `TEXT` | YES | — | Semantic description of the task's purpose, provided by the planner for the agent's context |
| `is_bridge` | `BOOLEAN` | NO | `FALSE` | When `TRUE`, this task is a prerequisite bridge task that must complete before the next phase is unlocked |
| `created_at` | `TIMESTAMPTZ` | YES | `CURRENT_TIMESTAMP` | When the task was inserted |
| `updated_at` | `TIMESTAMPTZ` | YES | `CURRENT_TIMESTAMP` | When the task was last modified (e.g., marked complete) |

**Indexes**
- `idx_dag_task_memory_session_id` on `(session_id)` — task list per run
- `idx_dag_task_memory_phase` on `(session_id, phase, completed)` — phase-aware task routing

---

### `dag_phase_summaries`

Stores the text summary produced by the reactor node at the end of each
execution phase. Summaries are loaded by the `final_reactor` node so it can
produce a consolidated outcome across all phases.

| Column | Type | Nullable | Default | Description |
|--------|------|----------|---------|-------------|
| `id` | `UUID` | NO | `gen_random_uuid()` | Unique summary identifier |
| `session_id` | `TEXT` | NO | — | Links the summary to its DAG run |
| `phase` | `INT` | NO | — | The phase number this summary covers |
| `summary` | `TEXT` | NO | — | LLM-generated narrative summary of what the phase accomplished |
| `created_at` | `TIMESTAMPTZ` | YES | `CURRENT_TIMESTAMP` | When the summary was written |

**Indexes**
- `idx_dag_phase_summaries_session_id` on `(session_id)` — load all summaries for a run

---

### `secure_value_mappings`

Stores encrypted secrets (API keys, tokens, passwords) produced by
`secure_value` nodes. Each secret is encrypted with `pgp_sym_encrypt` (AES-256
via `pgcrypto`) using the key from the `SECURE_VALUES_KEY` environment variable.
Rows expire after 1 hour and are deleted at session cleanup or by the background
expiry sweeper.

| Column | Type | Nullable | Default | Description |
|--------|------|----------|---------|-------------|
| `id` | `UUID` | NO | `gen_random_uuid()` | Unique mapping identifier |
| `session_id` | `VARCHAR(255)` | NO | — | Session that owns this secret; used for isolation and cleanup |
| `source_node_id` | `VARCHAR(255)` | NO | — | ID of the `secure_value` node that produced this secret |
| `hash_key` | `VARCHAR(255)` | NO | — | Deterministic hash of the secret (used as lookup key without exposing the plaintext) |
| `encrypted_value` | `BYTEA` | NO | — | AES-256 encrypted ciphertext produced by `pgp_sym_encrypt` |
| `field_name` | `VARCHAR(255)` | YES | — | Name of the field this secret corresponds to (e.g., `api_key`, `Authorization`) |
| `created_at` | `TIMESTAMPTZ` | YES | `NOW()` | When the secret was stored |
| `expires_at` | `TIMESTAMPTZ` | YES | `NOW() + 1 hour` | Absolute expiry time; rows past this timestamp are eligible for deletion |

**Constraints**
- `UNIQUE(session_id, hash_key)` — prevents duplicate secrets per session; an `ON CONFLICT` upsert refreshes the TTL

**Indexes**
- `idx_secure_session_id` on `(session_id)` — session cleanup
- `idx_secure_hash_key` on `(session_id, hash_key)` — fast decrypt lookup
- `idx_secure_expires_at` on `(expires_at)` — expiry sweep

**Required PostgreSQL extension**: `pgcrypto` (enabled by migration
`20260425000002_secure_value_mappings.sql`).

---

## Entity relationships

```
dag_runs ──< dag_task_memory     (dag_runs.session_id = dag_task_memory.session_id)
dag_runs ──< dag_phase_summaries (dag_runs.session_id = dag_phase_summaries.session_id)
dag_runs ──< secure_value_mappings (dag_runs.session_id = secure_value_mappings.session_id)

llm_node_history  ── standalone; new reads keyed by (agent_session_id, node_id), legacy reads by (session_id, node_id)
dag_runs ──< dag_runs (parent_session_id → session_id) — subgraph child tree
```

---

## Connection configuration

The engine uses `DATABASE_URL` (environment variable) as its **internal
database** — the one where DAG state, tasks, phase summaries, and secure values
are stored. Any PostgreSQL URL configured on an `llm_call` node's
`connection_url` field gets the same migrations applied and stores only
`llm_node_history` rows.

Set `DATABASE_URL` before starting the engine:

```bash
export DATABASE_URL="postgresql://user:pass@host:5432/dbname"
```

Pool behaviour is controlled by:

| Variable | Default | Description |
|----------|---------|-------------|
| `COLMENA_POOL_MAX_ENTRIES` | 100 | Maximum number of distinct connection pools |
| `COLMENA_POOL_MAX_CONN_PER_URL` | 2 | Connections per pool |
| `COLMENA_POOL_MIN_CONN_PER_URL` | 0 | Always-open connections per pool |
| `COLMENA_POOL_IDLE_TIMEOUT_SEC` | 30 | Idle connection timeout (seconds) |
| `COLMENA_POOL_MAX_LIFETIME_SEC` | 600 | Max connection lifetime (seconds) |
| `COLMENA_POOL_ACQUIRE_TIMEOUT_SEC` | 10 | Timeout waiting for a connection from the pool |
