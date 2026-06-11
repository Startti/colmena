use crate::llm::domain::{LlmRepository, ProviderKind};
use crate::llm::infrastructure::{AnthropicAdapter, GeminiAdapter, MockAdapter, OpenAiAdapter};
use std::sync::{Arc, RwLock};

pub struct LlmProviderFactory;

/// Process-global test override. When set, [`LlmProviderFactory::create`]
/// returns this adapter regardless of `kind`. Tests must serialize via
/// [`LlmProviderFactory::override_lock`] (or use [`OverrideGuard`] which
/// does it for you).
static OVERRIDE: once_cell::sync::Lazy<RwLock<Option<Arc<dyn LlmRepository>>>> =
    once_cell::sync::Lazy::new(|| RwLock::new(None));

/// Resolve a base_url override for `kind` from the environment.
///
/// Precedence: the provider-specific var (`OPENAI_BASE_URL`,
/// `GEMINI_BASE_URL`, `ANTHROPIC_BASE_URL`) wins; otherwise the
/// `COLMENA_LLM_BASE_URL` catch-all applies. `Mock` and `Generated` are
/// never overridden — they don't make network calls.
///
/// Returns `None` when no relevant var is set, preserving the hardcoded
/// production defaults baked into each adapter's `new()`.
#[allow(dead_code)]
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

impl LlmProviderFactory {
    pub fn create(kind: ProviderKind) -> Arc<dyn LlmRepository> {
        if let Some(adapter) = OVERRIDE.read().expect("override poisoned").as_ref() {
            tracing::info!(
                target: "colmena::llm_factory",
                "LlmProviderFactory: returning test override adapter"
            );
            return Arc::clone(adapter);
        }
        match kind {
            ProviderKind::OpenAi => Arc::new(OpenAiAdapter::new()),
            ProviderKind::Google => Arc::new(GeminiAdapter::new()),
            ProviderKind::Anthropic => Arc::new(AnthropicAdapter::new()),
            ProviderKind::Mock => Arc::new(MockAdapter::new()),
            // `Generated` is a sentinel for AttachmentRegistry rows — never a
            // live LLM provider. Return the mock as a safe placeholder; any
            // code path that actually calls it would be a logic bug.
            ProviderKind::Generated => Arc::new(MockAdapter::new()),
        }
    }

    pub fn create_all() -> Vec<(ProviderKind, Arc<dyn LlmRepository>)> {
        vec![
            (ProviderKind::OpenAi, Self::create(ProviderKind::OpenAi)),
            (ProviderKind::Google, Self::create(ProviderKind::Google)),
            (
                ProviderKind::Anthropic,
                Self::create(ProviderKind::Anthropic),
            ),
        ]
    }

    /// Test-only: install a process-global adapter override that takes precedence
    /// over `create()`'s normal dispatch. Tests using this MUST acquire
    /// [`LlmProviderFactory::override_lock`] first to serialize against other
    /// tests using the same hook. Pass `None` to clear.
    #[doc(hidden)]
    pub fn set_test_override(adapter: Option<Arc<dyn LlmRepository>>) {
        *OVERRIDE.write().expect("override poisoned") = adapter;
    }

    /// Returns the lock that callers acquire to serialize tests using
    /// [`LlmProviderFactory::set_test_override`]. Acquire BEFORE calling
    /// `set_test_override`; release AFTER the test finishes (RAII via
    /// [`OverrideGuard`] is recommended).
    #[doc(hidden)]
    pub fn override_lock() -> &'static std::sync::Mutex<()> {
        static LOCK: once_cell::sync::Lazy<std::sync::Mutex<()>> =
            once_cell::sync::Lazy::new(|| std::sync::Mutex::new(()));
        &LOCK
    }
}

/// RAII guard: install an adapter override on construction, clear on drop.
/// Holds the serialization lock for the entire scope.
#[doc(hidden)]
pub struct OverrideGuard<'a> {
    _lock: std::sync::MutexGuard<'a, ()>,
}

impl<'a> OverrideGuard<'a> {
    pub fn install(adapter: Arc<dyn LlmRepository>) -> Self {
        // Tolerate poisoning: a previous test panicked while holding the lock,
        // but the override slot is RwLock-managed and we always reset on Drop.
        // It is safe to recover the guard.
        let lock = match LlmProviderFactory::override_lock().lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        LlmProviderFactory::set_test_override(Some(adapter));
        tracing::info!(
            target: "colmena::llm_factory",
            "OverrideGuard: installed scripted adapter"
        );
        Self { _lock: lock }
    }
}

impl<'a> Drop for OverrideGuard<'a> {
    fn drop(&mut self) {
        LlmProviderFactory::set_test_override(None);
        tracing::info!(
            target: "colmena::llm_factory",
            "OverrideGuard: cleared override"
        );
    }
}

#[cfg(test)]
mod base_url_override_tests {
    use super::*;
    use crate::llm::domain::ProviderKind;

    fn with_clean_env<F: FnOnce()>(f: F) {
        let _lock = match LlmProviderFactory::override_lock().lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        for k in ["OPENAI_BASE_URL", "GEMINI_BASE_URL", "ANTHROPIC_BASE_URL", "COLMENA_LLM_BASE_URL"] {
            std::env::remove_var(k);
        }
        f();
        for k in ["OPENAI_BASE_URL", "GEMINI_BASE_URL", "ANTHROPIC_BASE_URL", "COLMENA_LLM_BASE_URL"] {
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
            std::env::set_var("GEMINI_BASE_URL", "http://127.0.0.1:4000/gemini/v1beta");
            std::env::set_var("COLMENA_LLM_BASE_URL", "http://catchall");
            assert_eq!(
                base_url_override(ProviderKind::Google),
                Some("http://127.0.0.1:4000/gemini/v1beta".to_string())
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
}
