//! Convert records-style data (output_sheet from `crdt_doc_run_python`)
//! into Y.Doc sheet writes. Owns sheet creation, name collision
//! resolution, and per-cell writes via `apply_set_cell_in_proc`.

use crate::crdt_documents::projection;
use crate::crdt_documents::tool_executor::{apply_add_sheet, apply_set_cell_in_proc};
use serde_json::{Map, Value};
use yrs::Doc;

/// v1 hard limit on rows written via `crdt_doc_run_python`. See BACKLOG
/// for v1.1 configurable path.
pub const MAX_OUTPUT_SHEET_ROWS: usize = 100_000;

/// Excel xlsx hard limit on sheet name length.
pub const MAX_SHEET_NAME_LEN: usize = 31;

#[derive(Debug, thiserror::Error)]
pub enum WriterError {
    #[error("sheet name is empty")]
    EmptyName,
}

#[derive(Debug, Clone)]
pub struct WriteResult {
    pub sheet_id: String,
    pub resolved_name: String,
    pub n_rows: usize,
    pub n_cols: usize,
    /// Set to `Some(MAX_OUTPUT_SHEET_ROWS)` when the input records exceeded
    /// the cap and were truncated.
    pub truncated_at: Option<usize>,
}

/// Write `records` as a new sheet named `requested_name` (with auto-suffix
/// on collision). Returns the resolved sheet metadata.
pub fn write_records_as_new_sheet(
    doc: &Doc,
    requested_name: &str,
    columns: &[String],
    records: &[Map<String, Value>],
) -> Result<WriteResult, WriterError> {
    if requested_name.is_empty() {
        return Err(WriterError::EmptyName);
    }

    let resolved = resolve_unique_sheet_name(doc, requested_name);
    let resolved_capped = if resolved.len() > MAX_SHEET_NAME_LEN {
        resolved[..MAX_SHEET_NAME_LEN].to_string()
    } else {
        resolved
    };

    let (rows_to_write, truncated_at) = if records.len() > MAX_OUTPUT_SHEET_ROWS {
        (
            &records[..MAX_OUTPUT_SHEET_ROWS],
            Some(MAX_OUTPUT_SHEET_ROWS),
        )
    } else {
        (records, None)
    };

    let sheet_id = apply_add_sheet(doc, &resolved_capped);

    // Write column names in row 1.
    for (i, col_name) in columns.iter().enumerate() {
        let addr = format!("{}{}", col_letter(i as u32), 1);
        // D-T8 TODO: replace with cascade recalc + formula_replaced_by_literal event emission.
        let _ = apply_set_cell_in_proc(doc, &sheet_id, &addr, &Value::String(col_name.clone()));
    }

    // Write data starting at row 2.
    for (r_idx, record) in rows_to_write.iter().enumerate() {
        let row_num = (r_idx + 2) as u32;
        for (c_idx, col_name) in columns.iter().enumerate() {
            let addr = format!("{}{}", col_letter(c_idx as u32), row_num);
            let val = record.get(col_name).cloned().unwrap_or(Value::Null);
            if val.is_null() {
                continue;
            }
            // D-T8 TODO: replace with cascade recalc + formula_replaced_by_literal event emission.
            let _ = apply_set_cell_in_proc(doc, &sheet_id, &addr, &val);
        }
    }

    Ok(WriteResult {
        sheet_id,
        resolved_name: resolved_capped,
        n_rows: rows_to_write.len(),
        n_cols: columns.len(),
        truncated_at,
    })
}

/// Resolve a unique sheet name. Tries `requested`, then `"requested (2)"`,
/// `"requested (3)"`, ..., up to `requested (999)`. Falls back to a unix
/// timestamp suffix if 1000 collisions hit (~impossible in practice).
pub fn resolve_unique_sheet_name(doc: &Doc, requested: &str) -> String {
    let proj = projection::project(doc);
    let existing: std::collections::HashSet<String> = proj["sheets"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|s| s["name"].as_str().map(String::from))
        .collect();
    if !existing.contains(requested) {
        return requested.to_string();
    }
    for i in 2..1000 {
        let candidate = format!("{requested} ({i})");
        if !existing.contains(&candidate) {
            return candidate;
        }
    }
    format!("{requested} {}", chrono::Utc::now().timestamp())
}

