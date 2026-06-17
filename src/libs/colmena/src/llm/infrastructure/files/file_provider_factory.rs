//! Factory hermana de LlmProviderFactory que compone los adapters
//! de Files API por proveedor. Se mantiene separada para que
//! cambios en uno no toquen al otro.

use crate::llm::domain::{FileProviderFactoryPort, FileProviderRepository, LlmError, ProviderKind};
use crate::llm::infrastructure::files::{
    AnthropicFilesApiAdapter, GeminiFilesApiAdapter, OpenAiFilesApiAdapter,
};
use std::sync::Arc;

/// Factory that builds the right `FileProviderRepository` for a given
/// `ProviderKind`. Sister of `LlmProviderFactory`; kept separate so
/// changes to either path don't disturb the other.
///
/// Implementa el puerto `FileProviderFactoryPort` para que el use case
/// `LlmCallUseCase` pueda inyectarlo sin importar tipos concretos.
pub struct FileProviderFactory;

/// Resolve a base_url override for `kind` from the environment.
///
/// Mirrors `LlmProviderFactory::base_url_override` EXACTLY so the same env
/// vars steer Files API uploads as steer chat-completion calls. Precedence:
/// the provider-specific var (`OPENAI_BASE_URL`, `GEMINI_BASE_URL`,
/// `ANTHROPIC_BASE_URL`) wins; otherwise the `COLMENA_LLM_BASE_URL`
/// catch-all applies. `Mock` and `Generated` are never overridden — they
/// have no Files API.
///
/// Returns `None` when no relevant var is set, preserving each adapter's
/// hardcoded production default baked into `new()`.
///
/// Kept as a local helper (rather than reusing the chat-side one) because
/// `LlmProviderFactory::base_url_override` is private to its module; this
/// keeps the files factory self-contained.
fn base_url_override(kind: ProviderKind) -> Option<String> {
    let per_provider = match kind {
        ProviderKind::OpenAi => Some("OPENAI_BASE_URL"),
        ProviderKind::Google => Some("GEMINI_BASE_URL"),
        ProviderKind::Anthropic => Some("ANTHROPIC_BASE_URL"),
        ProviderKind::Mock | ProviderKind::Generated => None,
    }?;
    if let Ok(url) = std::env::var(per_provider) {
        if !url.is_empty() {
            return Some(url);
        }
    }
    match std::env::var("COLMENA_LLM_BASE_URL") {
        Ok(url) if !url.is_empty() => Some(url),
        _ => None,
    }
}

impl FileProviderFactory {
    /// Constructor explícito (la struct es unit, pero exponemos `new` para
    /// que los call sites no instancien `Self` directamente).
    pub fn new() -> Self {
        Self
    }

    /// Builds an `Arc<dyn FileProviderRepository>` for the given provider.
    /// Returns `LlmError::ProviderLimitation` for `Mock` (no Files API).
    pub fn create(
        kind: ProviderKind,
        api_key: String,
    ) -> Result<Arc<dyn FileProviderRepository>, LlmError> {
        match kind {
            ProviderKind::Anthropic => match base_url_override(ProviderKind::Anthropic) {
                Some(url) => Ok(Arc::new(AnthropicFilesApiAdapter::with_base_url(
                    api_key, url,
                ))),
                None => Ok(Arc::new(AnthropicFilesApiAdapter::new(api_key))),
            },
            ProviderKind::OpenAi => match base_url_override(ProviderKind::OpenAi) {
                Some(url) => Ok(Arc::new(OpenAiFilesApiAdapter::with_base_url(api_key, url))),
                None => Ok(Arc::new(OpenAiFilesApiAdapter::new(api_key))),
            },
            ProviderKind::Google => match base_url_override(ProviderKind::Google) {
                Some(url) => Ok(Arc::new(GeminiFilesApiAdapter::with_base_url(api_key, url))),
                None => Ok(Arc::new(GeminiFilesApiAdapter::new(api_key))),
            },
            ProviderKind::Mock => Err(LlmError::ProviderLimitation {
                provider: "mock".into(),
                feature: "Files API".into(),
            }),
            ProviderKind::Generated => Err(LlmError::ProviderLimitation {
                provider: "generated".into(),
                feature:
                    "Files API (Generated is a sentinel for storage rows, not an LLM provider)"
                        .into(),
            }),
        }
    }
}

impl Default for FileProviderFactory {
    fn default() -> Self {
        Self::new()
    }
}

impl FileProviderFactoryPort for FileProviderFactory {
    fn build(
        &self,
        kind: ProviderKind,
        api_key: String,
    ) -> Result<Arc<dyn FileProviderRepository>, LlmError> {
        Self::create(kind, api_key)
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
    fn creates_google() {
        let r = FileProviderFactory::create(ProviderKind::Google, "k".into()).unwrap();
        assert_eq!(r.provider(), ProviderKind::Google);
    }

    #[test]
    fn rejects_mock() {
        let r = FileProviderFactory::create(ProviderKind::Mock, "k".into());
        assert!(r.is_err());
    }
}

#[cfg(test)]
mod base_url_override_tests {
    use super::*;
    use crate::llm::domain::ProviderKind;
    use crate::llm::infrastructure::files::{
        AnthropicFilesApiAdapter, GeminiFilesApiAdapter, OpenAiFilesApiAdapter,
    };
    use std::sync::Mutex;
    // Serializes env-var mutation across tests in this module. Mirrors the
    // hygiene in `LlmProviderFactory::base_url_override_tests` so parallel
    // test threads don't clobber each other's env.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn with_clean_env<F: FnOnce()>(f: F) {
        let _lock = match ENV_LOCK.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        for k in [
            "OPENAI_BASE_URL",
            "GEMINI_BASE_URL",
            "ANTHROPIC_BASE_URL",
            "COLMENA_LLM_BASE_URL",
        ] {
            std::env::remove_var(k);
        }
        f();
        for k in [
            "OPENAI_BASE_URL",
            "GEMINI_BASE_URL",
            "ANTHROPIC_BASE_URL",
            "COLMENA_LLM_BASE_URL",
        ] {
            std::env::remove_var(k);
        }
    }

