# Direct writes (no code)

When you already know the target address(es), write directly — no run_python.

## One cell → `gsheets_set_cell`

Args: `spreadsheet_id`, `sheet`, `addr` (A1, e.g. `"B7"`), `value`.

- A string starting with `=` is stored as a formula (USER_ENTERED); Google
  recalculates dependents server-side. `value: "=SUM(A1:A10)"`.
- Numbers/strings are written as-is. `value: 555`.

## A contiguous block → `gsheets_set_range`

Args: `spreadsheet_id`, `sheet`, `start_addr` (top-left A1), `values` (a 2-D
array, row-major). Writes the rectangle starting at `start_addr`, overwriting
whatever is there. Use it to append rows (start at the first free row) or to lay
down a small table.

```text
start_addr = "A2"
values = [["Lapices", 10], ["Cuadernos", 5]]   # writes A2:B3
```

## Working out the A1 address

- **Rows are 1-based and the header is row 1.** The first data row is row 2. A
  pandas DataFrame row at index `i` (header excluded) is sheet row `i + 2`.
- **Column letter = position in the header.** Read the tab once; the Nth data
  column (0-based) maps to its letter: col 0 → `A`, 1 → `B`, … 18 → `S`, 26 →
  `AA`. Derive it from the real header — never guess the letter.

For editing many rows matched by a condition (rather than known addresses), see
the `edit-rows` reference — find the row numbers in code first, then call
`gsheets_set_cell` per cell.
