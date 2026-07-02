# Pattern B — Row diff by key column

> **Use `data_run_python` for any analysis over >50 rows** — pass each sheet as a binding; the rows never load into LLM context. Use `gsheets_read` only for inspection / small reads / `value_render: "FORMULA"`. The code in this reference is the body of `data_run_python`s `code` argument; bind each sheet under the records-list name you pick (e.g. `records_a`, `records_b`).

> Same pattern as the crdt-doc equivalent — see `crdt-doc-cross-sheet-analysis` if you need the local-CRDT variant.

**When:** lists with unique identifiers (SKU, ID, email). You want to classify each row as `only_in_A`, `only_in_B`, `changed`, or `unchanged`. **This is the most common case.**

## Data flow

1. Call `data_run_python` binding each sheet directly (rows never enter context):
   `data_run_python({ bindings: [ {var: "records_a", spreadsheet_id: <id_a>, sheet: <tab_a>}, {var: "records_b", spreadsheet_id: <id_b>, sheet: <tab_b>} ], code: <below>, write_to_spreadsheet: <id_out> })`
2. The `code` classifies each row and assigns the `output_sheets` sink to write the classified table back; `output` carries a short status breakdown.

## Script

```python
import pandas as pd
a = pd.DataFrame(records_a)
b = pd.DataFrame(records_b)

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
    lambda r: 'changed' if diff_mask(r) else 'unchanged',
    axis=1,
)
classified_df = merged.drop(columns='_merge')

# Write the classified table back into a "Row diff" tab, in the same call.
output_sheets = {"Row diff": {"mode": "replace", "df": classified_df}}
output = merged['_status'].value_counts().to_dict()
```

## Output

- The `output_sheets` write creates a `Row diff` tab whose columns are the key column + every non-key column suffixed with `_a` / `_b` + a `_status` column.
- The write happens INSIDE the `data_run_python` call via the `output_sheets` sink (the DataFrame goes in the `df` field). You get back sink metadata — never the row contents.
- `output` carries only the small status breakdown dict (e.g. `{"only_in_A": 3, "changed": 7, ...}`).

**Tip:** replace `'SKU'` with the actual key column the user mentioned ("Producto", "ID", "email", etc).

**Tip:** for numeric drift specifically (don't care about which fields, just "did values move"), Pattern D (statistical) may be a better fit.
