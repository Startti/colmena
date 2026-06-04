# Pattern E — Join / enrich

**When:** you have a primary table (sales, transactions, leads...) and want to add columns from a lookup table (catalog, accounts, classification...). User language: "agregale", "enriquecé con", "trae los precios/categorías de".

```python
import pandas as pd
primary, lookup = dfs[sid_primary], dfs[sid_lookup]
# Optional header promotion on both if title row.

# Pick the columns you want to bring in. If user says "enrich with the prices",
# select only the join key + the relevant column(s):
enriched = primary.merge(
    lookup[['SKU', 'Category', 'Description']],
    on='SKU',
    how='left',   # 'left' keeps all primary rows, NaN where no match
)

# Report unmatched keys so the user can decide if it matters.
unmatched = enriched[enriched['Category'].isna()]
output_sheet = enriched
output = {
    'rows_enriched':    len(enriched) - len(unmatched),
    'unmatched_count':  len(unmatched),
    'unmatched_sample': unmatched['SKU'].head(5).tolist(),
}
```

**Output_sheet columns:** original primary columns + the columns brought from the lookup. Rows with no match in the lookup have NaN in the new columns.

**Variants:**
- For "intersect only" semantics (drop unmatched), use `how='inner'`.
- For "outer join" (keep keys from both sides), use `how='outer'`.
- If the key column has a different name on each side: `left_on='SKU', right_on='product_code'`.

**Gotcha:** if the key column has different dtypes (string vs number), the merge silently produces NO matches. Cast both: `primary['SKU'] = primary['SKU'].astype(str)` and same on `lookup`.
