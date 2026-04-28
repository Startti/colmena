use thiserror::Error;

/// Define los errores específicos del dominio para el motor DAG.
#[derive(Debug, Error)]
pub enum DagError {
    /// Se produce cuando el `DagRunUseCase` detecta una dependencia circular
    /// durante el ordenamiento topológico.
    #[error("Ciclo detectado en el grafo. No se puede ejecutar.")]
    CycleDetected,

    /// Se produce si un `node_type` en `graph.json` no existe
    /// en el registro de nodos (`NodeRegistryPort`).
    #[error("Nodo de tipo '{0}' no encontrado en el registro.")]
    NodeTypeNotFound(String),

    /// Se produce si un `node_id` referenciado en un borde
    /// no existe en el mapa de `nodes` del grafo.
    #[error("Nodo con ID '{0}' no encontrado en el grafo.")]
    NodeIdNotFound(String),

    /// Un error genérico que envuelve un error de ejecución
    /// devuelto por un `ExecutableNode`.
    #[error("Error de ejecución en el nodo: {0}")]
    NodeExecution(String),

    /// Ocurre cuando hay un error persistiendo o recuperando el estado del DAG (Postgres).
    #[error("Error de estado: {0}")]
    StateError(String),

    /// El graph contiene un node ID que viola una invariante estructural
    /// (por ejemplo, contiene `/` reservado para path qualifiers).
    #[error("Invalid node id '{node_id}': {reason}")]
    InvalidNodeId {
        node_id: String,
        reason: &'static str,
    },
}
