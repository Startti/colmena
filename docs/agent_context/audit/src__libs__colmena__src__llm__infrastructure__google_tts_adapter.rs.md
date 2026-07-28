# src/libs/colmena/src/llm/infrastructure/google_tts_adapter.rs

**Layer:** infrastructure  **Purpose:** Concrete adapter for Google Gemini TTS API. Synthesizes speech via `:generateContent` with `responseModalities: ["AUDIO"]`, decodes base64-encoded L16 PCM, and optionally wraps raw audio in RIFF/WAVE headers for playable WAV files.

## Symbols

- `DEFAULT_BASE_URL` (const) — Hardcoded Gemini API v1beta base URL
- `GoogleTtsAdapter` (struct) — HTTP client + API key holder for Gemini TTS synthesis
- `GoogleTtsAdapter::new()` (fn) — Constructor initializing reqwest client with provided API key and default base URL
- `GoogleTtsAdapter::with_base_url()` (fn) — Builder method to override base URL (for testing)
- `TtsRepository for GoogleTtsAdapter` (impl) — Trait implementation providing `synthesize()` and `provider_name()` methods
- `synthesize()` (async method) — Validates text/voice, calls Gemini API, decodes inlineData base64, optionally wraps PCM in WAV header, returns TtsResponse with audio bytes and mime type
- `provider_name()` (method) — Returns static string "google"
- `parse_sample_rate_from_mime()` (fn, private) — Parses Hz from mime type `audio/L16;rate=24000`, defaults to 24000 Hz
- `wrap_pcm_in_wav()` (fn, private) — Builds 44-byte RIFF/WAVE header + PCM payload in little-endian format per canonical WAVE spec
- `tests` (mod) — Test suite with 5 tests: happy-path PCM passthrough, WAV format wrapping, sample-rate parsing, missing candidates error, HTTP 403 error

## File-level notes

- Clean, focused adapter with no external dependencies beyond base64/serde_json/reqwest/async-trait
- Input validation (non-empty text/voice) guards provider calls
- Format degradation (unsupported mp3/opus) documented via warn! log with fallback to raw PCM — intentional behavior, not a stub
- Error handling exhaustive: transport errors, HTTP failures, missing/malformed response fields, empty audio, base64 decode failures
- WAV wrapping logic sound: correct byte order, precise header struct (RIFF descriptor → fmt sub-chunk → data sub-chunk), PCM payload preservation
- Tests use wiremock for isolation; no live provider calls needed; coverage includes happy path, format wrapping, parsing edge cases, error responses
- No dead code, no unfinished stubs, no obvious improvements
