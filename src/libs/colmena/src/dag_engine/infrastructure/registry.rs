use crate::dag_engine::application::ports::NodeRegistryPort;
use crate::dag_engine::application::ports::SubGraphExecutorPort;
use crate::dag_engine::application::secure_value_service::SecureValueService;
use crate::dag_engine::domain::node::ExecutableNode;
use crate::dag_engine::infrastructure::nodes::{
    debug::*, http::*, input::*, llm::*, math::*, orchestrator::*, output::*, python_node::*,
    socketio::*, sql::*, subgraph::*, task_memory_writer::*, trigger::*,
}; // Importa nuestros nodos
use std::collections::HashMap;
use std::sync::{Arc, Weak};

/// La implementación concreta (Adaptador) del `NodeRegistryPort`.
/// Utiliza un `HashMap` para almacenar instancias de todos los nodos disponibles.
pub struct HashMapNodeRegistry {
    nodes: HashMap<String, Arc<dyn ExecutableNode>>,
    toolkit_nodes:
        HashMap<String, Arc<dyn crate::dag_engine::domain::toolkit_node::ToolkitNode>>,
    subgraph_node: Option<Arc<SubGraphNode>>,
}

use crate::llm::infrastructure::ConversationRepositoryFactory;

impl HashMapNodeRegistry {
    /// Construye un nuevo registro e inicializa todos los nodos estándar.
    pub fn new(
        repository_factory: Arc<ConversationRepositoryFactory>,
        sql_port_factory: Arc<crate::dag_engine::infrastructure::sql_port_factory::SqlPortFactory>,
        task_memory_repo: Option<
            Arc<dyn crate::dag_engine::domain::state::DagTaskMemoryRepository>,
        >,
    ) -> Arc<Self> {
        HashMapNodeRegistry::new_with_secure_values(
            repository_factory,
            sql_port_factory,
            task_memory_repo,
            None,
        )
    }

