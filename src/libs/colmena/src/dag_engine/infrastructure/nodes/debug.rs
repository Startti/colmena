// --- IMPORTACIONES AÑADIDAS ---
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
        let input_val = inputs.get("input").cloned().unwrap_or(Value::Null);
        println!("[LogNode]: {}", serde_json::to_string_pretty(&input_val)?);

        // También envuelve su salida para ser consistente
        Ok(json!({ "output": input_val }))
    }
    fn description(&self) -> Option<&str> {
        Some("Log data to console for debugging. Useful for inspecting intermediate values in the flow.")
    }

    fn schema(&self) -> Value {
        json!({"type": "log", "inputs": {"input": "any"}, "outputs": {"output": "any"}})
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
    fn schema(&self) -> Value {
        json!({"type": "mock_input", "inputs": {}, "outputs": {"output": "any (from config)"}})
    }
}
