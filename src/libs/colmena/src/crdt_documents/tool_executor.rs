//! Tool-executor mutations: in-proc helpers that mutate a `yrs::Doc` directly
//! (used by `POST /spike/agent-op` and the multi-sheet LLM tool layer).
//! WS-client variant (`apply_set_cell_via_ws`) stays here for symmetry.

use anyhow::{anyhow, Result};
use serde_json::Value;
use yrs::{Any, Array, ArrayPrelim, Doc, Map, MapPrelim, ReadTxn, Transact, WriteTxn};

use crate::crdt_documents::formula_engine::{
    evaluate, parse, recalc_chain, CellResolver, EvalValue, ExcelError, FormulaSource, ParseOutcome,
};
use crate::crdt_documents::formula_engine_yrs_resolver::YrsResolver;
use crate::crdt_documents::recalc_observer::SERVER_TX_ORIGIN;

/// Outcome of an `apply_set_cell_in_proc` call.
///
/// Returns how many dependent cells were recalculated and any warnings
/// produced (unsupported functions → `NeedsBrowser`, evaluation errors,
/// cycle detection). Callers can attach this to tool results so the agent
/// can react (e.g. surface `NeedsBrowser` cells to the user).
#[derive(Debug, Clone, serde::Serialize, Default)]
#[must_use = "SetCellOutcome carries warnings + recalc counts; explicitly bind or `let _ = ...`"]
pub struct SetCellOutcome {
    pub cells_recalculated: usize,
    pub warnings: Vec<SetCellWarning>,
}

/// Warning produced during a set-cell + recalc operation.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "kind")]
pub enum SetCellWarning {
    /// The formula references at least one function not supported by the
    /// backend formula engine. The cell was persisted as a placeholder
    /// (`v = formula text`, `fs = needs_browser`) so the frontend can
    /// evaluate it.
    #[serde(rename = "needs_browser")]
    NeedsBrowser {
        addr: String,
        functions: Vec<String>,
    },
    /// Evaluation produced an Excel-style error (or an internal failure
    /// mapped to `#VALUE!`).
    #[serde(rename = "eval_error")]
    EvalError { addr: String, error: String },
    /// A cycle was detected while computing the recalc chain. The cells
    /// that could not be ordered are listed in `chain`.
    #[serde(rename = "cycle")]
    Cycle { chain: Vec<(String, String)> },
    /// The formula text starts with `=` but the parser couldn't make
    /// sense of it (malformed syntax, e.g. unclosed parens). The raw
    /// text is still persisted as a literal so the user's input is
    /// preserved; this warning lets the agent surface the parse error.
    #[serde(rename = "parse_error")]
    ParseError { addr: String, error: String },
}

