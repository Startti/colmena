# Implementation Plan: `gsheets_run_python` positional write-back (`update_by_position`)

## Summary
Add an `update_by_position` write mode to `gsheets_run_python` so an agent can edit
existing sheet rows by modifying the bound DataFrame **in place** and writing the
changes back **without computing any A1 address or needing a unique key**. The
dispatcher — which owns the row/column→sheet mapping — diffs the returned df
against the loaded snapshot by pandas index and writes only the changed cells.

## Motivation
Live E2E (2026-06-26) showed both gemini-flash and gemini-2.5-pro reliably make
**off-by-one coordinate errors** when they derive A1 addresses by hand:
- flash: row offset (`df index − 1` instead of `− 2`) → edited the wrong rows
  (masked by a CLIENT ID filter → 4/6 rows).
- pro: column letter (`Importe` → `U` instead of `V`) → wrote to the wrong column
  and corrupted `Tarifa`.

The tool mechanism is sound and values are computed correctly; the failure is the
manual A1 arithmetic. `update_in_place` already exists but **requires a unique key
column**, which the data (repeated CLIENT ID) doesn't have. Positional write-back
removes the arithmetic AND the unique-key requirement — the model writes natural
pandas (`df.loc[mask,'Importe'] = df['Cantidad']*df['Tarifa']`) and the framework
does the rest.

## Architectural Impact
- **Layers affected**: infrastructure only.
- **New traits/ports**: none — `SheetsClient::batch_update_cells` + `a1_addr()`
  already exist.
- **New adapters**: none.
- **Modified files**:
  - `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gsheets_run_python.rs`
  - `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/diff_writer.rs`
  - `.../gsheets_run_python_postlude.md` (the Python postlude)
  - `src/libs/colmena/text/tools/gsheets.yaml` (run_python description)
  - `src/libs/colmena/skills/gsheets-editing/SKILL.md` + `references/edit-rows.md`
  - `docs/BACKLOG.md` / CHANGELOG
- **Binding impact**: Python (PyO3) **no**, TypeScript (napi) **no** — internal to
  the synthetic-tool dispatcher; the only LLM-facing change is a new `mode` string.

## Design (the chosen approach — revised after code review)
- Mode: **explicit** `mode: 'update_by_position'` (no `key`).
- **Contract: the model returns the FULL bound df** (same row count `N` as loaded),
  modified **in place** — NOT a filtered subset. This is the load-bearing safety
  decision (see "Why the full df" below).
- Mapping is by the df **index label** (captured as `df.index`). The dispatcher
  **requires the index to be exactly the set `{0 .. N-1}`** (a full permutation);
  `sheet_row = index_label + 2` (header is sheet row 1).
- **Reuse the existing `diff_records`**: inject a synthetic `__index__` column =
  `df.index` into both the snapshot and the returned records and key on it — no
  new diff algorithm. Its duplicate-key validation catches `concat`/duplicate
  indices for free. (The `{0..N-1}` permutation check runs first, separately.)
- Diff source: the **snapshot the code loaded** (no concurrent re-read). Verified
  correct: a re-read would *revert* concurrent human edits on cells the model
  didn't touch (model's df holds the old value → diff vs current → rewrite old).
  Diffing against the load snapshot writes **only the cells the model changed**.
- Scope v1: **whole-sheet bindings** (`range` omitted). A binding with an explicit
  `range` is rejected for this mode in v1 (as_records would treat the range's first
  row as the header).

### Why the full df (the key review finding)
Supporting a **filtered subset** via the pandas index is **unsafe**: if the model
does `df.loc[mask]` and **also** `reset_index()`, the labels collapse to `0..K-1`
and map to the WRONG rows — and this is **indistinguishable** from a legitimate
"first K rows" subset, so it can't be validated. Requiring the full df makes the
footgun **detectable** (row count ≠ N → reject) and makes `reset_index()` on the
full df **harmless** (labels are already `0..N-1`). `sort()` is still fine because
the mapping is by label, not position. Only `sort()` + `reset_index(drop=True)`
*together* remain an (uncommon, documented) anti-pattern.

## Detailed Steps

1. **Retain the load snapshot + mapping** — `gsheets_run_python.rs` (binding load, ~L289-341)
   - Today the loaded records are injected into the sandbox and dropped. Before
     moving `inputs` into the sandbox call, retain per binding:
     `LoadedBinding { sheet: String, records: Vec<Map>, header_cols: Vec<String>, row_offset: usize, had_range: bool }`.
   - `header_cols` = the sheet's real column order (from the read; `read_range`
     already returns it). `row_offset = 2` for whole-sheet; set `had_range` when a
     `range` was passed.
   - Key the retained map by **sheet name** (output_sheets entries are keyed by
     sheet). If two bindings load the same sheet → record the first and flag
     ambiguity (reject positional writes for that sheet with a clear message).

2. **Postlude emits the index** — `gsheets_run_python_postlude.md`
   - For a spec entry with `mode == 'update_by_position'`, emit
     `{'mode','df_records','df_cols','df_index': list(df.index)}` (no `key`).
   - Keep existing shapes untouched (bare df, replace/update_in_place/overwrite).

3. **No new diff algorithm — reuse `diff_records`** — `diff_writer.rs`
   - `diff_records` already does cell-level diffing keyed on a column, with
     duplicate-key validation and `values_equal`. We reuse it with a synthetic
     `__index__` key (see step 4); no positional diff function needed.
   - Only add tests for the new usage (synthetic key over the index column).

