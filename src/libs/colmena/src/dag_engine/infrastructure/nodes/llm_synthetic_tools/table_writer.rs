//! Pure parsing/normalization for the `output_tables` SQL write-back sink
//! of `data_run_python`. The LLM's pandas code assigns a Python global
//! `output_tables = {"schema.table": <DataFrame-or-spec-dict>}`; after the
//! sandbox runs, that becomes a `serde_json::Value` (a JSON object). This
//! module turns that `Value` into typed [`TableWriteSpec`]s, validating
//! shape only — no I/O, no async, no SQL execution (later tasks own DB
//! validation + execution).
//!
//! See `docs/superpowers/specs/2026-07-01-data-run-python-design.md`.

use crate::gsheets::infrastructure::http_client::rectangle_to_records;
use serde::Deserialize;
use serde_json::{json, Map, Value};

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
}
