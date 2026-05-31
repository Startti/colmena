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
use crate::llm::domain::AttachmentRegistry;
use crate::storage::domain::OutputStorageRepository;
use std::collections::HashMap;
use std::sync::{Arc, Weak};

/// La implementación concreta (Adaptador) del `NodeRegistryPort`.
/// Utiliza un `HashMap` para almacenar instancias de todos los nodos disponibles.
pub struct HashMapNodeRegistry {
    nodes: HashMap<String, Arc<dyn ExecutableNode>>,
    toolkit_nodes: HashMap<String, Arc<dyn ToolkitNode>>,
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
            None,
            None,
        )
    }

    /// Construye el registro inyectando además un SecureValueService (para
    /// Secure Values en Tool Calling), un adapter de almacenamiento de
    /// outputs generados, y un AttachmentRegistry compartido. Cuando `storage`
    /// es `None`, los nodos de generación de media (image_generation, etc.)
    /// no quedan registrados — igual patrón que `secure_suspend` con
    /// `SecureValueService`. Cuando `attachment_registry` es `None`, los
    /// nodos media siguen registrados pero NO registran sus outputs (fail-soft).
    pub fn new_with_secure_values(
        repository_factory: Arc<ConversationRepositoryFactory>,
        sql_port_factory: Arc<crate::dag_engine::infrastructure::sql_port_factory::SqlPortFactory>,
        task_memory_repo: Option<
            Arc<dyn crate::dag_engine::domain::state::DagTaskMemoryRepository>,
        >,
        secure_value_service: Option<Arc<SecureValueService>>,
        storage: Option<Arc<dyn OutputStorageRepository>>,
        attachment_registry: Option<Arc<dyn AttachmentRegistry>>,
    ) -> Arc<Self> {
        Arc::new_cyclic(|weak_self| {
            let mut nodes: HashMap<String, Arc<dyn ExecutableNode>> = HashMap::new();

            // --- Plan A: AttachmentStreamResolver ---
            // Build a composite resolver from the registry + storage. When
            // both are present, http_request can resolve `$attachment:<document_id>`
            // by looking up the registry; on miss it falls back to treating the
            // identifier as a raw storage_key (backwards compat). When either
            // dependency is missing, the resolver is not built and http_request
            // falls back to using `storage.read_stream` directly (legacy).
            let attachment_resolver: Option<
                Arc<dyn crate::llm::domain::attachments::AttachmentStreamResolver>,
            > = match (attachment_registry.as_ref(), storage.as_ref()) {
                (Some(reg), Some(store)) => Some(Arc::new(
                    crate::llm::infrastructure::attachments::AttachmentStreamResolverImpl::new(
                        reg.clone(),
                        store.clone(),
                    ),
                )),
                _ => None,
            };

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
            // Pass the storage adapter so the HTTP node can resolve
            // `$attachment:<id>` placeholders in the body to bytes read
            // from outputs of image_generation/edit/tts.
            //
            // Plan A: also pass the AttachmentStreamResolver when available so
            // multipart parts sourced from `$attachment:<document_id>` look up
            // the document via the registry (with raw-storage_key fallback).
            let mut http_node = HttpNode::new();
            if let Some(st) = storage.clone() {
                http_node = http_node.with_storage(st);
            }
            if let Some(resolver) = attachment_resolver.clone() {
                http_node = http_node.with_attachment_resolver(resolver);
            }
            nodes.insert("http_request".to_string(), Arc::new(http_node));

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
            let mut llm_node = LlmNode::new(
                repository_factory.clone(),
                registry_weak,
                task_memory_repo.clone(),
            );
            // If a SecureValueService is available, attach it so tool calls can decrypt secrets
            if let Some(svc) = secure_value_service.clone() {
                llm_node = llm_node.with_secure_values(svc);
            }
            // Attach storage so the AttachmentResolver can read bytes for
            // `provider: Generated` rows when doing cross-provider lazy upload.
            if let Some(st) = storage.clone() {
                llm_node = llm_node.with_storage(st);
            }
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

            // --- Registrar Output Parser ---
            nodes.insert(
                "output_parser".to_string(),
                Arc::new(
                    crate::dag_engine::infrastructure::nodes::output_parser::OutputParserNode,
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

            // --- Registrar Image Generation (solo si hay storage adapter) ---
            if let Some(storage_arc) = storage.clone() {
                use crate::dag_engine::infrastructure::nodes::image_generation::ImageGenerationNode;
                let mut img = ImageGenerationNode::new(storage_arc);
                if let Some(svc) = secure_value_service.clone() {
                    img = img.with_secure_values(svc);
                }
                if let Some(reg) = attachment_registry.clone() {
                    img = img.with_attachment_registry(reg);
                }
                nodes.insert("image_generation".to_string(), Arc::new(img));
            }

            // --- Registrar TTS (solo si hay storage adapter) ---
            if let Some(storage_arc) = storage.clone() {
                use crate::dag_engine::infrastructure::nodes::tts::TtsNode;
                let mut tts = TtsNode::new(storage_arc);
                if let Some(svc) = secure_value_service.clone() {
                    tts = tts.with_secure_values(svc);
                }
                if let Some(reg) = attachment_registry.clone() {
                    tts = tts.with_attachment_registry(reg);
                }
                nodes.insert("tts".to_string(), Arc::new(tts));
            }

            // --- Registrar Image Edit (solo si hay storage adapter) ---
            if let Some(storage_arc) = storage.clone() {
                use crate::dag_engine::infrastructure::nodes::image_edit::ImageEditNode;
                let mut edit = ImageEditNode::new(storage_arc);
                if let Some(svc) = secure_value_service.clone() {
                    edit = edit.with_secure_values(svc);
                }
                if let Some(reg) = attachment_registry.clone() {
                    edit = edit.with_attachment_registry(reg);
                }
                nodes.insert("image_edit".to_string(), Arc::new(edit));
            }

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

            Self {
                nodes,
                toolkit_nodes,
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

    #[test]
    fn output_parser_registered_as_executable_node() {
        let reg = build_registry();
        assert!(
            reg.get_node("output_parser").is_some(),
            "output_parser must be registered as an ExecutableNode"
        );
    }
}

#[cfg(test)]
mod registry_api_explorer_tests {
    use super::*;

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
            None,
            None,
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

#[cfg(test)]
mod media_tools_injection_tests {
    //! Verifies that when the LLM node is configured with `tool_configurations`
    //! referencing media nodes (image_generation, image_edit, tts), the
    //! resulting `DagToolExecutor::available_tools()` returns ToolDefinitions
    //! for each — same path the LLM node uses before sending to the provider.
    //!
    //! Reproduces the exact tool_configurations shape from
    //! `tests/graphs/agents/multimedia_agent.json`. If this passes but the
    //! real graph doesn't tool-call, the bug lives downstream (request body,
    //! provider adapter, etc.). If this fails, the bug is in tool_configurations
    //! parsing or registry wiring.
    use super::*;
    use crate::dag_engine::domain::tool_configuration::ToolConfiguration;
    use crate::dag_engine::infrastructure::dag_tool_executor::DagToolExecutor;
    use crate::llm::domain::tool_executor::ToolExecutor;
    use crate::storage::domain::OutputStorageRepository;
    use crate::storage::infrastructure::LocalCacheStorageAdapter;
    use std::collections::HashMap;

    fn build_registry_with_storage() -> Arc<HashMapNodeRegistry> {
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
        let storage: Arc<dyn OutputStorageRepository> = Arc::new(LocalCacheStorageAdapter::new());
        HashMapNodeRegistry::new_with_secure_values(
            repo_factory,
            sql_factory,
            Some(task_memory),
            None,
            Some(storage),
            None,
        )
    }

    /// Reproduce the multimedia_agent.json tool_configurations as a parsed
    /// HashMap<String, ToolConfiguration>. Mirrors what `llm.rs` builds from
    /// the user's config JSON at execute time.
    fn media_tool_configs() -> HashMap<String, ToolConfiguration> {
        let json = serde_json::json!({
            "generate_image": {
                "name": "generate_image",
                "node_type": "image_generation",
                "description": "Generate an image from a prompt.",
                "node_schema": {
                    "provider": { "type": "string",  "fixed": "openai" },
                    "model":    { "type": "string",  "fixed": "gpt-image-1" },
                    "api_key":  { "type": "string",  "fixed": "${OPENAI_API_KEY}" },
                    "size":     { "type": "string",  "fixed": "1024x1024" },
                    "n":        { "type": "integer", "fixed": 1 },
                    "prompt":   { "type": "string", "required": true, "description": "Prompt." }
                }
            },
            "edit_image": {
                "name": "edit_image",
                "node_type": "image_edit",
                "description": "Edit an existing image.",
                "node_schema": {
                    "provider":   { "type": "string",  "fixed": "openai" },
                    "model":      { "type": "string",  "fixed": "gpt-image-1" },
                    "api_key":    { "type": "string",  "fixed": "${OPENAI_API_KEY}" },
                    "size":       { "type": "string",  "fixed": "1024x1024" },
                    "n":          { "type": "integer", "fixed": 1 },
                    "source_url": { "type": "string", "required": true, "description": "URL." },
                    "prompt":     { "type": "string", "required": true, "description": "Edit." }
                }
            },
            "synthesize_speech": {
                "name": "synthesize_speech",
                "node_type": "tts",
                "description": "TTS.",
                "node_schema": {
                    "provider": { "type": "string", "fixed": "openai" },
                    "model":    { "type": "string", "fixed": "tts-1" },
                    "api_key":  { "type": "string", "fixed": "${OPENAI_API_KEY}" },
                    "voice":    { "type": "string", "fixed": "alloy" },
                    "format":   { "type": "string", "fixed": "mp3" },
                    "text":     { "type": "string", "required": true, "description": "Text." }
                }
            }
        });
        serde_json::from_value(json).expect("tool_configurations must parse")
    }

    #[tokio::test]
    async fn registry_has_three_media_nodes_when_storage_present() {
        let reg = build_registry_with_storage();
        assert!(reg.get_node("image_generation").is_some());
        assert!(reg.get_node("image_edit").is_some());
        assert!(reg.get_node("tts").is_some());
    }

    #[tokio::test]
    async fn available_tools_exposes_all_three_media_tools() {
        let reg = build_registry_with_storage();
        let configs = media_tool_configs();
        let executor = DagToolExecutor::new(
            reg as Arc<dyn crate::dag_engine::application::ports::NodeRegistryPort>,
            configs,
        );
        let tools = executor.available_tools().await;

        let names: std::collections::HashSet<&str> =
            tools.iter().map(|t| t.name.as_str()).collect();
        // The three media tool names from the configurations MUST be present.
        for required in ["generate_image", "edit_image", "synthesize_speech"] {
            assert!(
                names.contains(required),
                "expected tool '{}' to be exposed; got: {:?}",
                required,
                names
            );
        }
    }

    #[tokio::test]
    async fn auto_enable_filter_keeps_configured_tools_without_enabled_tools_field() {
        // Reproduces `filter_enabled_tools` behavior — the default code path
        // when the graph does NOT declare `enabled_tools`. Configured aliases
        // alone must be enough to keep the tools in the array passed to OpenAI.
        use crate::dag_engine::infrastructure::nodes::llm::filter_enabled_tools;

        let reg = build_registry_with_storage();
        let configs = media_tool_configs();
        let aliases: std::collections::HashSet<String> = configs.keys().cloned().collect();
        let executor = DagToolExecutor::new(
            reg as Arc<dyn crate::dag_engine::application::ports::NodeRegistryPort>,
            configs,
        );
        let all_tools = executor.available_tools().await;

        // No enabled_tools provided → filter relies on auto-enable.
        let filtered = filter_enabled_tools(all_tools, None, &aliases);
        let names: std::collections::HashSet<&str> =
            filtered.iter().map(|t| t.name.as_str()).collect();
        assert!(
            names.contains("generate_image"),
            "generate_image dropped after filter_enabled_tools(None): got {:?}",
            names
        );
        assert!(names.contains("edit_image"));
        assert!(names.contains("synthesize_speech"));
    }

    /// Reproduces the broken JSON shape we hit before adding `type` on every
    /// fixed field. The `type` field is now OPTIONAL when `fixed` is present —
    /// so this configuration must parse cleanly and produce 3 tools.
    /// (Before the fix, parsing failed silently and tools array was empty.)
    #[tokio::test]
    async fn tool_configurations_with_only_fixed_fields_parse_cleanly() {
        let raw = serde_json::json!({
            "generate_image": {
                "name": "generate_image",
                "node_type": "image_generation",
                "node_schema": {
                    "provider": { "fixed": "openai" },
                    "model":    { "fixed": "gpt-image-1" },
                    "api_key":  { "fixed": "${OPENAI_API_KEY}" },
                    "prompt":   { "type": "string", "required": true, "description": "p" }
                }
            }
        });
        let configs: HashMap<String, ToolConfiguration> =
            serde_json::from_value(raw).expect("must parse cleanly without type on fixed fields");
        assert_eq!(configs.len(), 1);

        let reg = build_registry_with_storage();
        let executor = DagToolExecutor::new(
            reg as Arc<dyn crate::dag_engine::application::ports::NodeRegistryPort>,
            configs,
        );
        let tools = executor.available_tools().await;
        let names: std::collections::HashSet<&str> =
            tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains("generate_image"));
    }

    #[tokio::test]
    async fn generate_image_tool_has_only_prompt_as_llm_visible_param() {
        let reg = build_registry_with_storage();
        let configs = media_tool_configs();
        let executor = DagToolExecutor::new(
            reg as Arc<dyn crate::dag_engine::application::ports::NodeRegistryPort>,
            configs,
        );
        let tools = executor.available_tools().await;

        let gen = tools
            .iter()
            .find(|t| t.name == "generate_image")
            .expect("generate_image must be present");
        // All `fixed` fields should be hidden; only `prompt` is LLM-visible.
        let props: Vec<&str> = gen
            .parameters
            .properties
            .keys()
            .map(|s| s.as_str())
            .collect();
        assert_eq!(
            props,
            vec!["prompt"],
            "expected exactly ['prompt']; got {:?}",
            props
        );
        assert_eq!(gen.parameters.required, vec!["prompt"]);
    }
}