/// Sanity-check route: mutates the doc directly without going through WS.
///
/// Creates the workbook/sheets/cells structure on demand. Idempotent on
/// the sheet entry (looks up by `sheet_id`; creates if missing).
///
/// **Formula support (D-T5):** if `value` is a string starting with `=`,
/// the formula is parsed and evaluated server-side via
/// [`crate::crdt_documents::formula_engine`]. The cell ends up with
/// `{v: <evaluated value>, t: <type>, f: <formula text>, fs: "be"}` on
/// success, or `{v: <formula text>, t: 1, f: <formula text>, fs:
/// "needs_browser"}` if any referenced function is not supported by the
/// backend engine. After a successful write, all intra-sheet dependents
/// are recomputed in topological order.
///
/// Returns a [`SetCellOutcome`] carrying the number of dependents
/// recalculated and any warnings (unsupported functions, eval errors,
/// or cycles).
pub fn apply_set_cell_in_proc(
    doc: &Doc,
    sheet_id: &str,
    addr: &str,
    value: &Value,
) -> SetCellOutcome {
    let mut outcome = SetCellOutcome::default();

    // ── Formula path ────────────────────────────────────────────────
    if let Some(text) = value.as_str().filter(|s| s.starts_with('=')) {
        match parse(text) {
            ParseOutcome::Ok(ast) => {
                // `parse()` only returns `Ok` when every referenced
                // function is supported by formualizer — the
                // unsupported branch is handled by the `NeedsBrowser`
                // arm below.

                // Evaluate against the current doc state.
                let (eval_v, eval_t) = {
                    let resolver = YrsResolver::new(doc);
                    match evaluate(&ast, &resolver, sheet_id) {
                        Ok(EvalValue::Number(n)) => (serde_json::json!(n), 2u8),
                        Ok(EvalValue::String(s)) => (serde_json::json!(s), 1u8),
                        Ok(EvalValue::Bool(b)) => (serde_json::json!(b), 3u8),
                        Ok(EvalValue::Error(err)) => {
                            outcome.warnings.push(SetCellWarning::EvalError {
                                addr: addr.to_string(),
                                error: err.as_excel().to_string(),
                            });
                            (serde_json::json!(err.as_excel()), 4u8)
                        }
                        Err(e) => {
                            outcome.warnings.push(SetCellWarning::EvalError {
                                addr: addr.to_string(),
                                error: format!("internal: {e}"),
                            });
                            (serde_json::json!(ExcelError::Value.as_excel()), 4u8)
                        }
                    }
                };
                write_cell_raw(
                    doc,
                    sheet_id,
                    addr,
                    &eval_v,
                    eval_t,
                    Some(text),
                    Some(FormulaSource::Backend),
                );

                // Recalc downstream dependents.
                let chain = {
                    let resolver = YrsResolver::new(doc);
                    recalc_chain(addr, sheet_id, &resolver)
                };
                match chain {
                    Ok(chain) => {
                        for (sh, ad) in &chain {
                            recompute_dependent(doc, sh, ad);
                        }
                        outcome.cells_recalculated = chain.len();
                    }
                    Err(cycle) => {
                        outcome
                            .warnings
                            .push(SetCellWarning::Cycle { chain: cycle.chain });
                    }
                }
                return outcome;
            }
            ParseOutcome::NeedsBrowser { unsupported_fns } => {
                // `parse()` already detected unsupported functions and
                // refused to return an AST. Persist a `needs_browser`
                // placeholder so the frontend can evaluate it on the
                // client.
                write_cell_raw(
                    doc,
                    sheet_id,
                    addr,
                    &Value::String(text.to_string()),
                    1,
                    Some(text),
                    Some(FormulaSource::NeedsBrowser),
                );
                outcome.warnings.push(SetCellWarning::NeedsBrowser {
                    addr: addr.to_string(),
                    functions: unsupported_fns,
                });
                return outcome;
            }
            ParseOutcome::ParseError(msg) => {
                // Parser couldn't make sense of the text — surface the
                // error to the agent, then fall through to a literal
                // write so the raw text isn't lost.
                outcome.warnings.push(SetCellWarning::ParseError {
                    addr: addr.to_string(),
                    error: msg,
                });
            }
        }
    }

    // ── Literal path ────────────────────────────────────────────────
    let type_tag = json_value_type_tag(value);
    write_cell_raw(doc, sheet_id, addr, value, type_tag, None, None);

    // Even literal writes may invalidate dependents (e.g. setting a number
    // that another cell's formula references). Fast-path: skip the
    // dep-graph walk when the sheet has no formulas at all — important
    // for bulk writers like `df_writer` that may insert 100K+ literal
    // cells in a row.
    if sheet_has_any_formula(doc, sheet_id) {
        let chain = {
            let resolver = YrsResolver::new(doc);
            recalc_chain(addr, sheet_id, &resolver)
        };
        if let Ok(chain) = chain {
            for (sh, ad) in &chain {
                recompute_dependent(doc, sh, ad);
            }
            outcome.cells_recalculated = chain.len();
        }
    }
    outcome
}

/// Cheap probe: does the named sheet have at least one formula cell?
///
/// Reads the `has_formulas` flag we set on the sheet map whenever a
/// formula cell is written via `write_cell_raw`. Once set, the flag is
/// never cleared — a sheet that contained a formula and then had it
/// deleted will still report `true` and pay the cost of an empty
/// dep-graph walk on subsequent literal writes. That false-positive is
/// harmless (a `recalc_chain` that finds no dependents is fast) and
/// avoids the cost of accurate refcounting in the hot path.
///
/// Returns `false` when the sheet doesn't exist or the flag is missing.
fn sheet_has_any_formula(doc: &Doc, sheet_id: &str) -> bool {
    let txn = doc.transact();
    let Some(workbook) = txn.get_map("workbook") else {
        return false;
    };
    let Some(yrs::Out::YArray(sheets)) = workbook.get(&txn, "sheets") else {
        return false;
    };
    for i in 0..sheets.len(&txn) {
        let Some(yrs::Out::YMap(s)) = sheets.get(&txn, i) else {
            continue;
        };
        let Some(yrs::Out::Any(yrs::Any::String(id))) = s.get(&txn, "id") else {
            continue;
        };
        if id.as_ref() != sheet_id {
            continue;
        }
        return matches!(
            s.get(&txn, "has_formulas"),
            Some(yrs::Out::Any(yrs::Any::Bool(true)))
        );
    }
    false
}

