//! Pure parsing/normalization for the `output_tables` SQL write-back sink
//! of `data_run_python`. The LLM's pandas code assigns a Python global
//! `output_tables = {"schema.table": <DataFrame-or-spec-dict>}`; after the
//! sandbox runs, that becomes a `serde_json::Value` (a JSON object). This
//! module turns that `Value` into typed [`TableWriteSpec`]s, validating
//! shape only — no I/O, no async, no SQL execution (later tasks own DB
//! validation + execution).
//!
//! See `docs/superpowers/specs/2026-07-01-data-run-python-design.md`.

use super::diff_writer::{diff_records, CellChange};
use crate::dag_engine::domain::sql_permissions::{SqlOperation, SqlPermissions};
use crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::sql_bulk_tools::{
    split_qualified_table, validate_table_against_allowlist,
};
use crate::gsheets::infrastructure::http_client::rectangle_to_records;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};

/// Maximum rows accepted in a single `output_tables` write.
const MAX_ROWS: usize = 100_000;

/// How a table write should be applied. Bare DataFrames (no spec dict)
/// default to [`WriteMode::Append`] — deliberately conservative, never
/// `Replace`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WriteMode {
    Append,
    Update,
    Upsert,
    Replace,
}

/// A single table write, parsed from one entry of `output_tables`.
#[derive(Debug, Clone, PartialEq)]
pub struct TableWriteSpec {
    pub table: String,
    pub mode: WriteMode,
    pub records: Vec<Map<String, Value>>,
    pub key: Option<Vec<String>>,
    pub columns: Option<Vec<String>>,
}

/// Parse the `output_tables` global (`{ "schema.table": df-or-spec }`)
/// into typed write specs.
///
/// - A bare JSON array value ⇒ `WriteMode::Append`, records taken from
///   the array (2-D array with header row, or array of objects).
/// - A JSON object value is a spec dict: `df` (array, required), `mode`
///   (default `"append"`), `key` (string or array of strings), `columns`
///   (array of strings).
///
/// On malformed input, returns a structured JSON error envelope
/// (`{"error": "<Kind>", ...}`) instead of panicking.
pub fn parse_output_tables(value: &Value) -> Result<Vec<TableWriteSpec>, Value> {
    let Some(obj) = value.as_object() else {
        return Err(json!({
            "error": "InvalidSpec",
            "message": "output_tables must be a JSON object mapping table name to a DataFrame or spec dict",
        }));
    };

    let mut specs = Vec::with_capacity(obj.len());
    for (table, entry) in obj {
        let spec = match entry {
            Value::Array(_) => TableWriteSpec {
                table: table.clone(),
                mode: WriteMode::Append,
                records: normalize_records(entry),
                key: None,
                columns: None,
            },
            Value::Object(spec_obj) => parse_spec_dict(table, spec_obj)?,
            _ => {
                return Err(json!({
                    "error": "InvalidSpec",
                    "table": table,
                    "message": "table value must be an array (DataFrame) or an object (spec dict)",
                }));
            }
        };
        specs.push(spec);
    }
    Ok(specs)
}

fn parse_spec_dict(table: &str, spec_obj: &Map<String, Value>) -> Result<TableWriteSpec, Value> {
    let df = match spec_obj.get("df") {
        Some(Value::Array(rows)) => {
            if rows.is_empty() {
                return Err(json!({
                    "error": "EmptyDataFrame",
                    "table": table,
                    "message": "df must contain at least one row",
                }));
            }
            Value::Array(rows.clone())
        }
        Some(_) => {
            return Err(json!({
                "error": "InvalidSpec",
                "table": table,
                "message": "df must be an array",
            }));
        }
        None => {
            return Err(json!({
                "error": "InvalidSpec",
                "table": table,
                "message": "spec dict is missing required 'df' field",
            }));
        }
    };

    let mode = match spec_obj.get("mode") {
        None => WriteMode::Append,
        Some(Value::String(s)) => match s.to_lowercase().as_str() {
            "append" => WriteMode::Append,
            "update" => WriteMode::Update,
            "upsert" => WriteMode::Upsert,
            "replace" => WriteMode::Replace,
            other => {
                return Err(json!({
                    "error": "InvalidMode",
                    "table": table,
                    "mode": other,
                    "message": "mode must be one of: append, update, upsert, replace",
                }));
            }
        },
        Some(_) => {
            return Err(json!({
                "error": "InvalidMode",
                "table": table,
                "message": "mode must be a string",
            }));
        }
    };

    let key = match spec_obj.get("key") {
        Some(Value::String(s)) => Some(vec![s.clone()]),
        Some(Value::Array(items)) => {
            let strs: Vec<String> = items
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect();
            if strs.is_empty() {
                None
            } else {
                Some(strs)
            }
        }
        _ => None,
    };

    let columns = match spec_obj.get("columns") {
        Some(Value::Array(items)) => {
            let strs: Vec<String> = items
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect();
            if strs.is_empty() {
                None
            } else {
                Some(strs)
            }
        }
        _ => None,
    };

    Ok(TableWriteSpec {
        table: table.to_string(),
        mode,
        records: normalize_records(&df),
        key,
        columns,
    })
}

/// Normalize a JSON array into records (`Vec<Map<String, Value>>`).
/// An array of objects is used as-is. A 2-D array whose first row is a
/// header row is converted via [`rectangle_to_records`].
fn normalize_records(value: &Value) -> Vec<Map<String, Value>> {
    let is_2d = matches!(
        value.as_array().and_then(|a| a.first()),
        Some(Value::Array(_))
    );
    let records_value = if is_2d {
        rectangle_to_records(value)
    } else {
        value.clone()
    };
    records_value
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_object().cloned()).collect())
        .unwrap_or_default()
}

/// Union of column names across all records, in first-seen order.
fn input_columns(records: &[Map<String, Value>]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut cols = Vec::new();
    for record in records {
        for key in record.keys() {
            if seen.insert(key.clone()) {
                cols.push(key.clone());
            }
        }
    }
    cols
}

/// Validate a [`TableWriteSpec`] against permissions, the schema allowlist,
/// and table shape — before any SQL is executed. Pure function: no I/O, no
/// async, no DB access. Returns a structured JSON error envelope on the
/// first failing check (see module doc / spec §10 for error codes).
pub fn validate_write_spec(
    spec: &TableWriteSpec,
    allowed_schemas: &[String],
    perms: &SqlPermissions,
    table_exists: bool,
    table_columns: Option<&[String]>,
) -> Result<(), Value> {
    // 1. Schema/identifier allowlist.
    if let Err(message) = validate_table_against_allowlist(&spec.table, allowed_schemas) {
        return Err(json!({
            "error": "SchemaNotAllowed",
            "table": spec.table,
            "message": message,
        }));
    }

    // 2. Operation permission for the requested mode.
    let required_ops: &[SqlOperation] = match spec.mode {
        WriteMode::Append => &[SqlOperation::Insert],
        WriteMode::Update => &[SqlOperation::Update],
        WriteMode::Upsert => &[SqlOperation::Insert, SqlOperation::Update],
        WriteMode::Replace => &[SqlOperation::Insert, SqlOperation::Delete],
    };
    if !required_ops.iter().all(|op| perms.is_allowed(op)) {
        return Err(json!({
            "error": "OperationNotPermitted",
            "table": spec.table,
            "mode": format!("{:?}", spec.mode).to_lowercase(),
            "message": "the configured SQL permissions do not allow this write mode",
        }));
    }

    // 3. Table existence / auto-create gate.
    if !table_exists {
        match spec.mode {
            WriteMode::Update => {
                return Err(json!({
                    "error": "TableNotFound",
                    "table": spec.table,
                    "message": "cannot UPDATE: table does not exist",
                }));
            }
            WriteMode::Append | WriteMode::Upsert | WriteMode::Replace => {
                if !perms.is_allowed(&SqlOperation::CreateTable) {
                    return Err(json!({
                        "error": "TableNotFound",
                        "table": spec.table,
                        "message": "table does not exist and permissions do not allow CREATE TABLE (auto-create)",
                    }));
                }
            }
        }
    }
    let auto_creating = !table_exists;

    let cols = input_columns(&spec.records);

    // 4. Key presence (must be declared, non-empty list) for Update/Upsert.
    if matches!(spec.mode, WriteMode::Update | WriteMode::Upsert) {
        match &spec.key {
            None => {
                return Err(json!({
                    "error": "KeyColumnMissing",
                    "table": spec.table,
                    "message": "mode requires a 'key' column list",
                }));
            }
            Some(key) if key.is_empty() => {
                return Err(json!({
                    "error": "KeyColumnMissing",
                    "table": spec.table,
                    "message": "'key' must contain at least one column",
                }));
            }
            Some(_) => {}
        }
    }

    // 5. Non-empty records for Update/Upsert.
    if matches!(spec.mode, WriteMode::Update | WriteMode::Upsert) && spec.records.is_empty() {
        return Err(json!({
            "error": "EmptyDataFrame",
            "table": spec.table,
            "message": "records must contain at least one row",
        }));
    }

    // 4b. Key columns must be present in both input records and table.
    if matches!(spec.mode, WriteMode::Update | WriteMode::Upsert) {
        if let Some(key) = &spec.key {
            for k in key {
                let in_input = cols.iter().any(|c| c == k);
                let in_table = table_columns.is_none_or(|tc| tc.iter().any(|c| c == k));
                if !in_input || !in_table {
                    return Err(json!({
                        "error": "KeyColumnMissing",
                        "table": spec.table,
                        "key_column": k,
                        "message": "key column must be present in both the input records and the table",
                    }));
                }
            }
        }
    }

    // 6. Duplicate key values within input records (Update/Upsert only).
    if matches!(spec.mode, WriteMode::Update | WriteMode::Upsert) {
        if let Some(key) = &spec.key {
            let mut seen_keys = HashSet::new();
            for record in &spec.records {
                let key_values: Vec<Value> = key
                    .iter()
                    .map(|k| record.get(k).cloned().unwrap_or(Value::Null))
                    .collect();
                let key_repr = serde_json::to_string(&key_values).unwrap_or_default();
                if !seen_keys.insert(key_repr) {
                    return Err(json!({
                        "error": "DuplicateKeyInInput",
                        "table": spec.table,
                        "message": "duplicate key value found across input records",
                    }));
                }
            }
        }
    }

    // 7. Column names non-empty. (Uniqueness needs no check here: `cols` comes
    // from `input_columns`, which dedupes, and per-record JSON object keys are
    // inherently unique — a duplicate branch would be unreachable dead code.)
    for c in &cols {
        if c.is_empty() {
            return Err(json!({
                "error": "InvalidColumnName",
                "table": spec.table,
                "message": "column name must not be empty",
            }));
        }
    }

    // 8. Column mismatch against existing table (skip when auto-creating).
    if table_exists && !auto_creating {
        if let Some(tc) = table_columns {
            let table_col_set: HashSet<&String> = tc.iter().collect();
            let df_only: Vec<&String> =
                cols.iter().filter(|c| !table_col_set.contains(c)).collect();
            if !df_only.is_empty() {
                return Err(json!({
                    "error": "ColumnMismatch",
                    "table": spec.table,
                    "df_only": df_only,
                    "table_columns": tc,
                    "message": "input records contain columns not present in the existing table",
                }));
            }
        }
    }

    // 9. Row count cap.
    if spec.records.len() > MAX_ROWS {
        return Err(json!({
            "error": "TooManyRows",
            "table": spec.table,
            "row_count": spec.records.len(),
            "max_rows": MAX_ROWS,
            "message": "too many rows in a single write",
        }));
    }

    Ok(())
}

