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

- Don't issue any write — you are read-only. If the user asks you to "fix" or "add stock", clarify and hand off to the writer role.
- An item with qty exactly equal to reorder_point IS at risk — include it (the `<=` in the query, not `<`).
