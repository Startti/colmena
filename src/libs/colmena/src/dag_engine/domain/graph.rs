use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

/// La estructura raíz que representa un `graph.json` completo.
// --- AÑADIDO: Clone ---
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Graph {
    /// Un mapa de todos los nodos en el grafo, usando su ID como clave.
    pub nodes: HashMap<String, NodeConfig>,

    /// Una lista de todas las conexiones (bordes) entre los nodos.
    pub edges: Vec<Edge>,

    /// Optional IANA timezone identifier (e.g. `"Europe/Madrid"`) for the graph.
    /// Consumed by temporal-context features when injecting time-aware data into LLM prompts.
    #[serde(default)]
    pub timezone: Option<String>,

    /// Optional human-readable location string (e.g. `"Madrid, España"`).
    /// Consumed by geographic-context features for prompt injection.
    #[serde(default)]
    pub location: Option<String>,

    /// Optional BCP-47 locale tag (e.g. `"es-ES"`, `"en-US"`).
    /// Consumed by localization features for prompt injection.
    #[serde(default)]
    pub locale: Option<String>,
}

/// Representa la configuración de un único nodo en el grafo.
// --- AÑADIDO: Clone ---
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct NodeConfig {
    /// El tipo de nodo (ej. "add", "log").
    #[serde(rename = "type")]
    pub node_type: String,

    #[serde(default)]
    pub config: Value,

    /// Optional condition to determine if the node should run based on global `__colmena_loop_status`.
    /// Example: "FINISHED_PHASE", "NEXT_TURN", "FINISHED"
    #[serde(default)]
    pub trigger_on: Option<String>,

    /// Maximum number of times this node can be executed during a single DAG run.
    #[serde(default)]
    pub max_total_calls: Option<u32>,

    /// Maximum number of times this node can be executed, broken down by the caller node's ID.
    #[serde(default)]
    pub max_calls_from: Option<HashMap<String, u32>>,
}

/// Representa una conexión (borde) desde un nodo a otro.
// --- AÑADIDO: Clone ---
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Edge {
    pub from: String,
    pub to: String,

    /// Optional flag to indicate if this edge forms a backward cycle.
    /// Cyclic edges don't block the target node from executing initially.
    #[serde(default)]
    pub cyclic: Option<bool>,
}

