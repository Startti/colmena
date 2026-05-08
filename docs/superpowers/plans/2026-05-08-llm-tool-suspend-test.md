# LLM Tool Suspend — Test Coverage Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Provide deterministic CI coverage of the `llm_call` SUSPENDED-propagation flow (Spec 5) via a new `ScriptedAdapter`, plus one real-LLM `#[ignore]` integration test that smoke-tests the same flow against Gemini Flash.

**Architecture:** A new `ScriptedAdapter` implementing `LlmRepository` consumes a queue of `ScriptedResponse` values, returning them in order from `call()`. It supports `Text` and `ToolCall` variants. The deterministic integration test wires this adapter into the engine, runs a graph with `secure_suspend` registered as a tool, asserts SUSPENDED at the DAG level, then resumes and asserts completion. The real test uses the same graph but with a real Gemini provider, marked `#[ignore]`.

**Tech Stack:** Rust 1.95.0, existing `LlmRepository` trait, `LlmResponse::with_tool_calls`, integration test harness in `src/libs/colmena/tests/`.

**Spec:** [`docs/superpowers/specs/2026-05-08-llm-tool-suspend-test-design.md`](../specs/2026-05-08-llm-tool-suspend-test-design.md)

**Depends on:** Plan `2026-05-08-suspend-qa-response-format.md` (the Q/A format must be live first because the test asserts the Q/A path).

---

## File Structure

| File | Role |
|------|------|
| `src/libs/colmena/src/llm/infrastructure/scripted_adapter.rs` | NEW — `ScriptedAdapter` + `ScriptedResponse` enum + unit tests |
| `src/libs/colmena/src/llm/infrastructure/mod.rs` | Re-export `ScriptedAdapter` (pub for tests) |
| `src/libs/colmena/tests/llm_tool_suspend_integration.rs` | EXPAND — replace stub with real scripted-adapter-driven tests |
| `src/libs/colmena/tests/llm_tool_suspend_real.rs` | NEW — single `#[ignore]` real-Gemini test |
| `tests/graphs/advanced/llm_tool_suspend_smoke.json` | NEW — minimal graph: `llm_call` + `secure_suspend` as tool |

---

## Task 1 — `ScriptedAdapter` skeleton + unit tests

**Files:**
- Create: `src/libs/colmena/src/llm/infrastructure/scripted_adapter.rs`
- Modify: `src/libs/colmena/src/llm/infrastructure/mod.rs`

- [ ] **Step 1: Read existing adapter for the response shape**

Run: `grep -n "with_tool_calls\|ToolCall" src/libs/colmena/src/llm/domain/llm_response.rs | head -20`

Identify the existing constructors on `LlmResponse` for tool-call responses. The scripted adapter must produce identical shapes.

- [ ] **Step 2: Write the scripted adapter file with failing tests**

```rust
//! Test-only LLM adapter that emits a pre-recorded sequence of responses.
//!
//! Use to deterministically drive engine code paths that depend on specific
//! LLM behaviors (tool calls, suspend, multi-turn) without burning real API
//! quota or tolerating model nondeterminism.

use crate::llm::domain::{
    LlmError, LlmRepository, LlmRequest, LlmResponse, LlmStream,
};
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Mutex;

#[derive(Debug, Clone)]
pub enum ScriptedResponse {
    Text(String),
    ToolCall {
        id: String,
        tool_name: String,
        arguments: Value,
    },
}

pub struct ScriptedAdapter {
    queue: Mutex<Vec<ScriptedResponse>>,
}

impl ScriptedAdapter {
    pub fn new(script: Vec<ScriptedResponse>) -> Self {
        // Reverse so we can pop from the back in O(1).
        let mut q = script;
        q.reverse();
        Self {
            queue: Mutex::new(q),
        }
    }

    pub fn remaining(&self) -> usize {
        self.queue.lock().unwrap().len()
    }
}

#[async_trait]
impl LlmRepository for ScriptedAdapter {
    async fn call(&self, request: LlmRequest) -> Result<LlmResponse, LlmError> {
        let next = self
            .queue
            .lock()
            .unwrap()
            .pop()
            .ok_or_else(|| LlmError::RequestFailed {
                message: "scripted_adapter: script exhausted".to_string(),
            })?;

        match next {
            ScriptedResponse::Text(t) => LlmResponse::new(
                request.id().clone(),
                t,
                request.config().provider().clone(),
            ),
            ScriptedResponse::ToolCall {
                id,
                tool_name,
                arguments,
            } => {
                // Build a tool-call response; exact constructor depends on
                // existing helpers — adjust at implementation time.
                use crate::llm::domain::ToolCall;
                let tc = ToolCall::new(id, tool_name, arguments);
                LlmResponse::with_tool_calls(
                    request.id().clone(),
                    vec![tc],
                    request.config().provider().clone(),
                )
            }
        }
    }

    async fn stream(&self, _request: LlmRequest) -> Result<LlmStream, LlmError> {
        Err(LlmError::RequestFailed {
            message: "scripted_adapter: streaming not supported".to_string(),
        })
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }

    fn provider_name(&self) -> &'static str {
        "scripted"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // ...stub: see Step 4 for actual tests
}
```

