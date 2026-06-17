use napi::bindgen_prelude::*;
use napi_derive::napi;
use serde_json::Value;
use std::sync::Arc;

/// Read-only handle to a `HashMapNodeRegistry`; inspection only, no DB.
#[napi]
pub struct Registry {
    inner: Arc<crate::dag_engine::infrastructure::registry::HashMapNodeRegistry>,
}

#[napi]
impl Registry {
    /// Return all registered node type names, sorted alphabetically.
    #[napi]
    pub fn node_types(&self) -> Vec<String> {
        use crate::dag_engine::application::ports::NodeRegistryPort;
        let mut keys: Vec<String> = self.inner.get_all_nodes().keys().cloned().collect();
        keys.sort();
        keys
    }

    /// Return the sub-tool catalog for a toolkit node, given its static config.
    /// Throws if `node_type` is not a registered toolkit node.
    #[napi]
    pub fn toolkit_catalog(&self, node_type: String, config: Value) -> Result<Value> {
        use crate::dag_engine::application::ports::NodeRegistryPort;
        let tk = self.inner.get_toolkit_node(&node_type).ok_or_else(|| {
            Error::new(
                Status::InvalidArg,
                format!("not a toolkit node: {}", node_type),
            )
        })?;
        let entries: Vec<Value> = tk
            .sub_tool_catalog(&config)
            .into_iter()
            .map(|s| {
                serde_json::json!({
                    "name": s.name.as_ref(),
                    "description": s.description,
                    "required": s.required,
                })
            })
            .collect();
        Ok(Value::Array(entries))
    }
}

/// Stub task-memory repository used only by `default_registry` so the
/// inspection-only smoke harness does not need a database. Every method is a
/// no-op; these registries are not used to execute graphs.
struct SmokeTaskMemory;

#[async_trait::async_trait]
impl crate::dag_engine::domain::state::DagTaskMemoryRepository for SmokeTaskMemory {
    async fn add_task(
        &self,
        _task: &crate::dag_engine::domain::state::DagTask,
    ) -> std::result::Result<(), crate::dag_engine::domain::error::DagError> {
        Ok(())
    }
    async fn update_task_result(
        &self,
        _task_id: &str,
        _result: Value,
    ) -> std::result::Result<(), crate::dag_engine::domain::error::DagError> {
        Ok(())
    }
    async fn get_tasks_for_run(
        &self,
        _session_id: &str,
    ) -> std::result::Result<
        Vec<crate::dag_engine::domain::state::DagTask>,
        crate::dag_engine::domain::error::DagError,
    > {
        Ok(vec![])
    }
    async fn get_first_uncompleted_task(
        &self,
        _session_id: &str,
    ) -> std::result::Result<
        Option<crate::dag_engine::domain::state::DagTask>,
        crate::dag_engine::domain::error::DagError,
    > {
        Ok(None)
    }
    async fn delete_task(
        &self,
        _task_id: &str,
    ) -> std::result::Result<(), crate::dag_engine::domain::error::DagError> {
        Ok(())
    }
    async fn clear_tasks_for_run(
        &self,
        _session_id: &str,
    ) -> std::result::Result<(), crate::dag_engine::domain::error::DagError> {
        Ok(())
    }
    async fn get_current_phase(
        &self,
        _session_id: &str,
    ) -> std::result::Result<Option<i32>, crate::dag_engine::domain::error::DagError> {
        Ok(None)
    }
    async fn get_uncompleted_tasks_for_phase(
        &self,
        _session_id: &str,
        _phase: i32,
    ) -> std::result::Result<
        Vec<crate::dag_engine::domain::state::DagTask>,
        crate::dag_engine::domain::error::DagError,
    > {
        Ok(vec![])
    }
    async fn save_phase_summary(
        &self,
        _session_id: &str,
        _phase: i32,
        _summary: &str,
    ) -> std::result::Result<(), crate::dag_engine::domain::error::DagError> {
        Ok(())
    }
    async fn get_phase_summaries(
        &self,
        _session_id: &str,
    ) -> std::result::Result<
        Vec<crate::dag_engine::domain::state::DagPhaseSummary>,
        crate::dag_engine::domain::error::DagError,
    > {
        Ok(vec![])
    }
}

/// Builds an inspection-only `HashMapNodeRegistry` with no live DB connections.
/// `PgPoolRegistry::new` is in-memory; pools are pinned lazily on first use,
/// which is fine because this registry is only used for introspection.
#[napi]
pub fn default_registry() -> Registry {
    use crate::dag_engine::infrastructure::pool_registry::{PgPoolRegistry, PoolConfig};
    use crate::dag_engine::infrastructure::registry::HashMapNodeRegistry;
    use crate::dag_engine::infrastructure::sql_port_factory::SqlPortFactory;
    use crate::llm::infrastructure::ConversationRepositoryFactory;

    let pools = Arc::new(PgPoolRegistry::new(PoolConfig::defaults()));
    let conv = Arc::new(ConversationRepositoryFactory::new(pools.clone()));
    let sql = Arc::new(SqlPortFactory::new(pools));
    let task_memory: Arc<dyn crate::dag_engine::domain::state::DagTaskMemoryRepository> =
        Arc::new(SmokeTaskMemory);
    let inner = HashMapNodeRegistry::new(conv, sql, Some(task_memory));
    Registry { inner }
}
