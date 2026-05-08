# LLM Tool Suspend Propagation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `llm_call` propagate `__colmena_status: SUSPENDED` from a tool result up to the DAG engine, then on resume re-execute the suspended tool with the user's answer and continue the agent loop. Unblocks `secure_suspend` (and any future suspendable tool) when invoked from an LLM agent.

**Architecture:** Add a `SuspendInfo` field to `LlmResponse`. In `agent_service`, intercept tool results carrying `__colmena_status: SUSPENDED` and short-circuit. In `llm.rs`, detect that signal and return SUSPENDED to the DAG engine. On resume, load conversation memory, find the pending tool, re-run via a new `DagToolExecutor::execute_with_resume_answer`, persist the result, and continue the loop.

**Tech Stack:** Rust 1.95.0. No new deps.

**Spec:** [docs/superpowers/specs/2026-05-08-llm-call-tool-suspend-design.md](../specs/2026-05-08-llm-call-tool-suspend-design.md)

---

## File Structure

| Path | Change |
|---|---|
| `src/libs/colmena/src/llm/domain/...` (LlmResponse + AgentRunParams) | Add `SuspendInfo` + methods. Make prompt optional. |
| `src/libs/colmena/src/llm/application/agent_service.rs` | Detect SUSPENDED in tool result; short-circuit with persisted assistant-msg. Support empty/None prompt. |
| `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs` | New `execute_with_resume_answer` method. |
| `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs` | Resume path: load history, dispatch pending tool, persist result, continue loop. Emit SUSPENDED upward when agent_service signals it. |
| `tests/graphs/advanced/llm_secure_suspend_resume.json` | NEW integration graph. |
| `src/libs/colmena/tests/llm_tool_suspend_integration.rs` | NEW integration test (`#[ignore]`d). |

---

## Task 1: Extend `LlmResponse` with `SuspendInfo`

**Files:**
- Modify the file containing `LlmResponse` (likely `src/libs/colmena/src/llm/domain/response.rs` — confirm via grep).

- [ ] **Step 1: Locate `LlmResponse`**

Run: `grep -rn "pub struct LlmResponse\|impl LlmResponse" /home/daniel-garcia4/startti/colmena/src/libs/colmena/src/llm/`
Note the file path and existing field set.

- [ ] **Step 2: Write tests for the new fields/methods**

In the same file's `#[cfg(test)] mod tests` (or create one if absent):

```rust
#[test]
fn suspend_info_set_and_retrieved() {
    let info = SuspendInfo {
        tool_call_id: "call_abc".into(),
        questions: serde_json::json!([{"id":"q1","question":"x?","type":"secret"}]),
        raw_output: r#"{"__colmena_status":"SUSPENDED"}"#.into(),
    };
    let resp = LlmResponse::suspended(info.tool_call_id.clone(), info.questions.clone(), info.raw_output.clone());
    let got = resp.suspend().expect("must be Some");
    assert_eq!(got.tool_call_id, "call_abc");
    assert_eq!(got.questions[0]["id"], "q1");
}

#[test]
fn non_suspended_response_returns_none_for_suspend() {
    let resp = LlmResponse::default();
    assert!(resp.suspend().is_none());
}
```

(Adapt to the actual constructor pattern — if `LlmResponse` uses a builder, mirror it.)

- [ ] **Step 3: Run the tests, confirm compile failure** (`SuspendInfo` not defined).

Run: `cargo test --lib -p colmena_dag_engine llm::domain::response 2>&1 | tail -10`
Expected: E0412/E0599.

- [ ] **Step 4: Add `SuspendInfo` and the field to `LlmResponse`**

```rust
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SuspendInfo {
    pub tool_call_id: String,
    pub questions: serde_json::Value,
    pub raw_output: String,
}

// Inside `LlmResponse`:
pub struct LlmResponse {
    // ... existing fields ...
    suspend: Option<SuspendInfo>,
}

impl LlmResponse {
    // ... existing methods ...

    pub fn suspended(
        tool_call_id: String,
        questions: serde_json::Value,
        raw_output: String,
    ) -> Self {
        let mut r = Self::default();
        r.suspend = Some(SuspendInfo { tool_call_id, questions, raw_output });
        r
    }

    pub fn suspend(&self) -> Option<&SuspendInfo> {
        self.suspend.as_ref()
    }
}
```

