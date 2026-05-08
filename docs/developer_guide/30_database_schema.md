# Database Schema Reference

## Overview

This document is a quick reference for the three PostgreSQL tables at the core of Colmena's state persistence layer:

- `secure_value_mappings` — encrypted secrets for the Secure Values feature.
- `llm_node_history` — per-node conversational memory for `llm_call` nodes.
- `dag_runs` — DAG execution state, including suspended chains for HITL resume.

**Source of truth:** the migration files under `src/libs/colmena/migrations/postgres/`. This document is a derived reference; if there is ever a discrepancy, the migration files win. Consult them directly for the exact DDL.

---

## The Shared Pattern: `agent_session_id`-First Lookup

All three tables carry two session identifiers: `session_id` and `agent_session_id`.

`session_id` is ephemeral. It is generated per CLI invocation (or per HTTP request to the serve endpoint) and uniquely identifies a single DAG run. When Colmena is invoked again — even for the same logical agent — a new `session_id` is created. This makes `session_id` unsuitable as a stable key for cross-run continuity.

`agent_session_id` is stable. It is assigned by the caller (e.g. the ADP runtime or the canvas-builder frontend) and remains constant across all CLI invocations that belong to the same agent instance or conversation thread. It is the identifier that survives restarts, retries, and multi-step interactions.

The lookup convention applied uniformly across all three tables is:

> When `agent_session_id IS NOT NULL` on the incoming request, filter by `agent_session_id`.
> Otherwise fall back to `session_id`.

This means that if an ADP runtime provides an `agent_session_id`, a second run of the same agent will find and reuse the memory, secrets, and suspended state persisted during the first run — even though `session_id` changed. If no `agent_session_id` is provided (e.g. a plain `cargo run` test), the system behaves exactly as it did before the column was added: `session_id` isolation is the default.

This pattern enables several scenarios that require cross-session continuity:

- **Canvas-builder**: a user interacting with an agent over multiple browser sessions sees a consistent conversation and persistent secrets.
- **Multi-turn agents in ADP**: the runtime can resume a suspended HITL chain by `agent_session_id` alone, without needing to track the ephemeral `session_id` assigned to the previous run.
- **Secure Values across runs**: credentials collected via `secure_suspend` in one session are transparently available in the next session of the same agent.

Relevant design specs (under `docs/superpowers/specs/`):

- `2026-04-28-agent-session-id-design.md` — original platform spec for memory and DAG state.
- `2026-05-08-secure-values-agent-session-id-design.md` — extension of the same pattern to `secure_value_mappings`.

---

## Schema Tables

### `secure_value_mappings`

**Migrations:**
- `20260425000002_secure_value_mappings.sql` — creates the table and indexes, enables `pgcrypto`.
- `20260508000001_secure_values_agent_session_id.sql` — adds `agent_session_id` column and composite index.

**Full schema (post all migrations):**

```sql
CREATE EXTENSION IF NOT EXISTS pgcrypto;

CREATE TABLE IF NOT EXISTS secure_value_mappings (
    id              UUID         PRIMARY KEY DEFAULT gen_random_uuid(),
    session_id      VARCHAR(255) NOT NULL,
    agent_session_id TEXT,
    source_node_id  VARCHAR(255) NOT NULL,
    hash_key        VARCHAR(255) NOT NULL,
    encrypted_value BYTEA        NOT NULL,
    field_name      VARCHAR(255),
    created_at      TIMESTAMPTZ  DEFAULT NOW(),
    expires_at      TIMESTAMPTZ  DEFAULT (NOW() + INTERVAL '1 hour'),
    UNIQUE(session_id, hash_key)
);

CREATE INDEX IF NOT EXISTS idx_secure_session_id
    ON secure_value_mappings(session_id);
CREATE INDEX IF NOT EXISTS idx_secure_hash_key
    ON secure_value_mappings(session_id, hash_key);
CREATE INDEX IF NOT EXISTS idx_secure_expires_at
    ON secure_value_mappings(expires_at);
CREATE INDEX IF NOT EXISTS idx_secure_values_agent_hash
    ON secure_value_mappings(agent_session_id, hash_key);
```

**Purpose:** Stores secrets encrypted with AES-256 via `pgcrypto`. Real values never leave the row. LLMs see only opaque handles (`<sv_<name>>` or `<value_N>`). The `hash_key` is the stable reference used to inject the real value back at execution time.

