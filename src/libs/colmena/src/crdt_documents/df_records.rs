//! Convert Y.Doc workbook sheets into records-style data
//! (`Vec<Map<String, Value>>`) for ingestion by pandas
//! `DataFrame.from_records(...)` on the Python side. Assumes row 1 is
//! the header row; falls back to `col_A`, `col_B`, ... when headers are
//! missing or non-string.

use crate::crdt_documents::projection;
use serde_json::{Map, Value};
use std::collections::HashMap;
use yrs::Doc;

/// Combined size cap for the records produced across all sheets in one
/// `run_python` call (v1 hard limit; see BACKLOG for configurable path).
pub const COMBINED_RECORDS_SIZE_CAP_BYTES: usize = 100 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum RecordsError {
    #[error("sheet not found: {0}")]
    SheetNotFound(String),
    #[error("combined records size {actual} bytes exceeds cap {limit} bytes")]
    SizeCapExceeded { actual: usize, limit: usize },
}

#[derive(Debug, Clone)]
pub struct SheetRecords {
    pub sheet_id: String,
    pub columns: Vec<String>,
    /// Row-major: each inner Map is one row, keyed by column name.
    pub records: Vec<Map<String, Value>>,
}

/// Build records for one sheet.
pub fn build_sheet_records(doc: &Doc, sheet_id: &str) -> Result<SheetRecords, RecordsError> {
    let proj = projection::project(doc);
    let sheets = proj["sheets"].as_array().cloned().unwrap_or_default();
    let sheet = sheets
        .into_iter()
        .find(|s| s["id"].as_str() == Some(sheet_id))
        .ok_or_else(|| RecordsError::SheetNotFound(sheet_id.to_string()))?;
    let cells_map = sheet["cells"].as_object().cloned().unwrap_or_default();

    let mut parsed: Vec<(u32, u32, Value)> = Vec::new();
    for (addr, value) in cells_map.into_iter() {
        if let Some((row, col)) = parse_a1(&addr) {
            parsed.push((row, col, value));
        }
    }
    if parsed.is_empty() {
        return Ok(SheetRecords {
            sheet_id: sheet_id.to_string(),
            columns: Vec::new(),
            records: Vec::new(),
        });
    }

    let max_col = parsed.iter().map(|(_, c, _)| *c).max().unwrap();
    let max_row = parsed.iter().map(|(r, _, _)| *r).max().unwrap();

    let mut grid: Vec<Vec<Value>> = (0..=max_row)
        .map(|_| vec![Value::Null; (max_col + 1) as usize])
        .collect();
    for (r, c, v) in parsed {
        grid[r as usize][c as usize] = v;
    }

    // Headers from row 0 (1-indexed row 1).
    let columns: Vec<String> = grid[0]
        .iter()
        .enumerate()
        .map(|(i, v)| match v {
            Value::String(s) => s.clone(),
            Value::Null => format!("col_{}", col_letter(i as u32)),
            other => other.to_string().trim_matches('"').to_string(),
        })
        .collect();

    let mut records: Vec<Map<String, Value>> = Vec::new();
    for row in grid.iter().skip(1) {
        if row.iter().all(|v| v.is_null()) {
            continue;
        }
        let mut record = Map::new();
        for (i, v) in row.iter().enumerate() {
            let col_name = columns
                .get(i)
                .cloned()
                .unwrap_or_else(|| format!("col_{}", col_letter(i as u32)));
            record.insert(col_name, v.clone());
        }
        records.push(record);
    }

    Ok(SheetRecords {
        sheet_id: sheet_id.to_string(),
        columns,
        records,
    })
}

/// Build records for multiple sheets in one call. Enforces combined size cap.
pub fn build_records_for_sheets(
    doc: &Doc,
    sheet_ids: &[String],
) -> Result<HashMap<String, SheetRecords>, RecordsError> {
    let mut total_bytes: usize = 0;
    let mut out = HashMap::new();
    for sid in sheet_ids {
        let recs = build_sheet_records(doc, sid)?;
        let approx = serde_json::to_vec(&recs.records)
            .map(|v| v.len())
            .unwrap_or(0);
        total_bytes = total_bytes.saturating_add(approx);
        if total_bytes > COMBINED_RECORDS_SIZE_CAP_BYTES {
            return Err(RecordsError::SizeCapExceeded {
                actual: total_bytes,
                limit: COMBINED_RECORDS_SIZE_CAP_BYTES,
            });
        }
        out.insert(sid.clone(), recs);
    }
    Ok(out)
}

