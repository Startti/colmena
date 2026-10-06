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
//! reported, and wrapping its repository too would bill them twice. A side
//! call often uses a cheaper model than the node, and hosts price each usage
//! entry by its model, so its usage goes on an entry of its own,
//! `<node_id>::<purpose>`, with the model of the request (`SideCall`).

use crate::dag_engine::domain::observer::{ExecutionObserver, NodeEvent, SideCall};
use crate::llm::domain::{
    LlmError, LlmRepository, LlmRequest, LlmResponse, LlmStream, LlmStreamPart, LlmUsage,
};
use async_trait::async_trait;
use futures::StreamExt;
use std::sync::{Arc, Mutex};

/// Why a node calls the provider outside its answer loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidePurpose {
    /// The summary of an old turn (`llm_call`, with the node's key).
    HistoryCompaction,
    /// The summary of an attachment (`llm_call`, with the node's key).
    AttachmentSummary,
    /// The `guardrail_llm` critic of a `sql` node (with its own key).
    SqlGuardrail,
}

impl SidePurpose {
    fn side_call(self, request: &LlmRequest) -> SideCall {
        let provider = request.config().provider();
        let (purpose, node_key) = match self {
            Self::HistoryCompaction => ("history_compaction", true),
            Self::AttachmentSummary => ("attachment_summary", true),
            Self::SqlGuardrail => ("sql_guardrail", false),
        };
        SideCall {
            purpose: purpose.into(),
            model: provider.model().to_string(),
            provider: provider.kind().to_string(),
            node_key,
        }
    }
}

struct BilledLlm {
    inner: Arc<dyn LlmRepository>,
    observer: Arc<dyn ExecutionObserver>,
    purpose: SidePurpose,
}

/// `inner`, reporting the usage of each call it makes to `observer`, as a
/// side call for `purpose`. Without an observer there is no one to bill, and
/// `inner` comes back as it is.
pub fn billed(
    inner: Arc<dyn LlmRepository>,
    observer: Option<Arc<dyn ExecutionObserver>>,
    purpose: SidePurpose,
) -> Arc<dyn LlmRepository> {
    match observer {
        Some(observer) => Arc::new(BilledLlm {
            inner,
            observer,
            purpose,
        }),
        None => inner,
    }
}

#[async_trait]
impl LlmRepository for BilledLlm {
    async fn call(&self, request: LlmRequest) -> Result<LlmResponse, LlmError> {
        let side_call = self.purpose.side_call(&request);
        let response = self.inner.call(request).await?;
        if let Some(usage) = response.usage() {
            let event = NodeEvent::side_llm_usage(usage, side_call);
            self.observer.on_event(event);
        }
        Ok(response)
    }

    /// Reports the last `Usage` part once: when the stream ends (a provider
    /// may stream cumulative ones, and only the last is the call's total), or
    /// when it is dropped before it ends (a cancel cuts the call; the provider
    /// bills what it already sent).
    async fn stream(&self, request: LlmRequest) -> Result<LlmStream, LlmError> {
        let side_call = self.purpose.side_call(&request);
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
        let report = ReportOnDrop {
            last,
            observer: self.observer.clone(),
            side_call: Some(side_call),
        };
        // Reached at the end, or dropped unpolled with the stream: either way
        // `report` drops, once.
        let end =
            futures::stream::once(async move { drop(report) }).filter_map(|()| async { None });
        Ok(Box::pin(parts.chain(end)))
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

/// Reports a side call's last usage to `observer` when dropped.
struct ReportOnDrop {
    last: Arc<Mutex<Option<LlmUsage>>>,
    observer: Arc<dyn ExecutionObserver>,
    side_call: Option<SideCall>,
}

impl Drop for ReportOnDrop {
    fn drop(&mut self) {
        let usage = self.last.lock().unwrap_or_else(|p| p.into_inner()).take();
        if let (Some(usage), Some(side_call)) = (usage, self.side_call.take()) {
            let event = NodeEvent::side_llm_usage(&usage, side_call);
            self.observer.on_event(event);
        }
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
        /// `(prompt_tokens, side call)` of each usage event.
        fn usages(&self) -> Vec<(u32, Option<SideCall>)> {
            let events = self.0.lock().unwrap();
            let usages = events.iter().filter_map(|e| match e {
                NodeEvent::LlmUsage {
                    prompt_tokens,
                    side_call,
                    ..
                } => Some((*prompt_tokens, side_call.clone())),
                _ => None,
            });
            usages.collect()
        }
    }

    /// What a call of `request()` reports.
    fn side(purpose: &str, node_key: bool) -> Option<SideCall> {
        Some(SideCall {
            purpose: purpose.into(),
            model: "m".into(),
            provider: "mock".into(),
            node_key,
        })
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
        let repo = billed(
            Arc::new(inner),
            Some(recorder.clone()),
            SidePurpose::SqlGuardrail,
        );
        repo.call(request()).await.unwrap();
        assert_eq!(recorder.usages(), vec![(40, side("sql_guardrail", false))]);
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
        let purpose = SidePurpose::HistoryCompaction;
        let repo = billed(Arc::new(inner), Some(recorder.clone()), purpose);
        let parts: Vec<_> = repo.stream(request()).await.unwrap().collect().await;
        assert_eq!(parts.len(), 3, "every part passes through");
        let expected = vec![(40, side("history_compaction", true))];
        assert_eq!(recorder.usages(), expected);
    }

    #[tokio::test]
    async fn a_stream_dropped_mid_way_is_billed_once_with_its_last_usage() {
        let mut inner = MockLlmRepository::new();
        inner.expect_stream().times(1).returning(|_| {
            let parts = [
                LlmStreamPart::Usage(LlmUsage::new(40, 0)),
                LlmStreamPart::Usage(LlmUsage::new(40, 9)),
            ];
            let chunks: Vec<_> = parts.into_iter().map(chunk).collect();
            let hang = futures::stream::pending();
            Ok(Box::pin(futures::stream::iter(chunks).chain(hang)) as LlmStream)
        });
        let recorder = Arc::new(Recorder::default());
        let purpose = SidePurpose::AttachmentSummary;
        let repo = billed(Arc::new(inner), Some(recorder.clone()), purpose);
        let mut stream = repo.stream(request()).await.unwrap();
        stream.next().await;
        stream.next().await;
        assert!(recorder.usages().is_empty(), "nothing before the cut");
        drop(stream);
        let events = recorder.0.lock().unwrap();
        let completion = events.iter().filter_map(|e| match e {
            NodeEvent::LlmUsage {
                completion_tokens, ..
            } => Some(*completion_tokens),
            _ => None,
        });
        assert_eq!(completion.collect::<Vec<_>>(), vec![9]);
    }
}
