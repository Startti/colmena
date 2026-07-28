# src/libs/colmena/src/python_bindings/crdt_documents.rs

**Layer:** bindings  
**Purpose:** PyO3 bindings that expose CRDT documents functionality to Python via a `colmena.documents` submodule. Provides sheet management (list, read, add, write) backed by a lazy-initialized singleton `CrdtDocumentsRuntime`.

## Symbols

- `RUNTIME` (static, private) — OnceCell singleton storing the arc-wrapped CrdtDocumentsRuntime instance  
- `runtime()` (fn, private) — Lazily initializes CrdtDocumentsRuntime from `COLMENA_CRDT_DOCUMENTS_STORAGE_ROOT` env var (default `.colmena/crdt_documents`), blocks on current tokio runtime  
- `parse_id()` (fn, private) — Parses a string artifact ID and returns PyValueError on invalid format  
- `list_sheets()` (pyfunction, private) — Lists all sheets in a CRDT document, returns Python list of dicts with `sheet_id` and `name` [FLAG: unfinished — `#[allow(deprecated)]` on exported function suggests internal or scheduled deprecation]  
- `read_sheet()` (pyfunction, private) — Reads a specific sheet's cells as a dict mapping cell addresses to converted Python values (string/number/bool/null) [FLAG: unfinished — `#[allow(deprecated)]` on exported function]  
- `add_sheet()` (pyfunction, private) — Adds a new sheet to a CRDT document, marks it dirty, and records a change entry via ChangeTracker [FLAG: unfinished — `#[allow(deprecated)]` on exported function; improvement — swallows tokio runtime acquisition error silently (lines 114-119)]  
- `write_sheet()` (pyfunction, private) — Writes column headers (row 1) and rows of cell values (from row 2) in replace or append mode [FLAG: unfinished — `#[allow(deprecated)]` on exported function; improvement — swallows tokio runtime acquisition error silently (lines 173-178)]  
- `col_letter()` (fn, private) — Converts a 0-indexed column number to Excel-style letters (e.g., 0→A, 26→AA)  
- `pyobj_to_json()` (fn, private) — Converts a PyAny object to serde_json::Value, handling None/String/bool/f64 with fallback to formatted string  
- `register()` (fn, pub) — Registers the `documents` submodule on the parent `colmena` module, attaching the four exported pyfunctions  

## File-level notes

- **Systematic deprecation suppression**: All four exported pyfunctions (`list_sheets`, `read_sheet`, `add_sheet`, `write_sheet`) are decorated with `#[allow(deprecated)]`. This suggests either an internal API is deprecated and pending removal, or these functions themselves are scheduled for deprecation/replacement. Needs clarification on the deprecation roadmap.
- **Silent error swallowing on async handoff**: Both `add_sheet` and `write_sheet` silently ignore tokio runtime acquisition failures when calling `ChangeTracker.record()` (lines 114–119, 173–178). While documented in comments as intentional, this could mask real issues in production if the runtime is unexpectedly unavailable. Consider adding warning-level logging.
- **Discarded SetCellOutcome**: Lines 154 and 166 intentionally discard `SetCellOutcome` (which carries recalc warnings); this is documented as "Python binding has no caller surface for them" — acceptable design choice but limits introspection.
- **Runtime isolation via OnceCell**: The singleton pattern ensures one global runtime per Python process. Thread-safe via `Arc<...>` but process-wide memoization; suitable for embedded Python contexts but limits multi-runtime scenarios.
