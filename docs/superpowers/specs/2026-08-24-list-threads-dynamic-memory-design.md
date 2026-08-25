# Design — `list_threads` synthetic tool for `dynamic` memory

**Date:** 2026-08-24
**Status:** approved design, pending implementation plan
**Depends on:** `memory_mode` feature (PRs #193/#195/#198, merged to `develop`)

## Problem

A tool with `memory_mode: "dynamic"` keys its conversational memory under
`node_id = tool/<tool_name>/<thread_id>`, where `<thread_id>` is a string the model
invents per call. To **continue** a thread the model must reuse the exact `thread_id`.
After context compaction the model may no longer have the list of thread ids it created,
and — the primary use case here — it wants **general navigation**: to know at any time
which conversations exist with a given sub-agent, so it can list, pick, and continue the
right one.

The `[hilo: <id>]` echo (already shipped) keeps a just-used id in recent history, but does
not let the model enumerate all threads. `list_threads` fills that gap.

## Approach (chosen: A — pull synthetic tool)

A synthetic tool `list_threads` the model calls on demand, exposed only when at least one
configured tool is `dynamic`. Rejected alternatives:

- **B — push a thread catalog into the system message each turn.** Always visible, no
  round-trip, but pays tokens *and* a DB query every turn even when the model does not
  navigate, and grows unbounded as threads accumulate. Wrong tradeoff for "general
  navigation with potentially many threads".
- **C — hybrid (A now, B later).** A is enough; revisit only if a real need for
  always-on visibility appears.

Pull scales to many threads (fetched on demand), costs nothing when unused, and reuses
the established synthetic-tool pattern (`recall_history`).

## Components

1. **New repository method** on `ConversationRepository`
   (`src/libs/colmena/src/llm/domain/memory.rs`), implemented in all three backends
   (`postgres_conversation_repository.rs`, `sqlite_conversation_repository.rs`,
   `in_memory_conversation_repository.rs`):

   ```
   async fn list_node_activity(
       &self,
       keying: (&str, &str),      // ("agent_session_id"|"session_id", value)
       node_id_prefix: &str,      // e.g. "tool/archivador/"
   ) -> Result<Vec<NodeActivity>, LlmError>;
   ```

   where `NodeActivity { node_id: String, message_count: i64, last_activity: String
   (ISO-8601 UTC), opening: Option<String> }`.

   - Query shape (portable across Postgres and SQLite): group `llm_node_history` by
     `node_id` filtered by `<keying_col> = $1 AND node_id LIKE $2`, returning
     `count(*)`, `max(created_at)`, and — via a correlated subquery — the `content` of
     the earliest `role = 'user'` row for that `node_id` (the thread's *opening*).
   - `keying_col` comes from the closed 2-element set (`ConversationKey::keying()`
     convention at `memory.rs:35`), interpolated as an identifier; `keying` value and the
     `LIKE` prefix are **bound params**. The prefix is always built from a validated
     `tool_configurations` key, never from raw model input.

2. **New synthetic tool file** `nodes/llm_synthetic_tools/list_threads.rs`:
   - `pub const TOOL_LIST_THREADS: &str = "list_threads";`
   - `pub fn tool_list_threads() -> ToolDefinition` — one optional `tool` string param.
   - `pub async fn dispatch_list_threads(repo, keying, dynamic_tools, args) -> Value`.

3. **Dispatch branch** in `dag_tool_executor.rs::execute_inner`, next to the
   `RECALL_HISTORY_TOOL` arm: name-match `TOOL_LIST_THREADS`, pull
   `self.conversation_repository` + `self.conversation_key` (for keying) +
   `self.tool_configurations` (for the dynamic-tool set), call the dispatcher, wrap the
   JSON as a `ToolResult`.

4. **Exposure gating** in `nodes/llm.rs` (near where `recall_history` / `load_skill` are
   pushed): push `tool_list_threads()` into the tools vec **only when** at least one
   `ToolConfiguration` has `memory_mode == MemoryMode::Dynamic`. Eager (always visible
   when applicable) — not placed in the lazy `describe_tool` catalog.

5. **Text registry entry** in `text/tools/*.yaml` — description + summary (mandatory:
   `text::tool_description`/`tool_summary` panic at boot if missing).

## Data flow

1. `llm.rs` assembles tools → if any dynamic tool exists, appends `list_threads`.
2. Model calls `list_threads` (optionally with `tool: "<name>"`).
3. `execute_inner` matches `TOOL_LIST_THREADS` → `dispatch_list_threads`.
4. Dispatch resolves target dynamic-tool names:
   - no `tool` arg → all tools whose `memory_mode == Dynamic`.
   - `tool` arg → that one, **if** it is a known dynamic tool; else a correctable error
     listing the available dynamic tool names.
5. For each target name, `prefix = format!("tool/{name}/")`; call
   `repo.list_node_activity(keying, &prefix)`.
6. **Thread-id extraction (in Rust, not SQL — keeps the query DB-agnostic):** for each
   returned `node_id`, strip the `tool/<name>/` prefix and take the first remaining
   path segment as `thread_id`. Group rows by `thread_id`, summing `message_count`,
   taking `max(last_activity)`, and — when a thread spans several node_ids — the
   `opening` whose source `user` message is the earliest.
   - `tool/archivador/proyecto-alfa/keeper` → thread `proyecto-alfa`.
   - `tool/asesor/caso-12` (bare llm_call tool) → thread `caso-12`.
   - A subgraph thread with several internal llm_calls yields several node_ids under one
     `thread_id` → correctly merged into one thread.
7. Return JSON, threads sorted by `last_activity` descending.

## Return shape

```json
{
  "tools": [
    {
      "tool": "archivador",
      "threads": [
        { "thread_id": "proyecto-alfa", "messages": 7, "last_activity": "2026-08-24T19:12:03Z",
          "opening": "El presupuesto de Alfa es 5000 dolares." },
        { "thread_id": "proyecto-beta", "messages": 3, "last_activity": "2026-08-24T18:40:11Z",
          "opening": "El presupuesto de Beta es 8000 dolares." }
      ]
    }
  ]
}
```

- `opening` is the first `user` message of the thread, truncated to ~120 chars — a cheap,
  deterministic "why this thread started" hint. No LLM, no new column, no migration.
- An LLM-generated per-thread `summary` is **deferred to a future v1.2** (needs storage +
  a generation/refresh path); `opening` covers the navigation need for v1.
- Empty `tools`/`threads` lists when nothing exists (not an error).

## Keying

Reuse `ConversationKey::keying()` on the executor's `conversation_key`
(`dag_tool_executor.rs:129-130`, already wired via `with_conversation_history` at
`llm.rs`): prefers `agent_session_id`, falls back to `session_id`. In ADP the
`agent_session_id` is always present, so threads are found reliably; under a CLI run
without `--agent-session-id` the fallback keys on the ephemeral per-run `session_id`
(same limitation as the memory feature itself — documented, not fixed here).

## Error handling / edge cases

- No `conversation_repository` or `conversation_key` wired → "not wired" error result,
  mirroring the `recall_history` branch.
- `tool` arg names an unknown or non-dynamic tool → **correctable** tool error listing the
  available dynamic tool names (never a crash).
- No threads yet → empty result (success).
- Security: the `LIKE` prefix is built only from validated `tool_configurations` keys
  (closed set); keying value + prefix are bound params. First `DISTINCT`/`LIKE`/subquery
  in this repo layer — keep it parameterized (audit-sensitive, see `memory.rs:26-34`).

## Testing

- **Unit:** thread-id extraction (bare llm_call vs subgraph-with-child; multiple children
  under one thread merge to one); aggregation (counts, max activity, opening from earliest);
  `tool`-filter validation error; exposure gating (tool present iff a dynamic tool exists).
- **Repo:** in-memory backend test for `list_node_activity` (prefix filter, counts,
  opening). Postgres/SQLite behind the existing `#[ignore]`/env-gated pattern.
- **E2E:** reuse the dynamic 3-turn graph (`subgraph_thread_memory/`), then a 4th turn
  asking the model to list threads → assert the result contains `proyecto-alfa` and
  `proyecto-beta` with their counts and openings.

## Non-goals (v1)

- LLM-generated per-thread summaries (deferred v1.2).
- Deleting/renaming threads from the tool.
- Any push/catalog injection into the system message.
- `orchestrator` support (separate blocker: orchestrator is not tool-ready; it reads its
  config from `config`, which is empty on the tool path — unrelated to this feature).

## Delivery

Purely additive (new synthetic tool + new repo method; no change to existing signatures or
wire format) → ADP and bindings unaffected. Ships with unit tests, the in-memory repo
test, the E2E graph, and docs (dev guide §19 + `node_as_tools_reference.json` note). Sized
before `review start`; sliced with `chained-pr` if it exceeds the 400-line tier.
