# src/libs/colmena/src/crdt_documents/xlsx_import.rs

**Layer:** application  **Purpose:** Imports an XLSX file blob into a Yrs CRDT document, parsing sheets and cells while preserving CRDT structure; wipes existing sheets on import.

## Symbols

- `ImportError` (enum, pub) — Wraps calamine::XlsxError for XLSX parsing failures
- `ImportStats` (struct, pub) — Tracks sheets_imported (u32) and cells_imported (u64) counts
- `import_xlsx_into_doc` (fn, pub) — Main entry point; reads XLSX bytes, creates/clears sheets array in a Yrs::Doc, iterates all worksheets and non-empty cells, returns ImportStats or ImportError
- `format_a1` (fn, private) — Converts row/column indices to Excel cell address format (base-26 encoding, e.g., "A1", "Z10", "AA5")
- `datatype_to_any` (fn, private) — Maps calamine Data cell variants (String, Float, Int, Bool, DateTime, Error, DateTimeIso, DurationIso) to Yrs::Any values plus a type tag string ("s" for string, "n" for number, "b" for bool)
- `tests` (mod, cfg test) — Integration test module
- `read_fixture` (fn, private in tests) — Reads test XLSX file from multiple candidate relative paths, panics if none found
- `imports_spike_fixture` (fn, test) — Integration test verifying XLSX import produces ≥1 sheet, ≥700 cells, and expected sheet name/cell content

## File-level notes

- **Missing public documentation**: No `///` doc comments on public functions (`import_xlsx_into_doc`, `ImportError`, `ImportStats`) despite module-level doc; should document parameters, return semantics, and side-effects (clears existing sheets).
- **Silent worksheet load errors**: Line 43–46 silently skips worksheets that fail to load via `worksheet_range()` (Error → continue); users receive no indication of partial-import failure or which sheets were omitted.
- **Test fixture path brittleness**: `read_fixture()` tries multiple hardcoded relative paths and panics if none found; test portability depends on being run from specific working directories.
- **Yrs transaction atomicity**: All sheet/cell inserts batched into a single transaction (line 23); correct and efficient.
- **Cell address encoding**: `format_a1()` uses standard base-26 column encoding; algorithm is correct for Excel-style addresses.
