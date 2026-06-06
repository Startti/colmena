# Suspend resume_answer routing fix — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stop the DAG engine from injecting `__colmena_resume_answer` into nodes that weren't suspended in the prior run, and add a defensive fallthrough in `llm_call`'s resume branch so a fresh node downstream of a `suspend` never aborts with "no pending tool call found in conversation history".

**Architecture:** Two surgical changes — (1) gate the resume-answer injection in `run_use_case.rs` by a `HashSet<String>` of node ids that had `__colmena_status: SUSPENDED` in the restored snapshot, and (2) replace the `.ok_or("...no pending tool call...")?` in `llm.rs` with a `match` that falls through to fresh-run on `None` and emits a `tracing::warn!`. No public API changes, no protocol changes.

**Tech Stack:** Rust (cargo, tokio), async-stream, sqlx (PostgreSQL), `ScriptedAdapter` for deterministic LLM tests, JSON DAG graphs under `tests/graphs/`.

**Spec:** [`docs/superpowers/specs/2026-06-05-suspend-resume-answer-routing-fix-design.md`](../specs/2026-06-05-suspend-resume-answer-routing-fix-design.md)

---

## File map

| Action | Path | Responsibility |
|---|---|---|
| Create | `tests/graphs/basic/suspend_then_llm_resume.json` | Repro graph: `input → suspend → llm_call → log`. Mirrors ADP's failing case but with `google/gemini-2.5-flash` per default stack rule. |
| Create | `tests/graphs/basic/suspend_cascade.json` | Cascade graph: `input → suspend1 → suspend2 → log` with distinct ids. |
| Create | `src/libs/colmena/tests/suspend_resume_routing.rs` | End-to-end integration test using `ScriptedAdapter`. Two tests: ADP repro + cascade. |
| Modify | `src/libs/colmena/src/dag_engine/application/run_use_case.rs` | Add helper `compute_resuming_node_ids` (associated fn) + gate the injection at line 377. Add inline `#[cfg(test)]` unit tests for the helper. |
| Modify | `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs` | Replace `.ok_or(...)?` at line 1802 with `match` + fallthrough + `tracing::warn!`. |
| Modify | `docs/developer_guide/38_suspend_node.md` | Cross-link to the spec under "Referencias cruzadas". |
| Modify | `docs/CHANGELOG_2026-05.md` (or create `CHANGELOG_2026-06.md` if absent) | One-line entry describing the fix. |

---

## Task 1: Failing integration test — ADP repro (`suspend → llm_call`)

**Files:**
- Create: `tests/graphs/basic/suspend_then_llm_resume.json`
- Create: `src/libs/colmena/tests/suspend_resume_routing.rs`

- [ ] **Step 1.1: Write the failing graph fixture**

Create `tests/graphs/basic/suspend_then_llm_resume.json`:

```json
{
  "nodes": {
    "start": {
      "type": "mock_input",
      "config": { "input_data": "kickoff" }
    },
    "ask_name": {
      "type": "suspend",
      "config": {
        "id": "ask_name",
        "question": "¿Cuál es tu nombre?",
        "question_type": "open"
      }
    },
    "poet": {
      "type": "llm",
      "config": {
        "model": "google/gemini-2.5-flash",
        "system_message": "Devuelve un saludo corto al usuario. Una sola línea.",
        "stream": false
      }
    },
    "finish": { "type": "log", "config": { "prefix": "result:" } }
  },
  "edges": [
    { "from": "start",    "to": "ask_name" },
    { "from": "ask_name", "to": "poet" },
    { "from": "poet",     "to": "finish" }
  ]
}
```

