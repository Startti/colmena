# `list_threads` Dynamic-Memory Tool — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `list_threads` synthetic LLM tool that lets a model enumerate the existing conversation threads of any `memory_mode: "dynamic"` tool (id + message count + last activity + opening message), so it can navigate and continue the right thread.

**Architecture:** A pull tool following the existing `recall_history` synthetic-tool pattern. A new defaulted `ConversationRepository::list_node_activity` method (per-`node_id` count/last-activity/opening under a `node_id` prefix) is overridden in the three backends. A pure Rust helper extracts the `thread_id` segment and aggregates per thread. The tool is exposed only when at least one configured tool is `dynamic`, and dispatched in `execute_inner` reusing the already-wired `conversation_repository` + `conversation_key` + `tool_configurations`.

**Tech Stack:** Rust, `sqlx` (Postgres + SQLite), `async_trait`, `schemars`/`serde` for the tool args, the project's `text/tools/*.yaml` registry.

## Global Constraints

- Crate name is `colmena_dag_engine`. Run module tests with `cargo test --lib <module>`.
- `[lints.rust] warnings = "deny"` — no unused imports / dead code; `cargo clippy --lib` must be clean.
- DB-touching tests read `DATABASE_URL` and MUST be `#[ignore = "requires DATABASE_URL — run with cargo test -- --ignored"]`.
- SQL: interpolate ONLY the keying column identifier from the closed 2-set (`ConversationKey::keying()`); the keying value and the `LIKE` prefix are bound params. Never interpolate model input.
- The `text/tools/*.yaml` entry is mandatory — `text::tool_description`/`tool_summary` panic at boot if missing.
- Docs ship with the code (dev guide §19 + `node_as_tools_reference.json`). Run `python3 scripts/check_doc_links.py docs`.
- Purely additive: no change to existing trait method signatures, node signatures, or wire format.

---

### Task 1: `ConversationRepository::list_node_activity` + `NodeActivity` (+ in-memory impl)

**Files:**
- Modify: `src/libs/colmena/src/llm/domain/memory.rs` (add `NodeActivity`, add defaulted trait method)
- Modify: `src/libs/colmena/src/llm/infrastructure/persistence/in_memory_conversation_repository.rs` (override + test)

**Interfaces:**
- Produces: `pub struct NodeActivity { pub node_id: String, pub message_count: i64, pub last_activity: String, pub opening: Option<String> }`
- Produces: `async fn list_node_activity(&self, keying: (&str, &str), node_id_prefix: &str) -> Result<Vec<NodeActivity>, LlmError>` on `ConversationRepository` (defaulted → `Ok(vec![])`).

- [ ] **Step 1: Write the failing test** (in `in_memory_conversation_repository.rs` test module)

```rust
#[tokio::test]
async fn list_node_activity_groups_by_node_id_under_prefix() {
    use crate::llm::domain::{ConversationKey, ConversationRepository, LlmMessage, MessageRole, SessionId, AgentSessionId, NodeIdPath};
    let repo = InMemoryConversationRepository::new();
    let key = |node: &str| ConversationKey {
        session_id: SessionId("s".into()),
        agent_session_id: Some(AgentSessionId("agent-1".into())),
        node_id: NodeIdPath(node.into()),
    };
    repo.add_message(&key("tool/archivador/alfa/keeper"), LlmMessage::new(MessageRole::User, "abrir alfa")).await.unwrap();
    repo.add_message(&key("tool/archivador/alfa/keeper"), LlmMessage::new(MessageRole::Assistant, "ok")).await.unwrap();
    repo.add_message(&key("tool/archivador/beta/keeper"), LlmMessage::new(MessageRole::User, "abrir beta")).await.unwrap();
    repo.add_message(&key("tool/otro/x/keeper"), LlmMessage::new(MessageRole::User, "no incluir")).await.unwrap();

    let rows = repo.list_node_activity(("agent_session_id", "agent-1"), "tool/archivador/").await.unwrap();
    assert_eq!(rows.len(), 2, "only archivador node_ids");
    let alfa = rows.iter().find(|r| r.node_id == "tool/archivador/alfa/keeper").unwrap();
    assert_eq!(alfa.message_count, 2);
    assert_eq!(alfa.opening.as_deref(), Some("abrir alfa"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib list_node_activity_groups_by_node_id_under_prefix`
