//! D-T15 production repro: simulate the agent's exact tool sequence and
//! verify f/fs persistence + cascade recalc.
//!
//! Mirrors the live SSE from `crdt-yws-graph`:
//! 1. add_sheet "Ventas Q1"
//! 2. set_range headers (A1:D1)
//! 3. set_range product rows (A2:C5)
//! 4. set_range formulas D2:D5  ("=B2*C2" etc.)
//! 5. set_cell A7 "TOTAL"
//! 6. set_cell D7 "=SUM(D2:D5)"
//! 7. set_cell B2 = 25 → expect cascade recalc of D2 (and transitively D7)
//! 8. read with include_formulas → expect f/fs keys

use colmena::crdt_documents::{
    formula_engine::CellResolver,
    formula_engine_yrs_resolver::YrsResolver,
    recalc_observer::attach_recalc_observer,
    tool_executor::{apply_add_sheet, apply_set_cell_in_proc},
};
use std::sync::Arc;
use yrs::{Any, Array, Doc, Map, Out, ReadTxn, Transact};

fn dump_cell(doc: &Doc, sheet_id: &str, addr: &str, label: &str) {
    let txn = doc.transact();
    let Some(workbook) = txn.get_map("workbook") else {
        println!("{label}: workbook missing");
        return;
    };
    let Some(Out::YArray(sheets)) = workbook.get(&txn, "sheets") else {
        println!("{label}: sheets missing");
        return;
    };
    for i in 0..sheets.len(&txn) {
        let Some(Out::YMap(s)) = sheets.get(&txn, i) else {
            continue;
        };
        let Some(Out::Any(Any::String(id))) = s.get(&txn, "id") else {
            continue;
        };
        if id.as_ref() == sheet_id {
            let Some(Out::YMap(cells)) = s.get(&txn, "cells") else {
                println!("{label}: cells missing");
                return;
            };
            let Some(Out::YMap(cell)) = cells.get(&txn, addr) else {
                println!("{label}: cell {addr} missing");
                return;
            };
            let v = cell.get(&txn, "v");
            let f = cell.get(&txn, "f");
            let fs = cell.get(&txn, "fs");
            let t = cell.get(&txn, "t");
            println!("{label}: {addr} raw: v={v:?} f={f:?} fs={fs:?} t={t:?}");
            return;
        }
    }
    println!("{label}: sheet {sheet_id} not found");
}

