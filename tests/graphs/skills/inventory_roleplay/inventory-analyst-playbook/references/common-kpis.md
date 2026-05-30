# Common KPIs

## Units sold in a window

```sql
SELECT SUM(-qty_delta) AS units
FROM roleplay_t22.transactions
WHERE kind = 'sale'
  AND created_at >= '<start>' AND created_at < '<end>';
```

## Top 5 sellers in a window

```sql
SELECT sku, SUM(-qty_delta) AS units
FROM roleplay_t22.transactions
WHERE kind = 'sale'
  AND created_at >= '<start>'
GROUP BY sku
ORDER BY units DESC
LIMIT 5;
```

## Slow movers (no sales in last 30 days)

```sql
SELECT i.sku, i.name, i.qty
FROM roleplay_t22.inventory i
LEFT JOIN roleplay_t22.transactions t
  ON t.sku = i.sku AND t.kind = 'sale' AND t.created_at > NOW() - INTERVAL '30 days'
WHERE t.id IS NULL;
```
