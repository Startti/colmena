//! Byte acquisition for attachment summary generation.
//!
//! The main upload pipeline (`upload_streaming`) consumes the bytes and
//! does not retain them. The summary generator needs a separate copy.
//! For v1 we re-download (signed URLs) or re-read (paths). Inline sources
//! already carry the bytes in memory.
//!
//! No size cap is enforced here — the upload boundary (frontend) already
//! caps file sizes at 100 MB, so adding a redundant backend check would
//! be dead code.

use crate::llm::domain::attachments::AttachmentSource;
use crate::llm::domain::signed_url_fetcher::SignedUrlFetcher;
use futures::StreamExt;
use std::sync::Arc;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AcquireError {
    #[error("download error: {0}")]
    Download(String),

    #[error("read error: {0}")]
    Read(String),

    #[error("source has no retained bytes")]
    NoBytes,
}

/// Acquire the bytes of an attachment for local extraction.
///
/// `inline_bytes` carries the bytes from `FileSource::InlineBytes` upstream,
/// because they are not stored anywhere else after the upload streams them.
///
/// No size cap is enforced — the frontend already caps uploads at 100 MB,
/// so this helper accepts arbitrary-size inputs.
pub async fn acquire_bytes(
    source: &AttachmentSource,
    inline_bytes: Option<&[u8]>,
    fetcher: Arc<dyn SignedUrlFetcher>,
) -> Result<Vec<u8>, AcquireError> {
    match source {
        AttachmentSource::Inline => inline_bytes
            .map(|b| b.to_vec())
            .ok_or(AcquireError::NoBytes),

        AttachmentSource::Path(p) => tokio::fs::read(p)
            .await
            .map_err(|e| AcquireError::Read(e.to_string())),

        AttachmentSource::SignedUrl(url) => {
            let mut stream = fetcher
                .stream(url)
                .await
                .map_err(|e| AcquireError::Download(e.to_string()))?;
            let mut buf: Vec<u8> = Vec::new();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|e| AcquireError::Download(e.to_string()))?;
                buf.extend_from_slice(&chunk);
            }
            Ok(buf)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::domain::file_provider_repository::BoxedByteStream;
    use crate::llm::domain::LlmError;
    use async_trait::async_trait;
    use bytes::Bytes;
    use futures::stream;

    struct MockFetcher {
        body: Vec<u8>,
        chunk_size: usize,
    }

    impl MockFetcher {
        fn new(body: Vec<u8>) -> Self {
            Self {
                body,
                chunk_size: 8,
            }
        }
    }

    #[async_trait]
    impl SignedUrlFetcher for MockFetcher {
        async fn stream(&self, _url: &str) -> Result<BoxedByteStream, LlmError> {
            let body = self.body.clone();
            let chunk_size = self.chunk_size.max(1);
            let chunks: Vec<Result<Bytes, std::io::Error>> = body
                .chunks(chunk_size)
                .map(|c| Ok::<Bytes, std::io::Error>(Bytes::copy_from_slice(c)))
                .collect();
            Ok(Box::pin(stream::iter(chunks)))
        }
    }

    #[tokio::test]
    async fn inline_returns_provided_bytes() {
        let r = acquire_bytes(
            &AttachmentSource::Inline,
            Some(b"hello"),
            Arc::new(MockFetcher::new(vec![])),
        )
        .await
        .unwrap();
        assert_eq!(r, b"hello");
    }

    #[tokio::test]
    async fn inline_without_bytes_errors() {
        let r = acquire_bytes(
            &AttachmentSource::Inline,
            None,
            Arc::new(MockFetcher::new(vec![])),
        )
        .await;
        assert!(matches!(r, Err(AcquireError::NoBytes)));
    }

    #[tokio::test]
    async fn signed_url_returns_fetched_bytes() {
        let r = acquire_bytes(
            &AttachmentSource::SignedUrl("https://example.com/x".into()),
            None,
            Arc::new(MockFetcher::new(b"downloaded".to_vec())),
        )
        .await
        .unwrap();
        assert_eq!(r, b"downloaded");
    }
}
