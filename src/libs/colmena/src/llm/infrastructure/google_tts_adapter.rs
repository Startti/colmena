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

        // Gemini TTS returns audio whose container is determined by the model;
        // for the preview TTS models it's PCM 24kHz wrapped as audio/L16 — we
        // surface the API's reported mime_type in the response. The `format`
        // field on the request is currently advisory only because the Gemini
        // TTS API does not yet expose container selection.
        if !matches!(req.format, AudioFormat::Pcm | AudioFormat::Wav) {
            tracing::warn!(
                "google_tts_adapter: Gemini TTS returns L16 PCM regardless of requested format ({:?}). \
                 Container conversion is the caller's responsibility.",
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

        let mime_type = inline
            .get("mimeType")
            .and_then(|v| v.as_str())
            .unwrap_or("audio/L16")
            .to_string();

        let audio_bytes = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .map_err(|e| TtsError::Transport(format!("invalid base64: {e}")))?;

        if audio_bytes.is_empty() {
            return Err(TtsError::EmptyAudio);
        }

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
        let out = adapter.synthesize(req("hola")).await.unwrap();
        assert_eq!(out.audio_bytes, vec![0u8, 0, 0]);
        assert_eq!(out.mime_type, "audio/L16;rate=24000");
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
