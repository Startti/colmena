//! Adapter one-shot de `MessageSummarizer`: llamada sin historia con un modelo
//! barato; NO hard-corta la salida (el target es blando, por prompt). Bypassa
//! `LlmCallUseCase`, así el turno nunca entra a `llm_node_history`.

use crate::llm::domain::{
    LlmConfig, LlmError, LlmMessage, LlmProvider, LlmRepository, LlmRequest, MessageSummarizer,
    ProviderKind,
};
use async_trait::async_trait;
use std::sync::Arc;
use std::time::Duration;

pub struct LlmMessageSummarizer {
    repo: Arc<dyn LlmRepository>,
    provider: ProviderKind,
    api_key: String,
    model: String,
    timeout: Duration,
}

impl LlmMessageSummarizer {
    pub fn new(
        repo: Arc<dyn LlmRepository>,
        provider: ProviderKind,
        api_key: String,
        model: String,
        timeout: Duration,
    ) -> Self {
        Self {
            repo,
            provider,
            api_key,
            model,
            timeout,
        }
    }
}

#[async_trait]
impl MessageSummarizer for LlmMessageSummarizer {
    async fn summarize(&self, text: &str, target_chars: usize) -> Result<String, LlmError> {
        let system = format!(
            "Resumí el siguiente mensaje de una conversación en ~{target_chars} caracteres, \
             en UNA línea, conservando lo accionable (hechos, decisiones, resultados, ids). \
             Sin markdown, sin comillas, sin comentarios. Solo el resumen."
        );
        let sys = LlmMessage::system(system)?;
        let usr = LlmMessage::user(text.to_string())?;
        let provider = LlmProvider::new(
            self.provider.clone(),
            self.api_key.clone(),
            Some(self.model.clone()),
        )?;
        let request = LlmRequest::new(vec![sys, usr], LlmConfig::new(provider), false)?;

        let response = match tokio::time::timeout(self.timeout, self.repo.call(request)).await {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => return Err(e),
            Err(_) => {
                return Err(LlmError::RequestFailed {
                    message: format!("summarizer timeout after {:?}", self.timeout),
                })
            }
        };

        let out = response
            .content()
            .trim()
            .trim_matches('"')
            .replace(['\n', '\r'], " ")
            .trim()
            .to_string();
        if out.is_empty() {
            return Err(LlmError::RequestFailed {
                message: "summarizer returned empty".into(),
            });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::domain::{
        LlmProvider, LlmRequestId, LlmResponse, MockLlmRepository, ProviderKind,
    };

    fn mock_response(text: &str) -> LlmResponse {
        LlmResponse::new(
            LlmRequestId::from_string("req-sum".into()).unwrap(),
            text.into(),
            LlmProvider::new(ProviderKind::Mock, "k".into(), Some("m".into())).unwrap(),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn summarize_returns_trimmed_one_line() {
        let mut mock = MockLlmRepository::new();
        mock.expect_call()
            .times(1)
            .returning(|_| Ok(mock_response("  Resumió la cadena de cálculos.\n")));
        let s = LlmMessageSummarizer::new(
            std::sync::Arc::new(mock),
            ProviderKind::Mock,
            "k".into(),
            "m".into(),
            std::time::Duration::from_secs(5),
        );
        let out = s.summarize("texto largo...", 250).await.unwrap();
        assert_eq!(out, "Resumió la cadena de cálculos.");
        assert!(!out.contains('\n'));
    }
}
