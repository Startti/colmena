//! Factory hermana de LlmProviderFactory que compone los adapters
//! de Files API por proveedor. Se mantiene separada para que
//! cambios en uno no toquen al otro.

use crate::llm::domain::{FileProviderRepository, LlmError, ProviderKind};
use crate::llm::infrastructure::files::{
    AnthropicFilesApiAdapter, GeminiFilesApiAdapter, OpenAiFilesApiAdapter,
};
use std::sync::Arc;

/// Factory that builds the right `FileProviderRepository` for a given
/// `ProviderKind`. Sister of `LlmProviderFactory`; kept separate so
/// changes to either path don't disturb the other.
pub struct FileProviderFactory;

impl FileProviderFactory {
    /// Builds an `Arc<dyn FileProviderRepository>` for the given provider.
    /// Returns `LlmError::ProviderLimitation` for `Mock` (no Files API).
    pub fn create(
        kind: ProviderKind,
        api_key: String,
    ) -> Result<Arc<dyn FileProviderRepository>, LlmError> {
        match kind {
            ProviderKind::Anthropic => {
                Ok(Arc::new(AnthropicFilesApiAdapter::new(api_key)))
            }
            ProviderKind::OpenAi => Ok(Arc::new(OpenAiFilesApiAdapter::new(api_key))),
            ProviderKind::Gemini => Ok(Arc::new(GeminiFilesApiAdapter::new(api_key))),
            ProviderKind::Mock => Err(LlmError::ProviderLimitation {
                provider: "mock".into(),
                feature: "Files API".into(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_anthropic() {
        let r = FileProviderFactory::create(ProviderKind::Anthropic, "k".into()).unwrap();
        assert_eq!(r.provider(), ProviderKind::Anthropic);
    }

    #[test]
    fn creates_openai() {
        let r = FileProviderFactory::create(ProviderKind::OpenAi, "k".into()).unwrap();
        assert_eq!(r.provider(), ProviderKind::OpenAi);
    }

    #[test]
    fn creates_gemini() {
        let r = FileProviderFactory::create(ProviderKind::Gemini, "k".into()).unwrap();
        assert_eq!(r.provider(), ProviderKind::Gemini);
    }

    #[test]
    fn rejects_mock() {
        let r = FileProviderFactory::create(ProviderKind::Mock, "k".into());
        assert!(r.is_err());
    }
}
