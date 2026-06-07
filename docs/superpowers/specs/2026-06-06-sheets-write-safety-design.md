# Sheets Write Safety — Collision Policy + In-Place Updates

**Date:** 2026-06-06
**Status:** Approved (brainstorming)
**Scope:** colmena `gsheets_run_python` and `crdt_doc_run_python` dispatchers
**Related:** [2026-06-06-pandas-multisheet-and-exploration-design.md](2026-06-06-pandas-multisheet-and-exploration-design.md) (multi-sheet write-back), [2026-06-05-google-sheets-design.md](2026-06-05-google-sheets-design.md) (Subsystem E)

---

## Goal

Stop the dispatcher from silently overwriting existing sheet tabs, and give the LLM a token-cheap way to PATCH specific rows in a large existing sheet without re-uploading every row.

Two related changes shipped together as a single PR:

- **P1 (collision policy):** when a target tab already exists, the dispatcher fails by default and returns actionable metadata. Operator can opt back into the old `auto_suffix` behavior. The LLM must explicitly choose `update_in_place`, `overwrite`, or rename to proceed.
- **P2 (`update_in_place` mode):** `output_sheets` accepts a spec dict (in addition to bare DataFrames) that lets the dispatcher diff-write — only changed cells go through Sheets API; the LLM never sees row contents.

---

## Why now

The current implementation has two real problems observed in production demos:

**1. Silent overwrites.** Three code paths write to tabs differently:

| Path | Behavior when tab exists today |
|---|---|
| `output_sheets = {name: df}` (gsheets, multi-tab) | Auto-suffix `" (2)"` silently |
| `output_sheet` + `write_to_sheet` (crdt_doc legacy single-tab) | **Overwrite, destructive** |
| `gsheets_set_range` (low-level tool) | **Overwrite, destructive** |

Users have reported sheets being clobbered without warning, and even the auto-suffix path is confusing — they think their original "Sales" tab was updated when in fact `Sales (2)` was created next to it.

**2. No token-cheap partial update.** Patching 47 rows in a 1000-row sheet requires either:
- (a) Reading 1000 rows + writing 1000 rows back (12K cell writes, schema-destructive)
- (b) 47 separate `gsheets_set_cell` calls (47 LLM round-trips)

Both are wasteful. The dispatcher already has all the context — it should do the diff itself.

---

## Design

### §1 Collision policy (P1)

#### Node-level config

```json
"gsheets_run_python": {
  "node_type": "gsheets_run_python",
  "fixed_config": {
    "on_existing_sheet": "fail"
  }
}
```

Same field on `crdt_doc_run_python`. Values:

| Value | Behavior on collision |
|---|---|
| `fail` ⭐ default | Dispatcher cuts before writing. Returns structured error to LLM (see below). |
| `auto_suffix` | Current behavior: writes as `"Sales (2)"`. Provided for operators with explicit need. |
| `overwrite` | Replaces the tab. Operator accepts the risk; the LLM doesn't have to ask. |

`fail` applies to all three write paths in both dispatchers.

#### Error shape returned to the LLM

When `on_existing_sheet = "fail"` triggers, the dispatcher returns:

```json
{
  "error": "SheetExists",
  "tab": "Sales",
  "spreadsheet_id": "1xyz...",
  "current_state": {
    "n_rows": 4998,
    "n_cols": 12,
    "columns": ["product_id", "name", "category", "price", "cost", "..."],
    "last_modified": "2026-06-04T10:23:00Z"
  },
  "advice": "The tab 'Sales' already exists with data. Recommended: use a different name (e.g., 'Sales_analysis_2026_06_06'). If you must touch the existing tab, choose update_in_place (patch specific rows) or overwrite (replace everything — destructive).",
  "valid_next_moves": [
    {
      "action": "rename",
      "example_code": "output_sheets = {'Sales_q1_review': df}"
    },
    {
      "action": "update_in_place",
      "example_code": "output_sheets = {'Sales': {'mode':'update_in_place','df':df,'key':'product_id'}}"
    },
    {
      "action": "overwrite",
      "example_code": "output_sheets = {'Sales': {'mode':'overwrite','df':df}}"
    }
  ]
}
```

