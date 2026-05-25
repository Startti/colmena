//! Google Gemini TTS adapter. Calls Gemini's `:generateContent` endpoint
//! with `responseModalities: ["AUDIO"]`. Returns base64-encoded audio inside
//! the candidates[0].content.parts[0].inlineData structure.
//!
//! Uses the same `generativelanguage.googleapis.com/v1beta` base + API key
//! flow as the regular Gemini text adapter (see `gemini_adapter.rs`). No
//! OAuth flow needed — different from Vertex Imagen.

use async_trait::async_trait;
use base64::Engine;
use serde_json::Value;

use crate::llm::domain::tts::{AudioFormat, TtsRequest, TtsResponse};
use crate::llm::domain::tts_repository::{TtsError, TtsRepository};

const DEFAULT_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta";

pub struct GoogleTtsAdapter {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
}

impl GoogleTtsAdapter {
    pub fn new(api_key: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key,
            base_url: DEFAULT_BASE_URL.to_string(),
        }
    }

    pub fn with_base_url(mut self, base_url: String) -> Self {
        self.base_url = base_url;
        self
    }
}

#[async_trait]
impl TtsRepository for GoogleTtsAdapter {
    async fn synthesize(&self, req: TtsRequest) -> Result<TtsResponse, TtsError> {
        if req.text.trim().is_empty() {
            return Err(TtsError::InvalidInput("text must be non-empty".into()));
        }
        if req.voice.trim().is_empty() {
            return Err(TtsError::InvalidInput(
                "voice (= prebuilt voice name) must be non-empty".into(),
            ));
        }

        // Gemini TTS preview models return raw 16-bit linear PCM (mime
        // `audio/L16;rate=24000` or similar). We honor `format`:
        //   - Pcm: pass through as audio/L16 (raw)
        //   - Wav: wrap in a 44-byte WAV header so the file is playable
        //   - Other formats (mp3/opus): can't synthesize — warn and pass raw
        if !matches!(req.format, AudioFormat::Pcm | AudioFormat::Wav) {
            tracing::warn!(
                "google_tts_adapter: Gemini TTS returns L16 PCM; container conversion \
                 to {:?} is not implemented — emitting raw PCM with audio/L16 mime.",
                req.format
            );
        }

        let mut generation_config = serde_json::json!({
            "responseModalities": ["AUDIO"],
            "speechConfig": {
                "voiceConfig": {
                    "prebuiltVoiceConfig": { "voiceName": req.voice }
                }
            }
        });
        if let Some(speed) = req.speed {
            generation_config["speechConfig"]["speakingRate"] = serde_json::json!(speed);
        }

        let body = serde_json::json!({
            "contents": [{
                "role": "user",
                "parts": [{ "text": req.text }]
            }],
            "generationConfig": generation_config,
        });

        let url = format!(
            "{}/models/{}:generateContent?key={}",
            self.base_url, req.model, self.api_key
        );

        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| TtsError::Transport(e.to_string()))?;

        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            return Err(TtsError::ProviderFailed { status, body });
        }

        let payload: Value = resp
            .json()
            .await
            .map_err(|e| TtsError::Transport(format!("invalid JSON: {e}")))?;

        let inline = payload
            .pointer("/candidates/0/content/parts/0/inlineData")
            .ok_or_else(|| TtsError::ProviderFailed {
                status: 0,
                body: format!(
                    "missing candidates[0].content.parts[0].inlineData in response: {}",
                    payload
                ),
            })?;

        let b64 = inline.get("data").and_then(|v| v.as_str()).ok_or_else(|| {
            TtsError::ProviderFailed {
                status: 0,
                body: "inlineData.data missing or not a string".into(),
            }
        })?;

        let raw_mime = inline
            .get("mimeType")
            .and_then(|v| v.as_str())
            .unwrap_or("audio/L16")
            .to_string();

        let raw_bytes = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .map_err(|e| TtsError::Transport(format!("invalid base64: {e}")))?;

        if raw_bytes.is_empty() {
            return Err(TtsError::EmptyAudio);
        }

        // If the caller asked for WAV and the provider returned raw L16,
        // wrap with a 44-byte WAV header so the resulting file is playable
        // by any standard audio player / browser.
        let (audio_bytes, mime_type) =
            if matches!(req.format, AudioFormat::Wav) && raw_mime.starts_with("audio/L16") {
                let sample_rate = parse_sample_rate_from_mime(&raw_mime);
                let wav = wrap_pcm_in_wav(&raw_bytes, sample_rate, /* channels */ 1);
                (wav, "audio/wav".to_string())
            } else {
                (raw_bytes, raw_mime)
            };

        Ok(TtsResponse {
            audio_bytes,
            mime_type,
            duration_estimate_ms: None,
        })
    }

    fn provider_name(&self) -> &'static str {
        "google"
    }
}

