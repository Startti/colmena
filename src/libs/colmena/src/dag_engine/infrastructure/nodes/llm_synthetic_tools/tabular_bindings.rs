//! Polymorphic tabular-binding types for the `data_run_python` synthetic
//! LLM tool. A [`DataBinding`] describes ONE Python global to bind inside
//! the pandas sandbox, sourced from exactly one of four origins:
//! attachment (CSV/XLSX), Google Sheets, SQL `SELECT`, or inline JSON.
//!
//! This module owns the pure types + validation ([`classify_binding`],
//! [`validate_bindings`]) PLUS the concurrent resolver
//! ([`resolve_bindings`]) that turns a `Vec<DataBinding>` into sandbox
//! inputs by fetching each source in parallel. Dispatch wiring (choosing
//! which `SqlBindingCtx`/`SheetsClient`/`AttachmentFetcher` to pass in)
//! lives in the `data_run_python` dispatcher.

use crate::dag_engine::domain::sql_permissions::SqlPermissions;
use crate::dag_engine::domain::sql_ports::SqlConnectionPort;
use crate::gsheets::domain::{
    ReadOptions, ReadResponse, SheetsClient, SpreadsheetId, ValueRenderOption,
};
use crate::gsheets::infrastructure::http_client::rectangle_to_records;
use futures::future::join_all;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

// ── Binding type ─────────────────────────────────────────────────────

/// One tabular binding: a Python variable name plus exactly one data
/// source. The active source is determined structurally by
/// [`classify_binding`] — no explicit `source` discriminator field is
/// required from the LLM.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct DataBinding {
    /// Python variable name to bind the records to (e.g. "products").
    /// Accepts UX aliases `binding_name` and `name`.
    #[serde(alias = "binding_name", alias = "name")]
    pub var: String,

    /// Attachment document id (CSV/XLSX source).
    #[serde(default)]
    pub attachment_id: Option<String>,

    /// Google Sheets spreadsheet id (gsheets source).
    #[serde(default)]
    pub spreadsheet_id: Option<String>,

    /// Google Sheets tab name (gsheets source). NOT aliased to
    /// `sheet_name` — that field is reserved for the attachment-XLSX tab
    /// selector below; the two sources are structurally distinct.
    #[serde(default)]
    pub sheet: Option<String>,

    /// Optional A1 range (gsheets source only). Omit to read the entire
    /// tab.
    #[serde(default)]
    pub range: Option<String>,

    /// SQL `SELECT` statement (sql source).
    #[serde(default)]
    pub query: Option<String>,

    /// Inline table data — an array of `{col: val}` objects, or a 2-D
    /// array whose first row is the header (inline source).
    #[serde(default)]
    pub data: Option<Value>,

    /// CSV delimiter override (attachment source, CSV only).
    #[serde(default)]
    pub delimiter: Option<String>,

    /// XLSX tab/sheet selector (attachment source, XLSX only). Distinct
    /// from `sheet` (gsheets tab) — do not conflate the two.
    #[serde(default)]
    pub sheet_name: Option<String>,

    /// Header row index override, 0-based (attachment source).
    #[serde(default)]
    pub header_row: Option<u32>,
}

/// The structural source a [`DataBinding`] resolves to.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum BindingKind {
    Attachment,
    Gsheets,
    Sql,
    Inline,
}

