# src/libs/colmena/src/crdt_documents/df_records.rs

**Layer:** infrastructure  
**Purpose:** Converts Y.Doc CRDT workbook sheets into row-major records format (Vec<Map<String, Value>>) for pandas DataFrame ingestion on the Python side. Handles A1 cell address parsing, column naming with auto-generated fallbacks, and enforces combined size limits across multiple sheets.

## Symbols

- `COMBINED_RECORDS_SIZE_CAP_BYTES` (const, pub) — Hard size cap (100 MiB) for all records produced across sheets in one `run_python` call (v1 limit; see BACKLOG for configurability path)
- `RecordsError` (enum, pub, derives Debug/thiserror::Error) — Error type with two variants: `SheetNotFound` and `SizeCapExceeded` with actual/limit bytes
- `RecordsError::SheetNotFound` (enum variant) — Indicates requested sheet_id was not found in the Y.Doc projection
- `RecordsError::SizeCapExceeded` (enum variant) — Indicates combined records JSON exceeded size cap; reports actual and limit bytes
- `SheetRecords` (struct, pub, derives Debug/Clone) — Holds extracted sheet data: sheet_id, ordered column names, and row-major records (each row is a Map keyed by column name)
- `build_sheet_records` (fn, pub) — Extracts one sheet from Y.Doc, parses A1 cell addresses into a grid, uses row 0 as headers (with fallback to `col_A`, `col_B` for missing/non-string headers), and builds record list skipping all-null rows
- `build_records_for_sheets` (fn, pub) — Builds records for multiple sheets in sequence; accumulates JSON byte sizes and returns error if total exceeds cap
- `parse_a1` (fn, private) — Parses Excel A1 cell address string (e.g., "B2") to (0-indexed row, 0-indexed col) tuple; returns None if malformed
- `col_letter` (fn, private) — Converts 0-indexed column number to Excel letter(s) (e.g., 0→"A", 25→"Z", 26→"AA"); used as fallback header name for missing/null headers
- `tests` (mod, test) — Test suite with 8 tests covering happy path (inventory sheet), missing sheets, empty sheets, header-only sheets, non-string headers, sparse cells, and multi-sheet builds

## File-level notes

- **Inefficiency in `col_letter` (line 143):** Uses `s.insert(0, ...)` in a loop, which is O(n²) because each insert at position 0 shifts all prior characters. Should use `push` + `reverse` or build in reverse from the start for O(n) performance.

- **Well-tested:** Coverage includes edge cases (empty sheets, sparse cells, non-string headers, multi-sheet, size limits via integration).

- **Assumption:** Row 0 is always treated as the header row; falls back to generated names (`col_A`, `col_B`, ...) for missing or non-string headers. Encoded in the logic with no configuration option.

- **Size cap is approximate:** Uses `serde_json::to_vec(&recs.records).len()` per-sheet to estimate, not a precise byte count of the final wire format. Saturating arithmetic used to prevent overflow.
