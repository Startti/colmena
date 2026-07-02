# Pattern C — Schema diff

> **Use `data_run_python` for any analysis over >50 rows** — pass each sheet as a binding; the rows never load into LLM context. Use `gsheets_read` only for inspection / small reads / `value_render: "FORMULA"`. The code in this reference is the body of `data_run_python`s `code` argument; bind each sheet under the records-list name you pick (e.g. `records_a`, `records_b`).

> Same pattern as the crdt-doc equivalent — see `crdt-doc-cross-sheet-analysis` if you need the local-CRDT variant.

**When:** quick structural check — "qué columnas existen en cada uno y con qué tipo". Useful as a sanity check before any deeper diff (e.g. detect that Q4 added a "Discount" column or renamed "Precio" to "Price").

## Data flow

1. Call `data_run_python` binding each sheet directly (rows never enter context):
   `data_run_python({ bindings: [ {var: "records_a", spreadsheet_id: <id_a>, sheet: <tab_a>}, {var: "records_b", spreadsheet_id: <id_b>, sheet: <tab_b>} ], code: <below> })`
2. This is a pure structural check — no write-back. The `code` surfaces the column breakdown via `output`.

## Script

```python
import pandas as pd
a = pd.DataFrame(records_a)
b = pd.DataFrame(records_b)

cols_a, cols_b = set(a.columns), set(b.columns)
all_cols = sorted(cols_a | cols_b)

per_column = [{
    'column':  c,
    'in_A':    c in cols_a,
    'in_B':    c in cols_b,
    'dtype_A': str(a[c].dtype) if c in cols_a else None,
    'dtype_B': str(b[c].dtype) if c in cols_b else None,
} for c in all_cols]

# Schema tables are small (one row per column) — surface the whole thing in output.
output = {
    'only_in_A':  sorted(cols_a - cols_b),
    'only_in_B':  sorted(cols_b - cols_a),
    'in_both':    sorted(cols_a & cols_b),
    'per_column': per_column,
}
```

## Output

- `output.per_column` holds one record per distinct column: `column, in_A, in_B, dtype_A, dtype_B`.
- `output.only_in_A` / `only_in_B` / `in_both` give the quick set breakdown.
- This is a pure-analysis pattern: it writes nothing back. A schema table is small enough to live entirely in `output` (never dump large row sets there).

**Use case:** if the user's broader ask is "compare these reports" but the schemas don't match, run THIS first and surface the structural mismatch in chat before doing a value-level diff.