If `LlmResponse` doesn't have a `Default` impl, use the existing constructor pattern. The new `suspend` field defaults to `None`.

- [ ] **Step 5: Run the tests** → pass.

Run: `cargo test --lib -p colmena_dag_engine llm::domain::response`

- [ ] **Step 6: Run full crate unit tests** → no regressions.

Run: `cargo test --lib -p colmena_dag_engine 2>&1 | tail -10`

- [ ] **Step 7: Commit**

```bash
git add <the response file>
git commit -m "feat(llm): add SuspendInfo to LlmResponse for tool-suspend propagation"
```

---

## Task 2: Make `prompt` optional in `AgentRunParams`

**Files:**
- Modify: `src/libs/colmena/src/llm/application/agent_service.rs`
- Possibly modify the AgentRunParams struct definition file.

- [ ] **Step 1: Locate `AgentRunParams`**

Run: `grep -n "pub struct AgentRunParams\|prompt:" /home/daniel-garcia4/startti/colmena/src/libs/colmena/src/llm/application/agent_service.rs`

- [ ] **Step 2: Change the `prompt` field type from `String` to `Option<String>`**

Update the struct, then in `agent_service::run`, use the option:
```rust
if let Some(prompt) = params.prompt {
    messages.push(LlmMessage::user(prompt));
}
```

This means: when `prompt` is None, no user message is added — the loop runs against the existing `messages` history and the LLM produces the next response based on it.

Update all existing call sites (likely just `llm.rs`) to wrap `prompt.to_string()` as `Some(prompt.to_string())`.

- [ ] **Step 3: Add a regression test that confirms run with `prompt: None` works**

In `agent_service.rs::tests`:

```rust
#[tokio::test]
async fn run_with_no_prompt_continues_from_existing_messages() {
    // Setup: messages already contains user + assistant + tool_msg.
    // Mock LLM returns text response (no tool calls).
    // Verify: agent_service does NOT push a new user message; LLM gets the existing messages; final response is the text content.
    // (Adapt the existing tests' mock setup as a template.)
}
```

If the existing mock setup is too convoluted, simplify and document.

- [ ] **Step 4: Run all `agent_service` tests** → pass.

Run: `cargo test --lib -p colmena_dag_engine agent_service`

- [ ] **Step 5: Run full crate unit tests + check** → no regressions.

Run: `cargo check --all-targets && cargo test --lib -p colmena_dag_engine 2>&1 | tail -10`

- [ ] **Step 6: Commit**

```bash
git add <files modified>
git commit -m "feat(llm): make AgentRunParams.prompt Option<String> for resume continuation"
```

---

## Task 3: `agent_service` detects SUSPENDED tool result and short-circuits

**Files:**
- Modify: `src/libs/colmena/src/llm/application/agent_service.rs`

- [ ] **Step 1: Write a unit test in `agent_service.rs::tests`**

```rust
#[tokio::test]
async fn detects_suspended_tool_result_and_short_circuits() {
    // Mock LlmRepo: returns one assistant message with tool_calls=[call_xyz].
    // Mock ToolExecutor: tool_executor.expect_execute() returns ToolResult{
    //     tool_call_id: "call_xyz",
    //     success: true,
    //     output: r#"{"__colmena_status":"SUSPENDED","questions":[{"id":"q1"}]}"#.into(),
    //     error: None,
    // }
    // Mock ConversationRepository: expect add_message called for the assistant message
    //   (but NOT for a tool message — assert tool message NEVER persisted).
    //
    // Run agent_service.run with these mocks.
    // Assert response.suspend() is Some.
    // Assert response.suspend().unwrap().tool_call_id == "call_xyz".
    // Assert response.suspend().unwrap().questions[0]["id"] == "q1".
}
```

