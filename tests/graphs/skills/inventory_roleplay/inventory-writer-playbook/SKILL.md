---
name: inventory-writer-playbook
description: How to record inventory operations (purchases, sales, returns, adjustments) into the transactions table and keep inventory.qty in sync.
references:
  - name: sku-format
    description: SKU naming convention and validation regex.
  - name: transaction-kinds
    description: Valid values for transactions.kind and what each represents.
---

# Inventory writer — playbook

## Mindset

You are an operator who records movements of stock. Two tables matter:

- `roleplay_t22.inventory(sku, name, qty, reorder_point)` — the current stock per SKU.
- `roleplay_t22.transactions(id, sku, qty_delta, kind, created_at)` — the immutable log of every movement (positive for additions, negative for removals).

Every operation does TWO statements: append a row to `transactions` AND update `inventory.qty` by the same delta. If you skip the transactions log, the audit trail breaks.

## Before writing

- If you do not know the SKU's existence, run `SELECT sku FROM roleplay_t22.inventory WHERE sku = '...'` first.
- For SKU format validation, load reference `sku-format`.
- For the allowed `kind` values, load reference `transaction-kinds`.

## Standard pattern

```sql
INSERT INTO roleplay_t22.transactions (sku, qty_delta, kind)
VALUES ('ABC-001', -3, 'sale');

UPDATE roleplay_t22.inventory
SET qty = qty - 3
WHERE sku = 'ABC-001';
```

## Pitfalls

- `qty` should not go negative. Always read current qty first if the user asks for a decrement; reject if the result would be negative.
- The `kind` field is constrained — don't invent new values without the user explicitly approving.
