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
use std::sync::Arc;

use crate::dag_engine::domain::sql_permissions::SqlPermissions;
use crate::dag_engine::domain::sql_ports::SqlConnectionPort;
use crate::gsheets::domain::{SheetsClient, SpreadsheetId};
use crate::llm::domain::tools::ToolDefinition;
use crate::text;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

use super::attachment_writer::{write_output_attachments, AttachmentRegistrar};
use super::sheet_collision::{parse_policy, CollisionPolicy};
use super::sheet_writer::write_output_sheets;
use super::sql_bulk_tools::resolve_env_vars;
use super::table_writer::{parse_output_tables, write_output_tables};
use super::tabular_bindings::{
    deserialize_bindings_flexible, resolve_bindings, validate_bindings, AttachmentFetcher,
    DataBinding, SqlBindingCtx,
};

pub const TOOL_DATA_RUN_PYTHON: &str = "data_run_python";

const DATA_PY_PRELUDE: &str =
    include_str!("../../../../../text/prompts/python_sandbox/data_run_python_prelude.md");
const DATA_PY_POSTLUDE: &str =
    include_str!("../../../../../text/prompts/python_sandbox/data_run_python_postlude.md");

/// Wrap user-supplied Python with the colmena prelude/postlude so the
/// sandboxed script has `pd`/`np`/`stats` in scope and, on exit, packages
/// `output`/`output_tables`/`output_sheets`/`output_attachments` into a
/// single `output` dict the dispatcher parses.
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

    /// Python code, inline. Has access to `pandas as pd`, `numpy as np`,
    /// `scipy.stats as stats`, plus each binding's records list bound
    /// under its `var` name. Define `output` (any JSON-serializable
    /// value) — that is what the LLM sees. Exactly one of `code` or
    /// `code_ref` must be set.
    #[serde(default)]
    pub code: Option<String>,

    /// Reference key for stored code, resolved server-side via the
    /// operator-only `fixed_config.code_source_query` SQL binding instead
    /// of pasting code inline. Exactly one of `code` or `code_ref` must be
    /// set. Requires the operator to have configured `code_source_query`
    /// (and a `sql` capability) on this tool instance.
    #[serde(default)]
    pub code_ref: Option<String>,

    /// Optional target spreadsheet for `output_sheets` returned by the
    /// script (Google Sheets sink). Only meaningful when the `gsheets`
    /// capability is enabled.
    #[serde(default)]
    pub write_to_spreadsheet: Option<String>,

    /// Operator-set collision policy for `output_sheets` (`fail` |
    /// `auto_suffix` | `overwrite`). Sourced from
    /// `fixed_config.on_existing_sheet` — the executor wrapper injects it
    /// into the args before [`dispatch_core`] runs. Mirrors
    /// `gsheets_run_python`'s field of the same name.
    #[serde(default, alias = "on_existing_sheet")]
    pub on_existing_sheet: Option<String>,
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

// ── code_ref resolution ──────────────────────────────────────────────

/// Placeholder token substituted with the LLM-supplied `code_ref` key
/// before running `fixed_config.code_source_query`. Distinct from the
/// `${ENV}` / `$DYNAMIC` placeholder syntax used elsewhere so the two
/// substitution passes can never collide.
const CODE_REF_PLACEHOLDER: &str = "{{code_ref}}";

/// The synthetic binding var used to resolve `code_ref` through the
/// existing single-SELECT-binding path (`resolve_bindings`).
const CODE_REF_BINDING_VAR: &str = "__code_source";

/// Allowlist gate for the LLM-supplied `code_ref` key, applied BEFORE it is
/// substituted into `fixed_config.code_source_query`. This neutralizes SQL
/// injection through the substituted key; the resulting query still passes
/// through the existing SELECT-only gate + static validator in
/// `resolve_bindings`.
fn is_valid_code_ref_key(key: &str) -> bool {
    !key.is_empty()
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ':' | '/' | '-'))
}

/// Resolve a `code_ref` key to its stored code string via
/// `fixed_config.code_source_query`, reusing the single-SELECT-binding
/// fetch path (`resolve_bindings`) for the SELECT-only gate, static
/// validator, and adapter round-trip. Requires the result to be exactly
/// one row, one column, non-empty/non-whitespace text.
///
/// Returns `(resolved_code, sha256_hex)` on success.
async fn resolve_code_ref(
    code_ref: &str,
    code_source_query: Option<&str>,
    sql: Option<&SqlRuntime>,
    attach: &AttachmentFetcher<'_>,
) -> Result<(String, String), Value> {
    let (rt, query_template) = match (sql, code_source_query) {
        (Some(rt), Some(q)) if !q.trim().is_empty() => (rt, q),
        _ => {
            return Err(serde_json::json!({
                "error": "CodeRefNotEnabled",
                "message": "`code_ref` was supplied but this tool instance has no \
                             fixed_config.sql / fixed_config.code_source_query configured",
            }));
        }
    };

    if !is_valid_code_ref_key(code_ref) {
        return Err(serde_json::json!({
            "error": "invalid_args",
            "message": format!(
                "code_ref '{code_ref}' contains disallowed characters; only letters, \
                 digits, and '_.:/-' are permitted"
            ),
        }));
    }

    let resolved_query = query_template.replace(CODE_REF_PLACEHOLDER, code_ref);
    let sql_ctx = SqlBindingCtx {
        adapter: rt.adapter.clone(),
        permissions: rt.permissions.clone(),
        allowed_schemas: rt.allowed_schemas.clone(),
        tenant: rt.tenant.clone(),
    };
    let binding = DataBinding {
        var: CODE_REF_BINDING_VAR.to_string(),
        attachment_id: None,
        spreadsheet_id: None,
        sheet: None,
        range: None,
        query: Some(resolved_query),
        data: None,
        delimiter: None,
        sheet_name: None,
        header_row: None,
    };

    let resolved =
        resolve_bindings(std::slice::from_ref(&binding), Some(&sql_ctx), None, attach).await?;

    let records = resolved
        .inputs
        .get(CODE_REF_BINDING_VAR)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    if records.is_empty() {
        return Err(serde_json::json!({
            "error": "CodeRefNotFound",
            "message": format!("code_ref '{code_ref}' matched zero rows"),
        }));
    }
    if records.len() > 1 {
        return Err(serde_json::json!({
            "error": "CodeRefAmbiguous",
            "message": format!(
                "code_ref '{code_ref}' matched {} rows; expected exactly one",
                records.len()
            ),
        }));
    }
    let row = match records[0].as_object() {
        Some(o) => o,
        None => {
            return Err(serde_json::json!({
                "error": "CodeRefShapeInvalid",
                "message": "code_source_query result row is not an object",
            }));
        }
    };
    if row.len() != 1 {
        return Err(serde_json::json!({
            "error": "CodeRefShapeInvalid",
            "message": format!(
                "code_source_query must return exactly one column; got {}",
                row.len()
            ),
        }));
    }
    let cell = row.values().next().unwrap();
    let code_str = match cell {
        Value::String(s) => s.clone(),
        _ => {
            return Err(serde_json::json!({
                "error": "CodeRefShapeInvalid",
                "message": "code_source_query cell is not a text value",
            }));
        }
    };
    if code_str.trim().is_empty() {
        return Err(serde_json::json!({
            "error": "CodeRefEmpty",
            "message": format!("code_ref '{code_ref}' resolved to empty/whitespace-only code"),
        }));
    }

    let hash = {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(code_str.as_bytes());
        format!("{:x}", hasher.finalize())
    };

    Ok((code_str, hash))
}