**Note on `field_name`:** The column is nullable (`VARCHAR(255)` with no `NOT NULL`). It records the original config field name when the secret was collected; it may be `NULL` for secrets hashed from arbitrary node outputs.

**Writers:**
- `SecureValueService::persist_secret` — called from the `secure_suspend` resume path; stores credentials collected interactively.
- `SecureValueService::hash_output` — called from any non-LLM node that has `"secure": true` in its config; hashes and persists the node's output so the real value is not propagated through the graph.

**Readers:**
- `SecureValueService::inject_secrets` — called from `run_use_case.rs` and `dag_tool_executor.rs` before each non-LLM node executes. Replaces handles with decrypted values. Lookup is agent-first when `agent_session_id` is set, otherwise by `session_id`.

**Lifecycle:**
- `expires_at` defaults to 1 hour from creation. A background `cleanup_expired` task periodically reaps stale rows.
- `SecureValueService::cleanup(session_id)` is called when a DAG terminates to remove all secrets for that session.

---

### `llm_node_history`

**Migrations:**
- `20240101000000_initial_schema.sql` — creates the table with `session_id`-based lookup only.
- `20260428000002_llm_history_agent_and_node.sql` — adds `agent_session_id` and `node_id` columns; adds composite indexes for agent-first and session-first lookups.

**Full schema (post all migrations):**

```sql
CREATE TABLE IF NOT EXISTS llm_node_history (
    id            UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    session_id    TEXT        NOT NULL,
    agent_session_id TEXT,
    node_id       TEXT,
    role          TEXT        NOT NULL,
    content       TEXT        NOT NULL,
    tool_call_id  TEXT,
    tool_calls    JSONB,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_llm_node_history_session_id
    ON llm_node_history(session_id);
CREATE INDEX IF NOT EXISTS idx_llm_node_history_created_at
    ON llm_node_history(created_at);
CREATE INDEX IF NOT EXISTS idx_llm_history_agent_node
    ON llm_node_history(agent_session_id, node_id, created_at);
CREATE INDEX IF NOT EXISTS idx_llm_history_session_node
    ON llm_node_history(session_id, node_id, created_at);
```

**Purpose:** Per-`llm_call`-node conversational memory. Each message in a conversation (user, assistant, tool, tool_result) is stored as a row. The `node_id` column enables multiple `llm_call` nodes within the same session to maintain independent histories.

**Note on pre-existing rows:** Rows inserted before the `20260428000002` migration have `node_id = NULL` and `agent_session_id = NULL`. The reader excludes them from agent-scoped lookups by requiring `node_id IS NOT NULL`.

**Writers:**
- `ConversationRepository::add_message` — appends a single message row after each LLM turn.

**Readers:**
- `ConversationRepository::get_by_id` — reconstructs the full conversation for a node. Lookup follows agent-first convention (see `postgres_conversation_repository.rs:22-44`): if `agent_session_id` is set, filter by `(agent_session_id, node_id)`; otherwise by `(session_id, node_id)`.

**Lifecycle:** Rows are persistent; there is no automatic expiry. Manual cleanup is required for long-lived agents or test data.

---

### `dag_runs`

**Migrations:**
- `20240101000000_initial_schema.sql` — creates the table with `session_id` as primary key and basic `graph_json`, `all_outputs`, `status`, timestamps.
- `20260425000001_dag_runs_state_columns.sql` — adds 5 JSONB execution-state columns (`active_queue`, `execution_history`, `global_calls`, `caller_specific_calls`, `global_shared_state`).
- `20260428000001_dag_runs_agent_session_id.sql` — adds `agent_session_id` and `parent_session_id`; adds three indexes.

**Full schema (post all migrations):**