4. **Mode routing + write** — `gsheets_run_python.rs`
   - Add arm `"update_by_position" => do_update_by_position(client, ss, raw_name, entry, &loaded).await`.
   - `do_update_by_position`:
     - Look up the `LoadedBinding` for `raw_name`; error if absent
       (*"update_by_position needs the tab bound this run"*) or `had_range` (v1).
     - Read `df_index` from the entry. **Validate it is exactly the set `{0..N-1}`**
       where `N = loaded.records.len()` (a full permutation): row count must equal
       `N`, every label an int in `[0,N)`, no duplicates. Else hard error with the
       hint *"return the FULL df modified in place — don't filter/`reset_index`/`concat`."*
     - Build `col_to_index` from `loaded.header_cols` (positional). Map each df
       column **by name**; **skip** columns whose name is empty or appears more
       than once in the header (ambiguous) → report in `skipped_columns`.
       `comparable_cols` = df cols that resolve to a unique header position.
     - Inject `__index__` (the label) into each snapshot record (its position
       `0..N-1`) and each returned record (`df_index[j]`), then call
       `diff_records(&snapshot, &new, "__index__", Some(&comparable_cols), false, raw_name)`.
     - For each `CellChange` (carrying `key_value` = the index label and `column`):
       `sheet_row = key_value.parse::<usize>() + 2`; `col_idx = col_to_index[column]`;
       `addr = a1_addr(col_idx, sheet_row)`; `CellValue::from_json(new_value)`.
       Collect `Vec<(addr, CellValue)>`.
     - `client.batch_update_cells(ss, raw_name, updates)`.
     - Return `{tab, mode:"update_by_position", cells_written, rows_changed, skipped_columns}`.

5. **LLM-facing docs** — `gsheets.yaml` (gsheets_run_python description)
   - Document `update_by_position` as the way to edit existing rows: *bind the
     whole sheet, modify the df in place (`df.loc[mask,'col']=...`), return
     `output_sheets={'Tab':{'mode':'update_by_position','df':df}}`. Do NOT
     reset_index / sort / concat the bound df; do NOT use it to add rows.*

6. **Skill** — `gsheets-editing`
   - Collapse the "edit rows by UNIQUE vs NON-UNIQUE key" rows of the decision
     table into one: *edit existing rows → run_python, modify df in place,
     `update_by_position` (no key, no A1 math)*. Keep `update_in_place` (by key)
     as the alternative when a real unique key exists. Update `references/edit-rows.md`
     with the in-place example (the Importe = Cantidad×Tarifa case).

## Testing Strategy
- **Unit** (`gsheets_run_python.rs`, pure helpers):
  - index-permutation validation: accepts `{0..N-1}` (any order); rejects subset
    (count ≠ N), out-of-range label, duplicate label.
  - column mapping: resolves a unique named column → A1 letter; **skips** empty /
    duplicate-named header columns; `sheet_row = index_label + 2`.
  - synthetic-`__index__` diff over `diff_records`: only changed cells emitted;
    int↔float coercion is NOT flagged (via `values_equal`).
- **E2E (`#[ignore]`)**: reproduce experiment 2 — `Importe = Cantidad × Tarifa`
  over the 6 non-unique-key rows via `update_by_position` from a GENERIC system
  message; verify column **V** (not U) updated and **Tarifa (U) untouched**, and
  no other rows touched. (Harness + `dump_range`/`reset_cells`/`revert_experiment`
  already exist locally.)

## Documentation Updates
- `src/libs/colmena/text/tools/gsheets.yaml` — `update_by_position` mode.
- `src/libs/colmena/skills/gsheets-editing/SKILL.md` + `references/edit-rows.md`.
- `docs/BACKLOG.md` (mark) + CHANGELOG entry.

## Risks & Mitigations
| Risk | Impact | Mitigation |
|------|--------|------------|
| Model returns a **filtered subset** (`df.loc[mask]`) | index→row map ambiguous, wrong cells | **require the full df**: index set must equal `{0..N-1}`; row count ≠ N → hard error with hint |
| `sort()` + `reset_index(drop=True)` together | wrong cells written | documented anti-pattern (uncommon); `sort` alone is safe (mapped by label) |
| Model adds rows / `concat` (duplicate or >N indices) | bad mapping | caught by the `{0..N-1}` permutation check + `diff_records` duplicate-key validation |
| Model adds a column not in the sheet header | column lost | skip + report `skipped_columns` |
| **Empty / duplicate header names** (e.g. two `""` cols) | name→position ambiguous | skip those columns (can't address by name); report in `skipped_columns`. Pre-existing records-load limitation, shared with `update_in_place` |
| `range`-subset binding (as_records header semantics differ) | wrong mapping | reject in v1 with a clear message (whole-sheet only) |
| pandas type coercion (int↔float, NaN) | spurious diffs / rewrites | `values_equal` already compares numbers as f64 (verified) → no spurious int/float diff; NaN handling inherited from existing postlude |
| Snapshot staleness vs concurrent human edit | could overwrite a touched cell | only the cells the model *changed* are written (diff vs load snapshot); re-read was rejected because it would *revert* untouched concurrent edits |

## Open Questions
- **Non-blocking:** keep explicit `update_by_position`, or also auto-route
  `update_in_place` with no `key` to positional? (Decided: explicit, for clarity.)
- **Non-blocking:** v2 — support `range`-subset bindings; support adding rows
  (append) in the same call; opt-in concurrent re-read for last-writer-wins.

## Execution
Use `/rust_dev` for implementation (dispatcher + diff_writer + postlude), then the
gsheets E2E runbook for live verification.