/// Column SQL type inferred from scanning a column's non-null values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InferredType {
    BigInt,
    DoublePrecision,
    Boolean,
    TimestampTz,
    Text,
}

impl InferredType {
    fn as_sql(self) -> &'static str {
        match self {
            InferredType::BigInt => "BIGINT",
            InferredType::DoublePrecision => "DOUBLE PRECISION",
            InferredType::Boolean => "BOOLEAN",
            InferredType::TimestampTz => "TIMESTAMPTZ",
            InferredType::Text => "TEXT",
        }
    }
}

/// Infer the SQL type for a single column from its non-null values.
///
/// All-null (or no values at all) infers `TEXT`. Otherwise the column
/// must satisfy exactly one of the numeric/bool/timestamp categories
/// across every non-null value to get anything other than `TEXT`.
fn infer_column_type<'a>(values: impl Iterator<Item = &'a Value>) -> InferredType {
    let mut has_value = false;
    let mut all_int = true;
    let mut all_numeric = true;
    let mut has_float = false;
    let mut all_bool = true;
    let mut all_timestamp = true;

    for v in values {
        if v.is_null() {
            continue;
        }
        has_value = true;

        match v {
            Value::Number(n) => {
                all_bool = false;
                all_timestamp = false;
                if !(n.is_i64() || n.is_u64()) {
                    all_int = false;
                    has_float = true;
                }
            }
            Value::Bool(_) => {
                all_int = false;
                all_numeric = false;
                all_timestamp = false;
            }
            Value::String(s) => {
                all_int = false;
                all_numeric = false;
                all_bool = false;
                if chrono::DateTime::parse_from_rfc3339(s).is_err() {
                    all_timestamp = false;
                }
            }
            Value::Null => unreachable!("null filtered above"),
            Value::Object(_) | Value::Array(_) => {
                all_int = false;
                all_numeric = false;
                all_bool = false;
                all_timestamp = false;
            }
        }
    }

    if !has_value {
        return InferredType::Text;
    }
    if all_int && all_numeric {
        InferredType::BigInt
    } else if all_numeric && has_float {
        InferredType::DoublePrecision
    } else if all_bool {
        InferredType::Boolean
    } else if all_timestamp {
        InferredType::TimestampTz
    } else {
        InferredType::Text
    }
}

/// Quote a single SQL identifier with double quotes, escaping any
/// embedded `"` by doubling it per the SQL standard (e.g. `a"b` becomes
/// `"a""b"`). Prevents identifier break-out in generated DDL when a
/// column/table/schema name (LLM-produced) contains a quote character.
fn quote_ident(ident: &str) -> String {
    format!("\"{}\"", ident.replace('"', "\"\""))
}

/// Build a `CREATE TABLE IF NOT EXISTS` statement for an auto-created
/// `output_tables` sink target, inferring each column's SQL type from the
/// records' JSON values. Pure function: no I/O, no async, no SQL execution.
///
/// Column order follows first-seen order across `records` (see
/// [`input_columns`]). When `key` is `Some`, a trailing
/// `UNIQUE ("k1","k2")` clause is appended using those (quoted) columns.
pub fn infer_create_ddl(
    schema: &str,
    table: &str,
    records: &[Map<String, Value>],
    key: Option<&[String]>,
) -> String {
    let cols = input_columns(records);

    let mut col_defs: Vec<String> = cols
        .iter()
        .map(|col| {
            let values = records.iter().filter_map(|r| r.get(col));
            let ty = infer_column_type(values);
            format!("{} {}", quote_ident(col), ty.as_sql())
        })
        .collect();

    if let Some(key_cols) = key {
        let quoted: Vec<String> = key_cols.iter().map(|k| quote_ident(k)).collect();
        col_defs.push(format!("UNIQUE ({})", quoted.join(",")));
    }

    format!(
        "CREATE TABLE IF NOT EXISTS {}.{} ({})",
        quote_ident(schema),
        quote_ident(table),
        col_defs.join(", ")
    )
}

/// Build a parameterized `INSERT` statement for one chunk of rows against
/// `cols` (column order fixed across all rows). Missing keys in a record
/// bind SQL `NULL`. Pure function: no I/O, no async, no SQL execution —
/// values are returned as a flat, placeholder-ordered `Vec<Value>` for a
/// caller to bind later.
pub fn build_insert_sql(
    schema: &str,
    table: &str,
    cols: &[String],
    row_chunk: &[&Map<String, Value>],
) -> (String, Vec<Value>) {
    let quoted_cols: Vec<String> = cols.iter().map(|c| quote_ident(c)).collect();
    let mut params = Vec::with_capacity(cols.len() * row_chunk.len());
    let mut tuples = Vec::with_capacity(row_chunk.len());
    let mut n = 0usize;
    for row in row_chunk {
        let mut placeholders = Vec::with_capacity(cols.len());
        for col in cols {
            n += 1;
            placeholders.push(format!("${n}"));
            params.push(row.get(col).cloned().unwrap_or(Value::Null));
        }
        tuples.push(format!("({})", placeholders.join(",")));
    }

    let sql = format!(
        "INSERT INTO {}.{} ({}) VALUES {}",
        quote_ident(schema),
        quote_ident(table),
        quoted_cols.join(","),
        tuples.join(",")
    );
    (sql, params)
}

/// Build a parameterized `INSERT ... ON CONFLICT (key) DO UPDATE SET ...`
/// statement. Every column in `cols` that is not part of `key` gets an
/// `EXCLUDED`-sourced `DO UPDATE SET` clause. Pure function: no I/O, no
/// async, no SQL execution.
pub fn build_upsert_sql(
    schema: &str,
    table: &str,
    cols: &[String],
    key: &[String],
    row_chunk: &[&Map<String, Value>],
) -> (String, Vec<Value>) {
    let (insert_sql, params) = build_insert_sql(schema, table, cols, row_chunk);

    let quoted_key: Vec<String> = key.iter().map(|k| quote_ident(k)).collect();
    let set_clauses: Vec<String> = cols
        .iter()
        .filter(|c| !key.contains(c))
        .map(|c| {
            let q = quote_ident(c);
            format!("{q}=EXCLUDED.{q}")
        })
        .collect();

    let sql = format!(
        "{} ON CONFLICT ({}) DO UPDATE SET {}",
        insert_sql,
        quoted_key.join(","),
        set_clauses.join(",")
    );
    (sql, params)
}

/// Build parameterized `UPDATE` statements from a flat list of cell-level
/// [`CellChange`]s, grouped by `key_value`. Each group produces one
/// `UPDATE "schema"."table" SET "c1"=$1,... WHERE "key0"=$N` statement
/// (single-key path only — `key_value` binds last).
///
/// **Composite-key limitation:** `CellChange` carries a single opaque
/// `key_value` string, so when `key.len() > 1` there is no way to
/// reconstruct a correct multi-column `WHERE` clause from the changes
/// alone. In that case this function returns an empty `Vec` — callers
/// (the Task 7 executor) must detect this and fall back to a full-record
/// `UPDATE` built from the original records instead.
pub fn build_update_sql_from_changes(
    schema: &str,
    table: &str,
    key: &[String],
    changes: &[CellChange],
) -> Vec<(String, Vec<Value>)> {
    if key.len() != 1 {
        return Vec::new();
    }
    let key_col = quote_ident(&key[0]);

    let mut order: Vec<String> = Vec::new();
    let mut groups: std::collections::HashMap<String, Vec<&CellChange>> =
        std::collections::HashMap::new();
    for change in changes {
        if !groups.contains_key(&change.key_value) {
            order.push(change.key_value.clone());
        }
        groups
            .entry(change.key_value.clone())
            .or_default()
            .push(change);
    }

    order
        .into_iter()
        .map(|key_value| {
            let group = &groups[&key_value];
            let mut params = Vec::with_capacity(group.len() + 1);
            let mut set_clauses = Vec::with_capacity(group.len());
            for change in group.iter() {
                params.push(change.new_value.clone());
                set_clauses.push(format!("{}=${}", quote_ident(&change.column), params.len()));
            }
            params.push(Value::String(key_value));
            let sql = format!(
                "UPDATE {}.{} SET {} WHERE {}=${}",
                quote_ident(schema),
                quote_ident(table),
                set_clauses.join(","),
                key_col,
                params.len()
            );
            (sql, params)
        })
        .collect()
}

/// Upper bound on rows chunked into a single `INSERT`/`UPSERT` statement.
/// The actual chunk size is capped further by [`chunk_rows_for`] so that
/// `cols * chunk <= PG_MAX_BIND_PARAMS` — a wide table (many columns)
/// would otherwise blow past Postgres's 65535 bind-parameter limit
/// within a single `WRITE_CHUNK_SIZE`-row chunk (e.g. 70 cols * 1000
/// rows = 70,000 params > 65,535).
const WRITE_CHUNK_SIZE: usize = 1000;

/// Postgres's hard limit on bind parameters in a single prepared
/// statement.
const PG_MAX_BIND_PARAMS: usize = 65_535;

/// Compute the row-chunk size for a table with `num_cols` columns,
/// capping `cols * chunk_rows` under [`PG_MAX_BIND_PARAMS`] while never
/// exceeding [`WRITE_CHUNK_SIZE`] for normal (narrow) tables. Always
/// returns at least 1 (so a single row that itself has more columns
/// than the param budget still gets one — malformed, but not silently
/// dropped — chunk; such a table is already unrealistic).
fn chunk_rows_for(num_cols: usize) -> usize {
    let by_budget = PG_MAX_BIND_PARAMS / num_cols.max(1);
    WRITE_CHUNK_SIZE.min(by_budget.max(1))
}

/// Bind a single `serde_json::Value` onto a `sqlx::query::Query` builder,
/// mapping JSON scalar types to native Postgres types. `Array`/`Object`
/// values are bound as their JSON text representation (`::text` — callers
/// needing JSONB columns should cast in SQL, e.g. `$N::jsonb`).
fn bind_json_value<'q>(
    query: sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments>,
    value: &'q Value,
) -> sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments> {
    match value {
        Value::Null => query.bind(Option::<String>::None),
        Value::Bool(b) => query.bind(*b),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                query.bind(i)
            } else if let Some(f) = n.as_f64() {
                query.bind(f)
            } else {
                query.bind(n.to_string())
            }
        }
        Value::String(s) => query.bind(s.as_str()),
        Value::Array(_) | Value::Object(_) => query.bind(value.to_string()),
    }
}

