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

    /// Retorna una descripción legible por humanos (y LLMs) de lo que hace el nodo.
    /// Esto es crucial para que el LLM entienda cuándo y cómo usar este nodo como herramienta.
    fn description(&self) -> Option<&str> {
        None
    }
}
