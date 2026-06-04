---
name: crdt-doc-cross-sheet-analysis
description: Use when comparing two sheets, joining/enriching data from one sheet into another, or transforming rows based on conditions from another sheet. Activates the workflow list_my_artifacts → list_sheets_of → import_sheet → run_python. Documents 6 canonical pandas patterns with verbatim code snippets. Load this BEFORE writing any compare/join/enrich code.
---

# crdt-doc-cross-sheet-analysis — Operator Manual

The CRDT documents toolkit lets you compare, join, enrich and transform data
across sheets that may live in **different artifacts**. Source sheets are
**cloned** into your current artifact (snapshot, no live link); from that
point on it is standard pandas multi-sheet work.

## The canonical flow

1. `crdt_doc_list_my_artifacts` — discover artifacts in your session.
2. `crdt_doc_list_sheets_of({artifact_id})` — peek at the other artifact's sheets without cloning.
3. `crdt_doc_import_sheet({source_artifact_id, source_sheet_id})` — clone the sheet you need into the current artifact. Returns a new local `sheet_id`.
4. `crdt_doc_run_python({sheet_ids: [original, cloned], code: "...", write_to_sheet: "..."})` — do the analysis.

**Tip:** before importing, check `crdt_doc_list_sheets` of the current artifact — the sheet may already be cloned from a previous turn, in which case you can skip step 3.

## The 6 canonical patterns

All examples assume both DataFrames need the standard header promotion if the
source xlsx had a title row (see the `crdt-doc-run-python` skill). The
boilerplate at the top of every snippet is:

```python
import pandas as pd
a, b = dfs[sid_a], dfs[sid_b]
a.columns = a.iloc[0].tolist(); a = a.iloc[1:].reset_index(drop=True)
b.columns = b.iloc[0].tolist(); b = b.iloc[1:].reset_index(drop=True)
```

### Pattern A — Cell-by-cell diff (same shape, what value changed)

When: two versions of the same report that should have identical layout.

```python
# (boilerplate)
common_cols = [c for c in a.columns if c in b.columns]
diff = a[common_cols].compare(b[common_cols].reindex(a.index), align_axis=1)
diff.columns = [f"{c}_{side}" for c, side in diff.columns]
output_sheet = diff.reset_index(names='row_index')
output = f"{len(diff)} cells changed across {len(common_cols)} columns"
```

### Pattern B — Row diff by key column (the most common case)

When: lists with unique identifiers (SKU, ID, email). Tags each row as
`only_in_A`, `only_in_B`, `changed`, or `unchanged`.

```python
# (boilerplate)
merged = a.merge(b, on='SKU', how='outer', suffixes=('_a','_b'), indicator=True)
merged['_status'] = merged['_merge'].map({
    'left_only':  'only_in_A',
    'right_only': 'only_in_B',
    'both':       'present_in_both',
})
# CRITICAL: pandas' merge with indicator=True returns _merge as Categorical,
# and the .map() above propagates that dtype into _status. Writing new values
# ('changed', 'unchanged') to a Categorical column without first registering
# them as categories raises:
#   TypeError: Cannot setitem on a Categorical with a new category, set the categories first
# Cast to plain object dtype BEFORE the conditional reassignment below.
merged['_status'] = merged['_status'].astype('object')

shared = [c.removesuffix('_a') for c in merged.columns
          if c.endswith('_a') and f"{c.removesuffix('_a')}_b" in merged.columns]
def diff_mask(r):
    return any(
        r[f"{c}_a"] != r[f"{c}_b"]
        for c in shared
        if pd.notna(r[f"{c}_a"]) and pd.notna(r[f"{c}_b"])
    )
merged.loc[merged['_status'] == 'present_in_both', '_status'] = merged.apply(
    lambda r: 'changed' if diff_mask(r) else 'unchanged', axis=1)
output_sheet = merged.drop(columns='_merge')
output = merged['_status'].value_counts().to_dict()
```

### Pattern C — Schema diff (which columns exist, with which dtype)

When: quick structural check between two reports.

