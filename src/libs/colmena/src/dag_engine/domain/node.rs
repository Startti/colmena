use crate::dag_engine::domain::initializable_node::InitializableNode;
use crate::dag_engine::domain::observer::ExecutionObserver;
use serde_json::Value;
use std::collections::HashMap;
use std::error::Error as StdError;
use std::sync::Arc;

/// Un alias de tipo para las entradas de un nodo.
/// Usamos un HashMap para que las entradas sean nombradas (ej. "a", "b", "prompt").
pub type NodeInputs = HashMap<String, Value>;

/// El "Puerto" principal para todos los nodos ejecutables.
/// Define el contrato que debe implementar cualquier nodo (Adaptador).
/// `Send + Sync` son necesarios para que el trait pueda ser usado de forma segura
/// a través de threads, especialmente con `async`.
#[async_trait::async_trait]
pub trait ExecutableNode: Send + Sync {
    /// El método principal que ejecuta la lógica del nodo.
    ///
    /// # Argumentos
    /// * `inputs` - Un HashMap que contiene los outputs resueltos de los nodos anteriores.
    /// * `config` - El objeto `Value` de configuración estática del nodo desde `graph.json`.
    /// * `state` - Una referencia mutable al estado global del grafo (lo usaremos más en M2).
    /// * `observer` - Un observador opcional para notificar eventos de ejecución.
    ///
    /// # Retorna
    /// Un `Result` que contiene el `Value` de salida del nodo o un error.
    async fn execute(
        &self,
        inputs: &NodeInputs,
        config: &Value,
        state: &mut Value,
        observer: Option<Arc<dyn ExecutionObserver>>,
    ) -> Result<Value, Box<dyn StdError + Send + Sync>>;

    /// Retorna un `Value` de JSON Schema que describe la configuración del nodo,
    /// sus entradas esperadas y sus salidas.
    /// Esto será usado por el `frontend` en M2/M4.
    fn schema(&self) -> Value;

    /// Declares which configuration fields this node accepts, or `None` while
    /// the node has not been migrated to declare them.
    ///
    /// This is the phase-2 source of truth for the graph linter: rather than
    /// trusting the hand-maintained `docs/node_configurations.json` to say which
    /// fields a node reads, a node states it here, and a test asserts the two
    /// agree. `None` means "not yet declared — fall back to the catalog"; it is
    /// the default so migration is node-by-node and never breaks an existing
    /// implementation.
    ///
    /// Only the machine-checkable facts live here — field names, requiredness,
    /// accepted values, read-only. Prose (descriptions, examples, defaults)
    /// stays in the catalog, which is what humans and agents read.
    ///
    /// Deliberately separate from [`Self::schema`], whose free-form `inputs`
    /// block the tool executor consumes by substring; the two must not be
    /// conflated.
    fn config_schema(&self) -> Option<crate::dag_engine::domain::lint::NodeCatalogEntry> {
        None
    }

    /// Retorna una descripción legible por humanos (y LLMs) de lo que hace el nodo.
    /// Esto es crucial para que el LLM entienda cuándo y cómo usar este nodo como herramienta.
    fn description(&self) -> Option<&str> {
        None
    }

    /// The default input port name for this node.
    /// Used by the engine when an incoming edge specifies no target field.
    /// Return `None` if the node requires explicit field mapping.
    fn default_input(&self) -> Option<&str> {
        None
    }

    /// The default output port name for this node.
    /// Used by the engine when an outgoing edge specifies no source field.
    /// Return `None` if the node has no single primary output.
    fn default_output(&self) -> Option<&str> {
        None
    }

    /// Optional: return a reference to self as [`InitializableNode`] so the
    /// tool executor can call `initialize()` to enrich the tool description
    /// with database schema / capability context before the first LLM turn.
    ///
    /// Default: `None` (node does not participate in pre-flight initialization).
    /// Override in nodes that implement [`InitializableNode`] (currently `sql_query`).
    fn as_initializable(&self) -> Option<&dyn InitializableNode> {
        None
    }
}
