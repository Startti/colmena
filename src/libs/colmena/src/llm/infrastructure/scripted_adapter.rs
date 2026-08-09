//! Test-only LLM adapter that emits a pre-recorded sequence of responses.
//!
//! Use to deterministically drive engine code paths that depend on specific
//! LLM behaviors (text replies, tool_calls) without burning real API quota
//! or tolerating model nondeterminism.

use crate::llm::domain::tools::{FunctionCall, ToolCall};
use crate::llm::domain::{
    LlmError, LlmRepository, LlmRequest, LlmResponse, LlmStream, LlmStreamChunk, LlmStreamPart,
    ToolCallChunk,
};
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Mutex;

#[derive(Debug, Clone)]
pub enum ScriptedResponse {
    /// A plain text content response (no tool calls).
    Text(String),
    /// A tool-call response. `arguments` is a JSON Value; the adapter
    /// serializes it to the JSON string that the engine expects.
    ToolCall {
        id: String,
        tool_name: String,
        arguments: Value,
    },
}

/// LLM adapter that replays a pre-recorded queue of responses, popping the
/// next one on each `call()`. Returns an error when the script is exhausted.
///
/// `stream()` is intentionally unsupported — tests that need streaming
/// should use a real provider or extend this adapter explicitly.
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

    /// Number of script entries that have not been consumed.
    pub fn remaining(&self) -> usize {
        self.queue
            .lock()
            .expect("scripted_adapter mutex poisoned")
            .len()
    }
}

#[async_trait]
impl LlmRepository for ScriptedAdapter {
    async fn call(&self, request: LlmRequest) -> Result<LlmResponse, LlmError> {
        let next = self
            .queue
            .lock()
            .expect("scripted_adapter mutex poisoned")
            .pop()
            .ok_or_else(|| LlmError::RequestFailed {
                message: "scripted_adapter: script exhausted".to_string(),
            })?;

        match &next {
            ScriptedResponse::Text(t) => tracing::info!(
                target: "colmena::scripted_adapter",
                kind = "text",
                preview = %t.chars().take(40).collect::<String>(),
                remaining = self.remaining(),
                "scripted_adapter: yielding"
            ),
            ScriptedResponse::ToolCall { tool_name, .. } => tracing::info!(
                target: "colmena::scripted_adapter",
                kind = "tool_call",
                tool = %tool_name,
                remaining = self.remaining(),
                "scripted_adapter: yielding"
            ),
        }

        match next {
            ScriptedResponse::Text(t) => {
                LlmResponse::new(request.id().clone(), t, request.config().provider().clone())
            }
            ScriptedResponse::ToolCall {
                id,
                tool_name,
                arguments,
            } => {
                let args_str = super::tool_args::serialize_tool_args(&arguments, &tool_name);
                let function = FunctionCall::new(tool_name, args_str);
                let tool_call = ToolCall::new(id, function);
                let resp = LlmResponse::new(
                    request.id().clone(),
                    String::new(),
                    request.config().provider().clone(),
                )?;
                Ok(resp.with_tool_calls(vec![tool_call]))
            }
        }
    }

