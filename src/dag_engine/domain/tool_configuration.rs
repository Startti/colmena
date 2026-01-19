use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

/// Configuration for exposing a node as a tool
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolConfiguration {
    /// Name of the tool (shown to LLM)
    pub name: String,

    /// Human-readable description for the LLM
    pub description: String,

    /// Node type to execute
    pub node_type: String,

    /// Fixed configuration values (not exposed to LLM)
    pub fixed_config: HashMap<String, Value>,

    /// Which input parameters to expose to the LLM
    /// If None, expose all inputs not in fixed_config
    pub exposed_inputs: Option<Vec<String>>,

    /// Optional JSON Schema for parameters to override node schema
    pub parameters: Option<Value>,
}
