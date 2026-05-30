---
name: sales-analysis
description: How to analyze sales data — common tables, KPIs, and pitfalls.
---

# Sales analysis

## Tables (assumed)

- `public.orders(id, customer_id, status, total_amount, currency, created_at)`
- `public.order_items(order_id, sku, qty, unit_price)`

## Useful KPIs

- Revenue in a window:
  `SELECT SUM(total_amount) FROM public.orders WHERE status = 'completed' AND created_at >= $1 AND created_at < $2`
- Top SKUs by units:
  `SELECT sku, SUM(qty) AS units FROM public.order_items GROUP BY sku ORDER BY units DESC LIMIT 10`

## Pitfalls

- Filter out `status IN ('cancelled', 'refunded')` when computing revenue
  unless the user explicitly wants gross.
- Currencies may mix; if the dataset is multi-currency, group by currency
  or convert explicitly.
