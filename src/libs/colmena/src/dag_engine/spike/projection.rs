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
        let id = sheet_map
            .get(&txn, "id")
            .and_then(|v| match v {
                yrs::Out::Any(yrs::Any::String(s)) => Some(s.to_string()),
                _ => None,
            })
            .unwrap_or_default();
        let name = sheet_map
            .get(&txn, "name")
            .and_then(|v| match v {
                yrs::Out::Any(yrs::Any::String(s)) => Some(s.to_string()),
                _ => None,
            })
            .unwrap_or_default();

        let cells_map = match sheet_map.get(&txn, "cells") {
            Some(yrs::Out::YMap(m)) => m,
            _ => {
                sheets_out.push(json!({ "id": id, "name": name, "cells": {} }));
                continue;
            }
        };

        let mut cells_out = serde_json::Map::new();
        for (addr, cell_val) in cells_map.iter(&txn) {
            let cell_map = match cell_val {
                yrs::Out::YMap(m) => m,
                _ => continue,
            };
            let v = match cell_map.get(&txn, "v") {
                Some(yrs::Out::Any(any)) => any_to_json(&any),
                _ => continue,
            };
            cells_out.insert(addr.to_string(), v);
        }
        sheets_out.push(json!({ "id": id, "name": name, "cells": cells_out }));
    }

    json!({ "sheets": sheets_out })
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use yrs::{Array, Map, Transact, WriteTxn};

    #[test]
    fn empty_doc() {
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
}
