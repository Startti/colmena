//! Files API de Anthropic (beta).
//! Header obligatorio: `anthropic-beta: files-api-2025-04-14`.
//! Multipart/form-data con un único POST.

use crate::llm::domain::{
    BoxedByteStream, FileProviderRepository, LlmError, ProviderFileRef, ProviderKind,
};
use async_trait::async_trait;
use reqwest::multipart::{Form, Part};
use reqwest::{Body, Client};
use serde::Deserialize;
use std::time::Duration;

const BETA_HEADER: &str = "files-api-2025-04-14";

/// Adapter for Anthropic's Files API (beta endpoint, header
/// `anthropic-beta: files-api-2025-04-14`).
///
/// Uploads files via multipart/form-data in a single POST. Files
/// uploaded here do not expire automatically (Anthropic side), so
/// [`FileProviderRepository::ttl`] returns `None`.
pub struct AnthropicFilesApiAdapter {
    client: Client,
    base_url: String,
    api_key: String,
}

impl AnthropicFilesApiAdapter {
    /// Creates an adapter pointing at the production Anthropic API
    /// (`https://api.anthropic.com`) with sensible default timeouts.
    pub fn new(api_key: String) -> Self {
        Self {
            client: Self::default_client(),
            base_url: "https://api.anthropic.com".to_string(),
            api_key,
        }
    }

    /// Creates an adapter with a custom `base_url` (typically a
    /// wiremock server in tests).
    pub fn with_base_url(api_key: String, base_url: String) -> Self {
        let base_url = base_url.trim_end_matches('/').to_string();
        Self {
            client: Self::default_client(),
            base_url,
            api_key,
        }
    }

    fn default_client() -> Client {
        crate::shared::http_client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(600))
            .user_agent(concat!("colmena/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("default reqwest client should build")
    }

    /// The configured endpoint. Exposed for tests and diagnostics; lets
    /// factory tests assert that env-var overrides are honored.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }
}

#[derive(Debug, Deserialize)]
struct UploadResponse {
    id: String,
}

#[async_trait]
impl FileProviderRepository for AnthropicFilesApiAdapter {
    async fn upload_streaming(
        &self,
        stream: BoxedByteStream,
        mime_type: &str,
        filename: &str,
    ) -> Result<ProviderFileRef, LlmError> {
        let body = Body::wrap_stream(stream);
        // Falla de `mime_str` es violación de precondición del caller (mime
        // string mal formado), no error de upload — clasificarla como
        // `InvalidMimeType` permite que retry policies la distingan.
        let part = Part::stream(body)
            .file_name(filename.to_string())
            .mime_str(mime_type)
            .map_err(|e| LlmError::InvalidMimeType {
                mime: mime_type.to_string(),
                message: e.to_string(),
            })?;

        let form = Form::new().part("file", part);
        let url = format!("{}/v1/files", self.base_url);

        let response = self
            .client
            .post(&url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("anthropic-beta", BETA_HEADER)
            .multipart(form)
            .send()
            .await
            .map_err(|e| LlmError::FileApiUploadFailed {
                provider: "anthropic".into(),
                message: e.to_string(),
            })?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(LlmError::FileApiUploadFailed {
                provider: "anthropic".into(),
                message: format!("HTTP {}: {}", status, body),
            });
        }

        let parsed: UploadResponse =
            response
                .json()
                .await
                .map_err(|e| LlmError::FileApiUploadFailed {
                    provider: "anthropic".into(),
                    message: format!("invalid JSON response: {}", e),
                })?;

        Ok(ProviderFileRef {
            provider: ProviderKind::Anthropic,
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
        ProviderKind::Anthropic
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
    async fn upload_succeeds_returns_file_id() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/files"))
            .and(header("x-api-key", "test-key"))
            .and(header("anthropic-beta", BETA_HEADER))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "file_01abc",
                "type": "file",
                "filename": "x.pdf"
            })))
            .mount(&server)
            .await;

        let adapter = AnthropicFilesApiAdapter::with_base_url("test-key".into(), server.uri());
        let r = adapter
            .upload_streaming(fake_stream(b"PDF-CONTENT"), "application/pdf", "x.pdf")
            .await
            .unwrap();
        assert_eq!(r.provider_file_id, "file_01abc");
        assert_eq!(r.provider, ProviderKind::Anthropic);
        assert!(r.expires_at.is_none());
    }

    #[tokio::test]
    async fn upload_fails_on_413() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/files"))
            .respond_with(ResponseTemplate::new(413).set_body_string("too large"))
            .mount(&server)
            .await;

        let adapter = AnthropicFilesApiAdapter::with_base_url("k".into(), server.uri());
        let err = adapter
            .upload_streaming(fake_stream(b"data"), "application/pdf", "x.pdf")
            .await
            .unwrap_err();
        assert!(matches!(err, LlmError::FileApiUploadFailed { .. }));
    }

    #[test]
    fn ttl_is_none() {
        let a = AnthropicFilesApiAdapter::new("k".into());
        assert!(a.ttl().is_none());
    }

    #[tokio::test]
    async fn upload_classifies_invalid_mime_as_invalid_mime_type() {
        // No need to start a server: the error fires in `mime_str` before
        // any HTTP traffic.
        let adapter =
            AnthropicFilesApiAdapter::with_base_url("k".into(), "http://example.invalid".into());
        let err = adapter
            .upload_streaming(fake_stream(b"data"), "not a real mime", "x.pdf")
            .await
            .unwrap_err();
        match err {
            LlmError::InvalidMimeType { mime, .. } => {
                assert_eq!(mime, "not a real mime");
            }
            other => panic!("expected InvalidMimeType, got {:?}", other),
        }
    }
}
