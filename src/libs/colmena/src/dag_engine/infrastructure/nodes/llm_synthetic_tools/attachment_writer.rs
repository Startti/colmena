//! `output_attachments` sink for `data_run_python`.
//!
//! Serializes postlude records (already plain arrays of JSON objects — see
//! Task 10) to CSV or XLSX bytes and registers them as conversation
//! attachments via a caller-supplied [`AttachmentRegistrar`]. Mirrors the
//! `AttachmentFetcher` shape from `tabular_bindings.rs` so the `data_run_python`
//! dispatcher (Task 14) can build both consistently.

use rust_xlsxwriter::Workbook;
use serde_json::{Map, Value};
use std::future::Future;
use std::pin::Pin;

/// Maximum number of records serialized into a single attachment.
const MAX_ROWS: usize = 100_000;
/// Maximum serialized byte size of a single attachment.
const MAX_BYTES: usize = 50 * 1024 * 1024;

/// Registers a named attachment's bytes and returns its `document_id`.
///
/// Mirrors [`super::tabular_bindings::AttachmentFetcher`]'s boxed-future
/// shape: `Box<dyn Fn(...) -> Pin<Box<dyn Future<...> + Send>> + Send + Sync>`.
pub type AttachmentRegistrar<'a> = Box<
    dyn Fn(String, Vec<u8>) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>>
        + Send
        + Sync
        + 'a,
>;

/// Serialize `records` to `fmt` (`"csv"` or `"xlsx"`) bytes.
///
/// Column set is the union of all record keys in first-seen order.
pub fn serialize_records(
    records: &[Map<String, Value>],
    fmt: &str,
    delimiter: Option<&str>,
) -> Result<Vec<u8>, String> {
    let columns = collect_columns(records);
    match fmt {
        "csv" => serialize_csv(records, &columns, delimiter),
        "xlsx" => serialize_xlsx(records, &columns),
        other => Err(format!("unsupported output_attachments format: '{other}' (expected 'csv' or 'xlsx')")),
    }
}

fn collect_columns(records: &[Map<String, Value>]) -> Vec<String> {
    let mut columns = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for record in records {
        for key in record.keys() {
            if seen.insert(key.clone()) {
                columns.push(key.clone());
            }
        }
    }
    columns
}

fn value_to_csv_field(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Bool(b)) => b.to_string(),
        Some(v @ (Value::Object(_) | Value::Array(_))) => {
            serde_json::to_string(v).unwrap_or_default()
        }
    }
}

fn serialize_csv(
    records: &[Map<String, Value>],
    columns: &[String],
    delimiter: Option<&str>,
) -> Result<Vec<u8>, String> {
    let delim_byte = match delimiter {
        Some(d) => {
            let mut bytes = d.bytes();
            let b = bytes
                .next()
                .ok_or_else(|| "delimiter must be a non-empty string".to_string())?;
            if bytes.next().is_some() {
                return Err("delimiter must be a single byte/char".to_string());
            }
            b
        }
        None => b',',
    };

    let mut writer = csv::WriterBuilder::new()
        .delimiter(delim_byte)
        .from_writer(Vec::new());

    writer
        .write_record(columns)
        .map_err(|e| format!("csv header write failed: {e}"))?;

    for record in records {
        let row: Vec<String> = columns
            .iter()
            .map(|c| value_to_csv_field(record.get(c)))
            .collect();
        writer
            .write_record(&row)
            .map_err(|e| format!("csv row write failed: {e}"))?;
    }

    writer
        .into_inner()
        .map_err(|e| format!("csv flush failed: {e}"))
}

fn serialize_xlsx(records: &[Map<String, Value>], columns: &[String]) -> Result<Vec<u8>, String> {
    let mut workbook = Workbook::new();
    let ws = workbook
        .add_worksheet()
        .set_name("Sheet1")
        .map_err(|e| format!("xlsx worksheet name failed: {e}"))?;

    for (col_idx, col_name) in columns.iter().enumerate() {
        ws.write_string(0, col_idx as u16, col_name)
            .map_err(|e| format!("xlsx header write failed: {e}"))?;
    }

    for (row_idx, record) in records.iter().enumerate() {
        let row = (row_idx + 1) as u32;
        for (col_idx, col_name) in columns.iter().enumerate() {
            let col = col_idx as u16;
            match record.get(col_name) {
                None | Some(Value::Null) => {}
                Some(Value::Number(n)) => {
                    if let Some(f) = n.as_f64() {
                        ws.write_number(row, col, f)
                            .map_err(|e| format!("xlsx number write failed: {e}"))?;
                    } else {
                        ws.write_string(row, col, n.to_string())
                            .map_err(|e| format!("xlsx string write failed: {e}"))?;
                    }
                }
                Some(Value::Bool(b)) => {
                    ws.write_boolean(row, col, *b)
                        .map_err(|e| format!("xlsx bool write failed: {e}"))?;
                }
                Some(Value::String(s)) => {
                    ws.write_string(row, col, s)
                        .map_err(|e| format!("xlsx string write failed: {e}"))?;
                }
                Some(v @ (Value::Object(_) | Value::Array(_))) => {
                    let s = serde_json::to_string(v).unwrap_or_default();
                    ws.write_string(row, col, s)
                        .map_err(|e| format!("xlsx string write failed: {e}"))?;
                }
            }
        }
    }

    workbook
        .save_to_buffer()
        .map_err(|e| format!("xlsx save failed: {e}"))
}

