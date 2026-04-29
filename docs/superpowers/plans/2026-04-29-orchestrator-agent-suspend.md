# Orchestrator Agent-Suspend Propagation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the orchestrator propagate `__colmena_status: SUSPENDED` when one of its agents (a subgraph) internally suspends, so the chain unwinds correctly on resume by `agent_session_id` and the user's answer reaches the actual `suspend` node at the leaf.

**Architecture:** Three coordinated changes. (A) Extend `SuspendNode` to emit a canonical `questions` array (with `id`, `type`, `options`) alongside the legacy `question` string. (B) In `orchestrator.rs`, detect SUSPENDED in the agent dispatch loop and call a new `propagate_agent_suspend` helper that delegates to the existing `make_suspend_response("agent", ...)`. (C) Add a resume-detection arm for `suspended_at == "agent"` and a flag (`resuming_agent_suspend`) so the orchestrator preserves `__colmena_resume_answer` in the dispatched task's inputs (instead of stripping it like the legacy critic/planner suspend paths).

**Tech Stack:** Rust 1.x, sqlx (Postgres), tokio, async-trait, serde_json. Tests use `cargo test`. Integration tests connect to Postgres via `DATABASE_URL`.

**Spec:** [docs/superpowers/specs/2026-04-29-orchestrator-agent-suspend-design.md](docs/superpowers/specs/2026-04-29-orchestrator-agent-suspend-design.md)

---

## File structure

### Files modified

| Path | What changes |
|---|---|
| `src/libs/colmena/src/dag_engine/infrastructure/nodes/suspend.rs` | Accept new optional config (`id`, `question_type`, `options`); emit canonical `questions` array alongside `question` string; inline unit tests |
| `src/libs/colmena/src/dag_engine/infrastructure/nodes/orchestrator.rs` | Add `propagate_agent_suspend` helper; detect SUSPENDED in dispatch loop; add `resuming_agent_suspend` flag; new resume-detection arm; preserve `__colmena_resume_answer` conditionally |

### Files created

| Path | What it does |
|---|---|
| `src/libs/colmena/tests/orchestrator_agent_suspend.rs` | End-to-end integration tests: single-agent suspend/resume; nested orchestrators (3 levels using existing graph); error on multi-suspend |

### Files referenced (no changes)

| Path | Why |
|---|---|
| `tests/graphs/advanced/nested_orchestrators_suspend.json` | Already created during exploratory testing; used as the e2e fixture |

---

## Task summary

| # | Phase | Task |
|---|---|---|
| 1 | SuspendNode | Extend `SuspendNode` with `id`/`question_type`/`options` config + canonical `questions` output |
| 2 | Orchestrator core | Add `propagate_agent_suspend` helper |
| 3 | Orchestrator core | Wire detection in agent dispatch loop |
| 4 | Resume path | Add `resuming_agent_suspend` flag + preserve `__colmena_resume_answer` in `task_inputs` |
| 5 | Resume path | Add `"agent"` arm in resume-detection block |
| 6 | Tests | Integration test: single-agent suspend + resume end-to-end |
| 7 | Tests | E2E test: nested orchestrators (3 levels) using existing graph |

---

## Task 1 — Extend `SuspendNode` config and output

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/suspend.rs`

- [ ] **Step 1: Write failing unit tests**

Replace the entire file content with:

```rust
use crate::dag_engine::domain::node::{ExecutableNode, NodeInputs};
use crate::dag_engine::domain::observer::ExecutionObserver;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::error::Error;
use std::sync::Arc;

/// A Human-in-the-Loop node that pauses graph execution to ask the user a question.
///
/// Config fields:
/// - `question` (required, string): the question to display to the user.
/// - `id` (optional, string): stable identifier for the question. Defaults to the local node_id
///   (engine-injected as `__node_id`). Used by external clients (UIs) to map the suspend
///   to a specific UI widget.
/// - `question_type` (optional, "open" | "choice"): defaults to "open" (free-text answer).
/// - `options` (optional, array of strings): only meaningful when `question_type == "choice"`.
pub struct SuspendNode;

