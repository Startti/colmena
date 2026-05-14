//! Adapter for `AttachmentSummaryGenerator` that issues a one-shot,
//! history-less `LlmRepository::call`. Bypasses `LlmCallUseCase` so
//! the summary turn never lands in `llm_node_history`.

use crate::llm::domain::attachments::{
    AttachmentSummaryGenerator, SummaryConfig, SummaryError, SummaryInput, SummaryOutcome,
    SummarySource,
};
use crate::llm::domain::{
    FileData, LlmConfig, LlmMessage, LlmProvider, LlmRepository, LlmRequest,
};
use async_trait::async_trait;
use std::sync::Arc;

const SYSTEM_PROMPT_TEXT: &str = "You are a document cataloger. Given the first N \
characters of a document's extracted text, output a single short description \
(max {MAX_CHARS} characters) that helps a downstream LLM decide whether this \
document is relevant to a user's question. Focus on: document type, topic, and \
time period if relevant. No commentary, no quotes, no markdown. Just the \
description on one line.";

const SYSTEM_PROMPT_IMAGE: &str = "You are a document cataloger. Look at the \
attached image and output a single short description (max {MAX_CHARS} characters) \
that helps a downstream LLM decide whether this image is relevant to a user's \
question. Focus on: subject, type of image, salient details. No commentary, no \
markdown. Just the description on one line.";

/// LLM-backed implementation of [`AttachmentSummaryGenerator`].
///
/// The generator holds an [`LlmRepository`] directly and bypasses
/// `LlmCallUseCase`, so the summary turn never lands in
/// `llm_node_history`. The `api_key` and `model` travel through the
/// `SummaryConfig` and are baked into a fresh [`LlmProvider`] per call.
pub struct LlmAttachmentSummaryGenerator {
    repo: Arc<dyn LlmRepository>,
}

impl LlmAttachmentSummaryGenerator {
    /// Construct from an [`LlmRepository`]. The caller (typically
    /// `llm.rs::execute`) builds the repo via `LlmProviderFactory::create`
    /// keyed by the **main call's** provider — keeping the summary on the
    /// same provider for single-API-key flows.
    pub fn new(repo: Arc<dyn LlmRepository>) -> Self {
        Self { repo }
    }
}

