//! Convert records-style data (output_sheet from `crdt_doc_run_python`)
//! into Y.Doc sheet writes. Owns sheet creation, name collision
//! resolution, and per-cell writes via `apply_set_cell_in_proc`.
//!
//! D-T8: when pandas overwrites a cell that previously held a formula,
//! [`apply_set_cell_in_proc`]'s literal path already strips `f`/`fs` and
//! cascades a recalc through intra-sheet dependents. This module captures
//! the prior formula text BEFORE each write so the caller (the
//! `crdt_doc_run_python` dispatcher) can emit a
//! `formula_replaced_by_literal` CRDT event for downstream consumers.

use crate::crdt_documents::formula_engine::CellResolver;
use crate::crdt_documents::formula_engine_yrs_resolver::YrsResolver;
use crate::crdt_documents::projection;
use crate::crdt_documents::tool_executor::{
    apply_add_sheet, apply_set_cell_in_proc, SetCellWarning,
};
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
    #[error("sheet '{0}' does not exist")]
    SheetNotFound(String),
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
    /// D-T8: cells whose previous content was a formula and were
    /// overwritten with a literal during this write. In the
    /// new-sheet path this is always empty (the sheet was just created
    /// and has no prior formulas), but the field is part of the contract
    /// so callers can route the event-emission code through the same
    /// path regardless of where the writes landed.
    pub formula_replacements: Vec<FormulaReplacement>,
    /// D-T8: total intra-sheet dependent cells re-evaluated as a side
    /// effect of these writes (aggregated across all per-cell
    /// [`apply_set_cell_in_proc`] calls).
    pub cells_recalculated: usize,
    /// D-T8: warnings produced by per-cell
    /// [`apply_set_cell_in_proc`] calls (`NeedsBrowser`, `EvalError`,
    /// `Cycle`, `ParseError`). Always empty for the new-sheet path —
    /// populated only when records contain formulas (relevant to
    /// [`apply_records_to_doc`]).
    pub warnings: Vec<SetCellWarning>,
}

/// D-T8: outcome of a records-to-doc apply. Returned by
/// [`apply_records_to_doc`] so the caller (the `crdt_doc_run_python`
/// dispatcher) can emit `formula_replaced_by_literal` events and surface
/// the cascade recalc count and per-cell warnings in the tool result.
#[derive(Debug, Clone, Default)]
pub struct DfWriterOutcome {
    /// Cells that previously had a formula and were overwritten with a
    /// literal. Caller is expected to emit `formula_replaced_by_literal`
    /// events for each entry.
    pub formula_replacements: Vec<FormulaReplacement>,
    /// Total cells whose formulas were re-evaluated as a result of these
    /// writes.
    pub cells_recalculated: usize,
    /// Warnings produced by per-cell [`apply_set_cell_in_proc`] calls
    /// (e.g. `NeedsBrowser`, `EvalError`, `Cycle`, `ParseError`). Caller
    /// is expected to forward these to the tool result so the agent
    /// can react.
    pub warnings: Vec<SetCellWarning>,
}

/// D-T8: one cell whose formula was replaced by a literal value during a
/// records-to-doc apply.
#[derive(Debug, Clone)]
pub struct FormulaReplacement {
    pub sheet: String,
    pub addr: String,
    pub prior_formula: String,
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

    let mut formula_replacements: Vec<FormulaReplacement> = Vec::new();
    let mut cells_recalculated: usize = 0;
    let mut warnings: Vec<SetCellWarning> = Vec::new();

    // Write column names in row 1.
    for (i, col_name) in columns.iter().enumerate() {
        let addr = format!("{}{}", col_letter(i as u32), 1);
        write_one_cell(
            doc,
            &sheet_id,
            &addr,
            &Value::String(col_name.clone()),
            &mut formula_replacements,
            &mut cells_recalculated,
            &mut warnings,
        );
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
            write_one_cell(
                doc,
                &sheet_id,
                &addr,
                &val,
                &mut formula_replacements,
                &mut cells_recalculated,
                &mut warnings,
            );
        }
    }

    Ok(WriteResult {
        sheet_id,
        resolved_name: resolved_capped,
        n_rows: rows_to_write.len(),
        n_cols: columns.len(),
        truncated_at,
        formula_replacements,
        cells_recalculated,
        warnings,
    })
}

