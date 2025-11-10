use crate::domain::node::ExecutableNode;
use std::sync::Arc;

/// Define el "Puerto" que el `DagRunUseCase` utiliza para
/// obtener una implementación concreta de un nodo.
///
/// La infraestructura (`infrastructure`) será responsable de
/// implementar este trait.
pub trait NodeRegistryPort: Send + Sync {
    /// Busca y retorna una implementación de nodo basada en su
    /// `node_type` (ej. "add", "log").
    fn get_node(&self, node_type: &str) -> Option<Arc<dyn ExecutableNode>>;
}