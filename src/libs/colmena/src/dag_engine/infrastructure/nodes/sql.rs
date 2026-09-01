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
use crate::dag_engine::domain::sql_ports::{
    FunctionInfo, FunctionRegistryPort, SqlConnectionPort, TableSchema,
};
use crate::dag_engine::infrastructure::sql_function_registry::PgRegistryAdapter;
use crate::dag_engine::infrastructure::sql_llm_critic::LlmCriticAdapter;
use crate::dag_engine::infrastructure::sql_pool_adapter::PgPoolAdapter;
use crate::dag_engine::infrastructure::sql_port_factory::SqlPortFactory;
use crate::dag_engine::infrastructure::sql_static_validator::StaticRuleValidator;
use serde_json::{json, Value};
use std::error::Error as StdError;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::OnceCell;

/// Holds the fully-initialized state of a `SqlNode`.
/// Created exactly once per node instance via `OnceCell::get_or_try_init`.
struct SqlNodeInit {
    adapter: Arc<PgPoolAdapter>,
    description_supplement: String,
}

pub struct SqlNode {
    factory: Arc<SqlPortFactory>,
    /// Populated atomically on the first call that needs initialization.
    /// `OnceCell` prevents the TOCTOU race where two concurrent callers both
    /// observe `initialized == false` and both run the expensive setup work.
    init: OnceCell<SqlNodeInit>,
}

const MAX_SCHEMA_TABLES: usize = 40;
const MAX_SCHEMA_CHARS: usize = 8000;

impl SqlNode {
    pub fn new(factory: Arc<SqlPortFactory>) -> Self {
        Self {
            factory,
            init: OnceCell::new(),
        }
    }