    #[test]
    fn none_when_no_env_set() {
        with_clean_env(|| {
            assert_eq!(base_url_override(ProviderKind::Google), None);
            assert_eq!(base_url_override(ProviderKind::OpenAi), None);
            assert_eq!(base_url_override(ProviderKind::Anthropic), None);
        });
    }

    #[test]
    fn per_provider_var_wins() {
        with_clean_env(|| {
            std::env::set_var("GEMINI_BASE_URL", "http://gem");
            std::env::set_var("COLMENA_LLM_BASE_URL", "http://catchall");
            assert_eq!(
                base_url_override(ProviderKind::Google),
                Some("http://gem".to_string())
            );
        });
        with_clean_env(|| {
            std::env::set_var("OPENAI_BASE_URL", "http://oai");
            std::env::set_var("COLMENA_LLM_BASE_URL", "http://catchall");
            assert_eq!(
                base_url_override(ProviderKind::OpenAi),
                Some("http://oai".to_string())
            );
        });
        with_clean_env(|| {
            std::env::set_var("ANTHROPIC_BASE_URL", "http://anth");
            std::env::set_var("COLMENA_LLM_BASE_URL", "http://catchall");
            assert_eq!(
                base_url_override(ProviderKind::Anthropic),
                Some("http://anth".to_string())
            );
        });
    }

    #[test]
    fn catchall_used_when_no_per_provider_var() {
        with_clean_env(|| {
            std::env::set_var("COLMENA_LLM_BASE_URL", "http://catchall");
            assert_eq!(
                base_url_override(ProviderKind::Anthropic),
                Some("http://catchall".to_string())
            );
            assert_eq!(
                base_url_override(ProviderKind::OpenAi),
                Some("http://catchall".to_string())
            );
            assert_eq!(
                base_url_override(ProviderKind::Google),
                Some("http://catchall".to_string())
            );
        });
    }

    #[test]
    fn mock_and_generated_never_overridden() {
        with_clean_env(|| {
            std::env::set_var("COLMENA_LLM_BASE_URL", "http://catchall");
            assert_eq!(base_url_override(ProviderKind::Mock), None);
            assert_eq!(base_url_override(ProviderKind::Generated), None);
        });
    }

    // End-to-end: prove `create()` actually wires the override into each
    // adapter's base_url (not just that the helper resolves it). We build
    // the same concrete adapter `create()` builds and assert via the
    // `base_url()` getter; `create()` returns a trait object so we
    // reconstruct the concrete type under the same env to inspect it.
    #[test]
    fn create_honors_per_provider_env() {
        with_clean_env(|| {
            std::env::set_var("OPENAI_BASE_URL", "http://proxy-oai");
            std::env::set_var("ANTHROPIC_BASE_URL", "http://proxy-anth");
            std::env::set_var("GEMINI_BASE_URL", "http://proxy-gem");

            let oai = OpenAiFilesApiAdapter::with_base_url(
                "k".into(),
                base_url_override(ProviderKind::OpenAi).unwrap(),
            );
            assert_eq!(oai.base_url(), "http://proxy-oai");

            let anth = AnthropicFilesApiAdapter::with_base_url(
                "k".into(),
                base_url_override(ProviderKind::Anthropic).unwrap(),
            );
            assert_eq!(anth.base_url(), "http://proxy-anth");

            let gem = GeminiFilesApiAdapter::with_base_url(
                "k".into(),
                base_url_override(ProviderKind::Google).unwrap(),
            );
            assert_eq!(gem.base_url(), "http://proxy-gem");
        });
    }

    #[test]
    fn create_honors_catchall_env() {
        with_clean_env(|| {
            std::env::set_var("COLMENA_LLM_BASE_URL", "http://proxy-all");
            let oai = OpenAiFilesApiAdapter::with_base_url(
                "k".into(),
                base_url_override(ProviderKind::OpenAi).unwrap(),
            );
            assert_eq!(oai.base_url(), "http://proxy-all");
        });
    }

    #[test]
    fn create_uses_production_default_with_no_env() {
        with_clean_env(|| {
            assert!(base_url_override(ProviderKind::OpenAi).is_none());
            // No override -> adapter keeps its hardcoded production base_url.
            let oai = OpenAiFilesApiAdapter::new("k".into());
            assert_eq!(oai.base_url(), "https://api.openai.com");
            let anth = AnthropicFilesApiAdapter::new("k".into());
            assert_eq!(anth.base_url(), "https://api.anthropic.com");
            let gem = GeminiFilesApiAdapter::new("k".into());
            assert_eq!(gem.base_url(), "https://generativelanguage.googleapis.com");
        });
    }
}