NOTE: the exact constructor names on `LlmResponse` and `ToolCall` depend on what already exists in the codebase. Before writing the body, read `src/libs/colmena/src/llm/domain/llm_response.rs` AND `src/libs/colmena/src/llm/domain/tool_call.rs` (or wherever `ToolCall` lives) to align signatures.

- [ ] **Step 3: Add module export**

In `src/libs/colmena/src/llm/infrastructure/mod.rs`, add:

```rust
#[cfg(any(test, feature = "test-utils"))]
pub mod scripted_adapter;
#[cfg(any(test, feature = "test-utils"))]
pub use scripted_adapter::{ScriptedAdapter, ScriptedResponse};
```

If the project doesn't have a `test-utils` feature, drop the cfg-gate and just `pub mod scripted_adapter;` — the adapter is small and adds no production weight.

- [ ] **Step 4: Add unit tests inside the adapter file**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::domain::{LlmConfig, ProviderKind};

    fn make_request(text: &str) -> LlmRequest {
        // Use whatever the existing helper / builder pattern is. See e.g.
        // src/libs/colmena/src/llm/infrastructure/openai_adapter.rs tests
        // for the canonical construction.
        let config = LlmConfig::builder()
            .provider(ProviderKind::Mock)
            .model("test")
            .build()
            .unwrap();
        LlmRequest::single_user_message(config, text)
    }

    #[tokio::test]
    async fn yields_text_then_tool_call_in_order() {
        let adapter = ScriptedAdapter::new(vec![
            ScriptedResponse::Text("first".into()),
            ScriptedResponse::ToolCall {
                id: "t1".into(),
                tool_name: "echo".into(),
                arguments: serde_json::json!({"x": 1}),
            },
        ]);

        let r1 = adapter.call(make_request("hi")).await.unwrap();
        assert_eq!(r1.content(), "first");

        let r2 = adapter.call(make_request("hi again")).await.unwrap();
        assert!(r2.tool_calls().is_some());
        assert_eq!(r2.tool_calls().unwrap()[0].name(), "echo");
    }

    #[tokio::test]
    async fn errors_when_exhausted() {
        let adapter = ScriptedAdapter::new(vec![ScriptedResponse::Text("only".into())]);
        let _ = adapter.call(make_request("hi")).await.unwrap();
        let err = adapter.call(make_request("more")).await.unwrap_err();
        assert!(format!("{err}").contains("exhausted"));
    }
}
```

(Adjust constructor calls — `LlmConfig::builder`, `LlmRequest::single_user_message`, etc. — to match what exists. If those helpers don't exist, write the tests using whatever the codebase uses today.)

- [ ] **Step 5: Verify tests fail then pass**

Run: `cargo test -p colmena_dag_engine --lib scripted_adapter`
Expected: tests compile and pass once any signature mismatches against the real `LlmResponse` / `ToolCall` types are resolved.

- [ ] **Step 6: Commit**

```bash
git add src/libs/colmena/src/llm/infrastructure/scripted_adapter.rs \
        src/libs/colmena/src/llm/infrastructure/mod.rs