fn parse_a1(addr: &str) -> Option<(u32, u32)> {
    let split = addr.find(|c: char| c.is_ascii_digit())?;
    let col_part = &addr[..split];
    let row_part = &addr[split..];
    let row: u32 = row_part.parse().ok()?;
    let row = row.checked_sub(1)?;
    let mut col: u32 = 0;
    for ch in col_part.chars() {
        if !ch.is_ascii_uppercase() {
            return None;
        }
        col = col * 26 + (ch as u32 - 'A' as u32 + 1);
    }
    Some((row, col.checked_sub(1)?))
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
    use crate::crdt_documents::tool_executor::{apply_add_sheet, apply_set_cell_in_proc};

    fn make_doc_with_inventory() -> (Doc, String) {
        let doc = Doc::new();
        let sheet_id = apply_add_sheet(&doc, "Inventory");
        apply_set_cell_in_proc(&doc, &sheet_id, "A1", &serde_json::json!("Product"));
        apply_set_cell_in_proc(&doc, &sheet_id, "B1", &serde_json::json!("Qty"));
        apply_set_cell_in_proc(&doc, &sheet_id, "A2", &serde_json::json!("Apple"));
        apply_set_cell_in_proc(&doc, &sheet_id, "B2", &serde_json::json!(10));
        apply_set_cell_in_proc(&doc, &sheet_id, "A3", &serde_json::json!("Pear"));
        apply_set_cell_in_proc(&doc, &sheet_id, "B3", &serde_json::json!(20));
        (doc, sheet_id)
    }

    #[test]
    fn extracts_headers_and_rows() {
        let (doc, sid) = make_doc_with_inventory();
        let recs = build_sheet_records(&doc, &sid).unwrap();
        assert_eq!(recs.columns, vec!["Product".to_string(), "Qty".to_string()]);
        assert_eq!(recs.records.len(), 2);
        assert_eq!(recs.records[0]["Product"], serde_json::json!("Apple"));
        // Numbers round-trip through Y.Doc as f64.
        assert_eq!(recs.records[0]["Qty"], serde_json::json!(10.0));
        assert_eq!(recs.records[1]["Product"], serde_json::json!("Pear"));
    }

    #[test]
    fn missing_sheet_returns_not_found() {
        let doc = Doc::new();
        let err = build_sheet_records(&doc, "sh_does_not_exist").unwrap_err();
        assert!(matches!(err, RecordsError::SheetNotFound(_)));
    }

    #[test]
    fn empty_sheet_returns_empty_columns_and_rows() {
        let doc = Doc::new();
        let sid = apply_add_sheet(&doc, "Blank");
        let recs = build_sheet_records(&doc, &sid).unwrap();
        assert_eq!(recs.columns.len(), 0);
        assert_eq!(recs.records.len(), 0);
    }

    #[test]
    fn headers_only_returns_zero_rows_with_columns() {
        let doc = Doc::new();
        let sid = apply_add_sheet(&doc, "HeadersOnly");
        apply_set_cell_in_proc(&doc, &sid, "A1", &serde_json::json!("X"));
        apply_set_cell_in_proc(&doc, &sid, "B1", &serde_json::json!("Y"));
        let recs = build_sheet_records(&doc, &sid).unwrap();
        assert_eq!(recs.columns, vec!["X".to_string(), "Y".to_string()]);
        assert_eq!(recs.records.len(), 0);
    }

    #[test]
    fn non_string_headers_fall_back_to_stringified() {
        let doc = Doc::new();
        let sid = apply_add_sheet(&doc, "BadHeaders");
        apply_set_cell_in_proc(&doc, &sid, "A1", &serde_json::json!(1.5));
        apply_set_cell_in_proc(&doc, &sid, "B1", &serde_json::json!(2.5));
        apply_set_cell_in_proc(&doc, &sid, "A2", &serde_json::json!("data"));
        let recs = build_sheet_records(&doc, &sid).unwrap();
        assert_eq!(recs.columns, vec!["1.5".to_string(), "2.5".to_string()]);
    }

    #[test]
    fn sparse_cells_become_null_in_records() {
        let doc = Doc::new();
        let sid = apply_add_sheet(&doc, "Sparse");
        apply_set_cell_in_proc(&doc, &sid, "A1", &serde_json::json!("X"));
        apply_set_cell_in_proc(&doc, &sid, "B1", &serde_json::json!("Y"));
        apply_set_cell_in_proc(&doc, &sid, "A2", &serde_json::json!("filled"));
        let recs = build_sheet_records(&doc, &sid).unwrap();
        assert_eq!(recs.records.len(), 1);
        assert_eq!(recs.records[0]["X"], serde_json::json!("filled"));
        assert_eq!(recs.records[0]["Y"], serde_json::json!(null));
    }

    #[test]
    fn build_multiple_sheets() {
        let doc = Doc::new();
        let s1 = apply_add_sheet(&doc, "First");
        let s2 = apply_add_sheet(&doc, "Second");
        apply_set_cell_in_proc(&doc, &s1, "A1", &serde_json::json!("A"));
        apply_set_cell_in_proc(&doc, &s1, "A2", &serde_json::json!(1));
        apply_set_cell_in_proc(&doc, &s2, "A1", &serde_json::json!("B"));
        apply_set_cell_in_proc(&doc, &s2, "A2", &serde_json::json!(2));
        let map = build_records_for_sheets(&doc, &[s1.clone(), s2.clone()]).unwrap();
        assert_eq!(map.len(), 2);
        assert!(map.contains_key(&s1));
        assert!(map.contains_key(&s2));
    }
}