#[async_trait]
impl AttachmentSummaryGenerator for LlmAttachmentSummaryGenerator {
    async fn generate(
        &self,
        input: SummaryInput,
        config: &SummaryConfig,
    ) -> Result<SummaryOutcome, SummaryError> {
        // Build messages based on source type.
        let (system_text, user_msg) = match &input.source {
            SummarySource::ExtractedText(text) => {
                if text.trim().is_empty() {
                    return Ok(SummaryOutcome::Skipped {
                        reason: "extracted text was empty".into(),
                    });
                }
                let sys = SYSTEM_PROMPT_TEXT
                    .replace("{MAX_CHARS}", &config.max_output_chars.to_string());
                let usr = format!(
                    "Filename: {}\nMIME type: {}\nExtracted text (truncated):\n---\n{}\n---",
                    input.filename, input.mime_type, text
                );
                let msg = LlmMessage::user(usr)
                    .map_err(|e| SummaryError::LlmCallFailed(format!("build user msg: {}", e)))?;
                (sys, msg)
            }
            SummarySource::ImageBytes(bytes) => {
                let sys = SYSTEM_PROMPT_IMAGE
                    .replace("{MAX_CHARS}", &config.max_output_chars.to_string());
                let content = format!("Filename: {}", input.filename);
                let file = FileData::inline(
                    input.mime_type.clone(),
                    input.filename.clone(),
                    bytes.clone(),
                );
                let msg = LlmMessage::user_with_files(content, vec![file])
                    .map_err(|e| SummaryError::LlmCallFailed(format!("build user msg: {}", e)))?;
                (sys, msg)
            }
        };

        let system_msg = LlmMessage::system(system_text)
            .map_err(|e| SummaryError::LlmCallFailed(format!("build system msg: {}", e)))?;

        let provider = LlmProvider::new(
            config.provider.clone(),
            config.api_key.clone(),
            Some(config.model.clone()),
        )
        .map_err(|e| SummaryError::LlmCallFailed(format!("build provider: {}", e)))?;
        let llm_config = LlmConfig::new(provider);
        let request = LlmRequest::new(vec![system_msg, user_msg], llm_config, false)
            .map_err(|e| SummaryError::LlmCallFailed(format!("build request: {}", e)))?;

        let response = self
            .repo
            .call(request)
            .await
            .map_err(|e| SummaryError::LlmCallFailed(e.to_string()))?;

        // Normalise the output: trim, strip surrounding quotes, collapse newlines.
        let raw = response.content().trim().trim_matches('"').to_string();
        let collapsed = raw.replace(['\n', '\r'], " ").trim().to_string();
        if collapsed.is_empty() {
            return Err(SummaryError::EmptyResponse);
        }
        // Char-truncate to max_output_chars.
        let truncated = match collapsed.char_indices().nth(config.max_output_chars) {
            Some((cut, _)) => collapsed[..cut].to_string(),
            None => collapsed,
        };

        Ok(SummaryOutcome::Generated(truncated))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::domain::{LlmRequestId, LlmResponse, MockLlmRepository, ProviderKind};
    use std::time::Duration;

    fn cfg() -> SummaryConfig {
        SummaryConfig {
            provider: ProviderKind::Mock,
            model: "mock-model".into(),
            api_key: "test-key".into(),
            max_output_chars: 200,
            timeout: Duration::from_secs(5),
        }
    }

    fn mock_provider() -> LlmProvider {
        LlmProvider::new(
            ProviderKind::Mock,
            "test-key".into(),
            Some("mock-model".into()),
        )
        .unwrap()
    }

    fn mock_response(content: &str) -> LlmResponse {
        LlmResponse::new(
            LlmRequestId::from_string("req-summary-test".into()).unwrap(),
            content.into(),
            mock_provider(),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn generates_summary_from_extracted_text() {
        let mut mock = MockLlmRepository::new();
        mock.expect_call()
            .times(1)
            .returning(|_| Ok(mock_response("A Q3 financial report dated 2025-09")));
        let generator = LlmAttachmentSummaryGenerator::new(Arc::new(mock));

        let outcome = generator
            .generate(
                SummaryInput {
                    filename: "q3.pdf".into(),
                    mime_type: "application/pdf".into(),
                    source: SummarySource::ExtractedText("Quarterly results...".into()),
                },
                &cfg(),
            )
            .await
            .unwrap();
        match outcome {
            SummaryOutcome::Generated(s) => {
                assert!(s.contains("Q3"), "got {}", s);
            }
            o => panic!("expected Generated, got {:?}", o),
        }
    }

    #[tokio::test]
    async fn empty_extracted_text_returns_skipped() {
        let mock = MockLlmRepository::new(); // no call expected
        let generator = LlmAttachmentSummaryGenerator::new(Arc::new(mock));

        let outcome = generator
            .generate(
                SummaryInput {
                    filename: "x.pdf".into(),
                    mime_type: "application/pdf".into(),
                    source: SummarySource::ExtractedText("   ".into()),
                },
                &cfg(),
            )
            .await
            .unwrap();
        assert!(matches!(outcome, SummaryOutcome::Skipped { .. }));
    }

    #[tokio::test]
    async fn whitespace_only_response_returns_empty_response_err() {
        let mut mock = MockLlmRepository::new();
        mock.expect_call()
            .times(1)
            .returning(|_| Ok(mock_response("   ")));
        let generator = LlmAttachmentSummaryGenerator::new(Arc::new(mock));

        let err = generator
            .generate(
                SummaryInput {
                    filename: "x.pdf".into(),
                    mime_type: "application/pdf".into(),
                    source: SummarySource::ExtractedText("content".into()),
                },
                &cfg(),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, SummaryError::EmptyResponse));
    }

    #[tokio::test]
    async fn truncates_oversized_response_to_max_output_chars() {
        let long: String = "a".repeat(500);
        let mut mock = MockLlmRepository::new();
        mock.expect_call()
            .times(1)
            .returning(move |_| Ok(mock_response(&long)));
        let generator = LlmAttachmentSummaryGenerator::new(Arc::new(mock));

        let outcome = generator
            .generate(
                SummaryInput {
                    filename: "x.pdf".into(),
                    mime_type: "application/pdf".into(),
                    source: SummarySource::ExtractedText("content".into()),
                },
                &cfg(),
            )
            .await
            .unwrap();
        match outcome {
            SummaryOutcome::Generated(s) => assert_eq!(s.chars().count(), 200),
            o => panic!("expected Generated, got {:?}", o),
        }
    }

    #[tokio::test]
    async fn collapses_newlines_in_response() {
        let mut mock = MockLlmRepository::new();
        mock.expect_call()
            .times(1)
            .returning(|_| Ok(mock_response("line1\nline2\nline3")));
        let generator = LlmAttachmentSummaryGenerator::new(Arc::new(mock));

        let outcome = generator
            .generate(
                SummaryInput {
                    filename: "x.pdf".into(),
                    mime_type: "application/pdf".into(),
                    source: SummarySource::ExtractedText("content".into()),
                },
                &cfg(),
            )
            .await
            .unwrap();
        match outcome {
            SummaryOutcome::Generated(s) => {
                assert!(!s.contains('\n'));
                assert!(s.contains("line1 line2 line3"));
            }
            o => panic!("expected Generated, got {:?}", o),
        }
    }
}
