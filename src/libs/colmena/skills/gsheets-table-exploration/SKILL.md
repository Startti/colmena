---
name: gsheets-table-exploration
description: Patterns for exploring a single Google Sheets table — schema inspection first, top-N via nlargest, filters via query, type coercion, and how to ship tabular results back to the spreadsheet. Load BEFORE writing pandas code over one sheet.
when_to_load: When the agent needs to answer a question about one Google Sheets tab (top-N by some column, filter by category, group-and-aggregate) without joining other sheets. Pair this with `gsheets-cross-sheet-analysis` if you also need to merge across tabs.
references:
  - 01-inspect-schema-first
  - 02-top-n-patterns
  - 03-filter-and-query
  - 04-group-and-aggregate
  - 05-type-coercion
  - 06-output-shaping
---

# gsheets-table-exploration

Use this skill when you need to answer a question about ONE tab of a
Google Sheets spreadsheet — top-N, filters, groupings, simple stats.
The only tools you need are `gsheets_read` (peek + tiny reads) and
`data_run_python` (real analysis).

## The cardinal rule

**ALWAYS inspect the table BEFORE writing analytics.** Load reference
`01-inspect-schema-first` for the exact pattern. Skipping this step is
the #1 source of analyses that silently produce wrong answers (missing
columns, type errors, mixed numeric / text values from Google Sheets'
auto-typing).

## Decision tree

1. **Do you know the column names and types?** → If not, load
   `01-inspect-schema-first` and run `df.head(3)` + `df.dtypes` first.
2. **Is the question a top-N by a column?** → Load `02-top-n-patterns`.
   Use `df.nlargest(N, 'col')`, not `sort_values().head()`.
3. **Is the question a filter (where-clause-like)?** → Load
   `03-filter-and-query`.
4. **Is the question a grouped aggregation (sum/avg by category)?** →
   Load `04-group-and-aggregate`.
5. **Are numeric columns coming back as strings?** → Load
   `05-type-coercion`. Google Sheets often imports digits as text.
6. **Does the result belong in the spreadsheet (a new tab) or in the
   conversation (a short text summary)?** → Load `06-output-shaping`.
   Multi-tab write-back via `output_sheets` keeps row data out of the
   LLM context entirely.

## When to load `gsheets-cross-sheet-analysis` instead

Switch to `gsheets-cross-sheet-analysis` when the question involves
**two or more sheets** (joins, enrichment, comparison). That skill
covers `pd.merge`, FK→PK joins, and the cross-artifact `import_sheet`
flow.
