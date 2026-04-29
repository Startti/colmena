# Orchestrator Agent-Suspend Propagation — Design

**Status**: Draft
**Date**: 2026-04-29
**Author**: Daniel Garcia (brainstormed with Claude)

## 1. Problem statement

The `orchestrator` node already supports HITL (Human-in-the-Loop) for its **own** internal components: `planner`, `critic`, `phase_reactor`, and `final_reactor` all use `make_suspend_response(...)` to pause the run with `{ "__colmena_status": "SUSPENDED", questions: [...] }`.

However, when the orchestrator dispatches a **task to an agent** (which is invoked via `subgraph_node.execute(...)` at `orchestrator.rs:1487-1489`) and that agent's child graph internally suspends (e.g. it contains a `suspend` node), the orchestrator **silently ignores** the SUSPENDED status:

- The agent's child run is correctly persisted as SUSPENDED in `dag_runs`.
- But the orchestrator treats `agent_result` as a regular completed value, runs the critic against it (which evaluates a meaningless null/incomplete payload), runs the phase reactor, runs the final reactor, and **hallucinates** a final response based on no real agent output.

Reproduced live via `tests/graphs/advanced/nested_orchestrators_suspend.json` (a 3-level orchestrator nesting where the deepest agent contains a `suspend` node):

```
After Run 1 (chat_session = chat_nested_susp):
  root_run                          COMPLETED   ← outer orchestrator finished, hallucinated
  team_leader subgraph              COMPLETED   ← inner orchestrator finished, hallucinated
  confirm_specialist subgraph       SUSPENDED   ← orphan; no parent is waiting
```

The orphan SUSPENDED row can never be resumed cleanly because both ancestors are COMPLETED and consider the chat finished.

This is the only meaningful HITL gap left in Colmena's orchestrator after the `agent_session_id` feature. Closing it lets users build chat-style HITL flows that include orchestrator-mediated agent dispatch (e.g. "the planner sub-agent will ask you to confirm the booking date").

## 2. Goals and non-goals

### Goals

- When an agent's `subgraph_node.execute(...)` returns `__colmena_status: SUSPENDED`, the orchestrator must propagate that status as its own output and persist its own run as SUSPENDED.
- The orchestrator's task-tracking semantics must remain consistent: the suspended task stays `completed=false` in `dag_task_memory`; on resume, it is re-dispatched.
- The resume path (using `find_resume_entry` from the prior `agent_session_id` feature) must cascade end-to-end through arbitrary depth of orchestrator nesting in a single user invocation.
- The `SuspendNode` must be extended to fully describe its question (id, type, options) so the orchestrator-level wrapping carries faithful semantics to the external client (UI).
- Backward compatibility: graphs that don't contain orchestrator+suspend nesting must behave identically.

### Non-goals

- Multiple agents suspending **concurrently** in the same phase. Single-leaf assumption from the original `agent_session_id` design carries through. Defensive error if it happens.
- Cancellation of in-flight parallel agents when one suspends. We **wait for all** to complete (or suspend), then post-process. Less surgical, but simpler and avoids partial state.
- Changes to the resume mechanism for `--session-id` legacy callers (it already handles agent-suspend correctly through the existing subgraph_node resume path once propagation works).
- Cross-orchestrator routing (e.g., one orchestrator's suspend triggering another's). Out of scope.

## 3. Architecture

The change has three coordinated parts, each in a different file:

```
┌─────────────────────────────────────┐
│ suspend.rs (SuspendNode)            │  Section A — emit canonical shape
│   config: { id?, question_type?,    │
│             options? }              │
│   output: { question, questions }   │
└──────────────┬──────────────────────┘
               │ (agent's child run produces this output)
               ▼
┌─────────────────────────────────────┐
│ subgraph.rs (SubGraphNode)          │  (no changes — bubble-up already correct)
│   bubbles up __colmena_status:      │
│   SUSPENDED to its parent           │
└──────────────┬──────────────────────┘
               │
               ▼
┌─────────────────────────────────────┐
│ orchestrator.rs (OrchestratorNode)  │  Sections B, C, D — main change
│   1. Detect SUSPENDED in            │
│      agent_result (post-dispatch)   │
│   2. Convert questions array        │
│   3. make_suspend_response("agent") │
│   4. Resume detection on re-entry   │
│   5. Preserve resume_answer         │
│      in task_inputs                 │
└─────────────────────────────────────┘
```