/// D-T8: write a batch of records into an EXISTING sheet, where each
/// record's keys are A1-notation cell addresses and values are the
/// literal values to set. Unlike [`write_records_as_new_sheet`], this
/// function does NOT create the sheet — it writes in-place, which is
/// where formula-replacement detection actually matters (the
/// new-sheet path can never have prior formulas).
///
/// For each cell address about to be written, this function peeks the
/// current `f` value via [`YrsResolver::get_formula`]; if present, the
/// prior formula is recorded in the returned [`DfWriterOutcome`] so the
/// caller can emit a `formula_replaced_by_literal` CRDT event. The
/// actual write goes through [`apply_set_cell_in_proc`], whose literal
/// path already strips `f`/`fs` and cascades a recalc through intra-sheet
/// dependents.
///
/// Null values in a record are skipped (matches the new-sheet path).
pub fn apply_records_to_doc(
    doc: &Doc,
    sheet_id: &str,
    records: &[Map<String, Value>],
) -> Result<DfWriterOutcome, WriterError> {
    if sheet_id.is_empty() {
        return Err(WriterError::EmptyName);
    }
    // Reject writes to a sheet that doesn't exist. Without this guard,
    // `apply_set_cell_in_proc` would silently create the sheet via its
    // lazy `get_or_insert` logic — undesired in the pandas write-back
    // path (typo / race condition → silent shadow sheet).
    //
    // Scoped so the read txn drops before any per-cell write opens its
    // own mut txn downstream.
    {
        let resolver = YrsResolver::new(doc);
        if !resolver.sheet_exists(sheet_id) {
            return Err(WriterError::SheetNotFound(sheet_id.to_string()));
        }
    }
    let mut outcome = DfWriterOutcome::default();
    for record in records {
        for (addr, val) in record {
            if val.is_null() {
                continue;
            }
            write_one_cell(
                doc,
                sheet_id,
                addr,
                val,
                &mut outcome.formula_replacements,
                &mut outcome.cells_recalculated,
                &mut outcome.warnings,
            );
        }
    }
    Ok(outcome)
}

