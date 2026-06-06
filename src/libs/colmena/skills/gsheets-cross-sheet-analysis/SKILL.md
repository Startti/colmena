---
name: gsheets-cross-sheet-analysis
description: Use when comparing two sheets, joining/enriching data from one sheet into another, or transforming rows based on conditions from another sheet — same patterns as crdt-doc-cross-sheet-analysis but for Google Sheets via gsheets_* tools. Load THIS skill first; then load the specific pattern reference you need.
references:
  - name: pattern-a-cell-diff
    description: Cell-by-cell diff between two sheets with identical shape (DataFrame.compare). Use when comparing two versions of the same report.
  - name: pattern-b-row-diff
    description: Row-level diff by a key column — tags each row only_in_A / only_in_B / changed / unchanged. The MOST COMMON case.
  - name: pattern-c-schema-diff
    description: Compare column structure of two sheets (which exist where, with what dtype).
  - name: pattern-d-statistical
    description: Statistical comparison of numeric columns (mean, std, t-test) to detect drift between two snapshots.
  - name: pattern-e-join-enrich
    description: Bring columns from one sheet into another via left join. Reports unmatched keys.
  - name: pattern-f-conditional-transform
    description: Apply per-row rules defined in another sheet (e.g. discounts by Region with min Qty).
---

# gsheets-cross-sheet-analysis — Index

Compare, join, enrich and transform data across Google Sheets tabs that
may live in different spreadsheets. Source data is **read into pandas**;
results are written back via `gsheets_set_range`.

## The canonical flow

1. `gsheets_list_sheets({spreadsheet_id})` — discover tabs in the
   spreadsheet you have an id for.
2. `gsheets_read({spreadsheet_id, sheet, range?, as_records: true})` —
   pull source data as `[{col: val, ...}, ...]` records, ready for
   `pd.DataFrame(records)`.
3. `run_python({script, inputs})` — do the analysis (joins, diffs,
   pivots, etc.). Output is more records.
4. `gsheets_add_sheet({spreadsheet_id, name})` if needed, then
   `gsheets_set_range({spreadsheet_id, sheet, start_addr, values_2d})`
   with `[headers, ...rows]` as the 2D array.

## When to load which reference

Decide ONE pattern based on what the user is asking for. Then `load_skill('gsheets-cross-sheet-analysis', reference='<pattern-name>')` to get the verbatim code:

| User says… | Load reference |
|---|---|
| "compará", "qué cambió", "diferencias entre" with a key column | `pattern-b-row-diff` |
| "compará" sin key (mismo shape, mismas columnas y filas) | `pattern-a-cell-diff` |
| "qué columnas tiene cada uno" / structural check | `pattern-c-schema-diff` |
| "los precios cambiaron significativamente" / "hay drift" | `pattern-d-statistical` |
| "agregale [columna]", "enriquecé con", "trae los precios de" | `pattern-e-join-enrich` |
| "aplicale las reglas de", "calculá descuentos según" | `pattern-f-conditional-transform` |

For multi-output requests ("comparalas y enriquecé"), load multiple references in sequence — one analysis per pattern.

Tool name parity with the local-CRDT skill is intentional — same
patterns, different backend.

## Working with multiple spreadsheets

`gsheets_*` tools all take an explicit `spreadsheet_id`. To compare
across two spreadsheets, the agent receives BOTH ids in its prompt (or
via prior tool results) and threads them per call. There is no
"current spreadsheet" or session state — every call is explicit.

If both sheets live in the same spreadsheet, you still pass
`spreadsheet_id` on every call and only vary the `sheet` (tab title).

## Anti-patterns

- ❌ Calling `gsheets_read` without `as_records: true` when you intend
  to feed pandas — you'll get a 2D array and have to reshape manually.
- ❌ Forcing a merge with mixed-type key columns without `pd.to_numeric`
  / `astype(str)` on both sides.
- ❌ Writing back with `gsheets_set_range` without first ensuring the
  destination sheet exists (use `gsheets_add_sheet` or check
  `gsheets_list_sheets`).
