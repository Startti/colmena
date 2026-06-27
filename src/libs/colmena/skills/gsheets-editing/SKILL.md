---
name: gsheets-editing
description: Use when WRITING or EDITING a Google Sheet with the gsheets tools — pick the right tool/mode for the edit (set_cell vs set_range vs run_python update_by_position/update_in_place/overwrite/new-tab), edit rows with no unique key, write live formulas by column name ({{Column}}), and create + populate sheets. Covers the decision table and the duplicate-key pitfall. Load the reference for your scenario.
references:
  - name: edit-rows
    description: Edit existing rows that match a condition. Primary way → run_python + update_by_position (bind the WHOLE sheet, modify the df in place, return it whole; no unique key, no A1 math). update_in_place is the alternative when a column is truly UNIQUE. Also covers writing live formulas with {{Column}} placeholders and filling them down a column/condition/range. Worked pandas code, the full-df contract, and why overwrite / whole-column reassignment are wrong here.
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
| **Edit existing rows matched by a condition (key unique OR not)** | `gsheets_run_python`: bind the whole sheet, modify the df **in place** (`df.loc[mask,'col']=...`), return the WHOLE df under `output_sheets` mode **`update_by_position`** | filtering the returned df; `reset_index`/`sort`+`reset_index`/`concat`; `overwrite` |
| Edit rows by a **UNIQUE** key column (advanced/portable) | `gsheets_run_python` + `update_in_place` (`key` = that column) | — |
| Put a **live formula** in cells (recalculates when inputs change) | `gsheets_run_python`: write `'={{ColA}}*{{ColB}}'` (column NAMES in double braces) to the target rows under `update_by_position`/`update_in_place` — the dispatcher fills the real A1 per row | computing column letters by hand inside the formula (off-by-one → `#VALUE!`) |
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
- **Prefer mechanisms that compute A1 for you.** `update_by_position` and the
  `{{Column}}` formula placeholders resolve the row/column address in the
  dispatcher — do NOT compute column letters or row numbers yourself for these.
  Hand math is off-by-one whenever an empty/duplicate header column shifts the
  positions (you land in the wrong column → `#VALUE!`).
- **Only derive A1 for a KNOWN-address `set_cell`/`set_range`.** There, rows are
  1-based (header is row 1, so DataFrame index `i` → sheet row `i + 2`) and the
  column letter is the header position (col 0 → A, 18 → S, …) — read it, don't
  guess.
- **Writing to an existing tab fails by default** (collision policy) — only
  `update_in_place` patches in place. A bare DataFrame / `overwrite` replaces.
- **Never `overwrite` to change a few cells** — it wipes everything else in the
  tab (formatting, formulas, untouched columns).

Load the reference that matches your task: `edit-rows`, `create-and-populate`,
or `cell-and-range-writes`.
