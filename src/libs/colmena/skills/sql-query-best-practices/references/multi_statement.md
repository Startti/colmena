---
name: multi_statement
description: How multi-statement queries work — atomicity, output semantics, LIMIT, ordering. Read before sending more than one ; in a query.
---

# Multi-statement queries

Use this when your call has more than one `;` in it.

## How it works

The whole query runs as a SINGLE transaction:

1. Statements execute top to bottom, sequentially.
2. If any statement fails → rollback EVERYTHING.
3. The result returned to you is the output of the LAST statement.
4. SELECTs that are NOT the last statement run normally but their rows
   are discarded.
5. `LIMIT` auto-injection applies ONLY to the last SELECT.

You never need (and must not write) BEGIN, COMMIT, or ROLLBACK.

## ✅ Patterns

### Mutation + confirm SELECT at the end
```sql
INSERT INTO orders (user_id, amount) VALUES (1, 100), (2, 200);
SELECT id, user_id, amount, created_at
  FROM orders
  WHERE user_id IN (1, 2)
  ORDER BY id DESC;
```
Output = the rows from the final SELECT. The INSERT's row count is gone.

### Multiple writes to related tables
```sql
INSERT INTO orders (id, user_id, total) VALUES (123, 42, 250.00);
INSERT INTO order_items (order_id, product_id, qty) VALUES
  (123, 1, 2),
  (123, 7, 1);
SELECT o.id, o.total, array_agg(i.product_id ORDER BY i.product_id) AS products
FROM orders o
JOIN order_items i ON i.order_id = o.id
WHERE o.id = 123
GROUP BY o.id;
```

### Multiple UPDATEs with verification
```sql
UPDATE products SET price = price * 1.10 WHERE category = 'A';
UPDATE products SET price = price * 1.05 WHERE category = 'B';
SELECT category, count(*) AS n, avg(price) AS avg_price
  FROM products
  WHERE category IN ('A', 'B')
  GROUP BY category;
```

## ❌ Anti-patterns

### Manual BEGIN/COMMIT
```sql
BEGIN;                       -- ❌ syntax error
INSERT INTO t VALUES (1);
COMMIT;                       -- ❌ syntax error
```
The transaction is automatic. Just omit them.

### Multiple SELECTs hoping to see all results
```sql
SELECT count(*) FROM orders;   -- ⚠️ rows discarded
SELECT count(*) FROM users;     -- ⚠️ rows discarded
SELECT count(*) FROM products;  -- only this one returns
```
If you need multiple distinct query results, make separate calls.

## When NOT to use multi-statement

- You only need one statement → don't add extras.
- You want each result back → split into multiple calls.
- The mutations are unrelated → split (one transaction per logical operation).
