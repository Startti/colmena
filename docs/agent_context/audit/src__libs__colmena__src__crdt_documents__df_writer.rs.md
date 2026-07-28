# src/libs/colmena/src/crdt_documents/df_writer.rs

**Layer:** infrastructure  
**Purpose:** Converts records-style data (output from `crdt_doc_run_python`) into Y.Doc sheet writes, handling sheet creation, name collision resolution, and per-cell writes with formula-replacement tracking.

## Symbols

- `MAX_OUTPUT_SHEET_ROWS` (const, pub) — Hard row limit (100,000) for records written via `crdt_doc_run_python`
- `MAX_SHEET_NAME_LEN` (const, pub) — Excel xlsx hard limit on sheet name length (31 characters)
- `WriterError` (enum, pub) — Error type for sheet write operations; variants: `EmptyName`, `SheetNotFound(String)`
- `WriteResult` (struct, pub) — Metadata about a completed sheet write operation (sheet_id, resolved_name, row/column counts, truncation flag, formula replacements, recalc count, warnings)
- `DfWriterOutcome` (struct, pub) — Outcome of records-to-doc apply with aggregated formula replacements, recalc count, and per-cell warnings
- `FormulaReplacement` (struct, pub) — Single cell whose formula was replaced by literal (sheet, address, prior formula text)
- `write_records_as_new_sheet` (fn, pub) — Write records as a new sheet with collision-aware naming and row truncation
- `apply_records_to_doc` (fn, pub) — Write records into existing sheet, detecting formula-to-literal replacements and cascading recalcs; rejects nonexistent sheets
- `write_one_cell` (fn, private) — Helper that peeks prior formula (if any) before write, records replacement entry, accumulates recalc count and warnings
- `resolve_unique_sheet_name` (fn, pub) — Resolve unique sheet name by appending suffixes (2, 3, ..., 999) or unix timestamp on collision
- `col_letter` (fn, private) — Convert 0-indexed column number to Excel letter notation (A, B, ..., Z, AA, AB, ...)
- `tests` (mod, private) — 13 test functions covering new-sheet writes, collision resolution, formula replacement, cell recalculation, null handling, and truncation

## File-level notes

- **Well-tested**: comprehensive coverage of happy paths, error cases (empty names, missing sheets), formula replacement detection, and cascade recalculation.
- **D-T8 design**: file implements formula-replacement tracking (prior formula text captured before write via `YrsResolver::get_formula`) so callers can emit `formula_replaced_by_literal` CRDT events. Formula-to-formula rewrites are explicitly skipped (not "replaced by literal").
- **Null value semantics**: consistent across both write paths — nulls in records are skipped, preserving existing cell content.
- **Minor optimization note**: `col_letter` uses `String::insert(0, ...)` which is O(n) per insertion (O(n²) overall), acceptable for typical column counts but could use reverse iteration + one append instead. Not a correctness issue.
- **No error conditions unhandled**: sheet creation guarded by existence check, empty names rejected, row truncation tracked, all per-cell outcomes propagated to result.
