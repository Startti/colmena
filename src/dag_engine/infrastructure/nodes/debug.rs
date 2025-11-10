// --- IMPORTACIONES AÑADIDAS ---
use crate::domain::node::{ExecutableNode, NodeInputs}; // Importa nuestro trait y tipo
use serde_json::{json, Value}; // Importa Value y la macro json!
use std::error::Error as StdError; // Importa el trait de error estándar
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
    ) -> Result<Value, Box<dyn StdError>> {
        let input_val = inputs.get("input").cloned().unwrap_or(Value::Null);
        println!("[LogNode]: {}", serde_json::to_string_pretty(&input_val)?);

        // También envuelve su salida para ser consistente
        Ok(json!({ "output": input_val }))
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
    ) -> Result<Value, Box<dyn StdError>> {
        // Devuelve su propia configuración como salida
        Ok(config.clone())
    }
    fn schema(&self) -> Value {
        json!({"type": "mock_input", "inputs": {}, "outputs": {"output": "any (from config)"}})
    }
}
