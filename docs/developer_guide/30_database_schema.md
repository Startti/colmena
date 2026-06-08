# Colmena — Database Schema Reference

Colmena persists state in PostgreSQL (recommended) and supports SQLite as a
lightweight alternative for the LLM conversation history. There are **two
families of tables**:

1. **Engine tables** — created by versioned migrations under
   `src/libs/colmena/migrations/{postgres,sqlite}/`, applied at engine startup
   via `sqlx::migrate!()` from [`engine.rs`](../../src/libs/colmena/src/dag_engine/engine.rs)
   and the LLM `ConversationRepositoryFactory`
   ([`repository_factory.rs`](../../src/libs/colmena/src/llm/infrastructure/persistence/repository_factory.rs)).
   The migration runner uses the `_sqlx_migrations` system table to track
   applied versions; it is idempotent and `ignore_missing` is enabled so the
   engine boots cleanly against databases with old/consolidated history.
2. **Sandbox tables** — created lazily on first use by the SQL node
   infrastructure (`PgRegistryAdapter::ensure_schema()`), inside the
   configurable sandbox schema (default: `sandbox`).

## Migration files

All files live under `src/libs/colmena/migrations/`.

### PostgreSQL (`migrations/postgres/`)

| File | Purpose |
|------|---------|
| `20240101000000_initial_schema.sql` | Drops legacy `chat_messages`, creates `llm_node_history`, `dag_runs` (base columns), `dag_task_memory`, `dag_phase_summaries` |
| `20260425000001_dag_runs_state_columns.sql` | Adds the 5 JSONB execution-state columns to `dag_runs` (`active_queue`, `execution_history`, `global_calls`, `caller_specific_calls`, `global_shared_state`). Uses `ADD COLUMN IF NOT EXISTS` so it is safe on already-upgraded DBs |
| `20260425000002_secure_value_mappings.sql` | Enables the `pgcrypto` extension and creates `secure_value_mappings` |
| `20260428000001_dag_runs_agent_session_id.sql` | Adds `agent_session_id`, `parent_session_id` columns and 3 indices to `dag_runs` |
| `20260428000002_llm_history_agent_and_node.sql` | Adds `agent_session_id`, `node_id` columns and 2 composite indices to `llm_node_history` |
| `20260502000001_provider_file_cache.sql` | Creates `provider_file_cache` for the Files API caching feature (large documents via signed URL) |
| `20260508000001_secure_values_agent_session_id.sql` | Adds `agent_session_id` column + `idx_secure_values_agent_hash` composite index to `secure_value_mappings` |
| `20260511000001_secure_values_24h_ttl.sql` | Sliding TTL bump: changes `secure_value_mappings.expires_at` default from `NOW() + 1 hour` to `NOW() + 24 hours` |
| `20260513000001_conversation_attachments.sql` | Creates `conversation_attachments` (per-`agent_session_id` registry of files attached to a conversation; survives across runs) |
| `20260525000001_attachment_uniform_resolution.sql` | Adds `storage_key`, `origin`, `last_used_at` to `conversation_attachments` + `idx_conv_attachments_session_used` |
| `20260603000000_crdt_doc_changes.sql` | Creates the CRDT-documents change log: `crdt_doc_events`, `crdt_doc_session_cursors`, `crdt_doc_session_artifacts`, plus 3 indices. Backs the `crdt_doc_get_recent_changes` LLM tool and per-agent attachment list |

### SQLite (`migrations/sqlite/`)

| File | Purpose |
|------|---------|
| `20240101000000_create_chat_messages.sql` | Drops legacy `chat_messages`, creates `llm_node_history` (TEXT-typed mirror of the Postgres table) |
| `20260303000000_create_dag_task_memory.sql` | Creates the SQLite mirror of `dag_task_memory` (base columns only) |
| `20260408000000_add_is_bridge_to_dag_task.sql` | Adds the missing `phase`, `parallel`, `context`, and `is_bridge` columns. SQLite does NOT support `ADD COLUMN IF NOT EXISTS`, so this file must only be applied on fresh schemas |
| `20260428000001_llm_history_agent_and_node.sql` | Mirrors the Postgres llm-history migration: adds `agent_session_id`, `node_id`, and the two composite indices |
| `20260513000001_conversation_attachments.sql` | SQLite mirror of the Postgres `conversation_attachments` table (TEXT/INTEGER-typed) |
| `20260525000001_attachment_uniform_resolution.sql` | Mirrors the Postgres `storage_key`/`origin`/`last_used_at` extension on `conversation_attachments` |
| `20260603000000_crdt_doc_changes.sql` | SQLite mirror of the CRDT change-log tables (TEXT/INTEGER-typed). Used when the CRDT runtime points at a SQLite backend in tests / local dev |

