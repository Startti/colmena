//! Port and value objects for attachment summary generation.
//!
//! The summary call runs in parallel with the main `llm_call`. It is a
//! one-shot, history-less invocation: it must NOT write to
//! `llm_node_history`. Implementations live in the infrastructure layer.

use crate::llm::domain::ProviderKind;
use async_trait::async_trait;
use std::time::Duration;
use thiserror::Error;

/// What the generator is asked to summarise.
#[derive(Debug, Clone)]
pub struct SummaryInput {
    pub filename: String,
    pub mime_type: String,
    pub source: SummarySource,
}

/// The actual payload fed to the model.
#[derive(Debug, Clone)]
pub enum SummarySource {
    /// Pre-extracted and char-truncated text (PDF, plain, markdown, etc.).
    ExtractedText(String),
    /// Raw image bytes; the generator will attach them as a vision input.
    ImageBytes(Vec<u8>),
}

/// Configuration for one summary call.
#[derive(Debug, Clone)]
pub struct SummaryConfig {
    pub provider: ProviderKind,
    pub model: String,
    pub api_key: String,
    pub max_output_chars: usize,
    pub timeout: Duration,
}

/// Result of attempting to generate a summary for one attachment.
///
/// Not an `Err` for skipped/empty cases because they are **expected**
/// outcomes that should still flow through normal control flow (and
/// be persisted as `description = null`), not unhandled errors.
#[derive(Debug, Clone)]
pub enum SummaryOutcome {
    Generated(String),
    Skipped { reason: String },
    Failed { reason: String },
}

/// Error type for the generator port. Returned only for unexpected
/// infrastructure failures (network, malformed request). Predictable
/// "no summary" cases use `SummaryOutcome::Skipped` / `Failed` instead.
#[derive(Debug, Error)]
pub enum SummaryError {
    #[error("llm call failed: {0}")]
    LlmCallFailed(String),

    #[error("empty model response")]
    EmptyResponse,
}

/// Generates a single-line summary for one attachment.
#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait AttachmentSummaryGenerator: Send + Sync {
    async fn generate(
        &self,
        input: SummaryInput,
        config: &SummaryConfig,
    ) -> Result<SummaryOutcome, SummaryError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_input_text_holds_text() {
        let i = SummaryInput {
            filename: "x.pdf".into(),
            mime_type: "application/pdf".into(),
            source: SummarySource::ExtractedText("abc".into()),
        };
        match i.source {
            SummarySource::ExtractedText(t) => assert_eq!(t, "abc"),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn summary_outcome_variants_carry_data() {
        let gen = SummaryOutcome::Generated("hello".into());
        let skip = SummaryOutcome::Skipped {
            reason: "image-only".into(),
        };
        let fail = SummaryOutcome::Failed {
            reason: "timeout".into(),
        };
        assert!(matches!(gen, SummaryOutcome::Generated(_)));
        assert!(matches!(skip, SummaryOutcome::Skipped { .. }));
        assert!(matches!(fail, SummaryOutcome::Failed { .. }));
    }

    #[test]
    fn summary_error_display_includes_reason() {
        let e = SummaryError::LlmCallFailed("rate limit".into());
        assert!(format!("{}", e).contains("rate limit"));
    }
}
