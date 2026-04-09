use crate::dag_engine::domain::error::DagError;
use crate::dag_engine::domain::node::ExecutableNode;
use serde_json::Value;
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

    /// Retorna todos los nodos registrados.
    fn get_all_nodes(&self) -> std::collections::HashMap<String, Arc<dyn ExecutableNode>>;
}

/// Define el "Puerto" que un Nodo SubGraph utiliza para ejecutar
/// su grafo hijo interno. Esto evita la dependencia circular entre
/// la capa de Nodos y el DagRunUseCase.
#[async_trait::async_trait]
pub trait SubGraphExecutorPort: Send + Sync {
    /// Ejecuta un subgrafo desde cero.
    async fn run_subgraph(
        &self,
        session_id: &str,
        graph_json: Value,
        global_state: Value,
        observer: Option<Arc<dyn crate::dag_engine::domain::observer::ExecutionObserver>>,
    ) -> Result<Value, DagError>;

    /// Reanuda un subgrafo suspendido tras un Human-in-the-Loop.
    async fn resume_subgraph(
        &self,
        session_id: &str,
        answer: String,
        observer: Option<Arc<dyn crate::dag_engine::domain::observer::ExecutionObserver>>,
    ) -> Result<Value, DagError>;
}