/// Internal helper: peek prior formula, write the cell, accumulate the
/// replacement entry + recalc count. Used by both the new-sheet writer
/// and [`apply_records_to_doc`].
fn write_one_cell(
    doc: &Doc,
    sheet_id: &str,
    addr: &str,
    value: &Value,
    formula_replacements: &mut Vec<FormulaReplacement>,
    cells_recalculated: &mut usize,
    warnings: &mut Vec<SetCellWarning>,
) {
    // Peek the prior formula (if any) BEFORE the write — once
    // `apply_set_cell_in_proc` takes the literal path, `f`/`fs` are
    // cleared and the prior formula text is gone.
    //
    // If the new value is itself a formula (starts with `=`), this is
    // formula-to-formula, not a "replaced by literal" event — skip the
    // record. The cascade recalc still runs as part of
    // `apply_set_cell_in_proc`.
    let new_is_formula = matches!(value.as_str(), Some(s) if s.starts_with('='));
    if !new_is_formula {
        let prior = {
            let resolver = YrsResolver::new(doc);
            resolver.get_formula(sheet_id, addr)
        };
        if let Some(prior_formula) = prior {
            formula_replacements.push(FormulaReplacement {
                sheet: sheet_id.to_string(),
                addr: addr.to_string(),
                prior_formula,
            });
        }
    }
    let set_outcome = apply_set_cell_in_proc(doc, sheet_id, addr, value);
    *cells_recalculated += set_outcome.cells_recalculated;
    warnings.extend(set_outcome.warnings);
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
    fn overwriting_formula_cell_records_replacement() {
        // D-T8: when pandas overwrites a formula cell with a literal,
        // the prior formula text must be captured in the outcome AND
        // the cell must end up with no `f`/`fs`.
        let doc = Doc::new();
        let _ = apply_set_cell_in_proc(&doc, "Sheet1", "A1", &json!(10));
        let _ = apply_set_cell_in_proc(&doc, "Sheet1", "B1", &json!("=A1*2"));
        // B1 now has f="=A1*2".

        // Pandas overwrites B1 with literal 999 (and re-writes A1).
        let records = vec![make_record(&[("A1", json!(10)), ("B1", json!(999))])];
        let outcome = apply_records_to_doc(&doc, "Sheet1", &records).unwrap();

        // Expect exactly one replacement: B1 with prior_formula "=A1*2".
        assert_eq!(outcome.formula_replacements.len(), 1);
        let r = &outcome.formula_replacements[0];
        assert_eq!(r.sheet, "Sheet1");
        assert_eq!(r.addr, "B1");
        assert_eq!(r.prior_formula, "=A1*2");

        // The cell now has no f/fs.
        use yrs::{Array as _, Map as _, Out, ReadTxn, Transact};
        let txn = doc.transact();
        let workbook = txn.get_map("workbook").unwrap();
        let Out::YArray(sheets) = workbook.get(&txn, "sheets").unwrap() else {
            panic!("sheets")
        };
        let Out::YMap(sheet) = sheets.get(&txn, 0).unwrap() else {
            panic!("sheet0")
        };
        let Out::YMap(cells) = sheet.get(&txn, "cells").unwrap() else {
            panic!("cells")
        };
        let Out::YMap(b1) = cells.get(&txn, "B1").unwrap() else {
            panic!("B1")
        };
        assert!(b1.get(&txn, "f").is_none(), "f should be cleared");
        assert!(b1.get(&txn, "fs").is_none(), "fs should be cleared");
        let Some(Out::Any(yrs::Any::Number(v))) = b1.get(&txn, "v") else {
            panic!("v not Number")
        };
        assert!((v - 999.0).abs() < 1e-9, "v={v}");
    }

    #[test]
    fn overwriting_input_cell_recalculates_dependent_formulas() {
        // D-T8: when pandas overwrites an INPUT cell (a literal that a
        // formula depends on), the dependent formula must recompute via
        // the cascade in `apply_set_cell_in_proc`.
        use crate::crdt_documents::formula_engine::CellResolver;
        use crate::crdt_documents::formula_engine_yrs_resolver::YrsResolver;

        let doc = Doc::new();
        let _ = apply_set_cell_in_proc(&doc, "Sheet1", "A1", &json!(10));
        let _ = apply_set_cell_in_proc(&doc, "Sheet1", "B1", &json!("=A1*2")); // -> 20

        // Pandas overwrites A1 only with literal 50 — B1's formula
        // stays untouched and must recompute to 100 via the cascade.
        let records = vec![make_record(&[("A1", json!(50))])];
        let outcome = apply_records_to_doc(&doc, "Sheet1", &records).unwrap();

        // B1 must have recalculated to 100 (the dependent formula refreshed).
        let r = YrsResolver::new(&doc);
        let b1 = r.get("Sheet1", "B1").unwrap();
        assert_eq!(b1.v.as_f64(), Some(100.0));

        // outcome.cells_recalculated >= 1 (B1 recalculated).
        assert!(
            outcome.cells_recalculated >= 1,
            "got {}",
            outcome.cells_recalculated
        );
        // No formula replacements — A1 was a literal, not a formula.
        assert!(outcome.formula_replacements.is_empty());
    }

    #[test]
    fn apply_records_to_doc_rejects_empty_sheet_id() {
        let doc = Doc::new();
        let err = apply_records_to_doc(&doc, "", &[]).unwrap_err();
        assert!(matches!(err, WriterError::EmptyName));
    }

    #[test]
    fn apply_records_to_doc_skips_null_values_and_does_not_strip_existing_formula() {
        // D-T8: a null in the record means "don't touch this cell".
        // Verify the existing formula at that address is preserved.
        let doc = Doc::new();
        let _ = apply_set_cell_in_proc(&doc, "Sheet1", "A1", &json!(7));
        let _ = apply_set_cell_in_proc(&doc, "Sheet1", "B1", &json!("=A1*3")); // -> 21

        let records = vec![make_record(&[("B1", json!(null))])];
        let outcome = apply_records_to_doc(&doc, "Sheet1", &records).unwrap();
        assert!(outcome.formula_replacements.is_empty());
        assert_eq!(outcome.cells_recalculated, 0);

        // B1's formula still there.
        let resolver = crate::crdt_documents::formula_engine_yrs_resolver::YrsResolver::new(&doc);
        assert_eq!(
            resolver.get_formula("Sheet1", "B1").as_deref(),
            Some("=A1*3")
        );
    }

    #[test]
    fn apply_records_to_doc_propagates_set_cell_warnings() {
        use crate::crdt_documents::tool_executor::{apply_set_cell_in_proc, SetCellWarning};
        let doc = Doc::new();
        // Sheet must exist (fix 2's check rejects missing sheets).
        let _ = apply_set_cell_in_proc(&doc, "Sheet1", "A1", &json!(1));

        // Write a formula with an unknown function → NeedsBrowser warning.
        let records = vec![make_record(&[("A1", json!("=BOGUSFN(1)"))])];
        let outcome = apply_records_to_doc(&doc, "Sheet1", &records).unwrap();
        assert!(
            outcome
                .warnings
                .iter()
                .any(|w| matches!(w, SetCellWarning::NeedsBrowser { .. })),
            "expected NeedsBrowser warning, got {:?}",
            outcome.warnings
        );
    }

    #[test]
    fn apply_records_to_doc_rejects_nonexistent_sheet() {
        let doc = Doc::new();
        let records = vec![make_record(&[("A1", json!(1))])];
        let err = apply_records_to_doc(&doc, "Nonexistent", &records).unwrap_err();
        assert!(
            matches!(err, WriterError::SheetNotFound(_)),
            "expected SheetNotFound, got {err:?}"
        );
    }

    #[test]
    fn overwriting_formula_with_formula_does_not_record_replacement() {
        use crate::crdt_documents::tool_executor::apply_set_cell_in_proc;
        let doc = Doc::new();
        let _ = apply_set_cell_in_proc(&doc, "Sheet1", "A1", &json!(2));
        let _ = apply_set_cell_in_proc(&doc, "Sheet1", "B1", &json!("=A1*2"));

        // Overwrite B1 with a DIFFERENT formula (not a literal).
        let records = vec![make_record(&[("B1", json!("=A1*3"))])];
        let outcome = apply_records_to_doc(&doc, "Sheet1", &records).unwrap();
        // No replacement event — modifying a formula isn't "replaced by literal".
        assert!(outcome.formula_replacements.is_empty());
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
