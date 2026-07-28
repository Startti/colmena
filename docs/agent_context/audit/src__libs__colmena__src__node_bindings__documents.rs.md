# src/libs/colmena/src/node_bindings/documents.rs

**Layer:** bindings  **Purpose:** Provides napi-based TypeScript bindings for CRDT document sheet operations (list, read, write, add sheets); zero-dependency wrapper around internal CrdtDocumentsRuntime.

## Symbols

- `RUNTIME` (const, private) — OnceCell<Arc<CrdtDocumentsRuntime>> singleton; lazily initialized on first async call to `runtime()`
- `runtime()` (fn, private async) — Initializes or returns the shared CrdtDocumentsRuntime from `COLMENA_CRDT_DOCUMENTS_STORAGE_ROOT` env var or defaults to `.colmena/crdt_documents`
- `parse_id()` (fn, private) — Parses a string artifact ID into ArtifactId type; returns InvalidArg error on parse failure
- `documents_list_sheets()` (fn, pub async napi) — Returns array of `{sheetId, name}` JSON objects for all sheets in an artifact [FLAG: improvement — `#[allow(deprecated)]` with no explanatory comment; indicates calling deprecated internals without context]
- `documents_read_sheet()` (fn, pub async napi) — Returns cell-addressed map (e.g., `{"A1": "header", ...}`) of all cells in a specific sheet [FLAG: improvement — `#[allow(deprecated)]` with no explanatory comment]
- `documents_write_sheet()` (fn, pub async napi) — Writes column headers and row data to a sheet with mode "replace" or "append"; writes headers to row 1, data rows starting at row 2 [FLAG: improvement — repeated registry lookup pattern (lines 104-107) duplicates error handling from other functions; `#[allow(deprecated)]` lacks context]
- `documents_add_sheet()` (fn, pub async napi) — Adds a new sheet to artifact (creating artifact if needed); returns new sheet's UUID; records operation to tracker [FLAG: improvement — `#[allow(deprecated)]` lacks context]
- `col_letter()` (fn, private) — Converts column index (0-based u32) to Excel-style letter(s): 0→A, 25→Z, 26→AA, etc.; loop invariant unclear for non-obvious col divisions

## File-level notes

- **Repeated error pattern**: Registry lookups at lines 40–43, 62–65, 104–107 all use identical `rt.registry.get() -> "artifact not found"` error. Candidate for helper function to reduce duplication.
- **Silent data loss in JSON mapping**: Lines 48–49 and similar use `.as_str().unwrap_or("")` which silently converts missing/malformed fields to empty strings instead of propagating errors. Could hide data integrity issues.
- **Universal `#[allow(deprecated)]` without documentation**: Every public napi function suppresses deprecation warnings from internal calls (likely `crate::crdt_documents::*` functions). Lacks inline comment explaining what is deprecated and why this wrapping is necessary. Consider adding `// HACK:` or `// TODO:` if this is temporary.
- **Error message specificity**: Uses `Status::GenericFailure` for multiple distinct failure modes (artifact not found, sheet not found, mode validation). Could benefit from more specific statuses or structured error variants.
- **col_letter() bounds**: Works correctly for u32 (no negative inputs) but division logic (`col / 26 - 1`) is non-obvious; no bounds comment or test coverage visible in file.
