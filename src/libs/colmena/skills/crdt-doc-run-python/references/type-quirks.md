# Type quirks to remember

## All numbers arrive as float64

Yjs has only one numeric type. Pandas reads `12` as `12.0`. If you need integer comparisons:

```python
df['Qty'].astype(int)
# OR for the assertion side:
assert df['Qty'].iloc[0] == 12.0  # NOT 12
```

This means a strict `df['Sku_ID'] == 12345` will fail if `df['Sku_ID']` is f64 and you compare against int. Cast one side.

## Empty xlsx cells become None/NaN

`Total`-style columns left blank during import arrive as `None` or `NaN`. Arithmetic on them produces `NaN` for the whole row. Defenses:

```python
df['Precio'] = pd.to_numeric(df['Precio'], errors='coerce')   # forces non-numeric → NaN
df = df.dropna(subset=['Precio'])                              # OR drop the rows
df['Precio'] = df['Precio'].fillna(0)                          # OR replace
```

## Mixed-type columns

If an import had a stray text cell in an otherwise-numeric column, pandas may type the whole column as `object`. Symptoms: `is_numeric_dtype(df['x']) → False`, comparisons giving weird results.

```python
df['Precio'] = pd.to_numeric(df['Precio'], errors='coerce')
```

`errors='coerce'` turns un-parseable values into `NaN` instead of raising. You can then `dropna()` or `fillna()` as needed.

## Categorical from `pd.merge(..., indicator=True)`

The `_merge` column from `merge(indicator=True)` is Categorical with values `'left_only'`, `'right_only'`, `'both'`. If you `.map()` it to new values, the result column inherits the Categorical dtype. Writing values OUTSIDE the original categories raises:

```
TypeError: Cannot setitem on a Categorical with a new category, set the categories first
```

Fix: cast to object before writing new values.

```python
merged['_status'] = merged['_merge'].map({'left_only': 'only_in_A', ...})
merged['_status'] = merged['_status'].astype('object')   # CRITICAL
merged.loc[mask, '_status'] = 'changed'                  # now safe
```

## Boolean serialization

`True`/`False` in pandas are fine in `output_sheets` DataFrames, but if you put them in `output` and they end up as numpy `bool_` (from a boolean Series), serialization may fail. Cast explicitly:

```python
output = {'is_match': bool(mask.any())}   # not just mask.any()
```
