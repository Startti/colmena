# 01 — Inspect schema first

**The rule:** before writing any analysis code, look at the table.
Always.

## Minimal recipe

```python
import pandas as pd
df = pd.DataFrame(dfs["products"])   # binding name from crdt_doc_run_python sheet_ids
print(df.head(3))                    # see the first 3 rows
print(df.dtypes)                     # see what pandas inferred per column
print(f"shape: {df.shape}")          # rows, cols
output = {
    "head_3":   df.head(3).to_dict('records'),
    "dtypes":   {c: str(t) for c, t in df.dtypes.items()},
    "n_rows":   len(df),
}
```

Run this FIRST, look at the response, THEN write the real analysis in a
second `crdt_doc_run_python` call. Two cheap calls beat one expensive
guess.

## Even cheaper: peek without pandas

If the document is large and you only need column names + a sample,
use `crdt_doc_read` with a tiny range:

```json
{
  "artifact_id": "...",
  "sheet": "products",
  "range": "A1:Z3",
  "include_formulas": false
}
```

This returns 3 rows of records (header + 2 examples) for the price of
one call. The LLM sees ~30 lines instead of 5000.

## Why this matters

CRDT sheets share the same Excel-style quirks that bite analyses:

- A column of integers may import as text if any cell starts with a
  leading apostrophe (`'44`).
- A "date" column may be a serial number, a string, or a mix.
- Names with non-ASCII characters (`Categoría`) may or may not survive
  round-tripping intact.

Looking at `head(3)` + `dtypes` catches all three before they corrupt
the analysis.