git commit -m "$(cat <<'EOF'
test(llm): add ScriptedAdapter for deterministic test scripts

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2 — Smoke graph for the suspend-via-tool flow

**Files:**
- Create: `tests/graphs/advanced/llm_tool_suspend_smoke.json`

- [ ] **Step 1: Author the graph**

Minimal shape: an `input` node feeding an `llm_call` node that has `secure_suspend` registered as a tool named `ask_secret`. The graph is keyed by `agent_session_id` so resume works.

```json
{
  "nodes": {
    "user_input": {
      "node_type": "input",
      "config": {
        "default": "Set up a connection — I need username and password."
      }
    },
    "agent": {
      "node_type": "llm_call",
      "config": {
        "provider": "mock",
        "model": "scripted",
        "system_message": "You collect credentials by calling ask_secret.",
        "connection_url": "${DATABASE_URL}",
        "tool_configurations": {
          "ask_secret": {
            "node_type": "secure_suspend",
            "description": "Collect one or more secrets from the user. Provide a list of {question, name} pairs.",
            "node_schema": {
              "secrets": {
                "type": "array",
                "required": true,
                "description": "Array of {question: string, name: string}"
              }
            }
          }
        }
      }
    }
  },
  "edges": [
    { "from": "user_input", "to": "agent" }
  ]
}
```

(Adjust to match the exact shape required by the engine — verify against `tests/graphs/external/canvas_builder_controlled.json` which uses the same pattern.)

- [ ] **Step 2: Validate the graph parses**

Run: `cargo run --bin dag_engine -- run tests/graphs/advanced/llm_tool_suspend_smoke.json --agent-session-id agent_smoke_validate_001 2>&1 | head -40`

Expected: at minimum, the graph loads without parse errors. (It may fail at execution because the `mock` provider doesn't tool-call by default — that's fine for this validation step.)

- [ ] **Step 3: Commit**

```bash
git add tests/graphs/advanced/llm_tool_suspend_smoke.json
git commit -m "$(cat <<'EOF'
test(graphs): minimal llm_call+secure_suspend smoke graph

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3 — Deterministic integration test with `ScriptedAdapter`

**Files:**
- Modify: `src/libs/colmena/tests/llm_tool_suspend_integration.rs`

- [ ] **Step 1: Read the current stub**

Run: `cat src/libs/colmena/tests/llm_tool_suspend_integration.rs`

Note what's there (likely `#[ignore]`'d stubs explaining why) and identify the helper utilities the suite already uses (test container setup, DAG run harness, etc.).

- [ ] **Step 2: Replace stub with three deterministic tests**

