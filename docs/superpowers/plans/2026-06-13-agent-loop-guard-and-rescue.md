# Agent Loop Guard + Graceful Rescue Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the per-turn `max_iterations` cap with a per-signature loop guard (nudge on repeated identical tool calls) plus a graceful "rescue" (forced final synthesis) so productive multi-step agents never die prematurely or return a bare error.

**Architecture:** In `agent_service.rs` the ReAct loop bound becomes a per-run `max_turns` whose default is read from env `COLMENA_HARD_TURN_CAP` (fallback 50). The public `max_iterations` config key is re-purposed to drive a new `max_tool_repeats` budget (default 3): when the LLM emits the same `(name+args)` signature **consecutively** (a streak that resets the moment a different signature appears) it gets a "nudge" on the 2nd (prior result + redirect, tool NOT re-executed) and triggers rescue on the 3rd. Both the loop guard and the turn ceiling end in one terminal LLM call with tools removed ("give your best final answer") returned as `Ok`, never `Err(MaxIterationsReached)`. `AgentService.run` is used by **six** nodes; the five single-shot ones (`planner`, `reactor`, `critic`, `orchestrator`, `extract_with_schema`) set `max_turns: Some(1)` to keep their one-turn behavior.

**Tech Stack:** Rust, `serde_json`, `mockall` (test mocks), `tokio`, `include_str!` text registry.

---

## File Structure

| File | Responsibility | Change |
|---|---|---|
| `src/libs/colmena/src/llm/application/agent_service.rs` | The ReAct loop | signature helpers, `invoke_llm`/`accumulate_usage` extraction, loop guard + nudge + rescue, field rename, tests |
| `src/libs/colmena/text/prompts/agent_loop/repeat_nudge.md` | LLM-facing nudge text | new |
| `src/libs/colmena/text/prompts/agent_loop/rescue_synthesis.md` | LLM-facing rescue instruction | new |
| `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs` | Node wiring | read `max_iterations` → `max_tool_repeats`, `max_turns: None`, update 2 construction sites + log |
| `nodes/{planner,reactor,critic,orchestrator}.rs`, `nodes/util/extract_with_schema.rs` | Single-shot callers | `max_iterations: Some(1)` → `max_turns: Some(1), max_tool_repeats: None` |
| `docs/developer_guide/14_llm_deep_dive.md` | LLM node docs | document new semantics |
| `docs/CHANGELOG_*.md` | Change log | add entry |

**Note on `MaxIterationsReached`:** the variant stays in `LlmError` (compat) but is no longer returned by the normal path.

---

## Task 1: Pure signature helpers

**Files:**
- Modify: `src/libs/colmena/src/llm/application/agent_service.rs` (add free functions near the other module-level helpers, e.g. after `strip_leading_temporal_block`; add tests in the existing `#[cfg(test)] mod tests`)

- [ ] **Step 1: Write the failing tests**

Add to the `#[cfg(test)] mod tests` block:

```rust
#[test]
fn tool_call_signature_is_key_order_independent() {
    let a = tool_call_signature("read", r#"{"a":1,"b":2}"#);
    let b = tool_call_signature("read", r#"{"b":2,"a":1}"#);
    assert_eq!(a, b, "object key order must not change the signature");
}

#[test]
fn tool_call_signature_is_name_and_args_sensitive() {
    assert_ne!(
        tool_call_signature("read", r#"{"a":1}"#),
        tool_call_signature("write", r#"{"a":1}"#),
        "different tool names must differ"
    );
    assert_ne!(
        tool_call_signature("read", r#"{"range":"A1"}"#),
        tool_call_signature("read", r#"{"range":"B2"}"#),
        "different args must differ"
    );
}

#[test]
fn tool_call_signature_handles_nested_and_invalid_json() {
    // nested object key order also normalized
    let a = tool_call_signature("t", r#"{"x":{"p":1,"q":2}}"#);
    let b = tool_call_signature("t", r#"{"x":{"q":2,"p":1}}"#);
    assert_eq!(a, b);
    // invalid JSON falls back to the raw string (still deterministic)
    let c = tool_call_signature("t", "not json");
    let d = tool_call_signature("t", "not json");
    assert_eq!(c, d);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib tool_call_signature`
Expected: FAIL — `cannot find function tool_call_signature in this scope`.

- [ ] **Step 3: Implement the helpers**

Add at module level (not inside `impl`):

```rust
/// Canonical `(name, arguments)` signature used to detect repeated tool calls.
/// Object keys are sorted recursively so `{"a":1,"b":2}` and `{"b":2,"a":1}`
/// collapse to one key. Invalid-JSON arguments fall back to the raw string.
/// The `\u{0}` separator cannot appear in a JSON token, so name and args never
/// collide.
fn tool_call_signature(name: &str, arguments: &str) -> String {
    let canon = serde_json::from_str::<serde_json::Value>(arguments)
        .map(|v| canonical_json(&v))
        .unwrap_or_else(|_| arguments.to_string());
    format!("{name}\u{0}{canon}")
}

/// Deterministic, key-sorted serialization of a JSON value (for signatures only).
fn canonical_json(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let inner: Vec<String> = keys
                .into_iter()
                .map(|k| {
                    let key = serde_json::to_string(k).unwrap_or_default();
                    format!("{}:{}", key, canonical_json(&map[k]))
                })
                .collect();
            format!("{{{}}}", inner.join(","))
        }
        serde_json::Value::Array(arr) => {
            let inner: Vec<String> = arr.iter().map(canonical_json).collect();
            format!("[{}]", inner.join(","))
        }
        other => other.to_string(),
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib tool_call_signature`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/llm/application/agent_service.rs
git commit -m "feat(agent-loop): add canonical tool-call signature helper

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 2: LLM-facing text registry files

