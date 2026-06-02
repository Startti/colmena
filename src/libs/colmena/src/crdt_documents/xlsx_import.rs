//! Read an in-memory xlsx blob and populate a fresh sheet structure in the
//! given `yrs::Doc`. Wipes any existing sheets first ("import replaces").

use calamine::{open_workbook_from_rs, Data, Reader, Xlsx};
use std::io::Cursor;
use yrs::{Array, ArrayPrelim, Doc, Map, MapPrelim, Transact, WriteTxn};

#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error("xlsx: {0}")]
    Xlsx(#[from] calamine::XlsxError),
}

pub struct ImportStats {
    pub sheets_imported: u32,
    pub cells_imported: u64,
}

pub fn import_xlsx_into_doc(doc: &Doc, bytes: &[u8]) -> Result<ImportStats, ImportError> {
    let cursor = Cursor::new(bytes.to_vec());
    let mut wb: Xlsx<_> = open_workbook_from_rs(cursor)?;

    let mut txn = doc.transact_mut();
    let workbook = txn.get_or_insert_map("workbook");

    // Wipe existing sheets.
    let sheets_arr = match workbook.get(&txn, "sheets") {
        Some(yrs::Out::YArray(a)) => {
            while a.len(&txn) > 0 {
                a.remove(&mut txn, 0);
            }
            a
        }
        _ => workbook.insert(&mut txn, "sheets", ArrayPrelim::default()),
    };

    let mut stats = ImportStats { sheets_imported: 0, cells_imported: 0 };
    let sheet_names: Vec<String> = wb.sheet_names().to_vec();
    for sheet_name in sheet_names {
        let range = match wb.worksheet_range(&sheet_name) {
            Ok(r) => r,
            _ => continue,
        };

        let sheet_id = format!("sh_{}", ulid::Ulid::new());
        let sheet_map = sheets_arr.push_back(&mut txn, MapPrelim::default());
        sheet_map.insert(&mut txn, "id", sheet_id.as_str());
        sheet_map.insert(&mut txn, "name", sheet_name.as_str());
        let cells = sheet_map.insert(&mut txn, "cells", MapPrelim::default());

        for (row_offset, row) in range.rows().enumerate() {
            for (col_offset, cell) in row.iter().enumerate() {
                if matches!(cell, Data::Empty) {
                    continue;
                }
                let addr = format_a1(row_offset as u32, col_offset as u32);
                let cell_map = cells.insert(&mut txn, addr.as_str(), MapPrelim::default());
                let (any, t) = datatype_to_any(cell);
                cell_map.insert(&mut txn, "v", any);
                cell_map.insert(&mut txn, "t", t);
                stats.cells_imported += 1;
            }
        }
        stats.sheets_imported += 1;
    }

    Ok(stats)
}

fn format_a1(row: u32, col: u32) -> String {
    let mut s = String::new();
    let mut c = col;
    loop {
        s.insert(0, (b'A' + (c % 26) as u8) as char);
        if c < 26 {
            break;
        }
        c = c / 26 - 1;
    }
    format!("{s}{}", row + 1)
}

fn datatype_to_any(d: &Data) -> (yrs::Any, &'static str) {
    match d {
        Data::String(s) => (yrs::Any::String(s.clone().into()), "s"),
        Data::Float(n) => (yrs::Any::Number(*n), "n"),
        Data::Int(n) => (yrs::Any::Number(*n as f64), "n"),
        Data::Bool(b) => (yrs::Any::Bool(*b), "b"),
        Data::DateTime(dt) => (yrs::Any::Number(dt.as_f64()), "n"),
        Data::Error(_) | Data::Empty => (yrs::Any::Null, "s"),
        Data::DateTimeIso(s) | Data::DurationIso(s) => {
            (yrs::Any::String(s.clone().into()), "s")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crdt_documents::projection::project;

    /// Try a few common paths the test might run from. Returns the first
    /// readable byte stream.
    fn read_fixture() -> Vec<u8> {
        let candidates = [
            "spike/fixtures/test.xlsx",
            "../../spike/fixtures/test.xlsx",
            "../../../spike/fixtures/test.xlsx",
        ];
        for path in &candidates {
            if let Ok(bytes) = std::fs::read(path) {
                return bytes;
            }
        }
        panic!(
            "could not find spike/fixtures/test.xlsx — current dir: {:?}",
            std::env::current_dir()
        );
    }

    #[test]
    fn imports_spike_fixture() {
        let bytes = read_fixture();
        let doc = Doc::new();
        let stats = import_xlsx_into_doc(&doc, &bytes).unwrap();
        assert!(
            stats.sheets_imported >= 1,
            "expected ≥1 sheet, got {}",
            stats.sheets_imported
        );
        assert!(
            stats.cells_imported >= 700,
            "expected ≥700 cells, got {}",
            stats.cells_imported
        );
        let v = project(&doc);
        assert_eq!(v["sheets"][0]["name"], "Hoja1");
        assert_eq!(v["sheets"][0]["cells"]["A3"], "SKU-0001");
    }
}
