use crate::llm::domain::LlmError;
use async_trait::async_trait;

/// Resume un único bloque de texto a una línea concisa (~target_chars), sin historia.
/// La implementación real usa un modelo barato; los tests usan un stub.
#[async_trait]
pub trait MessageSummarizer: Send + Sync {
    /// `target_chars` es un objetivo blando (se pide por prompt, no se hard-corta).
    async fn summarize(&self, text: &str, target_chars: usize) -> Result<String, LlmError>;
}