**Files:**
- Create: `src/libs/colmena/text/prompts/agent_loop/repeat_nudge.md`
- Create: `src/libs/colmena/text/prompts/agent_loop/rescue_synthesis.md`
- Modify: `src/libs/colmena/src/llm/application/agent_service.rs` (add two `include_str!` consts near the top, after the existing `const COMPACT_*` block)

- [ ] **Step 1: Create the nudge text file**

`src/libs/colmena/text/prompts/agent_loop/repeat_nudge.md`:

```markdown
Ya llamaste esta herramienta con exactamente los mismos argumentos; su resultado está arriba. No repitas la misma llamada: usá ese resultado, o si necesitás algo distinto cambiá los argumentos, probá otra herramienta, o respondé directamente.
```

- [ ] **Step 2: Create the rescue text file**

`src/libs/colmena/text/prompts/agent_loop/rescue_synthesis.md`:

```markdown
Llegaste al límite de pasos de esta tarea y no podés llamar más herramientas. Con la información que ya reuniste, dá tu mejor respuesta final ahora y aclará explícitamente qué quedó incompleto o sin verificar.
```

- [ ] **Step 3: Add the include_str! consts**

In `agent_service.rs`, after the `COMPACT_SUMMARY_LINE_MAX_CHARS` const (around line 33):

```rust
/// LLM-facing text shown when a tool call with an identical `(name+args)`
/// signature is repeated (loop guard). The prior result is prepended to this.
const REPEAT_NUDGE_TEXT: &str =
    include_str!("../../../text/prompts/agent_loop/repeat_nudge.md");

/// LLM-facing instruction for the forced final synthesis ("rescue"). Appended
/// as a user message before the terminal, tool-less LLM call.
const RESCUE_SYNTHESIS_TEXT: &str =
    include_str!("../../../text/prompts/agent_loop/rescue_synthesis.md");
```

- [ ] **Step 4: Verify it compiles**

Run: `cargo check --lib`
Expected: compiles (the consts are unused for now — `#[allow(dead_code)]` is NOT needed because Task 4 uses them in the same PR; if checking standalone fails on dead_code under `warnings = "deny"`, proceed directly to Task 4 before committing, or add a temporary `#[allow(dead_code)]` removed in Task 4).

To keep this task independently green under deny-warnings, add `#[allow(dead_code)]` above each const now; Task 4 removes both attributes when the consts gain real uses.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/text/prompts/agent_loop/ src/libs/colmena/src/llm/application/agent_service.rs
git commit -m "feat(agent-loop): add nudge + rescue text registry entries

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 3: Extract `invoke_llm` + `accumulate_usage` (pure refactor)

**Goal:** factor the call/stream branch and usage accumulation into reusable units so the rescue synthesis (Task 4) can reuse them. No behavior change — all existing tests stay green.

**Files:**
- Modify: `src/libs/colmena/src/llm/application/agent_service.rs`

- [ ] **Step 1: Add `accumulate_usage` free function**

Add at module level (near `tool_call_signature`):

```rust
/// Fold one response's usage into the running cumulative usage.
fn accumulate_usage(cumulative: &mut LlmUsage, response: &LlmResponse) {
    if let Some(usage) = response.usage() {
        cumulative.prompt_tokens += usage.prompt_tokens;
        cumulative.completion_tokens += usage.completion_tokens;
        cumulative.total_tokens += usage.total_tokens;
        if let Some(t) = usage.thinking_tokens {
            *cumulative.thinking_tokens.get_or_insert(0) += t;
        }
        if let Some(cr) = usage.cache_read_tokens {
            *cumulative.cache_read_tokens.get_or_insert(0) += cr;
        }
        if let Some(cw) = usage.cache_write_tokens {
            *cumulative.cache_write_tokens.get_or_insert(0) += cw;
        }
    }
}
```

- [ ] **Step 2: Replace the inline usage block with the call**

In `run`, replace the block currently at lines ~396–410 (the `if let Some(usage) = response.usage() { cumulative_usage.prompt_tokens += ... }`) with:

```rust
            // Accumulate usage for this step
            accumulate_usage(&mut cumulative_usage, &response);
```

- [ ] **Step 3: Add the `invoke_llm` method**

Add inside `impl AgentService`, after `run`:

```rust
    /// One LLM round-trip (stream or call) for `request`. Emits the
    /// `LlmMessageStart`/`LlmMessageFinish` bracket and forwards every stream
    /// part to `on_token` when present. Returns the assembled response and its
    /// completion usage.
    async fn invoke_llm(
        &self,
        request: LlmRequest,
        on_token: &Option<Box<dyn Fn(LlmStreamPart) + Send + Sync>>,
        config: &LlmConfig,
    ) -> Result<(LlmResponse, Option<LlmUsage>), LlmError> {
        if let Some(callback) = on_token {
            (callback)(LlmStreamPart::LlmMessageStart);
        }

        let mut completion_usage = None;
        let response = if let Some(callback) = on_token {
            let stream = self.llm_repository.stream(request).await?;
            use futures::StreamExt;
            let mut stream = stream;

            let mut full_content = String::new();
            let mut full_thinking = String::new();
            let mut captured_provider = config.provider().clone();
            let mut captured_req_id = crate::llm::domain::LlmRequestId::new();
            let mut accumulated_tool_calls: std::collections::HashMap<usize, ToolCall> =
                std::collections::HashMap::new();

            while let Some(chunk_result) = stream.next().await {
                match chunk_result {
                    Ok(chunk) => {
                        captured_req_id = chunk.request_id().clone();
                        captured_provider = chunk.provider().clone();
                        (callback)(chunk.part().clone());
                        match chunk.part() {
                            LlmStreamPart::Content(c) => full_content.push_str(c),
                            LlmStreamPart::ThinkingContent(c) => full_thinking.push_str(c),
                            LlmStreamPart::ToolCallChunk(tc) => {
                                let entry = accumulated_tool_calls
                                    .entry(tc.index)
                                    .or_insert_with(|| {
                                        ToolCall::new(
                                            tc.id.clone(),
                                            crate::llm::domain::FunctionCall::new(
                                                tc.name.clone(),
                                                String::new(),
                                            ),
                                        )
                                    });
                                if !tc.id.is_empty() && entry.id.is_empty() {
                                    entry.id = tc.id.clone();
                                }
                                if !tc.name.is_empty() && entry.function.name.is_empty() {
                                    entry.function.name = tc.name.clone();
                                }
                                entry.function.arguments.push_str(&tc.args_chunk);
                            }
                            LlmStreamPart::Usage(u) => completion_usage = Some(u.clone()),
                            LlmStreamPart::ThinkingStart
                            | LlmStreamPart::ThinkingEnd
                            | LlmStreamPart::LlmToolCallStart(_)
                            | LlmStreamPart::LlmToolCallFinish(_)
                            | LlmStreamPart::LlmMessageStart
                            | LlmStreamPart::LlmMessageFinish(_) => {}
                        }
                    }
                    Err(e) => return Err(e),
                }
            }

            let mut final_response =
                LlmResponse::new(captured_req_id, full_content, captured_provider)?;
            if !full_thinking.is_empty() {
                final_response = final_response.with_thinking_content(full_thinking);
            }
            if !accumulated_tool_calls.is_empty() {
                let tools: Vec<ToolCall> = accumulated_tool_calls.into_values().collect();
                final_response = final_response.with_tool_calls(tools);
            }
            if let Some(usage) = &completion_usage {
                final_response = final_response.with_usage(usage.clone());
            }
            final_response
        } else {
            let res = self.llm_repository.call(request).await?;
            completion_usage = res.usage().cloned();
            res
        };

        if let Some(callback) = on_token {
            (callback)(LlmStreamPart::LlmMessageFinish(completion_usage.clone()));
        }

        Ok((response, completion_usage))
    }
```