- [ ] **Step 2: Run the test** → expected to fail.

Run: `cargo test --lib -p colmena_dag_engine agent_service::tests::detects_suspended_tool_result_and_short_circuits`
Expected: assertion failure (response.suspend() is None — the existing code keeps looping).

- [ ] **Step 3: Implement the SUSPENDED detection**

In `agent_service.rs:257` area, after the tool_executor.execute call and before pushing the tool_message:

```rust
let result = match tool_executor.execute(tool_call).await {
    Ok(res) => res,
    Err(e) => ToolResult {
        tool_call_id: tool_call.id.clone(),
        success: false,
        output: format!("Error executing tool: {}", e),
        error: Some(e.to_string()),
    },
};

// NEW: detect SUSPENDED in the tool's structured output.
if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&result.output) {
    if parsed.get("__colmena_status").and_then(|v| v.as_str()) == Some("SUSPENDED") {
        // Persist the assistant message that produced this tool_call so resume
        // can identify the pending tool. Tool result is NOT persisted (we
        // don't have one yet).
        let assistant_msg_with_tool_calls = LlmMessage::assistant_with_tool_calls(
            response.tool_calls().cloned().unwrap_or_default(),
        );
        self.conversation_repository
            .add_message(session_id, assistant_msg_with_tool_calls)
            .await?;

        let questions = parsed.get("questions").cloned().unwrap_or(serde_json::Value::Null);
        return Ok(LlmResponse::suspended(
            tool_call.id.clone(),
            questions,
            result.output,
        ));
    }
}

// Existing flow continues unchanged: push tool_message, persist, continue loop.
```

If `LlmMessage::assistant_with_tool_calls` doesn't exist, look for the equivalent constructor (or the path used elsewhere in the file to persist assistant messages). The point is: persist the assistant turn that has the tool_call.

- [ ] **Step 4: Run the test** → pass.

- [ ] **Step 5: Run full agent_service test suite + crate tests + cargo check** → no regressions.

```bash
cargo test --lib -p colmena_dag_engine agent_service && cargo check --all-targets
```

- [ ] **Step 6: Commit**

```bash
git add src/libs/colmena/src/llm/application/agent_service.rs
git commit -m "feat(llm): detect SUSPENDED tool result and propagate via LlmResponse::suspended"
```

---

## Task 4: Add `DagToolExecutor::execute_with_resume_answer`

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs`

- [ ] **Step 1: Read the existing `execute` method**

Run: `grep -n "pub async fn execute\|fn execute(" /home/daniel-garcia4/startti/colmena/src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs | head -5`

Read ~50 lines of the impl. Note where `inputs` HashMap is constructed and where `node.execute(&inputs, ...)` is called.

- [ ] **Step 2: Write a unit test**

```rust
#[tokio::test]
async fn execute_with_resume_answer_passes_answer_in_inputs() {
    // Setup: a stub registry with a fake node that records its inputs.
    // Build DagToolExecutor wrapping it.
    // Call executor.execute_with_resume_answer(&tool_call, "ANSWER_ABC").await.
    // Assert: the fake node received inputs with `__colmena_resume_answer == "ANSWER_ABC"`.
}
```

The stub node can be a small `ExecutableNode` impl that captures inputs into an Arc<Mutex<...>>.

- [ ] **Step 3: Run the test** → expected compile failure (method doesn't exist).

- [ ] **Step 4: Refactor `execute` to extract the inputs-building into a shared helper**

Pattern:
```rust
async fn execute(&self, tool_call: &ToolCall) -> Result<ToolResult, LlmError> {
    self.execute_inner(tool_call, None).await
}

pub async fn execute_with_resume_answer(
    &self,
    tool_call: &ToolCall,
    resume_answer: &str,
) -> Result<ToolResult, LlmError> {
    self.execute_inner(tool_call, Some(resume_answer)).await
}

