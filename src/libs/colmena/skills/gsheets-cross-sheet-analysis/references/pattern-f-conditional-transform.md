# Pattern F — Conditional transform from another table

> **Use `data_run_python` for any analysis over >50 rows** — pass each sheet as a binding; the rows never load into LLM context. Use `gsheets_read` only for inspection / small reads / `value_render: "FORMULA"`. The code in this reference is the body of `data_run_python`s `code` argument; bind each sheet under the records-list name you pick (e.g. `records_a`, `records_b`).

> Same pattern as the crdt-doc equivalent — see `crdt-doc-cross-sheet-analysis` if you need the local-CRDT variant.

**When:** you have a rules table (discounts by region with min quantity, tax brackets, promotion eligibility...) and want to apply it row-by-row to your primary table. User language: "aplicale las reglas de", "calculá descuentos según", "marcale los que califican".

## Data flow

1. Call `data_run_python` binding each sheet directly (rows never enter context):
   `data_run_python({ bindings: [ {var: "records_ventas", spreadsheet_id: <id_ventas>, sheet: <tab_ventas>}, {var: "records_reglas", spreadsheet_id: <id_reglas>, sheet: <tab_reglas>} ], code: <below>, write_to_spreadsheet: <id_out> })`
2. The `code` applies the rules row-by-row and assigns the `output_sheets` sink to write the transformed table back; `output` carries how many rows were affected.

## Script

```python
import pandas as pd
ventas = pd.DataFrame(records_ventas)
reglas = pd.DataFrame(records_reglas)

# Bring rule columns alongside each row of the primary
ventas = ventas.merge(reglas, on='Region', how='left')

# Apply the rule per-row
mask = ventas['Cantidad'] >= ventas['MinQty']
ventas['Descuento'] = 0.0
ventas.loc[mask, 'Descuento'] = (
    ventas.loc[mask, 'Precio'] *
    ventas.loc[mask, 'DiscountPct'] / 100
)
ventas['PrecioFinal'] = ventas['Precio'] - ventas['Descuento']

# Drop the rule columns from the final output (they were a means to an end)
transformed = ventas.drop(columns=['MinQty', 'DiscountPct'])

# Write the transformed table back into a "Descuentos" tab, in the same call.
output_sheets = {"Descuentos": {"mode": "replace", "df": transformed}}
output = f"Applied discounts to {int(mask.sum())}/{len(ventas)} rows"
```

## Output

- The `output_sheets` write creates a `Descuentos` tab whose columns are the primary columns + the new derived columns (e.g. `Descuento`, `PrecioFinal`). Rule columns are dropped to keep the result clean.
- The write happens INSIDE the `data_run_python` call via the `output_sheets` sink (the DataFrame goes in the `df` field). You get back sink metadata, never the row contents.
- `output` carries only the short summary string (rows affected / total).

**Pattern:**
1. Merge the rules in via the matching join key.
2. Compute a boolean `mask` per row using the rule columns.
3. Assign new column values conditionally with `df.loc[mask, 'new_col'] = ...`.
4. Drop the rule columns from `result`.

**Gotcha:** if the rule lookup misses (no matching region), the rule columns become NaN; the mask gets evaluated as False for those rows (no transform applied). Decide if that's the behavior you want or if you should error / use a default.