- [ ] **Step 4: Replace the inline call/stream block in `run` with the helper**

Delete the `LlmStreamPart::LlmMessageStart` emit (lines ~184–186) AND the entire `let mut completion_usage = None; let mut response = if let Some(callback) = &on_token { ... } else { ... };` block plus the `LlmMessageFinish` emit (lines ~301–394). Replace all of it with:

```rust
            let (mut response, _completion_usage) =
                self.invoke_llm(request, &on_token, &config).await?;
```

(The `request` is still built by the unchanged lines above; the `COLMENA_DUMP_PROMPT_SIZES` diagnostic block stays as-is between request build and this call.)

- [ ] **Step 5: Run the full agent_service test suite**

Run: `cargo test --lib agent_service`
Expected: PASS — same set of tests as before (no behavior change). If a streaming test exists it must still pass; the message-start/finish ordering is preserved.

- [ ] **Step 6: Commit**

```bash
git add src/libs/colmena/src/llm/application/agent_service.rs
git commit -m "refactor(agent-loop): extract invoke_llm + accumulate_usage helpers

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 4: Loop guard, nudge, rescue + field rename

**Goal:** the behavioral core. Rename the param + add `max_turns`, add the `default_hard_turn_cap()` env helper, implement the consecutive-streak nudge, and replace the terminal `Err` with forced synthesis.

**Files:**
- Modify: `src/libs/colmena/src/llm/application/agent_service.rs`

- [ ] **Step 1: Write the failing behavior tests**

Add to `#[cfg(test)] mod tests`. These use a call-counter mock so the synthesis turn returns text. Add `use std::sync::atomic::{AtomicUsize, Ordering};` at the top of the test module if not present.

