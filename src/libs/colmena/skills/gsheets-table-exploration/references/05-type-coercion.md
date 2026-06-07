# 05 — Type coercion

Google Sheets sends numbers and dates in inconsistent ways. Always
coerce before doing math.

## Numbers

```python
import pandas as pd
df = pd.DataFrame(products)

# Convert a column that may contain stringy numbers (e.g., '44')
df['price'] = pd.to_numeric(df['price'], errors='coerce')
df['cost']  = pd.to_numeric(df['cost'],  errors='coerce')

# After this, rows with unparseable values become NaN. Filter or fill:
df = df.dropna(subset=['price'])
# OR
df['price'] = df['price'].fillna(0)
```

## Dates

```python
df['date'] = pd.to_datetime(df['date'], errors='coerce')
# Now you can use .dt accessors:
df['month'] = df['date'].dt.month
df_q1 = df[df['date'].dt.quarter == 1]
```

## The leading apostrophe trap

Google Sheets prefixes a cell with `'` to force "store as text". When
you read the cell via the API, the `'` does NOT appear in the value
(it's metadata), but the cell is still stored as a STRING. Pandas
inferring types will see a string column.

Symptom: `df['price'].sum()` produces a string concatenation instead of
a numeric sum.

Cure: always `pd.to_numeric` on columns you intend to do math on.

## Boolean values from Google Sheets

Booleans round-trip cleanly via the API. If you get strings `"TRUE"` /
`"FALSE"` instead, the column was imported as text:

```python
df['active'] = df['active'].astype(str).str.upper() == 'TRUE'
```