(`mock_input` is used instead of `input` so the test doesn't need stdin. The model field uses the default stack from user memory.)

- [ ] **Step 1.2: Write the failing integration test scaffold**

Create `src/libs/colmena/tests/suspend_resume_routing.rs`:

```rust
//! Integration tests for spec
//! `docs/superpowers/specs/2026-06-05-suspend-resume-answer-routing-fix-design.md`.
//!
//! Verifies the engine no longer cascades `__colmena_resume_answer` into
//! nodes that weren't suspended.
//!
//! Uses `ScriptedAdapter` for deterministic LLM responses (no real API call).
//!
//! Run with:
//!   source .env && cargo test --test suspend_resume_routing -- --ignored --nocapture

use colmena::dag_engine::domain::events::DagExecutionEvent;
use colmena::dag_engine::domain::graph::Graph;
use colmena::dag_engine::engine::{ColmenaEngine, EngineConfig};
use colmena::llm::infrastructure::{OverrideGuard, ScriptedAdapter, ScriptedResponse};
use futures::StreamExt;
use std::sync::Arc;

fn init_logs() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_test_writer()
        .try_init();
}

async fn engine() -> ColmenaEngine {
    dotenvy::dotenv().ok();
    let cfg = EngineConfig::from_env().await.unwrap();
    ColmenaEngine::new(cfg).await.unwrap()
}

async fn cleanup(chat: &str) {
    dotenvy::dotenv().ok();
    let url = std::env::var("DATABASE_URL").unwrap();
    let pool = sqlx::PgPool::connect(&url).await.unwrap();

    let has_agent_session: bool = sqlx::query_scalar(
        r#"SELECT EXISTS (
             SELECT 1 FROM information_schema.tables
             WHERE table_schema = 'public' AND table_name = 'agent_session'
           )"#,
    )
    .fetch_one(&pool)
    .await
    .unwrap_or(false);

    if has_agent_session {
        sqlx::query("DELETE FROM agent_session WHERE id = $1")
            .bind(chat)
            .execute(&pool)
            .await
            .ok();
    } else {
        sqlx::query("DELETE FROM dag_runs WHERE agent_session_id = $1")
            .bind(chat)
            .execute(&pool)
            .await
            .ok();
        sqlx::query("DELETE FROM llm_node_history WHERE agent_session_id = $1")
            .bind(chat)
            .execute(&pool)
            .await
            .ok();
    }
}

fn load_graph(path: &str) -> Graph {
    let raw = std::fs::read_to_string(path).expect("read graph fixture");
    serde_json::from_str(&raw).expect("parse graph fixture")
}

async fn run_until_done(
    eng: &ColmenaEngine,
    graph: Graph,
    agent: &str,
    answer: Option<String>,
) -> Vec<DagExecutionEvent> {
    let mut stream = eng
        .execute(graph, None, answer, Some(agent.to_string()), false)
        .await;
    let mut events = Vec::new();
    while let Some(ev) = stream.next().await {
        events.push(ev.expect("stream item"));
    }
    events
}

#[tokio::test]
#[ignore = "requires DATABASE_URL — run with `cargo test -- --ignored`"]
async fn suspend_then_llm_resume_runs_llm_fresh() {
    init_logs();
    let chat = "test_suspend_then_llm_resume";
    cleanup(chat).await;

    // ScriptedAdapter: any prompt → fixed completion.
    let scripted = Arc::new(ScriptedAdapter::with_responses(vec![
        ScriptedResponse::text("¡Hola Julián!"),
    ]));
    let _guard = OverrideGuard::install(scripted.clone());

    let eng = engine().await;
    let graph_path = "../../tests/graphs/basic/suspend_then_llm_resume.json";
    let graph = load_graph(graph_path);

    // Run 1: should suspend.
    let events1 = run_until_done(&eng, graph.clone(), chat, None).await;
    let finished = events1
        .iter()
        .find_map(|e| match e {
            DagExecutionEvent::GraphFinish { output } => Some(output.clone()),
            _ => None,
        })
        .expect("run 1 must emit GraphFinish");
    assert_eq!(
        finished
            .get("__colmena_status")
            .and_then(|v| v.as_str()),
        Some("SUSPENDED"),
        "run 1 must finish in SUSPENDED state, got: {finished:#}"
    );

    // Run 2: resume with Q/A — llm_call must run fresh, NOT error.
    let answer = "Q[ask_name]: ¿Cuál es tu nombre?\nA[ask_name]: Julián".to_string();
    let events2 = run_until_done(&eng, graph, chat, Some(answer)).await;

    // No error events.
    for e in &events2 {
        if let DagExecutionEvent::NodeError { node_id, error } = e {
            panic!("unexpected node error on resume: node={node_id} error={error}");
        }
    }

    // llm_call must have emitted a non-empty NodeFinish.
    let llm_out = events2
        .iter()
        .find_map(|e| match e {
            DagExecutionEvent::NodeFinish { node_id, output } if node_id == "poet" => {
                Some(output.clone())
            }
            _ => None,
        })
        .expect("poet must finish successfully");
    assert!(
        llm_out.to_string().to_lowercase().contains("julián")
            || llm_out.to_string().contains("Hola"),
        "expected scripted completion in poet output, got: {llm_out:#}"
    );

    cleanup(chat).await;
}
```

- [ ] **Step 1.3: Run the test and verify it FAILS with the ADP error**

Run:
```bash
source .env && cargo test --test suspend_resume_routing suspend_then_llm_resume_runs_llm_fresh -- --ignored --nocapture
```

Expected: FAIL. The panic should reference `NodeError { node_id: "poet", error: "...no pending tool call found in conversation history" }` (or a wrapper of that string), confirming we reproduced the ADP bug.

If it fails for a different reason (compilation error, fixture-path issue, missing `ScriptedResponse::text` API), fix only the test-harness issue and re-run; do NOT touch engine/llm.rs yet — Phase 1 of debugging said we already located the root cause.

- [ ] **Step 1.4: Commit the failing test**

```bash
git add tests/graphs/basic/suspend_then_llm_resume.json \
        src/libs/colmena/tests/suspend_resume_routing.rs
git commit -m "$(cat <<'EOF'
test(suspend): failing repro of suspend→llm_call resume bug

Reproduces ADP bug 2026-06-04: when an llm_call runs fresh downstream
of a suspend during a resume run, the engine injects
__colmena_resume_answer and the llm_call aborts in its resume branch
with "no pending tool call found in conversation history".

Test is marked #[ignore] per repo convention (requires DATABASE_URL).
Engine fix lands in a later commit; this commit captures the failure.
EOF
)"
```

---

## Task 2: Engine helper — `compute_resuming_node_ids`

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/application/run_use_case.rs` (add helper + inline unit tests)

- [ ] **Step 2.1: Write failing unit tests for the helper**

Find the `impl DagRunUseCase` block in `run_use_case.rs`. At the very bottom of the file (or in the existing `mod tests` if there is one — grep with `grep -n "mod tests" src/libs/colmena/src/dag_engine/application/run_use_case.rs`), append:

```rust
#[cfg(test)]
mod resuming_node_ids_tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashMap;

    #[test]
    fn empty_when_resume_answer_is_none() {
        let mut all = HashMap::new();
        all.insert(
            "n1".to_string(),
            json!({ "__colmena_status": "SUSPENDED" }),
        );
        let set = DagRunUseCase::compute_resuming_node_ids(&all, &None);
        assert!(set.is_empty(), "fresh run must yield empty set");
    }

    #[test]
    fn includes_only_suspended_nodes() {
        let mut all = HashMap::new();
        all.insert(
            "suspended_one".to_string(),
            json!({ "__colmena_status": "SUSPENDED", "question": "x" }),
        );
        all.insert(
            "ran_fine".to_string(),
            json!({ "output": 42 }),
        );
        all.insert(
            "another_suspend".to_string(),
            json!({ "__colmena_status": "SUSPENDED" }),
        );
        let set = DagRunUseCase::compute_resuming_node_ids(
            &all,
            &Some("anything".to_string()),
        );
        assert_eq!(set.len(), 2);
        assert!(set.contains("suspended_one"));
        assert!(set.contains("another_suspend"));
        assert!(!set.contains("ran_fine"));
    }

    #[test]
    fn finds_suspended_in_nested_output() {
        // Mirrors the orchestrator/subgraph wrap case where the SUSPENDED
        // marker is nested inside the parent's output structure.
        let mut all = HashMap::new();
        all.insert(
            "wrapper".to_string(),
            json!({
                "result": { "__colmena_status": "SUSPENDED" },
                "meta": { "child": "inner_node" }
            }),
        );
        let set = DagRunUseCase::compute_resuming_node_ids(
            &all,
            &Some("ans".to_string()),
        );
        assert!(set.contains("wrapper"));
    }

    #[test]
    fn empty_all_outputs_yields_empty_set() {
        let all: HashMap<String, serde_json::Value> = HashMap::new();
        let set = DagRunUseCase::compute_resuming_node_ids(
            &all,
            &Some("ans".to_string()),
        );
        assert!(set.is_empty());
    }
}
```

- [ ] **Step 2.2: Run the unit tests, expect "function not defined"**

Run:
```bash
cargo test --lib resuming_node_ids_tests 2>&1 | tail -20
```

Expected: FAIL — `cannot find function compute_resuming_node_ids in `DagRunUseCase``.

- [ ] **Step 2.3: Implement the helper**

In `run_use_case.rs`, find `fn find_status_by_key` (around line 726). Immediately above or below it, inside the same `impl DagRunUseCase` block, add:

```rust
/// Compute the set of node ids whose persisted output has
/// `__colmena_status: "SUSPENDED"` (recursive search via
/// `find_status_by_key`). Returns an empty set when `resume_answer`
/// is `None` — there's no run to resume, so nothing to inject into.
///
/// See spec
/// `docs/superpowers/specs/2026-06-05-suspend-resume-answer-routing-fix-design.md`
/// §4.1.1.
fn compute_resuming_node_ids(
    all_outputs: &std::collections::HashMap<String, Value>,
    resume_answer: &Option<String>,
) -> std::collections::HashSet<String> {
    if resume_answer.is_none() {
        return std::collections::HashSet::new();
    }
    all_outputs
        .iter()
        .filter_map(|(nid, out)| {
            if Self::find_status_by_key(out, "__colmena_status")
                == Some("SUSPENDED".to_string())
            {
                Some(nid.clone())
            } else {
                None
            }
        })
        .collect()
}
```

- [ ] **Step 2.4: Run the unit tests, verify they pass**

Run:
```bash
cargo test --lib resuming_node_ids_tests 2>&1 | tail -20
```

Expected: PASS — `test result: ok. 4 passed`.

- [ ] **Step 2.5: Commit**

```bash
git add src/libs/colmena/src/dag_engine/application/run_use_case.rs
git commit -m "$(cat <<'EOF'
feat(dag_engine): add compute_resuming_node_ids helper

