# Pattern E — Join / enrich

> **Use `gsheets_run_python` for any analysis over >50 rows** — pass each sheet as a binding; the rows never load into LLM context. Use `gsheets_read` only for inspection / small reads / `value_render: "FORMULA"`. The code in this reference is the body of `gsheets_run_python`s `code` argument; bind each sheet under the records-list name you pick (e.g. `records_a`, `records_b`).

> Same pattern as the crdt-doc equivalent — see `crdt-doc-cross-sheet-analysis` if you need the local-CRDT variant.

**When:** you have a primary table (sales, transactions, leads...) and want to add columns from a lookup table (catalog, accounts, classification...). User language: "agregale", "enriquecé con", "trae los precios/categorías de".

## Data flow

1. `gsheets_read({spreadsheet_id: <id_primary>, sheet: <tab_primary>, as_records: true})` → `records_primary`
2. `gsheets_read({spreadsheet_id: <id_lookup>, sheet: <tab_lookup>, as_records: true})` → `records_lookup`
   - If both live in the same spreadsheet, pass the same `spreadsheet_id` twice and only vary `sheet`.
3. `run_python({inputs: {records_primary, records_lookup}, script: <below>})`
4. Write enriched table back with `gsheets_set_range`.

## Script

```python
import pandas as pd
primary = pd.DataFrame(records_primary)
lookup  = pd.DataFrame(records_lookup)

# Pick the columns you want to bring in. If user says "enrich with the prices",
# select only the join key + the relevant column(s):
enriched = primary.merge(
    lookup[['SKU', 'Category', 'Description']],
    on='SKU',
    how='left',   # 'left' keeps all primary rows, NaN where no match
)

# Report unmatched keys so the user can decide if it matters.
unmatched = enriched[enriched['Category'].isna()]
result = enriched
output = {
    'rows_enriched':    len(enriched) - len(unmatched),
    'unmatched_count':  len(unmatched),
    'unmatched_sample': unmatched['SKU'].head(5).tolist(),
}
```

## Output

- `result` columns: original primary columns + the columns brought from the lookup. Rows with no match in the lookup have NaN in the new columns.
- Write back via `gsheets_set_range({spreadsheet_id, sheet, start_addr: "A1", values_2d: [headers] + rows})`. If the destination tab does not exist, create it first with `gsheets_add_sheet`.

**Variants:**
- For "intersect only" semantics (drop unmatched), use `how='inner'`.
- For "outer join" (keep keys from both sides), use `how='outer'`.
- If the key column has a different name on each side: `left_on='SKU', right_on='product_code'`.

**Gotcha:** if the key column has different dtypes (string vs number), the merge silently produces NO matches. Cast both: `primary['SKU'] = primary['SKU'].astype(str)` and same on `lookup`.
