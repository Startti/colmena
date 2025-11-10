use crate::application::ports::NodeRegistryPort;
use crate::domain::node::ExecutableNode;
use crate::infrastructure::nodes::{debug::*, math::*}; // Importa nuestros nodos
use std::collections::HashMap;
use std::sync::Arc;

/// La implementación concreta (Adaptador) del `NodeRegistryPort`.
/// Utiliza un `HashMap` para almacenar instancias de todos los nodos disponibles.
pub struct HashMapNodeRegistry {
    nodes: HashMap<String, Arc<dyn ExecutableNode>>,
}

impl HashMapNodeRegistry {
    /// Construye un nuevo registro e inicializa todos los nodos estándar.
    pub fn new() -> Self {
        let mut nodes: HashMap<String, Arc<dyn ExecutableNode>> = HashMap::new();

        // --- Registrar Nodos de Depuración ---
        nodes.insert("mock_input".to_string(), Arc::new(MockInputNode));
        nodes.insert("log".to_string(), Arc::new(LogNode));

        // --- Registrar Nodos Matemáticos ---
        nodes.insert("add".to_string(), Arc::new(AddNode));
        nodes.insert("subtract".to_string(), Arc::new(SubtractNode));
        nodes.insert("multiply".to_string(), Arc::new(MultiplyNode));
        nodes.insert("divide".to_string(), Arc::new(DivideNode));

        nodes.insert("exponential".to_string(), Arc::new(ExponentialNode));

        Self { nodes }
    }
}

/// Implementación del "Puerto" de la aplicación.
impl NodeRegistryPort for HashMapNodeRegistry {
    /// Busca un nodo por su `node_type` string.
    fn get_node(&self, node_type: &str) -> Option<Arc<dyn ExecutableNode>> {
        // `cloned()` aquí clona el `Arc` (incrementa el contador de referencia),
        // no el nodo en sí, lo cual es barato.
        self.nodes.get(node_type).cloned()
    }
}