```rust
fn loop_tool_call(args: &str) -> ToolCall {
    ToolCall {
        id: "call_loop".to_string(),
        call_type: "function".to_string(),
        function: FunctionCall {
            name: "loop".to_string(),
            arguments: args.to_string(),
        },
        response: None,
    }
}

fn text_response(text: &str) -> LlmResponse {
    LlmResponse::new(
        LlmRequestId::from_string("req".to_string()).unwrap(),
        text.to_string(),
        LlmProvider::new(ProviderKind::OpenAi, "key".to_string(), Some("gpt-4".to_string()))
            .unwrap(),
    )
    .unwrap()
}

fn tool_call_response(call: ToolCall) -> LlmResponse {
    text_response("").with_tool_calls(vec![call])
}

#[tokio::test]
async fn repeated_signature_nudges_then_rescues_with_synthesis() {
    let mut mock_llm = MockLlmRepo::new();
    let mut mock_conv = MockConversationRepo::new();
    let mut mock_tool_exec = MockToolExec::new();
    let key = test_key();

    mock_conv.expect_get_by_id().returning(|k| {
        Ok(Conversation { key: k.clone(), messages: vec![] })
    });
    mock_conv.expect_add_message().returning(|_, _| Ok(()));

    // The tool must be executed EXACTLY ONCE despite 3 identical requests:
    // occurrence 1 executes, 2 is nudged, 3 triggers rescue (no execution).
    mock_tool_exec.expect_execute().times(1).returning(|call| {
        Ok(ToolResult {
            tool_call_id: call.id.clone(),
            success: true,
            output: "first-result".to_string(),
            error: None,
        })
    });

    // Calls 1-3 → identical tool call; call 4 (synthesis, tool-less) → final text.
    let counter = Arc::new(AtomicUsize::new(0));
    let c = counter.clone();
    mock_llm.expect_call().returning(move |_req| {
        let n = c.fetch_add(1, Ordering::SeqCst);
        if n < 3 {
            Ok(tool_call_response(loop_tool_call("{}")))
        } else {
            Ok(text_response("Best-effort final answer."))
        }
    });

    let service = AgentService::new(Arc::new(mock_llm), Arc::new(mock_conv));
    let result = service
        .run(AgentRunParams {
            session_id: &key,
            prompt: Some("loop me".to_string()),
            messages: None,
            config: create_config(),
            tools: vec![],
            tool_executor: &mock_tool_exec,
            max_tool_repeats: Some(3),
            max_turns: None,
            on_token: None,
            tools_provider: None,
            attachment_resolver: None,
            agent_session_id: None,
        })
        .await;

    assert!(result.is_ok(), "rescue must return Ok, not Err");
    assert_eq!(result.unwrap().content(), "Best-effort final answer.");
}

#[tokio::test]
async fn distinct_signatures_are_never_nudged() {
    let mut mock_llm = MockLlmRepo::new();
    let mut mock_conv = MockConversationRepo::new();
    let mut mock_tool_exec = MockToolExec::new();
    let key = test_key();

    mock_conv.expect_get_by_id().returning(|k| {
        Ok(Conversation { key: k.clone(), messages: vec![] })
    });
    mock_conv.expect_add_message().returning(|_, _| Ok(()));

    // 5 DISTINCT calls all execute (no nudge), then a final text answer.
    mock_tool_exec.expect_execute().times(5).returning(|call| {
        Ok(ToolResult {
            tool_call_id: call.id.clone(),
            success: true,
            output: "ok".to_string(),
            error: None,
        })
    });

    let counter = Arc::new(AtomicUsize::new(0));
    let c = counter.clone();
    mock_llm.expect_call().returning(move |_req| {
        let n = c.fetch_add(1, Ordering::SeqCst);
        if n < 5 {
            Ok(tool_call_response(loop_tool_call(&format!("{{\"i\":{n}}}"))))
        } else {
            Ok(text_response("done"))
        }
    });

    let service = AgentService::new(Arc::new(mock_llm), Arc::new(mock_conv));
    let result = service
        .run(AgentRunParams {
            session_id: &key,
            prompt: Some("go".to_string()),
            messages: None,
            config: create_config(),
            tools: vec![],
            tool_executor: &mock_tool_exec,
            max_tool_repeats: Some(3),
            max_turns: None,
            on_token: None,
            tools_provider: None,
            attachment_resolver: None,
            agent_session_id: None,
        })
        .await;

    assert!(result.is_ok());
    assert_eq!(result.unwrap().content(), "done");
}

#[tokio::test]
async fn streak_resets_when_a_different_signature_appears() {
    // Sequence A, A, B, A: the first A-run reaches 2 (nudge) but never 3
    // because B resets the streak; the final A starts a fresh run. No rescue
    // from the loop guard — the model ends with a normal text answer.
    let mut mock_llm = MockLlmRepo::new();
    let mut mock_conv = MockConversationRepo::new();
    let mut mock_tool_exec = MockToolExec::new();
    let key = test_key();

    mock_conv.expect_get_by_id().returning(|k| {
        Ok(Conversation { key: k.clone(), messages: vec![] })
    });
    mock_conv.expect_add_message().returning(|_, _| Ok(()));

    // A executes (turn 0), A nudged (turn 1, no exec), B executes (turn 2),
    // A executes again (turn 3, fresh streak) → 3 real executions total.
    mock_tool_exec.expect_execute().times(3).returning(|call| {
        Ok(ToolResult {
            tool_call_id: call.id.clone(),
            success: true,
            output: "ok".to_string(),
            error: None,
        })
    });

    let counter = Arc::new(AtomicUsize::new(0));
    let c = counter.clone();
    mock_llm.expect_call().returning(move |_req| {
        let n = c.fetch_add(1, Ordering::SeqCst);
        match n {
            0 | 1 | 3 => Ok(tool_call_response(loop_tool_call("{}"))), // A
            2 => Ok(tool_call_response(ToolCall {
                id: "call_b".to_string(),
                call_type: "function".to_string(),
                function: FunctionCall { name: "other".to_string(), arguments: "{}".to_string() },
                response: None,
            })), // B (different signature → resets streak)
            _ => Ok(text_response("finished")),
        }
    });

    let service = AgentService::new(Arc::new(mock_llm), Arc::new(mock_conv));
    let result = service
        .run(AgentRunParams {
            session_id: &key,
            prompt: Some("vary".to_string()),
            messages: None,
            config: create_config(),
            tools: vec![],
            tool_executor: &mock_tool_exec,
            max_tool_repeats: Some(3),
            max_turns: None,
            on_token: None,
            tools_provider: None,
            attachment_resolver: None,
            agent_session_id: None,
        })
        .await;

    assert!(result.is_ok());
    assert_eq!(result.unwrap().content(), "finished");
}

#[tokio::test]
async fn max_turns_ceiling_triggers_synthesis_not_error() {
    let mut mock_llm = MockLlmRepo::new();
    let mut mock_conv = MockConversationRepo::new();
    let mut mock_tool_exec = MockToolExec::new();
    let key = test_key();

    mock_conv.expect_get_by_id().returning(|k| {
        Ok(Conversation { key: k.clone(), messages: vec![] })
    });
    mock_conv.expect_add_message().returning(|_, _| Ok(()));

    // Every turn makes a DISTINCT call (never nudged) so only the turn ceiling
    // can stop the loop. With max_turns = 4: 4 executions, then synthesis.
    mock_tool_exec.expect_execute().times(4).returning(|call| {
        Ok(ToolResult {
            tool_call_id: call.id.clone(),
            success: true,
            output: "ok".to_string(),
            error: None,
        })
    });

    let counter = Arc::new(AtomicUsize::new(0));
    let c = counter.clone();
    mock_llm.expect_call().returning(move |_req| {
        let n = c.fetch_add(1, Ordering::SeqCst);
        if n < 4 {
            Ok(tool_call_response(loop_tool_call(&format!("{{\"i\":{n}}}"))))
        } else {
            Ok(text_response("capped answer"))
        }
    });

    let service = AgentService::new(Arc::new(mock_llm), Arc::new(mock_conv));
    let result = service
        .run(AgentRunParams {
            session_id: &key,
            prompt: Some("loop forever".to_string()),
            messages: None,
            config: create_config(),
            tools: vec![],
            tool_executor: &mock_tool_exec,
            max_tool_repeats: Some(3),
            max_turns: Some(4),
            on_token: None,
            tools_provider: None,
            attachment_resolver: None,
            agent_session_id: None,
        })
        .await;

    assert!(result.is_ok(), "hitting the turn ceiling must synthesize, not error");
    assert_eq!(result.unwrap().content(), "capped answer");
}

#[tokio::test]
async fn single_shot_max_turns_one_returns_directly() {
    // The single-shot callers pass max_turns: Some(1). A turn-1 text answer
    // (no tools) returns normally — the loop guard never engages.
    let mut mock_llm = MockLlmRepo::new();
    let mut mock_conv = MockConversationRepo::new();
    let mock_tool_exec = MockToolExec::new();
    let key = test_key();

    mock_conv.expect_get_by_id().returning(|k| {
        Ok(Conversation { key: k.clone(), messages: vec![] })
    });
    mock_conv.expect_add_message().returning(|_, _| Ok(()));
    mock_llm.expect_call().times(1).returning(|_| Ok(text_response("one shot")));

    let service = AgentService::new(Arc::new(mock_llm), Arc::new(mock_conv));
    let result = service
        .run(AgentRunParams {
            session_id: &key,
            prompt: Some("answer once".to_string()),
            messages: None,
            config: create_config(),
            tools: vec![],
            tool_executor: &mock_tool_exec,
            max_tool_repeats: None,
            max_turns: Some(1),
            on_token: None,
            tools_provider: None,
            attachment_resolver: None,
            agent_session_id: None,
        })
        .await;

    assert!(result.is_ok());
    assert_eq!(result.unwrap().content(), "one shot");
}
```

