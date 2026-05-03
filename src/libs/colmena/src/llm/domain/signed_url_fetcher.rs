//! Port para descargar el cuerpo de una signed URL como stream de bytes.
//!
//! El use case `LlmCallUseCase` depende solo de este trait; las implementaciones
//! concretas (HTTP via reqwest, stubs de test, etc.) viven en `infrastructure/`.

use crate::llm::domain::{BoxedByteStream, LlmError};
use async_trait::async_trait;

/// Abre un stream de bytes contra una URL firmada (típicamente GCS V4).
///
/// Implementaciones deben respetar la semántica de la firma: la URL trae sus
/// credenciales en query params; añadir headers de autorización rompería la firma.
#[async_trait]
pub trait SignedUrlFetcher: Send + Sync {
    /// Descarga el cuerpo de la URL como `BoxedByteStream`.
    ///
    /// # Errors
    /// - `LlmError::NetworkError` en fallas de transporte (DNS, TCP, TLS, timeout).
    /// - `LlmError::SignedUrlFetchFailed` en cualquier status no-2xx.
    async fn stream(&self, url: &str) -> Result<BoxedByteStream, LlmError>;
}
