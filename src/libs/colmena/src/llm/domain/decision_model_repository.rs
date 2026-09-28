//! Vendor-neutral port for typed decision models (noul / choice / score).
//! Mirrors [`super::tts_repository::TtsRepository`]. The HTTP adapter that
//! implements this trait for TypeSafe Jev lives in
//! `crate::llm::infrastructure` (a later slice); this port has zero
//! infrastructure dependencies.

use async_trait::async_trait;
use thiserror::Error;

use crate::llm::domain::decision_model::{DecisionRequest, DecisionResponse};

/// Errors a [`DecisionModelRepository`] adapter can return. Distinguishes
/// the vendor's three error-body shapes (`Auth`/`InvalidRequest`), rate
/// limiting, server/overload errors, transport failures, and local
/// validation failures that never reach the network (`InvalidInput`,
/// `Configuration`).
#[derive(Debug, Error)]
pub enum DecisionModelError {
    #[error("decision model authentication failed: {0}")]
    Auth(String),

    #[error("decision model rejected the request (status {status}): {message}")]
    InvalidRequest {
        status: u16,
        error_type: Option<String>,
        message: String,
    },

    #[error("decision model rate limit exceeded")]
    RateLimited,

    #[error("decision model upstream error (status {status}): {body}")]
    Upstream { status: u16, body: String },

    #[error("decision model request timed out")]
    Timeout,

    #[error("decision model transport error: {0}")]
    Transport(String),

    #[error("decision model returned a malformed response: {0}")]
    MalformedResponse(String),

    #[error("decision model invalid input: {0}")]
    InvalidInput(String),

    #[error("decision model configuration error: {0}")]
    Configuration(String),
}

/// A decision-model adapter. Adapters are stateless and cheap to construct
/// — the router node builds a fresh one per `execute()` from the per-call
/// config, mirroring [`super::tts_repository::TtsRepository`].
#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait DecisionModelRepository: Send + Sync {
    async fn decide(
        &self,
        request: DecisionRequest,
    ) -> Result<DecisionResponse, DecisionModelError>;

    fn provider_name(&self) -> &'static str;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::domain::decision_model::{Answer, DecisionRequest, DecisionResponse};
    use serde_json::json;
    use std::collections::BTreeMap;

    fn sample_request() -> DecisionRequest {
        DecisionRequest {
            model: "jev-1.13.0".to_string(),
            state: json!("hola, quiero un reembolso"),
            questions: vec![],
        }
    }

    #[tokio::test]
    async fn mock_repository_returns_the_configured_response() {
        let mut mock = MockDecisionModelRepository::new();
        mock.expect_decide().returning(|_| {
            Ok(DecisionResponse {
                model: "jev-1.13.0".to_string(),
                answers: BTreeMap::from([(
                    "route".to_string(),
                    Answer::Choice {
                        choice: "refund".to_string(),
                        probabilities: BTreeMap::from([("refund".to_string(), 1.0)]),
                        confidence: 1.0,
                    },
                )]),
                usage: None,
            })
        });
        mock.expect_provider_name().return_const("typesafe");

        let response = mock.decide(sample_request()).await.unwrap();
        assert_eq!(mock.provider_name(), "typesafe");
        match response.answers.get("route").unwrap() {
            Answer::Choice { choice, .. } => assert_eq!(choice, "refund"),
            other => panic!("expected a choice answer, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn mock_repository_propagates_a_vendor_error() {
        let mut mock = MockDecisionModelRepository::new();
        mock.expect_decide()
            .returning(|_| Err(DecisionModelError::RateLimited));

        let err = mock.decide(sample_request()).await.unwrap_err();
        assert!(matches!(err, DecisionModelError::RateLimited));
    }

    #[test]
    fn invalid_request_error_message_carries_status_and_message() {
        let err = DecisionModelError::InvalidRequest {
            status: 400,
            error_type: Some("max_tokens_exceeded".to_string()),
            message: "state too long".to_string(),
        };
        let text = err.to_string();
        assert!(text.contains("400") && text.contains("state too long"));
    }
}