- [ ] **Step 2: Delete/replace the old max-iterations test**

Replace the entire `test_agent_service_max_iterations` test (lines ~1719–1792) — it asserts `Err(MaxIterationsReached)`, which no longer happens — with the three tests above (delete the old one).

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test --lib agent_service`
Expected: FAIL — `struct AgentRunParams has no field named max_tool_repeats` (and the new tests don't pass yet).

- [ ] **Step 4: Rename the param field and add `max_turns`**

In `AgentRunParams` (line ~65), change:

```rust
    pub max_iterations: Option<usize>,
```

to:

```rust
    /// Maximum number of times one `(name+args)` tool-call signature may be
    /// emitted **consecutively** before the loop guard rescues (forced final
    /// synthesis). The public `max_iterations` config key feeds this. Default 3.
    pub max_tool_repeats: Option<usize>,
    /// Hard ceiling on total LLM turns for this run. `None` → resolved from env
    /// `COLMENA_HARD_TURN_CAP` (fallback 50). Single-shot callers pass `Some(1)`.
    pub max_turns: Option<usize>,
```

Now EVERY `AgentRunParams { .. }` literal in this file's tests needs BOTH fields. Update each `max_iterations:` to `max_tool_repeats:` (the `None` sites at lines ~1542, 1645, 1707, 1869 and the `Some(5)` sites at ~2009, 2163, 2318 — keep their values) AND add `max_turns: None,` directly below each. (The new Task-4 tests above already include both fields.)

- [ ] **Step 5: Add the env helper, streak state, and loop bound**

Replace line ~111 (`let max_iter = params.max_iterations.unwrap_or(10);`) with:

```rust
        let max_tool_repeats = params.max_tool_repeats.unwrap_or(3);
        let max_turns = params.max_turns.unwrap_or_else(default_hard_turn_cap);
