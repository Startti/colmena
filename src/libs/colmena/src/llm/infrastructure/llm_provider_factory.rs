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
            ProviderKind::Gemini => Arc::new(GeminiAdapter::new()),
            ProviderKind::Anthropic => Arc::new(AnthropicAdapter::new()),
            ProviderKind::Mock => Arc::new(MockAdapter::new()),
        }
    }

    pub fn create_all() -> Vec<(ProviderKind, Arc<dyn LlmRepository>)> {
        vec![
            (ProviderKind::OpenAi, Self::create(ProviderKind::OpenAi)),
            (ProviderKind::Gemini, Self::create(ProviderKind::Gemini)),
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