/// Execute a parameterized statement built by [`build_insert_sql`] /
/// [`build_upsert_sql`] / [`build_update_sql_from_changes`] against the
/// open transaction, binding each `Value` in order.
async fn exec_bound(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    sql: &str,
    params: &[Value],
) -> Result<sqlx::postgres::PgQueryResult, sqlx::Error> {
    let mut query = sqlx::query(sql);
    for p in params {
        query = bind_json_value(query, p);
    }
    query.execute(&mut **tx).await
}

/// `true` only for SQLSTATE `42P10` (`invalid_column_reference` —
/// Postgres's code for "there is no unique or exclusion constraint
/// matching the ON CONFLICT specification"): the declared `key` is not
/// backed by any UNIQUE/PRIMARY KEY constraint on the target table at
/// all, so `ON CONFLICT (key)` has no arbiter to match.
///
/// Deliberately does **not** include `23505` (`unique_violation`): that
/// code also fires for a write-time collision on an *unrelated* unique
/// constraint (e.g. the table has `PRIMARY KEY (id)` and a separate
/// `UNIQUE (email)`, and an `ON CONFLICT (id)` upsert row collides on
/// `email`). Labeling that as "the ON CONFLICT key has no matching
/// UNIQUE constraint" would be misleading — the key IS backed by a
/// constraint; a different column collided. See
/// [`is_generic_constraint_violation`] for that case's mapping.
fn is_unique_violation(err: &sqlx::Error) -> bool {
    matches!(
        err,
        sqlx::Error::Database(db) if db.code().as_deref() == Some("42P10")
    )
}

/// `true` for SQLSTATE `23505` (`unique_violation`) — a genuine
/// constraint collision at write time that is unrelated to the
/// `ON CONFLICT` arbiter itself (see [`is_unique_violation`] doc for why
/// these two cases must not share an error code).
fn is_generic_constraint_violation(err: &sqlx::Error) -> bool {
    matches!(
        err,
        sqlx::Error::Database(db) if db.code().as_deref() == Some("23505")
    )
}

/// Introspect a table's columns via `information_schema.columns`. Returns
/// an empty `Vec` when the table does not exist (or has zero columns,
/// which is not a real-world case for user tables).
async fn introspect_columns(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    schema: &str,
    table: &str,
) -> Result<Vec<String>, sqlx::Error> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT column_name::text FROM information_schema.columns \
         WHERE table_schema = $1 AND table_name = $2 ORDER BY ordinal_position",
    )
    .bind(schema)
    .bind(table)
    .fetch_all(&mut **tx)
    .await?;
    Ok(rows.into_iter().map(|(c,)| c).collect())
}

/// [`build_update_sql_from_changes`] always binds `key_value` as a
/// `Value::String` (it's a `CellChange::key_value`, opaque and always
/// textual per `diff_writer::key_to_string`) in the *last* placeholder,
/// against a `WHERE "key_col"=$N` clause. When the key column's actual
/// SQL type isn't text (e.g. `BIGINT`), Postgres rejects the comparison
/// (`operator does not exist: bigint = text`) rather than coercing.
/// Rather than touching the shared pure builder (whose own unit tests
/// pin the exact `WHERE "id"=$N` SQL shape), cast the column side to
/// `text` at the call site — `"id"=$N` → `"id"::text=$N` — which is
/// type-safe regardless of the column's real type since `key_value`
/// came from that same column's value stringified.
fn cast_trailing_where_key_to_text(sql: &str) -> String {
    let Some(where_pos) = sql.rfind(" WHERE ") else {
        return sql.to_string();
    };
    let clause_start = where_pos + " WHERE ".len();
    let where_clause = &sql[clause_start..];
    let Some(eq_offset) = where_clause.find('=') else {
        return sql.to_string();
    };
    let (col, rest) = where_clause.split_at(eq_offset);
    format!("{}{col}::text{rest}", &sql[..clause_start])
}

/// Convert a JSON scalar to the same string representation
/// `diff_writer::key_to_string` uses for `CellChange::key_value`, so
/// composite-key fallback matching lines up exactly with the diff's
/// changed-row set. Returns `None` for null/array/object (dropped, same
/// as the diff).
fn scalar_key_to_string(v: &Value) -> Option<String> {
    match v {
        Value::Null => None,
        Value::Bool(b) => Some(b.to_string()),
        Value::Number(n) => Some(n.to_string()),
        Value::String(s) => Some(s.clone()),
        Value::Array(_) | Value::Object(_) => None,
    }
}

/// Build a per-row full-record `UPDATE` for every record, keyed by
/// (possibly composite) `key` columns. Used as the fallback path when a
/// cell-level diff is unavailable (no loaded snapshot) or infeasible
/// (composite key — [`build_update_sql_from_changes`] only supports a
/// single-column key).
///
/// When `cols` contains only key column(s) — i.e. there are no
/// non-key columns to `SET` — there is nothing to update for any
/// record, so no statements are built at all (an empty `SET` clause is
/// a Postgres syntax error). Callers must treat an empty result as "no
/// rows changed", not as a failure.
fn build_full_record_updates(
    schema: &str,
    table: &str,
    key: &[String],
    cols: &[String],
    records: &[Map<String, Value>],
) -> Vec<(String, Vec<Value>)> {
    let set_cols: Vec<&String> = cols.iter().filter(|c| !key.contains(c)).collect();
    if set_cols.is_empty() {
        return Vec::new();
    }
    records
        .iter()
        .map(|record| {
            let mut params = Vec::with_capacity(set_cols.len() + key.len());
            let mut set_clauses = Vec::with_capacity(set_cols.len());
            for col in &set_cols {
                params.push(record.get(*col).cloned().unwrap_or(Value::Null));
                set_clauses.push(format!("{}=${}", quote_ident(col), params.len()));
            }
            let mut where_clauses = Vec::with_capacity(key.len());
            for k in key {
                params.push(record.get(k).cloned().unwrap_or(Value::Null));
                where_clauses.push(format!("{}=${}", quote_ident(k), params.len()));
            }
            let sql = format!(
                "UPDATE {}.{} SET {} WHERE {}",
                quote_ident(schema),
                quote_ident(table),
                set_clauses.join(","),
                where_clauses.join(" AND ")
            );
            (sql, params)
        })
        .collect()
}

/// Transactional executor for the `output_tables` SQL write-back sink.
/// Applies every [`TableWriteSpec`] in `specs` sequentially, inside a
/// single Postgres transaction: any failure rolls the whole batch back
/// (no partial writes). See module doc / spec §10-12 for the write-mode
/// semantics.
///
/// `loaded_snapshots` — keyed by the spec's qualified table name (e.g.
/// `"drp_test.people"`, matching `TableWriteSpec::table` verbatim) —
/// supplies the "before" state for diff-driven `Update`. When absent for
/// a given table, `Update` falls back to an unconditional per-row
/// full-record `UPDATE` keyed by `spec.key`.
///
/// `on_missing_table` (`"create"` | `"fail"`) and `on_existing_table`
/// (`"fail"` | `"append"` | `"overwrite"`) are operator-set policies from
/// the tool's `fixed_config.sql`. `on_missing_table == "fail"` blocks the
/// auto-create path even when permissions allow `CREATE TABLE`; `Replace`
/// on an existing table with `on_existing_table == "fail"` is rejected
/// (`TableExists`) BEFORE any `DELETE`. Both strings are validated up
/// front — an unknown value is rejected loudly (no silent default).
///
/// Returns `{"wrote_tables": [...]}` on success, or a structured error
/// `Value` (as produced by [`validate_write_spec`] / [`diff_writer`]) on
/// the first failure — the transaction is rolled back before returning.
#[allow(clippy::too_many_arguments)]
pub async fn write_output_tables(
    pool: &sqlx::PgPool,
    specs: Vec<TableWriteSpec>,
    allowed_schemas: &[String],
    perms: &SqlPermissions,
    tenant_user_id: Option<&str>,
    loaded_snapshots: &HashMap<String, Vec<Map<String, Value>>>,
    on_missing_table: &str,
    on_existing_table: &str,
    statement_timeout_ms: u64,
    work_mem_mb: u64,
) -> Value {
    // Validate the operator-set policy strings loudly before touching the
    // DB — an unknown value must never silently degrade to a default.
    if !matches!(on_missing_table, "create" | "fail") {
        return json!({
            "error": "InvalidPolicy",
            "field": "on_missing_table",
            "value": on_missing_table,
            "message": "on_missing_table must be one of: create, fail",
        });
    }
    if !matches!(on_existing_table, "fail" | "append" | "overwrite") {
        return json!({
            "error": "InvalidPolicy",
            "field": "on_existing_table",
            "value": on_existing_table,
            "message": "on_existing_table must be one of: fail, append, overwrite",
        });
    }

    let mut tx = match pool.begin().await {
        Ok(tx) => tx,
        Err(e) => {
            return json!({
                "error": "TransactionError",
                "message": format!("failed to begin transaction: {e}"),
            });
        }
    };

    if let Err(e) = sqlx::query(&format!(
        "SET LOCAL statement_timeout = {statement_timeout_ms}"
    ))
    .execute(&mut *tx)
    .await
    {
        let _ = tx.rollback().await;
        return json!({
            "error": "TransactionError",
            "message": format!("failed to set statement_timeout: {e}"),
        });
    }
    if let Err(e) = sqlx::query(&format!("SET LOCAL work_mem = '{work_mem_mb}MB'"))
        .execute(&mut *tx)
        .await
    {
        let _ = tx.rollback().await;
        return json!({
            "error": "TransactionError",
            "message": format!("failed to set work_mem: {e}"),
        });
    }
    if let Some(uid) = tenant_user_id {
        if let Err(e) = sqlx::query("SELECT set_config('app.current_user_id', $1, true)")
            .bind(uid)
            .execute(&mut *tx)
            .await
        {
            let _ = tx.rollback().await;
            return json!({
                "error": "TransactionError",
                "message": format!("failed to set tenant context: {e}"),
            });
        }
    }

    let mut wrote_tables: Vec<Value> = Vec::with_capacity(specs.len());

    for spec in &specs {
        match write_one_table(
            &mut tx,
            spec,
            allowed_schemas,
            perms,
            loaded_snapshots,
            on_missing_table,
            on_existing_table,
        )
        .await
        {
            Ok(result) => wrote_tables.push(result),
            Err(mut err) => {
                let _ = tx.rollback().await;
                if err.get("table").is_none() {
                    if let Value::Object(ref mut obj) = err {
                        obj.insert("table".to_string(), Value::String(spec.table.clone()));
                    }
                }
                return err;
            }
        }
    }

    if let Err(e) = tx.commit().await {
        return json!({
            "error": "TransactionError",
            "message": format!("failed to commit: {e}"),
        });
    }

    json!({ "wrote_tables": wrote_tables })
}

