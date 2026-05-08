//! Integration test for the LLM tool-call suspend → resume cycle (Gap 2).
//!
//! ## Goal
//! Run a graph with an `llm_call` node whose only tool is a suspendable stub.
//! On run 1 the LLM should call the tool, the tool returns
//! `__colmena_status: "SUSPENDED"`, and `agent_service` short-circuits propagating
//! SUSPENDED to the DAG. On run 2 with an answer the conversation is replayed and
//! the pending tool is re-dispatched with the user's answer.
//!
//! ## Why this test is DEFERRED
//!
//! Exercising the suspend path requires the LLM adapter to produce a `ToolCall` in
//! its streaming response (not plain text). The existing `MockAdapter` only returns
//! text strings — it has no mechanism for configurable tool-call responses:
//!
//! ```
//! // mock_adapter.rs — current behaviour
//! async fn stream(&self, request: LlmRequest) -> Result<LlmStream, LlmError> {
//!     let full_text = format!("Mock stream response to: {}", last_message.content());
//!     // ... emits only LlmStreamPart::Content chunks, never LlmStreamPart::ToolCall
//! }
//! ```
//!
//! Wiring a real LLM (OpenAI / Gemini / Anthropic) would make this test CI-hostile
//! (requires API keys, network, non-deterministic model output).
//!
//! To unblock Test B we need one of:
//!   1. Extend `MockAdapter` to accept a `Vec<MockResponse>` script (text OR tool_call)
//!      and replay them in sequence — then it can simulate turn 1 → tool_call → SUSPEND,
//!      turn 2 → text → COMPLETED.
//!   2. Add a new `ScriptedAdapter` implementing `LlmRepository` with pre-recorded
//!      responses, wired into the engine via a dedicated `"scripted"` provider kind.
//!
//! Both approaches are self-contained and do not require API keys, but they are their
//! own non-trivial implementation tasks. The suspend/resume path is already covered by
//! the manual end-to-end validation with `tests/graphs/advanced/secure_suspend_login_e2e.json`
//! (see commit bd47860 and the design doc `2026-05-08-llm-call-tool-suspend-design.md`).
//!
//! TODO: implement a scripted/mock LLM provider that can emit tool-call chunks, then
//! activate this test.
//!
//! Run with (once implemented):
//!   source .env && cargo test --test llm_tool_suspend_integration -- --ignored

/// Placeholder so the test binary compiles even though the test body is not yet written.
#[tokio::test]
#[ignore = "DEFERRED — MockAdapter cannot emit tool-call responses; see module-level doc for unblock plan"]
async fn llm_tool_suspend_and_resume_via_mock_provider() {
    // TODO: implement once a scripted LLM adapter is available.
    // Outline:
    //   1. Build a ScriptedAdapter with two scripted responses:
    //      - Turn 1: ToolCall { name: "ask_credential", arguments: "{}" }
    //      - Turn 2: Content("Login complete")
    //   2. Run a graph: input → llm_call (provider="scripted", tool=ask_credential) → log
    //      where ask_credential is backed by a secure_suspend node.
    //   3. Assert run 1 stream contains GraphFinish { __colmena_status: "SUSPENDED" }.
    //   4. Resume with --answer "mypassword".
    //   5. Assert run 2 stream contains GraphFinish without SUSPENDED, and the
    //      log node received the resolved secure-value handle or real value.
    todo!("implement ScriptedAdapter first — see module-level TODO");
}
