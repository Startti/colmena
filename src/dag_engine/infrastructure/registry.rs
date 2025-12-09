use crate::dag_engine::application::ports::NodeRegistryPort;
use crate::dag_engine::domain::node::ExecutableNode;
use crate::dag_engine::infrastructure::nodes::{
    debug::*, http::*, llm::*, math::*, python_node::*, trigger::*,
}; // Importa nuestros nodos
use std::collections::HashMap;
use std::sync::{Arc, Weak};

/// La implementación concreta (Adaptador) del `NodeRegistryPort`.
/// Utiliza un `HashMap` para almacenar instancias de todos los nodos disponibles.
pub struct HashMapNodeRegistry {
    nodes: HashMap<String, Arc<dyn ExecutableNode>>,
}

use crate::llm::infrastructure::ConversationRepositoryFactory;

impl HashMapNodeRegistry {
    /// Construye un nuevo registro e inicializa todos los nodos estándar.
    pub fn new(repository_factory: Arc<ConversationRepositoryFactory>) -> Arc<Self> {
        Arc::new_cyclic(|weak_self| {
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

            // --- Registrar Nodos de Trigger ---
            nodes.insert("trigger_webhook".to_string(), Arc::new(TriggerWebhookNode));

            // --- Registrar Nodos HTTP ---
            nodes.insert("http_request".to_string(), Arc::new(HttpNode));

            // --- Registrar Nodos LLM ---
            // Pass the weak reference to the registry to LlmNode
            let registry_weak = weak_self.clone() as Weak<dyn NodeRegistryPort>;
            nodes.insert(
                "llm_call".to_string(),
                Arc::new(LlmNode::new(repository_factory.clone(), registry_weak)),
            );

            // --- Registrar Nodos Python ---
            nodes.insert("python_script".to_string(), Arc::new(PythonNode));

            Self { nodes }
        })
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

    fn get_all_nodes(&self) -> std::collections::HashMap<String, Arc<dyn ExecutableNode>> {
        self.nodes.clone()
    }
}
