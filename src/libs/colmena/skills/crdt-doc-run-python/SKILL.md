---
name: crdt-doc-run-python
description: Use when calling the `crdt_doc_run_python` tool. This is a short index — load specific references for the exact detail you need (DataFrame shape rules, output protocol, type quirks, debugging). Activate as soon as you see `crdt_doc_run_python` in your tool list.
references:
  - name: dataframe-shape
    description: HOW the dispatcher projects sheets into pandas DataFrames. Critical reading — Y.Doc row 1 ALWAYS becomes columns, which means imported xlsx with a title row in A1 need explicit header promotion. Load this before writing any pandas code that references columns by name.
  - name: output-protocol
    description: How to set `output` and `output_sheet`, how `write_to_sheet` works, name collision behaviour. Load when you need to return data or persist a derived sheet.
  - name: type-quirks
    description: Y.Doc serializes all numbers as f64; NaN handling; mixed-type columns. Load if you hit unexpected dtype issues or your numeric comparisons fail.
  - name: debugging-keyerror
    description: How to recover from KeyError / shape errors WITHOUT looping the same bad assumption. Uses `loaded_sheet_columns` from error responses and `print(df.columns)` for fast diagnosis.
---

# crdt-doc-run-python — Index

`crdt_doc_run_python` runs sandboxed Python (pandas + numpy + scipy.stats) against one or more sheets. Full data lives server-side; you analyze without paying token cost per row.

## When to use vs not

- ✅ Aggregations, filters, joins on >50 rows.
- ✅ Statistical / probabilistic analysis.
- ✅ When you'd otherwise have to read hundreds of cells into your context.
- ❌ Setting a single cell — use `crdt_doc_set_cell`.
- ❌ Reading a known small range — use `crdt_doc_read`.

## Sandbox limits

- Imports whitelisted: `pandas`, `numpy`, `scipy`, plus `json`, `math`, `datetime`, `re`, `collections`. Others raise `ImportError` at AST-validation time.
- Timeout: **30 seconds** per call.
- Output cap: `output` and `stdout` truncated at 10 KB each.
- Combined sheet load cap: 100 MB across all `sheet_ids`.

## Load references on demand

Each reference is small and self-contained — load only what applies to your current call:

| Symptom / question | Load reference |
|---|---|
| About to write any pandas code | `dataframe-shape` (essential) |
| Need to persist a derived DataFrame | `output-protocol` |
| Numeric comparisons giving weird results | `type-quirks` |
| Got a KeyError or shape mismatch | `debugging-keyerror` |

For most cross-sheet work you'll load `dataframe-shape` once and the pattern-specific reference from `crdt-doc-cross-sheet-analysis`.

## Anti-patterns (short list)

- ❌ Reading the whole sheet with `crdt_doc_read` "to plan" your code. Use `A1:D5` for shape; let `run_python` handle the rest.
- ❌ Retrying the same `df.iloc[N]` with different N hoping it works. Read `loaded_sheet_columns` from the error first (see `debugging-keyerror`).
- ❌ Hardcoding column positions (`df.iloc[:, 2]`) when you have names (`df['Precio']`).
- ❌ Returning the full DataFrame as `output` for inspection. Use `output_sheet` (persisted) or `df.head().to_dict('records')` (small).
