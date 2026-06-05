# Pattern: Writing formulas to cells

## Basic — single formula

```jsonc
// tool call
{
  "name": "crdt_doc_set_cell",
  "arguments": {
    "sheet": "Sheet1",
    "addr": "C2",
    "value": "=B2*0.21"
  }
}

// expected result
{
  "ok": true,
  "cells_recalculated": 0,
  "warnings": []
}
```

`cells_recalculated` is non-zero if other cells already had formulas referencing C2.

## Multi-cell batch — derived column

```jsonc
{
  "name": "crdt_doc_set_range",
  "arguments": {
    "sheet": "Sheet1",
    "start_addr": "C2",
    "values": [["=B2*0.21"], ["=B3*0.21"], ["=B4*0.21"]]
  }
}
```

The batch evaluates each cell, then runs one recalc pass over the union of dependents. Output includes `total_cells_recalculated`.

## Evaluation errors

If a formula evaluates to `#DIV/0!` etc., the cell IS written (Excel-compatible: a cell with an error value is a valid state). The tool result includes:

```jsonc
{
  "ok": true,
  "cells_recalculated": 0,
  "warnings": [{"kind": "eval_error", "addr": "D2", "error": "#DIV/0!"}]
}
```

You can choose to: ignore (the user will see the error chip), rewrite the formula, or `set_cell` over it with a sentinel.

## Cycle detection

If your write creates a circular dependency (e.g. A1 references B1 and B1 references A1), the tool result includes:

```jsonc
{
  "ok": true,
  "warnings": [{"kind": "cycle", "chain": [["Sheet1","A1"], ["Sheet1","B1"]]}]
}
```

The cell is still written, but no value cascades. You should clear or rewrite one cell in the chain.

## Parse errors

Malformed formulas (e.g. `=SUM(`) generate a `parse_error` warning. The raw text is persisted as a string so the user sees their input — but no evaluation happens.

```jsonc
{
  "ok": true,
  "warnings": [{"kind": "parse_error", "addr": "E1", "error": "..."}]
}
```