    /// Construye el registro inyectando además un SecureValueService (para Secure Values en Tool Calling).
    pub fn new_with_secure_values(
        repository_factory: Arc<ConversationRepositoryFactory>,
        sql_port_factory: Arc<crate::dag_engine::infrastructure::sql_port_factory::SqlPortFactory>,
        task_memory_repo: Option<
            Arc<dyn crate::dag_engine::domain::state::DagTaskMemoryRepository>,
        >,
        secure_value_service: Option<Arc<SecureValueService>>,
    ) -> Arc<Self> {
        Arc::new_cyclic(|weak_self| {
            let mut nodes: HashMap<String, Arc<dyn ExecutableNode>> = HashMap::new();

            // --- Registrar Nodos de Depuración ---
            nodes.insert("mock_input".to_string(), Arc::new(MockInputNode));
            nodes.insert("log".to_string(), Arc::new(LogNode));
            nodes.insert("output".to_string(), Arc::new(OutputNode));

            // --- Registrar Nodos Matemáticos ---
            nodes.insert("add".to_string(), Arc::new(AddNode));
            nodes.insert("subtract".to_string(), Arc::new(SubtractNode));
            nodes.insert("multiply".to_string(), Arc::new(MultiplyNode));
            nodes.insert("divide".to_string(), Arc::new(DivideNode));

            nodes.insert("exponential".to_string(), Arc::new(ExponentialNode));

            // --- Registrar Nodos de Trigger ---
            nodes.insert("trigger_webhook".to_string(), Arc::new(TriggerWebhookNode));
            nodes.insert("input".to_string(), Arc::new(InputNode));

            // --- Registrar Nodos HTTP ---
            nodes.insert("http_request".to_string(), Arc::new(HttpNode));

            // --- Registrar Nodos Socket.IO ---
            nodes.insert("socketio_request".to_string(), Arc::new(SocketIoNode));

            // --- Register SQL Node ---
            nodes.insert(
                "sql_query".to_string(),
                Arc::new(SqlNode::new(sql_port_factory.clone())),
            );

            // --- Registrar Nodos LLM ---
            // Pass the weak reference to the registry to LlmNode
            let registry_weak = weak_self.clone() as Weak<dyn NodeRegistryPort>;
            let llm_node = LlmNode::new(
                repository_factory.clone(),
                registry_weak,
                task_memory_repo.clone(),
            );
            // If a SecureValueService is available, attach it so tool calls can decrypt secrets
            let llm_node = if let Some(svc) = secure_value_service.clone() {
                llm_node.with_secure_values(svc)
            } else {
                llm_node
            };
            nodes.insert("llm_call".to_string(), Arc::new(llm_node));

            // --- Registrar Nodos Python ---
            nodes.insert("python_script".to_string(), Arc::new(PythonNode));

            // --- Registrar Nodos Extraccion ---
            nodes.insert(
                "information_extraction".to_string(),
                Arc::new(
                    crate::dag_engine::infrastructure::nodes::extraction::ExtractionNode::new(
                        task_memory_repo.clone(),
                    ),
                ),
            );

            // --- Registrar Mock de Suspension ---
            nodes.insert(
                "suspend".to_string(),
                Arc::new(crate::dag_engine::infrastructure::nodes::suspend::SuspendNode),
            );

            // --- Registrar Loop Controller ---
            nodes.insert("loop_controller".to_string(), Arc::new(crate::dag_engine::infrastructure::nodes::loop_controller::LoopControllerNode::new()));

            // --- Registrar Orchestrator ---
            let registry_weak = weak_self.clone() as Weak<dyn NodeRegistryPort>;
            nodes.insert(
                "orchestrator".to_string(),
                Arc::new(OrchestratorNode::new(
                    task_memory_repo.clone(),
                    registry_weak,
                )),
            );

            // --- Registrar Task Memory Writer ---
            nodes.insert(
                "task_memory_writer".to_string(),
                Arc::new(TaskMemoryWriterNode::new(task_memory_repo.clone())),
            );

            // --- Registrar Planner ---
            nodes.insert(
                "planner".to_string(),
                Arc::new(
                    crate::dag_engine::infrastructure::nodes::planner::PlannerNode::new(
                        task_memory_repo.clone(),
                    ),
                ),
            );

            // --- Registrar Critic ---
            nodes.insert(
                "critic".to_string(),
                Arc::new(crate::dag_engine::infrastructure::nodes::critic::CriticNode::new()),
            );

            // --- Registrar Reactor ---
            nodes.insert(
                "reactor".to_string(),
                Arc::new(
                    crate::dag_engine::infrastructure::nodes::reactor::ReactorNode::new(Some(
                        task_memory_repo
                            .clone()
                            .unwrap_or_else(|| panic!("ReactorNode requires task_memory_repo")),
                    )),
                ),
            );

            // --- Registrar SubGraph ---
            let sub_node = Arc::new(SubGraphNode::new());
            nodes.insert(
                "subgraph".to_string(),
                sub_node.clone() as Arc<dyn ExecutableNode>,
            );

            Self {
                nodes,
                toolkit_nodes: HashMap::new(),
                subgraph_node: Some(sub_node),
            }
        })
    }
}

impl HashMapNodeRegistry {
    pub fn set_subgraph_executor(&self, executor: Arc<dyn SubGraphExecutorPort>) {
        if let Some(sub) = &self.subgraph_node {
            let _ = sub.executor.set(executor);
        }
    }

    /// Register a toolkit node. Stored in both maps (as `ExecutableNode` for
    /// normal DAG use and as `ToolkitNode` for sub-tool dispatch).
    ///
    /// Intended for **test** construction where the caller still has a fresh
    /// unique `Arc`. Production registration of toolkit nodes happens inside
    /// `new_with_secure_values` via direct field access on `&mut self`.
    /// Silently does nothing if the `Arc` is already shared (`Arc::get_mut`
    /// returns `None`).
    pub fn register_toolkit_node<N>(
        self: &mut Arc<Self>,
        node_type: impl Into<String>,
        node: Arc<N>,
    ) where
        N: crate::dag_engine::domain::toolkit_node::ToolkitNode + 'static,
    {
        if let Some(this) = Arc::get_mut(self) {
            let name = node_type.into();
            this.nodes
                .insert(name.clone(), node.clone() as Arc<dyn ExecutableNode>);
            this.toolkit_nodes.insert(
                name,
                node as Arc<dyn crate::dag_engine::domain::toolkit_node::ToolkitNode>,
            );
        }
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

    fn get_toolkit_node(
        &self,
        node_type: &str,
    ) -> Option<Arc<dyn crate::dag_engine::domain::toolkit_node::ToolkitNode>> {
        self.toolkit_nodes.get(node_type).cloned()
    }
}
