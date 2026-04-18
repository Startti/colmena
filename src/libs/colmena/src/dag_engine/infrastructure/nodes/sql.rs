//! SQL query node — executes PostgreSQL queries from a DAG.
//!
//! As an LLM tool (via `tool_configurations`), the primary use case. Configure
//! `connection_url`, `permissions`, `runtime_limits`, and `guardrail_*` as fixed
//! values in `node_schema`. The LLM only sees and provides the `query` parameter.

use crate::dag_engine::application::sql_execution_service::SqlExecutionService;
use crate::dag_engine::domain::initializable_node::{InitContext, InitializableNode};
use crate::dag_engine::domain::node::{ExecutableNode, NodeInputs};
use crate::dag_engine::domain::observer::ExecutionObserver;
use crate::dag_engine::domain::sql_errors::SqlNodeError;
use crate::dag_engine::domain::sql_permissions::SqlPermissions;
use crate::dag_engine::domain::sql_ports::{FunctionInfo, FunctionRegistryPort, SqlConnectionPort, TableInfo};
use crate::dag_engine::infrastructure::sql_function_registry::PgRegistryAdapter;
use crate::dag_engine::infrastructure::sql_llm_critic::LlmCriticAdapter;
use crate::dag_engine::infrastructure::sql_pool_adapter::PgPoolAdapter;
use crate::dag_engine::infrastructure::sql_static_validator::StaticRuleValidator;
use serde_json::{json, Value};
use std::error::Error as StdError;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;

pub struct SqlNode {
    pool_adapter: Arc<PgPoolAdapter>,
    initialized: Arc<RwLock<bool>>,
    /// Cached metadata for LLM description injection.
    cached_description: Arc<RwLock<Option<String>>>,
}

impl SqlNode {
    pub fn new() -> Self {
        let pool_adapter = Arc::new(PgPoolAdapter::new());
        Self {
            pool_adapter,
            initialized: Arc::new(RwLock::new(false)),
            cached_description: Arc::new(RwLock::new(None)),
        }
    }

    /// Resolve `${ENV_VAR}` in a string.
    fn resolve_env_vars(input: &str) -> Result<String, String> {
        let mut result = String::new();
        let mut last_end = 0;
        while let Some(start) = input[last_end..].find("${") {
            let absolute_start = last_end + start;
            result.push_str(&input[last_end..absolute_start]);
            if let Some(end) = input[absolute_start..].find('}') {
                let absolute_end = absolute_start + end;
                let var_name = &input[absolute_start + 2..absolute_end];
                let val = std::env::var(var_name)
                    .map_err(|_| format!("Env var {} not found", var_name))?;
                result.push_str(&val);
                last_end = absolute_end + 1;
            } else {
                result.push_str(&input[absolute_start..]);
                last_end = input.len();
                break;
            }
        }
        result.push_str(&input[last_end..]);
        Ok(result)
    }

    /// Build the description supplement from table and function metadata.
    fn build_description_supplement(
        tables: &[TableInfo],
        functions: &[FunctionInfo],
        permissions: &SqlPermissions,
        max_rows: u64,
    ) -> String {
        let mut lines = Vec::new();

        if !tables.is_empty() {
            let mut current_schema = String::new();
            for table in tables {
                if table.schema_name != current_schema {
                    current_schema = table.schema_name.clone();
                    lines.push(format!("\nAvailable tables (schema: {}):", current_schema));
                }
                match &table.description {
                    Some(desc) => lines.push(format!("  - {} -- {}", table.table_name, desc)),
                    None => lines.push(format!("  - {}", table.table_name)),
                }
            }
        }

        if !functions.is_empty() {
            lines.push(String::new());
            lines.push("Available functions (sandbox):".to_string());
            for func in functions {
                let params = func.parameters.as_deref().unwrap_or("");
                lines.push(format!(
                    "  - {}({}) -- {}",
                    func.function_name, params, func.description
                ));
            }
        }

        lines.push(String::new());
        lines.push(format!("{} | Max rows: {}", permissions.describe_for_llm(), max_rows));
        lines.push("Use introspection queries to discover column details when needed.".to_string());

        lines.join("\n")
    }

