//! ToolkitNode — a node that exposes multiple sub-tools to the LLM.
//!
//! See `docs/superpowers/specs/2026-04-23-web-nodes-unified-design.md` §
//! "Runtime extension: multi-tool per node".
//!
//! The reserved input key `__sub_tool` identifies which sub-tool the LLM invoked.
//! Toolkit nodes branch on this key in their `execute()` implementation.

use crate::dag_engine::domain::node::ExecutableNode;
use crate::llm::domain::tools::ParameterProperty;
use serde_json::Value;
use std::collections::HashMap;

/// Reserved input key injected by `DagToolExecutor` to identify which sub-tool
/// of a toolkit node the LLM invoked.
pub const SUB_TOOL_INPUT_KEY: &str = "__sub_tool";

/// One sub-tool within a toolkit node.
#[derive(Debug, Clone)]
pub struct SubToolDefinition {
    /// Short programmatic name (no toolkit prefix). Examples: `"search"`, `"navigate"`.
    pub name: &'static str,
    /// Rich description shown to the LLM. Accuracy relies on this.
    pub description: String,
    /// JSON-Schema-style properties map for the LLM-visible parameters.
    pub properties: HashMap<String, ParameterProperty>,
    /// Names of the parameters the LLM is required to supply.
    pub required: Vec<String>,
}

/// Marker trait for nodes that expose multiple sub-tools.
///
/// A node that implements `ToolkitNode` is also an `ExecutableNode`; the runtime
/// dispatches on the reserved `__sub_tool` input key when executing the node.
///
/// `sub_tool_catalog(&config)` may return a **static** list (most toolkits here)
/// or a **dynamic** list computed from the node configuration (future work —
/// e.g. exposing each endpoint of an HTTP spec as its own sub-tool).
pub trait ToolkitNode: ExecutableNode {
    fn sub_tool_catalog(&self, config: &Value) -> Vec<SubToolDefinition>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sub_tool_input_key_is_reserved_constant() {
        assert_eq!(SUB_TOOL_INPUT_KEY, "__sub_tool");
    }

    #[test]
    fn sub_tool_definition_clone_is_cheap() {
        let def = SubToolDefinition {
            name: "search",
            description: "search the web".into(),
            properties: HashMap::new(),
            required: Vec::new(),
        };
        let cloned = def.clone();
        assert_eq!(cloned.name, "search");
    }
}
