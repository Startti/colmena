//! The scripted model of the parallel tool call E2Es
//! (`parallel_tool_identity.rs`, `parallel_tool_groups.rs`).

use async_trait::async_trait;
use colmena::llm::domain::{
    LlmError, LlmRepository, LlmRequest, LlmResponse, LlmStream, LlmStreamChunk, LlmStreamPart,
    ToolCallChunk,
};
use std::sync::Mutex;

/// `(id, tool, arguments)` in the order the model puts them in its message.
pub type Call = (&'static str, &'static str, &'static str);

/// `ScriptedAdapter` answers one tool call per response; a parallel turn needs
/// several in the same message. First turn: `calls`, streamed as the provider
/// would (one chunk each, with its index). Then: text. The engine streams
/// whenever an observer is attached, so `call` is never reached.
pub struct ParallelTurnModel {
    calls: &'static [Call],
    answered: Mutex<bool>,
    results: Mutex<Vec<String>>,
}

impl ParallelTurnModel {
    pub fn new(calls: &'static [Call]) -> Self {
        Self {
            calls,
            answered: Mutex::new(false),
            results: Mutex::new(Vec::new()),
        }
    }

    /// The `tool_call_id` of every tool result in the history of the last
    /// request, in the order the engine wrote them.
    pub fn results_seen(&self) -> Vec<String> {
        self.results.lock().unwrap().clone()
    }
}

#[async_trait]
impl LlmRepository for ParallelTurnModel {
    async fn call(&self, _request: LlmRequest) -> Result<LlmResponse, LlmError> {
        unimplemented!("a run with an observer streams")
    }

    async fn stream(&self, request: LlmRequest) -> Result<LlmStream, LlmError> {
        let first = !std::mem::replace(&mut *self.answered.lock().unwrap(), true);
        *self.results.lock().unwrap() = request
            .messages()
            .iter()
            .filter_map(|m| m.tool_call_id().map(str::to_string))
            .collect();
        let parts: Vec<LlmStreamPart> = if first {
            self.calls
                .iter()
                .enumerate()
                .map(|(index, (id, tool, args))| {
                    LlmStreamPart::ToolCallChunk(ToolCallChunk {
                        index,
                        id: id.to_string(),
                        name: tool.to_string(),
                        args_chunk: args.to_string(),
                        provider_signature: None,
                    })
                })
                .collect()
        } else {
            vec![LlmStreamPart::Content("Listo.".into())]
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
