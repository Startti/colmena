# 03 — Filter and query

Pandas offers two equivalent styles for filtering. Pick whichever reads
better for your query.

## Boolean mask (explicit)

```python
import pandas as pd
df = pd.DataFrame(products)
laptops = df[df['category'] == 'Laptops']
expensive = df[(df['price'] > 1000) & (df['category'] == 'Laptops')]
```

## `df.query()` (string DSL — often nicer for AND/OR chains)

```python
laptops = df.query("category == 'Laptops'")
expensive = df.query("price > 1000 and category == 'Laptops'")
# Reference an outer variable with @
threshold = 1500
above = df.query("price > @threshold")
```

## Set / range filters

```python
chosen = df[df['category'].isin(['Laptops', 'Smartphones'])]
in_range = df[df['price'].between(500, 1500)]
```

## Null handling

```python
df_clean = df[df['price'].notna()]      # drop rows with NaN price
df_zero  = df['price'].fillna(0)        # replace NaN with 0
```

## Combining filter + top-N

```python
top_3_laptops = df.query("category == 'Laptops'").nlargest(3, 'price')
```
