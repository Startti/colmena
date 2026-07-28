# src/libs/colmena/src/llm/domain/tts_repository.rs

**Layer:** domain  
**Purpose:** Defines the text-to-speech provider port (async interface) and domain errors. Three adapters are implemented in infrastructure: OpenAI, ElevenLabs, and Google Gemini TTS.

## Symbols

- `TtsError` (enum, pub) — domain error for TTS operations; derives thiserror::Error
- `TtsError::ProviderFailed { status: u16, body: String }` (variant) — error when provider returns HTTP error status
- `TtsError::EmptyAudio` (variant) — error when provider returns no audio content
- `TtsError::Transport(String)` (variant) — network or transport-layer error
- `TtsError::InvalidInput(String)` (variant) — error for invalid input to TTS
- `TtsError::UnsupportedOption(String)` (variant) — error when provider does not support a requested option
- `TtsRepository` (trait, pub) — async port for TTS providers; requires Send + Sync; annotated with #[async_trait] and test mock support via #[cfg_attr(test, mockall::automock)]
- `TtsRepository::synthesize` (async method in trait) — synthesize audio from TtsRequest, returning TtsResponse or TtsError
- `TtsRepository::provider_name` (method in trait) — returns the static display name of the provider

## File-level notes

- Clean, focused port definition with no infrastructure dependencies.
- Error variants are descriptive and cover all major failure modes.
- Uses async_trait for trait async methods and mockall for test mocking.
- Imports TtsRequest and TtsResponse from sibling `crate::llm::domain::tts` module.
- No dead code, unfinished stubs, or obvious improvements.
