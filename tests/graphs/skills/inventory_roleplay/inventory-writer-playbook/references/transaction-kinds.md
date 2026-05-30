# transaction.kind values

| kind | qty_delta sign | Meaning |
|---|---|---|
| purchase | positive | Stock received from supplier |
| sale | negative | Stock sold to customer |
| return | positive | Customer returned an item |
| adjustment | either | Manual correction (inventory count discrepancy) |
| transfer | either | Movement between warehouses (not used yet) |

Reject any kind not in this list.