/// Apply one [`TableWriteSpec`] within the already-open transaction.
/// Returns the per-table result object, or a structured error `Value` on
/// failure (caller rolls back).
#[allow(clippy::too_many_arguments)]
async fn write_one_table(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    spec: &TableWriteSpec,
    allowed_schemas: &[String],
    perms: &SqlPermissions,
    loaded_snapshots: &HashMap<String, Vec<Map<String, Value>>>,
    on_missing_table: &str,
    on_existing_table: &str,
) -> Result<Value, Value> {
    let (schema, table) = split_qualified_table(&spec.table);

    let table_columns = introspect_columns(tx, &schema, &table).await.map_err(|e| {
        json!({
            "error": "TransactionError",
            "table": spec.table,
            "message": format!("failed to introspect table columns: {e}"),
        })
    })?;
    let table_exists = !table_columns.is_empty();
    let table_columns_opt = if table_exists {
        Some(table_columns.as_slice())
    } else {
        None
    };

    validate_write_spec(
        spec,
        allowed_schemas,
        perms,
        table_exists,
        table_columns_opt,
    )?;

    // Operator policy gate (a): `on_missing_table == "fail"` blocks the
    // auto-create path even when the preset would allow CREATE TABLE.
    if !table_exists && on_missing_table == "fail" {
        return Err(json!({
            "error": "TableNotFound",
            "table": spec.table,
            "message": "table does not exist and on_missing_table policy is 'fail' \
                        (auto-create disabled by operator config)",
        }));
    }

    // Operator policy gate (b): a `Replace` (delete-then-insert) against an
    // EXISTING table with `on_existing_table == "fail"` is rejected BEFORE
    // any DELETE runs.
    if table_exists && spec.mode == WriteMode::Replace && on_existing_table == "fail" {
        return Err(json!({
            "error": "TableExists",
            "table": spec.table,
            "mode": "replace",
            "message": "table already exists and on_existing_table policy is 'fail'; \
                        refusing to replace (delete existing rows). Use a different \
                        table, or set on_existing_table to 'overwrite' to allow replace.",
        }));
    }

    let mut created = false;
    let mut created_ddl: Option<String> = None;
    if !table_exists
        && matches!(
            spec.mode,
            WriteMode::Append | WriteMode::Upsert | WriteMode::Replace
        )
    {
        let ddl = infer_create_ddl(&schema, &table, &spec.records, spec.key.as_deref());
        sqlx::query(&ddl).execute(&mut **tx).await.map_err(|e| {
            json!({
                "error": "TransactionError",
                "table": spec.table,
                "message": format!("failed to auto-create table: {e}"),
            })
        })?;
        created = true;
        created_ddl = Some(ddl);
    }

    let cols = input_columns(&spec.records);

    let mut result = json!({
        "table": spec.table,
        "mode": format!("{:?}", spec.mode).to_lowercase(),
        "created": created,
    });

    match spec.mode {
        WriteMode::Append => {
            let rows_affected = insert_chunked(tx, &schema, &table, &cols, &spec.records).await?;
            result["rows_affected"] = json!(rows_affected);
        }
        WriteMode::Upsert => {
            let key = spec.key.as_deref().unwrap_or(&[]);
            let rows_affected =
                upsert_chunked(tx, &schema, &table, &cols, key, &spec.records, &spec.table).await?;
            result["rows_affected"] = json!(rows_affected);
        }
        WriteMode::Replace => {
            let delete_sql = format!(
                "DELETE FROM {}.{}",
                quote_ident(&schema),
                quote_ident(&table)
            );
            sqlx::query(&delete_sql)
                .execute(&mut **tx)
                .await
                .map_err(|e| {
                    json!({
                        "error": "TransactionError",
                        "table": spec.table,
                        "message": format!("failed to delete existing rows for replace: {e}"),
                    })
                })?;
            let rows_affected = insert_chunked(tx, &schema, &table, &cols, &spec.records).await?;
            result["rows_affected"] = json!(rows_affected);
        }
        WriteMode::Update => {
            let key = spec.key.clone().unwrap_or_default();
            let (rows_affected, changes) =
                update_records(tx, &schema, &table, &key, &cols, spec, loaded_snapshots).await?;
            result["rows_affected"] = json!(rows_affected);
            if let Some(changes) = changes {
                result["changes"] = changes;
            }
        }
    }

    if let Some(ddl) = created_ddl {
        result["created_ddl"] = json!(ddl);
    }

    Ok(result)
}

/// Insert `records` in chunks of up to [`WRITE_CHUNK_SIZE`] rows (fewer
/// for wide tables — see [`chunk_rows_for`]), skipping any empty chunk
/// (defensive — an empty chunk would build a malformed `VALUES ()`
/// clause).
async fn insert_chunked(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    schema: &str,
    table: &str,
    cols: &[String],
    records: &[Map<String, Value>],
) -> Result<u64, Value> {
    let mut rows_affected = 0u64;
    for chunk in records.chunks(chunk_rows_for(cols.len())) {
        if chunk.is_empty() {
            continue;
        }
        let row_refs: Vec<&Map<String, Value>> = chunk.iter().collect();
        let (sql, params) = build_insert_sql(schema, table, cols, &row_refs);
        let res = exec_bound(tx, &sql, &params).await.map_err(|e| {
            json!({
                "error": "TransactionError",
                "message": format!("insert failed: {e}"),
            })
        })?;
        rows_affected += res.rows_affected();
    }
    Ok(rows_affected)
}

/// Upsert `records` in chunks of up to [`WRITE_CHUNK_SIZE`] rows (fewer
/// for wide tables — see [`chunk_rows_for`]), skipping any empty chunk.
/// Translates a Postgres unique-violation on the `ON CONFLICT` arbiter
/// into a structured `UpsertKeyNotUnique` error (the declared `key` is
/// not backed by a unique/PK constraint on the target table), and any
/// other unique-constraint collision into `ConstraintViolation`.
async fn upsert_chunked(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    schema: &str,
    table: &str,
    cols: &[String],
    key: &[String],
    records: &[Map<String, Value>],
    table_label: &str,
) -> Result<u64, Value> {
    let mut rows_affected = 0u64;
    for chunk in records.chunks(chunk_rows_for(cols.len())) {
        if chunk.is_empty() {
            continue;
        }
        let row_refs: Vec<&Map<String, Value>> = chunk.iter().collect();
        let (sql, params) = build_upsert_sql(schema, table, cols, key, &row_refs);
        let res = exec_bound(tx, &sql, &params).await.map_err(|e| {
            if is_unique_violation(&e) {
                json!({
                    "error": "UpsertKeyNotUnique",
                    "table": table_label,
                    "key": key,
                    "message": format!(
                        "upsert failed: key {key:?} has no matching UNIQUE/PRIMARY KEY \
                         constraint on the target table (ON CONFLICT arbiter not found): {e}"
                    ),
                })
            } else if is_generic_constraint_violation(&e) {
                json!({
                    "error": "ConstraintViolation",
                    "table": table_label,
                    "detail": e.to_string(),
                    "message": format!(
                        "upsert failed: a unique constraint (unrelated to the ON CONFLICT \
                         key {key:?}) was violated: {e}"
                    ),
                })
            } else {
                json!({
                    "error": "TransactionError",
                    "message": format!("upsert failed: {e}"),
                })
            }
        })?;
        rows_affected += res.rows_affected();
    }
    Ok(rows_affected)
}

