# Pattern A — Cell-by-cell diff

**When:** two versions of the same report that should have identical layout (same rows, same columns). You want to spot which cells changed value.

**Note:** if either sheet has a title row in A1 (single non-empty cell, rest empty in row 1), apply the standard header-promotion boilerplate first (see `crdt-doc-run-python` skill, reference `dataframe-shape`).

```python
import pandas as pd
a, b = dfs[sid_a], dfs[sid_b]
# Optional header promotion if A1 is a title row:
# a.columns = a.iloc[0].tolist(); a = a.iloc[1:].reset_index(drop=True)
# b.columns = b.iloc[0].tolist(); b = b.iloc[1:].reset_index(drop=True)

common_cols = [c for c in a.columns if c in b.columns]
diff = a[common_cols].compare(
    b[common_cols].reindex(a.index),
    align_axis=1,
)
# DataFrame.compare returns a MultiIndex on columns — flatten for sheet storage
diff.columns = [f"{c}_{side}" for c, side in diff.columns]
output_sheet = diff.reset_index(names='row_index')
output = f"{len(diff)} cells changed across {len(common_cols)} columns"
```

**Output_sheet columns:** `row_index, <colname>_self, <colname>_other, ...` for each column with at least one differing value.

**Anti-tip:** if shapes differ (one has extra rows), `DataFrame.compare` raises. In that case use Pattern B (row-diff by key) instead.