/// Internal helper: write `{v, t, f?, fs?}` to a cell, creating the
/// workbook/sheet/cells parents on demand. Idempotent on sheet lookup
/// by id.
///
/// If `formula_text` is `Some`, sets the `f` key; otherwise removes any
/// stale `f` value (turning a former formula cell back into a literal).
/// Same for `fs`.
fn write_cell_raw(
    doc: &Doc,
    sheet_id: &str,
    addr: &str,
    value_json: &Value,
    type_tag: u8,
    formula_text: Option<&str>,
    fs: Option<FormulaSource>,
) {
    // Tag the write so the D-T15 recalc observer skips it (server-
    // originated writes already include the recalc cascade — no need
    // for the observer to fire on its own writes).
    let mut txn = doc.transact_mut_with(SERVER_TX_ORIGIN);
    let workbook = txn.get_or_insert_map("workbook");
    let sheets_arr = match workbook.get(&txn, "sheets") {
        Some(yrs::Out::YArray(a)) => a,
        _ => workbook.insert(&mut txn, "sheets", ArrayPrelim::default()),
    };

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

    // Reuse the existing cell map if present so the CRDT history for
    // `v` / `t` / `f` / `fs` is preserved across overwrites. When the
    // cell is new, fall back to inserting a fresh empty map.
    let cell = match cells.get(&txn, addr) {
        Some(yrs::Out::YMap(c)) => c,
        _ => cells.insert(&mut txn, addr, MapPrelim::default()),
    };
    let (any, _) = json_to_any(value_json);
    cell.insert(&mut txn, "v", any);
    cell.insert(&mut txn, "t", yrs::Any::BigInt(type_tag as i64));

    if let Some(text) = formula_text {
        cell.insert(&mut txn, "f", text.to_string());
        // Sticky flag read by `sheet_has_any_formula` to short-circuit
        // the literal-write recalc walk when no formulas exist.
        sheet.insert(&mut txn, "has_formulas", yrs::Any::Bool(true));
    } else {
        // Load-bearing: clears any stale `f` from a prior formula
        // write so this cell becomes a true literal. `Map::remove`
        // is idempotent on missing keys.
        cell.remove(&mut txn, "f");
    }
    if let Some(src) = fs {
        cell.insert(&mut txn, "fs", src.as_str().to_string());
    } else {
        // Load-bearing: see `f` above.
        cell.remove(&mut txn, "fs");
    }
}

/// Re-evaluate a single dependent cell's formula and write the new value
/// back into the doc. Preserves `f` and `fs` (only `v` and `t` change).
///
/// Made `pub` so D-T8's `df_writer` (which recomputes after batch column
/// writes) can call the same code path.
pub fn recompute_dependent(doc: &Doc, sheet: &str, addr: &str) {
    // Pull the formula text out under a read txn, then drop the resolver
    // before we open a write txn. We collect the iterator into a Vec first
    // because the boxed iterator borrows the resolver, and we need the
    // resolver to drop before returning from this block.
    let formula_text: Option<String> = {
        let resolver = YrsResolver::new(doc);
        let formulas: Vec<(String, String)> = resolver.iter_formulas_in_sheet(sheet).collect();
        formulas
            .into_iter()
            .find(|(a, _)| a == addr)
            .map(|(_, t)| t)
    };
    let Some(ft) = formula_text else { return };
    let ParseOutcome::Ok(ast) = parse(&ft) else {
        return;
    };
    let (dv, dt) = {
        let resolver = YrsResolver::new(doc);
        match evaluate(&ast, &resolver, sheet) {
            Ok(EvalValue::Number(n)) => (serde_json::json!(n), 2u8),
            Ok(EvalValue::String(s)) => (serde_json::json!(s), 1u8),
            Ok(EvalValue::Bool(b)) => (serde_json::json!(b), 3u8),
            Ok(EvalValue::Error(e)) => (serde_json::json!(e.as_excel()), 4u8),
            Err(_) => (serde_json::json!(ExcelError::Value.as_excel()), 4u8),
        }
    };
    write_cell_raw(
        doc,
        sheet,
        addr,
        &dv,
        dt,
        Some(&ft),
        Some(FormulaSource::Backend),
    );
}