/// Attach `code_ref` + `code_hash` audit fields to a successful response
/// when the code was sourced via `code_ref` (no-op — fields stay
/// absent — for inline `code`). Extracted as a pure function so the
/// audit-field contract is unit-testable without executing the sandbox.
fn with_code_ref_audit(mut response: Value, audit: Option<&(String, String)>) -> Value {
    if let Some((code_ref, code_hash)) = audit {
        response["code_ref"] = Value::String(code_ref.clone());
        response["code_hash"] = Value::String(code_hash.clone());
    }
    response
}

// ── Dispatch (orchestration) ─────────────────────────────────────────

/// Wall-clock cap for one sandbox execution.
const CODE_TIMEOUT_SECS: u64 = 30;
/// Per-string byte cap on the surfaced `output` / `stdout` strings.
const OUTPUT_BYTE_CAP: usize = 10 * 1024;

/// Fully-resolved SQL runtime for a `data_run_python` call. Built once by
/// the executor wrapper from the tool's `fixed_config.sql`, then threaded
/// into [`dispatch_core`]. `adapter` (a trait object) drives the SQL
/// `SELECT` bindings; `pool` (the concrete `PgPool` behind it) drives the
/// `output_tables` write sink; both point at the same short-lived pool.
pub struct SqlRuntime {
    pub adapter: Arc<dyn SqlConnectionPort>,
    pub pool: Arc<sqlx::PgPool>,
    pub permissions: SqlPermissions,
    pub allowed_schemas: Vec<String>,
    pub tenant: Option<String>,
    pub on_missing_table: String,
    pub on_existing_table: String,
    /// Operator-configured write limits (from `fixed_config.sql.runtime_limits`),
    /// threaded into the `output_tables` write transaction so writes honor the
    /// same `statement_timeout`/`work_mem` the read side already applies.
    pub statement_timeout_ms: u64,
    pub work_mem_mb: u64,
}

/// Truncate a string to `cap` bytes on a UTF-8 boundary, appending a
/// `[truncated]` marker when cut. Mirrors `gsheets_run_python`/`attachment_run_python`.
fn truncate(s: &str, cap: usize) -> String {
    if s.len() <= cap {
        return s.to_string();
    }
    let mut end = cap;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{} [truncated]", &s[..end])
}

/// Cap the surfaced `output` value: serialize, and if it exceeds the byte
/// cap replace it with a truncated string form so a huge `output` never
/// floods the LLM context. Returns `(value, truncated)`.
fn cap_output(value: &Value, cap: usize) -> (Value, bool) {
    let as_text = value.to_string();
    if as_text.len() <= cap {
        return (value.clone(), false);
    }
    (Value::String(truncate(&as_text, cap)), true)
}