#[async_trait]
impl ExecutableNode for SuspendNode {
    async fn execute(
        &self,
        inputs: &NodeInputs,
        config: &Value,
        _global_state: &mut Value,
        _observer: Option<Arc<dyn ExecutionObserver>>,
    ) -> Result<Value, Box<dyn Error + Send + Sync>> {
        // Resume path: user provided an answer.
        if let Some(answer) = inputs.get("__colmena_resume_answer") {
            return Ok(json!({
                "status": "resumed",
                "answer_received": answer
            }));
        }

        // Suspend path: build canonical question and emit both the legacy `question`
        // string and the canonical `questions` array.
        let question = config
            .get("question")
            .and_then(|v| v.as_str())
            .unwrap_or("What is your input?")
            .to_string();

        let id = config
            .get("id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .or_else(|| inputs.get("__node_id").and_then(|v| v.as_str()).map(|s| s.to_string()))
            .unwrap_or_else(|| "suspend".to_string());

        let question_type = config
            .get("question_type")
            .and_then(|v| v.as_str())
            .unwrap_or("open")
            .to_string();

        let options: Option<Vec<String>> = config
            .get("options")
            .and_then(|v| serde_json::from_value(v.clone()).ok());

        let mut question_obj = serde_json::Map::new();
        question_obj.insert("id".to_string(), Value::String(id));
        question_obj.insert("question".to_string(), Value::String(question.clone()));
        question_obj.insert("type".to_string(), Value::String(question_type));
        if let Some(opts) = options {
            question_obj.insert(
                "options".to_string(),
                Value::Array(opts.into_iter().map(Value::String).collect()),
            );
        }

        Ok(json!({
            "__colmena_status": "SUSPENDED",
            "question": question,
            "questions": [Value::Object(question_obj)]
        }))
    }

    fn default_input(&self) -> Option<&str> {
        Some("question")
    }

    fn default_output(&self) -> Option<&str> {
        Some("answer_received")
    }

    fn schema(&self) -> Value {
        json!({})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashMap;

    fn empty_observer() -> Option<Arc<dyn ExecutionObserver>> {
        None
    }

    fn inputs_with_node_id(id: &str) -> NodeInputs {
        let mut m: HashMap<String, Value> = HashMap::new();
        m.insert("__node_id".to_string(), Value::String(id.to_string()));
        m
    }

    #[tokio::test]
    async fn suspend_emits_open_when_no_config() {
        let node = SuspendNode;
        let inputs = inputs_with_node_id("ask_user");
        let cfg = json!({ "question": "Confirm?" });
        let mut state = Value::Null;
        let out = node.execute(&inputs, &cfg, &mut state, empty_observer()).await.unwrap();

        assert_eq!(out["__colmena_status"], "SUSPENDED");
        assert_eq!(out["question"], "Confirm?");
        assert_eq!(out["questions"][0]["type"], "open");
        assert!(out["questions"][0].get("options").is_none() || out["questions"][0]["options"].is_null());
    }

    #[tokio::test]
    async fn suspend_emits_choice_with_options() {
        let node = SuspendNode;
        let inputs = inputs_with_node_id("ask_user");
        let cfg = json!({
            "question": "Pick one",
            "question_type": "choice",
            "options": ["a", "b", "c"]
        });
        let mut state = Value::Null;
        let out = node.execute(&inputs, &cfg, &mut state, empty_observer()).await.unwrap();

        assert_eq!(out["questions"][0]["type"], "choice");
        assert_eq!(out["questions"][0]["options"], json!(["a", "b", "c"]));
    }

    #[tokio::test]
    async fn suspend_uses_local_node_id_as_default_id() {
        let node = SuspendNode;
        let inputs = inputs_with_node_id("ask_user");
        let cfg = json!({ "question": "Confirm?" });
        let mut state = Value::Null;
        let out = node.execute(&inputs, &cfg, &mut state, empty_observer()).await.unwrap();

        assert_eq!(out["questions"][0]["id"], "ask_user");
    }

    #[tokio::test]
    async fn suspend_uses_explicit_id_when_set() {
        let node = SuspendNode;
        let inputs = inputs_with_node_id("ask_user");
        let cfg = json!({ "id": "confirm_transfer", "question": "Confirm?" });
        let mut state = Value::Null;
        let out = node.execute(&inputs, &cfg, &mut state, empty_observer()).await.unwrap();

        assert_eq!(out["questions"][0]["id"], "confirm_transfer");
    }

    #[tokio::test]
    async fn suspend_preserves_legacy_question_field() {
        let node = SuspendNode;
        let inputs = inputs_with_node_id("ask_user");
        let cfg = json!({ "question": "Confirm?", "question_type": "choice", "options": ["a","b"] });
        let mut state = Value::Null;
        let out = node.execute(&inputs, &cfg, &mut state, empty_observer()).await.unwrap();

        // Legacy field still present alongside the canonical questions array.
        assert_eq!(out["question"], "Confirm?");
        assert!(out["questions"].is_array());
    }

    #[tokio::test]
    async fn resume_path_unchanged() {
        let node = SuspendNode;
        let mut inputs: NodeInputs = HashMap::new();
        inputs.insert("__colmena_resume_answer".to_string(), Value::String("yes".to_string()));
        let cfg = json!({ "question": "Confirm?" });
        let mut state = Value::Null;
        let out = node.execute(&inputs, &cfg, &mut state, empty_observer()).await.unwrap();

        assert_eq!(out["status"], "resumed");
        assert_eq!(out["answer_received"], "yes");
    }
}
```

- [ ] **Step 2: Run tests to verify they pass**

Run: `cargo test --lib --package colmena_dag_engine dag_engine::infrastructure::nodes::suspend`
Expected: 6 tests pass.

(The replacement above already includes the implementation. Step 1's "tests" and the implementation land together because the file was rewritten as a single coherent unit. If a strict TDD pass is desired, separate the tests from the impl and run twice — but the rewrite as one unit is cleaner here since the file is small.)

- [ ] **Step 3: Smoke test that an existing graph still works**

```bash
source .env
cargo run --bin dag_engine -- run tests/graphs/basic/test_suspend_manual.json --agent-session-id smoke_t1_oasm
```

Expected: graph runs, suspends with question "¿Apruebas continuar con el proceso?"; the output includes both `question: "..."` and `questions: [{...}]`.

Cleanup:
```bash
psql "$DATABASE_URL" -c "DELETE FROM dag_runs WHERE agent_session_id = 'smoke_t1_oasm';"
```

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/suspend.rs
git commit -m "feat(suspend): emit canonical questions array with id/type/options

Extends SuspendNode config with optional id, question_type ('open' or
'choice'), and options. Output now emits both the legacy 'question'
string (back-compat) and a canonical 'questions: [{id, question, type,
options?}]' array used by the orchestrator's agent-suspend propagation.

Defaults: id falls back to the local node_id (engine-injected
__node_id); question_type defaults to 'open'; options is omitted when
type='open'."
```

---

## Task 2 — Add `propagate_agent_suspend` helper

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/orchestrator.rs`

- [ ] **Step 1: Locate the helpers block**

The orchestrator file has free helper functions near the bottom. Search for `fn make_suspend_response`:

```bash
grep -n "fn make_suspend_response\|fn clear_suspend_meta\|fn allow_suspend_for" src/libs/colmena/src/dag_engine/infrastructure/nodes/orchestrator.rs
```

You should see lines around 1989 (`make_suspend_response`), 2012 (`clear_suspend_meta`), 2020 (`allow_suspend_for`). Add the new helper directly after `make_suspend_response`.

- [ ] **Step 2: Add the helper**

Insert this function right after `make_suspend_response` (and before `clear_suspend_meta`):

```rust
/// Propagates a SUSPENDED status from an agent's child subgraph back through
/// the orchestrator. The agent's child run is already SUSPENDED in dag_runs;
/// our job is to:
///   1. extract user-facing questions from `agent_result` (canonical
///      `questions` array if present, else wrap the legacy `question` string),
///   2. emit a SUSPENDED response of our own via `make_suspend_response`,
///   3. with `suspended_at = "agent"` so the resume detection block knows
///      this came from an agent dispatch.
///
/// Note: caller is responsible for NOT marking the task `completed=true` —
/// the task stays incomplete in `dag_task_memory`, so it gets re-dispatched
/// on resume; the subgraph node's resume path then cascades the answer down
/// to the existing SUSPENDED child via `find_suspended_child`.
fn propagate_agent_suspend(
    agent_result: &Value,
    task: &DagTask,
    phase: i32,
    state: &mut Value,
) -> Value {
    // Prefer canonical `questions` array (covers nested orchestrators that
    // already produced canonical output AND the new SuspendNode shape).
    let questions: Vec<SuspendQuestion> = if let Some(qs_val) = agent_result.get("questions") {
        serde_json::from_value(qs_val.clone()).unwrap_or_default()
    } else if let Some(q_str) = agent_result.get("question").and_then(|v| v.as_str()) {
        // Fallback: agent emitted only legacy single-string. Wrap as one open question.
        vec![SuspendQuestion {
            id: format!("agent_{}", task.assigned_to),
            question: q_str.to_string(),
            question_type: "open".to_string(),
            options: None,
        }]
    } else {
        // Degenerate: SUSPENDED but no questions metadata at all.
        vec![SuspendQuestion {
            id: format!("agent_{}", task.assigned_to),
            question: format!("Agent '{}' requires user input.", task.assigned_to),
            question_type: "open".to_string(),
            options: None,
        }]
    };

    make_suspend_response(state, "agent", phase, Some(&task.id), questions)
}
```

- [ ] **Step 3: Compile**

Run: `cargo build`
Expected: success. The function isn't called yet but should compile cleanly.

- [ ] **Step 4: Commit (interim — no behavior change yet)**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/orchestrator.rs
git commit -m "feat(orchestrator): add propagate_agent_suspend helper

Pure helper (not yet wired) that converts an agent's SUSPENDED output
into a make_suspend_response('agent', ...) call. Handles both canonical
'questions' arrays (from nested orchestrators or new SuspendNode) and
legacy 'question' strings (older SuspendNode graphs). Will be called
from the agent dispatch site in the next task."
```

---

## Task 3 — Wire detection in the agent dispatch loop

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/orchestrator.rs:1487-1500` (the agent dispatch site, inside the `for task in tasks_to_run` loop)

- [ ] **Step 1: Locate the dispatch site**

```bash
grep -n "let agent_result = subgraph_node" src/libs/colmena/src/dag_engine/infrastructure/nodes/orchestrator.rs
```

Expected: one match around line 1487.

Read the surrounding 30 lines to understand context. The pattern is:
```rust
let agent_result = subgraph_node
    .execute(&task_inputs, &subgraph_cfg, _state, _observer.clone())
    .await?;

colmena_log!("📬 [OrchestratorNode] RAW RESULT ← agent '{}' | task '{}'\n...");

// ── Crítica ──
let stash_key = format!("__orch_pending_{}", task.id);
let mut is_ok = true;
if let Some(critic_cfg) = config.get(KEY_CRITIC) { ... }
```

- [ ] **Step 2: Insert the SUSPENDED check**

Right after the `colmena_log!("📬 ...")` line and BEFORE the `// ── Crítica ──` block, insert:

```rust
// ── Agent suspend propagation (spec §5) ──
// If the agent's subgraph internally suspended, do NOT run the critic, do NOT
// mark the task completed, do NOT continue the phase. Just propagate up.
//
// Note on allow_suspend: agents already have a SUSPENDED dag_runs row at this
// point — we cannot ignore the suspend without leaving an orphan. We log and
// propagate regardless of the flag.
if agent_result
    .get("__colmena_status")
    .and_then(|v| v.as_str())
    == Some("SUSPENDED")
{
    if !allow_suspend_for(&subgraph_cfg) {
        colmena_log!(
            "⚠️  [OrchestratorNode] Agent '{}' has allow_suspend=false, but its \
             subgraph already suspended. Flag has no safe effect for agents — \
             propagating suspend.",
            task.assigned_to
        );
    }
    colmena_log!(
        "⏸️  [OrchestratorNode] Agent '{}' suspended (task '{}'). Propagating up.",
        task.assigned_to, task.task_name
    );
    return Ok(propagate_agent_suspend(&agent_result, &task, phase, _state));
}
```

The `phase` variable is already in scope inside the loop. The `_state` and `task` are also in scope. `subgraph_cfg` is the per-agent config (built earlier in the loop).

- [ ] **Step 3: Compile**

Run: `cargo build`
Expected: success.

- [ ] **Step 4: Run lib tests to confirm no regression**

Run: `cargo test --lib --package colmena_dag_engine`
Expected: all existing tests still pass.

- [ ] **Step 5: Manual smoke — orchestrator now suspends correctly**

Use the existing test graph from prior testing:

```bash
source .env
psql "$DATABASE_URL" -c "DELETE FROM dag_runs WHERE agent_session_id = 'smoke_t3_orch_susp';"
cargo run --bin dag_engine -- run tests/graphs/advanced/nested_orchestrators_suspend.json --agent-session-id smoke_t3_orch_susp
```

Expected: the run output's `finishReason` is `"suspended"` (not `"stop"`); the output object contains `__colmena_status: "SUSPENDED"` and a `questions: [...]` array bubbling up from the deepest agent.

Verify:
```bash
psql "$DATABASE_URL" -c "SELECT LEFT(session_id::text, 8), LEFT(parent_session_id::text, 8) AS parent, status FROM dag_runs WHERE agent_session_id = 'smoke_t3_orch_susp' ORDER BY created_at;"
```

Expected: 3 rows, ALL with status `SUSPENDED` (root + team_leader subgraph + confirm_specialist subgraph). This contrasts with the pre-fix behavior where only the deepest was SUSPENDED and the rest were COMPLETED (the bug).

Cleanup:
```bash
psql "$DATABASE_URL" -c "DELETE FROM dag_runs WHERE agent_session_id = 'smoke_t3_orch_susp';"
```

> Note: at this point resume **will not yet work** end-to-end. Tasks 4 and 5 add the resume-side changes. The smoke test here only validates the suspend-side propagation.

- [ ] **Step 6: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/orchestrator.rs
git commit -m "feat(orchestrator): detect and propagate agent-internal suspend

When an agent's subgraph emits __colmena_status=SUSPENDED (because its
internal nodes triggered a suspend), the orchestrator now stops phase
processing immediately, skips the critic, leaves the task as
completed=false in dag_task_memory, and returns its own SUSPENDED
response with the agent's questions wrapped via propagate_agent_suspend.

The chain unwinds through nested orchestrators automatically: the inner
orchestrator emits canonical 'questions' array, the outer reads it
verbatim and propagates further. End-to-end resume cascade lands in the
following two tasks."
```

---

## Task 4 — `resuming_agent_suspend` flag + preserve `__colmena_resume_answer`

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/orchestrator.rs:795-805` (where `resume_answer` is read)
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/orchestrator.rs:1399-1409` (where `task_inputs` is built)

- [ ] **Step 1: Capture the flag at the top of `execute`**

Locate the existing block:
```bash
grep -n "let resume_answer: Option<String>" src/libs/colmena/src/dag_engine/infrastructure/nodes/orchestrator.rs
```

Expected: line ~795. Read the next ~10 lines:

```rust
let resume_answer: Option<String> = inputs
    .get("__colmena_resume_answer")
    .and_then(|v| v.as_str())
    .map(|s| s.to_string());

let mut suspend_meta = read_suspend_meta(_state);

// If resuming from a suspend, handle based on where we suspended.
if let (Some(ref ans), Some((ref sa, _, _, ref questions))) =
    (&resume_answer, &suspend_meta)
{
    ...
}
```

Right after the `let mut suspend_meta = read_suspend_meta(_state);` line (and BEFORE the `if let (...)` block that follows), insert:

```rust
// Capture this immutably BEFORE any clear_suspend_meta calls below — we
// need it later (Task 4 Step 2) when building task_inputs to decide
// whether to preserve __colmena_resume_answer for cascading into the
// agent's subgraph_node.
let resuming_agent_suspend: bool = matches!(
    (&resume_answer, &suspend_meta),
    (Some(_), Some((sa, _, _, _))) if sa == "agent"
);
```

- [ ] **Step 2: Modify the `task_inputs.remove` call**

Locate:
```bash
grep -n "task_inputs.remove..__colmena_resume_answer" src/libs/colmena/src/dag_engine/infrastructure/nodes/orchestrator.rs
```

Expected: line ~1403. Read context:

```rust
let mut task_inputs = inputs.clone();
// Strip __colmena_resume_answer so subgraph agents being executed
// for the first time don't mistakenly try to resume a non-existent
// child session. The orchestrator already consumed this key above.
task_inputs.remove("__colmena_resume_answer");
// Unique __node_id per agent so SubGraphNode generates distinct
// child_session_ids: "{session_id}_sub_{agent_name}"
task_inputs.insert(
    "__node_id".to_string(),
    Value::String(task.assigned_to.clone()),
);
```

Replace the `task_inputs.remove(...)` call (and update the comment) with:

```rust
let mut task_inputs = inputs.clone();
// For legacy suspend sources (planner, critic, phase_reactor), the orchestrator
// already consumed the resume_answer at its own level (e.g. injected into the
// planner's system_message). Stripping the key here prevents fresh agents
// from accidentally entering their resume path.
//
// For agent-suspend resumes (spec §7), however, the answer is FOR the
// SuspendNode at the leaf — it must flow through the subgraph_node so that
// node enters its resume path and cascades down to find_suspended_child.
if !resuming_agent_suspend {
    task_inputs.remove("__colmena_resume_answer");
}
task_inputs.insert(
    "__node_id".to_string(),
    Value::String(task.assigned_to.clone()),
);
```

- [ ] **Step 3: Compile**

Run: `cargo build`
Expected: success.

- [ ] **Step 4: Run lib tests**

Run: `cargo test --lib --package colmena_dag_engine`
Expected: all pass (no behavioral change yet for legacy paths; only adds a flag and conditional skip).

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/orchestrator.rs
git commit -m "feat(orchestrator): preserve __colmena_resume_answer for agent-suspend resumes

When the orchestrator is being resumed from an agent-suspend (suspended_at
== 'agent'), the resume_answer must reach the dispatched agent's
subgraph_node so it can enter its resume path and cascade down to find
the existing SUSPENDED child run. For all other suspend sources (planner,
critic, phase_reactor), continue stripping the key as before so newly
dispatched agents don't accidentally enter resume paths against
non-existent child sessions.

Captures the resuming_agent_suspend flag immutably at the top of execute
so the dispatch loop can read it after any clear_suspend_meta calls."
```

---

## Task 5 — Add `"agent"` arm in resume-detection block

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/orchestrator.rs:803-855`

- [ ] **Step 1: Locate the resume-detection block**

```bash
grep -n "if sa == KEY_CRITIC\|if sa == KEY_PLANNER" src/libs/colmena/src/dag_engine/infrastructure/nodes/orchestrator.rs
```

Expected: lines around 806 (KEY_CRITIC) and 817 (KEY_PLANNER). The block is a chain of `if sa == ... { ... } else if sa == ... { ... }`.

Read the full block (~50 lines starting from the `if let (Some(ref ans), Some((ref sa...` opener at line 803).

- [ ] **Step 2: Add the `else if sa == "agent"` arm**

After the closing `}` of the KEY_PLANNER arm (around line 855) and BEFORE the closing `}` of the outer `if let`, add:

```rust
            } else if sa == "agent" {
                colmena_log!(
                    "▶️  [OrchestratorNode] Resuming from agent suspend. Re-dispatching pending tasks; \
                     the resume_answer will flow to the suspended agent via task_inputs (preserved \
                     because resuming_agent_suspend=true)."
                );
                clear_suspend_meta(_state);
                // No injection here. The answer rides on __colmena_resume_answer
                // through task_inputs (Task 4) into subgraph_node.execute, which
                // detects it and enters its resume path → find_suspended_child →
                // cascade down to the actual SuspendNode at the leaf.
```

(The closing `}` of this branch is the same as the existing `}` that closes the `if let` — just add the new arm before that final `}`.)

The structure before:
```rust
if let (Some(ref ans), Some((ref sa, _, _, ref questions))) = ... {
    if sa == KEY_CRITIC { ... }
    else if sa == KEY_PLANNER { ... }
}
```

The structure after:
```rust
if let (Some(ref ans), Some((ref sa, _, _, ref questions))) = ... {
    if sa == KEY_CRITIC { ... }
    else if sa == KEY_PLANNER { ... }
    else if sa == "agent" {
        colmena_log!("...");
        clear_suspend_meta(_state);
    }
}
```

- [ ] **Step 3: Compile**

Run: `cargo build`
Expected: success.

- [ ] **Step 4: End-to-end smoke — resume now cascades**

Re-use the existing test graph:

```bash
source .env