```python
# (boilerplate)
cols_a, cols_b = set(a.columns), set(b.columns)
all_cols = sorted(cols_a | cols_b)
output_sheet = pd.DataFrame([{
    'column':  c,
    'in_A':    c in cols_a,
    'in_B':    c in cols_b,
    'dtype_A': str(a[c].dtype) if c in cols_a else None,
    'dtype_B': str(b[c].dtype) if c in cols_b else None,
} for c in all_cols])
output = {
    'only_in_A': sorted(cols_a - cols_b),
    'only_in_B': sorted(cols_b - cols_a),
    'in_both':   sorted(cols_a & cols_b),
}
```

### Pattern D — Statistical comparison (numeric drift)

When: you want to know if column distributions changed significantly between two snapshots.

```python
# (boilerplate)
from scipy import stats
numeric_cols = [
    c for c in a.columns
    if c in b.columns and pd.api.types.is_numeric_dtype(a[c])
]
rows = []
for c in numeric_cols:
    sa, sb = a[c].dropna(), b[c].dropna()
    if len(sa) < 2 or len(sb) < 2: continue
    t, p = stats.ttest_ind(sa, sb, equal_var=False)
    rows.append({
        'column':   c,
        'mean_A':   sa.mean(),   'mean_B':   sb.mean(),
        'std_A':    sa.std(),    'std_B':    sb.std(),
        'median_A': sa.median(), 'median_B': sb.median(),
        't_stat':   t,           'p_value':  p,
        'sig':      bool(p < 0.05),
    })
output_sheet = pd.DataFrame(rows)
output = f"{sum(r['sig'] for r in rows)} columns with p<0.05 (significant drift)"
```

### Pattern E — Join / enrich (bring info from another table)

When: you have a primary table (e.g. sales) and want to add columns from a
lookup table (e.g. catalog).

```python
ventas, catalog = dfs[sid_ventas], dfs[sid_catalog]
# (boilerplate for both)
enriched = ventas.merge(catalog[['SKU', 'Category', 'Description']], on='SKU', how='left')
unmatched = enriched[enriched['Category'].isna()]
output_sheet = enriched
output = {
    'rows_enriched':    len(enriched) - len(unmatched),
    'unmatched_count':  len(unmatched),
    'unmatched_sample': unmatched['SKU'].head(5).tolist(),
}
```

### Pattern F — Conditional transform (rules from another table)

When: you have a rules table (e.g. discounts by Region with min Qty) and want
to apply it row-by-row to your primary table.

```python
ventas, reglas = dfs[sid_ventas], dfs[sid_reglas]
# (boilerplate for both)
ventas = ventas.merge(reglas, on='Region', how='left')
mask = ventas['Cantidad'] >= ventas['MinQty']
ventas['Descuento'] = 0.0
ventas.loc[mask, 'Descuento'] = ventas.loc[mask, 'Precio'] * ventas.loc[mask, 'DiscountPct'] / 100
ventas['PrecioFinal'] = ventas['Precio'] - ventas['Descuento']
output_sheet = ventas.drop(columns=['MinQty', 'DiscountPct'])
output = f"Applied discounts to {int(mask.sum())}/{len(ventas)} rows"
```

## Anti-patterns

- ❌ Importing a sheet that is already cloned in this artifact. Always call `crdt_doc_list_sheets` first; the previous turn may have done the import already.
- ❌ Importing the principal back into itself (the tool rejects this with `self_import_forbidden`, but it's a sign you've lost the mental model).
- ❌ Forcing a merge with mixed-type key columns without `pd.to_numeric` / `astype(str)` on both sides. Always cast the join key on both DataFrames to the same dtype.
- ❌ Loading 4 sheets when you only need 2. The 100 MB combined cap applies — be intentional.

## Cleanup

Cloned sheets persist in the current artifact. v1 has no delete-sheet tool;
if you need to free space, point the user at the BACKLOG entry (or rerun in
a fresh artifact). The 100-sheets-per-artifact cap prevents accidental
runaway accumulation.
