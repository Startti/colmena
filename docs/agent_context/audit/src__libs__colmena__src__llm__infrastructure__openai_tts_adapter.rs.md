# src/libs/colmena/src/llm/infrastructure/openai_tts_adapter.rs

**Layer:** infrastructure  
**Purpose:** OpenAI TTS provider adapter implementing `TtsRepository` trait; calls OpenAI's `/v1/audio/speech` endpoint via reqwest with proper error handling and format conversion.

## Symbols

- `DEFAULT_BASE_URL` (const) — OpenAI API base URL ("https://api.openai.com")
- `OpenAiTtsAdapter` (struct, pub) — HTTP client wrapper holding reqwest client, API key, and configurable base URL
- `new` (fn, pub) — constructor taking api_key string; creates fresh reqwest client and sets default base URL
- `with_base_url` (fn, pub) — builder method to override base URL for testing/non-standard endpoints
- `format_token` (fn, private) — converts AudioFormat enum (Mp3/Wav/Opus/Pcm) to OpenAI format string token
- `TtsRepository::synthesize` (fn, pub async) — main trait impl; validates non-empty text, builds JSON request body (model/input/voice/response_format/speed), calls POST endpoint, maps HTTP/provider errors to domain TtsError, validates non-empty audio bytes
- `TtsRepository::provider_name` (fn, pub) — trait impl returning literal "openai"
- `req` (fn, test helper) — creates minimal TtsRequest for testing (text/voice/format/model with no speed)
- `happy_path_returns_audio_bytes_and_mime` (test, tokio async) — mocks 200 response with ID3 header bytes; verifies audio_bytes and mime_type round-trip
- `empty_text_short_circuits_without_http` (test, tokio async) — verifies empty/whitespace text returns InvalidInput error before HTTP call
- `error_400_maps_to_provider_failed` (test, tokio async) — mocks 400 error response; verifies status and body captured in ProviderFailed variant
- `empty_audio_response_errors` (test, tokio async) — mocks 200 response with zero bytes; verifies EmptyAudio error is returned

## File-level notes

- **Solid error handling:** transport errors (reqwest), provider failures (non-2xx), empty audio validation, empty text short-circuit all properly mapped to domain `TtsError` variants.
- **Clean boundary:** no domain layer dependencies; only imports from domain trait + types. No infrastructure leakage into public API.
- **Test coverage:** four tests cover happy path, input validation (before HTTP), error response (HTTP status code capture), and edge case (empty audio). Uses wiremock for HTTP mocking; no real API calls in tests.
- **Builder pattern:** `with_base_url` enables test/internal overrides without breaking production default.
- **Graceful fallback:** line 72 `.unwrap_or_default()` on error response text read silently falls back to empty string; reasonable for error context logging, not load-bearing.