/// Connects to `url` as a Yjs WS client, applies `set_cell(sheet_id, addr,
/// value)` to a local `Doc`, and ships the resulting update diff back to the
/// server so all peers converge.
///
/// Full Yjs sync v1 protocol (both sides exchange step1+step2 for proper merge):
///   1. Server → client: `[MSG_SYNC][STEP_1][server_sv]`
///   2. Client → server: `[MSG_SYNC][STEP_1][client_empty_sv]`
///   3. Server → client: `[MSG_SYNC][STEP_2][server_state_for_client]`
///   4. Client imports server state, then applies its mutation.
///   5. Client → server: `[MSG_SYNC][STEP_2][diff_against_server_sv]`
///   6. Client closes after a brief flush pause.
///
/// Steps 2–4 are critical: without importing the server's current CRDT state
/// first, the client's fresh `Doc` creates new conflicting CRDT object IDs for
/// the same top-level structure (workbook, sheets, cells maps). The CRDT merge
/// then resolves the key conflict by last-write-wins, silently discarding the
/// other agent's data.
pub async fn apply_set_cell_via_ws(
    url: &str,
    sheet_id: &str,
    addr: &str,
    value: &Value,
) -> Result<()> {
    use crate::crdt_documents::yjs_protocol::{
        decode_sync_step1_sv, decode_sync_step2_update, encode_sync_step1, encode_sync_step2,
    };
    use futures::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message as TMsg;
    use yrs::updates::decoder::Decode;
    use yrs::{ReadTxn, StateVector};

    let (mut ws, _resp) = tokio_tungstenite::connect_async(url)
        .await
        .map_err(|e| anyhow!("connect: {e}"))?;

    let local = Doc::new();

    // Step 1: Wait for the server's sync_step1 and capture the server SV.
    let server_sv: StateVector = loop {
        match ws.next().await {
            Some(Ok(TMsg::Binary(bytes))) => {
                if let Some(sv_bytes) = decode_sync_step1_sv(&bytes) {
                    break StateVector::decode_v1(&sv_bytes)
                        .map_err(|e| anyhow!("decode SV: {e:?}"))?;
                }
                // Not a step1 frame (e.g. awareness) — keep reading.
                continue;
            }
            Some(Ok(_)) => continue, // text / ping / pong frames
            Some(Err(e)) => return Err(anyhow!("ws recv: {e}")),
            None => return Err(anyhow!("ws closed before sync_step1")),
        }
    };

    // Step 2: Send our own sync_step1 so the server knows our (empty) state and
    // sends us its full state via sync_step2.
    let our_sv = local.transact().state_vector();
    ws.send(TMsg::Binary(encode_sync_step1(&our_sv).into()))
        .await
        .map_err(|e| anyhow!("send step1: {e}"))?;

    // Step 3: Wait for the server's sync_step2 (the server's current state).
    // We may receive other frames (awareness, update broadcasts) — skip them.
    // When the server's doc is empty, it will send an empty update; we still
    // need to receive it to complete the handshake.
    let server_state_bytes: Vec<u8> = loop {
        match ws.next().await {
            Some(Ok(TMsg::Binary(ref bytes))) => {
                if let Some(state) = decode_sync_step2_update(bytes) {
                    break state;
                }
                continue;
            }
            Some(Ok(_)) => continue,
            Some(Err(e)) => return Err(anyhow!("ws recv step2: {e}")),
            None => return Err(anyhow!("ws closed before sync_step2")),
        }
    };

    // Step 4: Import the server's state into our local doc, then apply mutation.
    // This ensures our doc shares the same CRDT object IDs as the server.
    {
        let update = yrs::Update::decode_v1(&server_state_bytes)
            .map_err(|e| anyhow!("decode server state: {e:?}"))?;
        local
            .transact_mut()
            .apply_update(update)
            .map_err(|e| anyhow!("apply server state: {e:?}"))?;
    }
    // D-T5: the WS protocol has no return channel for SetCellOutcome —
    // the diff we ship over the wire is the only payload the server sees.
    // To keep warnings visible in observability we log them here at WARN
    // level; the agent that triggered the WS write won't see them in its
    // tool result, but operators tailing logs will.
    let outcome = apply_set_cell_in_proc(&local, sheet_id, addr, value);
    if !outcome.warnings.is_empty() {
        tracing::warn!(
            sheet_id = %sheet_id,
            addr = %addr,
            warnings = ?outcome.warnings,
            "set_cell via WS produced warnings (cells_recalculated={})",
            outcome.cells_recalculated,
        );
    }

    // Step 5: Compute diff of our new mutation against the server's known SV,
    // then send it.
    let diff = local.transact().encode_diff_v1(&server_sv);
    let frame = encode_sync_step2(&diff);
    ws.send(TMsg::Binary(frame.into()))
        .await
        .map_err(|e| anyhow!("send step2: {e}"))?;

    // Give the server a moment to apply the update, then close.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    ws.send(TMsg::Close(None)).await.ok();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crdt_documents::formula_engine::CellResolver;
    use crate::crdt_documents::formula_engine_yrs_resolver::YrsResolver;
    use crate::crdt_documents::projection;

    #[test]
    fn set_cell_then_project_reflects_value() {
        let doc = Doc::new();
        let _ = apply_set_cell_in_proc(&doc, "s1", "A1", &Value::String("Hola".into()));
        let v = projection::project(&doc);
        eprintln!("projection: {v}");
        assert_eq!(
            v["sheets"][0]["cells"]["A1"],
            serde_json::Value::String("Hola".into())
        );
    }

    #[test]
    fn set_cell_persists_formula_and_evaluated_value() {
        let doc = Doc::new();
        let _ = apply_set_cell_in_proc(&doc, "Sheet1", "A1", &serde_json::json!(2));
        let _ = apply_set_cell_in_proc(&doc, "Sheet1", "A2", &serde_json::json!(3));
        let _ = apply_set_cell_in_proc(&doc, "Sheet1", "A3", &serde_json::json!(5));

        let outcome =
            apply_set_cell_in_proc(&doc, "Sheet1", "B1", &serde_json::json!("=SUM(A1:A3)"));

        let r = YrsResolver::new(&doc);
        let cell = r.get("Sheet1", "B1").expect("B1");
        assert_eq!(cell.v.as_f64(), Some(10.0));

        // Verify f + fs are persisted in the y-doc cell map.
        let txn = doc.transact();
        let workbook = txn.get_map("workbook").unwrap();
        let sheets = match workbook.get(&txn, "sheets").unwrap() {
            yrs::Out::YArray(a) => a,
            _ => panic!("expected sheets array"),
        };
        let yrs::Out::YMap(sheet) = sheets.get(&txn, 0).unwrap() else {
            panic!()
        };
        let yrs::Out::YMap(cells) = sheet.get(&txn, "cells").unwrap() else {
            panic!()
        };
        let yrs::Out::YMap(b1) = cells.get(&txn, "B1").unwrap() else {
            panic!()
        };
        let yrs::Out::Any(yrs::Any::String(f)) = b1.get(&txn, "f").unwrap() else {
            panic!()
        };
        assert_eq!(f.as_ref(), "=SUM(A1:A3)");
        let yrs::Out::Any(yrs::Any::String(fs)) = b1.get(&txn, "fs").unwrap() else {
            panic!()
        };
        assert_eq!(fs.as_ref(), "be");

        // No dependents reference B1, so nothing else recalculates.
        assert_eq!(outcome.cells_recalculated, 0);
        assert!(outcome.warnings.is_empty());
    }

    #[test]
    fn set_cell_recalculates_dependents_in_topo_order() {
        let doc = Doc::new();
        let _ = apply_set_cell_in_proc(&doc, "Sheet1", "A1", &serde_json::json!(1));
        let _ = apply_set_cell_in_proc(&doc, "Sheet1", "B1", &serde_json::json!("=A1+10")); // → 11
        let _ = apply_set_cell_in_proc(&doc, "Sheet1", "C1", &serde_json::json!("=B1*2")); // → 22

        // Now mutate A1 — B1 and C1 must update.
        let outcome = apply_set_cell_in_proc(&doc, "Sheet1", "A1", &serde_json::json!(5));

        assert_eq!(outcome.cells_recalculated, 2);

        let r = YrsResolver::new(&doc);
        assert_eq!(r.get("Sheet1", "B1").unwrap().v.as_f64(), Some(15.0));
        assert_eq!(r.get("Sheet1", "C1").unwrap().v.as_f64(), Some(30.0));
    }

    #[test]
    fn set_cell_with_unsupported_function_marks_needs_browser() {
        // Pick a function genuinely not in formualizer's registry. BOGUSFN
        // should be safe; verify in the test's precondition.
        let doc = Doc::new();
        assert!(
            !crate::crdt_documents::formula_engine::is_supported_fn("BOGUSFN"),
            "precondition: BOGUSFN must be unsupported"
        );
        let outcome =
            apply_set_cell_in_proc(&doc, "Sheet1", "A1", &serde_json::json!("=BOGUSFN(1)"));
        match outcome.warnings.as_slice() {
            [SetCellWarning::NeedsBrowser { addr, functions }] => {
                assert_eq!(addr, "A1");
                assert!(functions.iter().any(|f| f.eq_ignore_ascii_case("BOGUSFN")));
            }
            other => panic!("expected one NeedsBrowser warning, got {:?}", other),
        }
        let r = YrsResolver::new(&doc);
        let cell = r.get("Sheet1", "A1").unwrap();
        // v carries the formula text placeholder (string type).
        assert_eq!(cell.v.as_str(), Some("=BOGUSFN(1)"));
        assert_eq!(cell.t, 1);
    }

    #[test]
    fn set_cell_with_eval_error_emits_warning_and_persists_excel_error() {
        let doc = Doc::new();
        let outcome = apply_set_cell_in_proc(&doc, "Sheet1", "A1", &serde_json::json!("=1/0"));

        // Warning surfaces the Excel error.
        match outcome.warnings.as_slice() {
            [SetCellWarning::EvalError { addr, error }] => {
                assert_eq!(addr, "A1");
                assert!(
                    error.contains("#DIV/0!"),
                    "expected #DIV/0! error, got {error}"
                );
            }
            other => panic!("expected one EvalError warning, got {:?}", other),
        }

        // Cell persists with the error string + t=4.
        let r = YrsResolver::new(&doc);
        let cell = r.get("Sheet1", "A1").unwrap();
        assert_eq!(cell.v.as_str(), Some("#DIV/0!"));
        assert_eq!(cell.t, 4);
    }

    #[test]
    fn set_cell_with_cycle_emits_cycle_warning() {
        let doc = Doc::new();
        // Seed mutual references: A1=B1+1, B1=A1+1.
        let _ = apply_set_cell_in_proc(&doc, "Sheet1", "A1", &serde_json::json!(0));
        let _ = apply_set_cell_in_proc(&doc, "Sheet1", "B1", &serde_json::json!("=A1+1"));
        // Now change A1 to also reference B1 → cycle.
        let outcome = apply_set_cell_in_proc(&doc, "Sheet1", "A1", &serde_json::json!("=B1+1"));

        // The cell IS written (we don't roll back its value).
        let r = YrsResolver::new(&doc);
        let cell = r.get("Sheet1", "A1").unwrap();
        assert!(cell.v.as_f64().is_some() || cell.v.as_str().is_some());

        // Cycle warning surfaces.
        let cycle = outcome
            .warnings
            .iter()
            .find_map(|w| {
                if let SetCellWarning::Cycle { chain } = w {
                    Some(chain.clone())
                } else {
                    None
                }
            })
            .expect("expected one Cycle warning");
        // Chain should include both A1 and B1.
        let chain_set: std::collections::HashSet<_> = cycle.iter().cloned().collect();
        assert!(chain_set.contains(&("Sheet1".to_string(), "A1".to_string())));
        assert!(chain_set.contains(&("Sheet1".to_string(), "B1".to_string())));
    }

    #[test]
    fn literal_write_on_formula_free_sheet_skips_recalc() {
        // On a sheet with NO formulas, literal writes must not trigger
        // any recalc work. We verify this by writing a literal and asserting
        // cells_recalculated is 0 (no formulas to recalc) AND no extra
        // sheet-level keys are produced beyond what the literal write needs.
        let doc = Doc::new();
        // First write — sheet is brand new, no formulas exist.
        let outcome = apply_set_cell_in_proc(&doc, "Sheet1", "A1", &serde_json::json!(1));
        assert_eq!(outcome.cells_recalculated, 0);
        assert!(outcome.warnings.is_empty());

        // Second literal write — still no formulas.
        let outcome2 = apply_set_cell_in_proc(&doc, "Sheet1", "A2", &serde_json::json!(2));
        assert_eq!(outcome2.cells_recalculated, 0);
        assert!(outcome2.warnings.is_empty());

        // No "has_formulas" key should be present yet.
        let txn = doc.transact();
        let workbook = txn.get_map("workbook").unwrap();
        let sheets = match workbook.get(&txn, "sheets").unwrap() {
            yrs::Out::YArray(a) => a,
            _ => panic!(),
        };
        let yrs::Out::YMap(sheet) = sheets.get(&txn, 0).unwrap() else {
            panic!()
        };
        assert!(
            sheet.get(&txn, "has_formulas").is_none(),
            "has_formulas should not be set when no formula has ever been written"
        );

        // Now write a formula → flag must flip.
        drop(txn);
        let _ = apply_set_cell_in_proc(&doc, "Sheet1", "B1", &serde_json::json!("=A1+A2"));
        let txn = doc.transact();
        let workbook = txn.get_map("workbook").unwrap();
        let sheets = match workbook.get(&txn, "sheets").unwrap() {
            yrs::Out::YArray(a) => a,
            _ => panic!(),
        };
        let yrs::Out::YMap(sheet) = sheets.get(&txn, 0).unwrap() else {
            panic!()
        };
        let yrs::Out::Any(yrs::Any::Bool(flag)) = sheet.get(&txn, "has_formulas").unwrap() else {
            panic!("has_formulas should be a Bool after formula write");
        };
        assert!(flag, "has_formulas should be true after a formula write");
    }

    #[test]
    fn set_cell_with_parse_error_emits_warning_and_persists_literal() {
        let doc = Doc::new();
        let outcome = apply_set_cell_in_proc(
            &doc,
            "Sheet1",
            "A1",
            &serde_json::json!("=SUM("), // Malformed: unclosed paren
        );

        // Warning surfaces the parse error.
        let parse_err = outcome.warnings.iter().find_map(|w| {
            if let SetCellWarning::ParseError { addr, error } = w {
                Some((addr.clone(), error.clone()))
            } else {
                None
            }
        });
        assert!(
            parse_err.is_some(),
            "expected ParseError warning, got {:?}",
            outcome.warnings
        );

        // The cell still gets the raw text persisted as a literal (no f/fs).
        let r = YrsResolver::new(&doc);
        let cell = r.get("Sheet1", "A1").unwrap();
        assert_eq!(cell.v.as_str(), Some("=SUM("));
        // Type is string (1), not error (4).
        assert_eq!(cell.t, 1);
    }

    #[test]
    fn set_range_with_mixed_cells_recalculates() {
        let doc = Doc::new();
        let _ = apply_set_cell_in_proc(&doc, "Sheet1", "A1", &serde_json::json!(0));
        let _ = apply_set_cell_in_proc(&doc, "Sheet1", "B1", &serde_json::json!(0));
        let _ = apply_set_cell_in_proc(&doc, "Sheet1", "D1", &serde_json::json!("=A1+B1"));

        let mut total_recalc = 0usize;
        let o1 = apply_set_cell_in_proc(&doc, "Sheet1", "A1", &serde_json::json!(5));
        total_recalc += o1.cells_recalculated;
        let o2 = apply_set_cell_in_proc(&doc, "Sheet1", "B1", &serde_json::json!(10));
        total_recalc += o2.cells_recalculated;
        let o3 = apply_set_cell_in_proc(&doc, "Sheet1", "C1", &serde_json::json!(20));
        total_recalc += o3.cells_recalculated;

        // D1 was recalculated by the time o2 landed (and again by o1).
        assert!(
            total_recalc >= 2,
            "expected at least 2 recalcs, got {total_recalc}"
        );
        let r = YrsResolver::new(&doc);
        assert_eq!(r.get("Sheet1", "D1").unwrap().v.as_f64(), Some(15.0));
    }
}

