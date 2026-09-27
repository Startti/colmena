//! Factory for [`DecisionModelRepository`] adapters. Mirrors
//! `tts_provider_factory.rs`: the router's `decision_model` mode uses this
//! to map the per-call `config.provider` string to a concrete adapter.

use std::sync::Arc;

use crate::llm::domain::decision_model_repository::{DecisionModelError, DecisionModelRepository};
use crate::llm::infrastructure::TypesafeJevAdapter;

pub fn build_decision_model_repository(
    provider: &str,
    api_key: String,
) -> Result<Arc<dyn DecisionModelRepository>, DecisionModelError> {
    match provider {
        "typesafe" => Ok(Arc::new(TypesafeJevAdapter::new(api_key)?)),
        other => Err(DecisionModelError::Configuration(format!(
            "unknown decision-model provider '{other}' (expected typesafe)"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_the_typesafe_adapter() {
        let r = build_decision_model_repository("typesafe", "k".into()).unwrap();
        assert_eq!(r.provider_name(), "typesafe");
    }

    #[test]
    fn unknown_provider_is_a_configuration_error() {
        let result = build_decision_model_repository("jev", "k".into());
        match result {
            Ok(_) => panic!("expected error for unknown provider"),
            Err(e) => assert!(matches!(e, DecisionModelError::Configuration(_))),
        }
    }

    #[test]
    fn empty_key_is_a_configuration_error_naming_the_env_var() {
        let result = build_decision_model_repository("typesafe", "   ".into());
        match result {
            Ok(_) => panic!("expected error for empty api_key"),
            Err(DecisionModelError::Configuration(msg)) => {
                assert!(msg.contains("TYPESAFE_API_KEY"))
            }
            Err(other) => panic!("expected Configuration, got {other:?}"),
        }
    }
}
