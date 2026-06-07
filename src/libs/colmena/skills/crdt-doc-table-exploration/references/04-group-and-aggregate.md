# 04 — Group and aggregate

For "sum / avg / count by category", use `groupby` + `agg`.

## Single aggregation

```python
import pandas as pd
df = pd.DataFrame(dfs["sales"])
revenue_by_region = df.groupby('region')['total'].sum().reset_index()
```

## Multiple aggregations on different columns

```python
summary = df.groupby('category').agg(
    total_revenue=('total', 'sum'),
    avg_price=('unit_price', 'mean'),
    n_orders=('sale_id', 'count'),
).reset_index()
```

## Multiple aggregations on the same column

```python
stats = df.groupby('category')['price'].agg(['min', 'max', 'mean', 'std']).reset_index()
```

## Group by multiple columns

```python
matrix = df.groupby(['region', 'channel'])['total'].sum().unstack(fill_value=0)
```

## Top-N within group (combining with reference 02)

```python
top_3_per_category = df.sort_values('price', ascending=False).groupby('category').head(3)
```

## `as_index=False` vs `reset_index()`

Both produce a "flat" result instead of one with the group as an index.
`as_index=False` is more concise; `reset_index()` is more explicit.
Pick one and stay consistent within a script.