fn json_to_any(v: &Value) -> (Any, &'static str) {
    match v {
        Value::String(s) => (Any::String(s.clone().into()), "s"),
        Value::Number(n) => (n.as_f64().map(Any::Number).unwrap_or(Any::Null), "n"),
        Value::Bool(b) => (Any::Bool(*b), "b"),
        _ => (Any::Null, "s"),
    }
}

/// Map a JSON value to the numeric type tag used by the formula engine
/// (1=string, 2=number, 3=bool, 4=error/other). Used by `write_cell_raw`
/// callers that need a `u8` tag.
///
/// Catch-all (`Null` / `Array` / `Object`) maps to `4` (error) — those
/// values aren't representable as scalar cell types, so `#N/A`-style
/// error semantics is a more honest signal than silently coercing them
/// to a string.
fn json_value_type_tag(v: &Value) -> u8 {
    match v {
        Value::String(_) => 1,
        Value::Number(_) => 2,
        Value::Bool(_) => 3,
        _ => 4,
    }
}

// ─── multi-sheet helpers ─────────────────────────────────────────────────────

/// Append a new sheet to the workbook. Returns the generated sheet id
/// (format: `sh_<ULID>`).
pub fn apply_add_sheet(doc: &Doc, name: &str) -> String {
    let mut txn = doc.transact_mut_with(SERVER_TX_ORIGIN);
    let wb = txn.get_or_insert_map("workbook");
    let sheets = match wb.get(&txn, "sheets") {
        Some(yrs::Out::YArray(a)) => a,
        _ => wb.insert(&mut txn, "sheets", ArrayPrelim::default()),
    };
    let sheet_id = format!("sh_{}", ulid::Ulid::new());
    let sheet = sheets.push_back(&mut txn, MapPrelim::default());
    sheet.insert(&mut txn, "id", sheet_id.as_str());
    sheet.insert(&mut txn, "name", name);
    sheet.insert(&mut txn, "cells", MapPrelim::default());
    sheet_id
}

