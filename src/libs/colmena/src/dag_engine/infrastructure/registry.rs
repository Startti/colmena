use crate::dag_engine::application::ports::NodeRegistryPort;
use crate::dag_engine::application::ports::SubGraphExecutorPort;
use crate::dag_engine::application::secure_value_service::SecureValueService;
use crate::dag_engine::domain::node::ExecutableNode;
use crate::dag_engine::domain::toolkit_node::ToolkitNode;
use crate::dag_engine::infrastructure::nodes::{
    current_time::*, debug::*, document_nodes::*, http::*, input::*, llm::*, math::*,
    orchestrator::*, output::*, python_node::*, socketio::*, sql::*, subgraph::*,
    task_memory_writer::*, trigger::*,
}; // Importa nuestros nodos
use std::collections::HashMap;
use std::sync::{Arc, Weak};

/// La implementación concreta (Adaptador) del `NodeRegistryPort`.
/// Utiliza un `HashMap` para almacenar instancias de todos los nodos disponibles.
pub struct HashMapNodeRegistry {
    nodes: HashMap<String, Arc<dyn ExecutableNode>>,
    toolkit_nodes: HashMap<String, Arc<dyn ToolkitNode>>,
    subgraph_node: Option<Arc<SubGraphNode>>,
    /// Nodes that need to be notified on conversation close. Populated at
    /// construction time and consumed by `subscribe_lifecycle`.
    lifecycle_subscribers:
        Vec<Arc<dyn crate::web::domain::lifecycle::ConversationLifecycleSubscriber>>,
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

            // --- Registrar Nodo de Tiempo ---
            nodes.insert("current_time".to_string(), Arc::new(CurrentTimeNode));

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

            // --- Registrar secure_suspend (solo si hay SecureValueService) ---
            if let Some(svc) = secure_value_service.clone() {
                nodes.insert(
                    "secure_suspend".to_string(),
                    Arc::new(
                        crate::dag_engine::infrastructure::nodes::secure_suspend::SecureSuspendNode::new(
                            svc,
                        ),
                    ),
                );
            }

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

            // --- Register Tavily Client ---
            let tavily = Arc::new({
                use crate::dag_engine::infrastructure::nodes::tavily_client::TavilyClientNode;
                let n = TavilyClientNode::new();
                if let Some(svc) = secure_value_service.clone() {
                    n.with_secure_values(svc)
                } else {
                    n
                }
            });
            nodes.insert(
                "tavily_client".to_string(),
                tavily.clone() as Arc<dyn ExecutableNode>,
            );

            // --- Registrar Nodos de Documentos ---
            nodes.insert(
                "document_create".to_string(),
                Arc::new(DocumentCreateNode::new()),
            );
            nodes.insert(
                "document_edit".to_string(),
                Arc::new(DocumentEditNode::new()),
            );
            nodes.insert(
                "document_read".to_string(),
                Arc::new(DocumentReadNode::new()),
            );

            // --- Register API Explorer ---
            let api_explorer = {
                use crate::dag_engine::infrastructure::nodes::api_explorer::ApiExplorerNode;
                let n = ApiExplorerNode::new();
                if let Some(svc) = secure_value_service.clone() {
                    Arc::new(n.with_secure_values(svc))
                } else {
                    Arc::new(n)
                }
            };
            nodes.insert(
                "api_explorer".to_string(),
                api_explorer.clone() as Arc<dyn ExecutableNode>,
            );

            // --- Registrar SubGraph ---
            let sub_node = Arc::new(SubGraphNode::new());
            nodes.insert(
                "subgraph".to_string(),
                sub_node.clone() as Arc<dyn ExecutableNode>,
            );

            let mut toolkit_nodes: HashMap<String, Arc<dyn ToolkitNode>> = HashMap::new();
            toolkit_nodes.insert(
                "tavily_client".to_string(),
                tavily.clone() as Arc<dyn ToolkitNode>,
            );
            toolkit_nodes.insert(
                "api_explorer".to_string(),
                api_explorer.clone() as Arc<dyn ToolkitNode>,
            );

            let lifecycle_subscribers: Vec<
                Arc<dyn crate::web::domain::lifecycle::ConversationLifecycleSubscriber>,
            > = vec![api_explorer.clone()
                as Arc<dyn crate::web::domain::lifecycle::ConversationLifecycleSubscriber>];