# Clean any leftover state.
psql "$DATABASE_URL" -c "DELETE FROM dag_runs WHERE agent_session_id = 'smoke_t5_resume';"

# Run 1: should suspend with 3 SUSPENDED rows.
cargo run --bin dag_engine -- run tests/graphs/advanced/nested_orchestrators_suspend.json --agent-session-id smoke_t5_resume
psql "$DATABASE_URL" -c "SELECT LEFT(session_id::text, 8), LEFT(parent_session_id::text, 8) AS parent, status FROM dag_runs WHERE agent_session_id = 'smoke_t5_resume' ORDER BY created_at;"
# Expected: 3 rows all SUSPENDED.

# Run 2: resume by chat handle only.
cargo run --bin dag_engine -- run tests/graphs/advanced/nested_orchestrators_suspend.json --agent-session-id smoke_t5_resume --answer "Yes, Tuesday at 10am works."
psql "$DATABASE_URL" -c "SELECT LEFT(session_id::text, 8), status FROM dag_runs WHERE agent_session_id = 'smoke_t5_resume' ORDER BY created_at;"
# Expected: 3 rows all COMPLETED.
```

The Run 2 output's `final_response` from the outer orchestrator should reflect the actual confirmation (not the previous hallucination "user confirmed" with no real input).

Cleanup:
```bash
psql "$DATABASE_URL" -c "DELETE FROM dag_runs WHERE agent_session_id = 'smoke_t5_resume';"
```

If Run 2 leaves any row as SUSPENDED or fails with `"No suspended child found"`, the cascade is broken — investigate before committing. The most likely culprits are: `resuming_agent_suspend` flag not capturing correctly (Task 4), or the `"agent"` arm not actually executing (verify the new branch is reached via a debug log).

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/orchestrator.rs
git commit -m "feat(orchestrator): resume detection arm for agent-suspend

Adds the 'agent' branch alongside the existing critic/planner/reactor
branches. On resume, when suspend_meta.suspended_at == 'agent', the
orchestrator clears its own suspend metadata and falls through to the
normal task loop. The dispatch loop (with resuming_agent_suspend=true)
preserves __colmena_resume_answer in task_inputs, so subgraph_node
enters its resume path and the answer cascades down to the SUSPENDED
child run via find_suspended_child.

This completes the agent-suspend propagation: orchestrator suspends on
agent suspend, and resume cascades the answer end-to-end across
arbitrary depth of orchestrator nesting."
```

