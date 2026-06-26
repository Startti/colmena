---
name: gsheets-editing
description: Use when WRITING or EDITING a Google Sheet with the gsheets tools — pick the right tool/mode for the edit (set_cell vs set_range vs run_python update_in_place/overwrite/new-tab), edit rows by a unique OR a non-unique key, and create + populate sheets. Covers the decision table, the duplicate-key pitfall, and 1-based row / column-letter rules. Load the reference for your scenario.
references:
  - name: edit-rows
    description: Edit existing rows that match a condition. By a UNIQUE key column → run_python + update_in_place. By a value that REPEATS (or by position) → run_python to find the 1-based row numbers, then gsheets_set_cell per cell. Worked pandas code for both, plus the duplicate-key failure and why overwrite is wrong here.
  - name: create-and-populate
    description: Create new containers. New tab WITH data → name it in run_python output_sheets (creates + fills in one call). Empty tab → gsheets_add_sheet. Brand-new spreadsheet file → gsheets_create_spreadsheet then populate. Examples of each.
  - name: cell-and-range-writes
    description: Direct writes without code — gsheets_set_cell (one cell, formulas with leading =) and gsheets_set_range (a 2-D block from an A1 anchor). How to derive the A1 address — 1-based rows (header is row 1) and the column letter from the header position.
---

# Editing Google Sheets — choose the right write mechanism

The gsheets toolkit has several ways to change a sheet. Picking the wrong one
either fails (duplicate-key) or destroys data (overwrite). Choose by the SHAPE
of the edit:

| You want to… | Use | Avoid |
|---|---|---|
| Set a few KNOWN cells (you know the A1 addresses) | `gsheets_set_cell` (one) / `gsheets_set_range` (contiguous block) | — |
| Edit rows matched by a **UNIQUE** key column | `gsheets_run_python` + `output_sheets` mode `update_in_place` (`key` = that column) | — |
| Edit rows matched by a **NON-UNIQUE** value, or by position | `gsheets_run_python` to find the row numbers → `gsheets_set_cell` per cell | **NOT** `update_in_place` (fails on duplicate keys); **NOT** `overwrite` |
| Create a NEW tab **with data** | `gsheets_run_python` with a NEW sheet name in `output_sheets` (creates + fills) | `add_sheet` then `set_range` (two steps) |
| Create a new **empty** tab | `gsheets_add_sheet` | — |
| Create a new **spreadsheet file** | `gsheets_create_spreadsheet`, then populate | — |
| Append rows at the bottom | `gsheets_set_range` from the first free row | — |
| Rebuild a whole existing tab (intentional) | `gsheets_run_python` + `output_sheets` mode `overwrite` | overwrite by default — it drops formatting/formulas |
| Compute over ALL rows (filter/aggregate/dedupe) | `gsheets_run_python` (rows never pass through the model) | reading + eyeballing a truncated preview |

## Rules that bite people

- **`update_in_place` needs a UNIQUE key.** If the identifying value repeats
  across rows (line-items sharing one id), it fails. Fall back to finding row
  numbers in code → `gsheets_set_cell`. See the `edit-rows` reference.
- **Row numbers are 1-based and the header is row 1.** A pandas row at
  DataFrame index `i` (header excluded) is sheet row `i + 2`.
- **Column letter = position in the header.** The Nth data column (0-based N)
  maps to its A1 letter (col 0 → A, 18 → S, …). Derive it from the header you
  read; don't guess.
- **Writing to an existing tab fails by default** (collision policy) — only
  `update_in_place` patches in place. A bare DataFrame / `overwrite` replaces.
- **Never `overwrite` to change a few cells** — it wipes everything else in the
  tab (formatting, formulas, untouched columns).

Load the reference that matches your task: `edit-rows`, `create-and-populate`,
or `cell-and-range-writes`.