            Self {
                nodes,
                toolkit_nodes,
                subgraph_node: Some(sub_node),
                lifecycle_subscribers,
            }
        })
    }

    /// Subscribes every lifecycle-aware node held by this registry to the shared
    /// [`ConversationLifecycleBus`]. Call once at engine setup so closing a
    /// conversation evicts per-conversation caches in stateful web nodes.
    pub async fn subscribe_lifecycle(
        &self,
        bus: &crate::web::domain::lifecycle::ConversationLifecycleBus,
    ) {
        for sub in &self.lifecycle_subscribers {
            bus.subscribe(sub.clone()).await;
        }
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
        N: ToolkitNode + 'static,
    {
        let this = Arc::get_mut(self);
        debug_assert!(
            this.is_some(),
            "register_toolkit_node called on a shared Arc<HashMapNodeRegistry>; \
             this helper is for fresh test-construction only"
        );
        if let Some(this) = this {
            let name = node_type.into();
            this.nodes
                .insert(name.clone(), node.clone() as Arc<dyn ExecutableNode>);
            this.toolkit_nodes
                .insert(name, node as Arc<dyn ToolkitNode>);
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

    fn get_toolkit_node(&self, node_type: &str) -> Option<Arc<dyn ToolkitNode>> {
        self.toolkit_nodes.get(node_type).cloned()
    }
}

#[cfg(test)]
mod registry_tavily_tests {
    use super::*;
    use crate::dag_engine::domain::error::DagError;
    use crate::dag_engine::domain::state::{DagPhaseSummary, DagTask, DagTaskMemoryRepository};
    use crate::dag_engine::infrastructure::pool_registry::{PgPoolRegistry, PoolConfig};
    use crate::dag_engine::infrastructure::sql_port_factory::SqlPortFactory;
    use crate::llm::infrastructure::ConversationRepositoryFactory;
    use async_trait::async_trait;
    use serde_json::Value;

    pub(super) struct StubTaskMemory;

    #[async_trait]
    impl DagTaskMemoryRepository for StubTaskMemory {
        async fn add_task(&self, _task: &DagTask) -> Result<(), DagError> {
            Ok(())
        }
        async fn update_task_result(&self, _task_id: &str, _result: Value) -> Result<(), DagError> {
            Ok(())
        }
        async fn get_tasks_for_run(&self, _session_id: &str) -> Result<Vec<DagTask>, DagError> {
            Ok(vec![])
        }
        async fn get_first_uncompleted_task(
            &self,
            _session_id: &str,
        ) -> Result<Option<DagTask>, DagError> {
            Ok(None)
        }
        async fn delete_task(&self, _task_id: &str) -> Result<(), DagError> {
            Ok(())
        }
        async fn clear_tasks_for_run(&self, _session_id: &str) -> Result<(), DagError> {
            Ok(())
        }
        async fn get_current_phase(&self, _session_id: &str) -> Result<Option<i32>, DagError> {
            Ok(None)
        }
        async fn get_uncompleted_tasks_for_phase(
            &self,
            _session_id: &str,
            _phase: i32,
        ) -> Result<Vec<DagTask>, DagError> {
            Ok(vec![])
        }
        async fn save_phase_summary(
            &self,
            _session_id: &str,
            _phase: i32,
            _summary: &str,
        ) -> Result<(), DagError> {
            Ok(())
        }
        async fn get_phase_summaries(
            &self,
            _session_id: &str,
        ) -> Result<Vec<DagPhaseSummary>, DagError> {
            Ok(vec![])
        }
    }

    pub(super) fn build_registry() -> Arc<HashMapNodeRegistry> {
        let pool_registry = Arc::new(PgPoolRegistry::new(PoolConfig::defaults()));
        let repo_factory = Arc::new(ConversationRepositoryFactory::new(pool_registry.clone()));
        let sql_factory = Arc::new(SqlPortFactory::new(pool_registry));
        let task_memory: Arc<dyn DagTaskMemoryRepository> = Arc::new(StubTaskMemory);
        HashMapNodeRegistry::new(repo_factory, sql_factory, Some(task_memory))
    }

    #[test]
    fn tavily_client_registered_as_executable_node() {
        let reg = build_registry();
        let node = reg.get_node("tavily_client");
        assert!(
            node.is_some(),
            "tavily_client must be registered as an ExecutableNode"
        );
    }

    #[test]
    fn tavily_client_registered_as_toolkit_node() {
        let reg = build_registry();
        let tk = reg.get_toolkit_node("tavily_client");
        assert!(
            tk.is_some(),
            "tavily_client must be registered as a ToolkitNode"
        );
        let cat = tk.unwrap().sub_tool_catalog(&serde_json::json!({}));
        assert_eq!(cat.len(), 2);
    }
}

#[cfg(test)]
mod registry_api_explorer_tests {
    use super::*;
    use crate::web::domain::lifecycle::ConversationLifecycleBus;

    #[test]
    fn api_explorer_registered_as_executable_node() {
        let reg = super::registry_tavily_tests::build_registry();
        let node = reg.get_node("api_explorer");
        assert!(node.is_some(), "api_explorer must be registered");
    }

    #[test]
    fn api_explorer_registered_as_toolkit_node_with_five_sub_tools() {
        let reg = super::registry_tavily_tests::build_registry();
        let tk = reg.get_toolkit_node("api_explorer");
        assert!(
            tk.is_some(),
            "api_explorer must be registered as ToolkitNode"
        );
        let cat = tk.unwrap().sub_tool_catalog(&serde_json::json!({}));
        assert_eq!(cat.len(), 5);
    }

    #[tokio::test]
    async fn subscribe_lifecycle_attaches_api_explorer_subscriber() {
        let reg = super::registry_tavily_tests::build_registry();
        let bus = ConversationLifecycleBus::new();
        reg.subscribe_lifecycle(&bus).await;
        // Notification should reach api_explorer's lifecycle hook without panic.
        bus.notify_conversation_closed("conv-x").await;
    }
}

#[cfg(test)]
mod registry_secure_suspend_tests {
    use super::*;
    use crate::dag_engine::application::secure_value_service::SecureValueService;
    use crate::dag_engine::domain::error::DagError;
    use crate::dag_engine::domain::secure_value_repository::SecureValueRepository;
    use async_trait::async_trait;

    pub(super) struct NoopRepo;

    #[async_trait]
    impl SecureValueRepository for NoopRepo {
        async fn persist(
            &self,
            _: &str,
            _: Option<&str>,
            _: &str,
            _: &str,
            _: &str,
            _: &str,
        ) -> Result<(), DagError> {
            Ok(())
        }
        async fn decrypt(
            &self,
            _: &str,
            _: Option<&str>,
            _: &str,
        ) -> Result<Option<String>, DagError> {
            Ok(None)
        }
        async fn exists(&self, _: &str, _: Option<&str>, _: &str) -> Result<bool, DagError> {
            Ok(false)
        }
        async fn cleanup(&self, _: &str) -> Result<(), DagError> {
            Ok(())
        }
        async fn cleanup_expired(&self) -> Result<u64, DagError> {
            Ok(0)
        }
        async fn cleanup_expired_for_run(
            &self,
            _session_id: &str,
            _agent_session_id: Option<&str>,
        ) -> Result<u64, DagError> {
            Ok(0)
        }
    }

    fn build_registry_with_secure_values() -> Arc<HashMapNodeRegistry> {
        let pool_registry = Arc::new(
            crate::dag_engine::infrastructure::pool_registry::PgPoolRegistry::new(
                crate::dag_engine::infrastructure::pool_registry::PoolConfig::defaults(),
            ),
        );
        let repo_factory = Arc::new(
            crate::llm::infrastructure::ConversationRepositoryFactory::new(pool_registry.clone()),
        );
        let sql_factory = Arc::new(
            crate::dag_engine::infrastructure::sql_port_factory::SqlPortFactory::new(pool_registry),
        );
        let task_memory: Arc<dyn crate::dag_engine::domain::state::DagTaskMemoryRepository> =
            Arc::new(super::registry_tavily_tests::StubTaskMemory);
        let svc = Arc::new(SecureValueService::new(Arc::new(NoopRepo) as Arc<_>));
        HashMapNodeRegistry::new_with_secure_values(
            repo_factory,
            sql_factory,
            Some(task_memory),
            Some(svc),
        )
    }

    #[test]
    fn secure_suspend_registered_when_secure_value_service_present() {
        let reg = build_registry_with_secure_values();
        assert!(
            reg.get_node("secure_suspend").is_some(),
            "secure_suspend must be registered when SecureValueService is wired"
        );
    }

    #[test]
    fn secure_suspend_not_registered_when_secure_value_service_absent() {
        let reg = super::registry_tavily_tests::build_registry();
        assert!(
            reg.get_node("secure_suspend").is_none(),
            "secure_suspend must NOT be registered without SecureValueService"
        );
    }
}