```rust
//! Deterministic coverage of `llm_call`'s SUSPENDED-propagation path
//! (Spec 5) using ScriptedAdapter — no real LLM provider needed.

use colmena_dag_engine::llm::infrastructure::{ScriptedAdapter, ScriptedResponse};
// ...whatever harness imports the integration suite already uses.

#[tokio::test]
async fn suspend_propagates_when_tool_returns_suspended() {
    // Arrange: scripted adapter that, on its first call, emits a tool_call
    // to ask_secret with a single {question, name} pair.
    let adapter = ScriptedAdapter::new(vec![
        ScriptedResponse::ToolCall {
            id: "call_1".into(),
            tool_name: "ask_secret".into(),
            arguments: serde_json::json!({
                "secrets": [{"question": "What is your username?", "name": "username"}]
            }),
        },
    ]);

    // Act: run the smoke graph with this adapter wired in.
    let outcome = run_graph_with_adapter(
        "tests/graphs/advanced/llm_tool_suspend_smoke.json",
        adapter,
        "agent_test_suspend_propagate_001",
    ).await;

    // Assert: the run is SUSPENDED at the DAG level with the expected questions.
    assert_eq!(outcome.status(), "SUSPENDED");
    let questions = outcome.questions();
    assert_eq!(questions.len(), 1);
    assert_eq!(questions[0]["question"], "What is your username?");
}

#[tokio::test]
async fn resume_replays_and_completes() {
    // Arrange: TWO scripted entries — first is the tool_call (drives the
    // suspend), second is the post-tool-result text response that closes
    // the agent loop.
    let adapter = ScriptedAdapter::new(vec![
        ScriptedResponse::ToolCall {
            id: "call_1".into(),
            tool_name: "ask_secret".into(),
            arguments: serde_json::json!({
                "secrets": [{"question": "User?", "name": "u"}]
            }),
        },
        ScriptedResponse::Text("Saved username.".into()),
    ]);

    let agent_id = "agent_test_resume_replay_001";

    // Run 1: suspend.
    let outcome1 = run_graph_with_adapter(
        "tests/graphs/advanced/llm_tool_suspend_smoke.json",
        adapter,
        agent_id,
    ).await;
    assert_eq!(outcome1.status(), "SUSPENDED");

    // Run 2: resume with Q/A answer. The adapter for run 2 should ONLY contain
    // the follow-up text — the tool re-execution doesn't drive a new LLM call
    // until after the tool result is folded back in.
    //
    // (Adjust per how run_graph_with_adapter handles persistence between runs;
    //  the harness may re-use the same adapter or build a new one.)
    let resume_adapter = ScriptedAdapter::new(vec![
        ScriptedResponse::Text("Saved username.".into()),
    ]);

    let outcome2 = resume_graph_with_adapter(
        "tests/graphs/advanced/llm_tool_suspend_smoke.json",
        resume_adapter,
        agent_id,
        "Q1: User?\nA1: alice",
    ).await;

    assert_eq!(outcome2.status(), "COMPLETED");
}

#[tokio::test]
async fn multiple_secrets_resolved_via_qa_format() {
    let adapter = ScriptedAdapter::new(vec![
        ScriptedResponse::ToolCall {
            id: "call_1".into(),
            tool_name: "ask_secret".into(),
            arguments: serde_json::json!({
                "secrets": [
                    {"question": "User?", "name": "u"},
                    {"question": "Pass?", "name": "p"}
                ]
            }),
        },
        ScriptedResponse::Text("Saved both credentials.".into()),
    ]);

    let agent_id = "agent_test_multi_secret_001";

    let outcome1 = run_graph_with_adapter(
        "tests/graphs/advanced/llm_tool_suspend_smoke.json",
        adapter,
        agent_id,
    ).await;
    assert_eq!(outcome1.status(), "SUSPENDED");
    assert_eq!(outcome1.questions().len(), 2);

    let resume_adapter = ScriptedAdapter::new(vec![
        ScriptedResponse::Text("Saved both credentials.".into()),
    ]);
    let outcome2 = resume_graph_with_adapter(
        "tests/graphs/advanced/llm_tool_suspend_smoke.json",
        resume_adapter,
        agent_id,
        "Q1: User?\nA1: alice\nQ2: Pass?\nA2: hunter2",
    ).await;
    assert_eq!(outcome2.status(), "COMPLETED");

    // Verify both secure values landed in the DB keyed by agent_session_id.
    // (Use whatever the existing test harness exposes for this — likely
    //  a helper on the test fixture or direct sqlx query.)
    let stored = list_secure_handles_for_agent(agent_id).await;
    assert!(stored.iter().any(|h| h.contains("<sv_u>")));
    assert!(stored.iter().any(|h| h.contains("<sv_p>")));
}
```

The functions `run_graph_with_adapter`, `resume_graph_with_adapter`, and `list_secure_handles_for_agent` likely don't exist as named — the implementer must either:
- Use whatever the existing integration harness exposes (look at `secure_suspend_integration.rs` for the pattern), OR
- Wrap the engine entrypoint inline in this file.

The point is: the test is responsible for wiring the `ScriptedAdapter` into the engine's `LlmProviderFactory` (or the equivalent override hook) for the duration of the test.

- [ ] **Step 3: Run the deterministic tests**