Costs ~150 tokens, but prevents accidental destruction of large sheets. The `advice` string nudges toward rename — the safest option.

The `last_modified` field requires fetching `spreadsheets.get` with `includeGridData=false` — already a cheap call. We surface it because it helps the LLM (and the human reading its response) decide if the existing data is stale or current.

#### Pre-flight check

For BOTH gsheets and crdt_doc, before any write happens:
1. List target tabs that will receive writes.
2. For each, check existence.
3. If any existing tab is going to be touched AND `on_existing_sheet != "auto_suffix"`, evaluate per-spec:
   - Bare DataFrame entries → trigger collision behavior (`fail`/`overwrite`/`auto_suffix`).
   - Spec dict with `mode: "update_in_place"` → ALLOWED (this is the point).
   - Spec dict with `mode: "overwrite"` → ALLOWED (explicit request).
   - Spec dict with `mode: "replace"` → trigger collision behavior (same as bare DataFrame).
4. If trigger fires under `fail`, abort the whole write batch and return the error above. No partial writes.

### §2 `output_sheets` accepted shapes (P2)

The script assigns `output_sheets` as a dict. Each value can be:

```python
# Shape 1 — bare DataFrame (current behavior, mode defaults to "replace")
output_sheets = {"Resumen": df_resumen}
# Equivalent to: {"Resumen": {"mode": "replace", "df": df_resumen}}

# Shape 2 — spec dict, mode "update_in_place"
output_sheets = {
    "Sales": {
        "mode": "update_in_place",
        "df": df_modified,           # required
        "key": "product_id",         # required — column identifying rows
        "columns": ["price", "stock"]  # optional — only patch these columns; default = all common columns
    }
}

# Shape 3 — spec dict, mode "overwrite" (explicit, replaces "Sales" entirely)
output_sheets = {"Sales": {"mode": "overwrite", "df": df}}

# Shape 4 — spec dict, mode "replace" (creates new tab; collides if exists)
output_sheets = {"NewTab": {"mode": "replace", "df": df}}
```

Modes summary:

| Mode | If tab exists | If tab missing | What gets written |
|---|---|---|---|
| `replace` (default) | Triggers collision policy | Creates tab | Full DataFrame |
| `update_in_place` | Required (otherwise error: nothing to patch) | Error: `UpdateRequiresExistingTab` | Cell-level diff only |
| `overwrite` | Allowed (explicit) | Creates tab | Full DataFrame |

#### `update_in_place` dispatcher algorithm

1. Re-fetch the current tab from the source of truth (Sheets API for gsheets; CRDT in-memory for crdt_doc). **Row contents never enter the LLM context** — they flow only between dispatcher and storage.
2. Run validations (§3). On any failure: return error, no writes.
3. Build two indexed DataFrames using `key`:
   ```python
   cur = pd.DataFrame(current_rows).set_index(key)
   new = args.df.set_index(key)
   ```
4. Restrict columns: if `columns` arg is provided, use it; else use `set(new.columns) & set(cur.columns) - {key}`.
5. For each row in `new` that exists in `cur`, for each column in the restricted set: if `cur.loc[k, col] != new.loc[k, col]`, record a cell change at `(row_a1_of_k, col_a1_of_col)`. Use NaN-safe comparison.
6. If no changes: return `{"changes": {"rows": 0, "cells": 0}}` without an API call.
7. Otherwise: issue a single `spreadsheets.values.batchUpdate` (gsheets) or atomic CRDT op set (crdt_doc) with all changes.
8. Return:
   ```json
   {
     "tab": "Sales",
     "mode": "update_in_place",
     "changes": {"rows": 47, "cells": 47, "columns": ["price"]},
     "unchanged": {"rows": 953},
     "skipped": {"rows_not_in_target": 2, "reason": "rows in your DataFrame with key values not present in 'Sales' were ignored"}
   }
   ```

#### Rows in `new` not present in `cur`

Default: **silently ignored**, but reported in the `skipped` field so the LLM can verify. This is the common case where the LLM filters in pandas and only the matching subset gets written back — the unmatched rows are not bugs.

