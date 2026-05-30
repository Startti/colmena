---
name: expense-analysis
description: How to analyze expenses — categories, vendor rollups, period comparisons.
---

# Expense analysis

## Tables (assumed)

- `public.expenses(id, vendor, category, amount, currency, paid_at)`

## Useful KPIs

- Spend by category in a window:
  `SELECT category, SUM(amount) FROM public.expenses WHERE paid_at >= $1 GROUP BY category ORDER BY SUM(amount) DESC`
- Top vendors by spend YTD:
  `SELECT vendor, SUM(amount) FROM public.expenses WHERE paid_at >= date_trunc('year', NOW()) GROUP BY vendor ORDER BY SUM(amount) DESC LIMIT 10`

## Pitfalls

- Reimbursements may appear as negative amounts — `SUM(amount)` gives net
  spend; use `SUM(GREATEST(amount, 0))` for gross.
