# Agent Session ID — Design

**Status**: Draft
**Date**: 2026-04-28
**Author**: Daniel Garcia (brainstormed with Claude)

## 1. Problem statement

Today the DAG engine has a single identifier per execution — `session_id` — generated as a UUID v4 at run start. This identifier serves two distinct purposes simultaneously:

1. **Storage handle for one DAG execution**: primary key of `dag_runs`, used by the suspend/resume mechanism.
2. **LLM memory key**: every `llm_call` node inside the run writes to `llm_node_history` keyed by this same `session_id`.

This conflation works for one-shot scripts but breaks down for chat-style applications where:

- A conversation spans **multiple runs over time** (each user message triggers a fresh execution, but the assistant should remember earlier turns).
- A run can suspend at any depth (root, subgraph, sub-subgraph), and the external chat client wants to resume by passing only the conversation handle, not the deepest run's UUID.
- A grafo can contain multiple `llm_call` nodes that today silently share the same conversation history because `__colmena_session_id` overrides any per-node configuration.

The fix is to introduce a **chat-scoped identifier** — `agent_session_id` — that lives above `session_id` and groups all runs (and their subgraphs) belonging to the same conversation.

## 2. Goals and non-goals

### Goals

- Add an `agent_session_id` field that the caller can pass instead of (or in addition to) `--session-id`.
- Resolve suspended state by `agent_session_id` alone: the engine finds the deepest suspended run in the conversation tree and continues from there.
- Persist LLM memory across runs of the same conversation: when a run completes and a new one starts under the same `agent_session_id`, `llm_call` nodes load prior history automatically.
- Eliminate the silent collision between multiple `llm_call` nodes in the same run by adding `node_id` to the LLM history key.
- Maintain full backward compatibility: any existing graph or call site that does not use `agent_session_id` behaves exactly as today.

### Non-goals

- Multiple suspended leaves in parallel under one `agent_session_id`. The design assumes at most one leaf is awaiting user input at a time. (See Future work.)
- Cross-`agent_session_id` memory sharing.
- Garbage collection / TTL of completed conversations beyond what already exists.
- API for listing conversation history or visualizing the run tree (the schema supports it, but the endpoints are out of scope here).

## 3. Schema changes

### 3.1 `dag_runs` — add two columns

```sql
ALTER TABLE dag_runs
    ADD COLUMN agent_session_id VARCHAR(255),
    ADD COLUMN parent_session_id VARCHAR(255);

CREATE INDEX idx_dag_runs_agent_session_id ON dag_runs(agent_session_id);
CREATE INDEX idx_dag_runs_parent_session_id ON dag_runs(parent_session_id);
CREATE INDEX idx_dag_runs_agent_status ON dag_runs(agent_session_id, status);
```

| Column | Type | Nullable | Purpose |
|---|---|---|---|
| `agent_session_id` | `VARCHAR(255)` | YES | Chat / conversation handle. NULL for legacy runs that didn't pass one. |
| `parent_session_id` | `VARCHAR(255)` | YES | `session_id` of the immediate parent run when this row is a subgraph. NULL for root runs. |

`session_id` remains the primary key. The new columns are aditive and nullable; existing rows keep working.

The `parent_session_id` column is included from day one even though the leaf-resolution query does not strictly require it, because (a) it lets a future UI render the tree of runs cheaply, and (b) it makes the relationship explicit instead of implicit in a string prefix.

### 3.2 `llm_node_history` — add two columns and change the read key

```sql
ALTER TABLE llm_node_history
    ADD COLUMN agent_session_id TEXT,
    ADD COLUMN node_id TEXT;

CREATE INDEX idx_llm_history_agent_node ON llm_node_history(agent_session_id, node_id, created_at);
CREATE INDEX idx_llm_history_session_node ON llm_node_history(session_id, node_id, created_at);
```

| Column | Type | Nullable | Purpose |
|---|---|---|---|
| `agent_session_id` | `TEXT` | YES | Conversation handle. NULL for legacy rows / runs that didn't pass one. |
| `node_id` | `TEXT` | YES | Path-qualified identifier of the `llm_call` node that wrote this row. NULL for legacy rows. |

`session_id` is preserved on every row (informational / auditing — "this message was generated during run X"). The change is which columns we **read** by:

- **New behavior** when the run carries an `agent_session_id`:
  ```sql
  SELECT ... FROM llm_node_history
   WHERE agent_session_id = $1 AND node_id = $2
   ORDER BY created_at;
  ```
- **Legacy fallback** when `agent_session_id IS NULL`:
  ```sql
  SELECT ... FROM llm_node_history
   WHERE session_id = $1 AND node_id = $2
   ORDER BY created_at;
  ```
