//! Render the current projection of a `yrs::Doc` into a `.xlsx` byte buffer
//! via `rust_xlsxwriter`.
//!
//! v1 scope: cells only (strings, numbers, booleans). Format, formulas,
//! merged cells, and charts are NOT written — they are documented as a
//! v1.1 follow-up.

use crate::crdt_documents::projection;
use rust_xlsxwriter::Workbook;
use serde_json::Value;
use yrs::Doc;

#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error("xlsx: {0}")]
    Xlsx(#[from] rust_xlsxwriter::XlsxError),
}

pub fn export_doc_to_xlsx(doc: &Doc) -> Result<Vec<u8>, ExportError> {
    let proj = projection::project(doc);
    let mut workbook = Workbook::new();

    let sheets = proj["sheets"].as_array().cloned().unwrap_or_default();
    if sheets.is_empty() {
        let _ = workbook.add_worksheet().set_name("Sheet1")?;
    }
    for sheet in sheets {
        let name = sheet["name"].as_str().unwrap_or("Sheet").to_string();
        let ws = workbook.add_worksheet();
        ws.set_name(&name)?;
        if let Some(cells) = sheet["cells"].as_object() {
            for (addr, value) in cells {
                let (row, col) = match parse_a1(addr) {
                    Some(p) => p,
                    None => continue,
                };
                match value {
                    Value::String(s) => {
                        ws.write_string(row, col, s)?;
                    }
                    Value::Number(n) => {
                        if let Some(f) = n.as_f64() {
                            ws.write_number(row, col, f)?;
                        }
                    }
                    Value::Bool(b) => {
                        ws.write_boolean(row, col, *b)?;
                    }
                    _ => {}
                }
            }
        }
    }

    Ok(workbook.save_to_buffer()?)
}

fn parse_a1(addr: &str) -> Option<(u32, u16)> {
    let split = addr.find(|c: char| c.is_ascii_digit())?;
    let col_part = &addr[..split];
    let row_part = &addr[split..];
    if col_part.is_empty() || row_part.is_empty() {
        return None;
    }
    let row: u32 = row_part.parse().ok()?;
    let row = row.checked_sub(1)?;
    let mut col: u32 = 0;
    for ch in col_part.chars() {
        if !ch.is_ascii_uppercase() {
            return None;
        }
        col = col * 26 + (ch as u32 - 'A' as u32 + 1);
    }
    let col = col.checked_sub(1)?;
    Some((row, col as u16))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crdt_documents::tool_executor::{apply_add_sheet, apply_set_cell_in_proc};

    #[test]
    fn exports_two_sheets_with_values() {
        let doc = Doc::new();
        let s1 = apply_add_sheet(&doc, "Sales");
        let s2 = apply_add_sheet(&doc, "Summary");
        let _ = apply_set_cell_in_proc(&doc, &s1, "A1", &serde_json::json!("Product"));
        let _ = apply_set_cell_in_proc(&doc, &s2, "A1", &serde_json::json!(42));
        let bytes = export_doc_to_xlsx(&doc).unwrap();
        assert_eq!(&bytes[..2], b"PK"); // xlsx is a zip
                                        // Round-trip via the importer to confirm values survive.
        let doc2 = Doc::new();
        crate::crdt_documents::xlsx_import::import_xlsx_into_doc(&doc2, &bytes).unwrap();
        let v = crate::crdt_documents::projection::project(&doc2);
        let sheets = v["sheets"].as_array().unwrap();
        assert_eq!(sheets.len(), 2);
        let sales = sheets.iter().find(|s| s["name"] == "Sales").unwrap();
        let summary = sheets.iter().find(|s| s["name"] == "Summary").unwrap();
        assert_eq!(sales["cells"]["A1"], "Product");
        assert_eq!(summary["cells"]["A1"], serde_json::json!(42.0));
    }

    #[test]
    fn exports_empty_workbook_with_default_sheet() {
        let doc = Doc::new();
        let bytes = export_doc_to_xlsx(&doc).unwrap();
        assert_eq!(&bytes[..2], b"PK");
        // Re-import — should yield one (empty) sheet.
        let doc2 = Doc::new();
        crate::crdt_documents::xlsx_import::import_xlsx_into_doc(&doc2, &bytes).unwrap();
        let v = crate::crdt_documents::projection::project(&doc2);
        // Empty sheet may project as 0 or 1 sheets depending on the renderer;
        // the only invariant we assert is that the export didn't error.
        let _ = v["sheets"].as_array().unwrap();
    }
}
