# src/libs/colmena/src/crdt_documents/recalc_observer.rs

**Layer:** infrastructure  **Purpose:** Server-side transaction observer that detects browser-originated cell mutations in shared CRDT documents and triggers dependent formula recalculation without deadlock.

## Symbols

- `SERVER_TX_ORIGIN` (const, pub) — Binary tag (`b"colmena:server:recalc"`) stamped on writes from `apply_set_cell_in_proc` and `recompute_dependent` to prevent observer re-entry.
- `attach_recalc_observer(doc: Arc<Doc>)` (fn, pub) — Attaches an after-transaction observer to a `yrs::Doc`; spawns background thread for each untagged write to recompute formulas across the workbook; returns `Subscription` that must be kept alive.
- `collect_all_formula_addrs<T: ReadTxn>(txn: &T)` (fn, private) — Walks the workbook map structure to enumerate all `(sheet_id, addr)` pairs where a cell has a non-empty formula (`f` key).
- `tests::simulate_browser_write(doc: &Doc, sheet_id: &str, addr: &str, value: f64)` (fn, private) — Test helper that opens an untagged transaction and mutates a cell's value and type, simulating a remote WS sync write.
- `tests::wait_for_value(doc: &Doc, sheet: &str, addr: &str, expected: f64, timeout_ms: u64)` (fn, private) — Test helper that polls a cell value with 10ms sleep intervals until it matches expected value or timeout expires.
- `tests::observer_recalcs_dependents_on_browser_write()` (test) — E2E verification that a browser write (A1 = 50) triggers observer and recalculates dependent formula (B1 = A1*2 → 100).
- `tests::observer_skips_server_originated_writes()` (test) — Verifies that tagged writes (via `apply_set_cell_in_proc`) do not trigger observer body, using an `AtomicUsize` counter.
- `tests::observer_no_op_when_workbook_is_empty()` (test) — Edge case: observer fires on untagged write to empty Doc with no workbook map; must not panic.

## File-level notes

- **Pragmatic v1 algorithm**: the observer enumerates *every* formula cell in the workbook per untagged write (O(formulas) time), not just changed cells. This is acknowledged as a pragmatic choice with documented optimization path ("optimise to per-changed-cell when this becomes a hot path") — intentional tradeoff for typical spreadsheet sizes (<10K cells).
- **Deadlock prevention**: recalculation work is deferred to a background thread because the observer fires from inside `TransactionMut::commit()` with the store's RwLock still held in write mode; opening a new transaction from the callback would deadlock. The Arc clone keeps the Doc alive if dropped before the thread runs.
- **Loop prevention**: writes produced by recalc are tagged with `SERVER_TX_ORIGIN` via `transact_mut_with`, and the observer's first check is `txn.origin()` to return early on match, preventing observer re-entry.
- **Test design**: uses `AtomicUsize` with `SeqCst` ordering for thread-safe counter in `observer_skips_server_originated_writes`. Polling loop in `wait_for_value` uses 10ms sleep intervals suitable for test timing.
- All imports from yrs 0.26 API are current; no deprecated API usage.