/// Dispatch `Update`: diff-driven (against `loaded_snapshots`) when a
/// snapshot is available for this table, else an unconditional per-row
/// full-record `UPDATE` keyed by `key`. Returns `(rows_affected,
/// Some(changes_summary))` for the diff-driven path (`changes_summary`
/// is `{"rows": N, "cells": M}`), or `(rows_affected, None)` for the
/// full-record fallback.
async fn update_records(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    schema: &str,
    table: &str,
    key: &[String],
    cols: &[String],
    spec: &TableWriteSpec,
    loaded_snapshots: &HashMap<String, Vec<Map<String, Value>>>,
) -> Result<(u64, Option<Value>), Value> {
    if let Some(snapshot) = loaded_snapshots.get(&spec.table) {
        let key0 = key.first().ok_or_else(|| {
            json!({
                "error": "KeyColumnMissing",
                "table": spec.table,
                "message": "'key' must contain at least one column",
            })
        })?;
        let diff = diff_records(
            snapshot,
            &spec.records,
            key0,
            spec.columns.as_deref(),
            false,
            &spec.table,
        )
        .map_err(|e| {
            let mut v = e.to_json();
            if let Value::Object(ref mut obj) = v {
                obj.insert("table".to_string(), Value::String(spec.table.clone()));
            }
            v
        })?;

        if diff.changes.is_empty() {
            return Ok((0, Some(json!({"rows": 0, "cells": 0}))));
        }

        let statements = if key.len() == 1 {
            build_update_sql_from_changes(schema, table, key, &diff.changes)
                .into_iter()
                .map(|(sql, params)| (cast_trailing_where_key_to_text(&sql), params))
                .collect()
        } else {
            // Composite key: `CellChange::key_value` is a single opaque
            // string, so cell-level changes can't be replayed via a
            // multi-column WHERE clause — fall back to full-record
            // UPDATE for every row that had at least one change.
            // diff_records() only reports a change for rows whose key
            // matched an existing snapshot row, so `key.first()` here
            // (the diff's own key column) is safe to re-derive from.
            let changed_key_values: HashSet<&String> =
                diff.changes.iter().map(|c| &c.key_value).collect();
            let changed_records: Vec<Map<String, Value>> = spec
                .records
                .iter()
                .filter(|r| {
                    r.get(key0)
                        .and_then(scalar_key_to_string)
                        .is_some_and(|s| changed_key_values.contains(&s))
                })
                .cloned()
                .collect();
            build_full_record_updates(schema, table, key, cols, &changed_records)
        };

        let mut rows_touched = 0u64;
        for (sql, params) in &statements {
            let res = exec_bound(tx, sql, params).await.map_err(|e| {
                json!({
                    "error": "TransactionError",
                    "message": format!("update failed: {e}"),
                })
            })?;
            rows_touched += res.rows_affected();
        }
        let changes = json!({"rows": diff.rows_changed, "cells": diff.changes.len()});
        Ok((rows_touched, Some(changes)))
    } else {
        let statements = build_full_record_updates(schema, table, key, cols, &spec.records);
        let mut rows_affected = 0u64;
        for (sql, params) in &statements {
            let res = exec_bound(tx, sql, params).await.map_err(|e| {
                json!({
                    "error": "TransactionError",
                    "message": format!("update failed: {e}"),
                })
            })?;
            rows_affected += res.rows_affected();
        }
        Ok((rows_affected, None))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn bare_dataframe_is_append() {
        let v = json!({"analytics.t": [{"a":1}]});
        let specs = parse_output_tables(&v).unwrap();
        assert_eq!(specs[0].mode, WriteMode::Append);
        assert_eq!(specs[0].table, "analytics.t");
        assert_eq!(specs[0].records.len(), 1);
    }

    #[test]
    fn spec_dict_reads_mode_and_key() {
        let v = json!({"t": {"mode":"upsert","df":[{"id":1,"x":2}],"key":"id"}});
        let s = &parse_output_tables(&v).unwrap()[0];
        assert_eq!(s.mode, WriteMode::Upsert);
        assert_eq!(s.key.as_ref().unwrap(), &vec!["id".to_string()]);
    }

    #[test]
    fn invalid_mode_errors() {
        let v = json!({"t": {"mode":"delete","df":[]}});
        assert!(parse_output_tables(&v).is_err());
    }

    #[test]
    fn bare_2d_array_converted_to_records() {
        let v = json!({"t": [["a","b"],[1,2],[3,4]]});
        let specs = parse_output_tables(&v).unwrap();
        assert_eq!(specs[0].mode, WriteMode::Append);
        assert_eq!(specs[0].records.len(), 2);
        assert_eq!(specs[0].records[0].get("a").unwrap(), &json!(1));
        assert_eq!(specs[0].records[0].get("b").unwrap(), &json!(2));
    }

    #[test]
    fn missing_df_errors() {
        let v = json!({"t": {"mode":"append"}});
        let err = parse_output_tables(&v).unwrap_err();
        assert_eq!(err["error"], "InvalidSpec");
    }

    #[test]
    fn empty_df_errors() {
        let v = json!({"t": {"df": []}});
        let err = parse_output_tables(&v).unwrap_err();
        assert_eq!(err["error"], "EmptyDataFrame");
    }

    #[test]
    fn key_as_array_of_strings() {
        let v = json!({"t": {"df":[{"id":1,"x":2}],"key":["id","x"]}});
        let s = &parse_output_tables(&v).unwrap()[0];
        assert_eq!(
            s.key.as_ref().unwrap(),
            &vec!["id".to_string(), "x".to_string()]
        );
    }

    #[test]
    fn default_mode_is_append_for_spec_dict() {
        let v = json!({"t": {"df":[{"id":1}]}});
        let s = &parse_output_tables(&v).unwrap()[0];
        assert_eq!(s.mode, WriteMode::Append);
    }

    #[test]
    fn columns_optional_field_parsed() {
        let v = json!({"t": {"df":[{"id":1}],"columns":["id","name"]}});
        let s = &parse_output_tables(&v).unwrap()[0];
        assert_eq!(
            s.columns.as_ref().unwrap(),
            &vec!["id".to_string(), "name".to_string()]
        );
    }

    #[test]
    fn non_object_top_level_errors() {
        let v = json!([1, 2, 3]);
        assert!(parse_output_tables(&v).is_err());
    }

    // --- validate_write_spec ---

    use crate::dag_engine::domain::sql_permissions::SqlPermissions;

    fn perms(preset: &str, schemas: &[&str]) -> SqlPermissions {
        SqlPermissions::from_config(Some(&json!({
            "preset": preset,
            "allowed_schemas": schemas,
        })))
        .unwrap()
    }

    #[test]
    fn update_requires_key() {
        let spec = TableWriteSpec {
            table: "a.t".into(),
            mode: WriteMode::Update,
            records: vec![],
            key: None,
            columns: None,
        };
        let p = perms("read_write", &["a"]);
        let err =
            validate_write_spec(&spec, &["a".into()], &p, true, Some(&["id".into()])).unwrap_err();
        assert_eq!(err["error"], "KeyColumnMissing");
    }

    #[test]
    fn append_needs_insert_permission() {
        let mut record = Map::new();
        record.insert("id".into(), json!(1));
        let spec = TableWriteSpec {
            table: "a.t".into(),
            mode: WriteMode::Append,
            records: vec![record],
            key: None,
            columns: None,
        };
        let ro = perms("read_only", &["a"]);
        let err =
            validate_write_spec(&spec, &["a".into()], &ro, true, Some(&["id".into()])).unwrap_err();
        assert_eq!(err["error"], "OperationNotPermitted");
    }

    #[test]
    fn schema_outside_allowlist_rejected() {
        let mut record = Map::new();
        record.insert("id".into(), json!(1));
        let spec = TableWriteSpec {
            table: "secret.t".into(),
            mode: WriteMode::Append,
            records: vec![record],
            key: None,
            columns: None,
        };
        let p = perms("read_write", &["a"]);
        let err = validate_write_spec(&spec, &["a".into()], &p, true, Some(&[])).unwrap_err();
        assert_eq!(err["error"], "SchemaNotAllowed");
    }

    #[test]
    fn update_on_missing_table_is_table_not_found() {
        let mut record = Map::new();
        record.insert("id".into(), json!(1));
        let spec = TableWriteSpec {
            table: "a.t".into(),
            mode: WriteMode::Update,
            records: vec![record],
            key: Some(vec!["id".into()]),
            columns: None,
        };
        let p = perms("read_write", &["a"]);
        let err = validate_write_spec(&spec, &["a".into()], &p, false, None).unwrap_err();
        assert_eq!(err["error"], "TableNotFound");
    }

    #[test]
    fn append_on_missing_table_without_create_table_perm_is_table_not_found() {
        let mut record = Map::new();
        record.insert("id".into(), json!(1));
        let spec = TableWriteSpec {
            table: "a.t".into(),
            mode: WriteMode::Append,
            records: vec![record],
            key: None,
            columns: None,
        };
        let p = perms("read_write", &["a"]);
        let err = validate_write_spec(&spec, &["a".into()], &p, false, None).unwrap_err();
        assert_eq!(err["error"], "TableNotFound");
    }

    #[test]
    fn append_on_missing_table_with_create_table_perm_is_ok() {
        let mut record = Map::new();
        record.insert("id".into(), json!(1));
        let spec = TableWriteSpec {
            table: "a.t".into(),
            mode: WriteMode::Append,
            records: vec![record],
            key: None,
            columns: None,
        };
        let p = perms("full", &["a"]);
        let ok = validate_write_spec(&spec, &["a".into()], &p, false, None);
        assert!(ok.is_ok());
    }

    #[test]
    fn upsert_requires_non_empty_records() {
        let spec = TableWriteSpec {
            table: "a.t".into(),
            mode: WriteMode::Upsert,
            records: vec![],
            key: Some(vec!["id".into()]),
            columns: None,
        };
        let p = perms("read_write", &["a"]);
        let err =
            validate_write_spec(&spec, &["a".into()], &p, true, Some(&["id".into()])).unwrap_err();
        assert_eq!(err["error"], "EmptyDataFrame");
    }

    #[test]
    fn duplicate_key_in_input_rejected() {
        let mut r1 = Map::new();
        r1.insert("id".into(), json!(1));
        let mut r2 = Map::new();
        r2.insert("id".into(), json!(1));
        let spec = TableWriteSpec {
            table: "a.t".into(),
            mode: WriteMode::Upsert,
            records: vec![r1, r2],
            key: Some(vec!["id".into()]),
            columns: None,
        };
        let p = perms("read_write", &["a"]);
        let err =
            validate_write_spec(&spec, &["a".into()], &p, true, Some(&["id".into()])).unwrap_err();
        assert_eq!(err["error"], "DuplicateKeyInInput");
    }

    #[test]
    fn column_mismatch_reports_df_only() {
        let mut record = Map::new();
        record.insert("id".into(), json!(1));
        record.insert("extra".into(), json!("x"));
        let spec = TableWriteSpec {
            table: "a.t".into(),
            mode: WriteMode::Append,
            records: vec![record],
            key: None,
            columns: None,
        };
        let p = perms("read_write", &["a"]);
        let err =
            validate_write_spec(&spec, &["a".into()], &p, true, Some(&["id".into()])).unwrap_err();
        assert_eq!(err["error"], "ColumnMismatch");
        assert_eq!(err["df_only"], json!(["extra"]));
    }

    #[test]
    fn too_many_rows_rejected() {
        let mut record = Map::new();
        record.insert("id".into(), json!(1));
        let records: Vec<Map<String, Value>> = (0..100_001).map(|_| record.clone()).collect();
        let spec = TableWriteSpec {
            table: "a.t".into(),
            mode: WriteMode::Append,
            records,
            key: None,
            columns: None,
        };
        let p = perms("read_write", &["a"]);
        let err =
            validate_write_spec(&spec, &["a".into()], &p, true, Some(&["id".into()])).unwrap_err();
        assert_eq!(err["error"], "TooManyRows");
    }

    #[test]
    fn valid_append_passes() {
        let mut record = Map::new();
        record.insert("id".into(), json!(1));
        let spec = TableWriteSpec {
            table: "a.t".into(),
            mode: WriteMode::Append,
            records: vec![record],
            key: None,
            columns: None,
        };
        let p = perms("read_write", &["a"]);
        let ok = validate_write_spec(&spec, &["a".into()], &p, true, Some(&["id".into()]));
        assert!(ok.is_ok());
    }

    // --- infer_create_ddl ---

    #[test]
    fn infers_types_and_unique_key() {
        let recs = vec![serde_json::from_value(
            serde_json::json!({"id":1,"price":9.5,"active":true,"name":"a"}),
        )
        .unwrap()];
        let ddl = infer_create_ddl("analytics", "t", &recs, Some(&["id".to_string()]));
        assert!(ddl.contains("\"id\" BIGINT"));
        assert!(ddl.contains("\"price\" DOUBLE PRECISION"));
        assert!(ddl.contains("\"active\" BOOLEAN"));
        assert!(ddl.contains("\"name\" TEXT"));
        assert!(ddl.contains("UNIQUE (\"id\")"));
        assert!(ddl.starts_with("CREATE TABLE IF NOT EXISTS \"analytics\".\"t\""));
    }

    #[test]
    fn all_null_column_is_text() {
        let recs = vec![
            serde_json::from_value(serde_json::json!({"a": null})).unwrap(),
            serde_json::from_value(serde_json::json!({"a": null})).unwrap(),
        ];
        let ddl = infer_create_ddl("s", "t", &recs, None);
        assert!(ddl.contains("\"a\" TEXT"));
    }

    #[test]
    fn iso8601_string_column_is_timestamptz() {
        let recs = vec![
            serde_json::from_value(serde_json::json!({"ts": "2026-07-01T12:00:00Z"})).unwrap(),
        ];
        let ddl = infer_create_ddl("s", "t", &recs, None);
        assert!(ddl.contains("\"ts\" TIMESTAMPTZ"));
    }

    #[test]
    fn composite_key_produces_multi_column_unique() {
        let recs = vec![serde_json::from_value(serde_json::json!({"a":1,"b":2})).unwrap()];
        let ddl = infer_create_ddl("s", "t", &recs, Some(&["a".to_string(), "b".to_string()]));
        assert!(ddl.contains("UNIQUE (\"a\",\"b\")"));
    }

    #[test]
    fn no_key_means_no_unique_clause() {
        let recs = vec![serde_json::from_value(serde_json::json!({"a":1})).unwrap()];
        let ddl = infer_create_ddl("s", "t", &recs, None);
        assert!(!ddl.contains("UNIQUE"));
    }

    #[test]
    fn mixed_int_and_float_column_is_double_precision() {
        let recs = vec![
            serde_json::from_value(serde_json::json!({"n": 1})).unwrap(),
            serde_json::from_value(serde_json::json!({"n": 2.5})).unwrap(),
        ];
        let ddl = infer_create_ddl("s", "t", &recs, None);
        assert!(ddl.contains("\"n\" DOUBLE PRECISION"));
    }

    #[test]
    fn ddl_escapes_embedded_quotes_in_identifiers() {
        let recs = vec![serde_json::from_value(serde_json::json!({"a\"b": 1})).unwrap()];
        let ddl = infer_create_ddl("s", "t", &recs, None);
        assert!(ddl.contains("\"a\"\"b\" BIGINT"), "got: {ddl}");
    }

    // --- SQL statement builders ---

    #[test]
    fn insert_uses_placeholders() {
        let mut r = serde_json::Map::new();
        r.insert("id".into(), serde_json::json!(1));
        r.insert("x".into(), serde_json::json!("a"));
        let (sql, params) = build_insert_sql("s", "t", &["id".into(), "x".into()], &[&r]);
        assert!(sql.starts_with("INSERT INTO \"s\".\"t\" (\"id\",\"x\") VALUES ($1,$2)"));
        assert_eq!(params.len(), 2);
    }

    #[test]
    fn upsert_has_on_conflict() {
        let mut r = serde_json::Map::new();
        r.insert("id".into(), serde_json::json!(1));
        r.insert("x".into(), serde_json::json!(2));
        let (sql, _) =
            build_upsert_sql("s", "t", &["id".into(), "x".into()], &["id".into()], &[&r]);
        assert!(sql.contains("ON CONFLICT (\"id\") DO UPDATE SET \"x\"=EXCLUDED.\"x\""));
    }

    #[test]
    fn insert_multi_row_numbers_placeholders_row_major() {
        let mut r1 = serde_json::Map::new();
        r1.insert("id".into(), serde_json::json!(1));
        r1.insert("x".into(), serde_json::json!("a"));
        let mut r2 = serde_json::Map::new();
        r2.insert("id".into(), serde_json::json!(2));
        r2.insert("x".into(), serde_json::json!("b"));
        let (sql, params) = build_insert_sql("s", "t", &["id".into(), "x".into()], &[&r1, &r2]);
        assert!(sql.contains("VALUES ($1,$2),($3,$4)"), "got: {sql}");
        assert_eq!(
            params,
            vec![
                serde_json::json!(1),
                serde_json::json!("a"),
                serde_json::json!(2),
                serde_json::json!("b"),
            ]
        );
    }

    #[test]
    fn insert_binds_null_for_missing_key() {
        let mut r = serde_json::Map::new();
        r.insert("id".into(), serde_json::json!(1));
        let (_, params) = build_insert_sql("s", "t", &["id".into(), "x".into()], &[&r]);
        assert_eq!(params, vec![serde_json::json!(1), serde_json::Value::Null]);
    }

    #[test]
    fn update_from_changes_groups_by_key_value_and_binds_key_last() {
        let changes = vec![
            CellChange {
                key_value: "1".into(),
                column: "name".into(),
                old_value: serde_json::json!("old"),
                new_value: serde_json::json!("new"),
            },
            CellChange {
                key_value: "1".into(),
                column: "age".into(),
                old_value: serde_json::json!(20),
                new_value: serde_json::json!(21),
            },
            CellChange {
                key_value: "2".into(),
                column: "name".into(),
                old_value: serde_json::json!("foo"),
                new_value: serde_json::json!("bar"),
            },
        ];
        let statements = build_update_sql_from_changes("s", "t", &["id".into()], &changes);
        assert_eq!(statements.len(), 2);

        let (sql1, params1) = &statements[0];
        assert!(sql1.starts_with("UPDATE \"s\".\"t\" SET"));
        assert!(sql1.contains("WHERE \"id\"=$"));
        assert_eq!(params1.last().unwrap(), &serde_json::json!("1"));

        let (sql2, params2) = &statements[1];
        assert!(sql2.contains("\"name\"=$1"));
        assert_eq!(params2.last().unwrap(), &serde_json::json!("2"));
    }

    #[test]
    fn update_from_changes_composite_key_returns_empty() {
        let changes = vec![CellChange {
            key_value: "1".into(),
            column: "name".into(),
            old_value: serde_json::json!("old"),
            new_value: serde_json::json!("new"),
        }];
        let statements =
            build_update_sql_from_changes("s", "t", &["id".into(), "tenant".into()], &changes);
        assert!(statements.is_empty());
    }

    // --- Fix 2: build_full_record_updates with key-only columns ---

    #[test]
    fn full_record_updates_skips_records_when_only_key_columns_present() {
        // Only the key column ("id") is present in `cols` — there are no
        // non-key columns to SET, so no UPDATE statements must be built
        // (an empty SET clause is a Postgres syntax error).
        let mut r = Map::new();
        r.insert("id".into(), json!(1));
        let statements =
            build_full_record_updates("s", "t", &["id".to_string()], &["id".to_string()], &[r]);
        assert!(
            statements.is_empty(),
            "expected no UPDATE statements when there are no non-key columns to set, got: {statements:?}"
        );
    }

    #[test]
    fn full_record_updates_still_builds_statements_when_non_key_cols_present() {
        let mut r = Map::new();
        r.insert("id".into(), json!(1));
        r.insert("name".into(), json!("Ana"));
        let statements = build_full_record_updates(
            "s",
            "t",
            &["id".to_string()],
            &["id".to_string(), "name".to_string()],
            &[r],
        );
        assert_eq!(statements.len(), 1);
        assert!(
            statements[0].0.contains("SET \"name\"=$1"),
            "got: {}",
            statements[0].0
        );
        assert!(
            !statements[0].0.contains("SET  WHERE"),
            "malformed empty SET: {}",
            statements[0].0
        );
    }

    // --- Fix 3: chunk_rows_for bind-parameter ceiling ---

    #[test]
    fn chunk_rows_for_narrow_table_uses_full_write_chunk_size() {
        assert_eq!(chunk_rows_for(5), WRITE_CHUNK_SIZE);
    }

    #[test]
    fn chunk_rows_for_wide_table_caps_under_pg_bind_param_limit() {
        // 70 columns * 1000 rows = 70,000 > 65,535 — must be capped.
        let cols = 70;
        let rows = chunk_rows_for(cols);
        assert!(
            rows < WRITE_CHUNK_SIZE,
            "expected a reduced chunk size, got {rows}"
        );
        assert!(
            cols * rows <= PG_MAX_BIND_PARAMS,
            "cols*rows={} must stay under the {} bind-parameter limit",
            cols * rows,
            PG_MAX_BIND_PARAMS
        );
    }

    #[test]
    fn chunk_rows_for_never_returns_zero() {
        assert!(chunk_rows_for(0) >= 1);
        assert!(chunk_rows_for(1_000_000) >= 1);
    }

    // --- write_output_tables (integration, real Postgres) ---

    fn full_perms(schemas: &[&str]) -> SqlPermissions {
        SqlPermissions::from_config(Some(&json!({
            "preset": "full",
            "allowed_schemas": schemas,
        })))
        .unwrap()
    }

    async fn test_pool() -> sqlx::PgPool {
        let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL must be set");
        sqlx::postgres::PgPoolOptions::new()
            .connect(&url)
            .await
            .unwrap()
    }

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL — run with `cargo test -- --ignored`"]
    async fn append_autocreates_and_inserts() {
        let pool = test_pool().await;
        sqlx::query("DROP TABLE IF EXISTS drp_test.people")
            .execute(&pool)
            .await
            .ok();
        sqlx::query("CREATE SCHEMA IF NOT EXISTS drp_test")
            .execute(&pool)
            .await
            .ok();
        let specs = parse_output_tables(&json!({
            "drp_test.people": [{"id":1,"name":"Ana"},{"id":2,"name":"Bo"}]
        }))
        .unwrap();
        let perms = full_perms(&["drp_test"]);
        let out = write_output_tables(
            &pool,
            specs,
            &["drp_test".into()],
            &perms,
            None,
            &Default::default(),
            "create",
            "overwrite",
            30000,
            64,
        )
        .await;
        assert_eq!(out["wrote_tables"][0]["created"], true, "got: {out}");
        assert_eq!(out["wrote_tables"][0]["rows_affected"], 2, "got: {out}");
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM drp_test.people")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 2);
    }

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL — run with `cargo test -- --ignored`"]
    async fn custom_runtime_limits_are_applied_to_write() {
        // Non-default statement_timeout/work_mem must produce valid SQL that
        // Postgres accepts — proves the operator's runtime_limits are threaded
        // into the write transaction rather than hardcoded to 30000ms/64MB.
        let pool = test_pool().await;
        sqlx::query("DROP TABLE IF EXISTS drp_test.rt_limits")
            .execute(&pool)
            .await
            .ok();
        sqlx::query("CREATE SCHEMA IF NOT EXISTS drp_test")
            .execute(&pool)
            .await
            .ok();
        let specs = parse_output_tables(&json!({
            "drp_test.rt_limits": [{"id":1,"v":"a"}]
        }))
        .unwrap();
        let perms = full_perms(&["drp_test"]);
        let out = write_output_tables(
            &pool,
            specs,
            &["drp_test".into()],
            &perms,
            None,
            &Default::default(),
            "create",
            "overwrite",
            60000, // non-default statement_timeout_ms
            128,   // non-default work_mem_mb
        )
        .await;
        assert!(out.get("error").is_none(), "got: {out}");
        assert_eq!(out["wrote_tables"][0]["rows_affected"], 1, "got: {out}");
    }

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL — run with `cargo test -- --ignored`"]
    async fn append_into_existing_table_does_not_recreate() {
        let pool = test_pool().await;
        sqlx::query("DROP TABLE IF EXISTS drp_test.append2")
            .execute(&pool)
            .await
            .ok();
        sqlx::query("CREATE SCHEMA IF NOT EXISTS drp_test")
            .execute(&pool)
            .await
            .ok();
        sqlx::query("CREATE TABLE drp_test.append2 (id BIGINT, name TEXT)")
            .execute(&pool)
            .await
            .unwrap();
        let specs = parse_output_tables(&json!({
            "drp_test.append2": [{"id":1,"name":"Ana"}]
        }))
        .unwrap();
        let perms = full_perms(&["drp_test"]);
        let out = write_output_tables(
            &pool,
            specs,
            &["drp_test".into()],
            &perms,
            None,
            &Default::default(),
            "create",
            "overwrite",
            30000,
            64,
        )
        .await;
        assert_eq!(out["wrote_tables"][0]["created"], false, "got: {out}");
        assert_eq!(out["wrote_tables"][0]["rows_affected"], 1, "got: {out}");
    }

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL — run with `cargo test -- --ignored`"]
    async fn upsert_updates_existing_and_inserts_new() {
        let pool = test_pool().await;
        sqlx::query("DROP TABLE IF EXISTS drp_test.upsert_t")
            .execute(&pool)
            .await
            .ok();
        sqlx::query("CREATE SCHEMA IF NOT EXISTS drp_test")
            .execute(&pool)
            .await
            .ok();
        sqlx::query("CREATE TABLE drp_test.upsert_t (id BIGINT PRIMARY KEY, name TEXT)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO drp_test.upsert_t (id, name) VALUES (1, 'Old')")
            .execute(&pool)
            .await
            .unwrap();

        let specs = parse_output_tables(&json!({
            "drp_test.upsert_t": {"mode":"upsert","df":[{"id":1,"name":"New"},{"id":2,"name":"Bo"}],"key":"id"}
        }))
        .unwrap();
        let perms = full_perms(&["drp_test"]);
        let out = write_output_tables(
            &pool,
            specs,
            &["drp_test".into()],
            &perms,
            None,
            &Default::default(),
            "create",
            "overwrite",
            30000,
            64,
        )
        .await;
        assert_eq!(out["wrote_tables"][0]["rows_affected"], 2, "got: {out}");

        let name: String = sqlx::query_scalar("SELECT name FROM drp_test.upsert_t WHERE id = 1")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(name, "New");
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM drp_test.upsert_t")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 2);
    }

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL — run with `cargo test -- --ignored`"]
    async fn upsert_without_unique_constraint_reports_key_not_unique() {
        let pool = test_pool().await;
        sqlx::query("DROP TABLE IF EXISTS drp_test.no_unique")
            .execute(&pool)
            .await
            .ok();
        sqlx::query("CREATE SCHEMA IF NOT EXISTS drp_test")
            .execute(&pool)
            .await
            .ok();
        sqlx::query("CREATE TABLE drp_test.no_unique (id BIGINT, name TEXT)")
            .execute(&pool)
            .await
            .unwrap();

        let specs = parse_output_tables(&json!({
            "drp_test.no_unique": {"mode":"upsert","df":[{"id":1,"name":"A"}],"key":"id"}
        }))
        .unwrap();
        let perms = full_perms(&["drp_test"]);
        let out = write_output_tables(
            &pool,
            specs,
            &["drp_test".into()],
            &perms,
            None,
            &Default::default(),
            "create",
            "overwrite",
            30000,
            64,
        )
        .await;
        assert_eq!(out["error"], "UpsertKeyNotUnique", "got: {out}");

        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM drp_test.no_unique")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 0, "failed upsert must roll back — no partial writes");
    }

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL — run with `cargo test -- --ignored`"]
    async fn upsert_unrelated_unique_collision_is_not_key_not_unique() {
        // Table has a PK on `id` (a real ON CONFLICT arbiter) AND a
        // separate UNIQUE column `email`. Upserting ON CONFLICT(id) with
        // a row whose email collides with an existing row must NOT be
        // reported as UpsertKeyNotUnique — the `id` key IS backed by a
        // constraint; a different column collided. Fix 1 regression test.
        let pool = test_pool().await;
        sqlx::query("DROP TABLE IF EXISTS drp_test.unrelated_unique")
            .execute(&pool)
            .await
            .ok();
        sqlx::query("CREATE SCHEMA IF NOT EXISTS drp_test")
            .execute(&pool)
            .await
            .ok();
        sqlx::query(
            "CREATE TABLE drp_test.unrelated_unique (\
                id BIGINT PRIMARY KEY, email TEXT UNIQUE, name TEXT)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO drp_test.unrelated_unique (id, email, name) \
             VALUES (1, 'a@x.com', 'Ana')",
        )
        .execute(&pool)
        .await
        .unwrap();

        // ON CONFLICT(id) for id=2 (no PK collision), but email collides
        // with the existing row's email — a genuine 23505 on the email
        // constraint, unrelated to the ON CONFLICT(id) arbiter.
        let specs = parse_output_tables(&json!({
            "drp_test.unrelated_unique": {
                "mode":"upsert",
                "df":[{"id":2,"email":"a@x.com","name":"Bo"}],
                "key":"id"
            }
        }))
        .unwrap();
        let perms = full_perms(&["drp_test"]);
        let out = write_output_tables(
            &pool,
            specs,
            &["drp_test".into()],
            &perms,
            None,
            &Default::default(),
            "create",
            "overwrite",
            30000,
            64,
        )
        .await;
        assert_ne!(
            out["error"], "UpsertKeyNotUnique",
            "an unrelated unique-constraint collision must not be mislabeled as \
             UpsertKeyNotUnique: got {out}"
        );
        assert_eq!(out["error"], "ConstraintViolation", "got: {out}");

        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM drp_test.unrelated_unique")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 1, "failed upsert must roll back — no partial writes");
    }

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL — run with `cargo test -- --ignored`"]
    async fn replace_deletes_then_inserts() {
        let pool = test_pool().await;
        sqlx::query("DROP TABLE IF EXISTS drp_test.replace_t")
            .execute(&pool)
            .await
            .ok();
        sqlx::query("CREATE SCHEMA IF NOT EXISTS drp_test")
            .execute(&pool)
            .await
            .ok();
        sqlx::query("CREATE TABLE drp_test.replace_t (id BIGINT, name TEXT)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO drp_test.replace_t (id, name) VALUES (99, 'Stale')")
            .execute(&pool)
            .await
            .unwrap();

        let specs = parse_output_tables(&json!({
            "drp_test.replace_t": {"mode":"replace","df":[{"id":1,"name":"Ana"}]}
        }))
        .unwrap();
        let perms = full_perms(&["drp_test"]);
        let out = write_output_tables(
            &pool,
            specs,
            &["drp_test".into()],
            &perms,
            None,
            &Default::default(),
            "create",
            "overwrite",
            30000,
            64,
        )
        .await;
        assert_eq!(out["wrote_tables"][0]["rows_affected"], 1, "got: {out}");

        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM drp_test.replace_t")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 1);
        let name: String = sqlx::query_scalar("SELECT name FROM drp_test.replace_t")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(name, "Ana");
    }

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL — run with `cargo test -- --ignored`"]
    async fn update_full_record_fallback_without_snapshot() {
        let pool = test_pool().await;
        sqlx::query("DROP TABLE IF EXISTS drp_test.update_fallback")
            .execute(&pool)
            .await
            .ok();
        sqlx::query("CREATE SCHEMA IF NOT EXISTS drp_test")
            .execute(&pool)
            .await
            .ok();
        sqlx::query("CREATE TABLE drp_test.update_fallback (id BIGINT PRIMARY KEY, name TEXT)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO drp_test.update_fallback (id, name) VALUES (1, 'Old')")
            .execute(&pool)
            .await
            .unwrap();

        let specs = parse_output_tables(&json!({
            "drp_test.update_fallback": {"mode":"update","df":[{"id":1,"name":"New"}],"key":"id"}
        }))
        .unwrap();
        let perms = full_perms(&["drp_test"]);
        let out = write_output_tables(
            &pool,
            specs,
            &["drp_test".into()],
            &perms,
            None,
            &Default::default(),
            "create",
            "overwrite",
            30000,
            64,
        )
        .await;
        assert_eq!(out["wrote_tables"][0]["rows_affected"], 1, "got: {out}");
        assert!(out["wrote_tables"][0].get("changes").is_none());

        let name: String =
            sqlx::query_scalar("SELECT name FROM drp_test.update_fallback WHERE id = 1")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(name, "New");
    }

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL — run with `cargo test -- --ignored`"]
    async fn update_with_only_key_column_is_clean_noop_not_syntax_error() {
        // Fix 2 regression test (end-to-end): the input records contain
        // ONLY the key column, so there is nothing to SET. Before the
        // fix, build_full_record_updates would emit `SET  WHERE ...`
        // (malformed SQL) on the no-snapshot fallback path, surfacing as
        // a confusing TransactionError after rollback. After the fix
        // this must be a clean zero-row result, not a SQL error.
        let pool = test_pool().await;
        sqlx::query("DROP TABLE IF EXISTS drp_test.update_key_only")
            .execute(&pool)
            .await
            .ok();
        sqlx::query("CREATE SCHEMA IF NOT EXISTS drp_test")
            .execute(&pool)
            .await
            .ok();
        sqlx::query("CREATE TABLE drp_test.update_key_only (id BIGINT PRIMARY KEY, name TEXT)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO drp_test.update_key_only (id, name) VALUES (1, 'Ana')")
            .execute(&pool)
            .await
            .unwrap();

        let specs = parse_output_tables(&json!({
            "drp_test.update_key_only": {"mode":"update","df":[{"id":1}],"key":"id"}
        }))
        .unwrap();
        let perms = full_perms(&["drp_test"]);
        let out = write_output_tables(
            &pool,
            specs,
            &["drp_test".into()],
            &perms,
            None,
            &Default::default(),
            "create",
            "overwrite",
            30000,
            64,
        )
        .await;
        assert!(
            out.get("error").is_none(),
            "expected a clean result, not an error (malformed SQL): {out}"
        );
        assert_eq!(out["wrote_tables"][0]["rows_affected"], 0, "got: {out}");

        // Row must be untouched (no-op, not a corrupted write).
        let name: String =
            sqlx::query_scalar("SELECT name FROM drp_test.update_key_only WHERE id = 1")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(name, "Ana");
    }

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL — run with `cargo test -- --ignored`"]
    async fn update_diff_driven_with_snapshot_skips_unchanged() {
        let pool = test_pool().await;
        sqlx::query("DROP TABLE IF EXISTS drp_test.update_diff")
            .execute(&pool)
            .await
            .ok();
        sqlx::query("CREATE SCHEMA IF NOT EXISTS drp_test")
            .execute(&pool)
            .await
            .ok();
        sqlx::query("CREATE TABLE drp_test.update_diff (id BIGINT PRIMARY KEY, name TEXT)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO drp_test.update_diff (id, name) VALUES (1, 'Old'), (2, 'Same')")
            .execute(&pool)
            .await
            .unwrap();

        let specs = parse_output_tables(&json!({
            "drp_test.update_diff": {"mode":"update","df":[{"id":1,"name":"New"},{"id":2,"name":"Same"}],"key":"id"}
        }))
        .unwrap();
        let perms = full_perms(&["drp_test"]);

        let mut snapshots: HashMap<String, Vec<Map<String, Value>>> = HashMap::new();
        let mut r1 = Map::new();
        r1.insert("id".into(), json!(1));
        r1.insert("name".into(), json!("Old"));
        let mut r2 = Map::new();
        r2.insert("id".into(), json!(2));
        r2.insert("name".into(), json!("Same"));
        snapshots.insert("drp_test.update_diff".to_string(), vec![r1, r2]);

        let out = write_output_tables(
            &pool,
            specs,
            &["drp_test".into()],
            &perms,
            None,
            &snapshots,
            "create",
            "overwrite",
            30000,
            64,
        )
        .await;
        assert_eq!(out["wrote_tables"][0]["rows_affected"], 1, "got: {out}");
        assert_eq!(out["wrote_tables"][0]["changes"]["rows"], 1, "got: {out}");
        assert_eq!(out["wrote_tables"][0]["changes"]["cells"], 1, "got: {out}");

        let name: String = sqlx::query_scalar("SELECT name FROM drp_test.update_diff WHERE id = 1")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(name, "New");
    }

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL — run with `cargo test -- --ignored`"]
    async fn update_diff_driven_zero_changes_writes_nothing() {
        let pool = test_pool().await;
        sqlx::query("DROP TABLE IF EXISTS drp_test.update_nochange")
            .execute(&pool)
            .await
            .ok();
        sqlx::query("CREATE SCHEMA IF NOT EXISTS drp_test")
            .execute(&pool)
            .await
            .ok();
        sqlx::query("CREATE TABLE drp_test.update_nochange (id BIGINT PRIMARY KEY, name TEXT)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO drp_test.update_nochange (id, name) VALUES (1, 'Same')")
            .execute(&pool)
            .await
            .unwrap();

        let specs = parse_output_tables(&json!({
            "drp_test.update_nochange": {"mode":"update","df":[{"id":1,"name":"Same"}],"key":"id"}
        }))
        .unwrap();
        let perms = full_perms(&["drp_test"]);

        let mut snapshots: HashMap<String, Vec<Map<String, Value>>> = HashMap::new();
        let mut r1 = Map::new();
        r1.insert("id".into(), json!(1));
        r1.insert("name".into(), json!("Same"));
        snapshots.insert("drp_test.update_nochange".to_string(), vec![r1]);

        let out = write_output_tables(
            &pool,
            specs,
            &["drp_test".into()],
            &perms,
            None,
            &snapshots,
            "create",
            "overwrite",
            30000,
            64,
        )
        .await;
        assert_eq!(out["wrote_tables"][0]["rows_affected"], 0, "got: {out}");
        assert_eq!(out["wrote_tables"][0]["changes"]["cells"], 0, "got: {out}");
    }

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL — run with `cargo test -- --ignored`"]
    async fn composite_key_update_falls_back_to_full_record() {
        let pool = test_pool().await;
        sqlx::query("DROP TABLE IF EXISTS drp_test.composite_update")
            .execute(&pool)
            .await
            .ok();
        sqlx::query("CREATE SCHEMA IF NOT EXISTS drp_test")
            .execute(&pool)
            .await
            .ok();
        sqlx::query(
            "CREATE TABLE drp_test.composite_update (tenant TEXT, id BIGINT, name TEXT, \
             PRIMARY KEY (tenant, id))",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO drp_test.composite_update (tenant, id, name) VALUES ('a', 1, 'Old')",
        )
        .execute(&pool)
        .await
        .unwrap();

        let specs = parse_output_tables(&json!({
            "drp_test.composite_update": {
                "mode":"update",
                "df":[{"tenant":"a","id":1,"name":"New"}],
                "key":["tenant","id"]
            }
        }))
        .unwrap();
        let perms = full_perms(&["drp_test"]);
        let out = write_output_tables(
            &pool,
            specs,
            &["drp_test".into()],
            &perms,
            None,
            &Default::default(),
            "create",
            "overwrite",
            30000,
            64,
        )
        .await;
        assert_eq!(out["wrote_tables"][0]["rows_affected"], 1, "got: {out}");

        let name: String = sqlx::query_scalar(
            "SELECT name FROM drp_test.composite_update WHERE tenant = 'a' AND id = 1",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(name, "New");
    }

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL — run with `cargo test -- --ignored`"]
    async fn validation_failure_rolls_back_whole_batch() {
        let pool = test_pool().await;
        sqlx::query("DROP TABLE IF EXISTS drp_test.batch_ok")
            .execute(&pool)
            .await
            .ok();
        sqlx::query("CREATE SCHEMA IF NOT EXISTS drp_test")
            .execute(&pool)
            .await
            .ok();

        // Two specs: the first would succeed (auto-create + insert), the
        // second is an Update on a table that doesn't exist — must fail
        // validation and roll back the whole transaction, including the
        // first spec's insert.
        let mut specs = parse_output_tables(&json!({
            "drp_test.batch_ok": [{"id":1,"name":"Ana"}]
        }))
        .unwrap();
        let mut bad_specs = parse_output_tables(&json!({
            "drp_test.batch_missing": {"mode":"update","df":[{"id":1,"name":"X"}],"key":"id"}
        }))
        .unwrap();
        specs.append(&mut bad_specs);

        let perms = full_perms(&["drp_test"]);
        let out = write_output_tables(
            &pool,
            specs,
            &["drp_test".into()],
            &perms,
            None,
            &Default::default(),
            "create",
            "overwrite",
            30000,
            64,
        )
        .await;
        assert_eq!(out["error"], "TableNotFound", "got: {out}");

        let exists: Option<i64> = sqlx::query_scalar(
            "SELECT count(*) FROM information_schema.tables \
             WHERE table_schema = 'drp_test' AND table_name = 'batch_ok'",
        )
        .fetch_optional(&pool)
        .await
        .unwrap();
        assert_eq!(
            exists,
            Some(0),
            "batch_ok must NOT have been created — whole transaction rolled back"
        );
    }

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL — run with `cargo test -- --ignored`"]
    async fn tenant_user_id_sets_session_config() {
        let pool = test_pool().await;
        sqlx::query("DROP TABLE IF EXISTS drp_test.tenant_t")
            .execute(&pool)
            .await
            .ok();
        sqlx::query("CREATE SCHEMA IF NOT EXISTS drp_test")
            .execute(&pool)
            .await
            .ok();

        let specs = parse_output_tables(&json!({
            "drp_test.tenant_t": [{"id":1}]
        }))
        .unwrap();
        let perms = full_perms(&["drp_test"]);
        let out = write_output_tables(
            &pool,
            specs,
            &["drp_test".into()],
            &perms,
            Some("user-123"),
            &Default::default(),
            "create",
            "overwrite",
            30000,
            64,
        )
        .await;
        assert_eq!(out["wrote_tables"][0]["rows_affected"], 1, "got: {out}");
        // SET LOCAL config is scoped to the committed transaction and not
        // observable afterward — this test mainly asserts the write
        // still succeeds end-to-end when tenant_user_id is supplied.
    }

    // --- Task 14: operator table-policy gates (unit, no DB) ---

    #[tokio::test]
    async fn write_output_tables_rejects_unknown_on_missing_table_policy() {
        // Reaches the policy validation BEFORE any pool.begin() — a bogus
        // (never-connected) pool is never touched, so no DB needed. We build
        // an already-closed lazy pool that would error if used.
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://invalid/db")
            .unwrap();
        let specs = parse_output_tables(&json!({"a.t": [{"id": 1}]})).unwrap();
        let perms = full_perms(&["a"]);
        let out = write_output_tables(
            &pool,
            specs,
            &["a".into()],
            &perms,
            None,
            &Default::default(),
            "banana",
            "fail",
            30000,
            64,
        )
        .await;
        assert_eq!(out["error"], "InvalidPolicy", "got: {out}");
        assert_eq!(out["field"], "on_missing_table");
    }

    #[tokio::test]
    async fn write_output_tables_rejects_unknown_on_existing_table_policy() {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://invalid/db")
            .unwrap();
        let specs = parse_output_tables(&json!({"a.t": [{"id": 1}]})).unwrap();
        let perms = full_perms(&["a"]);
        let out = write_output_tables(
            &pool,
            specs,
            &["a".into()],
            &perms,
            None,
            &Default::default(),
            "create",
            "zap",
            30000,
            64,
        )
        .await;
        assert_eq!(out["error"], "InvalidPolicy", "got: {out}");
        assert_eq!(out["field"], "on_existing_table");
    }

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL — run with `cargo test -- --ignored`"]
    async fn on_missing_table_fail_blocks_autocreate() {
        // A `full` preset would normally auto-create the table; the operator
        // policy `on_missing_table = "fail"` must override that and return
        // TableNotFound WITHOUT creating anything.
        let pool = test_pool().await;
        sqlx::query("DROP TABLE IF EXISTS drp_test.no_autocreate")
            .execute(&pool)
            .await
            .ok();
        sqlx::query("CREATE SCHEMA IF NOT EXISTS drp_test")
            .execute(&pool)
            .await
            .ok();
        let specs = parse_output_tables(&json!({
            "drp_test.no_autocreate": [{"id":1,"name":"Ana"}]
        }))
        .unwrap();
        let perms = full_perms(&["drp_test"]);
        let out = write_output_tables(
            &pool,
            specs,
            &["drp_test".into()],
            &perms,
            None,
            &Default::default(),
            "fail",
            "overwrite",
            30000,
            64,
        )
        .await;
        assert_eq!(out["error"], "TableNotFound", "got: {out}");

        let exists: Option<i64> = sqlx::query_scalar(
            "SELECT count(*) FROM information_schema.tables \
             WHERE table_schema = 'drp_test' AND table_name = 'no_autocreate'",
        )
        .fetch_optional(&pool)
        .await
        .unwrap();
        assert_eq!(exists, Some(0), "table must NOT have been auto-created");
    }

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL — run with `cargo test -- --ignored`"]
    async fn on_existing_table_fail_blocks_replace_before_delete() {
        // Replace against an existing, non-empty table with the default
        // `on_existing_table = "fail"` must be rejected as TableExists and
        // must NOT delete the existing row.
        let pool = test_pool().await;
        sqlx::query("DROP TABLE IF EXISTS drp_test.no_replace")
            .execute(&pool)
            .await
            .ok();
        sqlx::query("CREATE SCHEMA IF NOT EXISTS drp_test")
            .execute(&pool)
            .await
            .ok();
        sqlx::query("CREATE TABLE drp_test.no_replace (id BIGINT, name TEXT)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO drp_test.no_replace (id, name) VALUES (99, 'Stale')")
            .execute(&pool)
            .await
            .unwrap();

        let specs = parse_output_tables(&json!({
            "drp_test.no_replace": {"mode":"replace","df":[{"id":1,"name":"Ana"}]}
        }))
        .unwrap();
        let perms = full_perms(&["drp_test"]);
        let out = write_output_tables(
            &pool,
            specs,
            &["drp_test".into()],
            &perms,
            None,
            &Default::default(),
            "create",
            "fail",
            30000,
            64,
        )
        .await;
        assert_eq!(out["error"], "TableExists", "got: {out}");

        // Existing row must be untouched — the gate runs BEFORE any DELETE.
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM drp_test.no_replace")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 1, "replace must have been blocked before deleting");
    }
}
