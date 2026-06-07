---
name: crdt-doc-cross-sheet-analysis
description: Use when comparing two sheets, joining/enriching data from one sheet into another, or transforming rows based on conditions from another sheet. Documents the workflow (list_my_artifacts → list_sheets_of → import_sheet → run_python) and indexes 6 canonical pandas patterns. Load THIS skill first; then load the specific pattern reference you need.
references:
  - name: pattern-a-cell-diff
    description: Cell-by-cell diff between two sheets with identical shape (DataFrame.compare). Use when comparing two versions of the same report.
  - name: pattern-b-row-diff
    description: Row-level diff by a key column — tags each row only_in_A / only_in_B / changed / unchanged. The MOST COMMON case. Use for lists with unique identifiers (SKU, ID, email).
  - name: pattern-c-schema-diff
    description: Compare column structure of two sheets (which exist where, with what dtype). Quick structural check before any deeper diff.
  - name: pattern-d-statistical
    description: Statistical comparison of numeric columns (mean, std, t-test) to detect drift between two snapshots.
  - name: pattern-e-join-enrich
    description: Bring columns from one sheet into another via left join (e.g. add Category from a catalog). Reports unmatched keys.
  - name: pattern-f-conditional-transform
    description: Apply per-row rules defined in another sheet (e.g. discounts by Region with min Qty). Merge + mask + conditional assignment.
---

# crdt-doc-cross-sheet-analysis — Index

Compare, join, enrich and transform data across sheets that may live in different artifacts. Source sheets are **cloned** into the current artifact (snapshot, no live link); from there it's standard pandas multi-sheet work.

## The canonical flow

1. `crdt_doc_list_my_artifacts` — discover artifacts in your session.
2. `crdt_doc_list_sheets_of({artifact_id})` — peek at the other artifact's sheets without cloning.
3. `crdt_doc_import_sheet({source_artifact_id, source_sheet_id})` — clone the sheet into the current artifact.
4. `crdt_doc_run_python({sheet_ids: [original, cloned], code})` — do the analysis. Inside the code, assign `output_sheets = {<tab_name>: <DataFrame>}` to persist results as new tabs.

Before importing, call `crdt_doc_list_sheets` on the current artifact — the sheet may already be cloned from a previous turn.

## When to load which reference

Decide ONE pattern based on what the user is asking for. Then `load_skill('crdt-doc-cross-sheet-analysis', reference='<pattern-name>')` to get the verbatim code:

| User says... | Load reference |
|---|---|
| "compará", "qué cambió", "diferencias entre" with a key column | `pattern-b-row-diff` |
| "compará" sin key (mismo shape, mismas columnas y filas) | `pattern-a-cell-diff` |
| "qué columnas tiene cada uno" / structural check | `pattern-c-schema-diff` |
| "los precios cambiaron significativamente" / "hay drift" | `pattern-d-statistical` |
| "agregale [columna]", "enriquecé con", "trae los precios de" | `pattern-e-join-enrich` |
| "aplicale las reglas de", "calculá descuentos según" | `pattern-f-conditional-transform` |

For multi-output requests ("comparalas y enriquecé"), load multiple references in sequence — one `run_python` call per pattern.

## Updating existing tabs in place

When you want to change values for SOME rows in an existing tab WITHOUT
re-uploading the whole tab, use `update_in_place`. Example: change the
price of all electronics in a 1000-row Sales tab.

```python
import pandas as pd
sales = pd.DataFrame(sales_records)
mask = sales['category'] == 'Electronics'
sales.loc[mask, 'price'] = sales.loc[mask, 'price'] * 0.9  # 10% discount

output_sheets = {
    'Sales': {
        'mode': 'update_in_place',
        'df': sales,
        'key': 'product_id',
        'columns': ['price'],  # only patch this column
    }
}
output = {'updated_rows': int(mask.sum())}
```

The dispatcher computes the diff vs the live artifact and writes ONLY the
changed cells via per-cell ops — your 47-row change is 47 cell writes,
not 12000.

**Rules:**
- `key` must be a column with unique values (duplicates in either side reject).
- `columns` is optional — omit to patch all common columns.
- If your DataFrame has rows not in the tab, they're silently skipped
  (set `strict_match: True` to reject instead).
- If you want to CREATE a new tab, use the simple form `output_sheets = {name: df}`.

**Avoid:**
- Don't use `update_in_place` to ADD new rows (it only patches existing
  ones). Use `mode: 'overwrite'` or write a fresh tab via the bare-DataFrame form.

## Anti-patterns

- ❌ Importing a sheet that is already cloned in this artifact. Always call `crdt_doc_list_sheets` first.
- ❌ Importing the principal back into itself (the tool rejects this with `self_import_forbidden`).
- ❌ Forcing a merge with mixed-type key columns without `pd.to_numeric` / `astype(str)` on both sides.
- ❌ Loading 4 sheets when you only need 2 — the 100 MB combined cap applies.

## Cleanup

Cloned sheets persist in the current artifact. v1 has no delete-sheet tool; the 100-sheets-per-artifact cap prevents runaway accumulation.
