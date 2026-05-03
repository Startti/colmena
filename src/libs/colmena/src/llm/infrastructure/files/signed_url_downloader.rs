//! Descarga streaming de signed URLs (GCS) sin Authorization header.
//! La firma viaja en query params; añadir Authorization invalidaría la firma.

use crate::llm::domain::{BoxedByteStream, LlmError, SignedUrlFetcher};
use async_trait::async_trait;
use futures::TryStreamExt;
use reqwest::Client;

/// HTTP downloader for GCS V4 signed URLs.
///
/// Issues GET requests *without* an `Authorization` header — the signature
/// is encoded in the URL query parameters and any auth header would
/// invalidate it.
pub struct SignedUrlDownloader {
    client: Client,
}

impl SignedUrlDownloader {
    /// Creates a downloader with a default `reqwest::Client` configured
    /// with sensible timeouts.
    pub fn new() -> Self {
        Self {
            client: Self::default_client(),
        }
    }

    fn default_client() -> reqwest::Client {
        reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(10))
            // 600s = 10 min. Generous for files up to ~500 MB on slow connections.
            .timeout(std::time::Duration::from_secs(600))
            .user_agent(concat!("colmena/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("default reqwest client should build")
    }

    /// Creates a downloader reusing an existing `reqwest::Client`.
    /// Recommended in production to share the connection pool. The caller
    /// owns the timeout policy of the supplied client.
    pub fn with_client(client: Client) -> Self {
        Self { client }
    }

    /// Streams the response body of a signed URL as a `BoxedByteStream`.
    ///
    /// Dropping the returned stream early aborts the underlying request
    /// and releases the HTTP connection. The downloader does not retry —
    /// retry policy belongs in the use-case layer.
    ///
    /// # Errors
    /// - [`LlmError::NetworkError`] on transport failure (DNS, TCP, TLS, timeout).
    /// - [`LlmError::SignedUrlFetchFailed`] on any non-2xx HTTP status.
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

        let stream = response.bytes_stream().map_err(std::io::Error::other);

        Ok(Box::pin(stream))
    }
}

impl Default for SignedUrlDownloader {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SignedUrlFetcher for SignedUrlDownloader {
    async fn stream(&self, url: &str) -> Result<BoxedByteStream, LlmError> {
        SignedUrlDownloader::stream(self, url).await
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
        assert!(matches!(
            err,
            LlmError::SignedUrlFetchFailed { status: 403 }
        ));
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
        assert!(matches!(
            err,
            LlmError::SignedUrlFetchFailed { status: 404 }
        ));
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
        let req = received
            .iter()
            .find(|r| r.url.path() == "/no-auth.pdf")
            .unwrap();
        assert!(req.headers.get("authorization").is_none());
    }
}