impl Graph {
    /// Validates structural invariants the engine depends on.
    ///
    /// Currently rejects:
    /// - Node IDs containing `/` — the engine uses `/` to separate path-qualified
    ///   `node_id`s in subgraph hierarchies (`subgraph_node/inner_node`). Allowing
    ///   `/` in user-defined IDs would make the resulting paths ambiguous.
    /// - A misconfigured `memory_mode` on any tool (wrong node type, unknown value, or
    ///   a memory-bearing mode without a `connection_url` backend).
    /// - Malformed `node_schema` on any tool in `tool_configurations`.
    pub fn validate(&self) -> Result<(), crate::dag_engine::domain::error::DagError> {
        use crate::dag_engine::domain::error::DagError;
        use crate::dag_engine::domain::tool_configuration::{
            memory_backend_missing_reason, parse_node_schema, validate_memory_mode, MemoryMode,
            NodeSchema,
        };

        for node_id in self.nodes.keys() {
            if node_id.contains('/') {
                return Err(DagError::InvalidNodeId {
                    node_id: node_id.clone(),
                    reason: "character '/' is reserved for subgraph path qualifiers",
                });
            }
        }

        // Validate every tool's `node_schema` up front (fail-fast, before
        // execution / token spend). A malformed schema (e.g. an `array` field
        // without `items`) is rejected here with a precise, actionable message
        // instead of degrading silently or — historically — panicking. Since
        // `validate()` also runs for the child graphs of a `subgraph`, the error
        // surfaces as a tool result that an agent can read and self-correct on.
        for (node_id, node) in &self.nodes {
            let Some(tools) = node
                .config
                .get("tool_configurations")
                .and_then(|v| v.as_object())
            else {
                continue;
            };
            for (tool_name, tool_cfg) in tools {
                // Reject a misconfigured `memory_mode` up front (wrong node type, or a
                // mode not yet active) so a bad graph fails at load instead of silently
                // dropping the tool at build time. Only touches configs that set the
                // field — absent → stateless → skipped.
                if let Some(mm_value) = tool_cfg.get("memory_mode") {
                    let mode: MemoryMode =
                        serde_json::from_value(mm_value.clone()).map_err(|e| {
                            DagError::InvalidToolSchema {
                                node_id: node_id.clone(),
                                tool_name: tool_name.clone(),
                                reason: format!("invalid memory_mode value: {e}"),
                            }
                        })?;
                    let cfg_node_type = tool_cfg
                        .get("node_type")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    validate_memory_mode(cfg_node_type, mode).map_err(|reason| {
                        DagError::InvalidToolSchema {
                            node_id: node_id.clone(),
                            tool_name: tool_name.clone(),
                            reason,
                        }
                    })?;
                    // A memory-bearing mode also needs a persistence backend
                    // (connection_url); without one, memory would not survive across
                    // runs. Provable-absent → reject; unknowable (external child
                    // graph) → allowed.
                    if let Some(reason) =
                        memory_backend_missing_reason(cfg_node_type, mode, tool_cfg)
                    {
                        return Err(DagError::InvalidToolSchema {
                            node_id: node_id.clone(),
                            tool_name: tool_name.clone(),
                            reason,
                        });
                    }
                }

                let Some(schema_value) = tool_cfg.get("node_schema") else {
                    continue;
                };
                let schema: NodeSchema =
                    serde_json::from_value(schema_value.clone()).map_err(|e| {
                        DagError::InvalidToolSchema {
                            node_id: node_id.clone(),
                            tool_name: tool_name.clone(),
                            reason: format!("malformed node_schema: {e}"),
                        }
                    })?;
                parse_node_schema(&schema).map_err(|reason| DagError::InvalidToolSchema {
                    node_id: node_id.clone(),
                    tool_name: tool_name.clone(),
                    reason,
                })?;
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn graph_with_node_id(id: &str) -> Graph {
        let json = json!({
            "nodes": {
                id: { "type": "math", "config": {} }
            },
            "edges": []
        });
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn validate_rejects_slash_in_node_id() {
        let g = graph_with_node_id("router/inner");
        let err = g.validate().unwrap_err().to_string();
        assert!(
            err.contains("router/inner"),
            "error should name the offending id, got: {}",
            err
        );
        assert!(
            err.contains("'/'"),
            "error should mention the forbidden char, got: {}",
            err
        );
    }

    #[test]
    fn validate_accepts_clean_node_id() {
        let g = graph_with_node_id("router_inner");
        assert!(g.validate().is_ok());
    }

    /// Build a one-node graph whose `agent` node declares a single tool with the
    /// given `node_schema` JSON.
    fn graph_with_tool_schema(schema: serde_json::Value) -> Graph {
        let json = json!({
            "nodes": {
                "agent": {
                    "type": "llm_call",
                    "config": {
                        "tool_configurations": {
                            "my_tool": { "node_type": "http_request", "node_schema": schema }
                        }
                    }
                }
            },
            "edges": []
        });
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn validate_rejects_array_tool_field_without_items() {
        // Regression: an `array` node_schema field without `items` must be caught
        // at validate() (fail-fast), not panic or degrade silently.
        let g = graph_with_tool_schema(json!({
            "rows": { "type": "array", "required": true, "description": "list" }
        }));
        let err = g.validate().unwrap_err().to_string();
        assert!(err.contains("my_tool"), "should name the tool, got: {err}");
        assert!(err.contains("agent"), "should name the node, got: {err}");
        assert!(
            err.contains("items"),
            "should explain the missing `items`, got: {err}"
        );
    }

    #[test]
    fn validate_accepts_array_tool_field_with_items() {
        let g = graph_with_tool_schema(json!({
            "rows": {
                "type": "array",
                "items": { "type": "object" },
                "required": true,
                "description": "list"
            }
        }));
        assert!(g.validate().is_ok());
    }

    #[test]
    fn validate_accepts_node_without_tools() {
        let g = graph_with_node_id("plain_node");
        assert!(g.validate().is_ok());
    }

    /// One-node graph whose `agent` declares a single tool `my_tool` with the given
    /// node_type and memory_mode.
    fn graph_with_tool_memory(node_type: &str, memory_mode: serde_json::Value) -> Graph {
        let json = json!({
            "nodes": {
                "agent": {
                    "type": "llm_call",
                    "config": {
                        "tool_configurations": {
                            "my_tool": { "node_type": node_type, "memory_mode": memory_mode }
                        }
                    }
                }
            },
            "edges": []
        });
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn validate_rejects_memory_mode_on_non_capable_tool() {
        let g = graph_with_tool_memory("http_request", json!("dynamic"));
        let err = g.validate().unwrap_err().to_string();
        assert!(err.contains("my_tool"), "should name the tool, got: {err}");
        assert!(
            err.contains("only valid on memory-bearing"),
            "should explain the allowlist, got: {err}"
        );
    }

    #[test]
    fn validate_rejects_memory_mode_without_connection_url() {
        // persistent and dynamic are active but need a persistence backend; a tool
        // with no connection_url must be rejected at load.
        for mode in ["persistent", "dynamic"] {
            let g = graph_with_tool_memory("llm_call", json!(mode));
            let err = g.validate().unwrap_err().to_string();
            assert!(
                err.contains("needs a connection_url"),
                "{mode} should require a connection_url, got: {err}"
            );
        }
    }

    #[test]
    fn validate_accepts_memory_mode_with_connection_url() {
        for mode in ["persistent", "dynamic"] {
            let g: Graph = serde_json::from_value(json!({
                "nodes": { "agent": { "type": "llm_call", "config": {
                    "tool_configurations": { "asesor": {
                        "node_type": "llm_call",
                        "memory_mode": mode,
                        "fixed_config": { "connection_url": "${DATABASE_URL}" }
                    }}
                }}},
                "edges": []
            }))
            .unwrap();
            assert!(
                g.validate().is_ok(),
                "{mode} with connection_url should pass"
            );
        }
    }

    #[test]
    fn validate_rejects_unknown_memory_mode_value() {
        let g = graph_with_tool_memory("llm_call", json!("sometimes"));
        let err = g.validate().unwrap_err().to_string();
        assert!(
            err.contains("invalid memory_mode value"),
            "should flag the bad enum value, got: {err}"
        );
    }

    #[test]
    fn validate_accepts_stateless_and_absent_memory_mode() {
        assert!(graph_with_tool_memory("http_request", json!("stateless"))
            .validate()
            .is_ok());
        // absent memory_mode → default stateless → accepted
        let g: Graph = serde_json::from_value(json!({
            "nodes": { "agent": { "type": "llm_call", "config": {
                "tool_configurations": { "my_tool": { "node_type": "subgraph" } }
            }}},
            "edges": []
        }))
        .unwrap();
        assert!(g.validate().is_ok());
    }
}

#[cfg(test)]
mod temporal_context_tests {
    use super::*;

    #[test]
    fn graph_without_optional_fields_parses_with_none_defaults() {
        let json = r#"{"nodes": {}, "edges": []}"#;
        let g: Graph = serde_json::from_str(json).expect("must parse");
        assert!(g.timezone.is_none(), "timezone should be None when omitted");
        assert!(g.location.is_none(), "location should be None when omitted");
        assert!(g.locale.is_none(), "locale should be None when omitted");
    }

    #[test]
    fn graph_with_all_three_fields_parses_them() {
        let json = r#"{
            "nodes": {},
            "edges": [],
            "timezone": "Europe/Madrid",
            "location": "Madrid, España",
            "locale": "es-ES"
        }"#;
        let g: Graph = serde_json::from_str(json).expect("must parse");
        assert_eq!(g.timezone.as_deref(), Some("Europe/Madrid"));
        assert_eq!(g.location.as_deref(), Some("Madrid, España"));
        assert_eq!(g.locale.as_deref(), Some("es-ES"));
    }

    #[test]
    fn graph_with_partial_fields_parses() {
        let json = r#"{
            "nodes": {},
            "edges": [],
            "locale": "en-US"
        }"#;
        let g: Graph = serde_json::from_str(json).expect("must parse");
        assert!(g.timezone.is_none());
        assert!(g.location.is_none());
        assert_eq!(g.locale.as_deref(), Some("en-US"));
    }
}
