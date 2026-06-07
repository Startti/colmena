# Pattern: Reading cells with formula text

## Default — scalar values (pandas-friendly)

```jsonc
{
  "name": "crdt_doc_read",
  "arguments": { "sheet": "Sheet1" }
}
// → {"sheet_id": "Sheet1", "cells": {"A1": 5, "B1": 10}}
```

Use this shape any time you want to feed cells to `run_python`. pandas sees scalars and reads naturally.

## Formula-aware — include_formulas=true

```jsonc
{
  "name": "crdt_doc_read",
  "arguments": { "sheet": "Sheet1", "include_formulas": true }
}
// → {
//     "sheet_id": "Sheet1",
//     "cells": {
//       "A1": {"v": 5},
//       "B1": {"v": 10, "f": "=A1*2", "fs": "be"}
//     }
//   }
```

Cells without a formula stay as `{v}` only. Use this shape when:

- You want to know whether a value was computed or typed.
- You're about to rewrite a formula (read the existing one first).
- The user asked "why is this cell showing X" — the formula is the answer.

## Workflow tip

Before calling `include_formulas=true`, call `crdt_doc_list_sheets`: if `formula_count: 0`, skip the formula-aware read.

## `fs` meanings

- `"be"` — evaluated by the backend formula engine. `v` is trustable.
- `"fe"` — evaluated by the Univer frontend. `v` is trustable.
- `"needs_browser"` — function out of the backend's set. `v` holds the formula TEXT as a placeholder. See `needs-browser-fallback`.