/// Testable orchestration seam for `data_run_python`. Parses args, resolves
/// bindings (SQL / gsheets / attachment / inline) into sandbox inputs, runs
/// the wrapped user code in the restricted Python sandbox, then applies the
/// three write sinks (`output_tables`, `output_sheets`, `output_attachments`)
/// gated by which sources the operator enabled. Returns the tool-result
/// `Value` the LLM sees.
///
/// The executor wrapper [`dispatch_data_run_python_via_executor`] builds the
/// `sql`/`sheets`/`attach`/`registrar` inputs from a live `DagToolExecutor`;
/// unit tests build them directly (no DB / network needed for the
/// inline-only and SourceNotEnabled paths).
pub async fn dispatch_core(
    args: Value,
    enabled: &EnabledSources,
    sql: Option<&SqlRuntime>,
    sheets: Option<&Arc<dyn SheetsClient>>,
    attach: &AttachmentFetcher<'_>,
    registrar: &AttachmentRegistrar<'_>,
    code_source_query: Option<&str>,
) -> Value {
    // 1. Parse args.
    let parsed: DataRunPythonArgs = match serde_json::from_value(args) {
        Ok(a) => a,
        Err(e) => {
            return serde_json::json!({
                "error": "invalid_args",
                "message": e.to_string(),
            });
        }
    };

    // 1.5. Enforce exactly one of `code` / `code_ref` BEFORE any sandbox or
    //      SQL execution runs — see spec "Exactly-One-Of Code Source".
    match (&parsed.code, &parsed.code_ref) {
        (Some(_), Some(_)) => {
            return serde_json::json!({
                "error": "invalid_args",
                "message": "provide exactly one of `code` or `code_ref`, not both",
            });
        }
        (None, None) => {
            return serde_json::json!({
                "error": "invalid_args",
                "message": "provide exactly one of `code` or `code_ref`",
            });
        }
        _ => {}
    }

    if let Err(e) = validate_bindings(&parsed.bindings) {
        return e;
    }

    // 2. Build the SQL binding context (for SQL `SELECT` bindings) from the
    //    runtime, if SQL is configured. When absent, any SQL binding fails
    //    inside `resolve_bindings` with SourceNotEnabled{source:"sql"}.
    let sql_ctx = sql.map(|rt| SqlBindingCtx {
        adapter: rt.adapter.clone(),
        permissions: rt.permissions.clone(),
        allowed_schemas: rt.allowed_schemas.clone(),
        tenant: rt.tenant.clone(),
    });

    // 3. Resolve every binding into the sandbox inputs map (parallel fetch).
    let rbindings = match resolve_bindings(&parsed.bindings, sql_ctx.as_ref(), sheets, attach).await
    {
        Ok(r) => r,
        Err(e) => return e,
    };

    // 3.5. Resolve the code to run: inline `code` used verbatim, or
    //      `code_ref` resolved via `code_source_query` (step 1.5 guarantees
    //      exactly one of the two is set).
    let (user_code, code_ref_audit): (String, Option<(String, String)>) =
        match (&parsed.code, &parsed.code_ref) {
            (Some(c), None) => (c.clone(), None),
            (None, Some(code_ref)) => {
                match resolve_code_ref(code_ref, code_source_query, sql, attach).await {
                    Ok((code, hash)) => (code, Some((code_ref.clone(), hash))),
                    Err(e) => return e,
                }
            }
            _ => unreachable!("step 1.5 guarantees exactly one of code/code_ref is set"),
        };

    // 4. Wrap + run the user code in the restricted sandbox with a timeout.
    let wrapped = wrap_user_code(&user_code);
    let inputs = rbindings.inputs.clone();
    let loaded_columns = rbindings.loaded_columns.clone();
    let sandbox = tokio::time::timeout(
        std::time::Duration::from_secs(CODE_TIMEOUT_SECS),
        tokio::task::spawn_blocking(move || {
            crate::dag_engine::infrastructure::nodes::python_node::execute_sandboxed_helper(
                &wrapped,
                "restricted",
                CODE_TIMEOUT_SECS,
                &inputs,
            )
        }),
    )
    .await;

    let helper = match sandbox {
        Ok(Ok(Ok(r))) => r,
        Ok(Ok(Err(e))) => {
            return serde_json::json!({
                "output": Value::Null,
                "stdout": String::new(),
                "error": truncate(&e, OUTPUT_BYTE_CAP),
                "loaded_columns": loaded_columns,
            });
        }
        Ok(Err(join_err)) => {
            return serde_json::json!({
                "output": Value::Null,
                "stdout": String::new(),
                "error": format!("internal join error: {join_err}"),
                "loaded_columns": loaded_columns,
            });
        }
        Err(_) => {
            return serde_json::json!({
                "output": Value::Null,
                "stdout": String::new(),
                "error": format!("code execution exceeded {CODE_TIMEOUT_SECS}s timeout"),
                "loaded_columns": loaded_columns,
            });
        }
    };

    let stdout = truncate(&helper.stdout, OUTPUT_BYTE_CAP);
    // The postlude wraps everything into
    // {user_output, output_sheets, output_tables, output_attachments}.
    let wrapped_output = helper.output.unwrap_or(Value::Null);
    let user_output = wrapped_output
        .get("user_output")
        .cloned()
        .unwrap_or(Value::Null);
    let out_tables = wrapped_output
        .get("output_tables")
        .cloned()
        .unwrap_or(Value::Null);
    let out_sheets = wrapped_output
        .get("output_sheets")
        .cloned()
        .unwrap_or(Value::Null);
    let out_attachments = wrapped_output
        .get("output_attachments")
        .cloned()
        .unwrap_or(Value::Null);

    let has_tables = out_tables.as_object().is_some_and(|m| !m.is_empty());
    let has_sheets = out_sheets.as_object().is_some_and(|m| !m.is_empty());
    let has_attachments = out_attachments.as_object().is_some_and(|m| !m.is_empty());

    // 5. output_tables sink (SQL write-back).
    let mut wrote_tables: Option<Value> = None;
    if has_tables {
        let Some(rt) = sql else {
            return serde_json::json!({"error": "SourceNotEnabled", "source": "sql"});
        };
        let specs = match parse_output_tables(&out_tables) {
            Ok(s) => s,
            Err(e) => return e,
        };
        let result = write_output_tables(
            &rt.pool,
            specs,
            &rt.allowed_schemas,
            &rt.permissions,
            rt.tenant.as_deref(),
            &rbindings.sql_snapshots,
            &rt.on_missing_table,
            &rt.on_existing_table,
            rt.statement_timeout_ms,
            rt.work_mem_mb,
        )
        .await;
        wrote_tables = Some(result);
    }

    // 6. output_sheets sink (Google Sheets write-back).
    let mut wrote_sheets: Option<Value> = None;
    let mut sheets_warning: Option<&'static str> = None;
    let has_target = parsed.write_to_spreadsheet.is_some();
    if has_sheets {
        if !has_target {
            sheets_warning = Some(
                "Script assigned `output_sheets` but no `write_to_spreadsheet` arg \
                 was provided; the result was discarded.",
            );
        } else if !enabled.gsheets {
            return serde_json::json!({"error": "SourceNotEnabled", "source": "gsheets"});
        } else {
            let Some(client) = sheets else {
                return serde_json::json!({"error": "SourceNotEnabled", "source": "gsheets"});
            };
            let target = parsed.write_to_spreadsheet.as_deref().unwrap();
            // Operator-set policy: reject an invalid value loudly (consistent with
            // the SQL sink's InvalidPolicy) instead of silently defaulting.
            let policy: CollisionPolicy = match parsed
                .on_existing_sheet
                .as_deref()
                .map(parse_policy)
                .transpose()
            {
                Ok(p) => p.unwrap_or_default(),
                Err(e) => {
                    return serde_json::json!({
                        "error": "InvalidPolicy",
                        "field": "on_existing_sheet",
                        "message": format!("invalid on_existing_sheet: {e}"),
                    });
                }
            };
            // Per-tab load snapshots from the GSHEETS bindings enable
            // output_sheets `update_by_position` (diff the returned df against
            // the loaded rows by position). Empty when no gsheets binding was
            // loaded → that mode errors clearly (UpdateByPositionRequiresBinding).
            let results = write_output_sheets(
                client,
                &SpreadsheetId(target.to_string()),
                &out_sheets,
                policy,
                &rbindings.gsheets_snapshots,
            )
            .await;
            wrote_sheets = Some(Value::Array(results));
        }
    } else if has_target {
        sheets_warning = Some(
            "write_to_spreadsheet was set but the script did not assign \
             `output_sheets = {...}`; nothing was written.",
        );
    }

    // 7. output_attachments sink (register CSV/XLSX in the catalog).
    let mut wrote_attachments: Option<Value> = None;
    if has_attachments {
        wrote_attachments = Some(write_output_attachments(&out_attachments, registrar).await);
    }

    // 8. Assemble the response.
    let (output_capped, output_truncated) = cap_output(&user_output, OUTPUT_BYTE_CAP);
    let mut response = serde_json::json!({
        "output": output_capped,
        "stdout": stdout,
        "error": Value::Null,
    });
    if output_truncated {
        response["_output_truncated"] = serde_json::json!(true);
    }
    if let Some(v) = wrote_tables {
        response["wrote_tables"] = v;
    }
    if let Some(v) = wrote_sheets {
        response["wrote_sheets"] = v;
    }
    if let Some(v) = wrote_attachments {
        response["wrote_attachments"] = v;
    }
    if let Some(w) = sheets_warning {
        response["_warning"] = Value::String(w.to_string());
    }
    with_code_ref_audit(response, code_ref_audit.as_ref())
}