> **SQLite scope**: SQLite is supported for `llm_node_history`,
> `dag_task_memory`, `conversation_attachments` and the CRDT change-log
> tables (`crdt_doc_events`, `crdt_doc_session_cursors`,
> `crdt_doc_session_artifacts`). The DAG state machine (`dag_runs`,
> `dag_phase_summaries`, `secure_value_mappings`, `provider_file_cache`)
> and the SQL sandbox tables are PostgreSQL-only — features that depend
> on them (resume after suspend, secure values, the Files API cache,
> the SQL node's function registry) require a Postgres internal database.

### Migration scope: where each set runs

The same `migrations/postgres/` directory is applied in two places:

| Database role | When migrated | Migrations applied |
|---------------|---------------|--------------------|
| **Internal database** (`DATABASE_URL`, set in `EngineConfig.internal_database_url`) | Once at engine startup (`ColmenaEngine::new`) | All Postgres migrations |
| **External LLM database** (any `connection_url` configured on an `llm_call` node) | Lazily when the first `LlmConversationRepository` is built for that URL | All Postgres migrations (or all SQLite migrations if the URL is `sqlite://...`) |

In practice this means *any* database the engine touches will end up with the
full table set, but only the **internal** database is actually read from for
DAG state, tasks, phase summaries, and secure values. External databases will
typically only see writes to `llm_node_history`.

---

## Engine tables

### `llm_node_history`

Stores every message exchanged in an LLM conversation. **One row per message.**
Rows are loaded in full when an `llm_call` node needs to send conversation
history to a provider, and ordered chronologically by `created_at`.

A conversation is identified by the tuple `(agent_session_id, node_id)`
(post-migration, primary read path) or `(session_id, node_id)` (legacy
fallback). See **Read semantics** below.

| Column | Postgres type | SQLite type | Nullable | Default | Description |
|--------|---------------|-------------|----------|---------|-------------|
| `id` | `UUID` | `TEXT` | NO | `gen_random_uuid()` (PG only) | Unique message identifier (application-generated UUID on SQLite) |
| `session_id` | `TEXT` | `TEXT` | NO | — | Conversation thread identifier (the node-config `thread_id` / `session_id`) |
| `agent_session_id` | `TEXT` | `TEXT` | YES | — | Chat-scoped handle. Set when the engine run carries an `agent_session_id`; NULL otherwise |
| `node_id` | `TEXT` | `TEXT` | YES | — | Path-qualified identifier of the `llm_call` node that wrote this row (e.g. `"router"` or `"orchestrator/sales/responder"`). NULL for pre-migration rows |
| `role` | `TEXT` | `TEXT` | NO | — | Message author: `system`, `user`, `assistant`, or `tool` |
| `content` | `TEXT` | `TEXT` | NO | — | Message text body |
| `tool_call_id` | `TEXT` | `TEXT` | YES | — | Provider-assigned ID linking a `tool` message back to the `assistant` tool call that requested it |
| `tool_calls` | `JSONB` | `TEXT` | YES | — | Array of `ToolCall` objects serialized from an assistant response (present when `role = assistant` and the model requested tool execution). On SQLite the JSON is stored as a text blob |
| `created_at` | `TIMESTAMPTZ` | `TEXT` | NO | `NOW()` (PG) / app-set ISO-8601 (SQLite) | Wall-clock time when the message was appended |

**Indexes**

- `idx_llm_node_history_session_id` on `(session_id)` — legacy conversation load
- `idx_llm_node_history_created_at` on `(created_at)` — chronological ordering
- `idx_llm_history_agent_node` on `(agent_session_id, node_id, created_at)` — primary read path when an `agent_session_id` is present
- `idx_llm_history_session_node` on `(session_id, node_id, created_at)` — fallback for legacy reads

**Read semantics**

- When the run carries an `agent_session_id`, reads filter by
  `(agent_session_id, node_id)` — history persists across multiple runs of the
  same chat.
- When `agent_session_id IS NULL` (legacy mode), reads fall back to
  `(session_id, node_id)` — history is scoped to a single run.
- Pre-migration rows where `node_id IS NULL` are excluded from new reads.

**Write semantics**: writes always include `session_id`, `node_id`, and
`agent_session_id` (when set) so each row is fully attributed.

---

### `dag_runs`  *(PostgreSQL only)*

Stores the complete execution state of one DAG run. **One row per
`session_id`.** The row is upserted on every state transition (node start,
suspend, complete) by `PostgresDagStateRepository`. It powers the
suspend/resume mechanism, allowing the engine to reconstruct the in-flight
execution after a HITL (Human-in-the-Loop) pause or a process restart.

| Column | Type | Nullable | Default | Description |
|--------|------|----------|---------|-------------|
| `session_id` | `VARCHAR(255)` | NO | — | **Primary key.** Unique run identifier passed to the engine at startup |
| `agent_session_id` | `VARCHAR(255)` | YES | — | Chat / conversation handle. Groups multiple runs (and their subgraph children) under the same external chat session. NULL for legacy runs that did not opt in |
| `parent_session_id` | `VARCHAR(255)` | YES | — | When this row is a subgraph child, the parent run's `session_id`. NULL for root runs |
| `graph_json` | `JSONB` | NO | — | Complete DAG graph definition as submitted to the engine — the source of truth used when resuming |
| `all_outputs` | `JSONB` | NO | — | `HashMap<node_id, output_value>` — accumulated outputs from every node that has run |
| `status` | `VARCHAR(50)` | NO | — | Run lifecycle state: `RUNNING`, `SUSPENDED`, `COMPLETED`, or `FAILED` |
| `active_queue` | `JSONB` | NO | `'[]'::jsonb` | `VecDeque<node_id>` — nodes still waiting to execute, serialized as a JSON array |
| `execution_history` | `JSONB` | NO | `'[]'::jsonb` | `Vec<[caller_id, target_id]>` — ordered log of every node invocation in the run |
| `global_calls` | `JSONB` | NO | `'{}'::jsonb` | `HashMap<node_id, count>` — total number of times each node has been called; used for global call-limit checks |
| `caller_specific_calls` | `JSONB` | NO | `'{}'::jsonb` | `HashMap<caller_id, HashMap<target_id, count>>` — per-caller invocation counts; used for caller-scoped call limits |
| `global_shared_state` | `JSONB` | NO | `'{}'::jsonb` | Persistent whiteboard object readable and writable by every node in the run |
| `created_at` | `TIMESTAMPTZ` | YES | `CURRENT_TIMESTAMP` | When the run row was first inserted |
| `updated_at` | `TIMESTAMPTZ` | YES | `CURRENT_TIMESTAMP` | Timestamp of the most recent state save |

**Indexes**

- `idx_dag_runs_agent_session_id` on `(agent_session_id)`
- `idx_dag_runs_parent_session_id` on `(parent_session_id)` — fast subgraph-tree walks
- `idx_dag_runs_agent_status` on `(agent_session_id, status)` — fast leaf lookup (used by `find_suspended_leaf`)

**Tree linkage and suspend resolution**

When `parent_session_id IS NOT NULL`, the row represents a subgraph child. All
rows in a single conversation tree share the same `agent_session_id`. When the
engine receives a resume request for an `agent_session_id`, it walks the tree
to find the **topmost** SUSPENDED row — the run currently awaiting user
input — and replays it. (Earlier versions resumed from the deepest leaf; this
was changed in commit `44fba1d` to fix nested-orchestrator resume.)

---

### `dag_task_memory`

Tracks individual tasks within a multi-phase DAG loop (planner → agent →
reactor pattern). **One row per task.** The planner node inserts tasks; agent
nodes claim and complete them; the reactor node reads results and may insert
tasks for the next phase. Available on both PostgreSQL and SQLite.

| Column | Postgres type | SQLite type | Nullable | Default | Description |
|--------|---------------|-------------|----------|---------|-------------|
| `id` | `UUID` | `TEXT` | NO | — | Unique task identifier (generated by the application) |
| `session_id` | `VARCHAR(255)` | `TEXT` | NO | — | Links the task to its DAG run |
| `task_name` | `TEXT` | `TEXT` | NO | — | Human-readable task label assigned by the planner |
| `assigned_to` | `VARCHAR(255)` | `TEXT` | NO | — | Node ID or agent name responsible for executing this task |
| `completed` | `BOOLEAN` | `BOOLEAN` (0/1) | NO | `FALSE` / `0` | `TRUE` once the agent has written a result |
| `result` | `JSONB` | `TEXT` | YES | — | Task output written by the agent node upon completion |
| `phase` | `INT` | `INTEGER` | NO | `1` | Execution phase (1-based). Tasks with the same phase number may run in parallel; phase N+1 only starts once all phase-N tasks complete |
| `parallel` | `BOOLEAN` | `BOOLEAN` (0/1) | NO | `FALSE` / `0` | When `TRUE`, this task should run concurrently with other tasks in the same phase |
| `context` | `TEXT` | `TEXT` | YES | — | Semantic description of the task's purpose, provided by the planner for the agent's context |
| `is_bridge` | `BOOLEAN` | `BOOLEAN` (0/1) | NO | `FALSE` / `0` | When `TRUE`, this task is a prerequisite *bridge* task that must complete before the next phase is unlocked |
| `created_at` | `TIMESTAMPTZ` | `DATETIME` | YES | `CURRENT_TIMESTAMP` | When the task was inserted |
| `updated_at` | `TIMESTAMPTZ` | `DATETIME` | YES | `CURRENT_TIMESTAMP` | When the task was last modified (e.g., marked complete) |

**Indexes**

- `idx_dag_task_memory_session_id` on `(session_id)` — task list per run
- `idx_dag_task_memory_phase` on `(session_id, phase, completed)` *(Postgres only)* — phase-aware task routing

---

### `dag_phase_summaries`  *(PostgreSQL only)*

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

### `secure_value_mappings`  *(PostgreSQL only)*

Stores encrypted secrets (API keys, tokens, passwords) produced by
`secure_value` nodes. Each secret is encrypted with `pgp_sym_encrypt` (AES-256
via `pgcrypto`) using the key from the `SECURE_VALUES_KEY` environment
variable. Rows expire after **24 hours** by default (bumped from the original
1h on 2026-05-11 via `20260511000001_secure_values_24h_ttl.sql` to support
sliding-window reuse across consecutive runs of the same agent) and are
deleted at session cleanup or by the background expiry sweeper.

| Column | Type | Nullable | Default | Description |
|--------|------|----------|---------|-------------|
| `id` | `UUID` | NO | `gen_random_uuid()` | Unique mapping identifier |
| `session_id` | `VARCHAR(255)` | NO | — | Session that owns this secret; used for isolation and cleanup |
| `agent_session_id` | `TEXT` | YES | — | Chat-scoped handle. When set, the agent-first lookup path resolves secrets across multiple runs of the same agent (added by `20260508000001_secure_values_agent_session_id.sql`) |
| `source_node_id` | `VARCHAR(255)` | NO | — | ID of the `secure_value` node that produced this secret |
| `hash_key` | `VARCHAR(255)` | NO | — | Deterministic hash of the secret (used as a lookup key without exposing the plaintext) |
| `encrypted_value` | `BYTEA` | NO | — | AES-256 ciphertext produced by `pgp_sym_encrypt` |
| `field_name` | `VARCHAR(255)` | YES | — | Name of the field this secret corresponds to (e.g., `api_key`, `Authorization`) |
| `created_at` | `TIMESTAMPTZ` | YES | `NOW()` | When the secret was stored |
| `expires_at` | `TIMESTAMPTZ` | YES | `NOW() + INTERVAL '24 hours'` | Absolute expiry time; rows past this timestamp are eligible for deletion. Pre-existing rows keep whatever TTL they were written with — they are swept naturally by `cleanup_expired_for_run` as their owning runs complete |

**Constraints**

- `UNIQUE(session_id, hash_key)` — prevents duplicate secrets per session; an
  `ON CONFLICT` upsert refreshes the TTL.

**Indexes**

- `idx_secure_session_id` on `(session_id)` — session cleanup
- `idx_secure_hash_key` on `(session_id, hash_key)` — fast decrypt lookup
- `idx_secure_expires_at` on `(expires_at)` — expiry sweep
- `idx_secure_values_agent_hash` on `(agent_session_id, hash_key)` — agent-first decrypt lookup (added 2026-05-08)

> For the full design rationale of the agent-first lookup convention shared
> with `dag_runs` and `llm_node_history`, see
> [`30_database_schema.md`](30_database_schema.md#the-shared-pattern-agent_session_id-first-lookup).

**Required PostgreSQL extension**: `pgcrypto`, enabled by migration
`20260425000002_secure_value_mappings.sql`. The migration is a no-op if the
extension is already present.

---

### `provider_file_cache`  *(PostgreSQL only)*

Caches uploads to provider Files APIs (Anthropic, OpenAI, Gemini) keyed by
`(document_id, provider)`. Lets the engine reuse a previously-uploaded
`provider_file_id` across runs and sessions instead of re-uploading the same
multi-MB document on every LLM call. Backs the large-files feature (signed
URLs ≥ 30 MB) — see [`28_large_files_api.md`](28_large_files_api.md) for the
full flow.

| Column | Type | Nullable | Default | Description |
|--------|------|----------|---------|-------------|
| `document_id` | `TEXT` | NO | — | Caller-provided identifier (the `id` field in the `files` array of the `llm_call` node config). Stable across runs |
| `provider` | `TEXT` | NO | — | Lowercase provider name (`anthropic`, `openai`, `google`, `mock`). Validated by `parse_provider_from_row` — fail-fast on corrupted strings |
| `provider_file_id` | `TEXT` | NO | — | Opaque identifier returned by the provider's Files API after a successful upload (e.g., Anthropic `file_01abc`, OpenAI `file-...`, Gemini `files/...`) |
| `mime_type` | `TEXT` | NO | — | MIME type the file was uploaded with |
| `filename` | `TEXT` | NO | — | Filename submitted to the provider (used by some adapters for display) |
| `size_bytes` | `BIGINT` | YES | — | Hint from the emitter; not authoritative ground truth |
| `uploaded_at` | `TIMESTAMPTZ` | NO | `NOW()` | Wall-clock time of the successful upload |
| `expires_at` | `TIMESTAMPTZ` | YES | — | When the provider expires the file. Set to `uploaded_at + 48h` for Gemini; `NULL` for Anthropic/OpenAI (no expiry) |
| `last_used_at` | `TIMESTAMPTZ` | NO | `NOW()` | Touched on **every** cache hit via `UPDATE ... RETURNING` (not just on upsert). Foundation for future LRU eviction and activity metrics |

**Constraints**

- `PRIMARY KEY (document_id, provider)` — same document can live in multiple
  providers' caches simultaneously.

**Read semantics**

`lookup(document_id, provider)` runs:

```sql
UPDATE provider_file_cache
   SET last_used_at = NOW()
 WHERE document_id = $1 AND provider = $2
RETURNING document_id, provider, provider_file_id, mime_type, filename,
          size_bytes, uploaded_at, expires_at, last_used_at;
```

Touching `last_used_at` atomically with the read keeps the column accurate
without a second round-trip. If no row matches, `UPDATE` returns 0 rows —
same outcome as a `SELECT` MISS. The `is_likely_alive` heuristic
(`expires_at - NOW() > 5 min`) is applied in code, not SQL, so the
heuristic margin can evolve without touching the DB.

**Write semantics**

`upsert` uses `INSERT ... ON CONFLICT (document_id, provider) DO UPDATE SET ...`
with `last_used_at = NOW()` — every write is also a "touch". `invalidate`
deletes the row outright; the file in the provider stays alive (orphaned,
tracked in [tech-debt #4+#5](../superpowers/specs/2026-05-02-large-document-files-api-tech-debt.md#4--5-huérfanos-cache-rows--provider-files)).

**Connection scope**

Always uses `DATABASE_URL` (the engine's internal pool, via
`PgPoolRegistry`), regardless of any per-node `connection_url`. The cache is
transversal to all `llm_call` nodes and runs.

If `DATABASE_URL` is not set, `PostgresFileCache` is not built and the
feature degrades gracefully: every `llm_call` re-uploads on every run (no
cache hits), but other paths keep working.

---

### `conversation_attachments`

Per-`agent_session_id` registry of files attached to a conversation. Where
`provider_file_cache` is keyed by `(document_id, provider)` and is shared
across every run that re-uses the same document id, `conversation_attachments`
is keyed by `(agent_session_id, document_id, provider)` and tracks which files
have been bound to a specific chat — so that follow-up turns of the same agent
can find and re-attach them without the caller re-supplying the bytes.

Migrations: `20260513000001_conversation_attachments.sql` creates the table;
`20260525000001_attachment_uniform_resolution.sql` adds `storage_key`,
`origin`, `last_used_at`, and the activity index. Available on both
PostgreSQL and SQLite (SQLite mirror uses TEXT/INTEGER types and stores
timestamps as ISO-8601 strings).

| Column | Postgres type | SQLite type | Nullable | Default | Description |
|--------|---------------|-------------|----------|---------|-------------|
| `agent_session_id` | `TEXT` | `TEXT` | NO | — | Chat handle this attachment belongs to. Part of the composite primary key |
| `document_id` | `TEXT` | `TEXT` | NO | — | Caller-supplied document id (same convention as `provider_file_cache.document_id`). Part of the PK |
| `provider` | `TEXT` | `TEXT` | NO | — | Lowercase provider name (`anthropic`, `openai`, `google`, `generated`, …). Part of the PK |
| `provider_file_id` | `TEXT` | `TEXT` | NO | — | Opaque identifier returned by the provider's Files API |
| `mime_type` | `TEXT` | `TEXT` | NO | — | MIME type the file was registered with |
| `filename` | `TEXT` | `TEXT` | NO | — | Display name |
| `size_bytes` | `BIGINT` | `INTEGER` | YES | — | Size hint from the emitter |
| `label` | `TEXT` | `TEXT` | YES | — | Optional short label shown to the model / UI |
| `description` | `TEXT` | `TEXT` | YES | — | Optional long-form description |
| `source_kind` | `TEXT` | `TEXT` | NO | — | Where the file originated — e.g. `user_upload`, `generated`, `tool_output` |
| `source_value` | `TEXT` | `TEXT` | YES | — | Free-form pointer back to the source (URL, node id, etc.) |
| `registered_at` | `TIMESTAMPTZ` | `TEXT` | NO | `NOW()` / `CURRENT_TIMESTAMP` | First time the file was bound to this chat |
| `refreshed_at` | `TIMESTAMPTZ` | `TEXT` | NO | `NOW()` / `CURRENT_TIMESTAMP` | Updated when the row is re-registered (e.g. provider file rotated) |
| `storage_key` | `TEXT` | `TEXT` | YES | — | Reference into `OutputStorageRepository` for uniform attachment resolution (added 2026-05-25) |
| `origin` | `TEXT` | `TEXT` | YES | — | Semantic origin — e.g. `user_upload`, `generated_by:<node_id>`. Backfilled from `provider` for legacy rows (added 2026-05-25) |
| `last_used_at` | `TIMESTAMPTZ` | `TEXT` | YES | — | Touched on cache hit; used as the TTL clock for attachment eviction (added 2026-05-25) |

**Constraints**

- `PRIMARY KEY (agent_session_id, document_id, provider)` — same document can
  be attached to multiple agent sessions and live in multiple providers'
  caches simultaneously, but only once per `(session, provider)`.

**Indexes**

- `idx_conversation_attachments_session` on `(agent_session_id)` — list attachments for a chat
- `idx_conv_attachments_session_used` on `(agent_session_id, last_used_at)` — TTL / activity sweeps

**Relationship to `provider_file_cache`**: the two tables are complementary.
`provider_file_cache` is a global, session-agnostic upload cache to avoid
re-uploading the same bytes. `conversation_attachments` is the per-chat
binding layer that says "this provider file is currently attached to *this*
agent's conversation." A single upload may have one row in
`provider_file_cache` and zero-or-more rows in `conversation_attachments`.
There are no FK constraints between them — the link is by convention on
`(document_id, provider)`.

---

### `crdt_doc_events`

Per-artifact change log for the CRDT documents subsystem. Every mutation
applied to a `yrs::Doc` workbook (cell write, range write, sheet add, etc.)
records a one-line human-readable summary here so that LLM agents can ask
"what happened in this workbook since I last looked?" via the
`crdt_doc_get_recent_changes` synthetic tool. The CRDT engine itself stays
in memory + on-disk snapshot; this table is the durable side-channel for
narration.

Migration: `20260603000000_crdt_doc_changes.sql`. Available on both
PostgreSQL and SQLite (SQLite mirror uses `INTEGER PRIMARY KEY AUTOINCREMENT`
instead of `BIGSERIAL`, and stores timestamps as ISO-8601 `TEXT`).

| Column | Postgres type | SQLite type | Nullable | Default | Description |
|--------|---------------|-------------|----------|---------|-------------|
| `id` | `BIGSERIAL` | `INTEGER` (autoincrement) | NO | auto | Monotonic event id — used as the cursor checkpoint by `crdt_doc_session_cursors.last_event_id` |
| `artifact_id` | `TEXT` | `TEXT` | NO | — | Workbook handle (CRDT doc id, ULID-shaped) |
| `sheet_id` | `TEXT` | `TEXT` | YES | — | Sheet inside the workbook the event refers to. `NULL` for workbook-level events (e.g. `add_sheet`) |
| `origin` | `TEXT` | `TEXT` | NO | — | Free-form attribution — typically `agent:<id>`, `user:<id>`, or `python:<script>` |
| `summary` | `TEXT` | `TEXT` | NO | — | One-line narration produced by the `ChangeTracker` (e.g. `"Pricing: 2 changes by agent:orchestrator"`) |
| `created_at` | `TIMESTAMPTZ` | `TEXT` | NO | `NOW()` / `CURRENT_TIMESTAMP` | Event wall-clock time |

**Indexes**

- `crdt_doc_events_lookup` on `(artifact_id, id)` — fetch the tail of events for one workbook in order
- `crdt_doc_events_by_sheet` on `(artifact_id, sheet_id, id)` — same, scoped to a single sheet

There are no FK constraints — the `artifact_id` is owned by the CRDT runtime,
which may evict a workbook from disk; we keep the change log either way so a
later session can re-import the workbook and still read its history.

---

### `crdt_doc_session_cursors`

Per-`agent_session_id` bookmark into `crdt_doc_events`. Lets each agent ask
for "events since I last checked" without re-emitting events it already
narrated to the LLM.

Migration: `20260603000000_crdt_doc_changes.sql`. Same shape on both backends
(only the timestamp type differs).

| Column | Postgres type | SQLite type | Nullable | Default | Description |
|--------|---------------|-------------|----------|---------|-------------|
| `agent_session_id` | `TEXT` | `TEXT` | NO | — | Stable agent handle (same identifier as everywhere else). Part of the composite PK |
| `artifact_id` | `TEXT` | `TEXT` | NO | — | Workbook the cursor points at. Part of the PK |
| `last_event_id` | `BIGINT` | `INTEGER` | NO | — | Last `crdt_doc_events.id` already shown to this agent. Next call returns events with id > this value |
| `updated_at` | `TIMESTAMPTZ` | `TEXT` | NO | `NOW()` / `CURRENT_TIMESTAMP` | When the cursor was last advanced |

**Constraints**

- `PRIMARY KEY (agent_session_id, artifact_id)` — exactly one cursor per
  agent-workbook pair. `INSERT … ON CONFLICT DO UPDATE` on advance.

---

### `crdt_doc_session_artifacts`

Per-agent list of CRDT workbooks the agent has ever touched, sorted by
recency. Backs the catalog the LLM sees when it asks "which workbooks do I
have available?" and feeds the LRU eviction logic that keeps idle workbooks
out of memory.

Migration: `20260603000000_crdt_doc_changes.sql`. Same shape on both backends
(timestamp type differs).

| Column | Postgres type | SQLite type | Nullable | Default | Description |
|--------|---------------|-------------|----------|---------|-------------|
| `agent_session_id` | `TEXT` | `TEXT` | NO | — | Stable agent handle. Part of the composite PK |
| `artifact_id` | `TEXT` | `TEXT` | NO | — | Workbook handle. Part of the PK |
| `name` | `TEXT` | `TEXT` | NO | — | Display name shown to the LLM and the UI |
| `created_at` | `TIMESTAMPTZ` | `TEXT` | NO | `NOW()` / `CURRENT_TIMESTAMP` | First time this agent saw the workbook |
| `last_accessed_at` | `TIMESTAMPTZ` | `TEXT` | NO | `NOW()` / `CURRENT_TIMESTAMP` | Touched on every read/write. Drives the recency index |

**Constraints**

- `PRIMARY KEY (agent_session_id, artifact_id)` — one row per
  agent-workbook pair, updated on every touch.

**Indexes**

- `crdt_doc_session_artifacts_recent_idx` on
  `(agent_session_id, last_accessed_at DESC)` — list a single agent's
  workbooks most-recent-first; used both by the catalog query and by the
  LRU eviction sweep that closes idle workbooks.

---

## SQL sandbox tables  *(PostgreSQL only, runtime-created)*

These tables are **not** managed by `sqlx::migrate!()` — they are created
lazily by the `sql` node infrastructure
([`sql_function_registry.rs`](../../src/libs/colmena/src/dag_engine/infrastructure/sql_function_registry.rs))
the first time a graph that uses the SQL function registry runs against a
given database. They live inside the configurable sandbox schema (default
`sandbox`, configurable via the `sandbox_schema` field on `sql` node
permissions).

The `PgRegistryAdapter::ensure_schema()` call:

1. `CREATE SCHEMA IF NOT EXISTS <sandbox_schema>`
2. `CREATE TABLE IF NOT EXISTS <sandbox_schema>.function_registry (…)`
3. `COMMENT ON TABLE function_registry IS '…'`
4. `CREATE TABLE IF NOT EXISTS <sandbox_schema>.query_feedback (…)`
5. `COMMENT ON TABLE query_feedback IS '…'`

All five statements are issued one-by-one (sqlx forbids mixing DDL with
`COMMENT` in a single multi-statement query).

### `<sandbox>.function_registry`

Catalogs SQL functions created by AI agents inside the sandbox schema. The
agent can list previously-registered helper functions and reuse them rather
than re-deriving the same SQL from scratch.

| Column | Type | Nullable | Default | Description |
|--------|------|----------|---------|-------------|
| `id` | `SERIAL` | NO | auto | Surrogate primary key |
| `function_name` | `TEXT` | NO | — | Name of the registered function (without schema prefix) |
| `schema_name` | `TEXT` | NO | `'<sandbox_schema>'` | Schema where the function lives — defaulted to the configured sandbox schema |
| `parameters` | `TEXT` | YES | — | Free-form parameter signature (e.g. `"id INT, name TEXT"`) |
| `return_type` | `TEXT` | YES | — | Free-form return type description |
| `description` | `TEXT` | NO | — | Natural-language description of what the function does |
| `created_by_session` | `TEXT` | YES | — | `session_id` of the run that registered this function |
| `created_at` | `TIMESTAMPTZ` | YES | `NOW()` | When the function was first registered. The `register_function` upsert resets this on conflict |
| `last_used_at` | `TIMESTAMPTZ` | YES | — | Reserved for future hit-tracking; not currently written |
| `usage_count` | `INT` | YES | `0` | Reserved for future hit-tracking; not currently written |

**Constraints**

- `UNIQUE(schema_name, function_name)` — drives the upsert in
  `register_function`: registering an existing name overwrites parameters,
  return type, description, and `created_by_session`.

### `<sandbox>.query_feedback`

History of feedback emitted on agent-generated queries. Feedback comes from
two sources: the static SQL validator (rejects, warnings) and the LLM critic
(quality / correctness opinions). Used by the agent to improve subsequent
attempts.

| Column | Type | Nullable | Default | Description |
|--------|------|----------|---------|-------------|
| `id` | `SERIAL` | NO | auto | Surrogate primary key |
| `session_id` | `TEXT` | NO | — | Run that produced the offending query |
| `query_text` | `TEXT` | NO | — | The SQL that the feedback applies to |
| `feedback_type` | `TEXT` | NO | — | Category — e.g. `error`, `warning`, `suggestion` (free-form, set by the caller) |
| `source` | `TEXT` | NO | — | Origin of the feedback — e.g. `static_validator`, `llm_critic` |
| `message` | `TEXT` | NO | — | Human-readable feedback text |
| `created_at` | `TIMESTAMPTZ` | YES | `NOW()` | When the feedback was recorded |

No indexes are declared — the table is small and accessed by
`session_id`/recency at most.

---

## Row-Level Security on user-created tables

When a graph executes `CREATE TABLE` against a database where the SQL node has
`auto_rls = true` (default for the `restricted` permission profile),
`PgPoolAdapter::setup_rls_for_new_table` is invoked immediately after the DDL
([`nodes/sql.rs:391`](../../src/libs/colmena/src/dag_engine/infrastructure/nodes/sql.rs)).
This is *not* a schema migration but a runtime side-effect — included here
for completeness because it changes the shape of every user table.

Behavior depends on whether the table includes the configured tenant column
(default `user_id`):

| Has tenant column? | Action taken | Resulting policy |
|--------------------|--------------|------------------|
| Yes | Enable + force RLS, set column default to `current_setting('app.current_user_id')` | `colmena_tenant_isolation` — `USING/WITH CHECK (tenant_col = current_setting('app.current_user_id'))` |
| No | Auto-add tenant column with the same `DEFAULT`, then proceed as above | `colmena_tenant_isolation` (after column injection) |
| No (and auto-injection disabled) | Enable + force RLS only | `colmena_shared_read` — `FOR SELECT USING (true)` (read-only) |

Both `ENABLE ROW LEVEL SECURITY` and `FORCE ROW LEVEL SECURITY` are issued so
the table owner cannot bypass the policy. The tenant context is set per-query
via `SELECT set_config('app.current_user_id', $1, true)` inside the
transaction that wraps every executed statement.

The two engine-managed sandbox tables (`function_registry`, `query_feedback`)
are **not** RLS-protected — they are registry/log tables shared across
sessions.

---

## Entity relationships

```
dag_runs ──< dag_task_memory       (dag_runs.session_id = dag_task_memory.session_id)
dag_runs ──< dag_phase_summaries   (dag_runs.session_id = dag_phase_summaries.session_id)
dag_runs ──< secure_value_mappings (dag_runs.session_id = secure_value_mappings.session_id)
dag_runs ──< dag_runs              (parent_session_id → session_id)   subgraph child tree

llm_node_history ── standalone
                    new reads keyed by (agent_session_id, node_id)
                    legacy reads by (session_id, node_id)

provider_file_cache ── standalone
                       keyed by (document_id, provider) — caller-supplied id,
                       orthogonal to dag_runs sessions

conversation_attachments ── standalone, per agent_session_id
                            keyed by (agent_session_id, document_id, provider)
                            convention-linked to provider_file_cache by
                            (document_id, provider); no FK

crdt_doc_events            ── standalone, append-only per artifact_id
crdt_doc_session_cursors   ──> crdt_doc_events  (cursors.last_event_id → events.id;
                                                 convention only, no FK)
crdt_doc_session_artifacts ── standalone, per (agent_session_id, artifact_id);
                              convention-linked to events by artifact_id; no FK

sandbox.function_registry  ── standalone, shared across sessions
sandbox.query_feedback     ── standalone, scoped by session_id (column, no FK)
```

There are **no foreign-key constraints** between any of these tables. All
links are by convention on `session_id` / `agent_session_id`. This keeps
cross-database flexibility (e.g. `llm_node_history` may live in a different
Postgres instance than `dag_runs`) and avoids cascade-delete surprises during
HITL retries.

---

## Connection configuration

The engine uses `DATABASE_URL` (environment variable, mapped to
`EngineConfig.internal_database_url`) as its **internal database** — the one
where DAG state, tasks, phase summaries, and secure values are stored. Any
PostgreSQL URL configured on an `llm_call` node's `connection_url` field gets
the same migrations applied lazily, but in practice will only store
`llm_node_history` rows.

Set `DATABASE_URL` before starting the engine:

```bash
export DATABASE_URL="postgresql://user:pass@host:5432/dbname"
```

Pool behaviour is controlled by:

| Variable | Default | Description |
|----------|---------|-------------|
| `COLMENA_POOL_MAX_ENTRIES` | 100 | Maximum number of distinct connection pools held by `PgPoolRegistry` |
| `COLMENA_POOL_MAX_CONN_PER_URL` | 2 | Connections per pool |
| `COLMENA_POOL_MIN_CONN_PER_URL` | 0 | Always-open connections per pool |
| `COLMENA_POOL_IDLE_TIMEOUT_SEC` | 30 | Idle connection timeout (seconds) |
| `COLMENA_POOL_MAX_LIFETIME_SEC` | 600 | Max connection lifetime (seconds) |
| `COLMENA_POOL_ACQUIRE_TIMEOUT_SEC` | 10 | Timeout waiting for a connection from the pool |

The internal pool is *pinned* at engine startup (`registry.pin(...)`) so it
survives idle eviction; node-specific pools obtained via `get_or_create` are
subject to the eviction policy above.
