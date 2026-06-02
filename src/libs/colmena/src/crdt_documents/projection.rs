//! Pure projection from a `yrs::Doc` (Excel-shaped) into the minimal IR JSON
//! used by colmena's documents library. Read-only. Spec §4.3.

use serde_json::{json, Value};
use yrs::{Array, Doc, Map, ReadTxn, Transact};

/// Project the current state of `doc` to the minimal IR JSON.
///
/// Returns `{ "sheets": [ { "id", "name", "cells": { addr: value } } ] }`.
/// Cells whose payload is malformed (missing `v`) are skipped silently.
pub fn project(doc: &Doc) -> Value {
    let txn = doc.transact();
    let workbook = match txn.get_map("workbook") {
        Some(m) => m,
        None => return json!({ "sheets": [] }),
    };
    let sheets_arr = match workbook.get(&txn, "sheets") {
        Some(yrs::Out::YArray(a)) => a,
        _ => return json!({ "sheets": [] }),
    };

    let mut sheets_out = Vec::with_capacity(sheets_arr.len(&txn) as usize);
    for i in 0..sheets_arr.len(&txn) {
        let sheet_map = match sheets_arr.get(&txn, i) {
            Some(yrs::Out::YMap(m)) => m,
            _ => continue,
        };
        sheets_out.push(project_sheet(&txn, &sheet_map));
    }

    json!({ "sheets": sheets_out })
}

/// Project a single sheet's IR (used by `project` for each sheet, and by
/// `tool_executor::apply_reorder_sheets` to snapshot before reordering —
/// it currently inlines a near-identical helper, to be replaced post-Task 4).
pub(crate) fn project_sheet<T: yrs::ReadTxn>(
    txn: &T,
    sheet_map: &yrs::MapRef,
) -> serde_json::Value {
    use yrs::Map;
    let id = sheet_map
        .get(txn, "id")
        .and_then(|v| match v {
            yrs::Out::Any(yrs::Any::String(s)) => Some(s.to_string()),
            _ => None,
        })
        .unwrap_or_default();
    let name = sheet_map
        .get(txn, "name")
        .and_then(|v| match v {
            yrs::Out::Any(yrs::Any::String(s)) => Some(s.to_string()),
            _ => None,
        })
        .unwrap_or_default();
    let cells_map = match sheet_map.get(txn, "cells") {
        Some(yrs::Out::YMap(m)) => m,
        _ => return serde_json::json!({ "id": id, "name": name, "cells": {} }),
    };
    let mut cells_out = serde_json::Map::new();
    for (addr, cell_val) in cells_map.iter(txn) {
        let cell_map = match cell_val {
            yrs::Out::YMap(m) => m,
            _ => continue,
        };
        let v = match cell_map.get(txn, "v") {
            Some(yrs::Out::Any(any)) => any_to_json(&any),
            _ => continue,
        };
        cells_out.insert(addr.to_string(), v);
    }
    serde_json::json!({ "id": id, "name": name, "cells": cells_out })
}

fn any_to_json(any: &yrs::Any) -> Value {
    match any {
        yrs::Any::Null | yrs::Any::Undefined => Value::Null,
        yrs::Any::Bool(b) => Value::Bool(*b),
        yrs::Any::Number(n) => json!(n),
        yrs::Any::BigInt(n) => json!(n),
        yrs::Any::String(s) => Value::String(s.to_string()),
        _ => Value::Null,
    }
}

#[cfg(test)]
mod test_helpers {
    use yrs::{Any, Array, ArrayPrelim, Doc, Map, MapPrelim, Transact, WriteTxn};

    pub fn seed_simple(doc: &Doc, sheet_id: &str, sheet_name: &str, cells: &[(&str, &str)]) {
        let mut txn = doc.transact_mut();
        let workbook = txn.get_or_insert_map("workbook");
        let sheets_arr = workbook.insert(&mut txn, "sheets", ArrayPrelim::default());
        let sheet = sheets_arr.push_back(&mut txn, MapPrelim::default());
        sheet.insert(&mut txn, "id", sheet_id);
        sheet.insert(&mut txn, "name", sheet_name);
        let cells_map = sheet.insert(&mut txn, "cells", MapPrelim::default());
        for (addr, val) in cells {
            let cell = cells_map.insert(&mut txn, *addr, MapPrelim::default());
            cell.insert(&mut txn, "v", Any::String((*val).into()));
            cell.insert(&mut txn, "t", Any::String("s".into()));
        }
    }

