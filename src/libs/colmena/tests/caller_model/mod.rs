//! A scripted model that answers according to who calls it, for an E2E where
//! a parent agent and its child agents all reach the one process-global
//! override (`parallel_tool_suspend.rs`). The script sees each request (its
//! system prompt and its messages) and returns a reply and how long to wait
//! before giving it. Every request is kept, in arrival order.

use async_trait::async_trait;
use colmena::llm::domain::{
    LlmError, LlmMessage, LlmRepository, LlmRequest, LlmResponse, LlmStream, LlmStreamChunk,
    LlmStreamPart, MessageRole, ToolCallChunk,
};
use std::sync::Mutex;
use std::time::Duration;

/// A request as the model received it.
#[derive(Clone)]
pub struct Request {
    pub system: String,
    pub messages: Vec<LlmMessage>,
}

impl Request {
    fn of(request: &LlmRequest) -> Self {
        let messages = request.messages().to_vec();
        let system = messages
            .iter()
            .find(|m| m.role() == &MessageRole::System)
            .map(|m| m.content().to_string())
            .unwrap_or_default();
        Self { system, messages }
    }

    /// The role of the message the request ends on.
    pub fn last_role(&self) -> MessageRole {
        self.messages.last().map(|m| m.role().clone()).unwrap()
    }

    /// The content of the last message.
    pub fn last_content(&self) -> &str {
        self.messages.last().map(|m| m.content()).unwrap_or("")
    }

    /// The content of the last `user` message: the prompt of this run.
    pub fn last_user(&self) -> &str {
        self.messages
            .iter()
            .rev()
            .find(|m| m.role() == &MessageRole::User)
            .map(|m| m.content())
            .unwrap_or("")
    }
}

/// What the model answers: tool calls `(id, tool, arguments)`, in the order
/// it puts them in its message, or text.
pub enum Reply {
    Calls(Vec<(String, String, String)>),
    Text(String),
}

type Script = dyn Fn(&Request) -> (Duration, Reply) + Send + Sync;

pub struct CallerModel {
    script: Box<Script>,
    seen: Mutex<Vec<Request>>,
}

impl CallerModel {
    pub fn new(script: impl Fn(&Request) -> (Duration, Reply) + Send + Sync + 'static) -> Self {
        Self {
            script: Box::new(script),
            seen: Mutex::new(Vec::new()),
        }
    }

    /// Every request the model received, in arrival order.
    pub fn seen(&self) -> Vec<Request> {
        self.seen.lock().unwrap().clone()
    }

    async fn reply(&self, request: &LlmRequest) -> Reply {
        let seen = Request::of(request);
        self.seen.lock().unwrap().push(seen.clone());
        let (wait, reply) = (self.script)(&seen);
        tokio::time::sleep(wait).await;
        reply
    }
}

#[async_trait]
impl LlmRepository for CallerModel {
    /// Every agent of the graph streams (`stream: true`, and a run with an
    /// observer streams), so this is never reached.
    async fn call(&self, _request: LlmRequest) -> Result<LlmResponse, LlmError> {
        unimplemented!("every agent of the graph streams")
    }

    /// One chunk per call, each with its index, as a provider streams them.
    async fn stream(&self, request: LlmRequest) -> Result<LlmStream, LlmError> {
        let parts: Vec<LlmStreamPart> = match self.reply(&request).await {
            Reply::Text(text) => vec![LlmStreamPart::Content(text)],
            Reply::Calls(calls) => calls
                .into_iter()
                .enumerate()
                .map(|(index, (id, name, args_chunk))| {
                    LlmStreamPart::ToolCallChunk(ToolCallChunk {
                        index,
                        id,
                        name,
                        args_chunk,
                        provider_signature: None,
                    })
                })
                .collect(),
        };
        let last = parts.len() - 1;
        let chunks: Vec<Result<LlmStreamChunk, LlmError>> = parts
            .into_iter()
            .enumerate()
            .map(|(i, part)| {
                let provider = request.config().provider().clone();
                Ok(LlmStreamChunk::new(
                    request.id().clone(),
                    part,
                    provider,
                    i == last,
                ))
            })
            .collect();
        Ok(Box::pin(futures::stream::iter(chunks)))
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }

    fn provider_name(&self) -> &'static str {
        "scripted"
    }
}