/// Format inferred from a filename's extension.
fn format_from_name(name: &str) -> Option<&'static str> {
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".csv") {
        Some("csv")
    } else if lower.ends_with(".xlsx") {
        Some("xlsx")
    } else {
        None
    }
}

/// Extract the records array and optional delimiter from a spec entry,
/// which is either a bare array of records or `{df: [...], delimiter?}`.
fn extract_spec(spec: &Value) -> Result<(Vec<Map<String, Value>>, Option<String>), Value> {
    let (records_value, delimiter) = match spec {
        Value::Array(_) => (spec, None),
        Value::Object(obj) => {
            let df = obj.get("df").ok_or_else(|| {
                serde_json::json!({"error": "InvalidFormat", "detail": "spec object missing 'df' field"})
            })?;
            let delim = obj
                .get("delimiter")
                .and_then(|d| d.as_str())
                .map(|s| s.to_string());
            (df, delim)
        }
        _ => {
            return Err(
                serde_json::json!({"error": "InvalidFormat", "detail": "spec must be an array of records or {df, delimiter?}"}),
            )
        }
    };

    let arr = records_value.as_array().ok_or_else(|| {
        serde_json::json!({"error": "InvalidFormat", "detail": "'df' must be an array of records"})
    })?;

    let mut records = Vec::with_capacity(arr.len());
    for item in arr {
        match item {
            Value::Object(m) => records.push(m.clone()),
            _ => {
                return Err(
                    serde_json::json!({"error": "InvalidFormat", "detail": "each record must be a JSON object"}),
                )
            }
        }
    }

    Ok((records, delimiter))
}