## 4. Section A — `SuspendNode` extension

Extend `src/libs/colmena/src/dag_engine/infrastructure/nodes/suspend.rs`.

### 4.1 New optional config fields

```json
{
  "type": "suspend",
  "config": {
    "id": "confirm_transfer",          // optional; default: local node_id
    "question": "Confirm transfer?",
    "question_type": "choice",         // optional; default: "open"
    "options": ["yes", "no", "cancel"] // optional; only meaningful when type="choice"
  }
}
```

### 4.2 Output shape (canonical + legacy)

When suspending, the node emits BOTH the new canonical `questions` array AND the legacy single-string fields, so external clients reading either work:

```json
{
  "__colmena_status": "SUSPENDED",
  "question": "Confirm transfer?",
  "questions": [
    {
      "id": "confirm_transfer",
      "question": "Confirm transfer?",
      "type": "choice",
      "options": ["yes", "no", "cancel"]
    }
  ]
}
```

When the user provides only `config.question` (current legacy graphs), defaults kick in:

```json
{
  "__colmena_status": "SUSPENDED",
  "question": "Confirm?",
  "questions": [
    { "id": "<local-node_id>", "question": "Confirm?", "type": "open", "options": null }
  ]
}
```

### 4.3 Default `id` resolution

When `config.id` is absent, default to the **local `node_id`** of the suspend node within its own graph. This is the suspend node's id as declared in `nodes: { "<id>": { "type": "suspend", ... } }`.

The local node_id is available to the SuspendNode through engine-injected `__node_id` in its `inputs` (already present today).

```rust
let id = config
    .get("id")
    .and_then(|v| v.as_str())
    .map(|s| s.to_string())
    .or_else(|| inputs.get("__node_id").and_then(|v| v.as_str()).map(|s| s.to_string()))
    .unwrap_or_else(|| "suspend".to_string());
```

### 4.4 Resume answer (no change to existing logic)

When `__colmena_resume_answer` is present in inputs, the SuspendNode emits `{status: "resumed", answer_received: <answer>}` exactly as today. No change needed.

## 5. Section B — Detect agent suspend in orchestrator dispatch

Modify `orchestrator.rs:1487-1489` (the agent dispatch site).

### 5.1 Wrap the dispatch in a result classification

```rust
let agent_result = subgraph_node
    .execute(&task_inputs, &subgraph_cfg, _state, _observer.clone())
    .await?;

// NEW: detect suspended agent before any further processing.
if agent_result.get("__colmena_status").and_then(|v| v.as_str()) == Some("SUSPENDED") {
    // Skip critic, skip phase reactor, skip update_task_result.
    // Just propagate the suspend upward.
    return propagate_agent_suspend(
        agent_result,
        &task,
        phase,
        _state,
    );
}

// (Existing: critic, retries, mark completed, etc.)
```

### 5.2 The `propagate_agent_suspend` helper

Builds a `Vec<SuspendQuestion>` from whatever the agent emitted, then delegates to existing `make_suspend_response`:

```rust
fn propagate_agent_suspend(
    agent_result: Value,
    task: &DagTask,
    phase: i32,
    state: &mut Value,
) -> Result<Value, Box<dyn Error + Send + Sync>> {
    // Prefer canonical `questions` array (handles nested orchestrators that
    // already produced canonical output, AND new SuspendNode output).
    // Fall back to legacy `question` string.
    let questions: Vec<SuspendQuestion> = if let Some(qs_val) = agent_result.get("questions") {
        serde_json::from_value(qs_val.clone()).unwrap_or_default()
    } else if let Some(q_str) = agent_result.get("question").and_then(|v| v.as_str()) {
        vec![SuspendQuestion {
            id: format!("agent_{}", task.assigned_to),
            question: q_str.to_string(),
            question_type: "open".to_string(),
            options: None,
        }]
    } else {
        // Degenerate: agent suspended but emitted no questions metadata.
        vec![SuspendQuestion {
            id: format!("agent_{}", task.assigned_to),
            question: format!("Agent '{}' requires user input.", task.assigned_to),
            question_type: "open".to_string(),
            options: None,
        }]
    };

    Ok(make_suspend_response(state, "agent", phase, Some(&task.id), questions))
}
```

