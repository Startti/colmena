# Pattern D — Statistical comparison

> Same pattern as the crdt-doc equivalent — see `crdt-doc-cross-sheet-analysis` if you need the local-CRDT variant.

**When:** the user wants to know if numeric column distributions changed significantly between two snapshots (drift detection). Reports mean, std, median, and a t-test p-value per column.

## Data flow

1. `gsheets_read({spreadsheet_id: <id_a>, sheet: <tab_a>, as_records: true})` → `records_a`
2. `gsheets_read({spreadsheet_id: <id_b>, sheet: <tab_b>, as_records: true})` → `records_b`
3. `run_python({inputs: {records_a, records_b}, script: <below>})`

## Script

```python
import pandas as pd
from scipy import stats
a = pd.DataFrame(records_a)
b = pd.DataFrame(records_b)

numeric_cols = [
    c for c in a.columns
    if c in b.columns and pd.api.types.is_numeric_dtype(a[c])
]
rows = []
for c in numeric_cols:
    sa, sb = a[c].dropna(), b[c].dropna()
    if len(sa) < 2 or len(sb) < 2:
        continue
    t, p = stats.ttest_ind(sa, sb, equal_var=False)  # Welch's t-test (no equal-variance assumption)
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

## Output

- `output_sheet` columns: `column, mean_A, mean_B, std_A, std_B, median_A, median_B, t_stat, p_value, sig`.
- Write back via `gsheets_set_range` if the user wants a persistent report.

**Gotchas:**
- Columns with `<2` non-null values per side are silently skipped (t-test undefined).
- Pandas may have read numeric columns as `object` if there were any non-numeric cells. Cast with `pd.to_numeric(..., errors='coerce')` BEFORE this loop if `is_numeric_dtype` rejects what should be a numeric column.
- p<0.05 is a default cutoff; for small samples the t-test power is low — interpret with care.
