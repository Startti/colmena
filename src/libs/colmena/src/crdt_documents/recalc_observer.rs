//! D-T15: Server-side recalc observer for browser-originated edits.
//!
//! When the browser (or any WS peer) writes a cell via WS sync, our
//! [`crate::crdt_documents::tool_executor::apply_set_cell_in_proc`] path
//! is **not** invoked, so dependent formulas don't auto-recalculate.
//! This observer fills the gap: attached to each `yrs::Doc`, it fires
//! after every committed transaction, skips writes the server
//! originated (tagged with [`SERVER_TX_ORIGIN`]), and runs
//! [`recompute_dependent`] for every formula cell that may have been
//! invalidated.
//!
//! **Loop prevention.** Writes produced by `write_cell_raw` (inside
//! `apply_set_cell_in_proc`) and by `recompute_dependent` itself are
//! tagged with [`SERVER_TX_ORIGIN`] via
//! [`yrs::Transact::transact_mut_with`]. The observer's first action
//! is to inspect `txn.origin()` and return early when the tag matches,
//! so server-originated writes never re-fire the observer.
//!
//! **Pragmatic v1.** Rather than walk `txn.changed_parent_types()` to
//! pinpoint exactly which cells the peer mutated (gnarly in yrs 0.26
//! because each cell is its own nested `Y.Map`), the observer simply
//! enumerates every formula cell in the workbook and re-runs its
//! formula. `recompute_dependent` is a no-op for non-formula cells.
//! For typical spreadsheet sizes (<10K cells) the overhead is
//! negligible; optimise to per-changed-cell when this becomes a hot
//! path.
//!
//! See `docs/superpowers/plans/2026-06-05-crdt-formulas-implementation-plan.md`
//! (D-T15).

use std::sync::Arc;

use yrs::{Doc, ReadTxn, Subscription, TransactionAcqError, TransactionMut};

use crate::crdt_documents::tool_executor::recompute_dependent;

/// Binary origin tag stamped on every write the server makes through
/// [`crate::crdt_documents::tool_executor::apply_set_cell_in_proc`] and
/// [`recompute_dependent`]. The recalc observer skips transactions
/// whose `origin()` matches this tag so server-driven recalcs don't
/// recursively re-fire the observer.
pub const SERVER_TX_ORIGIN: &[u8] = b"colmena:server:recalc";

/// Attach a recalc observer to a shared [`yrs::Doc`]. The returned
/// [`Subscription`] **must be kept alive** for the observer to fire —
/// dropping it unsubscribes.
///
/// Holds a strong `Arc<Doc>` clone so the observer closure can open
/// transactions to read the workbook structure and write recalculated
/// cell values.
pub fn attach_recalc_observer(doc: Arc<Doc>) -> Result<Subscription, TransactionAcqError> {
    let doc_for_observer = Arc::clone(&doc);
    doc.observe_after_transaction(move |txn: &mut TransactionMut<'_>| {
        // 1. Skip our own writes.
        if let Some(origin) = txn.origin() {
            if origin.as_ref() == SERVER_TX_ORIGIN {
                return;
            }
        }

        // 2. Snapshot every (sheet_id, addr) that owns a formula, using the
        //    current txn's read view. We don't try to be surgical about
        //    *which* cells changed: walking `changed_parent_types` to map
        //    nested branch pointers back to (sheet, addr) is nontrivial in
        //    yrs 0.26, and `recompute_dependent` is idempotent. For typical
        //    sheets this is O(formulas) per browser write, which is fine.
        let formulas: Vec<(String, String)> = collect_all_formula_addrs(txn);
        if formulas.is_empty() {
            return;
        }

        // 3. Defer the recomputes to a background thread.
        //
        //    Why: `observe_after_transaction` fires from inside
        //    `TransactionMut::commit()`, which means the store's RwLock is
        //    still held in exclusive (write) mode by the txn we received.
        //    Opening another `transact_mut_with` on `doc_for_observer` from
        //    this callback would deadlock — the new txn would block waiting
        //    for the lock the current txn hasn't released yet.
        //
        //    By moving the recompute work onto a fresh thread, we let the
        //    triggering txn finish dropping (releasing the lock) and then
        //    the thread re-acquires the lock to apply the cascade. The
        //    writes are tagged with `SERVER_TX_ORIGIN` (inside
        //    `recompute_dependent` → `write_cell_raw`), so the observer
        //    skips them — no loop.
        //
        //    The thread is detached. If the Doc is dropped before it runs,
        //    the strong `Arc` clone keeps the Doc alive long enough for the
        //    recompute to finish; the resulting writes are harmless on a
        //    Doc no longer in the registry.
        let doc_for_thread = Arc::clone(&doc_for_observer);
        std::thread::spawn(move || {
            for (sh, ad) in formulas {
                recompute_dependent(&doc_for_thread, &sh, &ad);
            }
        });
    })
}