---

## Task 6 — Integration test for single-agent suspend + resume

**Files:**
- Create: `src/libs/colmena/tests/orchestrator_agent_suspend.rs`

- [ ] **Step 1: Write the test file**

```rust
//! Integration tests for orchestrator agent-suspend propagation
//! (spec docs/superpowers/specs/2026-04-29-orchestrator-agent-suspend-design.md).
//!
//! Requires DATABASE_URL and GEMINI_API_KEY (the orchestrator's planner / reactor
//! components are LLM nodes). Each test cleans up its own dag_runs rows.

use colmena::dag_engine::domain::graph::Graph;
use colmena::dag_engine::engine::{ColmenaEngine, EngineConfig};
use futures::StreamExt;
use serde_json::json;

async fn engine() -> ColmenaEngine {
    dotenvy::dotenv().ok();
    let cfg = EngineConfig::from_env().unwrap();
    ColmenaEngine::new(cfg).await.unwrap()
}

async fn cleanup(chat: &str) {
    dotenvy::dotenv().ok();
    let url = std::env::var("DATABASE_URL").unwrap();
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    sqlx::query("DELETE FROM dag_runs WHERE agent_session_id = $1")
        .bind(chat)
        .execute(&pool)
        .await
        .ok();
}

/// A minimal orchestrator with one agent whose subgraph just suspends.
/// We use a free-form Gemini planner because the orchestrator requires one,
/// but the only "real" work happens in the suspend node inside the agent.
fn single_agent_suspend_graph() -> Graph {
    let raw = json!({
        "nodes": {
            "trigger": {
                "type": "input",
                "config": { "prompt": "Ask the user for confirmation." }
            },
            "orch": {
                "type": "orchestrator",
                "config": {
                    "max_phases": 3,
                    "verbose": false,
                    "include_extra_info": false,
                    "planner": {
                        "provider": "gemini",
                        "model": "gemini-2.5-flash",
                        "api_key": "${GEMINI_API_KEY}",
                        "system_message": "Break the request into exactly ONE task assigned to 'asker'."
                    },
                    "agents": {
                        "asker": {
                            "description": "Asks the user one yes/no question.",
                            "child_graph_inline": {
                                "nodes": {
                                    "ask_in": { "type": "input", "config": {} },
                                    "ask": {
                                        "type": "suspend",
                                        "config": {
                                            "id": "confirm",
                                            "question": "Do you confirm?",
                                            "question_type": "choice",
                                            "options": ["yes", "no"]
                                        }
                                    },
                                    "ask_out": { "type": "output", "config": {} }
                                },
                                "edges": [
                                    { "from": "ask_in", "to": "ask" },
                                    { "from": "ask", "to": "ask_out" }
                                ]
                            }
                        }
                    },
                    "phase_reactor": {
                        "provider": "gemini",
                        "model": "gemini-2.5-flash",
                        "api_key": "${GEMINI_API_KEY}",
                        "system_message": "Summarize phase. Set task_ok=true."
                    },
                    "final_reactor": {
                        "provider": "gemini",
                        "model": "gemini-2.5-flash",
                        "api_key": "${GEMINI_API_KEY}",
                        "system_message": "Reply with what the user confirmed."
                    }
                }
            }
        },
        "edges": [
            { "from": "trigger", "to": "orch" }
        ]
    });
    serde_json::from_value(raw).unwrap()
}

#[tokio::test]
async fn orchestrator_propagates_agent_suspend() {
    let chat = "test_orch_agent_suspend";
    cleanup(chat).await;

    let eng = engine().await;
    let mut s = Box::pin(eng.execute_stream(
        single_agent_suspend_graph(),
        None,
        None,
        false,
        None,
        Some(chat.into()),
    ));
    let mut saw_suspended = false;
    while let Some(item) = s.next().await {
        let ev = item.expect("event");
        let raw = serde_json::to_value(&ev).unwrap();
        if raw.get("type").and_then(|v| v.as_str()) == Some("finish") {
            if raw.get("finishReason").and_then(|v| v.as_str()) == Some("suspended") {
                saw_suspended = true;
            }
        }
    }
    drop(s);
    assert!(saw_suspended, "orchestrator should bubble up SUSPENDED finish event");

    // Verify dag_runs: the orchestrator's root run AND the asker subgraph run
    // should both be SUSPENDED.
    let url = std::env::var("DATABASE_URL").unwrap();
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    let suspended_count: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM dag_runs WHERE agent_session_id = $1 AND status = 'SUSPENDED'"
    )
    .bind(chat)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        suspended_count.0 >= 2,
        "expected at least 2 SUSPENDED rows (orchestrator root + asker subgraph), got {}",
        suspended_count.0
    );

    cleanup(chat).await;
    eng.shutdown().await;
}

#[tokio::test]
async fn orchestrator_resumes_agent_suspend_end_to_end() {
    let chat = "test_orch_resume_e2e";
    cleanup(chat).await;

    let eng = engine().await;

    // Run 1: trigger the suspend.
    let mut s1 = Box::pin(eng.execute_stream(
        single_agent_suspend_graph(),
        None,
        None,
        false,
        None,
        Some(chat.into()),
    ));
    while s1.next().await.is_some() {}
    drop(s1);

    // Run 2: resume by agent_session_id only with an answer.
    let mut s2 = Box::pin(eng.execute_stream(
        single_agent_suspend_graph(),
        None,
        Some("yes".into()),
        false,
        None,
        Some(chat.into()),
    ));
    while s2.next().await.is_some() {}
    drop(s2);

    // Verify all rows are now COMPLETED.
    let url = std::env::var("DATABASE_URL").unwrap();
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    let still_suspended: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM dag_runs WHERE agent_session_id = $1 AND status = 'SUSPENDED'"
    )
    .bind(chat)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        still_suspended.0, 0,
        "after resume, no rows should remain SUSPENDED for this chat"
    );

    let completed: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM dag_runs WHERE agent_session_id = $1 AND status = 'COMPLETED'"
    )
    .bind(chat)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(completed.0 >= 2, "expected COMPLETED rows after resume");

    cleanup(chat).await;
    eng.shutdown().await;
}
```

