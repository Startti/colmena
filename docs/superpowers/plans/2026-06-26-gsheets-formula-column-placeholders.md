# Implementation Plan: Column-name formula placeholders (`{{Column}}`) for gsheets diff-writes

## Summary
Let an LLM author per-row Google Sheets formulas by **column name** instead of by
A1 letter. The model writes `={{Cantidad}}*{{Tarifa}}` into a diff-write cell; the
gsheets dispatcher — which already knows the real column position and the target
row — substitutes the placeholders into real A1 refs (`=S5*U5`) at write time. An
unknown column name is a **hard, structured error** (not a silent `#VALUE!`).

## Motivation
Live E2E (2026-06-26, gemini-2.5-**pro**, `update_by_position`): asked to set
`Importe = Cantidad*Tarifa` as a formula, the model emitted `=R5*T5` instead of
`=S5*U5` — off-by-one because it derived column letters from pandas `df.columns`
order, which had dropped an empty-header column. Result on the real sheet: `#VALUE!`
in all 4 rows. `update_by_position` correctly placed the cells (column V), but it
**cannot fix A1 refs inside an opaque formula string**. This reintroduces the exact
A1-arithmetic failure that positional write-back was built to eliminate — now living
inside the formula value, silent (`#VALUE!`), even on the strong model. Column-name
placeholders move that arithmetic from the model to the dispatcher and turn a silent
wrong-cell into a loud, self-correctable error.

## Architectural Impact
- **Layers affected**: infrastructure only (`dag_engine/infrastructure/nodes/llm_synthetic_tools/`).
- **New traits/ports**: none. **New adapters**: none.
- **Public API / bindings**: **no change** → ADP worker/api unaffected. New transform
  inside two existing private `do_update_*` functions; the wire format the model sees
  gains an *optional* placeholder syntax (purely additive).
- **No new mode / no new flag**: works inside the existing `update_by_position` and
  `update_in_place` modes.

## Design

### Syntax (decided)
`{{ColumnName}}` — double brace. Single `{}` is real Sheets array-literal syntax
(`={1,2;3,4}`); double `{{ }}` never appears in the formula grammar → zero ambiguity.
Delimited form supports column names with spaces (`{{CLIENT ID}}`). **MVP scope:
current-row only** — `{{Col}}` resolves to that column's cell in the *same* row
(`<col_letter><target_row>`). Column-only form (`{{Col:col}}` for `=SUM(...)`
aggregates) is a documented future extension, NOT in this MVP.

### Resolution rules
A cell's new value is resolved iff it is a **String that starts with `=`** (a
formula). Then every `{{...}}` token is substituted:
- inner text is trimmed and matched **exactly** against the resolvable column map;
- match → `<col_letter><target_row>` (e.g. `{{Cantidad}}` at row 5 → `S5`);
- no match → **abort the whole tab write** with a structured `FormulaUnknownColumn`
  error listing the valid column names (no partial writes — both `do_update_*`
  already collect all `cell_updates` before the single `batch_update_cells`, so we
  fail before any write).
- Non-formula strings, numbers, bools, null → returned unchanged (resolver is a
  no-op). A literal `{{` only ever has meaning inside a leading-`=` formula.