/// Executor entry point: build the SQL runtime, gsheets client, attachment
/// fetcher/registrar closures from a live `DagToolExecutor`, then delegate to
/// [`dispatch_core`]. Routed from `dag_tool_executor::execute_inner`.
pub async fn dispatch_data_run_python_via_executor(
    exec: &crate::dag_engine::infrastructure::dag_tool_executor::DagToolExecutor,
    tool_call: &crate::llm::domain::tools::ToolCall,
    fixed: &HashMap<String, Value>,
) -> Value {
    // Parse args string → Value.
    let mut args: Value = match serde_json::from_str(&tool_call.function.arguments) {
        Ok(v) => v,
        Err(e) => {
            return serde_json::json!({
                "error": "invalid_args",
                "message": format!("failed to parse tool arguments: {e}"),
            });
        }
    };

    // Inject the operator-set gsheets collision policy from fixed_config so
    // it reaches `DataRunPythonArgs.on_existing_sheet` (not an LLM field).
    if let Some(policy) = fixed.get("on_existing_sheet") {
        if let Value::Object(map) = &mut args {
            map.insert("on_existing_sheet".to_string(), policy.clone());
        }
    }

    // Build the SQL runtime from fixed_config.sql (if present + valid).
    let sql_cfg = match parse_sql_config(fixed) {
        Ok(c) => c,
        Err(e) => {
            return serde_json::json!({
                "error": "sql_config_invalid",
                "message": e,
            });
        }
    };

    let sql_runtime: Option<SqlRuntime> = match &sql_cfg {
        Some(cfg) => {
            let pool =
                match super::sql_bulk_tools::build_short_lived_pool(&cfg.connection_url).await {
                    Ok(p) => Arc::new(p),
                    Err(e) => {
                        return serde_json::json!({
                            "error": "sql_connect_failed",
                            "message": e,
                        });
                    }
                };
            let adapter = Arc::new(
                crate::dag_engine::infrastructure::sql_pool_adapter::PgPoolAdapter::new(
                    pool.clone(),
                    cfg.statement_timeout_ms,
                    cfg.work_mem_mb,
                ),
            );
            let adapter_dyn: Arc<dyn SqlConnectionPort> = adapter.clone();
            Some(SqlRuntime {
                adapter: adapter_dyn,
                pool: adapter.pool(),
                permissions: cfg.permissions.clone(),
                allowed_schemas: cfg.allowed_schemas.clone(),
                tenant: cfg.tenant_user_id.clone(),
                on_missing_table: cfg.on_missing_table.clone(),
                on_existing_table: cfg.on_existing_table.clone(),
                statement_timeout_ms: cfg.statement_timeout_ms,
                work_mem_mb: cfg.work_mem_mb,
            })
        }
        None => None,
    };

    // Determine which sources are enabled. The gsheets client builds from
    // process env; treat a build failure as "gsheets not available".
    let gsheets_client: Option<Arc<dyn SheetsClient>> = {
        use crate::gsheets::infrastructure::config::GSheetsConfig;
        use crate::gsheets::infrastructure::http_client::GoogleSheetsHttpClient;
        match GoogleSheetsHttpClient::from_config(&GSheetsConfig::from_env()) {
            Ok(c) => Some(Arc::new(c) as Arc<dyn SheetsClient>),
            Err(e) => {
                // Degrade to "gsheets not available" (a graph may legitimately not
                // use gsheets), but log the reason so a genuine credential/config
                // misconfiguration is diagnosable instead of silently disabling
                // the source.
                crate::colmena_log!(
                    "WARN: [data_run_python] gsheets client unavailable ({e}); \
                     Google Sheets source/sink disabled for this call"
                );
                None
            }
        }
    };
    let enabled = enabled_sources(fixed, gsheets_client.is_some());

    // Attachment fetcher: document_id → (bytes, format_hint). `StoredBytes.filename`
    // is the storage key (`<uuid>.bin`) for path/inline-registered files, which
    // defeats csv/xlsx format detection — the ORIGINAL mime/filename lives in the
    // conversation catalog. Prefer the catalog's mime_type (then its original
    // filename) so `parse_attachment_to_records` can recognize the format.
    let fetch: AttachmentFetcher = Box::new(move |id: String| {
        Box::pin(async move {
            let sb = exec.fetch_attachment_bytes(&id).await?;
            let hint = match exec.lookup_attachment_meta(&id) {
                Some((mime, filename)) => {
                    if !mime.is_empty() && mime != "application/octet-stream" {
                        mime
                    } else if !filename.is_empty() {
                        filename
                    } else {
                        sb.filename
                    }
                }
                None => sb.filename,
            };
            Ok((sb.bytes, hint))
        })
    });

    // Attachment registrar: (name, bytes) → document_id. Derive mime from ext.
    let register: AttachmentRegistrar = Box::new(move |name: String, bytes: Vec<u8>| {
        Box::pin(async move {
            let mime = mime_from_name(&name);
            exec.register_attachment_bytes(bytes, mime, name).await
        })
    });

    // Operator-only: WHERE stored code lives for `code_ref` resolution. Read
    // straight from `fixed` (never LLM-visible), mirroring `fixed_config.sql`.
    let code_source_query = fixed.get("code_source_query").and_then(|v| v.as_str());

    dispatch_core(
        args,
        &enabled,
        sql_runtime.as_ref(),
        gsheets_client.as_ref(),
        &fetch,
        &register,
        code_source_query,
    )
    .await
}

