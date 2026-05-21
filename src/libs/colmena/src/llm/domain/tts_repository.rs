//! Port for text-to-speech providers. Three adapters ship in
//! `crate::llm::infrastructure`: OpenAI, ElevenLabs, Google Gemini TTS.

use async_trait::async_trait;
use thiserror::Error;

use crate::llm::domain::tts::{TtsRequest, TtsResponse};

#[derive(Debug, Error)]
pub enum TtsError {
    #[error("tts provider request failed (status {status}): {body}")]
    ProviderFailed { status: u16, body: String },

    #[error("tts provider returned empty audio")]
    EmptyAudio,

    #[error("tts transport error: {0}")]
    Transport(String),

    #[error("tts invalid input: {0}")]
    InvalidInput(String),

    #[error("tts unsupported option for provider: {0}")]
    UnsupportedOption(String),
}

/// A text-to-speech adapter. Adapters are stateless and cheap to construct —
/// the node builds a fresh one per `execute()` based on the per-call config.
#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait TtsRepository: Send + Sync {
    async fn synthesize(&self, req: TtsRequest) -> Result<TtsResponse, TtsError>;

    fn provider_name(&self) -> &'static str;
}
