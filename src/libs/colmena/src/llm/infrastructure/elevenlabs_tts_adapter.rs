//! ElevenLabs TTS adapter. Calls `POST /v1/text-to-speech/{voice_id}`. The
//! `voice` field of [`TtsRequest`] is treated as the ElevenLabs `voice_id`.
//! Response body is raw audio bytes.
//!
//! `speed` is **not supported** by ElevenLabs and is silently ignored
//! (warning logged).

use async_trait::async_trait;

use crate::llm::domain::tts::{AudioFormat, TtsRequest, TtsResponse};
use crate::llm::domain::tts_repository::{TtsError, TtsRepository};

const DEFAULT_BASE_URL: &str = "https://api.elevenlabs.io";

pub struct ElevenLabsTtsAdapter {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
}

impl ElevenLabsTtsAdapter {
    pub fn new(api_key: String) -> Self {
        Self {
            client: crate::shared::http_client::client(),
            api_key,
            base_url: DEFAULT_BASE_URL.to_string(),
        }
    }

    pub fn with_base_url(mut self, base_url: String) -> Self {
        self.base_url = base_url;
        self
    }

    /// ElevenLabs uses qualified format names: `mp3_44100_128` etc. We pick
    /// the most common defaults for each container.
    fn output_format_token(fmt: AudioFormat) -> &'static str {
        match fmt {
            AudioFormat::Mp3 => "mp3_44100_128",
            AudioFormat::Wav => "pcm_44100", // wav-like raw PCM at 44.1 kHz
            AudioFormat::Opus => "opus_48000_128",
            AudioFormat::Pcm => "pcm_44100",
        }
    }
}

#[async_trait]
impl TtsRepository for ElevenLabsTtsAdapter {
    async fn synthesize(&self, req: TtsRequest) -> Result<TtsResponse, TtsError> {
        if req.text.trim().is_empty() {
            return Err(TtsError::InvalidInput("text must be non-empty".into()));
        }
        if req.voice.trim().is_empty() {
            return Err(TtsError::InvalidInput(
                "voice (= elevenlabs voice_id) must be non-empty".into(),
            ));
        }
        // ElevenLabs voice_ids are alphanumeric (with - and _). Reject anything
        // else early so we never need URL encoding in the path segment.
        if !req
            .voice
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(TtsError::InvalidInput(format!(
                "voice id '{}' contains non-alphanumeric characters",
                req.voice
            )));
        }
        if req.speed.is_some() {
            tracing::warn!(
                "elevenlabs_tts_adapter: `speed` is not supported by ElevenLabs and will be ignored"
            );
        }

        let body = serde_json::json!({
            "text": req.text,
            "model_id": req.model,
        });

        let url = format!(
            "{}/v1/text-to-speech/{}?output_format={}",
            self.base_url,
            req.voice,
            Self::output_format_token(req.format),
        );

        let resp = self
            .client
            .post(&url)
            .header("xi-api-key", &self.api_key)
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
        "elevenlabs"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn req(text: &str) -> TtsRequest {
        TtsRequest {
            text: text.into(),
            voice: "voice_xyz".into(),
            format: AudioFormat::Mp3,
            speed: None,
            model: "eleven_multilingual_v2".into(),
        }
    }

    #[tokio::test]
    async fn happy_path_uses_voice_in_path_and_xi_header() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/text-to-speech/voice_xyz"))
            .and(header("xi-api-key", "el-key"))
            .and(query_param("output_format", "mp3_44100_128"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![1u8, 2, 3]))
            .mount(&server)
            .await;

        let adapter = ElevenLabsTtsAdapter::new("el-key".into()).with_base_url(server.uri());
        let out = adapter.synthesize(req("hola")).await.unwrap();
        assert_eq!(out.audio_bytes, vec![1u8, 2, 3]);
        assert_eq!(out.mime_type, "audio/mpeg");
    }

    #[tokio::test]
    async fn error_401_maps_to_provider_failed() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/text-to-speech/voice_xyz"))
            .respond_with(ResponseTemplate::new(401).set_body_string("invalid key"))
            .mount(&server)
            .await;

        let adapter = ElevenLabsTtsAdapter::new("wrong".into()).with_base_url(server.uri());
        let err = adapter.synthesize(req("hi")).await.unwrap_err();
        match err {
            TtsError::ProviderFailed { status, body } => {
                assert_eq!(status, 401);
                assert!(body.contains("invalid key"));
            }
            other => panic!("expected ProviderFailed, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn empty_voice_errors_locally() {
        let mut r = req("hello");
        r.voice = "".into();
        let adapter = ElevenLabsTtsAdapter::new("k".into()).with_base_url("http://invalid".into());
        let err = adapter.synthesize(r).await.unwrap_err();
        assert!(matches!(err, TtsError::InvalidInput(_)));
    }
}
