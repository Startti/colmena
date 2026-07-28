# src/libs/colmena/src/llm/infrastructure/tts_provider_factory.rs

**Layer:** infrastructure  
**Purpose:** Factory function that instantiates the appropriate TTS provider adapter (OpenAI, ElevenLabs, or Google) based on a per-call provider string. Maps provider names to concrete `TtsRepository` implementations.

## Symbols

- `build_tts_repository` (function, pub) — Factory that creates and returns an Arc-wrapped `TtsRepository` adapter matching the given provider string, or returns `TtsError::UnsupportedOption` for unknown providers.
- `tests` (module, private) — Test module containing unit tests for provider instantiation and error handling.
- `builds_each_supported_provider` (function, private/test) — Test that instantiates each supported provider (openai, elevenlabs, google) and verifies its `provider_name()` output matches the input.
- `unknown_provider_errors` (function, private/test) — Test that verifies unknown provider strings (e.g., "nuance") return a `TtsError::UnsupportedOption`.

## File-level notes

- Clean, minimal factory implementation with no external infrastructure dependencies beyond trait imports.
- Test coverage is comprehensive: all three supported providers are tested, and error path (unknown provider) is verified.
- No dead code, unfinished work, or obvious improvements detected.