```sql
CREATE TABLE IF NOT EXISTS dag_runs (
    session_id           VARCHAR(255) PRIMARY KEY,
    graph_json           JSONB        NOT NULL,
    all_outputs          JSONB        NOT NULL,
    status               VARCHAR(50)  NOT NULL,
    created_at           TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP,
    updated_at           TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP,
    active_queue         JSONB        NOT NULL DEFAULT '[]'::jsonb,
    execution_history    JSONB        NOT NULL DEFAULT '[]'::jsonb,
    global_calls         JSONB        NOT NULL DEFAULT '{}'::jsonb,
    caller_specific_calls JSONB       NOT NULL DEFAULT '{}'::jsonb,
    global_shared_state  JSONB        NOT NULL DEFAULT '{}'::jsonb,
    agent_session_id     VARCHAR(255),
    parent_session_id    VARCHAR(255)
);

CREATE INDEX IF NOT EXISTS idx_dag_runs_agent_session_id
    ON dag_runs(agent_session_id);
CREATE INDEX IF NOT EXISTS idx_dag_runs_parent_session_id
    ON dag_runs(parent_session_id);
CREATE INDEX IF NOT EXISTS idx_dag_runs_agent_status
    ON dag_runs(agent_session_id, status);
```

**Purpose:** Persists the full DAG execution state — including the active node queue, execution history, and shared global state — so that a suspended chain can be resumed in a later CLI invocation or HTTP request. `parent_session_id` links subgraph runs to their parent.

**Writers:**
- `PostgresDagStateRepository::save_state` — called at each state transition (node completion, suspension, etc.).

**Readers:**
- `find_resume_entry(agent_session_id)` — locates a suspended chain when only the stable agent id is known. This allows the CLI to resume with `--agent-session-id` alone, without remembering the ephemeral `session_id` from the previous run.
- Standard load-by-session: used within a single run to restore state after a suspend/resume cycle.

**Lifecycle:** Rows persist across CLI invocations until the DAG completes or is explicitly cleaned up. The `status` column (`suspended`, `running`, `completed`, `failed`) tracks the lifecycle state. Test data should be pruned manually (see Operational Queries below).

---

## Operational Queries

The following snippets are intended for use in `psql` or any PostgreSQL client against a running Colmena database.

**List secrets persisted for a given agent session:**

```sql
SELECT hash_key, field_name, source_node_id, expires_at
FROM   secure_value_mappings
WHERE  agent_session_id = 'your-agent-session-id'
ORDER  BY expires_at;
```

**List secrets persisted for a given ephemeral session:**

```sql
SELECT hash_key, field_name, source_node_id, expires_at
FROM   secure_value_mappings
WHERE  session_id = 'your-session-id'
ORDER  BY expires_at;
```

**Find a suspended DAG chain for an agent:**

```sql
SELECT session_id, status, created_at, updated_at
FROM   dag_runs
WHERE  agent_session_id = 'your-agent-session-id'
  AND  status = 'suspended'
ORDER  BY updated_at DESC
LIMIT  5;
```

**Inspect conversation history for an llm_call node:**

```sql
SELECT role, LEFT(content, 120) AS content_preview, created_at
FROM   llm_node_history
WHERE  agent_session_id = 'your-agent-session-id'
  AND  node_id = 'your-llm-node-id'
ORDER  BY created_at;
```

**Count messages per node across a session (session fallback):**

```sql
SELECT node_id, role, COUNT(*) AS msg_count
FROM   llm_node_history
WHERE  session_id = 'your-session-id'
GROUP  BY node_id, role
ORDER  BY node_id, role;
```

**Remove expired secrets manually:**

```sql
DELETE FROM secure_value_mappings
WHERE  expires_at < NOW();
```

**Clean up test data across all three tables:**

```sql
-- Replace 'test_%' with your test prefix pattern
DELETE FROM secure_value_mappings  WHERE agent_session_id LIKE 'test_%';
DELETE FROM llm_node_history       WHERE agent_session_id LIKE 'test_%';
DELETE FROM dag_runs               WHERE agent_session_id LIKE 'test_%';
```

---

## Further Reading

- `docs/developer_guide/13_security_strategy.md` — Secure Values design, AES-256-GCM encryption, `secure_suspend`, and the injection pipeline.
- `docs/developer_guide/15_memory_guide.md` — Memory configuration (SQLite vs. PostgreSQL), environment variables, and connection pooling.
- `docs/dds/SECURE_VALUES_DISEÑO.md` — Architecture decision record for the Secure Values feature.
- `docs/superpowers/specs/2026-04-28-agent-session-id-design.md` — Original platform spec for `agent_session_id` across memory and DAG state.
- `docs/superpowers/specs/2026-05-08-secure-values-agent-session-id-design.md` — Extension of the pattern to `secure_value_mappings`.
- Migrations (source of truth): `src/libs/colmena/migrations/postgres/`.