Pure function that returns the set of node ids in a persisted
DagRunState whose output had __colmena_status: "SUSPENDED" (recursive
match via find_status_by_key). Empty when resume_answer is None.

Helper only — not yet wired into the run loop. Wiring lands in the
next commit so the engine change can be reverted in isolation.

Spec: docs/superpowers/specs/2026-06-05-suspend-resume-answer-routing-fix-design.md §4.1.1
EOF
)"
```

---

## Task 3: Engine wiring — gate the injection

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/application/run_use_case.rs:377-379` and surrounding init (around line 240–254)

- [ ] **Step 3.1: Compute the set once, before the loop**

Find the block after the `match (&resume_session_id, &agent_session_id)` (closes around line 240). Find the `if active_queue.is_empty()` block (around line 242). Insert the following AFTER line 254 (after the queue init block closes) and BEFORE line 256 (`if !global_shared_state.is_object()`):

```rust
            // Build the resuming-node-ids set BEFORE the main loop.
            // The loop's `all_outputs.remove(&node_id)` at the top of each
            // iteration destroys the SUSPENDED marker once a node re-executes,
            // so we snapshot the set up front.
            //
            // A node is "resuming" iff its persisted output has
            // `__colmena_status: "SUSPENDED"` (recursive). Used at line ~377
            // to gate `__colmena_resume_answer` injection.
            //
            // Spec: docs/superpowers/specs/2026-06-05-suspend-resume-answer-routing-fix-design.md §4.1
            let resuming_node_ids: std::collections::HashSet<String> =
                Self::compute_resuming_node_ids(&all_outputs, &resume_answer);
```