/// Rename a sheet by id. Returns `false` if no sheet with that id is found.
pub fn apply_rename_sheet(doc: &Doc, sheet_id: &str, new_name: &str) -> bool {
    let mut txn = doc.transact_mut_with(SERVER_TX_ORIGIN);
    // Find the index first (read phase), then mutate.
    let idx = find_sheet_index_in_txn(&txn, sheet_id);
    let Some(i) = idx else {
        return false;
    };
    let wb = txn.get_or_insert_map("workbook");
    let sheets = match wb.get(&txn, "sheets") {
        Some(yrs::Out::YArray(a)) => a,
        _ => return false,
    };
    if let Some(yrs::Out::YMap(m)) = sheets.get(&txn, i) {
        m.insert(&mut txn, "name", new_name);
        true
    } else {
        false
    }
}

/// Delete a sheet by id. Returns `false` if not found.
pub fn apply_delete_sheet(doc: &Doc, sheet_id: &str) -> bool {
    let mut txn = doc.transact_mut_with(SERVER_TX_ORIGIN);
    let idx = find_sheet_index_in_txn(&txn, sheet_id);
    let Some(i) = idx else {
        return false;
    };
    let wb = txn.get_or_insert_map("workbook");
    let sheets = match wb.get(&txn, "sheets") {
        Some(yrs::Out::YArray(a)) => a,
        _ => return false,
    };
    sheets.remove(&mut txn, i);
    true
}

