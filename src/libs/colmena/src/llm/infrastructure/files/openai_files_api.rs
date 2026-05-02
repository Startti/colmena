//! Files API de OpenAI.
//! POST /v1/files multipart con purpose=user_data (para uso con
//! responses/chat.completions referenciando file_id).

use crate::llm::domain::{
    BoxedByteStream, FileProviderRepository, LlmError, ProviderFileRef, ProviderKind,
};
use async_trait::async_trait;
use reqwest::multipart::{Form, Part};
use reqwest::{Body, Client};
use serde::Deserialize;
use std::time::Duration;

/// Adapter for OpenAI's Files API.
///
/// Uploads files via multipart/form-data with `purpose=user_data` so the
/// resulting `file_id` can be referenced in chat.completions / responses
/// content parts. Files do not expire automatically (OpenAI side).
pub struct OpenAiFilesApiAdapter {
    client: Client,
    base_url: String,
    api_key: String,
}

impl OpenAiFilesApiAdapter {
    /// Creates an adapter pointing at the production OpenAI API.
    pub fn new(api_key: String) -> Self {
        Self {
            client: Self::default_client(),
            base_url: "https://api.openai.com".into(),
            api_key,
        }
    }

    /// Creates an adapter with a custom `base_url` (typically a wiremock
    /// server in tests). Trims a trailing `/` defensively.
    pub fn with_base_url(api_key: String, base_url: String) -> Self {
        let base_url = base_url.trim_end_matches('/').to_string();
        Self {
            client: Self::default_client(),
            base_url,
            api_key,
        }
    }

    fn default_client() -> Client {
        Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(600))
            .user_agent(concat!("colmena/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("default reqwest client should build")
    }
}

#[derive(Debug, Deserialize)]
struct UploadResponse {
    id: String,
}

#[async_trait]
impl FileProviderRepository for OpenAiFilesApiAdapter {
    async fn upload_streaming(
        &self,
        stream: BoxedByteStream,
        mime_type: &str,
        filename: &str,
    ) -> Result<ProviderFileRef, LlmError> {
        let body = Body::wrap_stream(stream);
        let file_part = Part::stream(body)
            .file_name(filename.to_string())
            .mime_str(mime_type)
            .map_err(|e| LlmError::FileApiUploadFailed {
                provider: "openai".into(),
                message: format!("precondition: invalid mime '{}': {}", mime_type, e),
            })?;

        let form = Form::new()
            .text("purpose", "user_data")
            .part("file", file_part);

        let url = format!("{}/v1/files", self.base_url);
        let response = self
            .client
            .post(&url)
            .bearer_auth(&self.api_key)
            .multipart(form)
            .send()
            .await
            .map_err(|e| LlmError::FileApiUploadFailed {
                provider: "openai".into(),
                message: e.to_string(),
            })?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(LlmError::FileApiUploadFailed {
                provider: "openai".into(),
                message: format!("HTTP {}: {}", status, body),
            });
        }

        let parsed: UploadResponse = response.json().await.map_err(|e| {
            LlmError::FileApiUploadFailed {
                provider: "openai".into(),
                message: format!("invalid JSON response: {}", e),
            }
        })?;

        Ok(ProviderFileRef {
            provider: ProviderKind::OpenAi,
            provider_file_id: parsed.id,
            mime_type: mime_type.to_string(),
            filename: filename.to_string(),
            expires_at: None,
        })
    }

    fn ttl(&self) -> Option<Duration> {
        None
    }

    fn provider(&self) -> ProviderKind {
        ProviderKind::OpenAi
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use futures::stream;
    use serde_json::json;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn fake_stream(content: &[u8]) -> BoxedByteStream {
        let bytes = Bytes::copy_from_slice(content);
        Box::pin(stream::iter(vec![Ok::<_, std::io::Error>(bytes)]))
    }

    #[tokio::test]
    async fn upload_succeeds_with_bearer_and_purpose() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/files"))
            .and(header("authorization", "Bearer sk-test"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "file-abc123",
                "object": "file",
                "purpose": "user_data"
            })))
            .mount(&server)
            .await;

        let adapter = OpenAiFilesApiAdapter::with_base_url(
            "sk-test".into(),
            server.uri(),
        );
        let r = adapter
            .upload_streaming(fake_stream(b"PDF"), "application/pdf", "x.pdf")
            .await
            .unwrap();
        assert_eq!(r.provider_file_id, "file-abc123");
        assert_eq!(r.provider, ProviderKind::OpenAi);
        assert!(r.expires_at.is_none());
    }

    #[tokio::test]
    async fn upload_errors_on_400() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/files"))
            .respond_with(ResponseTemplate::new(400).set_body_string("bad"))
            .mount(&server)
            .await;

        let adapter = OpenAiFilesApiAdapter::with_base_url("k".into(), server.uri());
        let err = match adapter
            .upload_streaming(fake_stream(b"x"), "application/pdf", "x.pdf")
            .await
        {
            Ok(_) => panic!("expected error"),
            Err(e) => e,
        };
        assert!(matches!(err, LlmError::FileApiUploadFailed { .. }));
    }

    #[test]
    fn ttl_is_none() {
        let a = OpenAiFilesApiAdapter::new("k".into());
        assert!(a.ttl().is_none());
    }
}