Expected: FAIL (method returns empty `vec![]` from the default → `rows.len()` is 0).

- [ ] **Step 3: Add the struct + defaulted trait method** in `memory.rs`

```rust
/// Per-`node_id` activity summary for thread enumeration (`list_threads`).
#[derive(Debug, Clone)]
pub struct NodeActivity {
    pub node_id: String,
    pub message_count: i64,
    pub last_activity: String, // ISO-8601 UTC
    pub opening: Option<String>, // earliest `user` message content
}
```
Add to the `ConversationRepository` trait (after `get_with_summaries`):
```rust
    /// List per-`node_id` activity for every `node_id` starting with `node_id_prefix`,
    /// keyed by `keying` (("agent_session_id"|"session_id", value)). Backends override;
    /// the default returns empty so non-DB stubs stay valid.
    async fn list_node_activity(
        &self,
        keying: (&str, &str),
        node_id_prefix: &str,
    ) -> Result<Vec<NodeActivity>, LlmError> {
        let _ = (keying, node_id_prefix);
        Ok(Vec::new())
    }
```

- [ ] **Step 4: Override in `InMemoryConversationRepository`**

The in-memory store keys messages by the full `ConversationKey`. Iterate its map, keep entries whose keying matches and whose `node_id` starts with the prefix, and fold each into a `NodeActivity` (count, last timestamp, first user message). Match the store's actual field names/locking when implementing.