fn col_letter(mut col: u32) -> String {
    let mut s = String::new();
    loop {
        s.insert(0, (b'A' + (col % 26) as u8) as char);
        if col < 26 {
            break;
        }
        col = col / 26 - 1;
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_record(pairs: &[(&str, Value)]) -> Map<String, Value> {
        let mut m = Map::new();
        for (k, v) in pairs {
            m.insert(k.to_string(), v.clone());
        }
        m
    }

    #[test]
    fn write_basic_records_creates_sheet_with_headers_and_data() {
        let doc = Doc::new();
        let cols = vec!["Region".to_string(), "Sales".to_string()];
        let records = vec![
            make_record(&[("Region", json!("North")), ("Sales", json!(450))]),
            make_record(&[("Region", json!("South")), ("Sales", json!(320))]),
        ];
        let result = write_records_as_new_sheet(&doc, "Summary", &cols, &records).unwrap();
        assert_eq!(result.resolved_name, "Summary");
        assert_eq!(result.n_rows, 2);
        assert_eq!(result.n_cols, 2);
        assert!(result.truncated_at.is_none());

        let proj = projection::project(&doc);
        let sheet = proj["sheets"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["name"] == "Summary")
            .unwrap()
            .clone();
        assert_eq!(sheet["cells"]["A1"], json!("Region"));
        assert_eq!(sheet["cells"]["B1"], json!("Sales"));
        assert_eq!(sheet["cells"]["A2"], json!("North"));
        // Note: numeric values round-trip as f64 via the projection layer.
        assert_eq!(sheet["cells"]["B2"], json!(450.0));
        assert_eq!(sheet["cells"]["A3"], json!("South"));
    }

    #[test]
    fn collision_resolution_appends_suffix() {
        let doc = Doc::new();
        let _ = apply_add_sheet(&doc, "Summary");
        let cols = vec!["A".to_string()];
        let records = vec![make_record(&[("A", json!(1))])];
        let result = write_records_as_new_sheet(&doc, "Summary", &cols, &records).unwrap();
        assert_eq!(result.resolved_name, "Summary (2)");
    }

    #[test]
    fn nested_collision_keeps_advancing() {
        let doc = Doc::new();
        let _ = apply_add_sheet(&doc, "Summary");
        let _ = apply_add_sheet(&doc, "Summary (2)");
        let _ = apply_add_sheet(&doc, "Summary (3)");
        let cols = vec!["A".to_string()];
        let records = vec![make_record(&[("A", json!(1))])];
        let result = write_records_as_new_sheet(&doc, "Summary", &cols, &records).unwrap();
        assert_eq!(result.resolved_name, "Summary (4)");
    }

    #[test]
    fn empty_records_writes_only_headers() {
        let doc = Doc::new();
        let cols = vec!["X".to_string(), "Y".to_string()];
        let result = write_records_as_new_sheet(&doc, "Empty", &cols, &[]).unwrap();
        assert_eq!(result.n_rows, 0);
        assert_eq!(result.n_cols, 2);
    }

    #[test]
    fn rejects_empty_name() {
        let doc = Doc::new();
        let err = write_records_as_new_sheet(&doc, "", &[], &[]).unwrap_err();
        assert!(matches!(err, WriterError::EmptyName));
    }

    #[test]
    fn truncates_at_max_rows() {
        let doc = Doc::new();
        let cols = vec!["A".to_string()];
        let records: Vec<Map<String, Value>> = (0..MAX_OUTPUT_SHEET_ROWS + 100)
            .map(|i| make_record(&[("A", json!(i))]))
            .collect();
        let result = write_records_as_new_sheet(&doc, "Big", &cols, &records).unwrap();
        assert_eq!(result.n_rows, MAX_OUTPUT_SHEET_ROWS);
        assert_eq!(result.truncated_at, Some(MAX_OUTPUT_SHEET_ROWS));
    }
}
