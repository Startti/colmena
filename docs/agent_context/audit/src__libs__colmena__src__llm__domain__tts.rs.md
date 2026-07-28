# src/libs/colmena/src/llm/domain/tts.rs

**Layer:** domain  
**Purpose:** Defines TTS (text-to-speech) domain value objects and enums shared across provider adapters; provides audio format handling with MIME type and file extension mappings.

## Symbols

- `AudioFormat` (enum, pub) — Output audio container format with variants Mp3, Wav, Opus, Pcm; serialized as lowercase strings
- `AudioFormat::mp3` (enum variant, pub) — Represents MP3 audio format
- `AudioFormat::wav` (enum variant, pub) — Represents WAV audio format
- `AudioFormat::opus` (enum variant, pub) — Represents Opus audio format
- `AudioFormat::pcm` (enum variant, pub) — Represents PCM (Linear 16-bit) audio format
- `AudioFormat::mime_type` (method, pub) — Returns MIME type string for the audio format (e.g. "audio/mpeg", "audio/wav", "audio/ogg", "audio/L16")
- `AudioFormat::file_extension` (method, pub) — Returns file extension string for the audio format (e.g. "mp3", "wav", "opus", "pcm")
- `impl FromStr for AudioFormat` (trait impl, pub) — Enables string parsing to AudioFormat with case-insensitive alias support
- `FromStr::from_str` (method) — Parses string to AudioFormat; accepts aliases ("mpeg" → Mp3, "ogg" → Opus); rejects unknown formats with descriptive error
- `TtsRequest` (struct, pub) — Value object holding TTS request parameters: text, voice, format, speed (optional playback multiplier), and model identifier
- `TtsResponse` (struct, pub) — Value object holding TTS response data: audio_bytes, mime_type, and optional duration_estimate_ms (best-effort, provider-dependent)
- `tests` (module) — Unit test suite for AudioFormat parsing and MIME type handling
- `audio_format_round_trip_str` (test, fn) — Verifies that AudioFormat serializes to file_extension and deserializes back to the same variant
- `audio_format_accepts_aliases` (test, fn) — Verifies case-insensitive parsing and alias resolution (mpeg→Mp3, ogg→Opus, WAV→Wav)
- `audio_format_rejects_unknown` (test, fn) — Verifies that unknown format strings produce an error
- `default_is_mp3` (test, fn) — Verifies that AudioFormat::default() returns Mp3

## File-level notes

- Clean, focused domain types file with no infrastructure dependencies
- All public types are well-documented and include usage context (provider-specific voice identifiers, speed ranges, etc.)
- Test coverage is present but sparse: roundtrip parsing verified, aliases tested, error case tested, default tested (4 tests total)
- MIME type mappings are correct per RFC (audio/mpeg for MP3, audio/wav for WAV, audio/ogg for Opus, audio/L16 for PCM)
- FromStr trait impl uses Error = String, which is acceptable for domain parsing but could use a custom error type in future if stricter typing is desired