```

Add at module level (near `accumulate_usage`):

```rust
/// Default hard ceiling on total LLM turns, read from env `COLMENA_HARD_TURN_CAP`
/// (positive integer), falling back to 50. Pure cost/termination backstop —
/// reaching it triggers the same forced-synthesis rescue as the loop guard.
fn default_hard_turn_cap() -> usize {
    std::env::var("COLMENA_HARD_TURN_CAP")
        .ok()
        .and_then(|s| s.trim().parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(50)
}
```

Change the loop header (line ~176). Replace `for _iteration in 0..max_iter {` with the streak-state declarations + the new bound:

```rust
        // Loop-guard streak: counts CONSECUTIVE repeats of one signature, and
        // resets the moment a different signature appears (the model made
        // progress). `streak_first` is the raw output of this streak's one real
        // execution, echoed back in a nudge.
        let mut streak_sig: Option<String> = None;
        let mut streak_count: u32 = 0;
        let mut streak_first = String::new();

        // 3. ReAct Loop — bounded by the per-run turn ceiling. Productive work
        //    is gated by the loop guard below, not by turns.
        for _iteration in 0..max_turns {
```

And update the tracing at lines ~177–182 to use `max = max_turns`. (No `SigEntry` struct — the streak is three locals.)

- [ ] **Step 6: Implement the streak guard inside the tool loop**

Replace the `// D. Execute each tool call` loop (the `for tool_call in tool_calls { ... }` starting ~line 439, up to and including its trailing `continue;` at ~637) with the version below. For each tool call it first updates the streak (same signature → `+1`; different → reset to 1). `streak_count == 1` executes (storing `streak_first`); `>= max_tool_repeats` writes a closing tool message and flags rescue; otherwise (`>= 2`) it nudges. After answering every tool id in the turn, if rescue was flagged it breaks to synthesis.

```rust
                // D. Execute each tool call (with consecutive-streak loop guard)
                let mut rescue = false;
                for tool_call in tool_calls {
                    let sig = tool_call_signature(
                        &tool_call.function.name,
                        &tool_call.function.arguments,
                    );
                    if streak_sig.as_deref() == Some(sig.as_str()) {
                        streak_count += 1;
                    } else {
                        streak_sig = Some(sig.clone());
                        streak_count = 1;
                        streak_first.clear();
                    }
                    let count = streak_count;

                    // Repeated signature in a row (streak >= 2): nudge or rescue.
                    if count > 1 {
                        let body = if streak_first.is_empty() {
                            REPEAT_NUDGE_TEXT.to_string()
                        } else {
                            format!("{streak_first}\n\n{REPEAT_NUDGE_TEXT}")
                        };

                        if let Some(callback) = &on_token {
                            (callback)(LlmStreamPart::LlmToolCallStart(tool_call.clone()));
                            (callback)(LlmStreamPart::LlmToolCallFinish(ToolResult {
                                tool_call_id: tool_call.id.clone(),
                                output: body.clone(),
                                success: true,
                                error: None,
                            }));
                        }

                        let mut nudged_call = tool_call.clone();
                        nudged_call.response = Some(serde_json::Value::String(body.clone()));
                        all_tool_calls_executed.push(nudged_call);

                        let tool_message =
                            LlmMessage::tool(tool_call.id.clone(), body)?;
                        messages.push(tool_message.clone());
                        self.conversation_repository
                            .add_message(session_id, tool_message)
                            .await?;

                        if count >= max_tool_repeats as u32 {
                            // Loop guard tripped: still answer the rest of this
                            // turn's tool ids (done by continuing the loop), then
                            // break to synthesis after the for-loop.
                            rescue = true;
                        }
                        continue;
                    }

                    // Streak start (count == 1): real execution (existing path).
                    let mut executed_call = tool_call.clone();

                    if let Some(callback) = &on_token {
                        (callback)(LlmStreamPart::LlmToolCallStart(tool_call.clone()));
                    }

                    let result = match tool_executor.execute(tool_call).await {
                        Ok(res) => res,
                        Err(e) => ToolResult {
                            tool_call_id: tool_call.id.clone(),
                            success: false,
                            output: format!("Error executing tool: {}", e),
                            error: Some(e.to_string()),
                        },
                    };

                    let parsed_sentinel =
                        serde_json::from_str::<serde_json::Value>(&result.output).ok();
                    if let Some(parsed) = parsed_sentinel.as_ref() {
                        if parsed.get("__colmena_status").and_then(|v| v.as_str())
                            == Some("SUSPENDED")
                        {
                            tracing::info!(
                                target: "colmena::agent",
                                tool_call_id = %result.tool_call_id,
                                "agent_service: SUSPENDED detected in tool result, short-circuiting agent loop"
                            );
                            let questions = parsed
                                .get("questions")
                                .cloned()
                                .unwrap_or(serde_json::Value::Null);
                            return Ok(LlmResponse::suspended(
                                result.tool_call_id.clone(),
                                questions,
                                result.output.clone(),
                            ));
                        }
                        if parsed.get("__colmena_status").and_then(|v| v.as_str())
                            == Some("LOAD_ATTACHMENT")
                        {
                            let document_id = parsed
                                .get("document_id")
                                .and_then(|v| v.as_str())
                                .unwrap_or("");
                            tracing::info!(
                                target: "colmena::attachment",
                                event = "attachment.loaded",
                                document_id = %document_id,
                                "LOAD_ATTACHMENT sentinel received"
                            );
                            let resolver = match &params_resolver {
                                Some(r) => r.clone(),
                                None => {
                                    let tool_message = LlmMessage::tool(
                                        result.tool_call_id.clone(),
                                        r#"{"error":"load_attachment_unsupported","reason":"no AttachmentResolver wired"}"#.to_string(),
                                    )?;
                                    messages.push(tool_message.clone());
                                    self.conversation_repository
                                        .add_message(session_id, tool_message)
                                        .await?;
                                    continue;
                                }
                            };
                            let sid = match params_agent_session_id.as_ref() {
                                Some(s) => s.clone(),
                                None => {
                                    let tool_message = LlmMessage::tool(
                                        result.tool_call_id.clone(),
                                        r#"{"error":"load_attachment_session_missing"}"#
                                            .to_string(),
                                    )?;
                                    messages.push(tool_message.clone());
                                    self.conversation_repository
                                        .add_message(session_id, tool_message)
                                        .await?;
                                    continue;
                                }
                            };
                            let resolved = resolver.resolve(&sid, document_id).await;
                            let (ack_text, synthetic_user) = match resolved {
                                Ok(Some(file_data)) => {
                                    let body = format!(
                                        "[Attachment '{}' loaded; content follows in the next message]",
                                        document_id
                                    );
                                    let synth = LlmMessage::user_with_files(
                                        format!("[Attachment requested by the model: {}]", document_id),
                                        vec![file_data],
                                    )?;
                                    (body, Some(synth))
                                }
                                Ok(None) => (
                                    format!(
                                        "{{\"error\":\"unknown_document_id\",\"document_id\":\"{}\"}}",
                                        document_id
                                    ),
                                    None,
                                ),
                                Err(e) => (
                                    format!(
                                        "{{\"error\":\"attachment_expired_unrecoverable\",\"document_id\":\"{}\",\"reason\":\"{}\"}}",
                                        document_id,
                                        e.replace('"', "'")
                                    ),
                                    None,
                                ),
                            };
                            let loaded = synthetic_user.is_some();
                            let tool_message =
                                LlmMessage::tool(result.tool_call_id.clone(), ack_text)?;
                            messages.push(tool_message.clone());
                            self.conversation_repository
                                .add_message(session_id, tool_message)
                                .await?;
                            if let Some(user_msg) = synthetic_user {
                                messages.push(user_msg);
                                let marker_text = format!(
                                    "[load_attachment(\"{}\") was invoked. Document \
                                     content was available for this turn only. Call \
                                     load_attachment again if you need to re-read it.]",
                                    document_id
                                );
                                let marker_msg = LlmMessage::user(marker_text)?;
                                self.conversation_repository
                                    .add_message(session_id, marker_msg)
                                    .await?;
                            }
                            if let Some(callback) = &on_token {
                                let sse_payload = serde_json::json!({
                                    "document_id": document_id,
                                    "status": if loaded { "loaded" } else { "error" },
                                })
                                .to_string();
                                (callback)(LlmStreamPart::LlmToolCallFinish(ToolResult {
                                    tool_call_id: result.tool_call_id.clone(),
                                    output: sse_payload,
                                    success: loaded,
                                    error: None,
                                }));
                            }
                            continue;
                        }
                    }

                    // Store this streak's first result so a later repeat can
                    // echo it in the nudge.
                    streak_first = result.output.clone();

                    let parsed_output = serde_json::from_str::<serde_json::Value>(&result.output)
                        .unwrap_or_else(|_| serde_json::Value::String(result.output.clone()));
                    executed_call.response = Some(parsed_output);
                    all_tool_calls_executed.push(executed_call);

                    if let Some(callback) = &on_token {
                        (callback)(LlmStreamPart::LlmToolCallFinish(result.clone()));
                    }

                    let tool_message =
                        LlmMessage::tool(result.tool_call_id.clone(), result.output.clone())?;
                    messages.push(tool_message.clone());
                    self.conversation_repository
                        .add_message(session_id, tool_message)
                        .await?;
                }

                if rescue {
                    break;
                }
                continue;
```

- [ ] **Step 7: Replace the terminal `Err` with forced synthesis**

Replace the final `Err(LlmError::MaxIterationsReached { max: max_iter })` (line ~648, just before the closing `}` of `run`) with:

```rust
        // Reached here by the hard turn ceiling OR a loop-guard `break`.
        // Forced final synthesis ("rescue"): one terminal, tool-less LLM call.
        tracing::info!(
            target: "colmena::agent",
            "agent_service: forced final synthesis (rescue)"
        );
        messages.push(LlmMessage::user(RESCUE_SYNTHESIS_TEXT.to_string())?);

        let request_messages =
            compact_old_load_skill_in_history(&messages, COMPACT_LOAD_SKILL_KEEP_RECENT_MSGS);
        let request_messages = compact_history_to_summary(
            &request_messages,
            COMPACT_SUMMARY_KEEP_FIRST_MSGS,
            COMPACT_SUMMARY_KEEP_RECENT_MSGS,
            COMPACT_SUMMARY_MAX_LINES,
            COMPACT_SUMMARY_LINE_MAX_CHARS,
        );
        let should_stream = on_token.is_some();
        // No tools on the request → the model cannot call a tool.
        let request = LlmRequest::new(request_messages, config.clone(), should_stream)?;
        let (mut response, _usage) = self.invoke_llm(request, &on_token, &config).await?;

        accumulate_usage(&mut cumulative_usage, &response);
        self.conversation_repository
            .add_message(session_id, response.message().clone())
            .await?;

        let content = response.content();
        if !content.is_empty() {
            if !cumulative_content.is_empty() {
                cumulative_content.push_str("\n\n");
            }
            cumulative_content.push_str(content);
        }

        response = response.with_usage(cumulative_usage);
        response = response.with_content(cumulative_content);
        if !all_tool_calls_executed.is_empty() {
            response = response.with_tool_calls(all_tool_calls_executed);
        }
        Ok(response)
```

- [ ] **Step 8: Remove the temporary `#[allow(dead_code)]` from Task 2**

The two text consts are now used — delete both `#[allow(dead_code)]` attributes added in Task 2 Step 4.

- [ ] **Step 9: Run the tests**

Run: `cargo test --lib agent_service`
Expected: PASS — including the 3 new tests; no `MaxIterationsReached` assertions remain.

- [ ] **Step 10: Clippy + fmt**

Run: `cargo clippy --lib 2>&1 | tail -5 && cargo fmt`
Expected: no warnings (deny-warnings); fmt clean.

- [ ] **Step 11: Commit**

```bash
git add src/libs/colmena/src/llm/application/agent_service.rs
git commit -m "feat(agent-loop): per-signature loop guard with nudge + forced-synthesis rescue

max_iterations no longer caps turns; it now feeds max_tool_repeats (default 3).
Hitting the loop guard or the 50-turn background ceiling forces a tool-less final
synthesis returned as Ok instead of Err(MaxIterationsReached).

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 5: Wire `llm.rs` (config key → `max_tool_repeats`)

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs` (lines ~1249–1263 comment/log; ~2854 and ~2876 construction sites)

- [ ] **Step 1: Update the resolution comment + log**

Replace the comment block + `tracing::info!` at lines ~1249–1263 with:

```rust
        // The public `max_iterations` key now drives the consecutive-repeat loop
        // guard (max_tool_repeats), NOT a turn cap. The hard turn ceiling is
        // resolved from env inside AgentService (max_turns: None below). Reads
        // inputs first (dynamic from upstream), then config, defaulting to 3.
        let max_tool_repeats: usize = inputs
            .get("max_iterations")
            .and_then(|v| v.as_u64())
            .or_else(|| config.get("max_iterations").and_then(|v| v.as_u64()))
            .map(|n| n as usize)
            .unwrap_or(3);

        tracing::info!(
            target: "colmena::llm",
            max_tool_repeats,
            "llm_call_max_tool_repeats_resolved"
        );
```

- [ ] **Step 2: Update both `AgentRunParams` construction sites in `llm.rs`**

At lines ~2854 and ~2876, change `max_iterations: Some(max_iterations),` to the two new fields (`llm_call` is multi-turn → `max_turns: None` so the env default applies):

```rust
                max_tool_repeats: Some(max_tool_repeats),
                max_turns: None,
```

- [ ] **Step 3: Update the 5 single-shot callers**

Each of these has exactly one `AgentRunParams { .. }` literal with `max_iterations: Some(1),`. Replace that line with `max_turns: Some(1),\n            max_tool_repeats: None,` (match the file's indentation) to preserve one-turn behavior:

- `src/.../nodes/planner.rs:385`
- `src/.../nodes/reactor.rs:302`
- `src/.../nodes/critic.rs:265`
- `src/.../nodes/orchestrator.rs:731`
- `src/.../nodes/util/extract_with_schema.rs:72`

For example, in `planner.rs`:

```rust
            max_turns: Some(1),
            max_tool_repeats: None,
```

Verify each with:

```bash
grep -rn "max_iterations\|max_turns\|max_tool_repeats" \
  src/libs/colmena/src/dag_engine/infrastructure/nodes/{planner,reactor,critic,orchestrator}.rs \
  src/libs/colmena/src/dag_engine/infrastructure/nodes/util/extract_with_schema.rs
```

Expected: no `max_iterations:` left; each file shows `max_turns: Some(1)` + `max_tool_repeats: None`.

- [ ] **Step 4: Build**

Run: `cargo check --lib`
Expected: compiles — no remaining references to the old `AgentRunParams.max_iterations` field anywhere.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/
git commit -m "feat(agent-loop): wire max_iterations to max_tool_repeats; single-shot callers use max_turns

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 6: Docs + CHANGELOG

**Files:**
- Modify: `docs/developer_guide/14_llm_deep_dive.md`
- Modify: the current `docs/CHANGELOG_*.md` (newest one)

- [ ] **Step 1: Document the new semantics**

Add a subsection to `docs/developer_guide/14_llm_deep_dive.md` (Spanish, matching the file). Cover, in prose:
- `max_iterations` (config key) ahora controla **repeticiones consecutivas por firma** `(name+args)`, default **3** — no turnos. La racha se **resetea** cuando el modelo emite una firma distinta (hace progreso).
- Mecánica: 1ª ejecución real → 2ª nudge (no re-ejecuta, devuelve el resultado previo + redirección) → 3ª rescate.
- Techo duro de turnos: por-run `max_turns`, default vía env **`COLMENA_HARD_TURN_CAP` (fallback 50)**, no configurable desde el grafo. Los nodos single-shot (planner/reactor/critic/orchestrator/extract_with_schema) lo fijan en 1.
- Rescate = síntesis final forzada (una llamada sin tools), devuelta como respuesta normal; ya **no** hay `Err(MaxIterationsReached)` por el camino normal.

- [ ] **Step 2: Add a CHANGELOG entry**

Append a dated entry summarizing the change and the ADP sweep note (see Task 7).

- [ ] **Step 3: Commit**

```bash
git add docs/
git commit -m "docs(agent-loop): document loop guard + rescue semantics

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 7: ADP sweep + verification

**Files:**
- Read-only sweep of the ADP repo at `/Users/danielgarcia/startti/adp/apps/service/ia/platform/`

- [ ] **Step 1: Sweep the ADP worker for the removed error path**

Run:

```bash
grep -rn "MaxIterationsReached\|max_iterations" /Users/danielgarcia/startti/adp/apps/service/ia/platform/{worker,api}/src/ 2>/dev/null
```

Expected: either no matches, or only sites that propagate the `Result` (safe — receiving `Ok` instead of `Err` is strictly better). If any code branches on `MaxIterationsReached` as a control signal, STOP and report — it needs an ADP-side change before colmena develop is pushed.

- [ ] **Step 2: Run the full verbose suite (CI parity)**

Run: `cargo test --verbose 2>&1 | tail -30`
Expected: all unit + integration + doctests pass (the project rule: `--verbose`, not `--lib`, before push).

- [ ] **Step 3: E2E — loop guard rescues a real flash agent (best effort)**

Build a graph that nudges flash off a loop and confirm it returns a final answer rather than dying. Save SSE to `/tmp/colmena_e2e/agent_loop_guard.sse` and present a friendly report (input, turns taken, whether a nudge fired, final answer). Source `.env` for `GEMINI_API_KEY` first.

```bash
mkdir -p /tmp/colmena_e2e
set -a; source .env; set +a
cargo run --bin dag_engine -- run tests/graphs/agents/<loop_guard_graph>.json \
  --agent-session-id agent_loop_guard_001 > /tmp/colmena_e2e/agent_loop_guard.sse 2>&1
```

Expected: the run completes with a synthesized final answer (no `MaxIterationsReached`); the SSE shows at least one nudged tool result if the model repeated a call.

- [ ] **Step 4: Commit any E2E graph added**

```bash
git add tests/graphs/agents/
git commit -m "test(agent-loop): E2E graph for loop guard + rescue

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Self-Review notes

- **Spec coverage:** §2 remap + §2.1 two-knob/six-caller → Task 4 (field + `max_turns`) + Task 5 (llm.rs + 5 single-shot callers); §3 streak loop detector → Task 4 Steps 5–6; §4 rescue → Task 4 Step 7; §5 wiring (incl. `default_hard_turn_cap` env) → Task 4 Step 5 + Task 5; §6 ADP sweep → Task 7 Step 1; §7 text registry → Task 2; §8 testing (streak reset, max_turns ceiling, single-shot) → Task 4 + Task 7; docs → Task 6.
- **No new public key** (`max_tool_repeats`/`max_turns` are internal `AgentRunParams` fields; only `max_iterations` is read from the graph; ceiling is env-only) — matches the decision.
- **Six callers covered:** `llm_call` (Task 5 Step 2) + `planner`/`reactor`/`critic`/`orchestrator`/`extract_with_schema` (Task 5 Step 3); all test literals get both new fields (Task 4 Step 4).
- **Type consistency:** streak locals `streak_sig: Option<String>` / `streak_count: u32` / `streak_first: String`, `tool_call_signature(&str,&str)->String`, `default_hard_turn_cap()->usize`, `accumulate_usage(&mut LlmUsage,&LlmResponse)`, `invoke_llm(LlmRequest,&Option<...>,&LlmConfig)->Result<(LlmResponse,Option<LlmUsage>)>`, `AgentRunParams.max_tool_repeats: Option<usize>` + `max_turns: Option<usize>` — all referenced consistently. No `SigEntry` struct (streak is three locals).
- **`count >= max_tool_repeats as u32`** (where `count` is the consecutive `streak_count`) with default 3 → execute(1), nudge(2), rescue(3): matches "nudge en la 2ª, rescate en la 3ª". A different signature resets the streak before it reaches 3.