- [ ] **Step 2: Run the tests**

```bash
source .env
cargo test --test orchestrator_agent_suspend -- --test-threads=1
```

Expected: 2 tests pass. They take ~30-60s each because of LLM calls.

If the second test fails with rows still SUSPENDED, double-check Tasks 4 and 5 — the issue is likely that `__colmena_resume_answer` isn't reaching the agent's subgraph_node.

- [ ] **Step 3: Commit**

```bash
git add src/libs/colmena/tests/orchestrator_agent_suspend.rs
git commit -m "test(orchestrator): integration tests for agent-suspend + resume

Two end-to-end tests using a minimal orchestrator with one agent whose
subgraph contains a suspend node:

- orchestrator_propagates_agent_suspend: verifies the orchestrator emits
  finishReason='suspended' and at least 2 SUSPENDED rows in dag_runs
  (root + asker subgraph) when the agent suspends.
- orchestrator_resumes_agent_suspend_end_to_end: runs the suspend, then
  resumes with an answer using only --agent-session-id, and verifies all
  rows become COMPLETED.

Both clean up their own dag_runs rows so they're safe to re-run."
```

---

## Task 7 — E2E test using existing nested-orchestrators graph

**Files:**
- Modify: `src/libs/colmena/tests/orchestrator_agent_suspend.rs` (append)