    /// Generate a simple session ID using a timestamp.
    fn new_session_id() -> String {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        format!("sql-session-{}", ts)
    }
}

impl Default for SqlNode {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl InitializableNode for SqlNode {
    async fn initialize(
        &self,
        config: &Value,
    ) -> Result<InitContext, Box<dyn StdError + Send + Sync>> {
        let connection_url_raw = config
            .get("connection_url")
            .and_then(|v| v.as_str())
            .ok_or("sql_query node requires 'connection_url' in config")?;
        let connection_url = Self::resolve_env_vars(connection_url_raw)
            .map_err(|e| format!("Failed to resolve connection_url: {}", e))?;

        let runtime_limits = config.get("runtime_limits");
        let statement_timeout_ms = runtime_limits
            .and_then(|r| r.get("statement_timeout_ms"))
            .and_then(|v| v.as_u64())
            .unwrap_or(30_000);
        let work_mem_mb = runtime_limits
            .and_then(|r| r.get("work_mem_mb"))
            .and_then(|v| v.as_u64())
            .unwrap_or(64);
        let max_rows = runtime_limits
            .and_then(|r| r.get("max_rows"))
            .and_then(|v| v.as_u64())
            .unwrap_or(100);

        let permissions = SqlPermissions::from_config(config.get("permissions"))
            .map_err(|e| format!("Invalid permissions config: {}", e))?;

        // Connect (via trait)
        {
            let conn: &dyn SqlConnectionPort = &*self.pool_adapter;
            conn.connect(&connection_url, statement_timeout_ms, work_mem_mb)
                .await
                .map_err(|e| format!("Failed to initialize SQL pool: {}", e))?;
        }

        // Create registry adapter sharing the same pool
        let sandbox_schema = permissions.sandbox_schema().to_string();
        let pool_ref = self.pool_adapter.pool_ref();
        let registry = PgRegistryAdapter::new(pool_ref, sandbox_schema);

        // ensure_schema + list_functions via trait
        {
            let reg: &dyn FunctionRegistryPort = &registry;
            let _ = reg.ensure_schema().await; // Best-effort
        }

        // Load table metadata
        let allowed_schemas: Vec<String> = config
            .get("permissions")
            .and_then(|p| p.get("allowed_schemas"))
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
            .unwrap_or_default();

        let tables: Vec<TableInfo> = {
            let conn: &dyn SqlConnectionPort = &*self.pool_adapter;
            conn.load_table_metadata(&allowed_schemas)
                .await
                .unwrap_or_default()
        };

        let functions: Vec<FunctionInfo> = {
            let reg: &dyn FunctionRegistryPort = &registry;
            reg.list_functions().await.unwrap_or_default()
        };

        let supplement = Self::build_description_supplement(&tables, &functions, &permissions, max_rows);

        *self.cached_description.write().await = Some(supplement.clone());
        *self.initialized.write().await = true;

        Ok(InitContext {
            description_supplement: Some(supplement),
        })
    }
}

#[async_trait::async_trait]
impl ExecutableNode for SqlNode {
    async fn execute(
        &self,
        inputs: &NodeInputs,
        config: &Value,
        _state: &mut Value,
        observer: Option<Arc<dyn ExecutionObserver>>,
    ) -> Result<Value, Box<dyn StdError + Send + Sync>> {
        let query = inputs
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or("sql_query node requires 'query' input")?;

        let permissions = SqlPermissions::from_config(config.get("permissions"))
            .map_err(|e| format!("Invalid permissions: {}", e))?;

        let runtime_limits = config.get("runtime_limits");
        let max_rows = runtime_limits
            .and_then(|r| r.get("max_rows"))
            .and_then(|v| v.as_u64())
            .unwrap_or(100);

        let validator = Arc::new(StaticRuleValidator) as Arc<dyn crate::dag_engine::domain::sql_ports::SqlValidatorPort>;

        let critic_enabled = config
            .get("guardrail_llm")
            .and_then(|g| g.get("enabled"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let critic: Option<Arc<dyn crate::dag_engine::domain::sql_ports::SqlCriticPort>> = if critic_enabled {
            let guardrail_cfg = config.get("guardrail_llm").unwrap();
            let provider = guardrail_cfg.get("provider").and_then(|v| v.as_str()).unwrap_or("openai").to_string();
            let model = guardrail_cfg.get("model").and_then(|v| v.as_str()).unwrap_or("gpt-4o-mini").to_string();
            let api_key_raw = guardrail_cfg.get("api_key").and_then(|v| v.as_str()).unwrap_or("");
            let api_key = Self::resolve_env_vars(api_key_raw).unwrap_or_default();

            Some(Arc::new(LlmCriticAdapter::new(
                provider,
                model,
                api_key,
            )) as Arc<dyn crate::dag_engine::domain::sql_ports::SqlCriticPort>)
        } else {
            None
        };

        let sandbox_schema = permissions.sandbox_schema().to_string();
        let pool_ref = self.pool_adapter.pool_ref();
        let registry = Arc::new(PgRegistryAdapter::new(pool_ref, sandbox_schema))
            as Arc<dyn crate::dag_engine::domain::sql_ports::FunctionRegistryPort>;

        let service = SqlExecutionService::new(
            self.pool_adapter.clone() as Arc<dyn crate::dag_engine::domain::sql_ports::SqlConnectionPort>,
            validator,
            critic,
            registry,
        );

        let session_id = Self::new_session_id();
        let schema_context = self
            .cached_description
            .read()
            .await
            .clone()
            .unwrap_or_default();

        println!("[SqlNode] Executing: {}", &query[..query.len().min(100)]);

        match service.execute(query, &permissions, max_rows, &session_id, &schema_context).await {
            Ok(result) => {
                println!("[SqlNode] {} rows, truncated: {}", result.row_count, result.truncated);

                if let Some(obs) = &observer {
                    for warning in &result.warnings {
                        obs.on_event(crate::dag_engine::domain::observer::NodeEvent::LlmToken {
                            token: format!("\nSQL Warning: {}", warning),
                        });
                    }
                    for hint in &result.optimization_hints {
                        obs.on_event(crate::dag_engine::domain::observer::NodeEvent::LlmToken {
                            token: format!("\nSQL Optimization: {}", hint),
                        });
                    }
                }

                Ok(result.to_json())
            }
            Err(e) => {
                println!("[SqlNode] Error: {}", e);
                Ok(json!({
                    "error": e.to_string(),
                    "source": match &e {
                        SqlNodeError::Blocked { .. } => "static_validator",
                        SqlNodeError::CriticRejected { .. } => "llm_critic",
                        _ => "execution",
                    }
                }))
            }
        }
    }

    fn schema(&self) -> Value {
        json!({
            "name": "sql_query",
            "description": "Execute PostgreSQL queries with granular permission control and validation.",
            "config": {
                "connection_url": "string (required, supports ${ENV_VAR})",
                "permissions": "object (optional, default: read_only preset)",
                "runtime_limits": "object (optional, max_rows, statement_timeout_ms, work_mem_mb)",
                "guardrail_enabled": "boolean (optional, enables static validation rules)",
                "guardrail_llm": "object (optional, LLM critic config: enabled, provider, model, api_key)"
            },
            "inputs": {
                "query": "string (required, the SQL query to execute)"
            },
            "outputs": {
                "output": "array or object (query results)",
                "row_count": "integer",
                "truncated": "boolean"
            }
        })
    }

    fn description(&self) -> Option<&str> {
        Some("Execute PostgreSQL queries with permission control, static validation, and optional LLM security review.")
    }

    fn default_input(&self) -> Option<&str> {
        Some("query")
    }

    fn default_output(&self) -> Option<&str> {
        Some("output")
    }
}