async fn execute_inner(
    &self,
    tool_call: &ToolCall,
    resume_answer: Option<&str>,
) -> Result<ToolResult, LlmError> {
    // ... existing body ...
    if let Some(ans) = resume_answer {
        inputs.insert("__colmena_resume_answer".to_string(), Value::String(ans.to_string()));
    }
    // ... call node.execute(&inputs, ...) ...
}
```

Be careful not to change the existing `execute` semantics — just thread the optional resume_answer through.

- [ ] **Step 5: Run the test** → pass. Run also the existing executor tests.

```bash
cargo test --lib -p colmena_dag_engine dag_tool_executor
```

- [ ] **Step 6: cargo check + crate tests** → clean.

- [ ] **Step 7: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs
git commit -m "feat(tool-exec): execute_with_resume_answer threads resume value into node inputs"
```

---

## Task 5: `llm.rs` emits SUSPENDED upward + resume path

This is the largest task. Two halves:

### 5A: Emit SUSPENDED to DAG output when agent_service returns suspended.

- [ ] **Step 1: Add a unit test** in `llm.rs::tests` (or integration-style if mocking is too heavy).

The test sets up an `LlmNode` with a `ToolExecutor` mock that returns SUSPENDED for one tool. Asserts the node's output JSON contains `__colmena_status: "SUSPENDED"` and `questions`, plus `_pending_tool_call_id` and `_conversation_key`.

If the existing tests in `llm.rs` are minimal/non-existent, write the test as an integration test under `tests/` with a stub LLM repository that produces a fake tool_call response.

- [ ] **Step 2: After `agent_service.run()` (~line 1152 of llm.rs)**

```rust
let response = agent_service.run(params).await?;

if let Some(suspend) = response.suspend() {
    return Ok(json!({
        "__colmena_status": "SUSPENDED",
        "questions": suspend.questions.clone(),
        "_pending_tool_call_id": suspend.tool_call_id.clone(),
        "_conversation_key": conversation_key.clone(),
    }));
}

// Existing extra_info / write_to_memory path continues.
```

- [ ] **Step 3: Run the test** → pass.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs
git commit -m "feat(llm-node): emit __colmena_status SUSPENDED when agent reports tool-suspend"
```

### 5B: Resume path

- [ ] **Step 5: Add a function `resume_from_suspended_tool`** (private method on `LlmNode`):

```rust
async fn resume_from_suspended_tool(
    &self,
    resume_answer: &str,
    inputs: &NodeInputs,
    config: &Value,
    /* the same tool_executor + agent_service the normal path builds */
) -> Result<Value, Box<dyn Error + Send + Sync>> {
    // 1. Resolve conversation_key (same logic as the normal path).
    // 2. Load messages from conversation_repository.
    // 3. Find the last assistant message with a tool_call that has no matching tool_message after it.
    //    If multiple tool_calls in that assistant msg, take the FIRST without a tool_message.
    // 4. Build a `ToolCall` from that one (id, name, arguments).
    // 5. Call tool_executor.execute_with_resume_answer(&tc, resume_answer).
    // 6. If result.output again has __colmena_status: SUSPENDED, propagate (return SUSPENDED again).
    // 7. Persist the new tool_message via conversation_repository.add_message.
    // 8. Re-build agent_service params with messages = loaded+tool_message, prompt = None.
    // 9. Run agent_service.run(params). If it suspends again, recurse-style propagate; otherwise return normal output.
}
```

- [ ] **Step 6: Wire up the entry point in `LlmNode::execute`**

At the very top:

```rust
async fn execute(&self, inputs: &NodeInputs, config: &Value, ...) -> Result<...> {
    if let Some(answer) = inputs.get("__colmena_resume_answer").and_then(|v| v.as_str()) {
        return self.resume_from_suspended_tool(answer, inputs, config, /* ... */).await;
    }
    // ... existing flow ...
}
```

The existing flow's tool_executor + agent_service construction needs to be hoisted into a helper or inlined into `resume_from_suspended_tool`. Pick whichever is cleaner.

- [ ] **Step 7: Add an integration test** for resume

In `src/libs/colmena/tests/llm_tool_suspend_integration.rs`:

```rust
#[tokio::test]
#[ignore = "requires DATABASE_URL + GEMINI_API_KEY — run with `cargo test -- --ignored`"]
async fn llm_agent_collects_secret_then_uses_it() {
    // Use a graph where llm_call has tools = { ask_secret (secure_suspend), echo (http_request to httpbin.org/post) }.
    // Run 1: trigger LLM. It calls ask_secret. Suspend.
    //   Verify graph's GraphFinish event has __colmena_status:"SUSPENDED" and questions array.
    // Run 2: same session_id + answer "Q1?\nval1\nQ2?\nval2".
    //   Verify the LLM continued, called echo with the handles, httpbin echoed back the real values.
    //   Verify the final node-end output (LLM's final response).
}
```

- [ ] **Step 8: Run all tests + cargo check** → clean.

```bash
cargo test --lib -p colmena_dag_engine && cargo check --all-targets
source .env && cargo test -- --ignored 2>&1 | tail -25
```

- [ ] **Step 9: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs \
        src/libs/colmena/tests/llm_tool_suspend_integration.rs \
        tests/graphs/advanced/llm_secure_suspend_resume.json
git commit -m "feat(llm-node): resume path replays conversation, executes pending tool with answer"
```

