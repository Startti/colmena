use futures::StreamExt;
use napi::bindgen_prelude::*;
use napi_derive::napi;
use serde_json::Value;
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

/// Owned, `'static` stream of SSE-mapped DAG parts (each a `serde_json::Value`).
pub type DagPartStream = std::pin::Pin<
    Box<
        dyn futures::Stream<
                Item = std::result::Result<Value, crate::dag_engine::domain::error::DagError>,
            > + Send,
    >,
>;

/// Async-iterator handle over a running DAG's SSE-mapped events. Each `pull()`
/// resolves to the next `{ type: ... }` event, or `null` when the graph finishes.
#[napi]
pub struct DagStreamHandle {
    stream: Arc<Mutex<DagPartStream>>,
}

#[napi]
impl DagStreamHandle {
    /// Returns the next DAG event, or `null` when the graph finishes.
    #[napi]
    pub async fn pull(&self) -> Result<Option<Value>> {
        let mut stream = self.stream.lock().await;
        match stream.next().await {
            Some(Ok(part)) => Ok(Some(part)),
            Some(Err(e)) => Err(Error::new(Status::GenericFailure, e.to_string())),
            None => Ok(None),
        }
    }
}

impl DagStreamHandle {
    pub fn new(stream: DagPartStream) -> Self {
        Self {
            stream: Arc::new(Mutex::new(stream)),
        }
    }
}
