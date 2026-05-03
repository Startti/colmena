//! Catalog management for lazy tool loading. Pure data types and pure functions
//! over conversation messages — no I/O, no provider awareness.

use crate::llm::domain::tools::{ToolDefinition, ToolParameters};
use std::collections::HashMap;

pub struct CatalogEntry {
    pub name: String,
    pub summary: String,
}

pub fn reconstruct_discovered_set(
    _messages: &[crate::llm::domain::LlmMessage],
    _catalog: &[CatalogEntry],
) -> std::collections::HashSet<String> {
    std::collections::HashSet::new()
}

pub fn summary_for_catalog(_summary: Option<&str>, _description: &str) -> String {
    String::new()
}

pub fn build_describe_tool_definition(_pending: &[&CatalogEntry]) -> ToolDefinition {
    ToolDefinition {
        name: super::DESCRIBE_TOOL_NAME.to_string(),
        description: String::new(),
        parameters: ToolParameters {
            schema_type: "object".to_string(),
            properties: HashMap::new(),
            required: vec![],
        },
        input_schema_override: None,
    }
}