/// Derive an attachment MIME type from a filename extension. Only the two
/// export formats `write_output_attachments` produces are recognized; any
/// other extension falls back to a generic binary type.
fn mime_from_name(name: &str) -> String {
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".csv") {
        "text/csv".to_string()
    } else if lower.ends_with(".xlsx") {
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet".to_string()
    } else {
        "application/octet-stream".to_string()
    }
}

// ── Tool builder ─────────────────────────────────────────────────────

/// Strip capability-gated blocks from the static base description so the
/// model only ever reads guidance for sources/sinks the operator actually
/// enabled. Blocks are delimited by standalone marker lines
/// `<!--SQL-->`/`<!--/SQL-->` and `<!--SHEETS-->`/`<!--/SHEETS-->` (in the
/// `data_run_python.yaml` description). A block's content is kept (markers
/// removed) when its capability is enabled and dropped entirely when not.
/// SQL and SHEETS blocks are disjoint — no nesting.
fn render_gated_description(base: &str, enabled: &EnabledSources) -> String {
    let mut out = String::with_capacity(base.len());
    let mut skip = false;
    for line in base.lines() {
        match line.trim() {
            "<!--SQL-->" => {
                skip = !enabled.sql;
                continue;
            }
            "<!--SHEETS-->" => {
                skip = !enabled.gsheets;
                continue;
            }
            "<!--/SQL-->" | "<!--/SHEETS-->" => {
                skip = false;
                continue;
            }
            _ => {}
        }
        if !skip {
            out.push_str(line);
            out.push('\n');
        }
    }
    // Drop the trailing newline introduced by the last push.
    while out.ends_with('\n') {
        out.pop();
    }
    out
}