Opt-in stricter behavior via `strict_match: true` in the spec dict — if any `new` row has a key value not present in `cur`, the write fails entirely. Useful for "I'm only modifying existing rows, anything else is a mistake" workflows.

### §3 Validations (pre-write, reject before API call)

| Check | Trigger | Error code | Message template |
|---|---|---|---|
| Key column missing in current | `key not in current.columns` | `KeyColumnMissing` | `"Key column '{key}' not found in tab '{tab}'. Available columns: {cols}."` |
| Key column missing in new df | `key not in new.columns` | `KeyColumnMissing` | `"Key column '{key}' not in your DataFrame. DataFrame columns: {cols}."` |
| Duplicate keys in current (STRICT) | `current[key].duplicated().any()` | `DuplicateKeyInTarget` | `"Cannot update_in_place: key '{key}' has {n} duplicate values in tab '{tab}' (rows {row_examples}). Use mode=overwrite, or pick a unique-valued key column."` |
| Duplicate keys in new df | `new[key].duplicated().any()` | `DuplicateKeyInInput` | `"Cannot update_in_place: key '{key}' has {n} duplicate values in your DataFrame. Each row must be uniquely identified."` |
| Column not in target | Any column in `columns` arg or in `new` cols is not in `cur` cols | `ColumnMismatch` | `"Column mismatch: your DataFrame has columns {new_cols} but tab '{tab}' has {cur_cols}. Column(s) {extra} don't exist in the tab. RECOMMENDED: use a different tab name (e.g., '{tab}_enriched') to write the new shape as a fresh tab."` |
| Schema-changing overwrite | `mode == "overwrite"` AND `set(new.columns) != set(cur.columns)` | `SchemaChange` | `"Overwriting '{tab}' would change its schema: current {cur_cols} → new {new_cols}. This is likely a mistake. RECOMMENDED: use a different tab name. To proceed anyway, add 'allow_schema_change: true' to the spec dict."` |

**Duplicate strictness — design decision logged.** We chose strict mode (option `a` in brainstorming): if `key` has ANY duplicates in the current tab, `update_in_place` rejects, even if the rows being touched would themselves be unique. Reasoning:
- Simpler mental model — "the key column must be a key."
- Tables with duplicate values in the natural PK column are a schema problem worth surfacing.
- The escape hatch (`mode: "overwrite"`) is one config change away.

**Overwrite permissiveness — design decision logged.** When the LLM explicitly passes `mode: "overwrite"`, the dispatcher allows it (subject to the schema-change check). Rationale: the `on_existing_sheet: "fail"` policy already forced the LLM through an error round-trip; if it comes back with explicit `overwrite`, it knows what it's doing. Operators can still ban overwrite globally by setting `on_existing_sheet: "fail"` and not whitelisting `overwrite` in… actually no, we don't add a separate ban; if an operator wants to forbid overwrite, they review tool calls before sending. Out of scope here.

### §4 Removal of legacy single-tab path (crdt_doc)

`crdt_doc_run_python` currently accepts top-level args `write_to_sheet: String` + script-global `output_sheet: DataFrame`. This is the predecessor of `output_sheets = {name: df}`. ADP has no consumers depending on it. Remove:

- Drop `write_to_sheet` field from `CrdtDocRunPythonArgs`.
- Drop the postlude block that wraps a global `output_sheet` into the output JSON.
- Drop the dispatcher branch that handles `write_to_sheet + output_sheet`.

Migrate test graphs:
- `tests/graphs/crdt_documents/c_import_analysis.json`
- `tests/graphs/crdt_documents/f_cross_artifact_smoke.json`
- `tests/graphs/crdt_documents/c_pandas_smoke.json`

Each currently has a system message or prompt instructing the LLM to use `write_to_sheet` and `output_sheet`. Rewrite to use `output_sheets = {"NameHere": df}` patterns. Verify each graph still completes end-to-end via `test_graph`.

### §5 Updates to docs and YAML

#### `src/libs/colmena/text/tools/gsheets.yaml`

Extend `gsheets_run_python.description` to document:
- New `output_sheets` spec dict shape with `mode`.
- The three modes (`replace`, `update_in_place`, `overwrite`) and when to use each.
- The collision policy and the error the LLM might receive.

