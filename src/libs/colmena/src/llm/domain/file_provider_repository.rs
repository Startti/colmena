//! Port para subir archivos al Files API de un proveedor LLM.
//!
//! Implementado por adapters específicos por proveedor en
//! `llm/infrastructure/files/`. Consumido por `LlmCallUseCase` cuando
//! materializa archivos cuya `FileSource` es `SignedUrl`.

use crate::llm::domain::{LlmError, ProviderFileRef, ProviderKind};
use async_trait::async_trait;
use bytes::Bytes;
use futures::Stream;
use std::pin::Pin;
use std::time::Duration;

/// Stream de bytes que el adapter consume para hacer upload.
/// Se construye típicamente desde `reqwest::Response::bytes_stream()`.
pub type BoxedByteStream =
    Pin<Box<dyn Stream<Item = Result<Bytes, std::io::Error>> + Send>>;

#[async_trait]
pub trait FileProviderRepository: Send + Sync {
    /// Sube un archivo al Files API consumiendo el stream.
    async fn upload_streaming(
        &self,
        stream: BoxedByteStream,
        mime_type: &str,
        filename: &str,
    ) -> Result<ProviderFileRef, LlmError>;

    /// TTL del archivo en este proveedor.
    /// `None` = no expira (Anthropic, OpenAI).
    /// `Some(d)` = expira en `d` desde uploaded_at (Gemini = 48h).
    fn ttl(&self) -> Option<Duration>;

    /// Identifica al proveedor para keying del cache.
    fn provider(&self) -> ProviderKind;
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockProvider;
    #[async_trait]
    impl FileProviderRepository for MockProvider {
        async fn upload_streaming(
            &self,
            _stream: BoxedByteStream,
            mime_type: &str,
            filename: &str,
        ) -> Result<ProviderFileRef, LlmError> {
            Ok(ProviderFileRef {
                provider: ProviderKind::Mock,
                provider_file_id: "mock-id".into(),
                mime_type: mime_type.into(),
                filename: filename.into(),
                expires_at: None,
            })
        }
        fn ttl(&self) -> Option<Duration> { None }
        fn provider(&self) -> ProviderKind { ProviderKind::Mock }
    }

    #[tokio::test]
    async fn mock_provider_returns_ref() {
        let p = MockProvider;
        let stream: BoxedByteStream = Box::pin(futures::stream::empty());
        let r = p.upload_streaming(stream, "application/pdf", "x.pdf").await.unwrap();
        assert_eq!(r.provider_file_id, "mock-id");
        assert_eq!(r.provider, ProviderKind::Mock);
    }
}