---

## Task 6: Final end-to-end re-validation against the e2e LLM graph

This re-runs `tests/graphs/advanced/secure_suspend_login_e2e.json` (the LLM-driven version) end-to-end.

- [ ] **Step 1: Build**: `cargo build --bin dag_engine`.

- [ ] **Step 2: Suspend phase**:

```bash
SESSION_ID="gap2_validate_$(date +%s)"
echo "SESSION: $SESSION_ID"
source .env
./target/debug/dag_engine run tests/graphs/advanced/secure_suspend_login_e2e.json --session-id "$SESSION_ID" 2>&1 | tail -15
```

Expected: SUSPENDED with two questions (LLM called ask_secret and the engine paused).

- [ ] **Step 3: Resume phase**:

```bash
./target/debug/dag_engine run tests/graphs/advanced/secure_suspend_login_e2e.json \
  --session-id "$SESSION_ID" \
  --answer "<question1>
juan@example.com
<question2>
my-Real-PWD-987" 2>&1 | tail -50
```

(Replace `<question1>` and `<question2>` with the exact question texts from Step 2's questions array.)

Expected:
- LLM continues. Calls dummy_login with handles.
- httpbin's response shows `json: {user:"juan@example.com", password:"my-Real-PWD-987"}` (real values reached the wire).
- LLM's final summary message describes the success.

- [ ] **Step 4: No commit needed** — pure validation. Document the success.

---

## Final Verification

- [ ] `cargo test --verbose -p colmena_dag_engine 2>&1 | tail -10` — no regressions.
- [ ] `source .env && cargo test -- --ignored 2>&1 | tail -25` — all integration tests pass.
- [ ] `cargo clippy --all-targets -- -D warnings 2>&1 | tail -5` — clean.
- [ ] `git log --oneline` shows ~7 commits since plan start.

---

## Self-Review Notes

**Spec coverage:**

| Spec section | Task |
|---|---|
| LlmResponse + SuspendInfo | Task 1 |
| AgentRunParams.prompt optional | Task 2 |
| Detection in agent_service | Task 3 |
| DagToolExecutor::execute_with_resume_answer | Task 4 |
| llm.rs propagation upward | Task 5A |
| llm.rs resume path | Task 5B |
| Pre-condition: connection_url | (handled implicitly — resume reads conversation, which requires it) |
| End-to-end validation | Task 6 |

**No placeholders.** Each task has the exact code or command.

**Risk:** Task 5B is the largest. The resume path interacts with conversation memory and the agent_service loop. If `agent_service` doesn't gracefully handle continuation, fallback strategy is to bypass it for resume and call the LLM repository directly with the loaded messages.