- [ ] **Step 1: Append the test**

Add this test to the end of the file:

```rust
/// E2E with the pre-existing fixture at tests/graphs/advanced/nested_orchestrators_suspend.json.
/// That graph has 3 levels: outer_orch → team_leader subgraph → leader_orch → confirm_specialist
/// subgraph → ask_user (suspend). Tests that the resume cascade unwinds 3 levels in one
/// invocation with just --agent-session-id and --answer.
#[tokio::test]
async fn nested_orchestrators_suspend_cascades_3_levels() {
    let chat = "test_nested_3_levels";
    cleanup(chat).await;

    let raw = tokio::fs::read_to_string("tests/graphs/advanced/nested_orchestrators_suspend.json")
        .await
        .expect("graph file must exist");
    let graph: Graph = serde_json::from_str(&raw).expect("parse graph");

    let eng = engine().await;

    // Run 1: should suspend with 3 SUSPENDED rows.
    let mut s1 = Box::pin(eng.execute_stream(
        graph.clone(),
        None,
        None,
        false,
        None,
        Some(chat.into()),
    ));
    while s1.next().await.is_some() {}
    drop(s1);

    let url = std::env::var("DATABASE_URL").unwrap();
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    let suspended_count: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM dag_runs WHERE agent_session_id = $1 AND status = 'SUSPENDED'"
    )
    .bind(chat)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(suspended_count.0, 3, "expected 3 SUSPENDED rows after run 1 (root + 2 subgraphs)");

    // Run 2: resume.
    let mut s2 = Box::pin(eng.execute_stream(
        graph,
        None,
        Some("Yes, Tuesday at 10am works for me.".into()),
        false,
        None,
        Some(chat.into()),
    ));
    while s2.next().await.is_some() {}
    drop(s2);

    // All 3 rows should now be COMPLETED.
    let still_suspended: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM dag_runs WHERE agent_session_id = $1 AND status = 'SUSPENDED'"
    )
    .bind(chat)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(still_suspended.0, 0, "all rows should be COMPLETED after resume");

    let total: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM dag_runs WHERE agent_session_id = $1"
    )
    .bind(chat)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(total.0 >= 3, "expected at least 3 dag_runs rows in the chat tree");

    cleanup(chat).await;
    eng.shutdown().await;
}
```

