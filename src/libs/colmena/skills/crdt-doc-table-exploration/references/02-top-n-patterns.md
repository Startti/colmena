# 02 — Top-N patterns

For "the N largest/smallest by some column", use `nlargest` / `nsmallest`.
Do NOT use `sort_values().head()` — the latter is a strict superset of
work for the same result.

## Top-N by one column

```python
import pandas as pd
df = pd.DataFrame(dfs["products"])
top_5 = df.nlargest(5, 'price')[['product_id', 'name', 'price']]
output = top_5.to_dict('records')
```

## Bottom-N

```python
bottom_5 = df.nsmallest(5, 'price')
```

## Top-N by a computed column

```python
df['margin'] = df['price'] - df['cost']
top_5_margin = df.nlargest(5, 'margin')[['name', 'margin']]
```

## Top-N within each group

```python
# Top 3 per category
df.groupby('category').apply(
    lambda g: g.nlargest(3, 'price')
).reset_index(drop=True)
```

## Anti-pattern: do NOT use max_rows_to_load for top-N

There is no `max_rows_to_load` parameter on bindings. Even if there were,
loading only the first N rows then sorting would produce arbitrary
results — the first N rows are not the largest. Load the column you
need to rank by, do the ranking in pandas, then return just the top-N.

For large CRDT sheets, use `crdt_doc_read` with a `range` to fetch only a slice.