The "resolvable column map" is the **addressable** one (unique, non-empty header
names) — same map already used to place the target cell. Referencing an empty- or
duplicate-named column via a placeholder therefore errors (can't disambiguate),
which is correct.

### How letter & row are derived (verified against code, 2026-06-26)
- **Letter** = `col_letter(addressable_columns(header_cols)[name])`, where `header_cols`
  is a **raw** row-1 read (`as_records:false`). The raw read **preserves interior
  empty-header columns in position**, so `name → true index → true letter`. This is
  precisely why the dispatcher is correct where the model is not: the model derived
  letters from pandas `df.columns`, which **drops** the empty column and shifts every
  later letter left by one (`Cantidad` 18→17 → S→R). Confirmed by the live run:
  `inspected_sheets.columns` (records-derived) had **no** empty column, while
  `preview_markdown` (raw) showed the empty `|  |` at index 2. Real positions
  (idx 18→S, 20→U, 21→V) match the live sheet exactly.
- **Row** = the **same row as the target cell** being written: `idx + 2` in
  `update_by_position` (header is row 1, snapshot index 0 → sheet row 2), or the key's
  resolved row in `update_in_place`. A per-row formula references its own row, so one
  `target_row` serves the target cell and all its `{{...}}` placeholders.

### Diff interaction (already safe)
Snapshots load with `UnformattedValue` → a formula cell reads as its computed
number. The model's `={{...}}` string differs from that number → diff marks it
changed → it gets written. Re-running re-writes the same formula (not strictly
idempotent, harmless). Documented as a note.

## Detailed Steps

### 1. New helper + error type
File: `.../llm_synthetic_tools/gsheets_run_python.rs` (or a small new
`formula_template.rs` in the same dir, re-exported — decide at implementation; logic
identical).
```rust
/// Resolve `{{ColumnName}}` placeholders inside a formula string into real A1
/// refs for `target_row`. Only strings starting with `=` are processed; every
/// other value is returned unchanged. Errors if a placeholder names a column
/// that is not in `resolvable` (unique, non-empty header names).
fn resolve_formula_placeholders(
    value: &serde_json::Value,
    resolvable: &std::collections::HashMap<String, usize>,
    target_row: usize,
) -> Result<serde_json::Value, FormulaResolveError>;

struct FormulaResolveError { unknown: String, valid: Vec<String> }
impl FormulaResolveError { fn to_json(&self, tab: &str) -> serde_json::Value }
```
Implementation: hand-written `{{` … `}}` scan (no regex dep; matches repo style).
Factor a tiny `col_letter(col_index) -> String` out of `a1_addr` so
`a1_addr(c,row) == format!("{}{}", col_letter(c), row)` and the resolver reuses
`col_letter`.

### 2. Wire into `do_update_by_position` (~line 1111-1123)
Resolve `chg.new_value` with `target_row = idx + 2` against `col_to_index` (already
`addressable_columns(...)` here) before `CellValue::from_json`; on `Err`,
`return err.to_json(raw_name)` (abort before the batch write).

### 3. Wire into `do_update_in_place` (~line 855-868)
`col_to_index` there includes ALL header columns (line 849). For resolution, build an
**addressable** map (`addressable_columns(&header_cols)`) so placeholders can't target
ambiguous columns. `target_row = *row`. Same abort-on-Err.

### 3b. REQUIRED FIX (found during review): widen `do_update_in_place` header read
Line 794 reads the header as `Some("A1:Z1")` — **capped at column Z (26 cols)**. Change
to `Some("1:1")` (full row), matching `do_update_by_position`. This is needed for two
reasons:
- **Feature blocker:** a `{{Column}}` whose real position is past Z (the live test
  sheet has 29 cols) would falsely resolve to `FormulaUnknownColumn`.
- **Latent pre-existing bug it also fixes:** today, a *normal* (non-formula)
  update_in_place edit to a column past Z is **silently dropped** — line 857
  `col_to_index.get(&chg.column)` returns `None` → `continue`, so the cell is never
  written and no error is surfaced. Widening to `1:1` fixes that silent data loss.

No downside: `1:1` returns the entire row 1, `as_records:false`, identical shape to the
capped read for sheets ≤26 cols.

### 4. Tests (unit, inline `#[cfg(test)] mod`)
- **"how we know the letter" guard:** `addressable_columns(["a","","b"])` → `b` maps to
  index **2** (letter C), NOT 1 — i.e. an interior empty-header column keeps the true
  positions. This is the exact property that makes the dispatcher's letter correct
  where the model's `df.columns` (empty dropped) is wrong.
- `={{Cantidad}}*{{Tarifa}}`, Cantidad→18, Tarifa→20, row 5 → `=S5*U5`.
- `{{CLIENT ID}}` (space in name) resolves.
- unknown `{{Cantdad}}` → Err; `.valid` contains real names.
- non-formula string `"hola {{x}}"` unchanged; number `5` unchanged; null unchanged.
- array literal `={1,2;3,4}` (single brace) untouched.
- `>26` columns: index 27 → `AB...` correct (guards the `col_letter` refactor).
- multiple tokens + surrounding text: `=IF({{A}}>0,{{B}},0)` row 3, A→2,B→3 → `=IF(C3>0,D3,0)`.
- PyO3 sandbox can't run under `cargo test` → rely on release-binary E2E for the full path.

### 5. Docs / LLM-facing surface
- `text/tools/gsheets.yaml` — under the `update_by_position` example: *"Para fórmulas,
  referenciá columnas por nombre con `{{Nombre}}` (misma fila):
  `df.loc[mask,'Importe'] = '={{Cantidad}}*{{Tarifa}}'`. Nunca calcules letras de
  columna a mano."*
- `skills/gsheets-editing/references/edit-rows.md` — new "Fórmulas" subsection:
  `{{col}}` pattern, the "no calcules A1" rule, the unknown-column error contract.
- Python postlude unchanged (already emits `df_index`).

## Testing Strategy
- Unit: resolver tests above (`cargo test --lib`).
- Full: `cargo test --verbose` (CI parity) + `cargo clippy -- -D warnings` + `cargo fmt`.
- Live E2E (release binary, OAuth from Secret Manager, in-memory only): re-run the
  formula experiment on `Hoja 16` / client `TCIb1afd2…` rows 5-8. Expect
  `={{Cantidad}}*{{Tarifa}}` → sheet shows `=S5*U5` (FORMULA render) and computed
  Importe equals the known literal (1614600, …). Revert V5:V8 via `reset_cells`.
  Negative run: a typo placeholder must return `FormulaUnknownColumn`.

## Documentation Updates
- `text/tools/gsheets.yaml`, `skills/gsheets-editing/references/edit-rows.md`
- CHANGELOG (gsheets section)
- Memory: extend `project_gsheets_write_reliability_pr126.md`.

## Risks & Mitigations
| Risk | Impact | Mitigation |
|------|--------|------------|
| `{{` wanted literally inside a formula | Wrong substitution | `{{ }}` reserved inside leading-`=` formulas only; real Sheets formulas never contain `{{`. Documented. |
| Model references a slightly-wrong column name | Write aborts | *Desired*: loud `FormulaUnknownColumn` (with valid names) beats silent `#VALUE!`. |
| Partial write if one cell errors mid-batch | Inconsistent sheet | Resolve ALL cells first; abort before `batch_update_cells` (both paths already collect-then-write). |
| `col_letter` refactor breaks `a1_addr` for >26 cols | Wrong A1 everywhere | Keep `a1_addr` semantics; explicit AA/AB unit test. |
| Re-run rewrites same formula (not idempotent) | Redundant write, no harm | Documented note. |

## Open Questions
None blocking. (Column-only `{{Col:col}}` aggregate form intentionally deferred.)

## Execution
Implement with `/rust_dev`. Additive, no public API change → no ADP sweep required.
Stacks on `feat/gsheets-update-by-position` (or develop once #126/#127 merge).