/// Reorder sheets. `new_order` must be a permutation of the existing sheet
/// ids (same set, different order). Returns `false` on any mismatch.
///
/// Implementation: snapshot-and-restore, because `yrs::Array` has no
/// in-place move operation.
pub fn apply_reorder_sheets(doc: &Doc, new_order: &[String]) -> bool {
    // Phase 1: snapshot (read-only txn).
    let snapshots: Vec<serde_json::Value> = {
        let txn = doc.transact();
        let wb = match txn.get_map("workbook") {
            Some(m) => m,
            None => return false,
        };
        let sheets = match wb.get(&txn, "sheets") {
            Some(yrs::Out::YArray(a)) => a,
            _ => return false,
        };
        let len = sheets.len(&txn);
        if len as usize != new_order.len() {
            return false;
        }
        (0..len)
            .filter_map(|i| match sheets.get(&txn, i) {
                Some(yrs::Out::YMap(m)) => Some(snapshot_sheet_inline(&txn, &m)),
                _ => None,
            })
            .collect()
    };

    // Validate that new_order is a permutation of existing ids.
    let mut existing_ids: Vec<String> = snapshots
        .iter()
        .filter_map(|s| s["id"].as_str().map(str::to_string))
        .collect();
    if existing_ids.len() != new_order.len() {
        return false;
    }
    let mut requested = new_order.to_vec();
    existing_ids.sort();
    requested.sort();
    if existing_ids != requested {
        return false;
    }

    // Phase 2: clear and re-insert in new order (write txn).
    // NOTE: this function uses two separate transactions (read then write).
    // Between them, a concurrent peer could apply an update via the server's
    // WS handler — that peer's changes to the sheets array would be silently
    // lost when the write phase clears+restores. Acceptable for v1 because
    // tools 3-14 are invoked from a single DAG agent at a time. When multi-peer
    // concurrent reorder becomes a requirement (Task 15+), wrap both phases in
    // a per-document Mutex or fold the validation into a single transact_mut.
    let mut txn = doc.transact_mut_with(SERVER_TX_ORIGIN);
    let wb = txn.get_or_insert_map("workbook");
    let sheets = match wb.get(&txn, "sheets") {
        Some(yrs::Out::YArray(a)) => a,
        _ => return false,
    };
    let current_len = sheets.len(&txn);
    for i in (0..current_len).rev() {
        sheets.remove(&mut txn, i);
    }
    for desired_id in new_order {
        let snap = match snapshots
            .iter()
            .find(|s| s["id"].as_str() == Some(desired_id))
        {
            Some(s) => s,
            None => return false,
        };
        let new_sheet = sheets.push_back(&mut txn, MapPrelim::default());
        new_sheet.insert(&mut txn, "id", desired_id.as_str());
        let name = snap["name"].as_str().unwrap_or("");
        new_sheet.insert(&mut txn, "name", name);
        let cells_map = new_sheet.insert(&mut txn, "cells", MapPrelim::default());
        if let Some(obj) = snap["cells"].as_object() {
            for (addr, v) in obj {
                let cell = cells_map.insert(&mut txn, addr.as_str(), MapPrelim::default());
                let (any, t) = json_to_any(v);
                cell.insert(&mut txn, "v", any);
                cell.insert(&mut txn, "t", t);
            }
        }
    }
    true
}

