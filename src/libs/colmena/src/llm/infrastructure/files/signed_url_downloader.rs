//! Descarga streaming de signed URLs (GCS) sin Authorization header.
//! La firma viaja en query params; añadir Authorization invalidaría la firma.

use crate::llm::domain::{BoxedByteStream, LlmError};
use futures::TryStreamExt;
use reqwest::Client;

pub struct SignedUrlDownloader {
    client: Client,
}

impl SignedUrlDownloader {
    pub fn new() -> Self {
        Self { client: Client::new() }
    }

    pub fn with_client(client: Client) -> Self {
        Self { client }
    }

    pub async fn stream(&self, url: &str) -> Result<BoxedByteStream, LlmError> {
        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| LlmError::NetworkError {
                message: format!("signed URL fetch failed: {}", e),
            })?;

        let status = response.status();
        if !status.is_success() {
            return Err(LlmError::SignedUrlFetchFailed {
                status: status.as_u16(),
            });
        }

        let stream = response
            .bytes_stream()
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e));

        Ok(Box::pin(stream))
    }
}

impl Default for SignedUrlDownloader {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn stream_returns_body_chunks_on_2xx() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/file.pdf"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"hello world"))
            .mount(&server)
            .await;

        let downloader = SignedUrlDownloader::new();
        let url = format!("{}/file.pdf?sig=x", server.uri());
        let mut stream = downloader.stream(&url).await.unwrap();

        let mut all = Vec::new();
        while let Some(chunk) = stream.next().await {
            all.extend_from_slice(&chunk.unwrap());
        }
        assert_eq!(all, b"hello world");
    }

    #[tokio::test]
    async fn stream_errors_on_403() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/expired.pdf"))
            .respond_with(ResponseTemplate::new(403))
            .mount(&server)
            .await;

        let downloader = SignedUrlDownloader::new();
        let url = format!("{}/expired.pdf", server.uri());
        let err = match downloader.stream(&url).await {
            Ok(_) => panic!("expected error, got Ok"),
            Err(e) => e,
        };
        assert!(matches!(err, LlmError::SignedUrlFetchFailed { status: 403 }));
    }

    #[tokio::test]
    async fn stream_errors_on_404() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/missing.pdf"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        let downloader = SignedUrlDownloader::new();
        let url = format!("{}/missing.pdf", server.uri());
        let err = match downloader.stream(&url).await {
            Ok(_) => panic!("expected error, got Ok"),
            Err(e) => e,
        };
        assert!(matches!(err, LlmError::SignedUrlFetchFailed { status: 404 }));
    }

    #[tokio::test]
    async fn stream_does_not_send_authorization() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/no-auth.pdf"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"ok"))
            .mount(&server)
            .await;

        let downloader = SignedUrlDownloader::new();
        let url = format!("{}/no-auth.pdf", server.uri());
        let result = downloader.stream(&url).await;
        assert!(result.is_ok());
        // Validar via received requests:
        let received = server.received_requests().await.unwrap();
        let req = received.iter().find(|r| r.url.path() == "/no-auth.pdf").unwrap();
        assert!(req.headers.get("authorization").is_none());
    }
}
