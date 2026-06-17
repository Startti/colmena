use futures::StreamExt;
use napi::bindgen_prelude::*;
use napi_derive::napi;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Async-iterator handle over an LLM text stream. Each `pull()` resolves to the
/// next chunk, or `null` when the stream is exhausted. The TS facade attaches
/// `[Symbol.asyncIterator]` so callers use `for await (const chunk of stream)`.
#[napi]
pub struct LlmStreamHandle {
    stream: Arc<Mutex<crate::llm::domain::LlmStream>>,
}

#[napi]
impl LlmStreamHandle {
    /// Returns the next text chunk, or `null` when the stream is exhausted.
    #[napi]
    pub async fn pull(&self) -> Result<Option<String>> {
        let mut stream = self.stream.lock().await;
        match stream.next().await {
            Some(Ok(chunk)) => Ok(Some(chunk.content().to_string())),
            Some(Err(e)) => Err(Error::new(Status::GenericFailure, e.to_string())),
            None => Ok(None),
        }
    }
}

impl LlmStreamHandle {
    pub fn new(stream: crate::llm::domain::LlmStream) -> Self {
        Self {
            stream: Arc::new(Mutex::new(stream)),
        }
    }
}