    /// Seeds a sheet with `n` cells alternating string and number values.
    pub fn seed_n_cells(doc: &Doc, n: usize) {
        let mut txn = doc.transact_mut();
        let workbook = txn.get_or_insert_map("workbook");
        let sheets_arr = workbook.insert(&mut txn, "sheets", ArrayPrelim::default());
        let sheet = sheets_arr.push_back(&mut txn, MapPrelim::default());
        sheet.insert(&mut txn, "id", "s1");
        sheet.insert(&mut txn, "name", "Hoja1");
        let cells_map = sheet.insert(&mut txn, "cells", MapPrelim::default());
        for i in 0..n {
            let row = (i / 26) + 1;
            let col = (b'A' + (i % 26) as u8) as char;
            let addr = format!("{}{}", col, row);
            let cell = cells_map.insert(&mut txn, addr.as_str(), MapPrelim::default());
            if i % 2 == 0 {
                cell.insert(&mut txn, "v", Any::String(format!("val_{i}").into()));
                cell.insert(&mut txn, "t", Any::String("s".into()));
            } else {
                cell.insert(&mut txn, "v", Any::Number(i as f64));
                cell.insert(&mut txn, "t", Any::String("n".into()));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use yrs::{Array, Map, Transact, WriteTxn};

    #[test]
    fn empty_doc_projects_to_empty_sheets() {
        let doc = Doc::new();
        let v = project(&doc);
        assert_eq!(v, json!({ "sheets": [] }));
    }

    #[test]
    fn projects_single_sheet_with_string_cells() {
        let doc = Doc::new();
        test_helpers::seed_simple(&doc, "s1", "Hoja1", &[("A1", "Hola"), ("B1", "Mundo")]);
        let v = project(&doc);
        assert_eq!(
            v,
            json!({
                "sheets": [
                    { "id": "s1", "name": "Hoja1",
                      "cells": { "A1": "Hola", "B1": "Mundo" } }
                ]
            })
        );
    }

    #[test]
    fn malformed_cell_without_v_is_skipped() {
        let doc = Doc::new();
        {
            let mut txn = doc.transact_mut();
            let workbook = txn.get_or_insert_map("workbook");
            let sheets_arr = workbook.insert(&mut txn, "sheets", yrs::ArrayPrelim::default());
            let sheet = sheets_arr.push_back(&mut txn, yrs::MapPrelim::default());
            sheet.insert(&mut txn, "id", "s1");
            sheet.insert(&mut txn, "name", "Hoja1");
            let cells_map = sheet.insert(&mut txn, "cells", yrs::MapPrelim::default());
            let bad = cells_map.insert(&mut txn, "A1", yrs::MapPrelim::default());
            bad.insert(&mut txn, "t", yrs::Any::String("s".into()));
        }
        let v = project(&doc);
        assert_eq!(v["sheets"][0]["cells"], json!({}));
    }

    #[test]
    fn projects_multiple_sheets() {
        let doc = Doc::new();
        test_helpers::seed_simple(&doc, "s1", "Sales", &[("A1", "Apple")]);
        // Append a second sheet manually inside a fresh transaction.
        {
            use yrs::{Any, Map, MapPrelim, Transact, WriteTxn};
            let mut txn = doc.transact_mut();
            let wb = txn.get_or_insert_map("workbook");
            let sheets = match wb.get(&txn, "sheets") {
                Some(yrs::Out::YArray(a)) => a,
                _ => unreachable!(),
            };
            let s = sheets.push_back(&mut txn, MapPrelim::default());
            s.insert(&mut txn, "id", "s2");
            s.insert(&mut txn, "name", "Summary");
            let cells = s.insert(&mut txn, "cells", MapPrelim::default());
            let c = cells.insert(&mut txn, "B2", MapPrelim::default());
            c.insert(&mut txn, "v", Any::Number(42.0));
            c.insert(&mut txn, "t", Any::String("n".into()));
        }
        let v = project(&doc);
        assert_eq!(v["sheets"].as_array().unwrap().len(), 2);
        assert_eq!(v["sheets"][0]["name"], "Sales");
        assert_eq!(v["sheets"][1]["name"], "Summary");
        assert_eq!(v["sheets"][1]["cells"]["B2"], serde_json::json!(42.0));
    }

    #[test]
    #[ignore = "R2.1 benchmark — run with `cargo test -- --ignored --nocapture`"]
    fn r2_1_benchmark_1000_cells_p50_under_50ms() {
        use std::time::Instant;
        let doc = Doc::new();
        test_helpers::seed_n_cells(&doc, 1000);

        let mut samples: Vec<u128> = Vec::with_capacity(100);
        for _ in 0..100 {
            let start = Instant::now();
            let _ = project(&doc);
            samples.push(start.elapsed().as_micros());
        }
        samples.sort_unstable();
        let p50_us = samples[50];
        let p95_us = samples[95];
        println!(
            "projection p50 = {:.2}ms, p95 = {:.2}ms (1000 cells, 100 runs)",
            p50_us as f64 / 1000.0,
            p95_us as f64 / 1000.0
        );
        // R2.1 GO threshold: p50 < 50ms.
        assert!(
            p50_us < 50_000,
            "p50 was {}us, want < 50000us (50ms)",
            p50_us
        );
    }
}
