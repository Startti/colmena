//! Registro de modelos baratos por provider para tareas internas de resumen.
//!
//! Fuente de verdad: `text/config/cheap_models.yaml` (embebido en compile-time).
//! Resolución (mayor a menor prioridad):
//!   1. Env `COLMENA_CHEAP_MODEL_<PROVIDER>` (runtime, sin rebuild).
//!   2. El YAML embebido.
//!   3. El modelo default del provider.

use crate::llm::domain::ProviderKind;
use std::collections::HashMap;
use std::sync::OnceLock;

const CHEAP_MODELS_YAML: &str = include_str!("../../../text/config/cheap_models.yaml");

static CHEAP_MODELS: OnceLock<HashMap<String, String>> = OnceLock::new();

fn registry() -> &'static HashMap<String, String> {
    CHEAP_MODELS.get_or_init(|| {
        serde_yaml::from_str(CHEAP_MODELS_YAML)
            .unwrap_or_else(|e| panic!("text/config/cheap_models.yaml malformed: {e}"))
    })
}

/// Resuelve el modelo barato para un provider: env → yaml → default del provider.
pub fn cheap_model_for(provider: ProviderKind) -> String {
    let key = provider.to_string(); // "openai" | "google" | "anthropic" | ...
    let env_var = format!("COLMENA_CHEAP_MODEL_{}", key.to_uppercase());
    if let Ok(v) = std::env::var(&env_var) {
        let v = v.trim().to_string();
        if !v.is_empty() {
            return v;
        }
    }
    if let Some(m) = registry().get(&key) {
        return m.clone();
    }
    provider.default_model().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yaml_has_the_three_real_providers() {
        let r = registry();
        assert_eq!(
            r.get("google").map(|s| s.as_str()),
            Some("gemini-2.5-flash")
        );
        assert_eq!(r.get("openai").map(|s| s.as_str()), Some("gpt-4o-mini"));
        assert!(r.contains_key("anthropic"));
    }

    #[test]
    fn yaml_default_when_no_env() {
        std::env::remove_var("COLMENA_CHEAP_MODEL_GOOGLE");
        assert_eq!(cheap_model_for(ProviderKind::Google), "gemini-2.5-flash");
    }

    #[test]
    fn env_override_wins() {
        std::env::set_var("COLMENA_CHEAP_MODEL_OPENAI", "gpt-4o-mini-test-override");
        assert_eq!(
            cheap_model_for(ProviderKind::OpenAi),
            "gpt-4o-mini-test-override"
        );
        std::env::remove_var("COLMENA_CHEAP_MODEL_OPENAI");
    }

    #[test]
    fn never_returns_gemini_1_5() {
        std::env::remove_var("COLMENA_CHEAP_MODEL_GOOGLE");
        assert_ne!(cheap_model_for(ProviderKind::Google), "gemini-1.5-flash");
    }
}
