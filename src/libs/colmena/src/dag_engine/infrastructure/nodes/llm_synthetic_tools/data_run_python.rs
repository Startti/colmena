//! LLM tool `data_run_python` — the unified cross-store sandbox tool.
//! Binds each tabular source ([`DataBinding`], from `tabular_bindings`)
//! where the data lives NOW (attachment, Google Sheets, SQL `SELECT`,
//! or inline JSON) and runs sandboxed Python over all of them at once.
//! Rows never enter the LLM's context — only the script's `output`
//! (plus any `output_tables` / `output_sheets` / `output_attachments`
//! sinks defined by later tasks) crosses the wire.
//!
//! This module owns the args struct, capability gating types, and the
//! dynamic-description tool builder only. Dispatch/execution lives in
//! later tasks (see `docs/superpowers/specs/2026-07-01-data-run-python-design.md`).

use std::collections::HashMap;

use crate::dag_engine::domain::sql_permissions::SqlPermissions;
use crate::llm::domain::tools::ToolDefinition;
use crate::text;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

use super::sql_bulk_tools::resolve_env_vars;
use super::tabular_bindings::{deserialize_bindings_flexible, DataBinding};

pub const TOOL_DATA_RUN_PYTHON: &str = "data_run_python";

const DATA_PY_PRELUDE: &str =
    include_str!("../../../../../text/prompts/python_sandbox/data_run_python_prelude.md");
const DATA_PY_POSTLUDE: &str =
    include_str!("../../../../../text/prompts/python_sandbox/data_run_python_postlude.md");

/// Wrap user-supplied Python with the colmena prelude/postlude so the
/// sandboxed script has `pd`/`np`/`stats` in scope and, on exit, packages
/// `output`/`output_tables`/`output_sheets`/`output_attachments` into a
/// single `output` dict the dispatcher parses.
///
/// `#[allow(dead_code)]` until Task 14 wires the dispatcher that calls
/// this from `execute`.
#[allow(dead_code)]
fn wrap_user_code(user_code: &str) -> String {
    format!("{DATA_PY_PRELUDE}{user_code}{DATA_PY_POSTLUDE}")
}

// ── Args ─────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DataRunPythonArgs {
    /// Tabular bindings to load. Each becomes a Python global (a list of
    /// `{col: val}` dicts) under the name given by `var`. At least one
    /// entry required. Bindings are fetched IN PARALLEL, each from its
    /// own source (attachment, Google Sheets, SQL `SELECT`, or inline
    /// JSON data).
    #[serde(deserialize_with = "deserialize_bindings_flexible")]
    pub bindings: Vec<DataBinding>,

    /// Python code. Has access to `pandas as pd`, `numpy as np`,
    /// `scipy.stats as stats`, plus each binding's records list bound
    /// under its `var` name. Define `output` (any JSON-serializable
    /// value) — that is what the LLM sees.
    pub code: String,

    /// Optional target spreadsheet for `output_sheets` returned by the
    /// script (Google Sheets sink). Only meaningful when the `gsheets`
    /// capability is enabled.
    #[serde(default)]
    pub write_to_spreadsheet: Option<String>,
}

// ── Capability gating ────────────────────────────────────────────────

/// Which data-store capabilities the operator has enabled for this tool
/// instance. `attachment` and `inline` sources are always available and
/// have no corresponding flag here.
#[derive(Debug, Clone, Copy, Default)]
pub struct EnabledSources {
    pub sql: bool,
    pub gsheets: bool,
}

/// Derive which sources are available for this tool instance from the
/// operator's `fixed_config` plus whatever the agent already exposes.
/// `sql` is enabled purely by the presence of a `sql` block in
/// `fixed_config` (validity is checked separately by
/// [`parse_sql_config`]); `gsheets` is enabled either because the agent
/// already has the gsheets toolkit active, or via an explicit
/// `enable_gsheets: true` flag in `fixed_config`.
pub fn enabled_sources(fixed: &HashMap<String, Value>, agent_has_gsheets: bool) -> EnabledSources {
    let sql = fixed.contains_key("sql");
    let gsheets = agent_has_gsheets || fixed.get("enable_gsheets") == Some(&Value::Bool(true));
    EnabledSources { sql, gsheets }
}

