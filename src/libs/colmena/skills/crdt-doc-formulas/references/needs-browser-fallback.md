# Pattern: Handling needs_browser warnings

When `crdt_doc_set_cell` returns:

```jsonc
{
  "ok": true,
  "cells_recalculated": 0,
  "warnings": [{
    "kind": "needs_browser",
    "addr": "E2",
    "functions": ["XLOOKUP"]
  }]
}
```

The cell IS written, but the backend can't evaluate it. `fs:"needs_browser"` and `v` is the formula text as placeholder. Three responses:

## 1. Accept — the user will see it computed when they open Univer

If the workflow continues only when a human reviews it anyway, do nothing. Move on. Univer evaluates the function correctly client-side.

## 2. Rewrite — find a supported equivalent

For lookups, `INDEX/MATCH` is widely supported:

```text
Before: =XLOOKUP(key, A:A, B:B, "")
After:  =IFERROR(INDEX(B:B, MATCH(key, A:A, 0)), "")
```

Then `set_cell` again with the rewrite. Surface to user: "I used INDEX/MATCH instead of XLOOKUP because the backend evaluator doesn't support XLOOKUP — visually identical."

## 3. Ask the user

If the function can't be replaced (custom add-in, sparkline, dynamic array), tell the user the cell needs them to open Univer once to materialise the value, then continue.