The `suspended_at` value is the new sentinel `"agent"`.

### 5.3 Parallel agents (per `parallel: true`)

When the orchestrator dispatches multiple agents in parallel within one phase, the existing pattern uses `futures::join_all` (or equivalent). Adapt the post-dispatch processing:

```rust
let agent_results: Vec<(DagTask, Value)> = futures::future::join_all(handles).await
    .into_iter()
    .map(|(task, res)| res.map(|v| (task, v)))
    .collect::<Result<Vec<_>, _>>()?;

let suspended: Vec<&(DagTask, Value)> = agent_results.iter()
    .filter(|(_, r)| r.get("__colmena_status").and_then(|v| v.as_str()) == Some("SUSPENDED"))
    .collect();

match suspended.len() {
    0 => {
        // Normal flow: critic, phase reactor, mark completed=true for each task.
    }
    1 => {
        // For agents that completed without suspending: still mark them completed=true
        // (don't lose their work). Skip critic for them too — keep critic decisions
        // for after the user resumes.
        for (t, r) in &agent_results {
            if r.get("__colmena_status").and_then(|v| v.as_str()) != Some("SUSPENDED") {
                repo.update_task_result(&t.id, r.clone()).await?;
            }
        }
        let (susp_task, susp_result) = suspended[0];
        return propagate_agent_suspend(susp_result.clone(), susp_task, phase, _state);
    }
    n => {
        return Err(format!(
            "Multiple agents ({}) suspended in the same phase. Single-leaf design only \
             supports one suspend at a time. Consider setting parallel=false on agents \
             that may suspend.",
            n
        ).into());
    }
}
```

The semantic decision documented here: **agent-suspend wins** over critic-suspend or any other deferred decision. If the critic would have wanted to suspend (e.g. max_retries hit on one of the completed agents), we **defer** it: leave that agent's task `completed=false` so it gets re-dispatched on resume; the critic will re-evaluate then. The agent-suspend question is what reaches the user.

## 6. Section C — Resume detection

Modify `orchestrator.rs:803-855` (the resume detection block at the top of `execute`).

### 6.1 Capture the resume mode early

```rust
let resume_answer: Option<String> = inputs
    .get("__colmena_resume_answer")
    .and_then(|v| v.as_str())
    .map(|s| s.to_string());

let suspend_meta = read_suspend_meta(_state);

// NEW: capture this once, immutably, BEFORE any clearing of suspend_meta.
let resuming_agent_suspend: bool = matches!(
    (&resume_answer, &suspend_meta),
    (Some(_), Some((sa, _, _, _))) if sa == "agent"
);
```

### 6.2 Add the `"agent"` arm

In the `if let (Some(ref ans), Some((ref sa, _, _, ref questions))) = ...` block, add a new arm after `KEY_PLANNER`:

```rust
else if sa == "agent" {
    colmena_log!(
        "▶️  [OrchestratorNode] Resuming from agent suspend. \
         Re-dispatching pending tasks; the resume_answer will flow to the suspended agent."
    );
    clear_suspend_meta(_state);
    // No injection: the answer flows through __colmena_resume_answer in task_inputs
    // (preserved by the change in Section D). The task loop below will iterate
    // incomplete tasks and re-dispatch; the subgraph_node will detect the answer
    // and enter its resume path, cascading down to the SUSPENDED leaf.
}
```

### 6.3 Cascade behavior (verification, not new code)

The cascade through nested orchestrators relies on existing pieces:

1. `find_resume_entry` (from prior PR) returns the **topmost** SUSPENDED ancestor.
2. The orchestrator's task loop re-dispatches the task whose `completed=false`.
3. `subgraph_node.execute` sees `__colmena_resume_answer` in its inputs (preserved by Section D), enters its resume path, calls `find_suspended_child(parent_session_id, ...)` to find the immediate SUSPENDED child run, and calls `executor.resume_subgraph(...)` recursively.
4. Each level of nested orchestrator repeats steps 2-3 until reaching the actual SuspendNode.
5. The SuspendNode receives the answer, completes, results unwind back up through every level.

End state after one user invocation: every node in the chain `COMPLETED`.

## 7. Section D — Preserve `__colmena_resume_answer` in `task_inputs`

Modify `orchestrator.rs:1399-1403`.

### 7.1 The current strip is too aggressive

```rust
let mut task_inputs = inputs.clone();
// Strip __colmena_resume_answer so subgraph agents being executed
// for the first time don't mistakenly try to resume a non-existent
// child session. The orchestrator already consumed this key above.
task_inputs.remove("__colmena_resume_answer");
```

This is correct for the **existing** suspend types (`planner`, `critic`, `phase_reactor`) where the orchestrator consumes the answer at its own level (e.g., injects the Q&A into the planner's system_message). After that, agents fired for the first time should not see the stale answer.

For agent-suspend, however, the answer is FOR the SuspendNode at the leaf — it must flow through.

### 7.2 The fix

Use the `resuming_agent_suspend` flag from Section 6.1:

```rust
let mut task_inputs = inputs.clone();
if !resuming_agent_suspend {
    // Legacy behavior: strip the answer so agents being newly dispatched don't
    // accidentally enter their resume path.
    task_inputs.remove("__colmena_resume_answer");
}
// (When resuming_agent_suspend is true, the answer is preserved so the
// subgraph_node's resume path can detect it and cascade to find the existing
// SUSPENDED child run.)

task_inputs.insert("__node_id".to_string(), Value::String(task.assigned_to.clone()));
// (rest unchanged)
```

## 8. Edge cases

### 8.1 `allow_suspend: false` on an agent

The existing `allow_suspend_for(component_cfg)` flag was designed for orchestrator-internal LLM components (critic / planner / reactor) — it lets the user suppress the suspend and continue, debug-printing the would-have-been questions.

For agent-suspend, this flag has different mechanics: **the agent's subgraph already suspended**; its `dag_runs` row is SUSPENDED. We cannot "ignore" the suspend without leaving an orphan row.

**Decision**: ignore `allow_suspend: false` for agents. Always propagate the suspend. Log a warning so the user knows the flag had no effect:

```rust
if !allow_suspend_for(agent_config) {
    colmena_log!(
        "⚠️  [OrchestratorNode] Agent '{}' has allow_suspend=false, but its subgraph \
         already suspended. The flag has no safe effect for agents — propagating suspend.",
        task.assigned_to
    );
}
return propagate_agent_suspend(...);
```

### 8.2 Nested orchestrator inside an agent (Test 6b case)

The inner orchestrator's `make_suspend_response` already returns `{__colmena_status: SUSPENDED, questions: [...]}`. The outer orchestrator's agent-suspend detection (Section 5.1) reads the canonical `questions` field and forwards it as-is. No transformation; ids/types/options from the deepest level reach the external client untouched.

This works for arbitrary depth.

### 8.3 Critic interaction when agent-suspend wins

Per Section 5.3: when any agent in a parallel phase returns SUSPENDED, the orchestrator **skips the critic entirely** for that phase. Non-suspended agents' results are saved with `completed=true` directly, without critic evaluation.

This is a deliberate simplification. Pre-existing behavior (no agent-suspend in the phase) is unchanged: each agent's result still passes through the critic normally.

Implication: in a phase where one agent suspends, the others' outputs are accepted as-is even if the critic would have flagged them. On resume, the orchestrator does NOT re-evaluate the already-completed agents. The next phase (if any) proceeds with the accepted results.

This trades off some critical-evaluation rigor for behavioral clarity: when there's a user-facing suspend, the orchestrator commits to "everything that finished is accepted; everything that suspended will resume". The user only sees the suspend question, not a critic question competing with it.

If users need the critic to re-evaluate non-suspended results after a resume, the workaround is to set `parallel: false` on the suspending agent so the critic runs serially per task. This is also the recommended structure when agent-suspend is expected.

### 8.4 Multiple agents suspending concurrently

Section 5.3 errors out: `Multiple agents (n) suspended in the same phase...`. This violates the single-leaf invariant the rest of the feature relies on.

If users hit this, the fix on their side is to set `parallel: false` on the suspending agent or restructure the phase. Documented.

## 9. Backward compatibility

| Scenario | Before | After | Breaking? |
|---|---|---|---|
| Graph without SuspendNode | Identical | Identical | No |
| Graph with SuspendNode, only `config.question` set | Output: `{question: str}` | Output: `{question: str, questions: [{type:"open", id:<local node_id>}]}` | No (`question` field preserved; `questions` is additive) |
| Orchestrator + agent that internally suspends | **BUG**: orphan SUSPENDED row, hallucinated final response | **FIX**: orchestrator suspends correctly, fully resumable | Soft-breaking (was a bug) |
| Orchestrator with critic/planner/reactor suspend | Identical | Identical | No (those branches untouched) |
| Resume by `--session-id` legacy | Identical | Identical | No |
| Resume by `--agent-session-id` for graphs with orchestrator-mediated agent-suspend | **BROKEN** | **WORKS** | Soft-breaking (was a bug) |

The only observable changes are bug fixes. No graph that previously worked correctly will behave differently. Document in release notes that orchestrator agent-suspend is now properly handled.

## 10. Test plan

Three layers:

### 10.1 Unit tests (`suspend.rs` inline `#[cfg(test)]`)

- `suspend_emits_open_when_no_config` — config with only `question`, output has `type: "open"`, `options: null`, id from `__node_id`.
- `suspend_emits_choice_with_options` — config with `question_type: "choice"` and `options: [...]`, output has `type: "choice"` and matching options array.
- `suspend_uses_local_node_id_as_default_id` — no `config.id`, id equals `__node_id` from inputs.
- `suspend_uses_explicit_id_when_set` — `config.id: "foo"`, id equals `"foo"`.
- `suspend_preserves_legacy_question_field` — output always emits `question: <string>` for back-compat alongside `questions: [...]`.

### 10.2 Integration tests (`tests/orchestrator_agent_suspend.rs`)

- `orchestrator_propagates_agent_suspend` — single-agent, agent suspends → orchestrator output has `__colmena_status: SUSPENDED`; `dag_runs` row for orchestrator is SUSPENDED; task in `dag_task_memory` is `completed=false`; `__orchestrator_suspend.suspended_at` is `"agent"`.
- `orchestrator_resumes_agent_suspend_end_to_end` — same setup, then resume with answer → all `dag_runs` rows COMPLETED; task `completed=true`; output reflects the actual resume answer (no hallucination).
- `orchestrator_with_parallel_agents_one_suspends` — 3 agents in parallel, one suspends → other two have `completed=true` with results saved; suspended one stays `completed=false`. Resume → only the suspended one is re-dispatched.
- `orchestrator_errors_on_multiple_concurrent_suspends` — 2 agents suspend → error `"Multiple agents (2) suspended..."`.

### 10.3 End-to-end with the existing graph

- `nested_orchestrators_suspend_cascades_3_levels` — uses `tests/graphs/advanced/nested_orchestrators_suspend.json` (already created during testing). Suspend → 3 SUSPENDED rows. Resume with `--agent-session-id` and an answer → all 3 COMPLETED. Final response cites the user's actual answer, not a hallucination.

Total: ~9 test cases across all 3 layers.

## 11. Open questions

None at this point.

## 12. Future work (out of scope)

- Allow multiple concurrent suspended agents within a phase (multi-leaf chat trees). Would require redesigning `find_resume_entry` and the `--answer` mechanism to disambiguate which question is being answered.
- Cancellation of in-flight parallel agents when one suspends (instead of waiting). Useful when agents are slow/expensive and the suspend should propagate immediately.
- Cross-phase or cross-orchestrator suspend forwarding (e.g., capturing critic-suspend even when agent-suspend wins, then surfacing it after the agent resumes). Currently we just defer with `completed=false` and re-evaluate on resume.