#### `src/libs/colmena/text/tools/crdt_doc.yaml` (or wherever crdt_doc_run_python's description lives)

Same extension. Also remove documentation of the now-deleted `write_to_sheet` arg.

#### `src/libs/colmena/skills/gsheets-cross-sheet-analysis/SKILL.md`

Add a new section: "Updating existing tabs in place" with concrete examples (e.g., "change price for all products where category=X").

#### `src/libs/colmena/skills/crdt-doc-table-exploration/`

Mirror update.

---

## Non-goals

- Per-call override of `on_existing_sheet` policy. The operator owns the policy; the LLM chooses among allowed actions but doesn't change the policy.
- New `append` / `upsert` / `delete_where` modes. Maybe later, not in this PR. `update_in_place` covers the dominant patching case; appends can already be modeled with `replace` writing to a new tab.
- Conflict resolution for concurrent edits (lost-update problem). Sheets API is last-writer-wins; we accept that for now.
- A typed `gsheets_query` tool for "top N most expensive products" type queries. Already decided against in the [Beta brainstorming](2026-06-06-pandas-multisheet-and-exploration-design.md) — pandas + skill guidance handles it.

---

## Testing

### Unit tests (Rust, per dispatcher)

For both `gsheets_run_python.rs` and `crdt_doc_run_python.rs`:

1. `collision_default_fails`: target tab exists, `on_existing_sheet` defaults to `fail` → assert returned error is `SheetExists` with `current_state` populated.
2. `collision_auto_suffix_writes_with_suffix`: target tab exists, `on_existing_sheet=auto_suffix` → tab written as `"Name (2)"`, response includes `resolved_name`.
3. `collision_overwrite_writes_over`: `on_existing_sheet=overwrite` → tab replaced entirely, no error.
4. `update_in_place_happy_path`: 1000-row tab, df with 47 modified rows, key=`product_id` → assert exactly 47 cell writes issued; response shows `changes.rows=47`.
5. `update_in_place_no_changes`: identical df → `changes.cells=0`, NO API call made.
6. `update_in_place_skips_unmatched`: df has 5 rows; 3 match keys in target, 2 don't → 3 patched, response shows `skipped.rows_not_in_target=2`.
7. `update_in_place_strict_match_rejects`: same as above with `strict_match: true` → error, no writes.
8. `validation_duplicate_key_in_target`: target has duplicate keys → `DuplicateKeyInTarget` error.
9. `validation_duplicate_key_in_input`: df has duplicate keys → `DuplicateKeyInInput`.
10. `validation_key_missing_in_target` / `..._in_input`.
11. `validation_column_mismatch`: df has extra column not in target → `ColumnMismatch`.
12. `validation_overwrite_schema_change`: explicit `mode=overwrite` with different cols → `SchemaChange` error.
13. `validation_overwrite_allow_schema_change`: with `allow_schema_change: true` → succeeds.
14. `bare_dataframe_still_works`: `output_sheets = {"X": df}` (no spec dict) → same behavior as today's `replace` mode, defaulting to collision policy.

### Integration tests (E2E with real Sheets)

Use the existing `tests/graphs/agents/gsheets_*.json` pattern. New graph `tests/graphs/agents/gsheets_update_in_place.json`:
- Pre-seeds a sheet with 100 products via `gsheets_set_range`.
- Asks the agent: "Change the price of products in category 'Electronics' to apply a 10% discount."
- Asserts: only N cells written (verifiable via the response payload), no schema change, no new tabs.

### Migration verification

After legacy removal:
- `cargo test --workspace` must pass.
- `cargo test --lib --features python` must pass.
- Each migrated graph runs cleanly via `cargo run --bin dag_engine -- run tests/graphs/crdt_documents/<file>.json`.

---

## Edge cases