// ─── internal utilities ──────────────────────────────────────────────────────

/// Find the array index of the sheet with the given id using a read txn.
/// Returns `None` if the sheet is not found or the workbook has no sheets.
fn find_sheet_index_in_txn<T: yrs::ReadTxn>(txn: &T, sheet_id: &str) -> Option<u32> {
    let wb = txn.get_map("workbook")?;
    let sheets = match wb.get(txn, "sheets") {
        Some(yrs::Out::YArray(a)) => a,
        _ => return None,
    };
    for i in 0..sheets.len(txn) {
        if let Some(yrs::Out::YMap(m)) = sheets.get(txn, i) {
            if let Some(yrs::Out::Any(yrs::Any::String(s))) = m.get(txn, "id") {
                if s.as_ref() == sheet_id {
                    return Some(i);
                }
            }
        }
    }
    None
}

/// Inline snapshot of a single sheet map into JSON (used by `apply_reorder_sheets`).
///
/// Local inline projection of a single sheet. Duplicates the logic that
/// Task 4 will add as canonical `projection::project_sheet`. Once that
/// helper lands, replace calls to this with `projection::project_sheet`
/// and delete this function.
///
/// TODO(Task 4): replace with `crate::crdt_documents::projection::project_sheet`.
fn snapshot_sheet_inline<T: yrs::ReadTxn>(txn: &T, sheet_map: &yrs::MapRef) -> serde_json::Value {
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
    let mut cells_out = serde_json::Map::new();
    if let Some(yrs::Out::YMap(cells_map)) = sheet_map.get(txn, "cells") {
        for (addr, cell_val) in cells_map.iter(txn) {
            if let yrs::Out::YMap(cell_map) = cell_val {
                if let Some(yrs::Out::Any(any)) = cell_map.get(txn, "v") {
                    let v = match any {
                        yrs::Any::Null | yrs::Any::Undefined => serde_json::Value::Null,
                        yrs::Any::Bool(b) => serde_json::Value::Bool(b),
                        yrs::Any::Number(n) => serde_json::json!(n),
                        yrs::Any::BigInt(n) => serde_json::json!(n),
                        yrs::Any::String(s) => serde_json::Value::String(s.to_string()),
                        _ => serde_json::Value::Null,
                    };
                    cells_out.insert(addr.to_string(), v);
                }
            }
        }
    }
    serde_json::json!({ "id": id, "name": name, "cells": cells_out })
}

#[cfg(test)]
mod multi_sheet_tests {
    use super::*;
    use crate::crdt_documents::projection::project;
    use yrs::Doc;

    #[test]
    fn add_sheet_appends_and_returns_unique_id() {
        let doc = Doc::new();
        let id1 = apply_add_sheet(&doc, "Sales");
        let id2 = apply_add_sheet(&doc, "Summary");
        assert_ne!(id1, id2);
        let v = project(&doc);
        assert_eq!(v["sheets"].as_array().unwrap().len(), 2);
        assert_eq!(v["sheets"][0]["name"], "Sales");
        assert_eq!(v["sheets"][1]["name"], "Summary");
    }

    #[test]
    fn rename_sheet_changes_name_only() {
        let doc = Doc::new();
        let id = apply_add_sheet(&doc, "Old");
        let _ = apply_set_cell_in_proc(&doc, &id, "A1", &serde_json::json!("kept"));
        assert!(apply_rename_sheet(&doc, &id, "New"));
        let v = project(&doc);
        assert_eq!(v["sheets"][0]["name"], "New");
        assert_eq!(v["sheets"][0]["cells"]["A1"], "kept");
    }

    #[test]
    fn delete_sheet_removes_it() {
        let doc = Doc::new();
        let a = apply_add_sheet(&doc, "A");
        let b = apply_add_sheet(&doc, "B");
        assert!(apply_delete_sheet(&doc, &a));
        let v = project(&doc);
        assert_eq!(v["sheets"].as_array().unwrap().len(), 1);
        assert_eq!(v["sheets"][0]["id"], b);
    }

    #[test]
    fn reorder_sheets_swaps() {
        let doc = Doc::new();
        let a = apply_add_sheet(&doc, "A");
        let b = apply_add_sheet(&doc, "B");
        assert!(apply_reorder_sheets(&doc, &[b.clone(), a.clone()]));
        let v = project(&doc);
        assert_eq!(v["sheets"][0]["id"], b);
        assert_eq!(v["sheets"][1]["id"], a);
    }
}
