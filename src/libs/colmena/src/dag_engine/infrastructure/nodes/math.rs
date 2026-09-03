// --- IMPORTACIONES AÑADIDAS ---
use crate::dag_engine::domain::lint::{FieldSpec, NodeCatalogEntry};
use crate::dag_engine::domain::node::{ExecutableNode, NodeInputs};
use serde_json::{json, Value};
use std::error::Error as StdError;
use std::sync::Arc;
use thiserror::Error;
// ------------------------------

#[derive(Error, Debug)]
enum MathError {
    #[error("Entrada no es un número: {0}")]
    NotANumber(String),
    #[error("División por cero")]
    DivisionByZero,
}

/// Extrae un f64 de un `Value` o devuelve un error `MathError`.
fn get_f64(val: Option<&Value>, input_name: &str) -> Result<f64, MathError> {
    val.and_then(Value::as_f64)
        .ok_or_else(|| MathError::NotANumber(input_name.to_string()))
}

// --- AddNode ---
pub struct AddNode;
#[async_trait::async_trait]
impl ExecutableNode for AddNode {
    async fn execute(
        &self,
        inputs: &NodeInputs,
        _config: &Value,
        _state: &mut Value,
        _observer: Option<Arc<dyn crate::dag_engine::domain::observer::ExecutionObserver>>,
    ) -> Result<Value, Box<dyn StdError + Send + Sync>> {
        let a = get_f64(inputs.get("a"), "a")?;
        let b = get_f64(inputs.get("b"), "b")?;
        Ok(json!({ "output": a + b }))
    }
    fn default_output(&self) -> Option<&str> {
        Some("output")
    }

    fn schema(&self) -> Value {
        json!({"type": "add", "inputs": {"a": "number", "b": "number"}, "outputs": {"output": "number"}})
    }

    fn config_schema(&self) -> Option<NodeCatalogEntry> {
        // Reads only its `a`/`b` inputs; `execute` takes `_config`.
        Some(NodeCatalogEntry::no_config())
    }
}

// --- SubtractNode ---
pub struct SubtractNode;
#[async_trait::async_trait]
impl ExecutableNode for SubtractNode {
    async fn execute(
        &self,
        inputs: &NodeInputs,
        _config: &Value,
        _state: &mut Value,
        _observer: Option<Arc<dyn crate::dag_engine::domain::observer::ExecutionObserver>>,
    ) -> Result<Value, Box<dyn StdError + Send + Sync>> {
        let a = get_f64(inputs.get("a"), "a")?;
        let b = get_f64(inputs.get("b"), "b")?;
        Ok(json!({ "output": a - b }))
    }
    fn default_output(&self) -> Option<&str> {
        Some("output")
    }

    fn schema(&self) -> Value {
        json!({"type": "subtract", "inputs": {"a": "number", "b": "number"}, "outputs": {"output": "number"}})
    }

    fn config_schema(&self) -> Option<NodeCatalogEntry> {
        // Reads only its `a`/`b` inputs; `execute` takes `_config`.
        Some(NodeCatalogEntry::no_config())
    }
}

// --- MultiplyNode ---
pub struct MultiplyNode;
#[async_trait::async_trait]
impl ExecutableNode for MultiplyNode {
    async fn execute(
        &self,
        inputs: &NodeInputs,
        _config: &Value,
        _state: &mut Value,
        _observer: Option<Arc<dyn crate::dag_engine::domain::observer::ExecutionObserver>>,
    ) -> Result<Value, Box<dyn StdError + Send + Sync>> {
        let a = get_f64(inputs.get("a"), "a")?;
        let b = get_f64(inputs.get("b"), "b")?;
        Ok(json!({ "output": a * b }))
    }
    fn default_output(&self) -> Option<&str> {
        Some("output")
    }

    fn schema(&self) -> Value {
        json!({"type": "multiply", "inputs": {"a": "number", "b": "number"}, "outputs": {"output": "number"}})
    }

    fn config_schema(&self) -> Option<NodeCatalogEntry> {
        // Reads only its `a`/`b` inputs; `execute` takes `_config`.
        Some(NodeCatalogEntry::no_config())
    }
}

// --- DivideNode ---
pub struct DivideNode;
#[async_trait::async_trait]
impl ExecutableNode for DivideNode {
    async fn execute(
        &self,
        inputs: &NodeInputs,
        _config: &Value,
        _state: &mut Value,
        _observer: Option<Arc<dyn crate::dag_engine::domain::observer::ExecutionObserver>>,
    ) -> Result<Value, Box<dyn StdError + Send + Sync>> {
        let a = get_f64(inputs.get("a"), "a")?;
        let b = get_f64(inputs.get("b"), "b")?;

        if b == 0.0 {
            return Err(Box::new(MathError::DivisionByZero));
        }
        Ok(json!({ "output": a / b }))
    }
    fn default_output(&self) -> Option<&str> {
        Some("output")
    }

    fn schema(&self) -> Value {
        json!({"type": "divide", "inputs": {"a": "number", "b": "number"}, "outputs": {"output": "number"}})
    }

    fn config_schema(&self) -> Option<NodeCatalogEntry> {
        // Reads only its `a`/`b` inputs; `execute` takes `_config`.
        Some(NodeCatalogEntry::no_config())
    }
}

pub struct ExponentialNode;
#[async_trait::async_trait]
impl ExecutableNode for ExponentialNode {
    async fn execute(
        &self,
        inputs: &NodeInputs,
        config: &Value,
        _state: &mut Value,
        _observer: Option<Arc<dyn crate::dag_engine::domain::observer::ExecutionObserver>>,
    ) -> Result<Value, Box<dyn StdError + Send + Sync>> {
        // 1. Obtener la base de las entradas
        let base = get_f64(inputs.get("input"), "input")?;

        // 2. Obtener el exponente de la configuración
        let exponent = get_f64(config.get("exponent"), "config.exponent")?;

        // 3. Calcular la potencia
        let result = base.powf(exponent);

        Ok(json!({ "output": result }))
    }

    fn default_input(&self) -> Option<&str> {
        Some("input")
    }

    fn default_output(&self) -> Option<&str> {
        Some("output")
    }

    fn schema(&self) -> Value {
        json!({
            "type": "exponential",
            "inputs": {"input": "number"},
            "config": {"exponent": "number"},
            "outputs": {"output": "number"}
        })
    }

    fn config_schema(&self) -> Option<NodeCatalogEntry> {
        // Reads `config.exponent` (required); `base` arrives as an input.
        Some(
            NodeCatalogEntry::no_config()
                .with_field("exponent", FieldSpec::of_type("number").required()),
        )
    }
}
