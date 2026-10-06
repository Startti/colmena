//! An `LlmRepository` that bills each of its calls to the node that made it.
//!
//! Hosts bill from the `NodeEvent::LlmUsage` events a node emits. The answer
//! loop (`AgentService`) reports its calls through its stream callback, but a
//! node also calls the provider on the side, with no callback: the cheap-model
//! summary of an old turn (history compaction), the summary of an attachment,
//! the SQL guardrail critic. Those calls go through this wrapper, which
//! reports each one's usage once, as it completes.
//!
//! Only side calls go through it: the answer loop's calls are already
//! reported, and wrapping its repository too would bill them twice.

use crate::dag_engine::domain::observer::{ExecutionObserver, NodeEvent};
use crate::llm::domain::{
    LlmError, LlmRepository, LlmRequest, LlmResponse, LlmStream, LlmStreamPart, LlmUsage,
};
use async_trait::async_trait;
use futures::StreamExt;
use std::sync::{Arc, Mutex};

struct BilledLlm {
    inner: Arc<dyn LlmRepository>,
    observer: Arc<dyn ExecutionObserver>,
}

/// `inner`, reporting the usage of each call it makes to `observer`. Without
/// an observer there is no one to bill, and `inner` comes back as it is.
pub fn billed(
    inner: Arc<dyn LlmRepository>,
    observer: Option<Arc<dyn ExecutionObserver>>,
) -> Arc<dyn LlmRepository> {
    match observer {
        Some(observer) => Arc::new(BilledLlm { inner, observer }),
        None => inner,
    }
}

#[async_trait]
impl LlmRepository for BilledLlm {
    async fn call(&self, request: LlmRequest) -> Result<LlmResponse, LlmError> {
        let response = self.inner.call(request).await?;
        if let Some(usage) = response.usage() {
            self.observer.on_event(NodeEvent::llm_usage(usage));
        }
        Ok(response)
    }

    /// Reports the last `Usage` part when the stream ends: a provider may
    /// stream cumulative ones, and only the last is the call's total.
    async fn stream(&self, request: LlmRequest) -> Result<LlmStream, LlmError> {
        let stream = self.inner.stream(request).await?;
        let last: Arc<Mutex<Option<LlmUsage>>> = Arc::default();
        let seen = last.clone();
        let parts = stream.inspect(move |chunk| {
            if let Ok(chunk) = chunk {
                if let LlmStreamPart::Usage(usage) = chunk.part() {
                    *seen.lock().unwrap_or_else(|p| p.into_inner()) = Some(usage.clone());
                }
            }
        });
        let observer = self.observer.clone();
        let report = futures::stream::once(async move {
            let usage = last.lock().unwrap_or_else(|p| p.into_inner()).take();
            if let Some(usage) = usage {
                observer.on_event(NodeEvent::llm_usage(&usage));
            }
        })
        .filter_map(|()| async { None });
        Ok(Box::pin(parts.chain(report)))
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        self.inner.health_check().await
    }

    async fn validate_credentials(&self, api_key: &str) -> Result<(), LlmError> {
        self.inner.validate_credentials(api_key).await
    }

    fn provider_name(&self) -> &'static str {
        self.inner.provider_name()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::domain::{
        LlmConfig, LlmMessage, LlmProvider, LlmRequestId, LlmStreamChunk, MockLlmRepository,
        ProviderKind,
    };

    #[derive(Default)]
    struct Recorder(Mutex<Vec<NodeEvent>>);

    impl ExecutionObserver for Recorder {
        fn on_event(&self, event: NodeEvent) {
            self.0.lock().unwrap().push(event);
        }
    }

    impl Recorder {
        fn usages(&self) -> Vec<u32> {
            let events = self.0.lock().unwrap();
            let usages = events.iter().filter_map(|e| match e {
                NodeEvent::LlmUsage { prompt_tokens, .. } => Some(*prompt_tokens),
                _ => None,
            });
            usages.collect()
        }
    }

    fn provider() -> LlmProvider {
        LlmProvider::new(ProviderKind::Mock, "k".into(), Some("m".into())).unwrap()
    }

    fn request() -> LlmRequest {
        let messages = vec![LlmMessage::user("hi".into()).unwrap()];
        LlmRequest::new(messages, LlmConfig::new(provider()), false).unwrap()
    }

    fn chunk(part: LlmStreamPart) -> Result<LlmStreamChunk, LlmError> {
        let id = LlmRequestId::from_string("r".into()).unwrap();
        Ok(LlmStreamChunk::new(id, part, provider(), false))
    }

    #[tokio::test]
    async fn a_call_is_billed_once_with_its_usage() {
        let mut inner = MockLlmRepository::new();
        inner.expect_call().times(1).returning(|_| {
            let id = LlmRequestId::from_string("r".into()).unwrap();
            let response = LlmResponse::new(id, "ok".into(), provider()).unwrap();
            Ok(response.with_usage(LlmUsage::new(40, 2)))
        });
        let recorder = Arc::new(Recorder::default());
        let repo = billed(Arc::new(inner), Some(recorder.clone()));
        repo.call(request()).await.unwrap();
        assert_eq!(recorder.usages(), vec![40]);
    }

    #[tokio::test]
    async fn a_stream_is_billed_once_with_its_last_usage() {
        let mut inner = MockLlmRepository::new();
        inner.expect_stream().times(1).returning(|_| {
            let parts = [
                LlmStreamPart::Usage(LlmUsage::new(1, 1)),
                LlmStreamPart::Content("ok".into()),
                LlmStreamPart::Usage(LlmUsage::new(40, 2)),
            ];
            let chunks: Vec<_> = parts.into_iter().map(chunk).collect();
            Ok(Box::pin(futures::stream::iter(chunks)) as LlmStream)
        });
        let recorder = Arc::new(Recorder::default());
        let repo = billed(Arc::new(inner), Some(recorder.clone()));
        let parts: Vec<_> = repo.stream(request()).await.unwrap().collect().await;
        assert_eq!(parts.len(), 3, "every part passes through");
        assert_eq!(recorder.usages(), vec![40]);
    }
}
