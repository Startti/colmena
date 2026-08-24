//! OpenAI TTS adapter. Calls `POST /v1/audio/speech`. Response body is the
//! raw audio bytes (no JSON envelope) — content-type depends on the
//! `response_format` field.

use async_trait::async_trait;

use crate::llm::domain::tts::{AudioFormat, TtsRequest, TtsResponse};
use crate::llm::domain::tts_repository::{TtsError, TtsRepository};

const DEFAULT_BASE_URL: &str = "https://api.openai.com";

pub struct OpenAiTtsAdapter {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
}

impl OpenAiTtsAdapter {
    pub fn new(api_key: String) -> Self {
        Self {
            client: crate::shared::http_client::client(),
            api_key,
            base_url: DEFAULT_BASE_URL.to_string(),
        }
    }

    /// Test/internal helper to override the base URL.
    pub fn with_base_url(mut self, base_url: String) -> Self {
        self.base_url = base_url;
        self
    }

    fn format_token(fmt: AudioFormat) -> &'static str {
        match fmt {
            AudioFormat::Mp3 => "mp3",
            AudioFormat::Wav => "wav",
            AudioFormat::Opus => "opus",
            AudioFormat::Pcm => "pcm",
        }
    }
}

#[async_trait]
impl TtsRepository for OpenAiTtsAdapter {
    async fn synthesize(&self, req: TtsRequest) -> Result<TtsResponse, TtsError> {
        if req.text.trim().is_empty() {
            return Err(TtsError::InvalidInput("text must be non-empty".into()));
        }

        let mut body = serde_json::json!({
            "model": req.model,
            "input": req.text,
            "voice": req.voice,
            "response_format": Self::format_token(req.format),
        });
        if let Some(speed) = req.speed {
            body["speed"] = serde_json::json!(speed);
        }

        let url = format!("{}/v1/audio/speech", self.base_url);
        let resp = self
            .client
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| TtsError::Transport(e.to_string()))?;

        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            return Err(TtsError::ProviderFailed { status, body });
        }

        let audio_bytes = resp
            .bytes()
            .await
            .map_err(|e| TtsError::Transport(e.to_string()))?
            .to_vec();
        if audio_bytes.is_empty() {
            return Err(TtsError::EmptyAudio);
        }

        Ok(TtsResponse {
            audio_bytes,
            mime_type: req.format.mime_type().to_string(),
            duration_estimate_ms: None,
        })
    }

    fn provider_name(&self) -> &'static str {
        "openai"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header_exists, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn req(text: &str) -> TtsRequest {
        TtsRequest {
            text: text.into(),
            voice: "alloy".into(),
            format: AudioFormat::Mp3,
            speed: None,
            model: "tts-1".into(),
        }
    }

    #[tokio::test]
    async fn happy_path_returns_audio_bytes_and_mime() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/audio/speech"))
            .and(header_exists("authorization"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_bytes(vec![0x49u8, 0x44, 0x33, 0x04]) // ID3 header bytes
                    .insert_header("content-type", "audio/mpeg"),
            )
            .mount(&server)
            .await;

        let adapter = OpenAiTtsAdapter::new("sk-test".into()).with_base_url(server.uri());
        let out = adapter.synthesize(req("hello world")).await.unwrap();
        assert_eq!(out.audio_bytes, vec![0x49, 0x44, 0x33, 0x04]);
        assert_eq!(out.mime_type, "audio/mpeg");
    }

    #[tokio::test]
    async fn empty_text_short_circuits_without_http() {
        let adapter = OpenAiTtsAdapter::new("sk-test".into())
            .with_base_url("http://does-not-exist.invalid".into());
        let err = adapter.synthesize(req("   ")).await.unwrap_err();
        assert!(matches!(err, TtsError::InvalidInput(_)));
    }

    #[tokio::test]
    async fn error_400_maps_to_provider_failed() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/audio/speech"))
            .respond_with(ResponseTemplate::new(400).set_body_string("bad voice"))
            .mount(&server)
            .await;

        let adapter = OpenAiTtsAdapter::new("sk-test".into()).with_base_url(server.uri());
        let err = adapter.synthesize(req("hi")).await.unwrap_err();
        match err {
            TtsError::ProviderFailed { status, body } => {
                assert_eq!(status, 400);
                assert!(body.contains("bad voice"));
            }
            other => panic!("expected ProviderFailed, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn empty_audio_response_errors() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/audio/speech"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(Vec::<u8>::new()))
            .mount(&server)
            .await;

        let adapter = OpenAiTtsAdapter::new("sk-test".into()).with_base_url(server.uri());
        let err = adapter.synthesize(req("hi")).await.unwrap_err();
        assert!(matches!(err, TtsError::EmptyAudio));
    }
}
