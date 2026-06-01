//! Agent-peer mutations. WS-client variant added in Task 9; in-proc
//! variant here (used by `POST /spike/agent-op`).

use serde_json::Value;
use yrs::{Any, Array, ArrayPrelim, Doc, Map, MapPrelim, Transact, WriteTxn};

/// Sanity-check route: mutates the doc directly without going through WS.
///
/// Creates the workbook/sheets/cells structure on demand. Idempotent on
/// the sheet entry (looks up by `sheet_id`; creates if missing).
pub fn apply_set_cell_in_proc(doc: &Doc, sheet_id: &str, addr: &str, value: &Value) {
    let mut txn = doc.transact_mut();
    let workbook = txn.get_or_insert_map("workbook");
    let sheets_arr = match workbook.get(&txn, "sheets") {
        Some(yrs::Out::YArray(a)) => a,
        _ => workbook.insert(&mut txn, "sheets", ArrayPrelim::default()),
    };

    // Find sheet by id, or push a new one.
    let mut sheet_idx: Option<u32> = None;
    for i in 0..sheets_arr.len(&txn) {
        if let Some(yrs::Out::YMap(m)) = sheets_arr.get(&txn, i) {
            if let Some(yrs::Out::Any(yrs::Any::String(s))) = m.get(&txn, "id") {
                if s.as_ref() == sheet_id {
                    sheet_idx = Some(i);
                    break;
                }
            }
        }
    }
    let sheet = match sheet_idx {
        Some(i) => match sheets_arr.get(&txn, i).unwrap() {
            yrs::Out::YMap(m) => m,
            _ => unreachable!(),
        },
        None => {
            let new_sheet = sheets_arr.push_back(&mut txn, MapPrelim::default());
            new_sheet.insert(&mut txn, "id", sheet_id);
            new_sheet.insert(&mut txn, "name", sheet_id);
            new_sheet.insert(&mut txn, "cells", MapPrelim::default());
            new_sheet
        }
    };

    let cells = match sheet.get(&txn, "cells") {
        Some(yrs::Out::YMap(m)) => m,
        _ => sheet.insert(&mut txn, "cells", MapPrelim::default()),
    };

    let cell = cells.insert(&mut txn, addr, MapPrelim::default());
    let (any, type_tag) = json_to_any(value);
    cell.insert(&mut txn, "v", any);
    cell.insert(&mut txn, "t", type_tag);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dag_engine::spike::projection;

    #[test]
    fn set_cell_then_project_reflects_value() {
        let doc = Doc::new();
        apply_set_cell_in_proc(&doc, "s1", "A1", &Value::String("Hola".into()));
        let v = projection::project(&doc);
        eprintln!("projection: {v}");
        assert_eq!(
            v["sheets"][0]["cells"]["A1"],
            serde_json::Value::String("Hola".into())
        );
    }
}

fn json_to_any(v: &Value) -> (Any, &'static str) {
    match v {
        Value::String(s) => (Any::String(s.clone().into()), "s"),
        Value::Number(n) => (
            n.as_f64().map(Any::Number).unwrap_or(Any::Null),
            "n",
        ),
        Value::Bool(b) => (Any::Bool(*b), "b"),
        _ => (Any::Null, "s"),
    }
}