Run: `cargo test -p colmena_dag_engine --test llm_tool_suspend_integration -- --ignored`
Expected: 3 tests pass. Setup likely needs `DATABASE_URL`; if so add `#[ignore = "requires DATABASE_URL"]` to each test and document it.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/tests/llm_tool_suspend_integration.rs
git commit -m "$(cat <<'EOF'
test(llm): deterministic coverage of llm_call SUSPENDED propagation

Replaces the deferred stub with three integration tests driven by
ScriptedAdapter: suspend propagation, resume-replay, and multi-secret
resolution via the Q/A format.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4 — Real-LLM smoke test (Gemini Flash, `#[ignore]`)

**Files:**
- Create: `src/libs/colmena/tests/llm_tool_suspend_real.rs`

- [ ] **Step 1: Author the test**

```rust
//! Real-LLM smoke test for `llm_call` SUSPENDED propagation.
//!
//! Marked `#[ignore]` so CI doesn't burn API quota. Run locally with:
//! `source .env && cargo test --test llm_tool_suspend_real -- --ignored`

#[tokio::test]
#[ignore = "requires GEMINI_API_KEY and DATABASE_URL — run with `cargo test -- --ignored`"]
async fn real_llm_drives_suspend_and_resumes() {
    // Force the real Gemini provider in the smoke graph by overriding the
    // `provider` field at load time, OR maintain a sibling graph that pins
    // Gemini Flash.
    //
    // Decide based on what's easiest in the current test harness — both
    // are fine. The test below assumes graph patching.

    let agent_id = "agent_real_suspend_001";

    let outcome1 = run_graph_real_llm(
        "tests/graphs/advanced/llm_tool_suspend_smoke.json",
        // Patch: force provider=gemini, model=gemini-2.5-flash, system message
        // that strongly suggests the tool.
        agent_id,
        "Set up an HTTP basic-auth login against https://httpbin.org. \
         Collect username and password from the user via ask_secret.",
    ).await;

    assert_eq!(outcome1.status(), "SUSPENDED");
    let questions = outcome1.questions();
    assert!(
        questions.len() >= 2,
        "expected at least 2 questions (username + password), got {}",
        questions.len()
    );

    // Resume with Q/A format. We don't care exactly what the model asked;
    // we just echo Q1/Q2 back and provide values.
    let answer = format!(
        "Q1: {}\nA1: alice\nQ2: {}\nA2: hunter2",
        questions[0]["question"].as_str().unwrap_or(""),
        questions[1]["question"].as_str().unwrap_or(""),
    );

    let outcome2 = resume_graph_real_llm(
        "tests/graphs/advanced/llm_tool_suspend_smoke.json",
        agent_id,
        &answer,
    ).await;

    assert_eq!(outcome2.status(), "COMPLETED");
}
```

`run_graph_real_llm` and `resume_graph_real_llm` are implementation-detail helpers that thinly wrap the existing CLI/engine entrypoint, with the graph's `provider` field patched to `"gemini"`. Use whichever pattern the codebase already favors.

- [ ] **Step 2: Run the test locally**

Run: `source .env && cargo test --test llm_tool_suspend_real -- --ignored`
Expected: passes against live Gemini Flash. If the model decides not to use the tool (rare but possible), tweak the system message until it reliably does, then commit.

- [ ] **Step 3: Commit**

```bash
git add src/libs/colmena/tests/llm_tool_suspend_real.rs
git commit -m "$(cat <<'EOF'
test(llm): real-Gemini smoke test for SUSPENDED tool flow

Single #[ignore]'d integration test that exercises llm_call +
secure_suspend against live Gemini Flash to catch drift between our
trait-level abstractions and what the provider actually emits.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 5 — Final verification

- [ ] **Step 1: Full local test suite**

Run: `source .env && cargo test --verbose -- --include-ignored 2>&1 | tail -40`
Expected: all tests pass, including the new scripted and real ones.

- [ ] **Step 2: CI-equivalent run (no `--ignored`)**

Run: `cargo test --verbose 2>&1 | tail -20`
Expected: all non-ignored tests pass. The real Gemini test stays skipped.

- [ ] **Step 3: Lint**

Run: `cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: clean.