#[test]
fn agent_sequence_persists_formulas_and_cascades_recalc() {
    let doc = Arc::new(Doc::new());

    // CRITICAL: production attaches this observer via doc_registry on every
    // artifact. If observer interference is the bug, this test reproduces it.
    let _sub = attach_recalc_observer(Arc::clone(&doc)).expect("attach observer");

    // 1. add_sheet "Ventas Q1"
    let sheet_id = apply_add_sheet(&doc, "Ventas Q1");
    println!("Created sheet: {sheet_id}");

    // 2. header row
    let _ = apply_set_cell_in_proc(&doc, &sheet_id, "A1", &serde_json::json!("Producto"));
    let _ = apply_set_cell_in_proc(&doc, &sheet_id, "B1", &serde_json::json!("Cantidad"));
    let _ = apply_set_cell_in_proc(&doc, &sheet_id, "C1", &serde_json::json!("Precio Unit"));
    let _ = apply_set_cell_in_proc(&doc, &sheet_id, "D1", &serde_json::json!("Subtotal"));

    // 3. product rows (4 rows)
    let _ = apply_set_cell_in_proc(&doc, &sheet_id, "A2", &serde_json::json!("Laptop"));
    let _ = apply_set_cell_in_proc(&doc, &sheet_id, "B2", &serde_json::json!(10));
    let _ = apply_set_cell_in_proc(&doc, &sheet_id, "C2", &serde_json::json!(1200));

    let _ = apply_set_cell_in_proc(&doc, &sheet_id, "A3", &serde_json::json!("Mouse"));
    let _ = apply_set_cell_in_proc(&doc, &sheet_id, "B3", &serde_json::json!(50));
    let _ = apply_set_cell_in_proc(&doc, &sheet_id, "C3", &serde_json::json!(25));

    let _ = apply_set_cell_in_proc(&doc, &sheet_id, "A4", &serde_json::json!("Keyboard"));
    let _ = apply_set_cell_in_proc(&doc, &sheet_id, "B4", &serde_json::json!(30));
    let _ = apply_set_cell_in_proc(&doc, &sheet_id, "C4", &serde_json::json!(75));

    let _ = apply_set_cell_in_proc(&doc, &sheet_id, "A5", &serde_json::json!("Monitor"));
    let _ = apply_set_cell_in_proc(&doc, &sheet_id, "B5", &serde_json::json!(15));
    let _ = apply_set_cell_in_proc(&doc, &sheet_id, "C5", &serde_json::json!(300));

    // 4. formulas D2:D5
    let o_d2 = apply_set_cell_in_proc(&doc, &sheet_id, "D2", &serde_json::json!("=B2*C2"));
    let o_d3 = apply_set_cell_in_proc(&doc, &sheet_id, "D3", &serde_json::json!("=B3*C3"));
    let o_d4 = apply_set_cell_in_proc(&doc, &sheet_id, "D4", &serde_json::json!("=B4*C4"));
    let o_d5 = apply_set_cell_in_proc(&doc, &sheet_id, "D5", &serde_json::json!("=B5*C5"));
    println!(
        "D2..D5 outcomes: recalc_counts=[{},{},{},{}], warnings=[{},{},{},{}]",
        o_d2.cells_recalculated,
        o_d3.cells_recalculated,
        o_d4.cells_recalculated,
        o_d5.cells_recalculated,
        o_d2.warnings.len(),
        o_d3.warnings.len(),
        o_d4.warnings.len(),
        o_d5.warnings.len(),
    );
    dump_cell(&doc, &sheet_id, "D2", "after-D2-write");

    // 5. A7 TOTAL label
    let _ = apply_set_cell_in_proc(&doc, &sheet_id, "A7", &serde_json::json!("TOTAL"));

    // 6. D7 = SUM(D2:D5)
    let o_d7 = apply_set_cell_in_proc(&doc, &sheet_id, "D7", &serde_json::json!("=SUM(D2:D5)"));
    println!(
        "D7 outcome: recalc_count={}, warnings={}",
        o_d7.cells_recalculated,
        o_d7.warnings.len(),
    );
    dump_cell(&doc, &sheet_id, "D7", "after-D7-write");

    // ── ASSERT: D2 must have f="=B2*C2" and fs="be" after write
    {
        let txn = doc.transact();
        let workbook = txn.get_map("workbook").expect("workbook");
        let Out::YArray(sheets) = workbook.get(&txn, "sheets").expect("sheets") else {
            panic!("sheets array missing");
        };
        let mut found_d2 = None;
        for i in 0..sheets.len(&txn) {
            let Some(Out::YMap(s)) = sheets.get(&txn, i) else {
                continue;
            };
            let Some(Out::Any(Any::String(id))) = s.get(&txn, "id") else {
                continue;
            };
            if id.as_ref() == sheet_id {
                let Some(Out::YMap(cells)) = s.get(&txn, "cells") else {
                    panic!("cells map missing for our sheet");
                };
                let Some(Out::YMap(d2)) = cells.get(&txn, "D2") else {
                    panic!("D2 missing from cells");
                };
                found_d2 = Some(d2);
                break;
            }
        }
        let d2 = found_d2.expect("D2 must exist after writing the formula");
        let f_val = d2.get(&txn, "f");
        let fs_val = d2.get(&txn, "fs");
        let v_val = d2.get(&txn, "v");
        println!("D2 raw (assertion phase): v={v_val:?} f={f_val:?} fs={fs_val:?}");
        assert!(
            matches!(f_val, Some(Out::Any(Any::String(_)))),
            "REPRO BUG (a): D2.f missing after formula write. Got: {f_val:?}"
        );
        assert!(
            matches!(fs_val, Some(Out::Any(Any::String(_)))),
            "REPRO BUG (a): D2.fs missing after formula write. Got: {fs_val:?}"
        );
    }

    // 7. Mutate B2 = 25, expect cascade recalc (D2 depends on B2, D7 on D2).
    let o_b2 = apply_set_cell_in_proc(&doc, &sheet_id, "B2", &serde_json::json!(25));
    println!(
        "B2 mutation outcome: cells_recalculated={}, warnings={}",
        o_b2.cells_recalculated,
        o_b2.warnings.len(),
    );
    dump_cell(&doc, &sheet_id, "B2", "after-B2-mut");
    dump_cell(&doc, &sheet_id, "D2", "after-B2-mut");
    dump_cell(&doc, &sheet_id, "D7", "after-B2-mut");

    assert!(
        o_b2.cells_recalculated >= 1,
        "REPRO BUG (b): changing B2 should recompute at least D2. Got: {}",
        o_b2.cells_recalculated
    );

    // 8. After cascade, D2 should be 30000 (25*1200).
    let r = YrsResolver::new(&doc);
    let d2 = r.get(&sheet_id, "D2").expect("D2 must resolve");
    assert_eq!(
        d2.v.as_f64(),
        Some(30000.0),
        "REPRO BUG: D2.v should be 30000 (25*1200) after B2 mutation. Got: {:?}",
        d2.v
    );

    // And D7 should reflect the new D2: 30000 + 1250 + 2250 + 4500 = 38000
    let d7 = r.get(&sheet_id, "D7").expect("D7 must resolve");
    println!("D7 final v={:?}", d7.v);
    assert_eq!(
        d7.v.as_f64(),
        Some(38000.0),
        "REPRO BUG: D7 (=SUM(D2:D5)) should be 38000 after B2 mutation. Got: {:?}",
        d7.v
    );

    println!("OK: all assertions passed — bug does NOT reproduce in test conditions");
}