```rust
async fn list_node_activity(
    &self,
    keying: (&str, &str),
    node_id_prefix: &str,
) -> Result<Vec<NodeActivity>, LlmError> {
    let (col, val) = keying;
    let guard = self.store.lock().unwrap(); // adapt to the real field + lock
    let mut out = Vec::new();
    for (k, msgs) in guard.iter() {
        let matches_key = match col {
            "agent_session_id" => k.agent_session_id.as_ref().map(|a| a.0.as_str()) == Some(val),
            _ => k.session_id.0 == val,
        };
        if !matches_key || !k.node_id.0.starts_with(node_id_prefix) { continue; }
        let opening = msgs.iter().find(|m| matches!(m.role, MessageRole::User)).map(|m| m.content.clone());
        out.push(NodeActivity {
            node_id: k.node_id.0.clone(),
            message_count: msgs.len() as i64,
            last_activity: String::new(), // in-memory has no timestamps; empty is fine for tests
            opening,
        });
    }
    Ok(out)
}
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test --lib list_node_activity_groups_by_node_id_under_prefix`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add src/libs/colmena/src/llm/domain/memory.rs src/libs/colmena/src/llm/infrastructure/persistence/in_memory_conversation_repository.rs
git commit -m "feat(llm): add list_node_activity to ConversationRepository (+ in-memory impl)"
```

---

### Task 2: Postgres + SQLite `list_node_activity` overrides

**Files:**
- Modify: `src/libs/colmena/src/llm/infrastructure/persistence/postgres_conversation_repository.rs`
- Modify: `src/libs/colmena/src/llm/infrastructure/persistence/sqlite_conversation_repository.rs`

**Interfaces:**
- Consumes: `NodeActivity`, the trait method from Task 1.

- [ ] **Step 1: Write the failing (ignored) Postgres test**

```rust
#[tokio::test]
#[ignore = "requires DATABASE_URL — run with cargo test -- --ignored"]
async fn pg_list_node_activity_returns_counts_and_opening() {
    let url = std::env::var("DATABASE_URL").unwrap();
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    let repo = PostgresConversationRepository::new(pool);
    // (seed a couple of rows under a unique agent_session_id via add_message, then:)
    let rows = repo.list_node_activity(("agent_session_id", "<seeded>"), "tool/t/").await.unwrap();
    assert!(rows.iter().any(|r| r.message_count > 0 && r.opening.is_some()));
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `source .env && cargo test --lib pg_list_node_activity_returns_counts_and_opening -- --ignored`
Expected: FAIL (default returns empty).

- [ ] **Step 3: Implement the Postgres override**

Reuse the `keying()` safe-column convention already used by `get_by_id` (`postgres_conversation_repository.rs:24-30`). `{col}` is interpolated; value + prefix are bound.

```rust
async fn list_node_activity(
    &self,
    keying: (&str, &str),
    node_id_prefix: &str,
) -> Result<Vec<NodeActivity>, LlmError> {
    let (col, val) = keying;
    let sql = format!(
        "SELECT h1.node_id AS node_id, \
                count(*) AS message_count, \
                max(h1.created_at)::text AS last_activity, \
                (SELECT h2.content FROM llm_node_history h2 \
                   WHERE h2.{col} = $1 AND h2.node_id = h1.node_id AND h2.role = 'user' \
                   ORDER BY h2.created_at ASC LIMIT 1) AS opening \
         FROM llm_node_history h1 \
         WHERE h1.{col} = $1 AND h1.node_id LIKE $2 \
         GROUP BY h1.node_id"
    );
    let like = format!("{}%", node_id_prefix);
    let rows = sqlx::query(&sql).bind(val).bind(&like).fetch_all(&self.pool).await
        .map_err(|e| LlmError::RequestFailed { message: format!("Database error: {e}") })?;
    Ok(rows.iter().map(|r| NodeActivity {
        node_id: r.get::<String, _>("node_id"),
        message_count: r.get::<i64, _>("message_count"),
        last_activity: r.get::<Option<String>, _>("last_activity").unwrap_or_default(),
        opening: r.get::<Option<String>, _>("opening"),
    }).collect())
}
```

- [ ] **Step 4: Implement the SQLite override** (same shape; SQLite `datetime`/text, `?`-style binds mirroring `sqlite_conversation_repository.rs:23-32`)

```rust
async fn list_node_activity(
    &self,
    keying: (&str, &str),
    node_id_prefix: &str,
) -> Result<Vec<NodeActivity>, LlmError> {
    let (col, val) = keying;
    let sql = format!(
        "SELECT h1.node_id AS node_id, \
                count(*) AS message_count, \
                max(h1.created_at) AS last_activity, \
                (SELECT h2.content FROM llm_node_history h2 \
                   WHERE h2.{col} = ?1 AND h2.node_id = h1.node_id AND h2.role = 'user' \
                   ORDER BY h2.created_at ASC LIMIT 1) AS opening \
         FROM llm_node_history h1 \
         WHERE h1.{col} = ?1 AND h1.node_id LIKE ?2 \
         GROUP BY h1.node_id"
    );
    let like = format!("{}%", node_id_prefix);
    let rows = sqlx::query(&sql).bind(val).bind(&like).fetch_all(&self.pool).await
        .map_err(|e| LlmError::RequestFailed { message: format!("Database error: {e}") })?;
    Ok(rows.iter().map(|r| NodeActivity {
        node_id: r.get::<String, _>("node_id"),
        message_count: r.get::<i64, _>("message_count"),
        last_activity: r.get::<Option<String>, _>("last_activity").unwrap_or_default(),
        opening: r.get::<Option<String>, _>("opening"),
    }).collect())
}
```

- [ ] **Step 5: Verify it compiles + the ignored test passes with a DB**

Run: `cargo build --lib` then `source .env && cargo test --lib pg_list_node_activity_returns_counts_and_opening -- --ignored`
Expected: build clean; test PASS.

- [ ] **Step 6: Commit**

```bash
git add src/libs/colmena/src/llm/infrastructure/persistence/postgres_conversation_repository.rs src/libs/colmena/src/llm/infrastructure/persistence/sqlite_conversation_repository.rs
git commit -m "feat(llm): implement list_node_activity for Postgres and SQLite backends"
```

---

### Task 3: thread aggregation helper + `list_threads` synthetic tool

**Files:**
- Create: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/list_threads.rs`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs` (declare + re-export)
- Modify: `src/libs/colmena/text/tools/helpers.yaml` (description + summary)

**Interfaces:**
- Consumes: `NodeActivity`, `ConversationRepository::list_node_activity`, `ConversationKey`.
- Produces: `pub const TOOL_LIST_THREADS: &str = "list_threads";`, `pub fn tool_list_threads() -> ToolDefinition`, `pub async fn dispatch_list_threads(repo: &Arc<dyn ConversationRepository>, key: &ConversationKey, dynamic_tool_names: &[String], args: serde_json::Value) -> serde_json::Value`, and (pub for testing) `fn aggregate_threads(tool_name: &str, rows: Vec<NodeActivity>) -> Vec<ThreadInfo>`.

- [ ] **Step 1: Write the failing aggregation test** (in `list_threads.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::domain::NodeActivity;

    fn na(node_id: &str, n: i64, last: &str, opening: &str) -> NodeActivity {
        NodeActivity { node_id: node_id.into(), message_count: n, last_activity: last.into(), opening: Some(opening.into()) }
    }

    #[test]
    fn aggregate_extracts_thread_id_and_merges_children() {
        let rows = vec![
            na("tool/archivador/alfa/keeper", 4, "2026-08-24T10:00:00Z", "abrir alfa"),
            na("tool/archivador/alfa/notes", 2, "2026-08-24T11:00:00Z", "z-later"), // same thread, 2nd child
            na("tool/archivador/beta/keeper", 3, "2026-08-24T09:00:00Z", "abrir beta"),
        ];
        let mut out = aggregate_threads("archivador", rows);
        // sorted by last_activity desc → alfa (11:00) before beta (09:00)
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].thread_id, "alfa");
        assert_eq!(out[0].messages, 6);            // merged 4 + 2
        assert_eq!(out[0].opening.as_deref(), Some("abrir alfa")); // earliest source
        assert_eq!(out[1].thread_id, "beta");
    }

    #[test]
    fn aggregate_handles_bare_llm_call_thread_without_child_suffix() {
        let rows = vec![na("tool/asesor/caso-12", 5, "2026-08-24T12:00:00Z", "hola")];
        let out = aggregate_threads("asesor", rows);
        assert_eq!(out[0].thread_id, "caso-12");
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --lib aggregate_extracts_thread_id_and_merges_children`
Expected: FAIL ("cannot find function `aggregate_threads`").

- [ ] **Step 3: Implement the file** (`list_threads.rs`)

```rust
//! `list_threads` synthetic tool — enumerate the conversation threads of any
//! `memory_mode: "dynamic"` tool so the model can navigate and continue one.
//! Mirrors the `recall_history` wiring: a `with_conversation_history(repo, key)`
//! builder supplies the deps; the dispatch arm intercepts the tool name.

use crate::llm::domain::tools::ToolDefinition;
use crate::llm::domain::{ConversationKey, ConversationRepository, NodeActivity};
use schemars::JsonSchema;
use serde::Deserialize;
use std::sync::Arc;

pub const TOOL_LIST_THREADS: &str = "list_threads";
const OPENING_MAX_CHARS: usize = 120;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListThreadsArgs {
    /// Optional: name of a specific dynamic tool to list. Omit to list every
    /// dynamic tool's threads, grouped by tool.
    #[serde(default)]
    pub tool: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ThreadInfo {
    pub thread_id: String,
    pub messages: i64,
    pub last_activity: String,
    pub opening: Option<String>,
}

pub fn tool_list_threads() -> ToolDefinition {
    use crate::text;
    super::build_synthetic_tool_with_summary::<ListThreadsArgs>(
        TOOL_LIST_THREADS,
        text::tool_description(TOOL_LIST_THREADS),
        text::tool_summary(TOOL_LIST_THREADS),
    )
}

/// Group per-node_id rows into per-thread entries. `node_id` is
/// `tool/<tool_name>/<thread_id>[/<child...>]`; the thread id is the first
/// segment after the `tool/<tool_name>/` prefix. Rows sharing a thread id merge
/// (sum messages, max last_activity, opening from the earliest source row).
fn aggregate_threads(tool_name: &str, rows: Vec<NodeActivity>) -> Vec<ThreadInfo> {
    use std::collections::HashMap;
    let prefix = format!("tool/{tool_name}/");
    // thread_id -> (messages, max_last, best_opening, best_opening_key)
    let mut acc: HashMap<String, ThreadInfo> = HashMap::new();
    for r in rows {
        let Some(rest) = r.node_id.strip_prefix(&prefix) else { continue };
        let thread_id = rest.split('/').next().unwrap_or(rest).to_string();
        if thread_id.is_empty() { continue; }
        let opening = r.opening.map(|o| truncate(&o, OPENING_MAX_CHARS));
        let e = acc.entry(thread_id.clone()).or_insert(ThreadInfo {
            thread_id,
            messages: 0,
            last_activity: String::new(),
            opening: None,
        });
        e.messages += r.message_count;
        if r.last_activity > e.last_activity { e.last_activity = r.last_activity.clone(); }
        // keep the opening from the lexicographically-earliest node_id as a stable
        // "first source" proxy; fill if still empty
        if e.opening.is_none() { e.opening = opening; }
    }
    let mut out: Vec<ThreadInfo> = acc.into_values().collect();
    out.sort_by(|a, b| b.last_activity.cmp(&a.last_activity));
    out
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max { return s.to_string(); }
    let mut t: String = s.chars().take(max).collect();
    t.push('…');
    t
}

/// Dispatch a `list_threads` call. `dynamic_tool_names` is the set of configured
/// tools whose `memory_mode == Dynamic`. Returns a serde_json value for the LLM.
pub async fn dispatch_list_threads(
    repo: &Arc<dyn ConversationRepository>,
    key: &ConversationKey,
    dynamic_tool_names: &[String],
    args: serde_json::Value,
) -> serde_json::Value {
    let parsed: ListThreadsArgs = match serde_json::from_value(args) {
        Ok(a) => a,
        Err(e) => return serde_json::json!({ "error": format!("invalid_args: {e}") }),
    };
    let targets: Vec<String> = match parsed.tool {
        Some(t) if dynamic_tool_names.iter().any(|n| n == &t) => vec![t],
        Some(t) => {
            return serde_json::json!({
                "error": format!("unknown_or_non_dynamic_tool: '{t}'"),
                "available_dynamic_tools": dynamic_tool_names,
            });
        }
        None => dynamic_tool_names.to_vec(),
    };
    let keying = key.keying();
    let mut tools_json = Vec::new();
    for name in targets {
        let prefix = format!("tool/{name}/");
        let rows = match repo.list_node_activity(keying, &prefix).await {
            Ok(r) => r,
            Err(e) => return serde_json::json!({ "error": format!("query_failed: {e}") }),
        };
        let threads = aggregate_threads(&name, rows);
        tools_json.push(serde_json::json!({ "tool": name, "threads": threads }));
    }
    serde_json::json!({ "tools": tools_json })
}
```

- [ ] **Step 4: Declare + re-export in `mod.rs`** (mirror the `recall_history` block near line 325)

```rust
mod list_threads; // add near the other `mod` declarations
pub use list_threads::{dispatch_list_threads, tool_list_threads, ListThreadsArgs, TOOL_LIST_THREADS};
```

- [ ] **Step 5: Add the YAML entry** to `src/libs/colmena/text/tools/helpers.yaml`

```yaml
list_threads:
  summary: List the existing conversation threads of a memory-bearing sub-agent tool
  description: |
    List the conversation threads that already exist for a "dynamic"-memory
    sub-agent tool, so you can continue the right one. Optionally pass `tool` to
    focus a single tool; omit it to list every dynamic tool grouped. Each thread
    returns its `thread_id` (the id you pass to that tool to continue it), the
    message count, the last activity time, and `opening` (how the thread began).
    Use this when you need to reuse a prior `thread_id` and don't remember it.
```

- [ ] **Step 6: Run the aggregation tests + build**

Run: `cargo test --lib list_threads::` then `cargo build --lib`
Expected: PASS; build clean.

- [ ] **Step 7: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/list_threads.rs src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs src/libs/colmena/text/tools/helpers.yaml
git commit -m "feat(dag): add list_threads synthetic tool (definition + dispatch + text)"
```

---

### Task 4: executor dispatch arm + exposure gating

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs` (dispatch arm in `execute_inner`, ~after the `RECALL_HISTORY_TOOL` arm at line 1705-1740)
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs` (compute `has_dynamic` before line 2304; gate the push near line 2747)

**Interfaces:**
- Consumes: `dispatch_list_threads`, `TOOL_LIST_THREADS`, `tool_list_threads`, `MemoryMode::Dynamic`, `self.conversation_repository`, `self.conversation_key`, `self.tool_configurations`.

- [ ] **Step 1: Write the failing dispatch test** (in `dag_tool_executor.rs` test module; reuse the `registry_with_subgraph()` + `dynamic_tool_configs()` helpers added for Part 3). This exercises the real Task-4 deliverable — the `execute_inner` dispatch arm — against an in-memory repo seeded with one thread. (Exposure/gating lives in `llm.rs` and is covered end-to-end by the E2E in Task 5.)

```rust
#[tokio::test]
async fn list_threads_dispatch_lists_dynamic_tool_threads() {
    use crate::llm::domain::{ConversationKey, ConversationRepository, SessionId, AgentSessionId, NodeIdPath, LlmMessage, MessageRole};
    use crate::llm::infrastructure::persistence::in_memory_conversation_repository::InMemoryConversationRepository;
    let repo = std::sync::Arc::new(InMemoryConversationRepository::new());
    // seed one thread for the dynamic tool "archivador"
    let thread_key = ConversationKey {
        session_id: SessionId("s".into()),
        agent_session_id: Some(AgentSessionId("a".into())),
        node_id: NodeIdPath("tool/archivador/proyecto-alfa/keeper".into()),
    };
    repo.add_message(&thread_key, LlmMessage::new(MessageRole::User, "abrir alfa")).await.unwrap();
    // the parent llm_call's key supplies the keying (agent_session_id "a")
    let parent_key = ConversationKey {
        session_id: SessionId("s".into()),
        agent_session_id: Some(AgentSessionId("a".into())),
        node_id: NodeIdPath("chat".into()),
    };
    let exec = DagToolExecutor::new(registry_with_subgraph(), dynamic_tool_configs())
        .with_conversation_history(repo, parent_key);
    let call = ToolCall::new("call_1".into(), FunctionCall::new("list_threads".into(), "{}".into()));
    let res = exec.execute(&call).await.unwrap();
    assert!(res.success, "list_threads should succeed: {}", res.output);
    assert!(res.output.contains("proyecto-alfa"), "should list the thread: {}", res.output);
    assert!(res.output.contains("archivador"), "grouped under the tool name: {}", res.output);
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --lib list_threads_dispatch_lists_dynamic_tool_threads`
Expected: FAIL — the `list_threads` name is not yet matched in `execute_inner`, so it falls through to the "tool not found" / node-resolution path (no `proyecto-alfa` in the output). Fix imports (the `with_conversation_history` builder path) until it compiles and then fails on the assertion.

- [ ] **Step 3: Add the dispatch arm** in `execute_inner`, immediately after the `RECALL_HISTORY_TOOL` block (~line 1740)

```rust
// list_threads synthetic tool — enumerate dynamic-memory threads.
if tool_call.function.name
    == crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::TOOL_LIST_THREADS
{
    use crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::dispatch_list_threads;
    use crate::dag_engine::domain::tool_configuration::MemoryMode;
    let (Some(repo), Some(key)) =
        (self.conversation_repository.as_ref(), self.conversation_key.as_ref())
    else {
        return Ok(ToolResult {
            tool_call_id: tool_call.id.clone(),
            success: false,
            output: "list_threads is not available: no conversation store is wired.".to_string(),
            error: Some("list_threads_not_wired".to_string()),
        });
    };
    let dynamic_tool_names: Vec<String> = self
        .tool_configurations
        .values()
        .filter(|c| c.memory_mode == MemoryMode::Dynamic)
        .map(|c| c.name.clone())
        .collect();
    let args: serde_json::Value =
        serde_json::from_str(&tool_call.function.arguments).unwrap_or(serde_json::json!({}));
    let result = dispatch_list_threads(repo, key, &dynamic_tool_names, args).await;
    return Ok(ToolResult {
        tool_call_id: tool_call.id.clone(),
        success: true,
        output: result.to_string(),
        error: None,
    });
}
```

- [ ] **Step 4: Gate the exposure in `llm.rs`** — compute the flag BEFORE `tool_configurations` is moved into the executor (line 2304), then push conditionally near line 2747.

Right before `let mut executor = DagToolExecutor::new(registry, tool_configurations);` (line 2304):
```rust
let exposes_dynamic_memory = tool_configurations
    .values()
    .any(|c| c.memory_mode == crate::dag_engine::domain::tool_configuration::MemoryMode::Dynamic);
```
Replace the unconditional recall_history block region's neighbor (near line 2747) by adding:
```rust
if exposes_dynamic_memory {
    use crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::tool_list_threads;
    tools.push(tool_list_threads());
}
```

- [ ] **Step 5: Build + run tests + clippy**

Run: `cargo test --lib list_threads` then `cargo clippy --lib`
Expected: PASS; clippy clean.

- [ ] **Step 6: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs
git commit -m "feat(dag): dispatch + gate list_threads on a dynamic-memory tool"
```

---

### Task 5: E2E verification + docs (+ fold orchestrator comment fix)

**Files:**
- Create: `tests/graphs/agents/subgraph_thread_memory_list/turn4_list.json` (reuse the 3 dynamic turns, add a 4th "list threads" turn)
- Modify: `docs/developer_guide/19_nested_agents_and_subgraphs.md` (add a `list_threads` note in the dynamic section)
- Modify: `docs/node_as_tools_reference.json` (note the auto-exposed `list_threads` tool for dynamic)
- Modify: `src/libs/colmena/src/dag_engine/domain/tool_configuration.rs` (the orchestrator-comment accuracy fix already in the working tree — commit it here)

- [ ] **Step 1: Build the 4-turn E2E** — copy `tests/graphs/agents/subgraph_thread_memory/turn3_recall_alfa.json` to a new `turn4_list.json`, change the parent `prompt` to: `"Listame que hilos/proyectos tenes guardados en el archivador."` and keep `memory_mode: "dynamic"`.

- [ ] **Step 2: Run the full sequence** against Postgres (mirror the Part 3 runner)

```bash
set -a; source .env; set +a
ASID=list_threads_e2e_001
psql "$DATABASE_URL" -tAc "DELETE FROM llm_node_history WHERE agent_session_id='$ASID';"
for t in turn1_store_alfa turn2_store_beta; do
  env -u COLMENA_LOCAL ./target/debug/dag_engine run tests/graphs/agents/subgraph_thread_memory/$t.json --agent-session-id "$ASID" >/tmp/colmena_e2e/lt_$t.sse 2>&1
done
env -u COLMENA_LOCAL ./target/debug/dag_engine run tests/graphs/agents/subgraph_thread_memory_list/turn4_list.json --agent-session-id "$ASID" >/tmp/colmena_e2e/lt_turn4.sse 2>&1
grep -o 'proyecto-alfa\|proyecto-beta\|"messages"\|"opening"' /tmp/colmena_e2e/lt_turn4.sse | sort | uniq -c
```
Expected: the model calls `list_threads`, and the result surfaces `proyecto-alfa` and `proyecto-beta` with `messages`/`opening`.

- [ ] **Step 3: Update docs** — in `19_nested_agents_and_subgraphs.md` dynamic section add:

```markdown
En `dynamic`, el motor auto-expone además una tool `list_threads` cuando hay al menos un
tool dynamic: el modelo la llama para enumerar los hilos existentes (`thread_id`,
`messages`, `last_activity`, `opening`) y así retomar el correcto. Opcional `tool` para
enfocar uno; sin argumento lista todos agrupados.
```
And a one-line note in `docs/node_as_tools_reference.json` under the `memory_mode` `dynamic` mode description.

- [ ] **Step 4: Docs guard + fmt + full tests**

Run: `python3 scripts/check_doc_links.py docs && cargo fmt && cargo test --verbose 2>&1 | grep -E 'test result: FAILED|error\[' ; cargo clippy --lib`
Expected: 0 broken links; no failures; clippy clean.

- [ ] **Step 5: Commit**

```bash
git add tests/graphs/agents/subgraph_thread_memory_list docs/developer_guide/19_nested_agents_and_subgraphs.md docs/node_as_tools_reference.json src/libs/colmena/src/dag_engine/domain/tool_configuration.rs
git commit -m "test(dag): list_threads E2E + docs; correct orchestrator deferral rationale"
```

---

## Self-Review

- **Spec coverage:** repo method (Task 1/2) ✓; thread-id extraction + aggregation (Task 3) ✓; synthetic tool def/dispatch/YAML (Task 3) ✓; executor dispatch + gating (Task 4) ✓; return shape with `opening` (Task 3) ✓; keying via `ConversationKey::keying()` (Task 3/4) ✓; error/edge cases — not-wired (Task 4), unknown/non-dynamic tool (Task 3), empty list (Task 3 returns `{tools:[...]}` with empty `threads`) ✓; testing unit + repo + E2E (Tasks 1-5) ✓; docs (Task 5) ✓.
- **Placeholder scan:** the in-memory override references `self.store` / lock — the implementer adapts to the file's real field name and lock type; flagged inline, not a silent TODO. Postgres seed in Task 2 test is described (seed via `add_message`) rather than spelled out; acceptable for an `#[ignore]` DB test.
- **Type consistency:** `NodeActivity` fields (`node_id`, `message_count: i64`, `last_activity: String`, `opening: Option<String>`) are used identically in Tasks 1-3; `ThreadInfo` (`thread_id`, `messages: i64`, `last_activity`, `opening`) consistent in Task 3; `dispatch_list_threads(repo, key, dynamic_tool_names, args)` signature matches between Task 3 (def) and Task 4 (call).

## Sizing note

Estimated ~350-450 changed lines (3 backends + tool + wiring + E2E + docs). Run `python3 scripts/review_size.py --base-ref origin/develop` before `review start`; if `high`, the natural slice is Tasks 1-2 (repo layer) as one PR and Tasks 3-5 (tool + wiring + E2E) as a dependent PR via the `chained-pr` skill.