(Be precise about insertion location. Use grep first:
```bash
grep -n "if active_queue.is_empty" src/libs/colmena/src/dag_engine/application/run_use_case.rs
grep -n "if !global_shared_state.is_object" src/libs/colmena/src/dag_engine/application/run_use_case.rs
```
Insert between those two blocks.)

- [ ] **Step 3.2: Gate the injection at line 377**

Replace the existing block:

```rust
                if let Some(ans) = &resume_answer {
                    inputs.insert("__colmena_resume_answer".to_string(), Value::String(ans.clone()));
                }
```

with:

```rust
                // Inject __colmena_resume_answer only for nodes that were SUSPENDED
                // in the persisted snapshot. See spec §3.1 and §4.1.2.
                if let Some(ans) = &resume_answer {
                    if resuming_node_ids.contains(&node_id) {
                        inputs.insert(
                            "__colmena_resume_answer".to_string(),
                            Value::String(ans.clone()),
                        );
                    } else {
                        tracing::trace!(
                            target: "colmena::dag_engine",
                            node_id = %node_id,
                            "resume_answer present but node was not in SUSPENDED set; skipping injection"
                        );
                    }
                }
```

- [ ] **Step 3.3: Run the integration test from Task 1**

```bash
source .env && cargo test --test suspend_resume_routing suspend_then_llm_resume_runs_llm_fresh -- --ignored --nocapture
```

Expected: PASS. `poet` node emits the scripted completion; no `NodeError`.

If it still fails, do NOT add more fixes. Re-read the error event payload, then revisit §3 of the spec — the gating logic should be the sole change needed. Likely culprits: wrong insertion location for the `let resuming_node_ids` binding (must be inside the `async_stream::try_stream!` async block), or `tracing` not imported.

If `tracing` is not yet imported in `run_use_case.rs`, add `use tracing;` at the top (alongside the existing `use` block) — but first check via:
```bash
grep -n "^use tracing\|^use crate::dag_engine.*tracing" src/libs/colmena/src/dag_engine/application/run_use_case.rs
```
If tracing IS already in scope (via re-export or another `use`), no import needed.

- [ ] **Step 3.4: Run the full unit suite to confirm no regression**

```bash
cargo test --lib 2>&1 | tail -30
```

Expected: PASS across all tests.

- [ ] **Step 3.5: Commit**

