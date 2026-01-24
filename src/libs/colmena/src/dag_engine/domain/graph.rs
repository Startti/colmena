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

    /// Configuración estática. `Value` ya implementa Clone por defecto.
    #[serde(default)]
    pub config: Value,
}

/// Representa una conexión (borde) desde un nodo a otro.
// --- AÑADIDO: Clone ---
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Edge {
    pub from: String,
    pub to: String,
}
