# src/libs/colmena/src/crdt_documents/tool_executor.rs

**Layer:** application  **Purpose:** Orchestrates in-process mutations to Yjs CRDT documents (cell writes, formula evaluation, recalc cascades) and multi-sheet operations; also provides WS protocol bridge for syncing mutations back to peers.

## Symbols

- `SetCellOutcome` (struct, pub) — Result wrapper carrying count of cells recalculated and warnings (unsupported functions, eval errors, cycles, parse errors) from `apply_set_cell_in_proc`
- `SetCellWarning` (enum, pub) — Tagged warning type emitted during set-cell + recalc: `NeedsBrowser` (unsupported function, placeholder persisted), `EvalError` (eval failure mapped to Excel error), `Cycle` (circular reference detected), `ParseError` (malformed formula syntax)
- `apply_set_cell_in_proc` (fn, pub) — Core mutation: parses formula strings (if prefixed `=`), evaluates via formula engine against current doc state, writes result + formula metadata to cell, triggers topological recalc of downstream dependents; returns outcome with recalc count and warnings
- `recompute_dependent` (fn, pub) — Re-evaluates a single dependent cell's formula by pulling formula text, parsing, evaluating, and writing new value back (used by `apply_set_cell_in_proc` and df_writer for batch recalc)
- `apply_set_cell_via_ws` (async fn, pub) — Yjs WS client: connects to URL, syncs full server state via v1 protocol (step1+step2 handshake both directions), applies mutation locally, ships diff back to server for peer convergence
- `apply_add_sheet` (fn, pub) — Appends new sheet to workbook with generated ULID-based id; returns generated sheet id
- `apply_rename_sheet` (fn, pub) — Updates sheet name by id; returns false if sheet not found
- `apply_delete_sheet` (fn, pub) — Removes sheet by id from sheets array; returns false if not found
- `apply_reorder_sheets` (fn, pub) — Reorders sheets by permutation validation + snapshot-and-restore (yrs::Array has no in-place move); validates new_order is permutation of existing ids, snapshots current state, clears array, re-inserts in new order; returns false on validation failure
- `sheet_has_any_formula` (fn, private) — Reads sticky `has_formulas` flag on sheet map to fast-path literal-write recalc (avoids empty dep-graph walk when no formulas exist); flag never cleared once set
- `write_cell_raw` (fn, private) — Core write helper: creates workbook/sheet/cells structure on demand, idempotent on sheet lookup by id, writes {v, t, f?, fs?} with CRDT history preservation, tags transaction with `SERVER_TX_ORIGIN` to skip observer firing
- `json_to_any` (fn, private) — Converts JSON value to Yrs `Any` type; returns tuple of (Any, &str) where string is short type abbreviation ("s", "n", "b")
- `json_value_type_tag` (fn, private) — Maps JSON value to numeric type tag used by formula engine (1=string, 2=number, 3=bool, 4=error/catch-all); used for persisting type metadata
- `find_sheet_index_in_txn` (fn, private) — Iterates sheets array under read txn to find index of sheet with given id; returns None if not found or workbook empty
- `snapshot_sheet_inline` (fn, private) — Snapshots single sheet map into JSON (id, name, cells) for `apply_reorder_sheets` phase 1; duplicates logic that should become canonical `projection::project_sheet` in Task 4

## File-level notes

- **Type encoding inconsistency:** `json_to_any` returns string tags ("s", "n", "b") but `json_value_type_tag` produces numeric tags (1, 2, 3, 4). Line 915 in `apply_reorder_sheets` uses the string tag result from `json_to_any` when persisting cell type, whereas `write_cell_raw` properly uses numeric BigInt tags — this mismatch may cause incorrect type metadata in reordered sheets. [FLAG: improvement]
- **TODO marker:** `snapshot_sheet_inline` (line 951) has TODO(Task 4) to replace itself with canonical `crate::crdt_documents::projection::project_sheet` once that lands; function duplicates projection logic. [FLAG: unfinished]
- **Known concurrency gap:** `apply_reorder_sheets` snapshot+restore (lines 841–919) uses separate read and write transactions; between them, concurrent peers via WS could apply updates that get silently lost when phase 2 clears+restores. Documented as acceptable for v1 (single agent per doc at a time) with note that Task 15+ multi-peer reorder requires per-document Mutex or single-transaction validation. [FLAG: improvement — architectural note]
- Formula evaluation path includes cycle detection (`recalc_chain` call) that emits `SetCellWarning::Cycle` when circular references exist; cells are still persisted (no rollback), warnings surface to caller for agent awareness.
- Literal cell writes on formula-free sheets short-circuit dependency graph walk via `sheet_has_any_formula` flag — important optimization for bulk writers like `df_writer` inserting 100K+ cells.
- WS sync protocol tagging with `SERVER_TX_ORIGIN` (recalc_observer.rs constant) prevents observer from firing on server's own writes, since recalc cascade is already applied in-proc before shipping diff.
- Extensive test coverage: 11 integration tests in `tests` module cover formula persistence, recalc topo ordering, unsupported functions, eval errors, cycles, parse errors, literal writes, range updates; 4 multi-sheet tests for add/rename/delete/reorder operations.
