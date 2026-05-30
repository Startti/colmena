---
name: stock-monitor-playbook
description: How to identify items below their reorder point and suggest reorder quantities.
---

# Stock monitor — playbook

## Mindset

Your job is to spot items at risk of running out. The `reorder_point` field on each inventory row is the threshold below which the item should be restocked.

## Standard query

```sql
SELECT sku, name, qty, reorder_point
FROM roleplay_t22.inventory
WHERE qty <= reorder_point
ORDER BY (reorder_point - qty) DESC;
```

The descending order surfaces the most-critical items first.

## Suggested reorder quantity

A simple rule: order enough to reach 3× the reorder point. So `suggested = max(0, 3*reorder_point - qty)`.

## Pitfalls

- Don't issue any write through THIS tool — `monitor_stock` is read-only. If the user asked to "fix" or "add stock" (e.g. record a purchase to replenish a critical item), switch tools in the same interaction: call `describe_tool("cargar_inventario")` next, then use it to record the operation. Don't ask the user to do it manually — chain the tools yourself and report what you did in both roles.
- An item with qty exactly equal to reorder_point IS at risk — include it (the `<=` in the query, not `<`).
