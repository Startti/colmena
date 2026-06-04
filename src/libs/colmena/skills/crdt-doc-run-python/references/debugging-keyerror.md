# Debugging KeyError and shape errors — do NOT loop

When `crdt_doc_run_python` returns an error like `KeyError: 'Precio'`, the response includes `loaded_sheet_columns` showing the ACTUAL columns of every sheet you loaded:

```json
{
  "error": "Python execution error: KeyError: 'Precio'",
  "loaded_sheet_columns": {
    "sh_01ABC…": ["Reporte Q3 2026", "col_B", "col_C", "col_D"]
  }
}
```

**FIRST action after any KeyError: read `loaded_sheet_columns` from the error response.** It will tell you exactly what columns exist. Do NOT retry the same `iloc[N]` or column-name guess.

## Diagnostic checklist

1. **`loaded_sheet_columns` shows what you expected?** Then the issue is a typo, case mismatch, or leading/trailing whitespace. Try `df.columns.str.strip()`.

2. **`loaded_sheet_columns` shows ONE string + `col_B/col_C/...` placeholders?** You're in Case B (title row in A1). Apply header promotion — see the `dataframe-shape` reference.

3. **Still stuck?** Run ONE diagnostic call: load the sheet, print metadata, return nothing:

```python
df = dfs[sid]
print('columns:', list(df.columns))
print('dtypes:', df.dtypes.to_dict())
print('first 3 rows:')
print(df.head(3))
output = 'diagnostic'   # required, even if empty-ish
```

`stdout` comes back to you in the response (capped at 10 KB). This single call resolves 99% of shape confusion.

## Common gotchas mapped to fixes

| Error | Likely cause | Fix |
|---|---|---|
| `KeyError: '<real column name>'` | Title row in A1, columns are `['Title', col_B, ...]` | Promote headers (see `dataframe-shape`) |
| `KeyError: '<misspelled>'` | Typo / case / whitespace | Print `df.columns` to confirm exact value |
| `KeyError: 0` | Numeric index lookup on string-indexed Series | Use `.iloc[0]` instead of `[0]` |
| `TypeError: '<' not supported between instances of 'str' and 'float'` | Numeric column has stray strings | `pd.to_numeric(col, errors='coerce')` |
| `TypeError: Cannot setitem on a Categorical with a new category` | Writing new value to Categorical column from `merge(indicator=True)` | `.astype('object')` before writing — see `type-quirks` |

## What NOT to do

- ❌ Retry the same code 3 times hoping it works.
- ❌ Try a different `iloc[N]` value without inspecting columns.
- ❌ Add try/except around the failing line as the "fix" — that hides the bug, doesn't resolve it.