```bash
git add src/libs/colmena/src/dag_engine/application/run_use_case.rs
git commit -m "$(cat <<'EOF'
fix(dag_engine): gate __colmena_resume_answer injection by SUSPENDED set

Restrict the injection of __colmena_resume_answer to nodes that
were SUSPENDED in the persisted snapshot. Computed once at run start
via compute_resuming_node_ids, gates the injection inside the main
node loop.

Fixes: llm_call downstream of a fresh suspend aborting with
"llm_call resume: no pending tool call found in conversation
history". Also fixes the suspend→suspend cascade variant of the
same root cause (validated in next commit).

No public API change. Orchestrator/subgraph keep their existing
internal threading; their bubble-up SUSPENDED output keeps them in
the resuming set, so their behavior is unchanged.

Spec: docs/superpowers/specs/2026-06-05-suspend-resume-answer-routing-fix-design.md
Reported: ADP 2026-06-04.
EOF
)"
```

---

## Task 4: Cascade integration test — `suspend → suspend → log`

**Files:**
- Create: `tests/graphs/basic/suspend_cascade.json`
- Modify: `src/libs/colmena/tests/suspend_resume_routing.rs` (add second test)

- [ ] **Step 4.1: Write the cascade graph fixture**

Create `tests/graphs/basic/suspend_cascade.json`:

```json
{
  "nodes": {
    "start": { "type": "mock_input", "config": { "input_data": "kickoff" } },
    "ask_one": {
      "type": "suspend",
      "config": { "id": "ask_one", "question": "Primera pregunta?", "question_type": "open" }
    },
    "ask_two": {
      "type": "suspend",
      "config": { "id": "ask_two", "question": "Segunda pregunta?", "question_type": "open" }
    },
    "finish": { "type": "log", "config": { "prefix": "cascade_result:" } }
  },
  "edges": [
    { "from": "start",   "to": "ask_one" },
    { "from": "ask_one", "to": "ask_two" },
    { "from": "ask_two", "to": "finish" }
  ]
}
```

- [ ] **Step 4.2: Add the cascade test**

Append to `src/libs/colmena/tests/suspend_resume_routing.rs`:

```rust
#[tokio::test]
#[ignore = "requires DATABASE_URL — run with `cargo test -- --ignored`"]
async fn suspend_cascade_resumes_each_node_independently() {
    init_logs();
    let chat = "test_suspend_cascade";
    cleanup(chat).await;

    let eng = engine().await;
    let graph_path = "../../tests/graphs/basic/suspend_cascade.json";
    let graph = load_graph(graph_path);

    // Run 1: must suspend at ask_one.
    let ev1 = run_until_done(&eng, graph.clone(), chat, None).await;
    let finish1 = ev1
        .iter()
        .find_map(|e| match e {
            DagExecutionEvent::GraphFinish { output } => Some(output.clone()),
            _ => None,
        })
        .expect("run 1 must emit GraphFinish");
    assert_eq!(
        finish1
            .get("questions")
            .and_then(|q| q.as_array())
            .and_then(|a| a.first())
            .and_then(|q| q.get("id"))
            .and_then(|v| v.as_str()),
        Some("ask_one"),
        "run 1 must pause at ask_one, got: {finish1:#}"
    );

    // Run 2: answer ask_one only. Must run ask_two fresh and pause there.
    let ans1 = "Q[ask_one]: Primera pregunta?\nA[ask_one]: alfa".to_string();
    let ev2 = run_until_done(&eng, graph.clone(), chat, Some(ans1)).await;
    for e in &ev2 {
        if let DagExecutionEvent::NodeError { node_id, error } = e {
            panic!("cascade resume #1 errored: node={node_id} error={error}");
        }
    }
    let finish2 = ev2
        .iter()
        .find_map(|e| match e {
            DagExecutionEvent::GraphFinish { output } => Some(output.clone()),
            _ => None,
        })
        .expect("run 2 must emit GraphFinish");
    assert_eq!(
        finish2
            .get("questions")
            .and_then(|q| q.as_array())
            .and_then(|a| a.first())
            .and_then(|q| q.get("id"))
            .and_then(|v| v.as_str()),
        Some("ask_two"),
        "run 2 must pause at ask_two, got: {finish2:#}"
    );

    // Run 3: answer ask_two. Must reach finish.
    let ans2 = "Q[ask_two]: Segunda pregunta?\nA[ask_two]: beta".to_string();
    let ev3 = run_until_done(&eng, graph, chat, Some(ans2)).await;
    for e in &ev3 {
        if let DagExecutionEvent::NodeError { node_id, error } = e {
            panic!("cascade resume #2 errored: node={node_id} error={error}");
        }
    }
    let reached_finish = ev3.iter().any(|e| {
        matches!(e, DagExecutionEvent::NodeFinish { node_id, .. } if node_id == "finish")
    });
    assert!(reached_finish, "run 3 must reach `finish` node");

    cleanup(chat).await;
}
```

- [ ] **Step 4.3: Run the cascade test, expect PASS**