/// Classify a binding by inspecting which discriminator fields are
/// present. Exactly one of the four sources must be present:
/// `attachment_id`, `spreadsheet_id` + `sheet`, `query`, or `data`.
///
/// `spreadsheet_id` and `sheet` count as a single discriminator (the
/// gsheets source) — supplying only one of the two is treated as
/// "gsheets source present" for ambiguity-counting purposes, so it still
/// surfaces as an error when combined with another source, but the
/// error message doesn't distinguish "incomplete gsheets" from
/// "ambiguous"; that's fine for a structural classifier.
pub fn classify_binding(b: &DataBinding) -> Result<BindingKind, String> {
    let has_attachment = b.attachment_id.is_some();
    let has_gsheets = b.spreadsheet_id.is_some() || b.sheet.is_some();
    let has_sql = b.query.is_some();
    let has_inline = b.data.is_some();

    let present_count = [has_attachment, has_gsheets, has_sql, has_inline]
        .iter()
        .filter(|x| **x)
        .count();

    match present_count {
        1 => {
            if has_attachment {
                Ok(BindingKind::Attachment)
            } else if has_gsheets {
                Ok(BindingKind::Gsheets)
            } else if has_sql {
                Ok(BindingKind::Sql)
            } else {
                Ok(BindingKind::Inline)
            }
        }
        _ => Err(format!(
            "binding '{}' must have exactly one source: attachment_id | (spreadsheet_id+sheet) | query | data",
            b.var
        )),
    }
}

/// Validate a full list of bindings:
/// - non-empty
/// - every `var` is non-empty after trimming
/// - no duplicate `var`s
/// - every binding classifies to exactly one source
///
/// Returns a structured `invalid_args` JSON envelope on error, suitable
/// for returning directly as a tool error result.
pub fn validate_bindings(bindings: &[DataBinding]) -> Result<(), Value> {
    let invalid = |message: String| {
        Err(serde_json::json!({
            "error": "invalid_args",
            "message": message,
        }))
    };

    if bindings.is_empty() {
        return invalid("`bindings` must contain at least one entry".to_string());
    }

    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for b in bindings {
        let trimmed = b.var.trim();
        if trimmed.is_empty() {
            return invalid("every binding must have a non-empty `var` name".to_string());
        }
        if !seen.insert(trimmed.to_string()) {
            return invalid(format!("duplicate binding var '{}'", trimmed));
        }
        if let Err(e) = classify_binding(b) {
            return invalid(e);
        }
    }

    Ok(())
}

// ── Concurrent resolution ────────────────────────────────────────────

/// SQL execution context for resolving [`BindingKind::Sql`] bindings.
/// Bundles everything `resolve_bindings` needs to validate + run a
/// `SELECT`-only query against the configured connection.
pub struct SqlBindingCtx {
    pub adapter: Arc<dyn SqlConnectionPort>,
    pub permissions: SqlPermissions,
    pub allowed_schemas: Vec<String>,
    pub tenant: Option<String>,
}

/// Row cap for a single SQL binding fetch. One above the documented
/// 100_000-row ceiling so `row_count > 100_000` can still be observed
/// (the adapter would otherwise silently truncate at exactly the cap).
const SQL_BINDING_MAX_ROWS: u64 = 100_001;
const SQL_BINDING_ROW_LIMIT: u64 = 100_000;

/// Fetches attachment bytes for [`BindingKind::Attachment`] bindings.
/// Input is the `attachment_id`; output is `(bytes, mime_type_or_filename)`
/// — the second element is passed as both `mime_type` and `filename` hints
/// to [`crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::sql_bulk_tools::parse_attachment_to_records`]
/// (format sniffing falls back to filename when mime is unrecognized).
pub type AttachmentFetcher<'a> = Box<
    dyn Fn(String) -> Pin<Box<dyn Future<Output = Result<(Vec<u8>, String), String>> + Send + 'a>>
        + Send
        + Sync
        + 'a,
>;

/// Output of [`resolve_bindings`]: sandbox-ready inputs plus bookkeeping
/// used by the `data_run_python` dispatcher (column metadata for error
/// messages, and per-table SQL snapshots for diff-driven UPDATE write-back).
#[derive(Debug)]
pub struct ResolvedBindings {
    /// `var` → records array, ready to inject as Python globals.
    pub inputs: serde_json::Map<String, Value>,
    /// `var` → column-name array, plus an aggregate `_loaded_columns` key
    /// (mirrors `gsheets_run_python`'s `_gsheets_loaded_columns`).
    pub loaded_columns: Value,
    /// Qualified table name → records, retained only for SQL bindings whose
    /// query is (best-effort detected as) a plain `SELECT * FROM <table>`.
    /// Used later for diff-driven UPDATE write-back.
    pub sql_snapshots: HashMap<String, Vec<serde_json::Map<String, Value>>>,
}