// ── SQL sink configuration ──────────────────────────────────────────

/// Parsed & validated `sql` block from a `data_run_python` tool's
/// `fixed_config`. Governs both the SQL `SELECT` binding source and the
/// `output_tables` write sink.
#[derive(Debug, Clone)]
pub struct SqlSinkConfig {
    pub connection_url: String,
    pub permissions: SqlPermissions,
    pub allowed_schemas: Vec<String>,
    pub statement_timeout_ms: u64,
    pub work_mem_mb: u64,
    pub on_missing_table: String,
    pub on_existing_table: String,
    pub tenant_user_id: Option<String>,
}

/// Parse the optional `sql` block out of a tool's `fixed_config`.
///
/// Returns `Ok(None)` when no `sql` block is present (SQL capability is
/// simply off). Returns `Err` when a `sql` block is present but invalid
/// (missing `connection_url`, or missing/empty
/// `permissions.allowed_schemas`) — a misconfigured `sql` block must
/// never silently degrade to "SQL disabled".
pub fn parse_sql_config(fixed: &HashMap<String, Value>) -> Result<Option<SqlSinkConfig>, String> {
    let sql = match fixed.get("sql") {
        Some(v) => v,
        None => return Ok(None),
    };

    let raw_connection_url = sql
        .get("connection_url")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            "tool fixed_config.sql is missing 'connection_url'. Operator must set \
         tool_configurations.<tool>.fixed_config.sql.connection_url to a Postgres \
         connection string."
                .to_string()
        })?;
    let connection_url = resolve_env_vars(raw_connection_url)
        .map_err(|e| format!("sql.connection_url env resolution failed: {e}"))?;

    let perms_value = sql.get("permissions");
    let allowed_schemas: Vec<String> = perms_value
        .and_then(|p| p.get("allowed_schemas"))
        .and_then(|s| s.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    if allowed_schemas.is_empty() {
        return Err(
            "tool fixed_config.sql.permissions.allowed_schemas must be non-empty.".to_string(),
        );
    }
    // Resolve `${ENV}` in `permissions.tenant_user_id` BEFORE building
    // `SqlPermissions`, so the embedded permissions object and the
    // top-level `SqlSinkConfig.tenant_user_id` field can never diverge —
    // see `tenant_user_id_env_is_resolved_in_both_places` regression test.
    let resolved_perms_value = match perms_value {
        Some(Value::Object(map))
            if map.get("tenant_user_id").and_then(|v| v.as_str()).is_some() =>
        {
            let raw = map.get("tenant_user_id").and_then(|v| v.as_str()).unwrap();
            let resolved = resolve_env_vars(raw).map_err(|e| {
                format!("sql.permissions.tenant_user_id env resolution failed: {e}")
            })?;
            let mut patched = map.clone();
            patched.insert("tenant_user_id".to_string(), Value::String(resolved));
            Some(Value::Object(patched))
        }
        Some(other) => Some(other.clone()),
        None => None,
    };
    let permissions = SqlPermissions::from_config(resolved_perms_value.as_ref())?;

    let runtime_limits = sql.get("runtime_limits");
    let statement_timeout_ms = runtime_limits
        .and_then(|r| r.get("statement_timeout_ms"))
        .and_then(|v| v.as_u64())
        .unwrap_or(30_000);
    let work_mem_mb = runtime_limits
        .and_then(|r| r.get("work_mem_mb"))
        .and_then(|v| v.as_u64())
        .unwrap_or(64);

    let on_missing_table = sql
        .get("on_missing_table")
        .and_then(|v| v.as_str())
        .unwrap_or("create")
        .to_string();
    let on_existing_table = sql
        .get("on_existing_table")
        .and_then(|v| v.as_str())
        .unwrap_or("fail")
        .to_string();

    // Sourced from the already-resolved `permissions` — never re-derived
    // from raw JSON, so this field and `permissions.tenant_user_id()` can
    // never disagree.
    let tenant_user_id = permissions.tenant_user_id().map(|s| s.to_string());

    Ok(Some(SqlSinkConfig {
        connection_url,
        permissions,
        allowed_schemas,
        statement_timeout_ms,
        work_mem_mb,
        on_missing_table,
        on_existing_table,
        tenant_user_id,
    }))
}

