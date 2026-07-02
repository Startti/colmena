# Pattern E — Join / enrich

> **Use `data_run_python` for any analysis over >50 rows** — pass each sheet as a binding; the rows never load into LLM context. Use `gsheets_read` only for inspection / small reads / `value_render: "FORMULA"`. The code in this reference is the body of `data_run_python`s `code` argument; bind each sheet under the records-list name you pick (e.g. `records_a`, `records_b`).

> Same pattern as the crdt-doc equivalent — see `crdt-doc-cross-sheet-analysis` if you need the local-CRDT variant.

**When:** you have a primary table (sales, transactions, leads...) and want to add columns from a lookup table (catalog, accounts, classification...). User language: "agregale", "enriquecé con", "trae los precios/categorías de".

## Data flow

1. Call `data_run_python` binding each sheet directly (rows never enter context):
   `data_run_python({ bindings: [ {var: "records_primary", spreadsheet_id: <id_primary>, sheet: <tab_primary>}, {var: "records_lookup", spreadsheet_id: <id_lookup>, sheet: <tab_lookup>} ], code: <below>, write_to_spreadsheet: <id_out> })`
   - If both live in the same spreadsheet, pass the same `spreadsheet_id` for both bindings and only vary `sheet`.
2. The `code` joins the lookup onto the primary and assigns the `output_sheets` sink to write the enriched table back; `output` carries the match counts.

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

# Write the enriched table back into an "Enriched" tab, in the same call.
output_sheets = {"Enriched": {"mode": "replace", "df": enriched}}
output = {
    'rows_enriched':    len(enriched) - len(unmatched),
    'unmatched_count':  len(unmatched),
    'unmatched_sample': unmatched['SKU'].head(5).tolist(),
}
```

## Output

- The `output_sheets` write creates an `Enriched` tab whose columns are the original primary columns + the columns brought from the lookup. Rows with no match in the lookup have NaN in the new columns.
- The write happens INSIDE the `data_run_python` call via the `output_sheets` sink (the DataFrame goes in the `df` field), which CREATES the tab for you — no separate `gsheets_add_sheet` needed. You get back sink metadata, never the row contents.
- `output` carries only the small match-count summary (enriched / unmatched counts + a sample of unmatched keys).

**Variants:**
- For "intersect only" semantics (drop unmatched), use `how='inner'`.
- For "outer join" (keep keys from both sides), use `how='outer'`.
- If the key column has a different name on each side: `left_on='SKU', right_on='product_code'`.

**Gotcha:** if the key column has different dtypes (string vs number), the merge silently produces NO matches. Cast both: `primary['SKU'] = primary['SKU'].astype(str)` and same on `lookup`.
