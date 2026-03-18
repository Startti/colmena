use crate::llm::domain::{LlmError, LlmMessage};
use async_trait::async_trait;

// Value Object para identificar hilos de conversación
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SessionId(pub String);

// Entidad que agrupa el historial
#[derive(Debug, Clone)]
pub struct Conversation {
    pub session_id: SessionId,
    pub messages: Vec<LlmMessage>,
}

// PUERTO: Contrato para el almacenamiento
#[async_trait]
pub trait ConversationRepository: Send + Sync {
    /// Recupera el historial completo
    async fn get_by_id(&self, id: &SessionId) -> Result<Conversation, LlmError>;

    /// Agrega un nuevo mensaje al historial
    async fn add_message(&self, id: &SessionId, message: LlmMessage) -> Result<(), LlmError>;

    /// Limpia el historial (opcional)
    async fn delete(&self, id: &SessionId) -> Result<(), LlmError>;
}