// ── Tool builder ─────────────────────────────────────────────────────

/// Build the [`ToolDefinition`] for `data_run_python`, appending a
/// dynamically-assembled "Available sources" section to the static base
/// description so the model only ever sees sources the operator actually
/// enabled.
pub fn tool_data_run_python(enabled: &EnabledSources) -> ToolDefinition {
    let mut description = String::from(text::tool_description(TOOL_DATA_RUN_PYTHON));

    description.push_str("\n\nAvailable sources:\n- Attachment (CSV/XLSX from the conversation catalog)\n- Inline JSON data");
    if enabled.gsheets {
        description.push_str("\n- Google Sheets");
    }
    if enabled.sql {
        description.push_str("\n- SQL database tables");
    }

    super::build_synthetic_tool_with_summary::<DataRunPythonArgs>(
        TOOL_DATA_RUN_PYTHON,
        &description,
        text::tool_summary(TOOL_DATA_RUN_PYTHON),
    )
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_definition_lists_only_enabled_sources() {
        let def = tool_data_run_python(&EnabledSources {
            sql: true,
            gsheets: false,
        });
        let desc = def.description.to_lowercase();
        assert!(desc.contains("sql") || desc.contains("database"));
        assert!(!desc.contains("google sheet"));
    }

    #[test]
    fn args_parse_minimal() {
        let a: DataRunPythonArgs = serde_json::from_value(serde_json::json!({
            "bindings":[{"var":"x","query":"SELECT 1"}], "code":"output=1"
        }))
        .unwrap();
        assert_eq!(a.bindings.len(), 1);
    }

    #[test]
    fn wrapped_code_packages_all_sinks() {
        let w = wrap_user_code("output = 1");
        assert!(w.contains("output_tables"));
        assert!(w.contains("output_sheets"));
        assert!(w.contains("output_attachments"));
        assert!(w.contains("user_output"));
    }

    #[test]
    fn prelude_imports_pandas() {
        let w = wrap_user_code("output = 1");
        assert!(w.contains("import pandas"));
    }

    // ── parse_sql_config ────────────────────────────────────────────

    #[test]
    fn sql_config_absent_is_none() {
        let f = std::collections::HashMap::new();
        assert!(parse_sql_config(&f).unwrap().is_none());
    }

    #[test]
    fn sql_config_missing_allowed_schemas_errors() {
        let mut f = std::collections::HashMap::new();
        f.insert(
            "sql".into(),
            serde_json::json!({"connection_url":"postgres://x"}),
        );
        assert!(parse_sql_config(&f).is_err());
    }

    #[test]
    fn sql_config_missing_connection_url_errors() {
        let mut f = std::collections::HashMap::new();
        f.insert(
            "sql".into(),
            serde_json::json!({
                "permissions": { "preset": "read_only", "allowed_schemas": ["public"] }
            }),
        );
        assert!(parse_sql_config(&f).is_err());
    }

    #[test]
    fn sql_config_valid_full_block_parses_with_defaults() {
        let mut f = std::collections::HashMap::new();
        f.insert(
            "sql".into(),
            serde_json::json!({
                "connection_url": "postgres://localhost/db",
                "permissions": {
                    "preset": "read_write_delete",
                    "allowed_schemas": ["analytics", "public"]
                }
            }),
        );
        let cfg = parse_sql_config(&f).unwrap().unwrap();
        assert_eq!(cfg.connection_url, "postgres://localhost/db");
        let mut schemas = cfg.allowed_schemas.clone();
        schemas.sort();
        assert_eq!(schemas, vec!["analytics".to_string(), "public".to_string()]);
        assert_eq!(cfg.on_missing_table, "create");
        assert_eq!(cfg.on_existing_table, "fail");
        assert_eq!(cfg.statement_timeout_ms, 30000);
        assert_eq!(cfg.work_mem_mb, 64);
        assert!(cfg.tenant_user_id.is_none());
    }

    #[test]
    fn sql_config_respects_runtime_limits_and_table_policies_and_tenant() {
        let mut f = std::collections::HashMap::new();
        f.insert(
            "sql".into(),
            serde_json::json!({
                "connection_url": "postgres://localhost/db",
                "permissions": {
                    "preset": "read_write_delete",
                    "allowed_schemas": ["public"],
                    "tenant_user_id": "user-123"
                },
                "runtime_limits": {
                    "statement_timeout_ms": 5000,
                    "work_mem_mb": 128
                },
                "on_missing_table": "fail",
                "on_existing_table": "append"
            }),
        );
        let cfg = parse_sql_config(&f).unwrap().unwrap();
        assert_eq!(cfg.statement_timeout_ms, 5000);
        assert_eq!(cfg.work_mem_mb, 128);
        assert_eq!(cfg.on_missing_table, "fail");
        assert_eq!(cfg.on_existing_table, "append");
        assert_eq!(cfg.tenant_user_id, Some("user-123".to_string()));
    }

    #[test]
    fn sql_config_resolves_env_var_in_connection_url() {
        std::env::set_var("DATA_RUN_PYTHON_TEST_DB_URL", "postgres://resolved/db");
        let mut f = std::collections::HashMap::new();
        f.insert(
            "sql".into(),
            serde_json::json!({
                "connection_url": "${DATA_RUN_PYTHON_TEST_DB_URL}",
                "permissions": { "preset": "read_only", "allowed_schemas": ["public"] }
            }),
        );
        let cfg = parse_sql_config(&f).unwrap().unwrap();
        assert_eq!(cfg.connection_url, "postgres://resolved/db");
        std::env::remove_var("DATA_RUN_PYTHON_TEST_DB_URL");
    }

    #[test]
    fn tenant_user_id_env_is_resolved_in_both_places() {
        std::env::set_var("DRP_TEST_TENANT", "tenant-42");
        let mut f = std::collections::HashMap::new();
        f.insert(
            "sql".into(),
            serde_json::json!({
                "connection_url": "postgres://x",
                "permissions": { "preset": "read_write", "allowed_schemas": ["a"], "tenant_user_id": "${DRP_TEST_TENANT}" }
            }),
        );
        let cfg = parse_sql_config(&f).unwrap().unwrap();
        // top-level field resolved
        assert_eq!(cfg.tenant_user_id.as_deref(), Some("tenant-42"));
        // embedded permissions accessor ALSO resolved (the bug: was "${DRP_TEST_TENANT}")
        assert_eq!(cfg.permissions.tenant_user_id(), Some("tenant-42"));
        std::env::remove_var("DRP_TEST_TENANT");
    }

    // ── enabled_sources ─────────────────────────────────────────────

    #[test]
    fn enabled_sources_sql_true_when_sql_block_present() {
        let mut f = std::collections::HashMap::new();
        f.insert(
            "sql".into(),
            serde_json::json!({
                "connection_url": "postgres://x",
                "permissions": { "allowed_schemas": ["public"] }
            }),
        );
        let enabled = enabled_sources(&f, false);
        assert!(enabled.sql);
        assert!(!enabled.gsheets);
    }

    #[test]
    fn enabled_sources_gsheets_true_when_agent_has_gsheets() {
        let f = std::collections::HashMap::new();
        let enabled = enabled_sources(&f, true);
        assert!(!enabled.sql);
        assert!(enabled.gsheets);
    }

    #[test]
    fn enabled_sources_gsheets_true_when_enable_gsheets_flag_set() {
        let mut f = std::collections::HashMap::new();
        f.insert("enable_gsheets".into(), serde_json::json!(true));
        let enabled = enabled_sources(&f, false);
        assert!(!enabled.sql);
        assert!(enabled.gsheets);
    }
}
