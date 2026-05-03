//! Port para construir el `FileProviderRepository` apropiado según el provider.
//!
//! El use case `LlmCallUseCase` depende solo de este trait, no del adapter
//! concreto. Esto permite inyectar implementaciones de test sin importar nada
//! de `infrastructure/`.

use crate::llm::domain::{FileProviderRepository, LlmError, ProviderKind};
use std::sync::Arc;

/// Resuelve `(ProviderKind, api_key)` → `Arc<dyn FileProviderRepository>`.
///
/// Implementaciones concretas (la `FileProviderFactory` real) viven en
/// `infrastructure/`. Tests pueden proveer un stub que devuelve un mock.
pub trait FileProviderFactoryPort: Send + Sync {
    /// Construye un repositorio de Files API para el provider dado.
    ///
    /// # Errors
    /// - `LlmError::ProviderLimitation` si el provider no soporta Files API
    ///   (ej. `Mock`).
    fn build(
        &self,
        kind: ProviderKind,
        api_key: String,
    ) -> Result<Arc<dyn FileProviderRepository>, LlmError>;
}
