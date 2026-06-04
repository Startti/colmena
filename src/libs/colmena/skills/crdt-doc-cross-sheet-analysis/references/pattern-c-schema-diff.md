# Pattern C — Schema diff

**When:** quick structural check — "qué columnas existen en cada uno y con qué tipo". Useful as a sanity check before any deeper diff (e.g. detect that Q4 added a "Discount" column or renamed "Precio" to "Price").

```python
import pandas as pd
a, b = dfs[sid_a], dfs[sid_b]
# Optional header promotion if A1 is a title row.

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

**Output_sheet columns:** `column, in_A, in_B, dtype_A, dtype_B` — one row per distinct column across both sheets.

**Use case:** if the user's broader ask is "compare these reports" but the schemas don't match, run THIS first and surface the structural mismatch in chat before doing a value-level diff.
