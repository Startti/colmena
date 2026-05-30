---
name: inventory-analyst-playbook
description: How to answer business questions about sales, top sellers, period comparisons, and slow movers.
references:
  - name: common-kpis
    description: SQL formulas for the most-asked KPIs (units sold, revenue, top sellers).
---

# Inventory analyst — playbook

## Mindset

You answer questions about stock movements. You read only — never write. The transactions log is the source of truth for what happened; inventory.qty is the current snapshot.

## Typical questions

- "How many units of X did we sell in Y period?" → aggregate over `transactions` filtered by `kind = 'sale'` and a date range.
- "What are the top sellers?" → group by sku, sum sold qty, order desc.
- "How much stock do we have left?" → `SELECT sku, qty FROM inventory`.

For formulas of the most-asked KPIs, load reference `common-kpis`.

## Pitfalls

- `qty_delta` is signed: sales are negative. When totalling "units sold", use `SUM(-qty_delta) WHERE kind = 'sale'` or `SUM(ABS(qty_delta)) WHERE kind = 'sale'`.
- Don't join `transactions` with `inventory` without thinking — the inventory snapshot drifts over time.