/// Build the [`ToolDefinition`] for `data_run_python`, appending a
/// dynamically-assembled "Available sources" section to the static base
/// description so the model only ever sees sources the operator actually
/// enabled.
pub fn tool_data_run_python(enabled: &EnabledSources) -> ToolDefinition {
    let mut description =
        render_gated_description(text::tool_description(TOOL_DATA_RUN_PYTHON), enabled);

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
    use super::super::attachment_writer::AttachmentRegistrar;
    use super::super::tabular_bindings::AttachmentFetcher;
    use super::*;

    fn noop_attach<'a>() -> AttachmentFetcher<'a> {
        Box::new(|_id| Box::pin(async { Err("no attachment fetcher in this test".to_string()) }))
    }
    fn noop_registrar<'a>() -> AttachmentRegistrar<'a> {
        Box::new(|_n, _b| Box::pin(async { Err("no registrar in this test".to_string()) }))
    }

    #[tokio::test]
    async fn dispatch_rejects_unconfigured_sql_source() {
        let args = serde_json::json!({
            "bindings": [{"var": "t", "query": "SELECT 1"}],
            "code": "output=1"
        });
        let attach = noop_attach();
        let reg = noop_registrar();
        let out = dispatch_core(
            args,
            &EnabledSources {
                sql: false,
                gsheets: false,
            },
            None,
            None,
            &attach,
            &reg,
            None,
        )
        .await;
        assert_eq!(out["error"], "SourceNotEnabled");
        assert_eq!(out["source"], "sql");
        assert!(out.get("code_ref").is_none(), "got: {out}");
        assert!(out.get("code_hash").is_none(), "got: {out}");
    }

    #[tokio::test]
    #[ignore = "executes the PyO3 pandas sandbox; the shared per-process interpreter \
                flakes with `No module named pandas` when another sandbox test inits it \
                first under parallel `cargo test` aggregation. Passes in isolation; the \
                happy path is covered by the live E2E graphs (Task 16). Run with \
                `cargo test -- --ignored`."]
    async fn dispatch_inline_binding_happy_path_no_sinks() {
        // Inline data only, no SQL/gsheets/attachment sources, no output_* sinks.
        // The sandbox computes `output` and it comes back verbatim with no
        // wrote_tables / wrote_sheets / wrote_attachments sections.
        pyo3::Python::initialize();
        let args = serde_json::json!({
            "bindings": [{"var": "rows", "data": [{"n": 2}, {"n": 3}]}],
            "code": "output = int(pd.DataFrame(rows)['n'].sum())"
        });
        let attach = noop_attach();
        let reg = noop_registrar();
        let out = dispatch_core(
            args,
            &EnabledSources {
                sql: false,
                gsheets: false,
            },
            None,
            None,
            &attach,
            &reg,
            None,
        )
        .await;
        assert_eq!(out["error"], Value::Null, "unexpected error: {out}");
        assert_eq!(out["output"], serde_json::json!(5), "got: {out}");
        assert!(out.get("wrote_tables").is_none(), "got: {out}");
        assert!(out.get("wrote_sheets").is_none(), "got: {out}");
        assert!(out.get("wrote_attachments").is_none(), "got: {out}");
    }

    #[tokio::test]
    #[ignore = "executes the PyO3 pandas sandbox; the shared per-process interpreter \
                flakes with `No module named pandas` when another sandbox test inits it \
                first under parallel `cargo test` aggregation. Passes in isolation. Run \
                with `cargo test -- --ignored`."]
    async fn dispatch_coerces_raw_numpy_scalar_output() {
        // Regression: assigning a bare numpy scalar (np.int64) to `output` —
        // without wrapping in int() — must serialize cleanly via the postlude
        // coercion helper, not fail with "unsupported type int64".
        pyo3::Python::initialize();
        let args = serde_json::json!({
            "bindings": [{"var": "rows", "data": [{"n": 2}, {"n": 3}]}],
            "code": "output = pd.DataFrame(rows)['n'].sum()"  // raw numpy int64
        });
        let attach = noop_attach();
        let reg = noop_registrar();
        let out = dispatch_core(
            args,
            &EnabledSources {
                sql: false,
                gsheets: false,
            },
            None,
            None,
            &attach,
            &reg,
            None,
        )
        .await;
        assert_eq!(out["error"], Value::Null, "unexpected error: {out}");
        assert_eq!(out["output"], serde_json::json!(5), "got: {out}");
    }

    #[tokio::test]
    #[ignore = "executes the PyO3 pandas sandbox; the shared per-process interpreter \
                flakes with `No module named pandas` when another sandbox test inits it \
                first under parallel `cargo test` aggregation. Passes in isolation. Run \
                with `cargo test -- --ignored`."]
    async fn dispatch_coerces_series_and_nested_numpy_output() {
        // The postlude coercion is recursive and maps a Series to a flat
        // list. A Series output → `[10, 20]` (not an index-keyed dict), and a
        // dict whose values are numpy scalars → each value coerced. Both
        // failed (or were mis-shaped) before the recursive helper.
        pyo3::Python::initialize();

        // Series → flat list.
        let series = dispatch_core(
            serde_json::json!({
                "bindings": [{"var": "rows", "data": [{"n": 10}, {"n": 20}]}],
                "code": "output = pd.DataFrame(rows)['n']"  // a Series
            }),
            &EnabledSources {
                sql: false,
                gsheets: false,
            },
            None,
            None,
            &noop_attach(),
            &noop_registrar(),
            None,
        )
        .await;
        assert_eq!(series["error"], Value::Null, "unexpected error: {series}");
        assert_eq!(
            series["output"],
            serde_json::json!([10, 20]),
            "got: {series}"
        );

        // Dict of numpy aggregates → each value coerced (was "unsupported type").
        let nested = dispatch_core(
            serde_json::json!({
                "bindings": [{"var": "rows", "data": [{"n": 2}, {"n": 3}]}],
                "code": "df = pd.DataFrame(rows)\noutput = {\"total\": df['n'].sum(), \"rows\": [df['n'].min()]}"
            }),
            &EnabledSources { sql: false, gsheets: false },
            None,
            None,
            &noop_attach(),
            &noop_registrar(),
            None,
        )
        .await;
        assert_eq!(nested["error"], Value::Null, "unexpected error: {nested}");
        assert_eq!(
            nested["output"]["total"],
            serde_json::json!(5),
            "got: {nested}"
        );
        assert_eq!(
            nested["output"]["rows"],
            serde_json::json!([2]),
            "got: {nested}"
        );
    }

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
    fn gated_description_keeps_only_enabled_blocks() {
        let base = "always\n<!--SQL-->\nsql-only\n<!--/SQL-->\nmid\n<!--SHEETS-->\nsheets-only\n<!--/SHEETS-->\ntail";

        let sql_only = render_gated_description(
            base,
            &EnabledSources {
                sql: true,
                gsheets: false,
            },
        );
        assert!(sql_only.contains("sql-only"));
        assert!(!sql_only.contains("sheets-only"));
        assert!(!sql_only.contains("<!--")); // markers stripped
        assert!(
            sql_only.contains("always") && sql_only.contains("mid") && sql_only.contains("tail")
        );

        let sheets_only = render_gated_description(
            base,
            &EnabledSources {
                sql: false,
                gsheets: true,
            },
        );
        assert!(sheets_only.contains("sheets-only"));
        assert!(!sheets_only.contains("sql-only"));

        let neither = render_gated_description(
            base,
            &EnabledSources {
                sql: false,
                gsheets: false,
            },
        );
        assert!(!neither.contains("sql-only"));
        assert!(!neither.contains("sheets-only"));
        assert!(neither.contains("always") && neither.contains("tail"));
    }

    #[test]
    fn gated_description_real_yaml_hides_sheets_when_disabled() {
        // Guard the actual shipped YAML: a SQL-only deployment must not
        // leak any Google Sheets guidance into the tool description.
        let sql_only = render_gated_description(
            text::tool_description(TOOL_DATA_RUN_PYTHON),
            &EnabledSources {
                sql: true,
                gsheets: false,
            },
        )
        .to_lowercase();
        assert!(!sql_only.contains("google sheet"));
        assert!(!sql_only.contains("update_by_position"));
        assert!(!sql_only.contains("write_to_spreadsheet"));
        assert!(sql_only.contains("output_tables"));

        // ...and a gsheets-only deployment must not advertise SQL sinks.
        let sheets_only = render_gated_description(
            text::tool_description(TOOL_DATA_RUN_PYTHON),
            &EnabledSources {
                sql: false,
                gsheets: true,
            },
        )
        .to_lowercase();
        assert!(!sheets_only.contains("output_tables"));
        assert!(sheets_only.contains("update_by_position"));
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

    // ── code_ref: precedence (exactly-one-of) ────────────────────────

    #[tokio::test]
    async fn code_ref_and_code_both_set_errors() {
        let args = serde_json::json!({
            "bindings": [{"var": "t", "data": [{"a": 1}]}],
            "code": "output=1",
            "code_ref": "some_key"
        });
        let attach = noop_attach();
        let reg = noop_registrar();
        let out = dispatch_core(
            args,
            &EnabledSources {
                sql: false,
                gsheets: false,
            },
            None,
            None,
            &attach,
            &reg,
            None,
        )
        .await;
        assert_eq!(out["error"], "invalid_args", "got: {out}");
        let msg = out["message"].as_str().unwrap_or_default();
        assert!(
            msg.contains("code") && msg.contains("code_ref"),
            "got: {out}"
        );
    }

    #[tokio::test]
    async fn code_ref_and_code_neither_set_errors() {
        let args = serde_json::json!({
            "bindings": [{"var": "t", "data": [{"a": 1}]}],
        });
        let attach = noop_attach();
        let reg = noop_registrar();
        let out = dispatch_core(
            args,
            &EnabledSources {
                sql: false,
                gsheets: false,
            },
            None,
            None,
            &attach,
            &reg,
            None,
        )
        .await;
        assert_eq!(out["error"], "invalid_args", "got: {out}");
        let msg = out["message"].as_str().unwrap_or_default();
        assert!(
            msg.contains("code") && msg.contains("code_ref"),
            "got: {out}"
        );
    }

    // ── code_ref: SQL resolution (resolve_code_ref) ──────────────────

    /// A [`SqlConnectionPort`] mock that returns a canned row set (or a
    /// canned execution error) for `execute_query`, and panics if any other
    /// method is called — `resolve_code_ref` only ever needs `execute_query`.
    struct CannedAdapter {
        rows: Vec<Value>,
        fail: Option<String>,
    }

    #[async_trait::async_trait]
    impl SqlConnectionPort for CannedAdapter {
        async fn execute_query(
            &self,
            _query: &str,
            _max_rows: u64,
            _tenant_user_id: Option<&str>,
        ) -> Result<
            crate::dag_engine::domain::sql_ports::QueryResult,
            crate::dag_engine::domain::sql_errors::SqlNodeError,
        > {
            if let Some(msg) = &self.fail {
                return Err(
                    crate::dag_engine::domain::sql_errors::SqlNodeError::ExecutionError(
                        msg.clone(),
                    ),
                );
            }
            Ok(crate::dag_engine::domain::sql_ports::QueryResult {
                output: Value::Array(self.rows.clone()),
                row_count: self.rows.len() as u64,
                truncated: false,
            })
        }
        async fn load_table_metadata(
            &self,
            _schemas: &[String],
        ) -> Result<
            Vec<crate::dag_engine::domain::sql_ports::TableInfo>,
            crate::dag_engine::domain::sql_errors::SqlNodeError,
        > {
            unreachable!()
        }
        async fn load_table_schemas(
            &self,
            _schemas: &[String],
        ) -> Result<
            Vec<crate::dag_engine::domain::sql_ports::TableSchema>,
            crate::dag_engine::domain::sql_errors::SqlNodeError,
        > {
            unreachable!()
        }
        async fn missing_schemas(
            &self,
            _schemas: &[String],
        ) -> Result<Vec<String>, crate::dag_engine::domain::sql_errors::SqlNodeError> {
            unreachable!()
        }
        async fn create_schema(
            &self,
            _schema: &str,
        ) -> Result<(), crate::dag_engine::domain::sql_errors::SqlNodeError> {
            unreachable!()
        }
        async fn execute_setup_sql(
            &self,
            _sql: &str,
        ) -> Result<(), crate::dag_engine::domain::sql_errors::SqlNodeError> {
            unreachable!()
        }
        fn is_connected(&self) -> bool {
            true
        }
    }

    /// Build a [`SqlRuntime`] wrapping `adapter`. The `pool` field uses a
    /// lazily-connected (never-touched) pool — `resolve_code_ref` and the
    /// SELECT-only binding path never read `SqlRuntime::pool` (only the
    /// `output_tables` write sink does), so no real DB is needed.
    fn sql_runtime_with(adapter: CannedAdapter) -> SqlRuntime {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://invalid/db")
            .unwrap();
        SqlRuntime {
            adapter: Arc::new(adapter),
            pool: Arc::new(pool),
            permissions: SqlPermissions::from_config(None).unwrap(),
            allowed_schemas: vec![],
            tenant: None,
            on_missing_table: "create".to_string(),
            on_existing_table: "fail".to_string(),
            statement_timeout_ms: 30_000,
            work_mem_mb: 64,
        }
    }

    const CODE_REF_QUERY: &str = "SELECT code FROM recipes WHERE name='{{code_ref}}'";

    #[tokio::test]
    async fn code_ref_not_enabled_fails_closed_no_sql_runtime() {
        let out = resolve_code_ref("recipe_a", Some(CODE_REF_QUERY), None, &noop_attach())
            .await
            .unwrap_err();
        assert_eq!(out["error"], "CodeRefNotEnabled", "got: {out}");
    }

    #[tokio::test]
    async fn code_ref_not_enabled_fails_closed_no_query() {
        let rt = sql_runtime_with(CannedAdapter {
            rows: vec![],
            fail: None,
        });
        let out = resolve_code_ref("recipe_a", None, Some(&rt), &noop_attach())
            .await
            .unwrap_err();
        assert_eq!(out["error"], "CodeRefNotEnabled", "got: {out}");
    }

    #[tokio::test]
    async fn code_ref_zero_rows_fails_closed() {
        let rt = sql_runtime_with(CannedAdapter {
            rows: vec![],
            fail: None,
        });
        let out = resolve_code_ref("recipe_a", Some(CODE_REF_QUERY), Some(&rt), &noop_attach())
            .await
            .unwrap_err();
        assert_eq!(out["error"], "CodeRefNotFound", "got: {out}");
    }

    #[tokio::test]
    async fn code_ref_ambiguous_fails_closed_multi_row() {
        let rt = sql_runtime_with(CannedAdapter {
            rows: vec![
                serde_json::json!({"code": "a"}),
                serde_json::json!({"code": "b"}),
            ],
            fail: None,
        });
        let out = resolve_code_ref("recipe_a", Some(CODE_REF_QUERY), Some(&rt), &noop_attach())
            .await
            .unwrap_err();
        assert_eq!(out["error"], "CodeRefAmbiguous", "got: {out}");
    }

    #[tokio::test]
    async fn code_ref_ambiguous_fails_closed_multi_col() {
        let rt = sql_runtime_with(CannedAdapter {
            rows: vec![serde_json::json!({"code": "a", "other": "x"})],
            fail: None,
        });
        let out = resolve_code_ref("recipe_a", Some(CODE_REF_QUERY), Some(&rt), &noop_attach())
            .await
            .unwrap_err();
        assert_eq!(out["error"], "CodeRefShapeInvalid", "got: {out}");
    }

    #[tokio::test]
    async fn code_ref_empty_fails_closed() {
        let rt = sql_runtime_with(CannedAdapter {
            rows: vec![serde_json::json!({"code": "   "})],
            fail: None,
        });
        let out = resolve_code_ref("recipe_a", Some(CODE_REF_QUERY), Some(&rt), &noop_attach())
            .await
            .unwrap_err();
        assert_eq!(out["error"], "CodeRefEmpty", "got: {out}");
    }

    #[tokio::test]
    async fn code_ref_fetch_error_fails_closed() {
        let rt = sql_runtime_with(CannedAdapter {
            rows: vec![],
            fail: Some("connection reset".to_string()),
        });
        let out = resolve_code_ref("recipe_a", Some(CODE_REF_QUERY), Some(&rt), &noop_attach())
            .await
            .unwrap_err();
        assert_eq!(out["error"], "SqlExecutionFailed", "got: {out}");
    }

    #[tokio::test]
    async fn code_ref_invalid_key_rejected() {
        let rt = sql_runtime_with(CannedAdapter {
            rows: vec![],
            fail: None,
        });
        let out = resolve_code_ref(
            "recipe'; DROP TABLE recipes;--",
            Some(CODE_REF_QUERY),
            Some(&rt),
            &noop_attach(),
        )
        .await
        .unwrap_err();
        assert_eq!(out["error"], "invalid_args", "got: {out}");
    }

    #[tokio::test]
    async fn code_ref_resolves_single_cell_and_hashes() {
        let rt = sql_runtime_with(CannedAdapter {
            rows: vec![serde_json::json!({"code": "output = 1"})],
            fail: None,
        });
        let (code, hash) =
            resolve_code_ref("recipe_a", Some(CODE_REF_QUERY), Some(&rt), &noop_attach())
                .await
                .unwrap();
        assert_eq!(code, "output = 1");
        assert_eq!(hash.len(), 64, "expected a sha256 hex digest, got: {hash}");
    }

    // ── code_ref: sandbox passthrough + audit fields ─────────────────

    #[tokio::test]
    #[ignore = "executes the PyO3 pandas sandbox; the shared per-process interpreter \
                flakes with `No module named pandas` when another sandbox test inits it \
                first under parallel `cargo test` aggregation. Passes in isolation. Run \
                with `cargo test -- --ignored`."]
    async fn code_ref_banned_construct_passthrough() {
        // Resolved code with a banned construct must surface the SAME
        // SandboxViolation shape as equivalent inline code — no relaxation
        // of restricted-mode rules on the code_ref path.
        pyo3::Python::initialize();
        let rt = sql_runtime_with(CannedAdapter {
            rows: vec![serde_json::json!({"code": "output = exec('1')"})],
            fail: None,
        });
        let args = serde_json::json!({
            "bindings": [{"var": "rows", "data": [{"n": 1}]}],
            "code_ref": "danger"
        });
        let attach = noop_attach();
        let reg = noop_registrar();
        let out = dispatch_core(
            args,
            &EnabledSources {
                sql: true,
                gsheets: false,
            },
            Some(&rt),
            None,
            &attach,
            &reg,
            Some(CODE_REF_QUERY),
        )
        .await;
        assert!(
            out["error"].as_str().is_some(),
            "expected a sandbox violation error, got: {out}"
        );
    }

    #[tokio::test]
    #[ignore = "executes the PyO3 pandas sandbox; the shared per-process interpreter \
                flakes with `No module named pandas` when another sandbox test inits it \
                first under parallel `cargo test` aggregation. Passes in isolation. Run \
                with `cargo test -- --ignored`."]
    async fn code_ref_happy_path_execution() {
        // Resolved valid code runs with the same semantics/output shape as
        // inline code, plus the `code_ref`/`code_hash` audit fields.
        pyo3::Python::initialize();
        let rt = sql_runtime_with(CannedAdapter {
            rows: vec![serde_json::json!({"code": "output = int(pd.DataFrame(rows)['n'].sum())"})],
            fail: None,
        });
        let args = serde_json::json!({
            "bindings": [{"var": "rows", "data": [{"n": 2}, {"n": 3}]}],
            "code_ref": "sum_recipe"
        });
        let attach = noop_attach();
        let reg = noop_registrar();
        let out = dispatch_core(
            args,
            &EnabledSources {
                sql: true,
                gsheets: false,
            },
            Some(&rt),
            None,
            &attach,
            &reg,
            Some(CODE_REF_QUERY),
        )
        .await;
        assert_eq!(out["error"], Value::Null, "unexpected error: {out}");
        assert_eq!(out["output"], serde_json::json!(5), "got: {out}");
        assert_eq!(out["code_ref"], "sum_recipe", "got: {out}");
        assert_eq!(out["code_hash"].as_str().unwrap().len(), 64, "got: {out}");
    }

    #[test]
    fn code_ref_result_includes_audit_fields() {
        let base = serde_json::json!({"output": 1, "stdout": "", "error": Value::Null});
        let out = with_code_ref_audit(base, Some(&("recipe_a".to_string(), "abc123".to_string())));
        assert_eq!(out["code_ref"], "recipe_a");
        assert_eq!(out["code_hash"], "abc123");
    }

    #[test]
    fn inline_code_result_omits_code_ref_fields() {
        let base = serde_json::json!({"output": 1, "stdout": "", "error": Value::Null});
        let out = with_code_ref_audit(base, None);
        assert!(out.get("code_ref").is_none(), "got: {out}");
        assert!(out.get("code_hash").is_none(), "got: {out}");
    }

    // ── code_ref: backward compatibility ──────────────────────────────

    #[tokio::test]
    async fn existing_inline_code_only_graph_unaffected() {
        // No `code_ref`, no `code_source_query` — output/error shape must be
        // byte-for-byte identical to pre-code_ref behavior.
        let args = serde_json::json!({
            "bindings": [{"var": "t", "query": "SELECT 1"}],
            "code": "output=1"
        });
        let attach = noop_attach();
        let reg = noop_registrar();
        let out = dispatch_core(
            args,
            &EnabledSources {
                sql: false,
                gsheets: false,
            },
            None,
            None,
            &attach,
            &reg,
            None,
        )
        .await;
        assert_eq!(out["error"], "SourceNotEnabled");
        assert_eq!(out["source"], "sql");
        assert!(out.get("code_ref").is_none(), "got: {out}");
        assert!(out.get("code_hash").is_none(), "got: {out}");
    }
}