| Case | Handling |
|---|---|
| `update_in_place` with empty df (0 rows) | Returns `{"changes":{"rows":0,"cells":0}}` immediately, no API call, no error. |
| `update_in_place` on empty target tab | `KeyColumnMissing` (the empty tab has no columns). LLM should switch to `replace`. |
| `key` column values include NaN | NaN keys are dropped from indexing on BOTH sides with a warning in the response (`skipped.null_keys`). |
| NaN cells in comparison | `pd.isna(a) == pd.isna(b)` short-circuits — NaN-to-NaN is "no change", NaN-to-value is a change. |
| `columns` arg references a column not in `new` | `ColumnMismatch` error, lists which. |
| Concurrent writer adds a row between read and write | Last-writer-wins, no detection. Documented limitation. |
| `output_sheets` value is `None` or non-dict-non-DataFrame | Pre-existing validation catches it; extend to mention spec-dict shape in the error message. |
| Operator sets `on_existing_sheet` to an unknown value | Init-time error from the node, clear message listing valid values. |

---

## Open items / follow-ups

These are intentionally deferred:

1. **Cost tracking.** `update_in_place` writes far fewer cells; the cost-tracking system (still pending per project status) should report cells-written, not rows.
2. **Frontend rendering of `update_in_place` results.** The "changed cells: 47" payload deserves a richer view than a generic JSON dump. Out of scope for the colmena PR; flag in ADP follow-up.
3. **Diff preview before write.** Future: a `dry_run: true` flag that returns what WOULD change without writing. Skip for now.
4. **append / upsert modes.** Likely in a follow-up spec; tracked in BACKLOG.

---

## Backward compatibility

- Existing graphs using `output_sheets = {name: df}` (bare DataFrame) continue to work — but **collision behavior changes** from `auto_suffix` to `fail`. This is a deliberate, breaking-but-safer change. Operators who depend on auto-suffix must set `fixed_config.on_existing_sheet: "auto_suffix"`.
- Existing graphs using legacy `write_to_sheet` + `output_sheet` (crdt_doc) break — they must be migrated. The 3 in-repo test graphs are migrated as part of this work. ADP has no other consumers (confirmed).
- The `gsheets_run_python` test `args_accept_output_sheets_as_tool_arg_silently` continues to pass — the LLM-tolerance shim for the misplaced top-level arg is unchanged.

---

## Files touched (approximate)

```
src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/
  gsheets_run_python.rs      MODIFY  (add policy + validation + update_in_place)
  crdt_doc_run_python.rs     MODIFY  (same as above + remove legacy single-tab)

src/libs/colmena/src/dag_engine/infrastructure/sheets/  (or equivalent)
  collision_policy.rs        CREATE  (shared validation + error shaping)
  diff_writer.rs             CREATE  (cell-diff algorithm, reusable)

src/libs/colmena/text/tools/
  gsheets.yaml               MODIFY  (description + modes documented)
  crdt_doc.yaml              MODIFY  (description + remove legacy)

src/libs/colmena/text/prompts/python_sandbox/
  gsheets_run_python_postlude.md   MODIFY  (handle spec-dict shape)
  crdt_doc_run_python_postlude.md  MODIFY  (same + drop output_sheet handling)

src/libs/colmena/skills/
  gsheets-cross-sheet-analysis/SKILL.md  MODIFY  (new section)
  crdt-doc-table-exploration/SKILL.md    MODIFY  (mirror)

tests/graphs/crdt_documents/
  c_import_analysis.json     MODIFY  (migrate off legacy)
  f_cross_artifact_smoke.json MODIFY
  c_pandas_smoke.json        MODIFY

tests/graphs/agents/
  gsheets_update_in_place.json CREATE  (E2E test)

docs/developer_guide/
  (relevant gsheets/crdt_doc guides)   MODIFY  (collision policy + modes)
```

---

## Implementation phase plan (preview, finalized in writing-plans)

Roughly 6-8 hours of work, decomposed:

1. Shared `collision_policy.rs` and `diff_writer.rs` modules with unit tests.
2. Wire policy into `gsheets_run_python` dispatcher.
3. Wire policy into `crdt_doc_run_python` dispatcher.
4. Add `update_in_place` mode to gsheets postlude + dispatcher.
5. Add same to crdt_doc.
6. Remove legacy `write_to_sheet` path from crdt_doc.
7. Migrate 3 test graphs.
8. Update YAML descriptions and skills.
9. New E2E integration test graph.

Full step-by-step plan: `docs/superpowers/plans/2026-06-06-sheets-write-safety.md` (next).
