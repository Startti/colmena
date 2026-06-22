# Number formats — exact pattern strings

Set via `format.number_format: { type, pattern }` in a `gsheets_format_range` op.

| Data | type | pattern | Renders |
|---|---|---|---|
| Money (whole) | `CURRENCY` | `$#,##0` | `$1,234` |
| Money (cents) | `CURRENCY` | `$#,##0.00` | `$1,234.56` |
| Percent | `PERCENT` | `0.0%` | `12.3%` (value `0.123`) |
| Thousands | `NUMBER` | `#,##0` | `1,234` |
| Date | `DATE` | `yyyy-mm-dd` | `2026-06-22` |
| Plain integer | `NUMBER` | `0` | `1234` |

Apply to the NUMERIC range only (e.g. the amounts block `B2:F8`), not to label
columns. Percent expects the underlying value as a fraction (0.123 → 12.3%).
