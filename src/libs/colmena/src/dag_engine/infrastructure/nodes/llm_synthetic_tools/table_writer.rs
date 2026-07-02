//! Pure parsing/normalization for the `output_tables` SQL write-back sink
//! of `data_run_python`. The LLM's pandas code assigns a Python global
//! `output_tables = {"schema.table": <DataFrame-or-spec-dict>}`; after the
//! sandbox runs, that becomes a `serde_json::Value` (a JSON object). This
//! module turns that `Value` into typed [`TableWriteSpec`]s, validating
//! shape only — no I/O, no async, no SQL execution (later tasks own DB
//! validation + execution).
//!
//! See `docs/superpowers/specs/2026-07-01-data-run-python-design.md`.

use crate::dag_engine::domain::sql_permissions::{SqlOperation, SqlPermissions};
use crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::sql_bulk_tools::validate_table_against_allowlist;
use crate::gsheets::infrastructure::http_client::rectangle_to_records;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::collections::HashSet;

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

    // 7. Column names non-empty & unique.
    {
        let mut seen_cols = HashSet::new();
        for c in &cols {
            if c.is_empty() {
                return Err(json!({
                    "error": "InvalidColumnName",
                    "table": spec.table,
                    "message": "column name must not be empty",
                }));
            }
            if !seen_cols.insert(c) {
                return Err(json!({
                    "error": "InvalidColumnName",
                    "table": spec.table,
                    "column": c,
                    "message": "duplicate column name in input records",
                }));
            }
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
}