- **Pre-migration rows** (where `node_id IS NULL`) are excluded by the new query because they predate the `node_id` concept. This is intentional: those rows belong to the old conflated keying scheme and should not contaminate per-node histories. They remain in the table for auditing and can be cleaned up by a separate maintenance task.

### 3.3 Migration files

Two new files under `src/libs/colmena/migrations/postgres/`:

| File | Purpose |
|---|---|
| `20260428000001_dag_runs_agent_session_id.sql` | Adds `agent_session_id`, `parent_session_id` columns + indices. |
| `20260428000002_llm_history_agent_and_node.sql` | Adds `agent_session_id`, `node_id` columns + indices. |

Both are aditive — no data backfill needed. SQLite migration files (if separate) follow the same shape.

## 4. Lifecycle and state machine

### 4.1 Run start

When the engine receives an execution request, it processes inputs in this order:

```
Input parameters: { agent_session_id?, session_id?, answer? }

1. If session_id is provided:
     → Direct resume by primary key (existing behavior).
     → If the row exists, load and continue.
     → agent_session_id from the row is preserved as authoritative.
     → If the caller also passed agent_session_id and they conflict, error out.

2. Else if agent_session_id is provided:
     → Look up dag_runs WHERE agent_session_id = $1 ORDER BY updated_at DESC.
     → If a SUSPENDED row exists in the tree:
         a. Resolve the leaf: SUSPENDED row that is NOT a parent of any other SUSPENDED row.
         b. Load its state. Inject `answer` as the resume value. Continue execution.
     → Else (no suspended runs, or only COMPLETED/FAILED rows):
         a. Create a new root run: session_id = uuid_v4(),
            agent_session_id = <given>, parent_session_id = NULL.
         b. Run the graph from scratch.
         c. LLM nodes inside the run will automatically inherit prior conversation
            history because they read by (agent_session_id, node_id).

3. Else (neither provided):
     → Pure legacy path. Generate session_id = uuid_v4(),
       agent_session_id = NULL, parent_session_id = NULL.
     → LLM nodes read by (session_id, node_id) — current behavior.
```

### 4.2 Leaf resolution query

```sql
SELECT session_id
  FROM dag_runs
 WHERE agent_session_id = $1
   AND status = 'SUSPENDED'
   AND session_id NOT IN (
       SELECT parent_session_id
         FROM dag_runs
        WHERE agent_session_id = $1
          AND parent_session_id IS NOT NULL
   )
 LIMIT 1;
```