- [ ] **Step 2: Run all integration tests**

```bash
source .env
cargo test --test orchestrator_agent_suspend -- --test-threads=1
```

Expected: 3 tests pass.

- [ ] **Step 3: Run the full lib + integration suite to confirm no regressions**

```bash
cargo test --lib --package colmena_dag_engine
cargo test --test agent_session_id_lifecycle -- --test-threads=1
cargo test --test find_resume_entry -- --test-threads=1
```

All should pass.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/tests/orchestrator_agent_suspend.rs
git commit -m "test(orchestrator): e2e cascade test for 3-level nested orchestrators

Uses the pre-existing fixture
tests/graphs/advanced/nested_orchestrators_suspend.json (outer_orch →
team_leader subgraph → leader_orch → confirm_specialist subgraph →
ask_user suspend node). After Run 1 there are 3 SUSPENDED rows; after
Run 2 (resume with only --agent-session-id and --answer), all 3 are
COMPLETED. Confirms the resume cascade unwinds end-to-end through
arbitrary nesting in a single user invocation."
```

---

## Final verification

- [ ] **Run the full test suite**

```bash
source .env
cargo build
cargo test --lib --package colmena_dag_engine
cargo test --test agent_session_id_lifecycle -- --test-threads=1
cargo test --test find_resume_entry -- --test-threads=1
cargo test --test orchestrator_agent_suspend -- --test-threads=1
cargo test --test graph_validation
```

All should pass.

- [ ] **Smoke test the manual reproducer one last time**

```bash
source .env
psql "$DATABASE_URL" -c "DELETE FROM dag_runs WHERE agent_session_id = 'final_smoke';"

