# LLM Tool Suspend — Integration Test Coverage

**Date:** 2026-05-08
**Status:** Design
**Scope:** Test infrastructure for `llm_call` SUSPENDED propagation (Spec 5)

## Problem

Spec 5 (`llm_call` propagates SUSPENDED) was validated only by manual e2e testing against live Gemini. The deferred test stub in `tests/llm_tool_suspend_integration.rs` cannot run because `MockAdapter` always returns plain text — it cannot emit a tool_call response on demand.

Two consequences:

1. **No CI coverage** for the SUSPENDED short-circuit in `agent_service` or the resume-replay path in `llm.rs`.
2. **No way to assert engine behavior** without burning real LLM API calls and tolerating model nondeterminism.

## Solution: Two-Track Coverage

### Track 1 — `ScriptedAdapter`

A new test-only LLM adapter that emits a pre-recorded sequence of responses, including tool_calls.

- Lives in `src/libs/colmena/src/llm/infrastructure/scripted_adapter.rs` behind `#[cfg(any(test, feature = "test-utils"))]` (or simply marked as `pub(crate)` and reused via the `tests/` integration harness — final placement decided at implementation time).
- Constructor: `ScriptedAdapter::new(script: Vec<ScriptedResponse>)`.
- Each `call()` invocation pops the next response from the script. If the script is exhausted, returns an error.
- `ScriptedResponse` variants:
  - `Text(String)` — content-only response
  - `ToolCall { id: String, tool_name: String, arguments: serde_json::Value }` — single tool_call response
  - `ToolCallThenText { tool_call: {...}, follow_up_text: String }` — convenience for "tool call, then final answer after the tool result is fed back"
- Integrates with the existing `LlmRepository` trait — drop-in replacement wherever `MockAdapter` is used today.

The adapter is general — usable for any future test that needs scripted LLM behavior (lazy tool loading, multi-turn agent loops, error injection).

### Track 2 — Real Integration Test (Gemini Flash)

A real-LLM integration test marked `#[ignore = "requires GEMINI_API_KEY"]`:

- Runs the canvas-builder-shaped flow (or a minimal equivalent: `llm_call` with `secure_suspend` registered as a tool).
- Drives with a system prompt that instructs the model to use the tool for the user's request (e.g. "the user wants to set up a connection — call ask_secret to collect credentials").
- Asserts the DAG suspends with the expected questions.
- Resumes with `--answer` (Q/A format), asserts the run completes and the secure values resolved correctly.

Marked `#[ignore]` so CI doesn't burn API quota. Run locally with `source .env && cargo test -- --ignored llm_tool_suspend_real`.

**Both tracks are needed:** the scripted adapter gives deterministic CI coverage of the engine's plumbing; the real integration test catches drift between our assumptions about provider tool-call semantics and what providers actually emit (especially after model upgrades).

## Track 1 — Scripted Adapter Test Coverage

The deterministic test (`tests/llm_tool_suspend_integration.rs`) exercises:

1. **Suspend propagation:** Script: tool_call to `ask_secret` with valid args. Engine: should detect `__colmena_status: SUSPENDED` in the tool result, short-circuit `agent_service`, and surface SUSPENDED at the DAG level.
2. **Resume replay:** Same setup, then resume with Q/A `--answer`. Script's second entry: `Text("Done — credentials saved.")`. Assert: tool re-executed with the user's answer, tool result merged, final assistant message produced.
3. **Multiple secrets in one tool call:** Script: tool_call with `secrets: [{question:"Q1?",name:"u"},{question:"Q2?",name:"p"}]`. Resume with two Q/A pairs. Assert both handles persisted.

## Track 2 — Real Test Coverage

A single happy-path test that exercises the full stack:

1. Configure `llm_call` with Gemini Flash + `secure_suspend` as a tool.
2. Trigger with a prompt that strongly suggests using the tool ("collect username and password to log in to https://httpbin.org/basic-auth/...").
3. Assert: DAG suspends with at least 2 questions in the `questions` array.
4. Resume with Q/A formatted answers.
5. Assert: run completes, terminal message is produced, secure values exist in DB keyed by the agent_session_id.

No exhaustive scenarios — the scripted adapter covers those. The real test only verifies "the wiring still works against a real provider."

## Out of Scope

- Mocking specific LLM providers' wire formats. The scripted adapter operates at the `LlmRepository` trait level (post-deserialization).
- Coverage for non-suspend tool calls. Existing tests already cover regular tool calling.
- Streaming tool calls. (`stream()` in `ScriptedAdapter` returns the same scripted content as a single chunk; richer streaming tests are future work.)