/// Enumerate every `(sheet_id, addr)` that has a non-empty `f` key.
/// Reads from the txn (which is a `ReadTxn` for any committed
/// `TransactionMut`).
fn collect_all_formula_addrs<T: ReadTxn>(txn: &T) -> Vec<(String, String)> {
    use yrs::{Any, Array, Map, Out};

    let mut out = Vec::new();
    let Some(workbook) = txn.get_map("workbook") else {
        return out;
    };
    let Some(Out::YArray(sheets)) = workbook.get(txn, "sheets") else {
        return out;
    };
    for i in 0..sheets.len(txn) {
        let Some(Out::YMap(sheet)) = sheets.get(txn, i) else {
            continue;
        };
        let Some(Out::Any(Any::String(id))) = sheet.get(txn, "id") else {
            continue;
        };
        let sheet_id = id.to_string();
        let Some(Out::YMap(cells)) = sheet.get(txn, "cells") else {
            continue;
        };
        for (addr, cell_val) in cells.iter(txn) {
            let Out::YMap(cell) = cell_val else { continue };
            if matches!(cell.get(txn, "f"), Some(Out::Any(Any::String(_)))) {
                out.push((sheet_id.clone(), addr.to_string()));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use serde_json::json;
    use yrs::{Any, Array, Map, MapPrelim, Out, ReadTxn, Transact, WriteTxn};

    use super::*;
    use crate::crdt_documents::formula_engine::CellResolver;
    use crate::crdt_documents::formula_engine_yrs_resolver::YrsResolver;
    use crate::crdt_documents::tool_executor::apply_set_cell_in_proc;

    /// Simulate a browser-originated cell write: open an *untagged*
    /// transaction directly and mutate `v` (and `t`) on an existing cell
    /// map, mirroring what the WS sync handler applies when an
    /// update frame lands.
    fn simulate_browser_write(doc: &Doc, sheet_id: &str, addr: &str, value: f64) {
        let mut txn = doc.transact_mut(); // no origin tag → looks like remote.
        let workbook = txn
            .get_map("workbook")
            .expect("workbook must exist for simulate_browser_write");
        let Some(Out::YArray(sheets)) = workbook.get(&txn, "sheets") else {
            panic!("sheets array missing");
        };
        let mut found: Option<yrs::MapRef> = None;
        for i in 0..sheets.len(&txn) {
            let Some(Out::YMap(sheet)) = sheets.get(&txn, i) else {
                continue;
            };
            let Some(Out::Any(Any::String(id))) = sheet.get(&txn, "id") else {
                continue;
            };
            if id.as_ref() == sheet_id {
                let Some(Out::YMap(cells)) = sheet.get(&txn, "cells") else {
                    panic!("cells missing for sheet");
                };
                let cell = match cells.get(&txn, addr) {
                    Some(Out::YMap(c)) => c,
                    _ => cells.insert(&mut txn, addr, MapPrelim::default()),
                };
                found = Some(cell);
                break;
            }
        }
        let cell = found.expect("sheet/cell must exist");
        cell.insert(&mut txn, "v", Any::Number(value));
        cell.insert(&mut txn, "t", Any::BigInt(2));
    }

    /// Block until `cell.v` equals `expected` or the deadline expires.
    /// The recalc observer dispatches work to a background thread, so the
    /// effect on a dependent cell isn't synchronous with the triggering
    /// write — we poll for a bounded interval before failing the test.
    fn wait_for_value(doc: &Doc, sheet: &str, addr: &str, expected: f64, timeout_ms: u64) -> f64 {
        let start = std::time::Instant::now();
        loop {
            let r = YrsResolver::new(doc);
            if let Some(cell) = r.get(sheet, addr) {
                if let Some(v) = cell.v.as_f64() {
                    if (v - expected).abs() < f64::EPSILON {
                        return v;
                    }
                    if start.elapsed().as_millis() as u64 >= timeout_ms {
                        return v;
                    }
                }
            }
            if start.elapsed().as_millis() as u64 >= timeout_ms {
                panic!("timed out waiting for {sheet}!{addr} == {expected}");
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    #[test]
    fn observer_recalcs_dependents_on_browser_write() {
        let doc = Arc::new(Doc::new());
        // Seed via the tagged path: observer (not yet attached) wouldn't
        // see these anyway.
        let _ = apply_set_cell_in_proc(&doc, "Sheet1", "A1", &json!(10));
        let _ = apply_set_cell_in_proc(&doc, "Sheet1", "B1", &json!("=A1*2"));

        // Sanity: B1 == 20 after the seed.
        {
            let r = YrsResolver::new(&doc);
            assert_eq!(r.get("Sheet1", "B1").unwrap().v.as_f64(), Some(20.0));
        }

        // Attach observer. Subscription must live for the rest of the test.
        let _sub = attach_recalc_observer(Arc::clone(&doc)).expect("attach observer");

        // Simulate a browser write: A1 = 50 via an untagged transaction.
        simulate_browser_write(&doc, "Sheet1", "A1", 50.0);

        // Observer dispatches recalc on a background thread — wait for it.
        let got = wait_for_value(&doc, "Sheet1", "B1", 100.0, 2000);
        assert_eq!(
            got, 100.0,
            "observer must recompute B1 after browser-originated A1 edit"
        );
    }

    #[test]
    fn observer_skips_server_originated_writes() {
        // Counter that increments every time the observer body runs past
        // the origin check (i.e., every time we DON'T skip).
        static OBSERVER_RAN: AtomicUsize = AtomicUsize::new(0);
        OBSERVER_RAN.store(0, Ordering::SeqCst);

        let doc = Arc::new(Doc::new());

        // Custom observer to count fires that DIDN'T skip.
        let _sub = doc
            .observe_after_transaction(move |txn: &mut TransactionMut<'_>| {
                if let Some(origin) = txn.origin() {
                    if origin.as_ref() == SERVER_TX_ORIGIN {
                        return;
                    }
                }
                OBSERVER_RAN.fetch_add(1, Ordering::SeqCst);
            })
            .expect("attach counter observer");

        // Several tagged (server) writes — counter must stay 0.
        let _ = apply_set_cell_in_proc(&doc, "Sheet1", "A1", &json!(1));
        let _ = apply_set_cell_in_proc(&doc, "Sheet1", "B1", &json!("=A1+1"));
        let _ = apply_set_cell_in_proc(&doc, "Sheet1", "A1", &json!(2));
        assert_eq!(
            OBSERVER_RAN.load(Ordering::SeqCst),
            0,
            "tagged writes must not trigger the observer body"
        );

        // One untagged write — counter must increment exactly once.
        simulate_browser_write(&doc, "Sheet1", "A1", 99.0);
        assert_eq!(
            OBSERVER_RAN.load(Ordering::SeqCst),
            1,
            "untagged write must trigger the observer exactly once"
        );
    }

    #[test]
    fn observer_no_op_when_workbook_is_empty() {
        // Edge case: brand-new Doc with no workbook map — observer must
        // not panic when fired against an untagged write.
        let doc = Arc::new(Doc::new());
        let _sub = attach_recalc_observer(Arc::clone(&doc)).expect("attach observer");

        // Open an untagged write txn that touches a *different* top-level
        // map, so the observer fires but `workbook` is still missing.
        {
            let mut txn = doc.transact_mut();
            let m = txn.get_or_insert_map("not_workbook");
            m.insert(&mut txn, "k", "v");
        }
        // If we got here without panicking, we're good.
    }
}