/// Parses the sample rate (Hz) from a Gemini `audio/L16;rate=24000` mime
/// type. Returns `24000` as a fallback — the default rate of every Gemini
/// TTS preview model documented as of 2026-05.
fn parse_sample_rate_from_mime(mime: &str) -> u32 {
    mime.split(';')
        .find_map(|part| {
            let trimmed = part.trim();
            trimmed
                .strip_prefix("rate=")
                .and_then(|rate| rate.parse::<u32>().ok())
        })
        .unwrap_or(24000)
}

/// Builds a WAV file (RIFF/WAVE container, 44-byte header + PCM data) from
/// raw 16-bit linear PCM samples. Little-endian, mono or stereo, no compression.
/// The header layout follows the canonical WAVE PCM format
/// (https://docs.fileformat.com/audio/wav/).
fn wrap_pcm_in_wav(pcm: &[u8], sample_rate_hz: u32, channels: u16) -> Vec<u8> {
    let bits_per_sample: u16 = 16;
    let byte_rate: u32 = sample_rate_hz * channels as u32 * bits_per_sample as u32 / 8;
    let block_align: u16 = channels * bits_per_sample / 8;
    let data_size: u32 = pcm.len() as u32;
    let chunk_size: u32 = 36 + data_size;

    let mut wav = Vec::with_capacity(44 + pcm.len());
    // RIFF chunk descriptor
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&chunk_size.to_le_bytes());
    wav.extend_from_slice(b"WAVE");
    // fmt sub-chunk
    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&16u32.to_le_bytes()); // sub-chunk size for PCM
    wav.extend_from_slice(&1u16.to_le_bytes()); // audio format: 1 = PCM
    wav.extend_from_slice(&channels.to_le_bytes());
    wav.extend_from_slice(&sample_rate_hz.to_le_bytes());
    wav.extend_from_slice(&byte_rate.to_le_bytes());
    wav.extend_from_slice(&block_align.to_le_bytes());
    wav.extend_from_slice(&bits_per_sample.to_le_bytes());
    // data sub-chunk
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_size.to_le_bytes());
    wav.extend_from_slice(pcm);
    wav
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn req(text: &str) -> TtsRequest {
        TtsRequest {
            text: text.into(),
            voice: "Kore".into(),
            format: AudioFormat::Pcm,
            speed: None,
            model: "gemini-2.5-flash-preview-tts".into(),
        }
    }

    #[tokio::test]
    async fn happy_path_decodes_inline_audio_and_surfaces_mime() {
        let server = MockServer::start().await;
        // "AAAA" base64 = [0,0,0]
        Mock::given(method("POST"))
            .and(path("/models/gemini-2.5-flash-preview-tts:generateContent"))
            .and(query_param("key", "g-key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "candidates": [{
                    "content": {
                        "parts": [{
                            "inlineData": {
                                "mimeType": "audio/L16;rate=24000",
                                "data": "AAAA"
                            }
                        }]
                    }
                }]
            })))
            .mount(&server)
            .await;

        let adapter = GoogleTtsAdapter::new("g-key".into()).with_base_url(server.uri());
        // Pcm request → no wrap, raw L16 passes through
        let out = adapter.synthesize(req("hola")).await.unwrap();
        assert_eq!(out.audio_bytes, vec![0u8, 0, 0]);
        assert_eq!(out.mime_type, "audio/L16;rate=24000");
    }

    #[tokio::test]
    async fn wav_format_wraps_pcm_with_header_and_remimes() {
        let server = MockServer::start().await;
        // "AAAA" base64 = [0,0,0] — 3 bytes of PCM
        Mock::given(method("POST"))
            .and(path("/models/gemini-2.5-flash-preview-tts:generateContent"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "candidates": [{
                    "content": {
                        "parts": [{
                            "inlineData": {
                                "mimeType": "audio/L16;rate=24000",
                                "data": "AAAA"
                            }
                        }]
                    }
                }]
            })))
            .mount(&server)
            .await;

        let adapter = GoogleTtsAdapter::new("g-key".into()).with_base_url(server.uri());
        let mut r = req("hola");
        r.format = AudioFormat::Wav;
        let out = adapter.synthesize(r).await.unwrap();
        // Mime should be the standard playable container, not raw L16
        assert_eq!(out.mime_type, "audio/wav");
        // 44-byte header + 3 bytes of PCM
        assert_eq!(out.audio_bytes.len(), 47);
        // RIFF/WAVE magic bytes — confirms a real WAV header
        assert_eq!(&out.audio_bytes[0..4], b"RIFF");
        assert_eq!(&out.audio_bytes[8..12], b"WAVE");
        assert_eq!(&out.audio_bytes[12..16], b"fmt ");
        assert_eq!(&out.audio_bytes[36..40], b"data");
        // PCM payload preserved verbatim
        assert_eq!(&out.audio_bytes[44..], &[0u8, 0, 0]);
    }

    #[test]
    fn parse_sample_rate_picks_rate_param_or_fallback() {
        assert_eq!(parse_sample_rate_from_mime("audio/L16;rate=24000"), 24000);
        assert_eq!(parse_sample_rate_from_mime("audio/L16;rate=48000"), 48000);
        // Whitespace tolerated
        assert_eq!(parse_sample_rate_from_mime("audio/L16; rate=16000"), 16000);
        // No rate param → 24000 default for Gemini
        assert_eq!(parse_sample_rate_from_mime("audio/L16"), 24000);
        // Garbage rate → fallback
        assert_eq!(parse_sample_rate_from_mime("audio/L16;rate=abc"), 24000);
    }

    #[tokio::test]
    async fn missing_candidates_errors() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/models/gemini-2.5-flash-preview-tts:generateContent"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({ "candidates": [] })),
            )
            .mount(&server)
            .await;

        let adapter = GoogleTtsAdapter::new("g-key".into()).with_base_url(server.uri());
        let err = adapter.synthesize(req("hi")).await.unwrap_err();
        match err {
            TtsError::ProviderFailed { status, body } => {
                assert_eq!(status, 0);
                assert!(body.contains("missing candidates"));
            }
            other => panic!("expected ProviderFailed, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn error_403_maps_to_provider_failed() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/models/gemini-2.5-flash-preview-tts:generateContent"))
            .respond_with(ResponseTemplate::new(403).set_body_string("api key invalid"))
            .mount(&server)
            .await;

        let adapter = GoogleTtsAdapter::new("g-key".into()).with_base_url(server.uri());
        let err = adapter.synthesize(req("hi")).await.unwrap_err();
        match err {
            TtsError::ProviderFailed { status, body } => {
                assert_eq!(status, 403);
                assert!(body.contains("api key invalid"));
            }
            other => panic!("expected ProviderFailed, got {:?}", other),
        }
    }
}
