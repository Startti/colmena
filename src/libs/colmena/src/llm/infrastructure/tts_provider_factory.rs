//! Factory for [`TtsRepository`] adapters. The `tts` node uses this to map
//! the per-call `config.provider` string to a concrete adapter.

use std::sync::Arc;

use crate::llm::domain::tts_repository::{TtsError, TtsRepository};
use crate::llm::infrastructure::{ElevenLabsTtsAdapter, GoogleTtsAdapter, OpenAiTtsAdapter};

pub fn build_tts_repository(
    provider: &str,
    api_key: String,
) -> Result<Arc<dyn TtsRepository>, TtsError> {
    match provider {
        "openai" => Ok(Arc::new(OpenAiTtsAdapter::new(api_key))),
        "elevenlabs" => Ok(Arc::new(ElevenLabsTtsAdapter::new(api_key))),
        "google" => Ok(Arc::new(GoogleTtsAdapter::new(api_key))),
        other => Err(TtsError::UnsupportedOption(format!(
            "unknown tts provider '{other}' (expected openai|elevenlabs|google)"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_each_supported_provider() {
        for p in ["openai", "elevenlabs", "google"] {
            let r = build_tts_repository(p, "k".into()).unwrap();
            assert_eq!(r.provider_name(), p);
        }
    }

    #[test]
    fn unknown_provider_errors() {
        // Can't use .unwrap_err() because Arc<dyn TtsRepository> isn't Debug.
        let result = build_tts_repository("nuance", "k".into());
        match result {
            Ok(_) => panic!("expected error for unknown provider"),
            Err(e) => assert!(matches!(e, TtsError::UnsupportedOption(_))),
        }
    }
}
