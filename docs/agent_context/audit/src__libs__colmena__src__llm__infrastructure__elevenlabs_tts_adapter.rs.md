# src/libs/colmena/src/llm/infrastructure/elevenlabs_tts_adapter.rs

**Layer:** infrastructure  
**Purpose:** ElevenLabs TTS adapter implementing the `TtsRepository` port. Converts text to audio via the ElevenLabs API, mapping `TtsRequest` to `/v1/text-to-speech/{voice_id}` and handling audio byte responses.

## Symbols

- `DEFAULT_BASE_URL` (const, private) — Canonical ElevenLabs API base URL
- `ElevenLabsTtsAdapter` (struct, pub) — Adapter holding reqwest client, API key, and configurable base URL
- `ElevenLabsTtsAdapter::new` (fn, pub) — Constructor accepting API key; initializes client and default base URL
- `ElevenLabsTtsAdapter::with_base_url` (fn, pub) — Builder method to override base URL (enables mocking in tests)
- `ElevenLabsTtsAdapter::output_format_token` (fn, private) — Maps `AudioFormat` enum to ElevenLabs format tokens (`mp3_44100_128`, `pcm_44100`, `opus_48000_128`)
- `TtsRepository impl for ElevenLabsTtsAdapter` (impl block) — Trait implementation with `#[async_trait]`
- `synthesize` (async fn, pub via trait) — Main entry point; validates text/voice, constructs POST request with model_id, handles non-success responses and empty audio, returns audio bytes with mime type
- `provider_name` (fn, pub via trait) — Returns static string "elevenlabs"
- `tests` (mod, cfg(test)) — Test module with wiremock fixtures
- `req` (fn in tests) — Factory helper building a `TtsRequest` with default voice_xyz and Mp3 format
- `happy_path_uses_voice_in_path_and_xi_header` (test, async) — Verifies POST to correct path with xi-api-key header and output_format query param; confirms audio bytes returned
- `error_401_maps_to_provider_failed` (test, async) — Confirms 401 errors surface as `TtsError::ProviderFailed` with status and body
- `empty_voice_errors_locally` (test, async) — Confirms input validation rejects empty voice field before network call

## File-level notes

- **Input validation:** Lines 50–69 implement thorough validation (non-empty text, non-empty voice, alphanumeric+dash+underscore only for voice_id) to prevent malformed requests and unnecessary URL encoding
- **Speed parameter ignored:** Line 70–74 explicitly warn that ElevenLabs does not support the `speed` parameter; warning is logged but request proceeds (non-blocking degradation)
- **Format mapping:** The `output_format_token` function maps a small enum cleanly; all four format variants (Mp3, Wav, Opus, Pcm) are covered with reasonable defaults
- **Error recovery:** Line 99 silently defaults error response bodies (`unwrap_or_default()`) when body read fails; status code is always captured, but body read errors are not logged separately (minor boundary concern, not a blocker)
- **Test coverage:** Three representative tests (happy path, auth failure, validation) using wiremock; no integration tests with real ElevenLabs credentials
