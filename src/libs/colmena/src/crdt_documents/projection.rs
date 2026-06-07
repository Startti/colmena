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

/// Formula-aware projection of a single sheet's cells.
///
/// Returns a flat A1-keyed map (same key shape as the default `project_sheet`
/// `cells` field). Each value is an object `{v}` for literal cells, or
/// `{v, f, fs}` when the cell carries formula metadata. Used by
/// `crdt_doc_read(include_formulas=true)` so the agent can audit or inspect
/// formulas without changing the default (scalar / pandas-friendly) shape.
///
/// Returns an empty map if `sheet_id` is not present in the workbook.
pub fn project_sheet_cells_with_formulas(
    doc: &Doc,
    sheet_id: &str,
) -> serde_json::Map<String, Value> {
    let txn = doc.transact();
    let mut out = serde_json::Map::new();
    let Some(workbook) = txn.get_map("workbook") else {
        return out;
    };
    let Some(yrs::Out::YArray(sheets)) = workbook.get(&txn, "sheets") else {
        return out;
    };
    for i in 0..sheets.len(&txn) {
        let Some(yrs::Out::YMap(sheet)) = sheets.get(&txn, i) else {
            continue;
        };
        let id_matches = matches!(
            sheet.get(&txn, "id"),
            Some(yrs::Out::Any(yrs::Any::String(ref s))) if s.as_ref() == sheet_id
        );
        if !id_matches {
            continue;
        }
        let Some(yrs::Out::YMap(cells)) = sheet.get(&txn, "cells") else {
            return out;
        };
        for (addr, cell_val) in cells.iter(&txn) {
            let yrs::Out::YMap(cell_map) = cell_val else {
                continue;
            };
            // Skip cells without `v` to match `project_sheet`'s contract
            // (malformed cells are silently dropped).
            let v = match cell_map.get(&txn, "v") {
                Some(yrs::Out::Any(any)) => any_to_json(&any),
                _ => continue,
            };
            let mut entry = serde_json::Map::new();
            entry.insert("v".to_string(), v);
            if let Some(yrs::Out::Any(yrs::Any::String(f))) = cell_map.get(&txn, "f") {
                entry.insert("f".to_string(), Value::String(f.to_string()));
            }
            if let Some(yrs::Out::Any(yrs::Any::String(fs))) = cell_map.get(&txn, "fs") {
                entry.insert("fs".to_string(), Value::String(fs.to_string()));
            }
            out.insert(addr.to_string(), Value::Object(entry));
        }
        break;
    }
    out
}

/// Count the number of cells in `sheet_id` that carry a non-empty formula
/// (i.e. their YMap has a string `f` key). Returns `0` when the sheet doesn't
/// exist or the workbook has no `sheets` array. Used by
/// `crdt_doc_list_sheets` so the agent can decide whether to pay the cost
/// of `crdt_doc_read(include_formulas=true)` on a subsequent call.
pub fn count_formulas_in_sheet(doc: &Doc, sheet_id: &str) -> u32 {
    let txn = doc.transact();
    let Some(workbook) = txn.get_map("workbook") else {
        return 0;
    };
    let Some(yrs::Out::YArray(sheets)) = workbook.get(&txn, "sheets") else {
        return 0;
    };
    for i in 0..sheets.len(&txn) {
        let Some(yrs::Out::YMap(sheet)) = sheets.get(&txn, i) else {
            continue;
        };
        let id_matches = matches!(
            sheet.get(&txn, "id"),
            Some(yrs::Out::Any(yrs::Any::String(ref s))) if s.as_ref() == sheet_id
        );
        if !id_matches {
            continue;
        }
        let Some(yrs::Out::YMap(cells)) = sheet.get(&txn, "cells") else {
            return 0;
        };
        let mut n = 0u32;
        for (_addr, cell_val) in cells.iter(&txn) {
            let yrs::Out::YMap(cell_map) = cell_val else {
                continue;
            };
            if let Some(yrs::Out::Any(yrs::Any::String(f))) = cell_map.get(&txn, "f") {
                if !f.is_empty() {
                    n += 1;
                }
            }
        }
        return n;
    }
    0
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

#[cfg(test)]
mod formula_projection_tests {
    use super::*;
    use crate::crdt_documents::tool_executor::{apply_add_sheet, apply_set_cell_in_proc};
    use yrs::Doc;

    #[test]
    fn project_with_formulas_emits_v_f_fs() {
        // Seed: A1=5 (literal), B1="=A1*2" (backend-eval formula).
        let doc = Doc::new();
        let sheet_id = apply_add_sheet(&doc, "Sheet1");
        let _ = apply_set_cell_in_proc(&doc, &sheet_id, "A1", &serde_json::json!(5));
        let _ = apply_set_cell_in_proc(&doc, &sheet_id, "B1", &serde_json::json!("=A1*2"));

        let cells = project_sheet_cells_with_formulas(&doc, &sheet_id);
        assert_eq!(cells.len(), 2, "expected A1 and B1, got {cells:?}");

        // A1 has no formula → just {v}.
        let a1 = cells["A1"].as_object().expect("A1 is object");
        assert_eq!(a1.get("v").and_then(|v| v.as_f64()), Some(5.0));
        assert!(a1.get("f").is_none(), "A1 should not have 'f'");
        assert!(a1.get("fs").is_none(), "A1 should not have 'fs'");

        // B1 has a formula → {v, f, fs}.
        let b1 = cells["B1"].as_object().expect("B1 is object");
        assert_eq!(
            b1.get("v").and_then(|v| v.as_f64()),
            Some(10.0),
            "B1 v should be evaluated 10.0"
        );
        assert_eq!(b1.get("f").and_then(|v| v.as_str()), Some("=A1*2"));
        assert_eq!(
            b1.get("fs").and_then(|v| v.as_str()),
            Some("be"),
            "B1 fs should be 'be' (backend-evaluated)"
        );
    }

    #[test]
    fn project_with_formulas_empty_sheet_returns_empty() {
        let doc = Doc::new();
        let out = project_sheet_cells_with_formulas(&doc, "nonexistent");
        assert!(out.is_empty());
    }

    #[test]
    fn project_with_formulas_unknown_sheet_id_returns_empty() {
        let doc = Doc::new();
        let real_sheet_id = apply_add_sheet(&doc, "Real");
        let _ = apply_set_cell_in_proc(&doc, &real_sheet_id, "A1", &serde_json::json!(1));
        let out = project_sheet_cells_with_formulas(&doc, "sh_does_not_exist");
        assert!(out.is_empty());
    }
}