    async fn stream(&self, request: LlmRequest) -> Result<LlmStream, LlmError> {
        let next = self
            .queue
            .lock()
            .expect("scripted_adapter mutex poisoned")
            .pop()
            .ok_or_else(|| LlmError::RequestFailed {
                message: "scripted_adapter: script exhausted".to_string(),
            })?;

        let provider = request.config().provider().clone();
        let req_id = request.id().clone();

        // Build the stream chunks for this scripted entry. We emit a single
        // representative chunk per entry — enough to drive the agent_service
        // accumulator in `application/agent_service.rs`.
        let chunks: Vec<Result<LlmStreamChunk, LlmError>> = match next {
            ScriptedResponse::Text(t) => vec![Ok(LlmStreamChunk::new(
                req_id,
                LlmStreamPart::Content(t),
                provider,
                true,
            ))],
            ScriptedResponse::ToolCall {
                id,
                tool_name,
                arguments,
            } => {
                let args_str = super::tool_args::serialize_tool_args(&arguments, &tool_name);
                vec![Ok(LlmStreamChunk::new(
                    req_id,
                    LlmStreamPart::ToolCallChunk(ToolCallChunk {
                        index: 0,
                        id,
                        name: tool_name,
                        args_chunk: args_str,
                        provider_signature: None,
                    }),
                    provider,
                    true,
                ))]
            }
        };

        let s = futures::stream::iter(chunks);
        Ok(Box::pin(s))
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
    use crate::llm::domain::{LlmConfig, LlmMessage, LlmProvider, ProviderKind};

    fn make_request(text: &str) -> LlmRequest {
        let provider = LlmProvider::new(
            ProviderKind::Mock,
            "mock-api-key".to_string(),
            Some("mock-model".to_string()),
        )
        .expect("valid provider");
        let config = LlmConfig::new(provider);
        let user_msg = LlmMessage::user(text.to_string()).expect("valid user message");
        LlmRequest::new(vec![user_msg], config, false).expect("valid request")
    }

    #[tokio::test]
    async fn yields_text_response() {
        let adapter = ScriptedAdapter::new(vec![ScriptedResponse::Text("hello".into())]);
        let resp = adapter.call(make_request("hi")).await.unwrap();
        assert_eq!(resp.content(), "hello");
        assert!(resp.tool_calls().is_none() || resp.tool_calls().unwrap().is_empty());
    }

    #[tokio::test]
    async fn yields_tool_call_with_serialized_arguments() {
        let adapter = ScriptedAdapter::new(vec![ScriptedResponse::ToolCall {
            id: "call_1".into(),
            tool_name: "ask_secret".into(),
            arguments: serde_json::json!({"secrets": [{"question":"?","name":"u"}]}),
        }]);
        let resp = adapter.call(make_request("hi")).await.unwrap();
        let tcs = resp.tool_calls().expect("must have tool calls");
        assert_eq!(tcs.len(), 1);
        assert_eq!(tcs[0].id, "call_1");
        assert_eq!(tcs[0].function.name, "ask_secret");
        // arguments must be a JSON string, not a Value.
        let parsed: serde_json::Value = serde_json::from_str(&tcs[0].function.arguments).unwrap();
        assert_eq!(parsed["secrets"][0]["name"], "u");
    }

    #[tokio::test]
    async fn yields_responses_in_script_order() {
        let adapter = ScriptedAdapter::new(vec![
            ScriptedResponse::Text("first".into()),
            ScriptedResponse::Text("second".into()),
            ScriptedResponse::ToolCall {
                id: "t1".into(),
                tool_name: "echo".into(),
                arguments: serde_json::json!({}),
            },
        ]);

        let r1 = adapter.call(make_request("a")).await.unwrap();
        assert_eq!(r1.content(), "first");

        let r2 = adapter.call(make_request("b")).await.unwrap();
        assert_eq!(r2.content(), "second");

        let r3 = adapter.call(make_request("c")).await.unwrap();
        assert_eq!(r3.tool_calls().unwrap()[0].function.name, "echo");
    }

    #[tokio::test]
    async fn errors_when_script_exhausted() {
        let adapter = ScriptedAdapter::new(vec![ScriptedResponse::Text("only".into())]);
        let _ = adapter.call(make_request("a")).await.unwrap();
        let err = adapter.call(make_request("b")).await.unwrap_err();
        assert!(format!("{err}").contains("exhausted"));
    }

    #[tokio::test]
    async fn remaining_decrements_per_call() {
        let adapter = ScriptedAdapter::new(vec![
            ScriptedResponse::Text("a".into()),
            ScriptedResponse::Text("b".into()),
        ]);
        assert_eq!(adapter.remaining(), 2);
        let _ = adapter.call(make_request("x")).await.unwrap();
        assert_eq!(adapter.remaining(), 1);
        let _ = adapter.call(make_request("y")).await.unwrap();
        assert_eq!(adapter.remaining(), 0);
    }

    #[tokio::test]
    async fn stream_emits_text_chunk() {
        use futures::StreamExt;
        let adapter = ScriptedAdapter::new(vec![ScriptedResponse::Text("hi".into())]);
        let mut s = adapter.stream(make_request("q")).await.expect("stream ok");
        let first = s.next().await.expect("one chunk").expect("ok");
        match first.part() {
            LlmStreamPart::Content(c) => assert_eq!(c, "hi"),
            other => panic!("expected Content, got {other:?}"),
        }
        assert!(s.next().await.is_none());
    }

    #[tokio::test]
    async fn stream_emits_tool_call_chunk() {
        use futures::StreamExt;
        let adapter = ScriptedAdapter::new(vec![ScriptedResponse::ToolCall {
            id: "t1".into(),
            tool_name: "echo".into(),
            arguments: serde_json::json!({"x": 1}),
        }]);
        let mut s = adapter.stream(make_request("q")).await.expect("stream ok");
        let first = s.next().await.expect("one chunk").expect("ok");
        match first.part() {
            LlmStreamPart::ToolCallChunk(tc) => {
                assert_eq!(tc.id, "t1");
                assert_eq!(tc.name, "echo");
                let parsed: serde_json::Value = serde_json::from_str(&tc.args_chunk).unwrap();
                assert_eq!(parsed["x"], 1);
            }
            other => panic!("expected ToolCallChunk, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn stream_errors_when_exhausted() {
        let adapter = ScriptedAdapter::new(vec![]);
        let err = match adapter.stream(make_request("a")).await {
            Ok(_) => panic!("expected error when script is exhausted"),
            Err(e) => e,
        };
        assert!(format!("{err}").contains("exhausted"));
    }
}
