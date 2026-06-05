//! Production `CellResolver` impl: reads from a `&yrs::Doc`. Kept in its
//! own file so `formula_engine.rs` stays yrs-free and trivially testable.
//!
//! The Y.Doc cell schema is `workbook.sheets[i].cells.<A1> = {v, t, f?, fs?}`
//! — `f`/`fs` are optional and only present once D-T5 starts persisting
//! formula text. Until then this resolver just sees `{v, t}` cells.

use crate::crdt_documents::formula_engine::{CellResolver, CellSnapshot};
use yrs::{Any, Array, Doc, Map, Out, ReadTxn, Transact};

pub struct YrsResolver<'a> {
    doc: &'a Doc,
}

impl<'a> YrsResolver<'a> {
    pub fn new(doc: &'a Doc) -> Self {
        Self { doc }
    }
}

impl<'a> CellResolver for YrsResolver<'a> {
    fn get(&self, sheet: &str, addr: &str) -> Option<CellSnapshot> {
        let txn = self.doc.transact();
        let workbook = txn.get_map("workbook")?;
        let sheets = match workbook.get(&txn, "sheets")? {
            Out::YArray(a) => a,
            _ => return None,
        };
        for i in 0..sheets.len(&txn) {
            let Some(Out::YMap(s)) = sheets.get(&txn, i) else {
                continue;
            };
            let Some(Out::Any(Any::String(id))) = s.get(&txn, "id") else {
                continue;
            };
            if id.as_ref() != sheet {
                continue;
            }
            let Some(Out::YMap(cells)) = s.get(&txn, "cells") else {
                return None;
            };
            let Some(Out::YMap(cell)) = cells.get(&txn, addr) else {
                return None;
            };
            let v = match cell.get(&txn, "v")? {
                Out::Any(a) => any_to_json(&a),
                _ => return None,
            };
            let t = match cell.get(&txn, "t") {
                Some(Out::Any(Any::BigInt(n))) => n as u8,
                Some(Out::Any(Any::Number(n))) => n as u8,
                _ => 1u8,
            };
            return Some(CellSnapshot { v, t });
        }
        None
    }

    fn sheet_exists(&self, sheet: &str) -> bool {
        let txn = self.doc.transact();
        let Some(workbook) = txn.get_map("workbook") else {
            return false;
        };
        let Some(Out::YArray(sheets)) = workbook.get(&txn, "sheets") else {
            return false;
        };
        for i in 0..sheets.len(&txn) {
            if let Some(Out::YMap(s)) = sheets.get(&txn, i) {
                if let Some(Out::Any(Any::String(id))) = s.get(&txn, "id") {
                    if id.as_ref() == sheet {
                        return true;
                    }
                }
            }
        }
        false
    }

    fn iter_formulas_in_sheet<'b>(
        &'b self,
        sheet: &str,
    ) -> Box<dyn Iterator<Item = (String, String)> + 'b> {
        let txn = self.doc.transact();
        let mut out: Vec<(String, String)> = Vec::new();
        let Some(workbook) = txn.get_map("workbook") else {
            return Box::new(out.into_iter());
        };
        let Some(Out::YArray(sheets)) = workbook.get(&txn, "sheets") else {
            return Box::new(out.into_iter());
        };
        for i in 0..sheets.len(&txn) {
            let Some(Out::YMap(s)) = sheets.get(&txn, i) else {
                continue;
            };
            let Some(Out::Any(Any::String(id))) = s.get(&txn, "id") else {
                continue;
            };
            if id.as_ref() != sheet {
                continue;
            }
            let Some(Out::YMap(cells)) = s.get(&txn, "cells") else {
                continue;
            };
            for (key, cell_val) in cells.iter(&txn) {
                let Out::YMap(cell) = cell_val else { continue };
                if let Some(Out::Any(Any::String(f))) = cell.get(&txn, "f") {
                    out.push((key.to_string(), f.to_string()));
                }
            }
        }
        Box::new(out.into_iter())
    }
}

fn any_to_json(a: &Any) -> serde_json::Value {
    match a {
        Any::Null | Any::Undefined => serde_json::Value::Null,
        Any::Bool(b) => serde_json::json!(b),
        Any::Number(n) => serde_json::json!(n),
        Any::BigInt(n) => serde_json::json!(n),
        Any::String(s) => serde_json::json!(s.as_ref()),
        Any::Buffer(_) | Any::Array(_) | Any::Map(_) => serde_json::Value::Null,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crdt_documents::tool_executor::apply_set_cell_in_proc;

    #[test]
    fn yrs_resolver_reads_literal_cell() {
        // Note: yrs stores numbers as f64 via `Any::Number`, so an integer
        // literal round-trips through as `42.0`. Compare against a float to
        // make that explicit (the underlying JSON value is numerically equal
        // but the serde `Number` representation differs Int vs Float).
        let doc = Doc::new();
        let _ = apply_set_cell_in_proc(&doc, "Sheet1", "A1", &serde_json::json!(42));
        let r = YrsResolver::new(&doc);
        let cell = r.get("Sheet1", "A1").expect("A1");
        assert_eq!(cell.v.as_f64(), Some(42.0));
    }

    #[test]
    fn yrs_resolver_reports_sheet_existence() {
        let doc = Doc::new();
        let _ = apply_set_cell_in_proc(&doc, "Sheet1", "A1", &serde_json::json!(1));
        let r = YrsResolver::new(&doc);
        assert!(r.sheet_exists("Sheet1"));
        assert!(!r.sheet_exists("Sheet99"));
    }

    #[test]
    fn yrs_resolver_iter_formulas_returns_empty_when_none() {
        let doc = Doc::new();
        let _ = apply_set_cell_in_proc(&doc, "Sheet1", "A1", &serde_json::json!(1));
        let r = YrsResolver::new(&doc);
        let formulas: Vec<_> = r.iter_formulas_in_sheet("Sheet1").collect();
        assert!(formulas.is_empty());
    }

    #[test]
    fn yrs_resolver_returns_none_for_missing_cell() {
        let doc = Doc::new();
        let r = YrsResolver::new(&doc);
        assert!(r.get("Sheet1", "A1").is_none());
        assert!(!r.sheet_exists("Sheet1"));
    }
}
