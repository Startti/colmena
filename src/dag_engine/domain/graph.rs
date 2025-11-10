use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

/// La estructura raíz que representa un `graph.json` completo.
#[derive(Debug, Deserialize, Serialize)]
pub struct Graph {
    /// Un mapa de todos los nodos en el grafo, usando su ID como clave.
    pub nodes: HashMap<String, NodeConfig>,

    /// Una lista de todas las conexiones (bordes) entre los nodos.
    pub edges: Vec<Edge>,
}

/// Representa la configuración de un único nodo en el grafo.
#[derive(Debug, Deserialize, Serialize)]
pub struct NodeConfig {
    /// El tipo de nodo (ej. "add", "log").
    /// Se usa `#[serde(rename = "type")]` para mapear desde el JSON "type".
    #[serde(rename = "type")]
    pub node_type: String,

    /// Configuración estática y específica del nodo (ej. un prompt, una URL).
    /// `#[serde(default)]` asegura que esto sea un `Value::Null` si falta en el JSON.
    #[serde(default)]
    pub config: Value,
}

/// Representa una conexión (borde) desde un nodo a otro.
#[derive(Debug, Deserialize, Serialize)]
pub struct Edge {
    /// El punto de salida. Formato: "node_id.output_name"
    /// (ej. "start_data.output" o "start_data.output.field_a")
    pub from: String,

    /// El punto de entrada. Formato: "node_id.input_name"
    /// (ej. "add_step.input_a")
    pub to: String,
}
