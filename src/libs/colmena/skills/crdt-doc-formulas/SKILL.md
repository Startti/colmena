---
name: crdt-doc-formulas
description: Use when writing or reading Excel-style formulas in a CRDT spreadsheet. Includes the {v,f,fs} cell schema, when to use include_formulas=true, and how to react to needs_browser warnings.
references:
  - name: write-formula
    description: How to write formulas via crdt_doc_set_cell / crdt_doc_set_range, the cells_recalculated field, and the four warning kinds (eval_error, cycle, parse_error, needs_browser). Load before any formula write.
  - name: read-with-formulas
    description: Default scalar read vs include_formulas=true read shapes, the {v,f,fs} cell schema, and the meaning of fs values (be / fe / needs_browser). Load when auditing, modifying an existing formula, or differentiating a literal from a computed value.
  - name: needs-browser-fallback
    description: Three responses to a needs_browser warning — accept (Univer will compute it), rewrite (e.g. XLOOKUP → INDEX/MATCH), or ask the user to open the workbook. Load only when you actually see a needs_browser warning in a tool response.
---

# crdt-doc-formulas

Backend understands `=...` formulas. Three things to know:

1. **Writing formulas.** Just call `crdt_doc_set_cell(sheet, addr, "=SUM(A1:A10)")` — the leading `=` triggers parse+evaluate server-side. Dependent cells auto-recalculate. See `write-formula` for examples and the `cells_recalculated` field in the tool response.

2. **Reading formulas vs values.** Default reads return scalar values (pandas-friendly). Pass `include_formulas=true` when you need to see the formula text — useful for auditing, modifying an existing formula, or differentiating a literal `42` from `=2*21`. See `read-with-formulas`.

3. **Browser-only functions.** Some Excel functions (e.g. unknown to the backend formula engine, custom add-ins) can't be evaluated server-side. Cells use `fs="needs_browser"` and the value is the formula text itself as placeholder. When you see a `needs_browser` warning, decide: ignore the cell, ask the user to open the workbook in Univer to refresh, or rewrite the formula using supported functions. See `needs-browser-fallback`.

## Quick reference

- Output of `crdt_doc_set_cell` with a formula: `{ok, cells_recalculated, warnings: []}`.
- Output of `crdt_doc_set_cell` with unsupported function: `warnings: [{kind:"needs_browser", addr, functions}]`. Cell IS written, value is the formula text.
- Output of `crdt_doc_read(include_formulas=true)`: each cell is `{v}` (literal) or `{v,f,fs}` (formula).
- `crdt_doc_list_sheets` returns `formula_count` per sheet — check before calling include_formulas=true to avoid noise.

## When NOT to use formulas

For derived values produced by `run_python`, prefer writing the computed numbers directly — pandas evaluation runs once at write-time, while server-side formulas re-evaluate every time an input changes. Use formulas when you want the formula to be visible to the user / live-updating, literals when you just want a snapshot.