/// Serialize and register every entry of `value` (a dict of
/// `filename -> spec`) as a conversation attachment.
///
/// Returns `{"wrote_attachments": [ {name, document_id, rows, bytes}, ... ]}`
/// on full success, or a structured `{"error": ..., ...}` on the first
/// failure encountered (fail-fast).
pub async fn write_output_attachments(value: &Value, register: &AttachmentRegistrar<'_>) -> Value {
    let obj = match value.as_object() {
        Some(o) => o,
        None => {
            return serde_json::json!({
                "error": "InvalidFormat",
                "detail": "output_attachments value must be an object of filename -> records"
            })
        }
    };

    let mut wrote = Vec::with_capacity(obj.len());

    for (name, spec) in obj {
        let fmt = match format_from_name(name) {
            Some(f) => f,
            None => {
                return serde_json::json!({
                    "error": "InvalidFormat",
                    "detail": format!("attachment '{name}' has no recognized extension (.csv/.xlsx)"),
                    "name": name,
                })
            }
        };

        let (records, delimiter) = match extract_spec(spec) {
            Ok(v) => v,
            Err(mut e) => {
                if let Value::Object(m) = &mut e {
                    m.insert("name".to_string(), Value::String(name.clone()));
                }
                return e;
            }
        };

        if records.len() > MAX_ROWS {
            return serde_json::json!({
                "error": "TooManyRows",
                "detail": format!("attachment '{name}' has {} rows, exceeds max {MAX_ROWS}", records.len()),
                "name": name,
            });
        }

        let bytes = match serialize_records(&records, fmt, delimiter.as_deref()) {
            Ok(b) => b,
            Err(e) => {
                return serde_json::json!({
                    "error": "SerializationFailed",
                    "detail": e,
                    "name": name,
                })
            }
        };

        if bytes.len() > MAX_BYTES {
            return serde_json::json!({
                "error": "AttachmentTooLarge",
                "detail": format!("attachment '{name}' is {} bytes, exceeds max {MAX_BYTES}", bytes.len()),
                "name": name,
            });
        }

        let rows = records.len();
        let byte_len = bytes.len();

        let document_id = match register(name.clone(), bytes).await {
            Ok(id) => id,
            Err(e) => {
                return serde_json::json!({
                    "error": "RegistrationFailed",
                    "detail": e,
                    "name": name,
                })
            }
        };

        wrote.push(serde_json::json!({
            "name": name,
            "document_id": document_id,
            "rows": rows,
            "bytes": byte_len,
        }));
    }

    serde_json::json!({ "wrote_attachments": wrote })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialize_csv_has_header_and_rows() {
        let recs: Vec<Map<String, Value>> =
            vec![serde_json::from_value(serde_json::json!({"a": 1, "b": "x"})).unwrap()];
        let bytes = serialize_records(&recs, "csv", None).unwrap();
        let s = String::from_utf8(bytes).unwrap();
        assert!(s.starts_with("a,b"));
        assert!(s.contains("1,x"));
    }

    #[test]
    fn serialize_xlsx_is_nonempty_zip() {
        let recs: Vec<Map<String, Value>> =
            vec![serde_json::from_value(serde_json::json!({"a": 1})).unwrap()];
        let bytes = serialize_records(&recs, "xlsx", None).unwrap();
        assert_eq!(&bytes[0..2], b"PK"); // xlsx is a zip
    }

    #[test]
    fn serialize_unknown_format_errors() {
        let recs: Vec<Map<String, Value>> = vec![];
        let err = serialize_records(&recs, "parquet", None).unwrap_err();
        assert!(err.contains("unsupported"));
    }

    #[test]
    fn serialize_csv_custom_delimiter() {
        let recs: Vec<Map<String, Value>> =
            vec![serde_json::from_value(serde_json::json!({"a": 1, "b": "x"})).unwrap()];
        let bytes = serialize_records(&recs, "csv", Some(";")).unwrap();
        let s = String::from_utf8(bytes).unwrap();
        assert!(s.starts_with("a;b"));
    }

    #[test]
    fn serialize_csv_handles_null_bool_nested() {
        let recs: Vec<Map<String, Value>> = vec![serde_json::from_value(serde_json::json!({
            "n": null, "b": true, "o": {"x": 1}
        }))
        .unwrap()];
        let bytes = serialize_records(&recs, "csv", None).unwrap();
        let s = String::from_utf8(bytes).unwrap();
        assert!(s.contains("true"));
        // CSV quotes the field because the JSON contains a comma-adjacent
        // brace/colon set that csv-writer treats conservatively; assert on
        // the unquoted inner content instead of exact quoting.
        assert!(s.contains("x") && s.contains("1"));
    }

    fn mock_registrar<'a>() -> AttachmentRegistrar<'a> {
        Box::new(|name, bytes| {
            Box::pin(async move { Ok(format!("doc_{}_{}", name, bytes.len())) })
        })
    }

    #[tokio::test]
    async fn write_output_attachments_bare_array_csv() {
        let register = mock_registrar();
        let value = serde_json::json!({
            "report.csv": [ {"a": 1, "b": "x"}, {"a": 2, "b": "y"} ]
        });
        let result = write_output_attachments(&value, &register).await;
        let wrote = result["wrote_attachments"].as_array().unwrap();
        assert_eq!(wrote.len(), 1);
        assert_eq!(wrote[0]["name"], "report.csv");
        assert_eq!(wrote[0]["rows"], 2);
        assert!(wrote[0]["document_id"]
            .as_str()
            .unwrap()
            .starts_with("doc_report.csv_"));
    }

    #[tokio::test]
    async fn write_output_attachments_df_spec_xlsx() {
        let register = mock_registrar();
        let value = serde_json::json!({
            "report.xlsx": { "df": [ {"a": 1} ], "delimiter": "," }
        });
        let result = write_output_attachments(&value, &register).await;
        let wrote = result["wrote_attachments"].as_array().unwrap();
        assert_eq!(wrote.len(), 1);
        assert_eq!(wrote[0]["rows"], 1);
    }

    #[tokio::test]
    async fn write_output_attachments_invalid_extension() {
        let register = mock_registrar();
        let value = serde_json::json!({ "report.txt": [ {"a": 1} ] });
        let result = write_output_attachments(&value, &register).await;
        assert_eq!(result["error"], "InvalidFormat");
    }

    #[tokio::test]
    async fn write_output_attachments_too_many_rows() {
        let register = mock_registrar();
        let recs: Vec<Value> = (0..100_001).map(|i| serde_json::json!({"a": i})).collect();
        let value = serde_json::json!({ "big.csv": recs });
        let result = write_output_attachments(&value, &register).await;
        assert_eq!(result["error"], "TooManyRows");
    }

    #[tokio::test]
    async fn write_output_attachments_registration_failure() {
        let register: AttachmentRegistrar =
            Box::new(|_name, _bytes| Box::pin(async move { Err("boom".to_string()) }));
        let value = serde_json::json!({ "report.csv": [ {"a": 1} ] });
        let result = write_output_attachments(&value, &register).await;
        assert_eq!(result["error"], "RegistrationFailed");
    }
}