    /// Governance fields that carry authority (credentials, permissions, sandbox
    /// limits). When present in the node's static `config`, they MUST NOT be
    /// overridable by an incoming edge input — otherwise an upstream node (e.g. an
    /// `llm_call`) whose output happens to contain a `permissions` key could
    /// escalate the SQL node's privileges. Everything else is *data* and lets an
    /// input override config (edge-driven data flow).
    const GOVERNANCE_KEYS: &'static [&'static str] = &[
        "connection_url",
        "permissions",
        "runtime_limits",
        "guardrail_llm",
        "setup_sql",
    ];

    /// Build the effective config for `execute`, merging the node's static
    /// `config` (base) with edge `inputs`.
    ///
    /// - **Data fields** (`query`, …): the value from `inputs` overrides `config`.
    /// - **Governance fields** ([`GOVERNANCE_KEYS`]): if `config` sets them, they
    ///   are authoritative and an input cannot override them.
    ///
    /// In the tool path `config` is always `{}`, so every value comes from
    /// `inputs` and behavior is byte-identical to the pre-merge implementation.
    fn build_effective_config(config: &Value, inputs: &NodeInputs) -> Value {
        let mut merged = config.as_object().cloned().unwrap_or_default();
        for (k, v) in inputs {
            let governance_locked =
                Self::GOVERNANCE_KEYS.contains(&k.as_str()) && merged.contains_key(k);
            if governance_locked {
                // config authored this governance field — an input cannot override it.
                continue;
            }
            merged.insert(k.clone(), v.clone());
        }
        Value::Object(merged)
    }

    /// Perform the full initialization and return the result.
    /// Called at most once — subsequent calls return the cached `SqlNodeInit`.
    async fn get_or_init(
        &self,
        config: &Value,
    ) -> Result<&SqlNodeInit, Box<dyn StdError + Send + Sync>> {
        // We need to own config data in the closure; clone the parts we need.
        let config_owned = config.clone();
        self.init
            .get_or_try_init(|| async move {
                Self::do_initialize_inner(&self.factory, &config_owned).await
            })
            .await
    }

    /// Body of the initialization logic — called once by `get_or_init`.
    async fn do_initialize_inner(
        factory: &Arc<SqlPortFactory>,
        config: &Value,
    ) -> Result<SqlNodeInit, Box<dyn StdError + Send + Sync>> {
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

        // Acquire adapter from factory (gets or creates the registry pool)
        let adapter = factory
            .get_adapter(&connection_url, statement_timeout_ms, work_mem_mb)
            .await
            .map_err(|e| format!("Failed to acquire SQL pool: {}", e))?;

        // Operator-driven schema provisioning: ensure every schema listed in
        // `allowed_schemas` exists, creating the missing ones. This is distinct
        // from LLM-issued CREATE SCHEMA (which stays blocked) — the names come
        // from fixed operator config. Hard-fails init if a missing schema can't
        // be created (e.g. the DB role lacks CREATE privilege).
        if permissions.create_schemas_if_missing() {
            let listed: Vec<String> = permissions
                .allowed_schemas_iter()
                .map(str::to_string)
                .collect();
            if !listed.is_empty() {
                let conn: &dyn SqlConnectionPort = adapter.as_ref();
                let missing = conn
                    .missing_schemas(&listed)
                    .await
                    .map_err(|e| format!("Failed to check existing schemas: {}", e))?;
                for schema in &missing {
                    tracing::debug!(
                        target: crate::dag_engine::log_policy::T_SQL,
                        schema = %schema,
                        "allowed_schema missing — creating"
                    );
                    conn.create_schema(schema)
                        .await
                        .map_err(|e| format!("Failed to create schema '{}': {}", schema, e))?;
                }
            }
        }

        // Operator-driven environment bootstrapping: run author-defined setup SQL
        // (DDL + seed). Operator trust-level — bypasses the LLM static validator,
        // same as schema provisioning above. Idempotency is the author's contract
        // (`CREATE ... IF NOT EXISTS`, `INSERT ... ON CONFLICT`). Atomic: any failure
        // rolls the whole block back and hard-fails init. Runs BEFORE metadata
        // introspection so the tool description reflects freshly-created tables.
        if let Some(setup_sql) = config.get("setup_sql").and_then(|v| v.as_str()) {
            let trimmed = setup_sql.trim();
            if !trimmed.is_empty() {
                tracing::debug!(
                    target: crate::dag_engine::log_policy::T_SQL,
                    bytes = trimmed.len(),
                    "running setup_sql"
                );
                let conn: &dyn SqlConnectionPort = adapter.as_ref();
                conn.execute_setup_sql(trimmed)
                    .await
                    .map_err(|e| format!("Failed to run setup_sql: {}", e))?;
            }
        }

        // Create registry adapter sharing the same pool
        let sandbox_schema = permissions.sandbox_schema().to_string();
        let registry = PgRegistryAdapter::new(adapter.pool(), sandbox_schema);

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
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        let tables: Vec<crate::dag_engine::domain::sql_ports::TableSchema> = {
            let conn: &dyn SqlConnectionPort = adapter.as_ref();
            conn.load_table_schemas(&allowed_schemas)
                .await
                .unwrap_or_default()
        };

        let functions: Vec<FunctionInfo> = {
            let reg: &dyn FunctionRegistryPort = &registry;
            reg.list_functions().await.unwrap_or_default()
        };

        let description_supplement =
            Self::build_description_supplement(&tables, &functions, &permissions, max_rows);

        // Auto-RLS setup if enabled
        if permissions.auto_rls() {
            tracing::debug!(
                target: crate::dag_engine::log_policy::T_SQL,
                "auto_rls enabled — setting up RLS policies"
            );
            let tenant_col = permissions.tenant_column();
            for table in &tables {
                if let Err(e) = adapter
                    .setup_rls_for_table(&table.schema_name, &table.table_name, tenant_col)
                    .await
                {
                    tracing::warn!(
                        target: crate::dag_engine::log_policy::T_SQL,
                        schema = %table.schema_name,
                        table = %table.table_name,
                        error = %e,
                        "RLS setup warning"
                    );
                }
            }
        }

        Ok(SqlNodeInit {
            adapter,
            description_supplement,
        })
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
        tables: &[TableSchema],
        functions: &[FunctionInfo],
        permissions: &SqlPermissions,
        max_rows: u64,
    ) -> String {
        let mut lines = Vec::new();

        // 1. Capability statement (always included — small).
        lines.push(permissions.describe_capabilities_nl());

        // 2. Schema render with graceful cap.
        if !tables.is_empty() {
            let rendered = Self::render_schema(tables);
            if tables.len() > MAX_SCHEMA_TABLES || rendered.len() > MAX_SCHEMA_CHARS {
                lines.push(String::new());
                let mut current = String::new();
                for t in tables {
                    if t.schema_name != current {
                        current = t.schema_name.clone();
                        lines.push(format!("Tablas (schema: {}):", current));
                    }
                    // Always use schema-qualified name so LLM writes correct SQL.
                    lines.push(format!("  - {}.{}", t.schema_name, t.table_name));
                }
                lines.push(
                    "(Schema grande: usá introspección sobre information_schema \
                     para ver columnas. IMPORTANTE: usá siempre schema.tabla en el SQL.)"
                        .to_string(),
                );
            } else {
                lines.push(String::new());
                lines.push(
                    "IMPORTANTE: usá siempre el nombre calificado schema.tabla en el SQL \
                     (ej: finanzas.gastos, no solo gastos)."
                        .to_string(),
                );
                lines.push(rendered);
            }
        }

        // 3. Functions (unchanged behavior).
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
        lines.push(format!("Max rows: {}", max_rows));

        // Multi-statement + anti-patterns block (shipped 2026-06-09 with Política C).
        // For deeper guidance load the `sql-query-best-practices` skill if the
        // operator enabled it.
        lines.push(String::new());
        lines.push(
            "Multi-statement queries: separá statements con `;`. Se ejecutan TODOS en \
             una transacción atómica (rollback completo si cualquiera falla). El output \
             devuelto es el resultado del ÚLTIMO statement. SELECTs anteriores se \
             ejecutan pero su resultado se descarta. LIMIT auto se aplica solo al \
             último SELECT."
                .to_string(),
        );
        lines.push(String::new());
        lines.push("NO: BEGIN, COMMIT, ROLLBACK      → automático, no escribirlos".to_string());
        lines.push(
            "NO: $1, ?, :name                  → pegá valores literales, escapá ' con ''"
                .to_string(),
        );
        // The ALTER anti-pattern is conditional: when the preset allows ADD COLUMN
        // we must NOT list plain ALTER as fully blocked — that confuses the LLM.
        if permissions
            .is_allowed(&crate::dag_engine::domain::sql_permissions::SqlOperation::AddColumn)
        {
            lines.push("NO: TRUNCATE, DROP              → siempre bloqueados".to_string());
            lines.push(
                "SÍ: ALTER TABLE <schema>.<tabla> ADD COLUMN <col> <tipo> \
                 → permitido con este preset (solo ADD COLUMN; nada más de ALTER)"
                    .to_string(),
            );
        } else {
            lines.push(
                "NO: TRUNCATE, DROP, ALTER         → bloqueado; usá DELETE con WHERE".to_string(),
            );
        }
        lines.push("NO: CREATE INDEX/VIEW/SCHEMA      → bloqueado; pedile al operator".to_string());
        lines.push("NO: GRANT, REVOKE                  → bloqueado".to_string());
        lines.push("NO: DELETE/UPDATE sin WHERE       → bloqueado".to_string());
        lines.push("NO: CREATE FUNCTION sin COMMENT ON FUNCTION → bloqueado".to_string());
        lines.push(String::new());
        lines.push(
            "Para bulk insert: hasta ~20 rows usá VALUES multi-row inline. Más allá, \
             si los datos vienen de un CSV/Excel adjunto, pedile al operator \
             sql_bulk_insert_from_attachment (cuando esté disponible)."
                .to_string(),
        );

        lines.join("\n")
    }

    /// Render tables with columns + keys for the tool description.
    fn render_schema(tables: &[crate::dag_engine::domain::sql_ports::TableSchema]) -> String {
        let mut out = String::new();
        let mut current = String::new();
        for t in tables {
            if t.schema_name != current {
                current = t.schema_name.clone();
                out.push_str(&format!("\nEsquema disponible (schema: {}):\n", current));
            }
            let pk: Vec<&str> = t
                .columns
                .iter()
                .filter(|c| c.is_pk)
                .map(|c| c.name.as_str())
                .collect();
            let pk_str = if pk.is_empty() {
                String::new()
            } else {
                format!("  [PK: {}]", pk.join(", "))
            };
            // Show schema-qualified name so LLM knows exactly what to write in SQL.
            let qualified = format!("{}.{}", t.schema_name, t.table_name);
            match &t.description {
                Some(d) => out.push_str(&format!("  • {}{}  -- {}\n", qualified, pk_str, d)),
                None => out.push_str(&format!("  • {}{}\n", qualified, pk_str)),
            }
            for c in &t.columns {
                let mut flags: Vec<&str> = Vec::new();
                if c.not_null {
                    flags.push("NOT NULL");
                }
                if c.is_unique {
                    flags.push("UNIQUE");
                }
                let fk = t.foreign_keys.iter().find(|f| f.column == c.name);
                let fk_str = fk
                    .map(|f| format!("  → {}.{}.{} (FK)", f.ref_schema, f.ref_table, f.ref_column))
                    .unwrap_or_default();
                let flag_str = if flags.is_empty() {
                    String::new()
                } else {
                    format!("  {}", flags.join(", "))
                };
                out.push_str(&format!(
                    "      - {} {}{}{}\n",
                    c.name, c.data_type, flag_str, fk_str
                ));
            }
        }
        out
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

#[async_trait::async_trait]
impl InitializableNode for SqlNode {
    async fn initialize(
        &self,
        config: &Value,
    ) -> Result<InitContext, Box<dyn StdError + Send + Sync>> {
        let init = self.get_or_init(config).await?;
        Ok(InitContext {
            description_supplement: Some(init.description_supplement.clone()),
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
        // Merge the node's static `config` with edge `inputs`. Data fields let an
        // input override config; governance fields (permissions, connection_url,
        // …) authored in `config` are authoritative and cannot be overridden by an
        // input. In the tool path `config` is `{}`, so every value comes from
        // `inputs` (tool-call behavior is unchanged). See `build_effective_config`.
        let effective_config = Self::build_effective_config(config, inputs);

        let query = effective_config
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or("sql_query node requires 'query' input")?;

        // Lazy initialization: connect on first call if not already initialized.
        // OnceCell ensures concurrent callers wait on the same future — no TOCTOU race.
        if self.init.get().is_none() {
            tracing::debug!(
                target: crate::dag_engine::log_policy::T_SQL,
                "first call — initializing connection pool"
            );
        }
        let init = self
            .get_or_init(&effective_config)
            .await
            .map_err(|e| format!("SqlNode initialization failed: {}", e))?;

        let permissions = SqlPermissions::from_config(effective_config.get("permissions"))
            .map_err(|e| format!("Invalid permissions: {}", e))?;

        // Resolve tenant_user_id (may contain ${ENV_VAR})
        let tenant_user_id: Option<String> = permissions.tenant_user_id().map(|raw| {
            Self::resolve_env_vars(raw).unwrap_or_else(|e| {
                tracing::warn!(
                    target: crate::dag_engine::log_policy::T_SQL,
                    error = %e,
                    "failed to resolve tenant_user_id"
                );
                raw.to_string()
            })
        });

        let runtime_limits = effective_config.get("runtime_limits");
        let max_rows = runtime_limits
            .and_then(|r| r.get("max_rows"))
            .and_then(|v| v.as_u64())
            .unwrap_or(100);

        // Static validation is unconditional by design: it is what blocks DROP,
        // TRUNCATE and unqualified DELETE/UPDATE, so there is deliberately no
        // switch to turn it off. The node's schema used to advertise a
        // `guardrail_enabled` flag that nothing ever read — operators set it
        // believing it controlled this, and it silently did nothing. The flag is
        // gone; do not reintroduce one. `guardrail_llm` below is the only
        // guardrail that is actually optional.
        let validator = Arc::new(StaticRuleValidator)
            as Arc<dyn crate::dag_engine::domain::sql_ports::SqlValidatorPort>;

        let critic_enabled = effective_config
            .get("guardrail_llm")
            .and_then(|g| g.get("enabled"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let critic: Option<Arc<dyn crate::dag_engine::domain::sql_ports::SqlCriticPort>> =
            if critic_enabled {
                let guardrail_cfg = effective_config.get("guardrail_llm").unwrap();
                let provider = guardrail_cfg
                    .get("provider")
                    .and_then(|v| v.as_str())
                    .unwrap_or("openai")
                    .to_string();
                let model = guardrail_cfg
                    .get("model")
                    .and_then(|v| v.as_str())
                    .unwrap_or("gpt-4o-mini")
                    .to_string();
                let api_key_raw = guardrail_cfg
                    .get("api_key")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                // Fail-fast at node initialization: a missing or empty
                // `api_key` here used to silently produce an `LlmCriticAdapter`
                // wrapping an empty string, which then exploded on the first
                // guardrailed query as `InvalidApiKey`. Surface the
                // misconfiguration at engine startup instead so it appears
                // before any traffic is served.
                let api_key = Self::resolve_env_vars(api_key_raw).map_err(|e| {
                    format!(
                        "sql node: guardrail_llm.api_key failed to resolve \
                         (`{api_key_raw}`): {e}. Set guardrail_llm.api_key to a \
                         non-empty literal or to an env-var placeholder like \
                         ${{OPENAI_API_KEY}} that is set at startup."
                    )
                })?;
                if api_key.is_empty() {
                    return Err(format!(
                        "sql node: guardrail_llm.enabled = true but api_key \
                         resolved to an empty string (raw value: `{api_key_raw}`). \
                         Either disable guardrail_llm or provide a real key."
                    )
                    .into());
                }

                Some(Arc::new(LlmCriticAdapter::new(provider, model, api_key))
                    as Arc<
                        dyn crate::dag_engine::domain::sql_ports::SqlCriticPort,
                    >)
            } else {
                None
            };

        let sandbox_schema = permissions.sandbox_schema().to_string();
        let adapter = init.adapter.clone();
        let registry = Arc::new(PgRegistryAdapter::new(adapter.pool(), sandbox_schema))
            as Arc<dyn crate::dag_engine::domain::sql_ports::FunctionRegistryPort>;

        let service = SqlExecutionService::new(
            adapter.clone() as Arc<dyn crate::dag_engine::domain::sql_ports::SqlConnectionPort>,
            validator,
            critic,
            registry,
        );

        let session_id = Self::new_session_id();
        let schema_context = init.description_supplement.clone();

        // Structured event: safe metadata only — never the raw query. See
        // docs/developer_guide/50_logging_and_observability.md for the
        // target namespace contract. The raw query goes through
        // `payload_trace!`, double-gated (EnvFilter directive AND
        // `COLMENA_LOG_PAYLOADS`) — full text, not truncated, since a
        // truncated payload only defeats debugging behind two gates.
        tracing::debug!(
            target: crate::dag_engine::log_policy::T_SQL,
            query_len = query.len(),
            session_id = %session_id,
            max_rows,
            "executing sql query"
        );
        crate::dag_engine::log_policy::payload_trace!(sql_query, query = %query);

        match service
            .execute(
                query,
                &permissions,
                max_rows,
                &session_id,
                &schema_context,
                tenant_user_id.as_deref(),
            )
            .await
        {
            Ok(result) => {
                tracing::debug!(
                    target: crate::dag_engine::log_policy::T_SQL,
                    row_count = result.row_count,
                    truncated = result.truncated,
                    "sql query executed"
                );

                // Post-CREATE TABLE: apply RLS to any new tables in the query
                if permissions.auto_rls() {
                    if let Ok(stmts) = crate::dag_engine::infrastructure::sql_ast::parse(query) {
                        for stmt in &stmts {
                            if let Some((schema, table)) =
                                crate::dag_engine::infrastructure::sql_ast::created_table_name(stmt)
                            {
                                tracing::debug!(
                                    target: crate::dag_engine::log_policy::T_SQL,
                                    schema = %schema,
                                    table = %table,
                                    "CREATE TABLE detected — applying RLS"
                                );
                                if let Err(e) = adapter
                                    .setup_rls_for_new_table(
                                        &schema,
                                        &table,
                                        permissions.tenant_column(),
                                    )
                                    .await
                                {
                                    tracing::warn!(
                                        target: crate::dag_engine::log_policy::T_SQL,
                                        schema = %schema,
                                        table = %table,
                                        error = %e,
                                        "RLS setup warning for new table"
                                    );
                                }
                            }
                        }
                    }
                }

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
                // Stays visible at the default production posture (`warn!`
                // on `colmena::sql`, not gated behind `payload_trace!`) —
                // a genuine error condition, not payload. The error string
                // can still echo fragments of the failed query if the
                // driver includes them; redacting that is a separate,
                // filed follow-up (see docs/developer_guide/50_logging_and_observability.md).
                let source = match &e {
                    SqlNodeError::Blocked { .. } => "static_validator",
                    SqlNodeError::CriticRejected { .. } => "llm_critic",
                    _ => "execution",
                };
                tracing::warn!(
                    target: crate::dag_engine::log_policy::T_SQL,
                    error = %e,
                    source,
                    "sql execution failed"
                );
                Ok(json!({
                    "error": e.to_string(),
                    "source": source
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

    /// Expose self as [`InitializableNode`] so the tool executor can call
    /// `initialize()` to fetch the DB schema + capability statement and
    /// inject them into the tool description seen by the LLM.
    fn as_initializable(
        &self,
    ) -> Option<&dyn crate::dag_engine::domain::initializable_node::InitializableNode> {
        Some(self)
    }
}

#[cfg(test)]
mod effective_config_tests {
    use super::*;
    use std::collections::HashMap;

    fn inputs(pairs: &[(&str, Value)]) -> NodeInputs {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    #[test]
    fn top_level_reads_query_and_governance_from_config() {
        // Top-level node: everything authored in `config`, no incoming edges.
        let config = json!({
            "connection_url": "postgres://x",
            "query": "INSERT INTO t VALUES (1)",
            "permissions": { "preset": "read_write" }
        });
        let eff = SqlNode::build_effective_config(&config, &HashMap::new());
        assert_eq!(
            eff.get("query").and_then(|v| v.as_str()),
            Some("INSERT INTO t VALUES (1)")
        );
        assert_eq!(
            eff.get("connection_url").and_then(|v| v.as_str()),
            Some("postgres://x")
        );
        assert_eq!(
            eff.pointer("/permissions/preset").and_then(|v| v.as_str()),
            Some("read_write")
        );
    }

    #[test]
    fn tool_path_empty_config_takes_all_from_inputs() {
        // Tool path: `config` is `{}`; fixed node_schema values arrive via inputs.
        let ins = inputs(&[
            ("query", json!("SELECT 1")),
            ("permissions", json!({ "preset": "read_only" })),
        ]);
        let eff = SqlNode::build_effective_config(&json!({}), &ins);
        assert_eq!(eff.get("query").and_then(|v| v.as_str()), Some("SELECT 1"));
        assert_eq!(
            eff.pointer("/permissions/preset").and_then(|v| v.as_str()),
            Some("read_only")
        );
    }

    #[test]
    fn data_field_query_is_overridable_by_input() {
        let config = json!({ "query": "SELECT 1", "permissions": { "preset": "read_write" } });
        let ins = inputs(&[("query", json!("SELECT 2"))]);
        let eff = SqlNode::build_effective_config(&config, &ins);
        assert_eq!(eff.get("query").and_then(|v| v.as_str()), Some("SELECT 2"));
    }

    #[test]
    fn governance_permissions_from_config_not_overridable_by_input() {
        // Security: an upstream node emitting `permissions` must NOT escalate.
        let config = json!({ "permissions": { "preset": "read_write" } });
        let ins = inputs(&[("permissions", json!({ "preset": "read_write_delete" }))]);
        let eff = SqlNode::build_effective_config(&config, &ins);
        assert_eq!(
            eff.pointer("/permissions/preset").and_then(|v| v.as_str()),
            Some("read_write"),
            "config governance must win over an incoming input"
        );
    }
}

#[cfg(test)]
mod supplement_tests {
    use super::*;
    use crate::dag_engine::domain::sql_permissions::SqlPermissions;
    use crate::dag_engine::domain::sql_ports::{ColumnInfo, ForeignKey, TableSchema};

    fn t() -> Vec<TableSchema> {
        vec![TableSchema {
            schema_name: "finanzas".into(),
            table_name: "gastos".into(),
            description: None,
            columns: vec![
                ColumnInfo {
                    name: "id".into(),
                    data_type: "integer".into(),
                    not_null: true,
                    is_pk: true,
                    is_unique: false,
                },
                ColumnInfo {
                    name: "categoria_id".into(),
                    data_type: "integer".into(),
                    not_null: false,
                    is_pk: false,
                    is_unique: false,
                },
            ],
            foreign_keys: vec![ForeignKey {
                column: "categoria_id".into(),
                ref_schema: "finanzas".into(),
                ref_table: "categorias".into(),
                ref_column: "id".into(),
            }],
        }]
    }

    #[test]
    fn supplement_includes_columns_pk_and_fk() {
        let perms = SqlPermissions::from_config(Some(&serde_json::json!({
            "preset": "read_write_delete",
            "allowed_schemas": ["finanzas"]
        })))
        .unwrap();
        let s = SqlNode::build_description_supplement(&t(), &[], &perms, 100);
        assert!(s.contains("PK: id"));
        assert!(s.contains("categoria_id"));
        assert!(s.contains("→ finanzas.categorias.id"));
        assert!(s.to_lowercase().contains("agregar columnas"));
    }
}

#[cfg(test)]
mod setup_sql_tests {
    use super::*;
    use crate::dag_engine::infrastructure::pool_registry::{PgPoolRegistry, PoolConfig};
    use crate::dag_engine::infrastructure::sql_port_factory::SqlPortFactory;
    use serde_json::json;

    fn unique(prefix: &str) -> String {
        format!(
            "{}_{}",
            prefix,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        )
    }

    fn fresh_node() -> SqlNode {
        let registry = Arc::new(PgPoolRegistry::new(PoolConfig::defaults()));
        let factory = Arc::new(SqlPortFactory::new(registry));
        SqlNode::new(factory)
    }

    async fn raw_pool() -> sqlx::PgPool {
        let url = std::env::var("TEST_DATABASE_URL").unwrap();
        sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect(&url)
            .await
            .unwrap()
    }

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL — run with `cargo test -- --ignored`"]
    async fn setup_sql_runs_at_init_and_table_is_introspected() {
        if std::env::var("TEST_DATABASE_URL").is_err() {
            eprintln!("skip: TEST_DATABASE_URL not set");
            return;
        }
        let schema = unique("colmena_node_setup");
        let setup = format!(
            "CREATE SCHEMA IF NOT EXISTS {s};\n\
             CREATE TABLE IF NOT EXISTS {s}.gastos (id SERIAL PRIMARY KEY, nombre TEXT UNIQUE NOT NULL);\n\
             INSERT INTO {s}.gastos (nombre) VALUES ('a'),('b') ON CONFLICT (nombre) DO NOTHING;",
            s = schema
        );
        // `${TEST_DATABASE_URL}` is resolved by SqlNode::resolve_env_vars.
        let config = json!({
            "connection_url": "${TEST_DATABASE_URL}",
            "permissions": { "preset": "read_write", "allowed_schemas": [schema] },
            "setup_sql": setup,
        });

        // Init runs setup_sql, then introspects — so the table shows in the supplement.
        let ctx = fresh_node()
            .initialize(&config)
            .await
            .expect("init with setup_sql");
        let supplement = ctx.description_supplement.unwrap_or_default();
        assert!(
            supplement.contains("gastos"),
            "description supplement should list the setup-created table:\n{}",
            supplement
        );

        // A second, independent node re-runs setup: seed must stay at 2 rows (idempotent).
        fresh_node()
            .initialize(&config)
            .await
            .expect("second init is a no-op");

        let pool = raw_pool().await;
        let count: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {}.gastos", schema))
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 2, "seed must be idempotent across inits");

        sqlx::query(&format!("DROP SCHEMA {} CASCADE", schema))
            .execute(&pool)
            .await
            .ok();
        pool.close().await;
    }

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL — run with `cargo test -- --ignored`"]
    async fn bad_setup_sql_hard_fails_init_and_rolls_back() {
        if std::env::var("TEST_DATABASE_URL").is_err() {
            eprintln!("skip: TEST_DATABASE_URL not set");
            return;
        }
        let schema = unique("colmena_node_setup_fail");
        // allowed_schemas is empty so schema provisioning does NOT pre-create the schema;
        // setup_sql creates it and then fails, so rollback must leave it absent.
        let config = json!({
            "connection_url": "${TEST_DATABASE_URL}",
            "permissions": { "preset": "read_only", "allowed_schemas": [] },
            "setup_sql": format!("CREATE SCHEMA IF NOT EXISTS {s};\nTHIS IS NOT VALID SQL;", s = schema),
        });

        let res = fresh_node().initialize(&config).await;
        assert!(res.is_err(), "bad setup_sql must hard-fail init");
        let err = format!("{}", res.err().unwrap());
        assert!(
            err.contains("setup_sql"),
            "error must mention setup_sql, got: {}",
            err
        );

        let pool = raw_pool().await;
        let exists: Option<String> = sqlx::query_scalar(
            "SELECT schema_name FROM information_schema.schemata WHERE schema_name = $1",
        )
        .bind(&schema)
        .fetch_optional(&pool)
        .await
        .unwrap();
        assert!(
            exists.is_none(),
            "failed setup_sql must roll back the schema"
        );
        pool.close().await;
    }

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL — run with `cargo test -- --ignored`"]
    async fn empty_or_absent_setup_sql_is_a_noop() {
        if std::env::var("TEST_DATABASE_URL").is_err() {
            eprintln!("skip: TEST_DATABASE_URL not set");
            return;
        }
        let schema = unique("colmena_empty_setup");

        // Whitespace-only setup_sql → trimmed empty → must be skipped; init succeeds.
        let whitespace_cfg = json!({
            "connection_url": "${TEST_DATABASE_URL}",
            "permissions": { "preset": "read_only", "allowed_schemas": [schema] },
            "setup_sql": "   \n  \t "
        });
        fresh_node()
            .initialize(&whitespace_cfg)
            .await
            .expect("init with whitespace-only setup_sql must succeed (no-op)");

        // Absent setup_sql → must also be a no-op; init succeeds.
        let absent_cfg = json!({
            "connection_url": "${TEST_DATABASE_URL}",
            "permissions": { "preset": "read_only", "allowed_schemas": [schema] }
        });
        fresh_node()
            .initialize(&absent_cfg)
            .await
            .expect("init without setup_sql must succeed (no-op)");

        // Proof of no-op: the schema is auto-created (create_schemas_if_missing) but
        // contains ZERO tables, because no setup_sql DDL ran.
        let pool = raw_pool().await;
        let table_count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM information_schema.tables WHERE table_schema = $1",
        )
        .bind(&schema)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            table_count, 0,
            "empty/absent setup_sql must not create any tables"
        );

        sqlx::query(&format!("DROP SCHEMA IF EXISTS {} CASCADE", schema))
            .execute(&pool)
            .await
            .ok();
        pool.close().await;
    }
}
