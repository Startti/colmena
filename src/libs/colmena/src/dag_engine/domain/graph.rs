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
