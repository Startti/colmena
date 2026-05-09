//! Test-only LLM adapter that emits a pre-recorded sequence of responses.
//!
//! Use to deterministically drive engine code paths that depend on specific
//! LLM behaviors (text replies, tool_calls) without burning real API quota
//! or tolerating model nondeterminism.

use crate::llm::domain::tools::{FunctionCall, ToolCall};
use crate::llm::domain::{LlmError, LlmRepository, LlmRequest, LlmResponse, LlmStream};
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
                let args_str = serde_json::to_string(&arguments).unwrap_or_default();
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

    async fn stream(&self, _request: LlmRequest) -> Result<LlmStream, LlmError> {
        Err(LlmError::RequestFailed {
            message: "scripted_adapter: streaming is not supported".to_string(),
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
        let parsed: serde_json::Value =
            serde_json::from_str(&tcs[0].function.arguments).unwrap();
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
    async fn stream_returns_error() {
        let adapter = ScriptedAdapter::new(vec![]);
        let result = adapter.stream(make_request("a")).await;
        match result {
            Err(e) => assert!(format!("{e}").contains("not supported")),
            Ok(_) => panic!("expected stream() to return an error"),
        }
    }
}