/// One resolved binding: `(var, records, optional_sql_snapshot)`, where the
/// snapshot is `(qualified_table_name, records)` retained only for a plain
/// `SELECT * FROM <table>` SQL binding.
type ResolvedOne = (
    String,
    Value,
    Option<(String, Vec<serde_json::Map<String, Value>>)>,
);

/// Structured error envelope: `{"error": kind, "binding": var, ...extra}`.
fn binding_error(kind: &str, var: &str, extra: Value) -> Value {
    let mut obj = serde_json::Map::new();
    obj.insert("error".to_string(), Value::String(kind.to_string()));
    obj.insert("binding".to_string(), Value::String(var.to_string()));
    if let Value::Object(extra_obj) = extra {
        for (k, v) in extra_obj {
            obj.insert(k, v);
        }
    }
    Value::Object(obj)
}

/// Pull column names off the first record of a records array. Mirrors
/// `gsheets_run_python::extract_columns`.
fn extract_columns(values: &Value) -> Vec<String> {
    match values {
        Value::Array(arr) => arr
            .first()
            .and_then(|v| v.as_object())
            .map(|obj| obj.keys().cloned().collect())
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// Normalize inline `data`: array-of-objects is kept as-is; a 2-D array
/// (first row = header) is converted to records via `rectangle_to_records`.
fn normalize_inline_data(raw: &Value) -> Value {
    match raw.as_array().and_then(|a| a.first()) {
        Some(Value::Array(_)) => rectangle_to_records(raw),
        _ => raw.clone(),
    }
}

/// Best-effort detection of a plain `SELECT * FROM <table>` query (no WHERE/
/// JOIN/etc.) so its records can be retained as a snapshot for diff-driven
/// UPDATE write-back. Returns the qualified table name (schema.table or bare
/// table) when it matches, else `None`. Intentionally conservative: only
/// fires on the single simplest shape.
fn detect_plain_select_star_table(
    stmt: &crate::dag_engine::infrastructure::sql_ast::Statement,
) -> Option<String> {
    use crate::dag_engine::infrastructure::sql_ast::Statement;
    use sqlparser::ast::{GroupByExpr, SetExpr, TableFactor};

    let Statement::Query(q) = stmt else {
        return None;
    };
    let SetExpr::Select(select) = q.body.as_ref() else {
        return None;
    };
    let group_by_empty = matches!(
        &select.group_by,
        GroupByExpr::Expressions(exprs, modifiers) if exprs.is_empty() && modifiers.is_empty()
    );
    // Must be a bare `SELECT *` with no WHERE/GROUP BY/HAVING and exactly one
    // FROM item that is a plain table (no JOIN).
    if select.selection.is_some()
        || !group_by_empty
        || select.having.is_some()
        || select.projection.len() != 1
    {
        return None;
    }
    if !matches!(
        select.projection.first(),
        Some(sqlparser::ast::SelectItem::Wildcard(_))
    ) {
        return None;
    }
    if select.from.len() != 1 || !select.from[0].joins.is_empty() {
        return None;
    }
    match &select.from[0].relation {
        TableFactor::Table { name, .. } => Some(name.to_string()),
        _ => None,
    }
}

/// Resolve every binding's records CONCURRENTLY (`join_all` over one async
/// future per binding) and assemble the sandbox-ready [`ResolvedBindings`].
///
/// Any single binding failure short-circuits the whole call — the returned
/// `Err` is a structured JSON envelope with `"binding": <var>` attached so
/// the LLM/dispatcher can pinpoint which source failed.
pub async fn resolve_bindings(
    bindings: &[DataBinding],
    sql: Option<&SqlBindingCtx>,
    sheets: Option<&Arc<dyn SheetsClient>>,
    attach: &AttachmentFetcher<'_>,
) -> Result<ResolvedBindings, Value> {
    let futures = bindings.iter().map(|b| resolve_one(b, sql, sheets, attach));
    let results: Vec<Result<ResolvedOne, Value>> = join_all(futures).await;

    let mut inputs = serde_json::Map::new();
    let mut loaded_columns = serde_json::Map::new();
    let mut sql_snapshots: HashMap<String, Vec<serde_json::Map<String, Value>>> = HashMap::new();

    for r in results {
        let (var, records, snapshot) = r?;
        let columns = extract_columns(&records);
        loaded_columns.insert(
            var.clone(),
            Value::Array(columns.into_iter().map(Value::String).collect()),
        );
        if let Some((table, recs)) = snapshot {
            sql_snapshots.insert(table, recs);
        }
        inputs.insert(var, records);
    }

    let loaded_columns = Value::Object(loaded_columns);
    inputs.insert("_loaded_columns".to_string(), loaded_columns.clone());

    Ok(ResolvedBindings {
        inputs,
        loaded_columns,
        sql_snapshots,
    })
}

/// Resolve a single binding to `(var, records, optional_sql_snapshot)`.
async fn resolve_one(
    b: &DataBinding,
    sql: Option<&SqlBindingCtx>,
    sheets: Option<&Arc<dyn SheetsClient>>,
    attach: &AttachmentFetcher<'_>,
) -> Result<ResolvedOne, Value> {
    let kind = classify_binding(b).map_err(|message| {
        binding_error(
            "invalid_args",
            &b.var,
            serde_json::json!({ "message": message }),
        )
    })?;

    match kind {
        BindingKind::Inline => {
            let raw = b.data.clone().unwrap_or(Value::Array(vec![]));
            let records = normalize_inline_data(&raw);
            Ok((b.var.clone(), records, None))
        }

        BindingKind::Attachment => {
            let attachment_id = b.attachment_id.clone().unwrap();
            let (bytes, mime_or_name) = attach(attachment_id).await.map_err(|message| {
                binding_error(
                    "AttachmentFetchFailed",
                    &b.var,
                    serde_json::json!({ "message": message }),
                )
            })?;
            let (_columns, records) =
                crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::sql_bulk_tools::parse_attachment_to_records(
                    &bytes,
                    &mime_or_name,
                    &mime_or_name,
                    b.delimiter.as_deref(),
                    b.sheet_name.as_deref(),
                    b.header_row,
                )
                .map_err(|message| {
                    binding_error(
                        "AttachmentParseFailed",
                        &b.var,
                        serde_json::json!({ "message": message }),
                    )
                })?;
            let records_value = Value::Array(records.into_iter().map(Value::Object).collect());
            Ok((b.var.clone(), records_value, None))
        }

        BindingKind::Gsheets => {
            let Some(client) = sheets else {
                return Err(binding_error(
                    "SourceNotEnabled",
                    &b.var,
                    serde_json::json!({ "source": "gsheets" }),
                ));
            };
            let Some(spreadsheet_id) = b.spreadsheet_id.clone() else {
                return Err(binding_error(
                    "invalid_args",
                    &b.var,
                    serde_json::json!({ "message": "gsheets binding requires spreadsheet_id" }),
                ));
            };
            let Some(sheet) = b.sheet.clone() else {
                return Err(binding_error(
                    "invalid_args",
                    &b.var,
                    serde_json::json!({ "message": "gsheets binding requires sheet" }),
                ));
            };
            let opts = ReadOptions {
                value_render: ValueRenderOption::UnformattedValue,
                as_records: true,
            };
            let ss = SpreadsheetId(spreadsheet_id);
            let resp: ReadResponse = client
                .read_range(&ss, &sheet, b.range.as_deref(), opts)
                .await
                .map_err(|e| {
                    let mut payload = super::gsheets_tools::error_to_json(e);
                    if let Value::Object(ref mut m) = payload {
                        m.insert("binding".to_string(), Value::String(b.var.clone()));
                    }
                    payload
                })?;
            let records = match resp.values {
                Value::Array(a) => Value::Array(a),
                Value::Null => Value::Array(vec![]),
                other => other,
            };
            Ok((b.var.clone(), records, None))
        }

        BindingKind::Sql => {
            let Some(ctx) = sql else {
                return Err(binding_error(
                    "SourceNotEnabled",
                    &b.var,
                    serde_json::json!({ "source": "sql" }),
                ));
            };
            let query = b.query.clone().unwrap();

            let stmts = crate::dag_engine::infrastructure::sql_ast::parse(&query).map_err(|e| {
                binding_error(
                    "BindingMustBeSelect",
                    &b.var,
                    serde_json::json!({ "message": format!("failed to parse SQL: {e}") }),
                )
            })?;
            if stmts.len() != 1 {
                return Err(binding_error(
                    "BindingMustBeSelect",
                    &b.var,
                    serde_json::json!({
                        "message": "binding `query` must be exactly one SELECT statement",
                    }),
                ));
            }
            let stmt = &stmts[0];
            if crate::dag_engine::infrastructure::sql_ast::classify(stmt)
                != Some(crate::dag_engine::domain::sql_permissions::SqlOperation::Select)
            {
                return Err(binding_error(
                    "BindingMustBeSelect",
                    &b.var,
                    serde_json::json!({
                        "message": "binding `query` must be a SELECT statement",
                    }),
                ));
            }

            let validator =
                crate::dag_engine::infrastructure::sql_static_validator::StaticRuleValidator;
            let validation = {
                use crate::dag_engine::domain::sql_ports::SqlValidatorPort;
                validator.validate(&query, &ctx.permissions)
            };
            if !validation.allowed {
                return Err(binding_error(
                    "SchemaNotAllowed",
                    &b.var,
                    serde_json::json!({
                        "message": validation
                            .block_reason
                            .unwrap_or_else(|| "query rejected by validator".to_string()),
                    }),
                ));
            }

            let snapshot_table = detect_plain_select_star_table(stmt);

            let query_result = ctx
                .adapter
                .execute_query(&query, SQL_BINDING_MAX_ROWS, ctx.tenant.as_deref())
                .await
                .map_err(|e| {
                    binding_error(
                        "SqlExecutionFailed",
                        &b.var,
                        serde_json::json!({ "message": e.to_string() }),
                    )
                })?;

            if query_result.row_count > SQL_BINDING_ROW_LIMIT {
                return Err(binding_error(
                    "BindingTooLarge",
                    &b.var,
                    serde_json::json!({
                        "row_count": query_result.row_count,
                        "limit": SQL_BINDING_ROW_LIMIT,
                    }),
                ));
            }

            let records: Vec<serde_json::Map<String, Value>> = query_result
                .output
                .as_array()
                .map(|a| a.iter().filter_map(|v| v.as_object().cloned()).collect())
                .unwrap_or_default();

            let snapshot = snapshot_table.map(|t| (t, records.clone()));
            let records_value = Value::Array(records.into_iter().map(Value::Object).collect());
            Ok((b.var.clone(), records_value, snapshot))
        }
    }
}

// ── Flexible bindings deserializer ───────────────────────────────────

/// Custom deserializer for `bindings` that accepts either:
///
/// 1. The canonical array form:
///    `[{"var":"products","query":"SELECT * FROM products"}, ...]`
///
/// 2. An object whose VALUES are binding-shaped objects (treat the key
///    as `var`):
///    `{"products": {"query":"SELECT * FROM products"}, ...}`
///
/// LLMs frequently emit form 2 despite the schema declaring
/// `Vec<DataBinding>`. This deserializer tolerates both shapes. Form 3
/// (values are bare strings) is rejected with a clear error — a bare
/// string cannot disambiguate which of the four sources is meant.
///
/// Ported from `gsheets_run_python.rs::deserialize_bindings_flexible`
/// and generalized to build [`DataBinding`] instead of `GsheetsBinding`.
pub fn deserialize_bindings_flexible<'de, D>(deserializer: D) -> Result<Vec<DataBinding>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::{self, MapAccess, SeqAccess, Visitor};
    use std::fmt;

    struct BindingsVisitor;

    impl<'de> Visitor<'de> for BindingsVisitor {
        type Value = Vec<DataBinding>;

        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("an array of bindings or an object whose values are bindings")
        }

        // Canonical array form
        fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
        where
            A: SeqAccess<'de>,
        {
            let mut out: Vec<DataBinding> = Vec::new();
            while let Some(elem) = seq.next_element::<DataBinding>()? {
                out.push(elem);
            }
            Ok(out)
        }

        // LLM-hallucinated dict form
        fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
        where
            A: MapAccess<'de>,
        {
            let mut out: Vec<DataBinding> = Vec::new();
            while let Some((key, value)) = map.next_entry::<String, Value>()? {
                match value {
                    // Form 2: value is binding-shaped object. Merge in `var` from key.
                    Value::Object(mut obj) => {
                        obj.insert("var".to_string(), Value::String(key.clone()));
                        let binding: DataBinding = serde_json::from_value(Value::Object(obj))
                            .map_err(de::Error::custom)?;
                        out.push(binding);
                    }
                    // Form 3: value is just a string. Ambiguous — reject.
                    Value::String(_) => {
                        return Err(de::Error::custom(format!(
                            "`bindings` entry '{key}' is a bare string; \
                             cannot determine its data source. Use the array form: \
                             [{{\"var\":\"{key}\",\"query\":\"...\"}}] (or attachment_id / \
                             spreadsheet_id+sheet / data)"
                        )));
                    }
                    _ => {
                        return Err(de::Error::custom(format!(
                            "`bindings` entry '{key}' has unexpected value type; \
                             expected an object with one of: attachment_id, \
                             spreadsheet_id+sheet, query, data"
                        )));
                    }
                }
            }
            Ok(out)
        }
    }

    deserializer.deserialize_any(BindingsVisitor)
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn b(v: Value) -> DataBinding {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn classify_recognizes_each_source() {
        assert_eq!(
            classify_binding(&b(json!({"var":"a","attachment_id":"doc1"}))).unwrap(),
            BindingKind::Attachment
        );
        assert_eq!(
            classify_binding(&b(json!({"var":"a","spreadsheet_id":"1x","sheet":"Q4"}))).unwrap(),
            BindingKind::Gsheets
        );
        assert_eq!(
            classify_binding(&b(json!({"var":"a","query":"SELECT 1"}))).unwrap(),
            BindingKind::Sql
        );
        assert_eq!(
            classify_binding(&b(json!({"var":"a","data":[]}))).unwrap(),
            BindingKind::Inline
        );
    }

    #[test]
    fn classify_rejects_ambiguous_and_empty() {
        assert!(classify_binding(&b(
            json!({"var":"a","attachment_id":"d","query":"SELECT 1"})
        ))
        .is_err());
        assert!(classify_binding(&b(json!({"var":"a"}))).is_err());
    }

    #[test]
    fn validate_rejects_duplicate_and_empty_var() {
        let dup = vec![
            b(json!({"var":"x","query":"SELECT 1"})),
            b(json!({"var":"x","data":[]})),
        ];
        assert!(validate_bindings(&dup).is_err());
        let empty = vec![b(json!({"var":"","data":[]}))];
        assert!(validate_bindings(&empty).is_err());
    }

    // ── resolve_bindings ────────────────────────────────────────────

    #[tokio::test]
    async fn inline_binding_resolves_to_records() {
        let bindings = vec![b(json!({"var":"t","data":[{"a":1},{"a":2}]}))];
        let noop: AttachmentFetcher =
            Box::new(|_id| Box::pin(async { Err("no attach".to_string()) }));
        let r = resolve_bindings(&bindings, None, None, &noop)
            .await
            .unwrap();
        assert_eq!(r.inputs.get("t").unwrap().as_array().unwrap().len(), 2);
        assert!(r.loaded_columns.get("t").is_some());
    }

    #[tokio::test]
    async fn inline_binding_2d_array_normalizes_via_header_row() {
        let bindings = vec![b(json!({
            "var": "t",
            "data": [["a", "b"], [1, 2], [3, 4]],
        }))];
        let noop: AttachmentFetcher =
            Box::new(|_id| Box::pin(async { Err("no attach".to_string()) }));
        let r = resolve_bindings(&bindings, None, None, &noop)
            .await
            .unwrap();
        let records = r.inputs.get("t").unwrap().as_array().unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].get("a").unwrap(), 1);
        assert_eq!(records[0].get("b").unwrap(), 2);
    }

    #[tokio::test]
    async fn sql_binding_without_ctx_errors_source_not_enabled() {
        let bindings = vec![b(json!({"var":"t","query":"SELECT 1"}))];
        let noop: AttachmentFetcher = Box::new(|_id| Box::pin(async { Err("x".to_string()) }));
        let e = resolve_bindings(&bindings, None, None, &noop)
            .await
            .unwrap_err();
        assert_eq!(e["error"], "SourceNotEnabled");
        assert_eq!(e["source"], "sql");
        assert_eq!(e["binding"], "t");
    }

    #[tokio::test]
    async fn gsheets_binding_without_client_errors_source_not_enabled() {
        let bindings = vec![b(json!({"var":"t","spreadsheet_id":"1x","sheet":"Sheet1"}))];
        let noop: AttachmentFetcher = Box::new(|_id| Box::pin(async { Err("x".to_string()) }));
        let e = resolve_bindings(&bindings, None, None, &noop)
            .await
            .unwrap_err();
        assert_eq!(e["error"], "SourceNotEnabled");
        assert_eq!(e["source"], "gsheets");
        assert_eq!(e["binding"], "t");
    }

    #[tokio::test]
    async fn multiple_inline_bindings_resolve_concurrently() {
        let bindings = vec![
            b(json!({"var":"a","data":[{"x":1}]})),
            b(json!({"var":"b","data":[{"y":2},{"y":3}]})),
        ];
        let noop: AttachmentFetcher =
            Box::new(|_id| Box::pin(async { Err("no attach".to_string()) }));
        let r = resolve_bindings(&bindings, None, None, &noop)
            .await
            .unwrap();
        assert_eq!(r.inputs.get("a").unwrap().as_array().unwrap().len(), 1);
        assert_eq!(r.inputs.get("b").unwrap().as_array().unwrap().len(), 2);
        assert!(r.loaded_columns.get("_loaded_columns").is_none());
        // aggregate lives on `loaded_columns` itself as an object keyed by var,
        // plus the sandbox-visible `_loaded_columns` global inside `inputs`.
        assert!(r.inputs.get("_loaded_columns").is_some());
    }

    #[tokio::test]
    async fn attachment_binding_uses_fetcher_and_parses_csv() {
        let bindings = vec![b(json!({"var":"t","attachment_id":"doc1"}))];
        let fetch: AttachmentFetcher = Box::new(|id| {
            Box::pin(async move {
                assert_eq!(id, "doc1");
                Ok((b"a,b\n1,2\n3,4\n".to_vec(), "text/csv".to_string()))
            })
        });
        let r = resolve_bindings(&bindings, None, None, &fetch)
            .await
            .unwrap();
        let records = r.inputs.get("t").unwrap().as_array().unwrap();
        assert_eq!(records.len(), 2);
    }

    #[tokio::test]
    async fn attachment_binding_fetch_error_propagates_with_binding_var() {
        let bindings = vec![b(json!({"var":"t","attachment_id":"doc1"}))];
        let fetch: AttachmentFetcher =
            Box::new(|_id| Box::pin(async { Err("fetch failed".to_string()) }));
        let e = resolve_bindings(&bindings, None, None, &fetch)
            .await
            .unwrap_err();
        assert_eq!(e["binding"], "t");
    }
}
