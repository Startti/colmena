//! Bounded byte acquisition for attachment summary generation.
//!
//! The main upload pipeline (`upload_streaming`) consumes the bytes and
//! does not retain them. The summary generator needs a separate copy.
//! For v1 we re-download (signed URLs) or re-read (paths). Inline sources
//! already carry the bytes in memory.
//!
//! Bounds are enforced at this layer (not in `SignedUrlFetcher`) so the
//! trait stays minimal and all existing fetcher implementations work
//! unmodified.

use crate::llm::domain::attachments::AttachmentSource;
use crate::llm::domain::signed_url_fetcher::SignedUrlFetcher;
use futures::StreamExt;
use std::sync::Arc;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AcquireError {
    #[error("file exceeds {max} bytes")]
    TooLarge { max: usize },

    #[error("download error: {0}")]
    Download(String),

    #[error("read error: {0}")]
    Read(String),

    #[error("source has no retained bytes")]
    NoBytes,
}

/// Acquire the bytes of an attachment for local extraction. Bounded by
/// `max_bytes` — exceeding it short-circuits with `TooLarge` (the partial
/// buffer is dropped).
///
/// `inline_bytes` carries the bytes from `FileSource::InlineBytes` upstream,
/// because they are not stored anywhere else after the upload streams them.
pub async fn acquire_bytes(
    source: &AttachmentSource,
    inline_bytes: Option<&[u8]>,
    max_bytes: usize,
    fetcher: Arc<dyn SignedUrlFetcher>,
) -> Result<Vec<u8>, AcquireError> {
    match source {
        AttachmentSource::Inline => match inline_bytes {
            Some(b) if b.len() > max_bytes => Err(AcquireError::TooLarge { max: max_bytes }),
            Some(b) => Ok(b.to_vec()),
            None => Err(AcquireError::NoBytes),
        },

        AttachmentSource::Path(p) => {
            let meta = tokio::fs::metadata(p)
                .await
                .map_err(|e| AcquireError::Read(e.to_string()))?;
            if meta.len() as usize > max_bytes {
                return Err(AcquireError::TooLarge { max: max_bytes });
            }
            tokio::fs::read(p)
                .await
                .map_err(|e| AcquireError::Read(e.to_string()))
        }

        AttachmentSource::SignedUrl(url) => {
            let mut stream = fetcher
                .stream(url)
                .await
                .map_err(|e| AcquireError::Download(e.to_string()))?;
            let mut buf: Vec<u8> = Vec::new();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|e| AcquireError::Download(e.to_string()))?;
                if buf.len() + chunk.len() > max_bytes {
                    return Err(AcquireError::TooLarge { max: max_bytes });
                }
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
            1024,
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
            1024,
            Arc::new(MockFetcher::new(vec![])),
        )
        .await;
        assert!(matches!(r, Err(AcquireError::NoBytes)));
    }

    #[tokio::test]
    async fn inline_too_large_errors() {
        let big = vec![0u8; 100];
        let r = acquire_bytes(
            &AttachmentSource::Inline,
            Some(&big),
            10,
            Arc::new(MockFetcher::new(vec![])),
        )
        .await;
        assert!(matches!(r, Err(AcquireError::TooLarge { max: 10 })));
    }

    #[tokio::test]
    async fn signed_url_returns_fetched_bytes() {
        let r = acquire_bytes(
            &AttachmentSource::SignedUrl("https://example.com/x".into()),
            None,
            1024,
            Arc::new(MockFetcher::new(b"downloaded".to_vec())),
        )
        .await
        .unwrap();
        assert_eq!(r, b"downloaded");
    }

    #[tokio::test]
    async fn signed_url_too_large_errors() {
        // 100-byte body, cap at 10 -> TooLarge mid-stream.
        let r = acquire_bytes(
            &AttachmentSource::SignedUrl("https://example.com/x".into()),
            None,
            10,
            Arc::new(MockFetcher::new(vec![1u8; 100])),
        )
        .await;
        assert!(matches!(r, Err(AcquireError::TooLarge { max: 10 })));
    }
}
