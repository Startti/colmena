//! Decode a Yjs update_v1 byte blob and produce a human-readable summary.
//!
//! v1 strategy: replay `before`'s state onto a clone, apply the update on
//! the clone, then diff the IR projections of both. Slow but simple — works
//! for any v1 IR shape without binary-update introspection. v1.1 can read
//! the update structure directly for per-cell deltas.

use crate::crdt_documents::projection::project;
use serde_json::Value;
use yrs::updates::decoder::Decode;
use yrs::{Doc, ReadTxn, Transact, Update};

/// Apply `update_bytes` to a clone of `before`, compute the projection diff,
/// return a summary string suitable for `ChangeTracker.record(...)`.
pub fn narrate(before: &Doc, update_bytes: &[u8]) -> String {
    let after = Doc::new();
    // Replay `before`'s state into `after`.
    let baseline = before
        .transact()
        .encode_state_as_update_v1(&yrs::StateVector::default());
    if let Ok(u) = Update::decode_v1(&baseline) {
        let _ = after.transact_mut().apply_update(u);
    }
    // Then apply the new update.
    if let Ok(u) = Update::decode_v1(update_bytes) {
        let _ = after.transact_mut().apply_update(u);
    }

    let before_proj = project(before);
    let after_proj = project(&after);
    summarize_diff(&before_proj, &after_proj)
}

fn summarize_diff(before: &Value, after: &Value) -> String {
    let before_sheets = sheets_by_id(before);
    let after_sheets = sheets_by_id(after);
    let mut lines: Vec<String> = Vec::new();

    // Detect added sheets.
    for (id, sheet) in &after_sheets {
        if !before_sheets.contains_key(id) {
            lines.push(format!(
                "added sheet '{}'",
                sheet["name"].as_str().unwrap_or("?")
            ));
        }
    }
    // Detect deleted sheets.
    for (id, sheet) in &before_sheets {
        if !after_sheets.contains_key(id) {
            lines.push(format!(
                "deleted sheet '{}'",
                sheet["name"].as_str().unwrap_or("?")
            ));
        }
    }
    // Detect cell-level changes per common sheet.
    for (id, after_sheet) in &after_sheets {
        let Some(before_sheet) = before_sheets.get(id) else {
            continue;
        };
        let name = after_sheet["name"].as_str().unwrap_or("?");
        let bc = before_sheet["cells"]
            .as_object()
            .cloned()
            .unwrap_or_default();
        let ac = after_sheet["cells"]
            .as_object()
            .cloned()
            .unwrap_or_default();
        let mut added: Vec<String> = Vec::new();
        let mut changed: Vec<String> = Vec::new();
        for (addr, av) in &ac {
            match bc.get(addr) {
                None => added.push(format!("{name}!{addr}={av}")),
                Some(bv) if bv != av => changed.push(format!("{name}!{addr}: {bv} → {av}")),
                _ => {}
            }
        }
        if added.len() > 5 {
            lines.push(format!("{} cells added in {name}", added.len()));
        } else {
            lines.extend(added);
        }
        if changed.len() > 5 {
            lines.push(format!("{} cells updated in {name}", changed.len()));
        } else {
            lines.extend(changed);
        }
    }
    if lines.is_empty() {
        "no detectable change".into()
    } else {
        lines.join("; ")
    }
}

fn sheets_by_id(proj: &Value) -> std::collections::HashMap<String, Value> {
    proj["sheets"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|s| s["id"].as_str().map(|id| (id.to_string(), s.clone())))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crdt_documents::tool_executor::{apply_add_sheet, apply_set_cell_in_proc};
    use serde_json::json;

    #[test]
    fn summarises_added_cell() {
        let before = Doc::new();
        let sid = apply_add_sheet(&before, "S");

        // Clone before's state into a working doc and apply a mutation,
        // then capture only the diff update.
        let baseline = before
            .transact()
            .encode_state_as_update_v1(&yrs::StateVector::default());
        let working = Doc::new();
        if let Ok(u) = Update::decode_v1(&baseline) {
            working.transact_mut().apply_update(u).unwrap();
        }
        let sv_before = working.transact().state_vector();
        let _ = apply_set_cell_in_proc(&working, &sid, "A1", &json!("hello"));
        let diff = working.transact().encode_diff_v1(&sv_before);

        let summary = narrate(&before, &diff);
        assert!(summary.contains("A1"));
        assert!(summary.contains("hello"));
    }

    #[test]
    fn summarises_added_sheet() {
        let before = Doc::new();
        let baseline = before
            .transact()
            .encode_state_as_update_v1(&yrs::StateVector::default());
        let working = Doc::new();
        if let Ok(u) = Update::decode_v1(&baseline) {
            working.transact_mut().apply_update(u).unwrap();
        }
        let sv_before = working.transact().state_vector();
        apply_add_sheet(&working, "Sales");
        let diff = working.transact().encode_diff_v1(&sv_before);

        let summary = narrate(&before, &diff);
        assert!(summary.contains("added sheet"));
        assert!(summary.contains("Sales"));
    }

    #[test]
    fn summarises_changed_cell() {
        let before = Doc::new();
        let sid = apply_add_sheet(&before, "S");
        let _ = apply_set_cell_in_proc(&before, &sid, "A1", &json!("old"));

        let baseline = before
            .transact()
            .encode_state_as_update_v1(&yrs::StateVector::default());
        let working = Doc::new();
        if let Ok(u) = Update::decode_v1(&baseline) {
            working.transact_mut().apply_update(u).unwrap();
        }
        let sv_before = working.transact().state_vector();
        let _ = apply_set_cell_in_proc(&working, &sid, "A1", &json!("new"));
        let diff = working.transact().encode_diff_v1(&sv_before);

        let summary = narrate(&before, &diff);
        assert!(summary.contains("A1"));
        assert!(summary.contains("→"));
    }

    #[test]
    fn empty_update_yields_no_detectable_change_message() {
        let before = Doc::new();
        let _sid = apply_add_sheet(&before, "S");
        // An update encoding only the state already known returns essentially no diff.
        let sv = before.transact().state_vector();
        let nothing = before.transact().encode_diff_v1(&sv);
        let summary = narrate(&before, &nothing);
        assert_eq!(summary, "no detectable change");
    }
}
