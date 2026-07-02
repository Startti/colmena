# Pattern D — Statistical comparison

> **Use `data_run_python` for any analysis over >50 rows** — pass each sheet as a binding; the rows never load into LLM context. Use `gsheets_read` only for inspection / small reads / `value_render: "FORMULA"`. The code in this reference is the body of `data_run_python`s `code` argument; bind each sheet under the records-list name you pick (e.g. `records_a`, `records_b`).

> Same pattern as the crdt-doc equivalent — see `crdt-doc-cross-sheet-analysis` if you need the local-CRDT variant.

**When:** the user wants to know if numeric column distributions changed significantly between two snapshots (drift detection). Reports mean, std, median, and a t-test p-value per column.

## Data flow

1. Call `data_run_python` binding each sheet directly (rows never enter context):
   `data_run_python({ bindings: [ {var: "records_a", spreadsheet_id: <id_a>, sheet: <tab_a>}, {var: "records_b", spreadsheet_id: <id_b>, sheet: <tab_b>} ], code: <below> })`
   - The stats table is small (one row per numeric column), so the default flow surfaces it via `output`. If the user wants a persistent report, add `write_to_spreadsheet: <id_out>` and assign the `output_sheets` sink (commented below).
2. The `code` runs a Welch t-test per numeric column; `output` carries the table plus a short significance count.

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
stats_df = pd.DataFrame(rows)

# One row per numeric column — small enough to return inline.
output = {
    'summary': f"{sum(r['sig'] for r in rows)} columns with p<0.05 (significant drift)",
    'per_column': rows,
}

# Optional persistent report: only when the tool was called with write_to_spreadsheet.
# output_sheets = {"Drift": {"mode": "replace", "df": stats_df}}
```

## Output

- `output.per_column` holds one record per numeric column: `column, mean_A, mean_B, std_A, std_B, median_A, median_B, t_stat, p_value, sig`. `output.summary` is the one-line significance count.
- To persist a report, call `data_run_python` with `write_to_spreadsheet: <id_out>` and uncomment the `output_sheets` sink assignment — the write then happens INSIDE the same call (the DataFrame goes in the `df` field), returning sink metadata, not rows.

**Gotchas:**
- Columns with `<2` non-null values per side are silently skipped (t-test undefined).
- Pandas may have read numeric columns as `object` if there were any non-numeric cells. Cast with `pd.to_numeric(..., errors='coerce')` BEFORE this loop if `is_numeric_dtype` rejects what should be a numeric column.
- p<0.05 is a default cutoff; for small samples the t-test power is low — interpret with care.
