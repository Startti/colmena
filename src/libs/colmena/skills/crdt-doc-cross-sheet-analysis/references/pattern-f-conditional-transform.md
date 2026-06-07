# Pattern F — Conditional transform from another table

**When:** you have a rules table (discounts by region with min quantity, tax brackets, promotion eligibility...) and want to apply it row-by-row to your primary table. User language: "aplicale las reglas de", "calculá descuentos según", "marcale los que califican".

```python
import pandas as pd
ventas, reglas = dfs[sid_ventas], dfs[sid_reglas]
# Optional header promotion on both if title row.

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
result = ventas.drop(columns=['MinQty', 'DiscountPct'])
output_sheets = {'Transformed': result}
output = f"Applied discounts to {int(mask.sum())}/{len(ventas)} rows"
```

**Result columns:** primary columns + the new derived columns (e.g. `Descuento`, `PrecioFinal`). Rule columns dropped to keep the result clean.

**Pattern:**
1. Merge the rules in via the matching join key.
2. Compute a boolean `mask` per row using the rule columns.
3. Assign new column values conditionally with `df.loc[mask, 'new_col'] = ...`.
4. Drop the rule columns from `result`.

**Gotcha:** if the rule lookup misses (no matching region), the rule columns become NaN; the mask gets evaluated as False for those rows (no transform applied). Decide if that's the behavior you want or if you should error / use a default.