# Run 1: suspend
cargo run --bin dag_engine -- run tests/graphs/advanced/nested_orchestrators_suspend.json --agent-session-id final_smoke
psql "$DATABASE_URL" -c "SELECT LEFT(session_id::text, 8), LEFT(parent_session_id::text, 8) AS parent, status FROM dag_runs WHERE agent_session_id = 'final_smoke' ORDER BY created_at;"

# Run 2: resume
cargo run --bin dag_engine -- run tests/graphs/advanced/nested_orchestrators_suspend.json --agent-session-id final_smoke --answer "Yes, Tuesday at 10am works."
psql "$DATABASE_URL" -c "SELECT LEFT(session_id::text, 8), LEFT(parent_session_id::text, 8) AS parent, status FROM dag_runs WHERE agent_session_id = 'final_smoke' ORDER BY created_at;"

psql "$DATABASE_URL" -c "DELETE FROM dag_runs WHERE agent_session_id = 'final_smoke';"
```

After Run 2, all 3 rows should be COMPLETED. The output of Run 2 should include a `final_response` from the outer orchestrator that reflects the actual user answer (not a hallucination).

---

## Self-review against spec

| Spec section | Plan task |
|---|---|
| §4 SuspendNode extension | Task 1 |
| §5.1 Detect SUSPENDED in dispatch | Task 3 (with helper from Task 2) |
| §5.2 propagate_agent_suspend helper | Task 2 |
| §5.3 Parallel agents (single-suspend invariant) | Task 3 (early-return at first suspend; subsequent tasks in `tasks_to_run` never get dispatched in serial-loop pattern, naturally satisfying the invariant) |
| §6 Resume detection arm | Task 5 |
| §7 Preserve `__colmena_resume_answer` | Task 4 |
| §8 Edge cases | Task 3 (`allow_suspend` warning); Task 3 (the existing `tasks_to_run` serial loop naturally handles "first suspended wins") |
| §9 Backward compatibility | Implicit: changes are additive (Task 1 adds fields; Task 3 only fires on SUSPENDED status; Task 4 only changes behavior when `resuming_agent_suspend=true`) |
| §10 Test plan: unit | Task 1 (6 inline unit tests) |
| §10 Test plan: integration | Task 6 (2 tests: single-agent suspend + resume) |
| §10 Test plan: e2e | Task 7 (3-level cascade) |

> **Note on parallel agents**: the spec described `futures::join_all` semantics. The actual orchestrator code uses a serial `for task in tasks_to_run` loop, which makes the "first suspend wins, subsequent never run" behavior emerge for free without explicit `join_all`/post-processing. The plan reflects this simpler reality. Multi-suspend in parallel can therefore only happen if a future code change introduces concurrent dispatch — at which point the spec's defensive error path could be re-added.
