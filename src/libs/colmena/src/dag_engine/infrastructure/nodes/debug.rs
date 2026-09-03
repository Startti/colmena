// --- IMPORTACIONES AÑADIDAS ---
use crate::dag_engine::domain::lint::NodeCatalogEntry;
use crate::dag_engine::domain::node::{ExecutableNode, NodeInputs};
use serde_json::{json, Value};
use std::error::Error as StdError;
use std::sync::Arc;
// ------------------------------

// --- LogNode ---
/// Un nodo simple que imprime sus entradas a la consola y las pasa.
pub struct LogNode;
#[async_trait::async_trait]
impl ExecutableNode for LogNode {
    async fn execute(
        &self,
        inputs: &NodeInputs,
        _config: &Value,
        _state: &mut Value,
        _observer: Option<Arc<dyn crate::dag_engine::domain::observer::ExecutionObserver>>,
    ) -> Result<Value, Box<dyn StdError + Send + Sync>> {
        // Flexibilidad: Buscar llaves comunes o tomar TODO lo que venga inyectado (Auto-Flattening)
        let input_val = if let Some(val) = inputs
            .get("input")
            .or(inputs.get("result"))
            .or(inputs.get("output"))
        {
            val.clone()
        } else if !inputs.is_empty() {
            // Si no hay ninguna de las llaves estándar pero hay entradas, las mostramos todas
            // Esto es ideal para conexiones blunt { "from": "A", "to": "log" }
            let mut map = serde_json::Map::new();
            for (k, v) in inputs {
                map.insert(k.clone(), v.clone());
            }
            Value::Object(map)
        } else {
            Value::Null
        };

        println!("[LogNode]: {}", serde_json::to_string_pretty(&input_val)?);

        Ok(input_val)
    }
    fn description(&self) -> Option<&str> {
        Some("Log data to console for debugging. Useful for inspecting intermediate values in the flow.")
    }

    fn default_input(&self) -> Option<&str> {
        Some("input")
    }

    fn default_output(&self) -> Option<&str> {
        Some("output")
    }

    fn schema(&self) -> Value {
        json!({"type": "log", "inputs": {"input": "any"}, "outputs": {"output": "any"}})
    }

    fn config_schema(&self) -> Option<NodeCatalogEntry> {
        // `execute` takes `_config`; the payload is the `input` edge.
        Some(NodeCatalogEntry::no_config())
    }
}

// --- MockInputNode ---
/// ¡NO CAMBIAR! Este nodo es especial.
/// Su trabajo es emitir su config como el objeto de datos raíz.
pub struct MockInputNode;
#[async_trait::async_trait]
impl ExecutableNode for MockInputNode {
    async fn execute(
        &self,
        _inputs: &NodeInputs,
        config: &Value,
        _state: &mut Value,
        _observer: Option<Arc<dyn crate::dag_engine::domain::observer::ExecutionObserver>>,
    ) -> Result<Value, Box<dyn StdError + Send + Sync>> {
        // Devuelve su propia configuración como salida
        Ok(config.clone())
    }

    fn default_output(&self) -> Option<&str> {
        None // Emits raw config, no specific output field
    }

    fn schema(&self) -> Value {
        json!({"type": "mock_input", "inputs": {}, "outputs": {"output": "any (from config)"}})
    }

    fn config_schema(&self) -> Option<NodeCatalogEntry> {
        // `execute` returns `config.clone()`: every key is payload, none a setting.
        Some(NodeCatalogEntry::open_config())
    }
}