```bash
source .env && cargo test --test suspend_resume_routing suspend_cascade_resumes_each_node_independently -- --ignored --nocapture
```

Expected: PASS. (The engine fix from Task 3 already covers cascade. This test guards against future regression.)

- [ ] **Step 4.4: Commit**

```bash
git add tests/graphs/basic/suspend_cascade.json \
        src/libs/colmena/tests/suspend_resume_routing.rs
git commit -m "$(cat <<'EOF'
test(suspend): integration test for suspend→suspend cascade resume

Three-run scenario: pause at suspend1, resume answering only
suspend1, must pause fresh at suspend2; resume answering suspend2,
must reach finish. Validates that the engine fix from the prior
commit also covers the cascade variant where the second suspend
previously errored with "missing answer for <id>" because it
received the wrong Q/A payload.

Spec: docs/superpowers/specs/2026-06-05-suspend-resume-answer-routing-fix-design.md §5 row 2
EOF
)"
```

---

## Task 5: Defensive guard in `llm.rs`

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs:1799-1810`

- [ ] **Step 5.1: Verify current code**

Run:
```bash
sed -n '1799,1812p' src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs
```

Confirm the block matches the spec's "ANTES" snippet (§4.2.1). If it diverges (e.g., line drift), use that output as the new reference.

- [ ] **Step 5.2: Replace the `.ok_or(...)?` with a `match`**

Edit `llm.rs`. Replace:

```rust
        if let Some(answer) = resume_answer.as_deref() {
            let conversation = conversation_repo.get_by_id(&conversation_key).await?;
            let pending = find_pending_tool_call(&conversation.messages)
                .ok_or("llm_call resume: no pending tool call found in conversation history")?;

            tracing::info!(
                target: "colmena::llm_node",
                "llm_call: resume — replaying pending tool with user answer"
            );
            let result = tool_executor
                .execute_with_resume_answer(&pending, answer)
                .await?;
```

with:

```rust
        if let Some(answer) = resume_answer.as_deref() {
            let conversation = conversation_repo.get_by_id(&conversation_key).await?;
            let maybe_pending = find_pending_tool_call(&conversation.messages);
            if let Some(pending) = maybe_pending {
                tracing::info!(
                    target: "colmena::llm_node",
                    "llm_call: resume — replaying pending tool with user answer"
                );
                let result = tool_executor
                    .execute_with_resume_answer(&pending, answer)
                    .await?;
```

Then, AT THE END of the existing resume block (just before the closing `}` of the outer `if let Some(answer) = resume_answer.as_deref()`), add the `else` for `maybe_pending`:

```rust
            } else {
                // Defense-in-depth: if the engine's per-node gating
                // (run_use_case.rs §4.1) is broken and we received
                // __colmena_resume_answer despite having no pending tool
                // call, fall through to the fresh-run path instead of
                // aborting the DAG.
                //
                // Spec: docs/superpowers/specs/2026-06-05-suspend-resume-answer-routing-fix-design.md §4.2.1
                let node_name = inputs
                    .get("__node_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("(unknown)");
                tracing::warn!(
                    target: "colmena::llm_node",
                    node_id = node_name,
                    "llm_call: resume_answer present but no pending tool call in history; \
                     falling through to fresh run (engine routing may be broken)"
                );
                // Intentional fallthrough — control continues to the
                // standard agent_service.run path below.
            }
        }
```

Important: the `if let Some(pending) = ... { ... } else { ... }` must be the new shape of that block. The whole `if let Some(answer)` outer block must remain — only the inner control flow changes.

Use the Edit tool with enough surrounding context to make the replacement unique (the `tracing::info!` line + `execute_with_resume_answer` are good anchors).

- [ ] **Step 5.3: Build to confirm no syntax error**

```bash
cargo build --lib 2>&1 | tail -20
```

Expected: clean build, no warnings (per the repo's deny-warnings policy).

If `inputs` is not in scope at the point of the `tracing::warn!`, swap to `node_id_path_str` or any other already-in-scope variable. Search for `__node_id` references nearby with `grep -n "__node_id" src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs` to confirm scope.

- [ ] **Step 5.4: Re-run Task 1's test to confirm still PASS**

```bash
source .env && cargo test --test suspend_resume_routing -- --ignored --nocapture
```

Expected: PASS for both tests.

- [ ] **Step 5.5: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs
git commit -m "$(cat <<'EOF'
fix(llm_call): defensive fallthrough when resume_answer has no pending tool

Replace the unconditional `.ok_or("...no pending tool call...")?`
with a `match` that falls through to the fresh-run path when
find_pending_tool_call returns None. Emits a structured warn! log
so the regression is observable in tracing/Cloud Logging.

This is a belt-and-suspenders complement to the engine fix in
run_use_case.rs: in normal operation the fallthrough branch is
unreachable (the engine no longer injects __colmena_resume_answer
into a non-suspended llm_call). The guard ensures that if a future
engine refactor regresses the routing, users get a fresh run with
a warning instead of a hard DAG abort.

Spec: docs/superpowers/specs/2026-06-05-suspend-resume-answer-routing-fix-design.md §4.2.1
EOF
)"
```

---

## Task 6: Regression check — LLM-with-suspend-tool path

**Files:** none new (run existing test).

- [ ] **Step 6.1: Run the canonical LLM-with-suspend-tool integration test**

```bash
source .env && cargo test --test llm_tool_suspend_integration -- --ignored --nocapture 2>&1 | tail -40
```

Expected: PASS. This is the case where the `llm_call` is the one that suspends (via a tool), gets resumed, and continues. Behavior must be unchanged.

If any test in that suite fails:
- STOP.
- Re-read the spec §5 row 6 ("LLM-with-suspend-tool"). The `llm_call` should still be in `resuming_node_ids` because its own output is SUSPENDED.
- Investigate via `cargo test --test llm_tool_suspend_integration -- --ignored --nocapture` (no test filter) to see which sub-test fails.
- Do NOT proceed to Task 7 until this regression test passes.

- [ ] **Step 6.2: Run the full integration suite for broad regression coverage**

```bash
source .env && cargo test --verbose -- --ignored 2>&1 | tail -60
```

Expected: all `#[ignore]`-gated tests PASS (or are appropriately skipped when an env var is missing — see the test source for which vars are required).

- [ ] **Step 6.3: No commit needed** — this task only verifies.

---

## Task 7: Documentation cross-link + CHANGELOG

**Files:**
- Modify: `docs/developer_guide/38_suspend_node.md`
- Modify or Create: `docs/CHANGELOG_2026-06.md`

- [ ] **Step 7.1: Add cross-link in the suspend developer guide**

Find the "Referencias cruzadas" section in `docs/developer_guide/38_suspend_node.md` (near the bottom, section 9). Add this bullet (preserving Spanish convention):

```markdown
- **Fix de ruteo de `resume_answer`**: [`docs/superpowers/specs/2026-06-05-suspend-resume-answer-routing-fix-design.md`](../superpowers/specs/2026-06-05-suspend-resume-answer-routing-fix-design.md) — el engine inyecta `__colmena_resume_answer` solo en nodos que estaban SUSPENDED en el snapshot persistido.
```

- [ ] **Step 7.2: Add CHANGELOG entry**

Check whether `docs/CHANGELOG_2026-06.md` exists:
```bash
ls docs/CHANGELOG_2026-06.md 2>/dev/null || echo "create new"
```

If it does NOT exist, create it with this content (Spanish, matching `CHANGELOG_2026-05.md` style — open that file first to mirror its header):

```markdown
# CHANGELOG — 2026-06

## 2026-06-05

### Bugfixes

- **`dag_engine`**: el engine deja de inyectar `__colmena_resume_answer` en nodos
  que no estaban suspendidos en el snapshot persistido. Arregla el error
  `llm_call resume: no pending tool call found in conversation history` cuando un
  `llm_call` está aguas abajo de un `suspend`, y también la cascada
  `suspend → suspend` que fallaba con `missing answer`. Sin cambio de API
  pública. Spec:
  [`docs/superpowers/specs/2026-06-05-suspend-resume-answer-routing-fix-design.md`](superpowers/specs/2026-06-05-suspend-resume-answer-routing-fix-design.md).
- **`llm_call`**: guard defensivo en la rama de resume. Si la rama recibe
  `__colmena_resume_answer` pero no hay un tool call pendiente en el historial,
  loggea `warn!` y cae a fresh run en vez de errorear.
```

If it EXISTS, append the same entries under a new `## 2026-06-05` heading (or under the existing one if today's heading is already present).

- [ ] **Step 7.3: Commit docs**

```bash
git add docs/developer_guide/38_suspend_node.md docs/CHANGELOG_2026-06.md
git commit -m "$(cat <<'EOF'
docs(suspend): cross-link the resume_answer routing fix spec + CHANGELOG

Adds a "Referencias cruzadas" entry in 38_suspend_node.md pointing
at the routing-fix design doc, and a CHANGELOG entry under June 5.
EOF
)"
```

---

## Task 8: ADP repo sweep + final verification

**Files:** none changed in this task.

- [ ] **Step 8.1: Sweep the ADP worker/api for any consumer of `__colmena_resume_answer`**

Per CLAUDE.md's breaking-change discipline (worker pulls colmena develop directly via Cargo):

```bash
grep -rn "__colmena_resume_answer\|colmena_resume_answer" \
  /Users/danielgarcia/startti/adp/apps/service/ia/platform/worker/src/ \
  /Users/danielgarcia/startti/adp/apps/service/ia/platform/api/src/ \
  2>/dev/null || echo "no matches — clean"
```

Expected: zero matches. The key is internal to colmena's engine; ADP should never read it directly.

If matches appear: STOP. Inspect each match. If the ADP code reads the key from a node it doesn't own, that's a pre-existing anti-pattern (would have been wrong regardless of this fix); flag to the user before continuing.

- [ ] **Step 8.2: Run the full Rust test suite (CI-equivalent)**

```bash
source .env && cargo test --verbose 2>&1 | tail -40
```

Expected: PASS across all unit, integration, and doctests. This is the same command CI runs (per CLAUDE.md "CI vs local").

- [ ] **Step 8.3: Clippy + fmt sanity**

```bash
cargo clippy --all-targets -- -D warnings 2>&1 | tail -20
cargo fmt --check
```

Expected: clippy clean, fmt clean. If fmt complains, run `cargo fmt` and amend the most recent commit (`git commit --amend --no-edit`) or commit the formatting as a separate `style:` commit.

- [ ] **Step 8.4: Run the two new graphs via the CLI for a smoke validation**

```bash
source .env

# Repro: should suspend on run 1, complete on run 2
cargo run --bin dag_engine -- run \
  tests/graphs/basic/suspend_then_llm_resume.json \
  --agent-session-id agent_smoke_55 \
  > /tmp/colmena_e2e/suspend_then_llm_resume.run1.sse 2>&1

cargo run --bin dag_engine -- run \
  tests/graphs/basic/suspend_then_llm_resume.json \
  --agent-session-id agent_smoke_55 \
  --answer "Q[ask_name]: ¿Cuál es tu nombre?
A[ask_name]: Julián" \
  > /tmp/colmena_e2e/suspend_then_llm_resume.run2.sse 2>&1

# Cascade
cargo run --bin dag_engine -- run \
  tests/graphs/basic/suspend_cascade.json \
  --agent-session-id agent_smoke_cascade \
  > /tmp/colmena_e2e/suspend_cascade.run1.sse 2>&1

cargo run --bin dag_engine -- run \
  tests/graphs/basic/suspend_cascade.json \
  --agent-session-id agent_smoke_cascade \
  --answer "Q[ask_one]: Primera pregunta?
A[ask_one]: alfa" \
  > /tmp/colmena_e2e/suspend_cascade.run2.sse 2>&1

cargo run --bin dag_engine -- run \
  tests/graphs/basic/suspend_cascade.json \
  --agent-session-id agent_smoke_cascade \
  --answer "Q[ask_two]: Segunda pregunta?
A[ask_two]: beta" \
  > /tmp/colmena_e2e/suspend_cascade.run3.sse 2>&1
```

Run `mkdir -p /tmp/colmena_e2e` first if the directory doesn't exist (per user memory rule).

Inspect each `.sse` file:
- `suspend_then_llm_resume.run1.sse`: contains `finishReason: "suspended"`.
- `suspend_then_llm_resume.run2.sse`: contains a `node-end` for `poet` with a non-empty completion; NO `error` events.
- `suspend_cascade.run1.sse`: suspends at `ask_one`.
- `suspend_cascade.run2.sse`: suspends at `ask_two`.
- `suspend_cascade.run3.sse`: reaches `finish` cleanly.

- [ ] **Step 8.5: Build a friendly report and present to the user**

Per user memory rule ("Graph runs → save to /tmp + friendly report"), summarize each SSE file (input, key payload, tokens if any, summary verdict) and present in chat. Do NOT paste raw SSE.

- [ ] **Step 8.6: No commit in this task** — verification only.

---

## Final state check

After Task 8 completes:

- [ ] All commits on the worktree branch:
  ```
  docs(suspend): cross-link the resume_answer routing fix spec + CHANGELOG
  test(suspend): integration test for suspend→suspend cascade resume
  fix(llm_call): defensive fallthrough when resume_answer has no pending tool
  fix(dag_engine): gate __colmena_resume_answer injection by SUSPENDED set
  feat(dag_engine): add compute_resuming_node_ids helper
  test(suspend): failing repro of suspend→llm_call resume bug
  docs(spec): suspend resume_answer routing fix (Approach B)
  ```
  (Order is bottom-to-top in `git log` — the spec commit is the oldest.)

- [ ] `cargo test --verbose` passes.
- [ ] `cargo clippy --all-targets -- -D warnings` passes.
- [ ] `cargo fmt --check` passes.
- [ ] ADP sweep clean.
- [ ] Smoke CLI runs reported.

At this point invoke `superpowers:finishing-a-development-branch` to decide how to integrate (PR, merge, etc.).