Per the chosen scope (single leaf at a time), this query returns 0 or 1 rows. If it returns 0, no leaf is awaiting and the engine starts a fresh root run. If it returns more than one (shouldn't happen but defensive), the engine errors out so the caller can decide what to do.

### 4.3 Run completion

When a root run finishes (`status = 'COMPLETED'`), no special action: its row stays in `dag_runs` for history. The next call with the same `agent_session_id` will see the COMPLETED row, find no SUSPENDED leaf, and start a fresh root run.

Subgraph runs naturally complete before their parent (the parent is what waits for them). When a subgraph completes, the subgraph node in the parent collects its outputs and continues; the subgraph's `dag_runs` row stays as part of the conversation tree.

### 4.4 Subgraph spawn

The `subgraph` node today derives a child `session_id` as `format!("{}_sub_{}", parent, node_id)`. Under this design:

- Child `session_id` becomes a fresh UUID (no longer string-derived).
- Child `dag_runs` row carries `agent_session_id` inherited from the parent row.
- Child `dag_runs` row carries `parent_session_id` set to the parent run's `session_id`.
- The conversation tree is now navigable via `parent_session_id`, regardless of how subgraph IDs are formatted.

This is a breaking change for anyone introspecting the string-prefix convention, but the convention is internal and undocumented — see Backward compat for guards.

## 5. API surface

### 5.1 CLI

```
cargo run --bin dag_engine -- run <graph.json>
    [--agent-session-id <chat_handle>]
    [--session-id <run_uuid> | --resume-id <run_uuid>]
    [--answer <text>]
    [--include-extra-info]
```

- `--agent-session-id` is new. When passed alone it triggers the chat-style lifecycle (Section 4.1, branch 2).
- `--session-id` keeps its current meaning: direct resume by run UUID. Both flags can coexist for advanced cases (debug a specific subgraph run inside a known chat).
- If both are passed and they conflict (the named `session_id` row has a different `agent_session_id`), the engine errors before executing.

### 5.2 HTTP (Serve mode)

The Axum server exposes the same flow via:

- Header `X-Agent-Session-Id: <chat_handle>`, OR
- Body field `agent_session_id` in the JSON payload.

Headers take precedence over body fields when both are present. The existing `session_id` and `answer` body fields are unchanged.

### 5.3 Internal propagation

The engine injects two keys into every node's `inputs` map at execution time:

- `__colmena_session_id` (already exists) — the current run's UUID.
- `__colmena_agent_session_id` (new) — the conversation handle, or `null` if absent.

Nodes that need either value (today only `llm_call`, `subgraph`, and the secure-value layer) read from these keys.

## 6. LLM memory model

### 6.1 Read priority for the history key

In [llm.rs](src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs), the resolution becomes:

```rust
// agent_session_id presence — the key is always injected by the engine,
// but it carries a non-null string only when the caller passed one.
let agent_id: Option<&str> = inputs
    .get("__colmena_agent_session_id")
    .and_then(|v| v.as_str())          // None when JSON null or missing
    .filter(|s| !s.is_empty());

// session_id (always present, current behavior).
let session_id = inputs
    .get("__colmena_session_id").and_then(|v| v.as_str())
    .expect("engine always injects this");

// Per-node identity (NEW): path-qualified node_id passed by the engine.
let node_id_qualified = inputs
    .get("__colmena_node_id_path").and_then(|v| v.as_str())
    .expect("engine must inject this");

// Mode flag: read by agent_session_id when present, else legacy session_id.
let history_mode = match agent_id {
    Some(_) => HistoryMode::Conversation,   // (agent_session_id, node_id)
    None    => HistoryMode::Run,            // (session_id, node_id) — legacy
};
```

The repository implementation switches its query based on `history_mode`. Writes always include `agent_session_id` (may be NULL), `session_id`, and `node_id` so the row is fully attributed; only the read filter changes.

### 6.2 Path-qualified `node_id`

The engine computes a path string for every node before invoking it, walking from the root through any subgraph parents:

- Root-level node `router` → `"router"`
- Node `responder` inside a subgraph node `ventas` → `"ventas/responder"`
- Same in a sub-subgraph `inner` → `"ventas/inner/responder"`

The path uses `/` as separator. `/` is rejected from user-defined node IDs at graph load time (validation step) to prevent ambiguity. This validation is a small additional check in the graph loader — see Migration plan.

The path is injected into every node's `inputs` as `__colmena_node_id_path`. Today nodes already know their own ID via context; this just surfaces the qualified version in a uniform place.

### 6.3 Cross-run memory continuity

When a chat's first run completes and a second message arrives:

1. Engine creates a new root run (new `session_id`, same `agent_session_id`).
2. Each `llm_call` node, when reading history, queries by `(agent_session_id, node_id)` — no `session_id` filter.
3. Rows from the previous run are returned chronologically, so the new run's LLM sees the full prior conversation.
4. New rows written by the new run carry the new `session_id` in their column for auditing, but read queries don't filter by it.

Subgraphs work the same way: a subgraph's inner `llm_call` reads by `(agent_session_id, "ventas/responder")`. If this is the second time a `ventas` subgraph runs in the chat, the inner LLM picks up history from the first run automatically because the path is stable across runs of the same graph definition.

### 6.4 Caveat: changing the graph mid-conversation

If the user changes the graph definition between runs (renames a node, restructures subgraphs), node paths shift and the history of the renamed node becomes orphaned (still in the table, but no node reads it anymore). This is acceptable — same risk exists today with `thread_id` configurations. We document it in the migration notes.

## 7. Subgraph behavior

### 7.1 Recap of changes to `subgraph.rs`

| Concern | Today | Under this design |
|---|---|---|
| Child `session_id` | `format!("{parent}_sub_{node_id}")` | `Uuid::new_v4().to_string()` |
| Child `agent_session_id` | (does not exist) | inherited from parent `__colmena_agent_session_id` |
| Child `parent_session_id` | implicit in the string | written explicitly to `dag_runs` |
| Child node path prefix | (not used) | `<parent_path>/<subgraph_node_id>` |

### 7.2 Why the deterministic `_sub_` derivation is dropped

The string-derivation guaranteed the same child `session_id` if the parent re-spawned the same subgraph node. Under the new model:

- The `dag_runs` row uniqueness comes from the explicit (parent_session_id, role-in-parent) relationship via `parent_session_id`.
- LLM memory continuity comes from path-qualified `node_id` keyed by `agent_session_id`, which is more robust than relying on the child run UUID.

So the deterministic UUID is no longer load-bearing. We replace it with a fresh UUID for clarity and to remove an implicit invariant.

## 8. Backward compatibility

### 8.1 Graphs that don't use `agent_session_id`

Behavior is **byte-identical** to today, with one improvement and one trivial change:

- Improvement: `llm_node_history` now also carries `node_id` per row, so users running multiple `llm_call` nodes in one graph stop interleaving their conversations. (This is technically a behavior change — see 8.3.)
- Trivial change: pre-existing rows without `node_id` are excluded from new reads, but they were already useless (interleaved). No data loss.

### 8.2 CLI scripts using only `--session-id`

Unchanged — branch 1 of section 4.1. The flag still works exactly as before.

### 8.3 `llm_node_history` interleaving fix is a soft semantic change

The fix to silently-shared LLM histories is technically a behavior change for anyone who today relies on (or tolerates) the interleaving. This includes graphs where two `llm_call` nodes in sequence both pulled from the same merged history. After the change, each node has its own history; the second node's history is empty until it speaks for itself.

We accept this because:
- The current behavior is a documented bug (the docs claim per-node `thread_id` works, which the code ignores).
- No known user relies on the bug intentionally.
- The new behavior matches the documented intent.

We note it in the release/migration notes.

### 8.4 The `subgraph._sub_` string convention

If any external tool inspects `dag_runs.session_id` looking for the `_sub_` prefix, it breaks. We do not believe any such tool exists, but the migration note flags it.

## 9. Migration plan

### 9.1 Schema (run automatically on engine boot via `sqlx::migrate!()`)

1. `20260428000001_dag_runs_agent_session_id.sql` — adds `agent_session_id`, `parent_session_id`, indices.
2. `20260428000002_llm_history_agent_and_node.sql` — adds `agent_session_id`, `node_id`, indices.

Both aditive, idempotent, no backfill.

### 9.2 Code rollout, in order

1. Add graph-loader validation: reject node IDs containing `/`. (Must come first so the path-qualifier in step 2 has no ambiguity.)
2. Compute path-qualified `node_id` and inject `__colmena_agent_session_id` + `__colmena_node_id_path` into every node's inputs. No behavior change yet.
3. Update `llm_call` repository to write the new `agent_session_id` and `node_id` columns. Read path still legacy.
4. Switch read path in `llm_call`: prefer `(agent_session_id, node_id)` keying when present; fall back to `(session_id, node_id)` otherwise.
5. Update `subgraph` node: write `parent_session_id` to `dag_runs`, use a fresh UUID for the child `session_id`, propagate the inherited `agent_session_id` and the extended path prefix.
6. Add CLI/HTTP surface (`--agent-session-id`, `X-Agent-Session-Id` header / body field).
7. Implement the lifecycle decision logic in `run_use_case.rs` (Section 4.1 branches 1–3) including the leaf-resolution query and the conflict check.
8. Update docs (`docs/developer_guide/30_database_schema.md`, `15_memory_guide.md`, `19_nested_agents_and_subgraphs.md`).

### 9.3 Test matrix

| Scenario | Coverage |
|---|---|
| Legacy: no `agent_session_id`, single `llm_call`, memory persists by `session_id` | existing tests |
| Legacy: no `agent_session_id`, two `llm_call` nodes — each gets its own history (new) | new test |
| Chat: first message, no prior state, root run created with `agent_session_id` | new test |
| Chat: second message, prior run COMPLETED, new run inherits LLM history | new test |
| Chat: prior run SUSPENDED at root, resume by `agent_session_id` only | new test |
| Chat: prior run SUSPENDED in subgraph leaf, resume by `agent_session_id` only | new test |
| Chat: prior run SUSPENDED in sub-subgraph leaf, resume by `agent_session_id` only | new test |
| Subgraph: same subgraph definition runs twice, inner `llm_call` keeps history | new test |
| Subgraph: two distinct subgraph instances with same internal node id, histories isolated | new test |
| Validation: graph with `/` in a node id is rejected at load | new test |
| Conflict: `--agent-session-id A` + `--session-id <uuid>` whose row has `agent_session_id B` → error | new test |

## 10. Future work

- **Multiple suspended leaves under one `agent_session_id`** (Pregunta 2 / option C): would require a `pending_question_id` or similar disambiguator. Out of scope here.
- **Listing / visualizing the conversation tree**: now trivial to query, but no API exposed.
- **TTL / retention** of completed conversations: today nothing expires runs; orthogonal.
- **Renaming the legacy `thread_id` references** in docs to match the actual `session_id` field the code reads.
- **Cleanup utility** to delete `llm_node_history` rows where `node_id IS NULL` after operators confirm legacy data is no longer needed.

## 11. Open questions

None at this point — all major decisions resolved during brainstorming. The implementation plan (next document) will refine ordering and test details.
