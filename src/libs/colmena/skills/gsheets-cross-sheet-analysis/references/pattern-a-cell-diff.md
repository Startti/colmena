# Pattern A — Cell-by-cell diff

> Same pattern as the crdt-doc equivalent — see `crdt-doc-cross-sheet-analysis` if you need the local-CRDT variant.

**When:** two versions of the same report that should have identical layout (same rows, same columns). You want to spot which cells changed value.

## Data flow

1. `gsheets_read({spreadsheet_id: <id_a>, sheet: <tab_a>, as_records: true})` → `records_a`
2. `gsheets_read({spreadsheet_id: <id_b>, sheet: <tab_b>, as_records: true})` → `records_b`
3. `run_python({inputs: {records_a, records_b}, script: <below>})`
4. Write result back with `gsheets_set_range`.

## Script

```python
import pandas as pd
a = pd.DataFrame(records_a)
b = pd.DataFrame(records_b)

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

## Output

- `output_sheet` columns: `row_index, <colname>_self, <colname>_other, ...` for each column with at least one differing value.
- Write back as 2D array: `[output_sheet.columns.tolist()] + output_sheet.values.tolist()` via `gsheets_set_range({spreadsheet_id, sheet, start_addr: "A1", values_2d: <2d>})`.

**Anti-tip:** if shapes differ (one has extra rows), `DataFrame.compare` raises. In that case use Pattern B (row-diff by key) instead.
