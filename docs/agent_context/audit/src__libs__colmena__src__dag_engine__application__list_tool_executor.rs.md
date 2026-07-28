# src/libs/colmena/src/dag_engine/application/list_tool_executor.rs

**Layer:** application  
**Purpose:** Deterministic iteration engine for `for_each` node: dispatches a closure over N rows with concurrency/error policies, maintaining stable index ordering regardless of completion order.

## Symbols

- `DEFAULT_MAX_ITEMS` (const, pub) — default 1000-item limit for list execution
- `OnError` (enum, pub) — error policy enum: `Continue` (collect all) or `Abort` (stop at first error)
- `OnError::Continue` (variant) — continue policy
- `OnError::Abort` (variant) — abort policy
- `ExecPolicy` (struct, pub) — execution configuration: `on_error`, `concurrency`, `max_items` fields
- `ExecPolicy::default()` (impl, method) — creates default policy (Continue, concurrency=1, max=1000)
- `ItemStatus` (enum, pub) — result status: `Ok` or `Err`
- `ItemStatus::Ok` (variant) — successful item result
- `ItemStatus::Err` (variant) — failed item result
- `ItemResult` (struct, pub) — per-item result record: index, input, status, output, error
- `run_list()` (async fn, pub) — main entry point; dispatches closure over rows sequentially or concurrently, returns index-ordered results
- `run_sequential()` (async fn, private) — sequential execution path for concurrency ≤ 1; aborts early if `on_error == Abort`
- `tests` (module, test) — 4 unit tests covering Continue/Abort policies, index ordering, and best-effort abort under concurrency

## File-level notes

- `max_items` field in `ExecPolicy` is defined (line 19) but **never checked or enforced** in either `run_list` or `run_sequential`. Tests and default creation reference it, but actual execution logic ignores it. Likely a planned feature not yet implemented.
- Best-effort Abort under concurrency (lines 66–70) is intentional and documented: in-flight items complete, but no NEW items start after error observed. Strict cancellation deferred to backlog.
- Test coverage is solid: sequential Continue/Abort, parallel index ordering, and concurrent Abort edge cases all verified.
