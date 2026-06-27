# gsheets `update_by_position` Adds New Columns — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `gsheets_run_python` mode `update_by_position` append columns that exist in the returned DataFrame but not in the sheet header, instead of silently dropping them.

**Architecture:** Add a pure, unit-testable helper `plan_new_columns` that maps df-only columns to the next free column indices and emits the header + body cells to write. Wire it into `do_update_by_position` so its writes merge into the single existing `batch_update_cells`, with formulas resolved (including refs to other new columns) and the new columns reported via a new additive `added_columns` field.

**Tech Stack:** Rust (crate `colmena_dag_engine`), `serde_json`, Google Sheets `values:batchUpdate`. Spec: [`docs/superpowers/specs/2026-06-26-gsheets-update-by-position-add-columns-design.md`](../specs/2026-06-26-gsheets-update-by-position-add-columns-design.md).

All paths below are relative to the worktree root `/Users/danielgarcia/startti/colmena/.claude/worktrees/competent-bhabha-7e5274`. The single source file touched for code is:
`src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gsheets_run_python.rs` (referred to as **THE FILE** below).

---

## Task 1: Pure helper `plan_new_columns` + types (TDD)

**Files:**
- Modify: THE FILE — add `PlannedCell`/`NewColumnPlan` types and `fn plan_new_columns` near the other pure helpers (after `addressable_columns`, around line 975, before `do_update_by_position`).
- Test: THE FILE — add tests to the existing `mod position_tests` (around line 1614, after `resolve_per_row_formula_to_real_a1`).

- [ ] **Step 1: Write the failing tests**

Add these tests inside `mod position_tests` (it already has `use super::*; use serde_json::json;` and an `idx()` helper):

```rust
fn rec(pairs: &[(&str, serde_json::Value)]) -> serde_json::Map<String, serde_json::Value> {
    pairs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect()
}

#[test]
fn plan_new_columns_appends_after_last_header() {
    let header = vec!["a".to_string(), "b".to_string()]; // A, B occupied → new lands at C (idx 2)
    let new = vec![
        rec(&[("a", json!(1)), ("b", json!(2)), ("Margen", json!("=C-D"))]),
        rec(&[("a", json!(3)), ("b", json!(4)), ("Margen", json!("=C-D"))]),
    ];
    let plan = plan_new_columns(&header, &new, &idx(&[0, 1]));
    assert_eq!(plan.added, vec![("Margen".to_string(), 2)]);
    // header cell at row 1 + one body cell per record (rows 2,3).
    assert_eq!(
        plan.cells,
        vec![
            PlannedCell { col_idx: 2, row: 1, raw: json!("Margen") },
            PlannedCell { col_idx: 2, row: 2, raw: json!("=C-D") },
            PlannedCell { col_idx: 2, row: 3, raw: json!("=C-D") },
        ]
    );
}

#[test]
fn plan_new_columns_multiple_get_consecutive_indices() {
    let header = vec!["a".to_string()]; // A occupied → new cols at B(1), C(2)
    let new = vec![rec(&[("a", json!(1)), ("x", json!(10)), ("y", json!(20))])];
    let plan = plan_new_columns(&header, &new, &idx(&[0]));
    assert_eq!(plan.added, vec![("x".to_string(), 1), ("y".to_string(), 2)]);
}

#[test]
fn plan_new_columns_ignores_existing_header_names() {
    let header = vec!["a".to_string(), "b".to_string()];
    let new = vec![rec(&[("a", json!(1)), ("b", json!(9))])]; // both already in header
    let plan = plan_new_columns(&header, &new, &idx(&[0]));
    assert!(plan.added.is_empty());
    assert!(plan.cells.is_empty());
}

#[test]
fn plan_new_columns_skips_all_null_column() {
    let header = vec!["a".to_string()];
    let new = vec![
        rec(&[("a", json!(1)), ("Empty", serde_json::Value::Null)]),
        rec(&[("a", json!(2)), ("Empty", serde_json::Value::Null)]),
    ];
    let plan = plan_new_columns(&header, &new, &idx(&[0, 1]));
    assert!(plan.added.is_empty(), "all-null column must not create an orphan header");
    assert!(plan.cells.is_empty());
}

#[test]
fn plan_new_columns_uses_df_index_for_row_mapping() {
    let header = vec!["a".to_string()]; // new col at B(1)
    // df_index is out of natural order: record 0 → sheet row 3, record 1 → sheet row 2.
    let new = vec![
        rec(&[("a", json!(1)), ("m", json!("r3"))]),
        rec(&[("a", json!(2)), ("m", json!("r2"))]),
    ];
    let plan = plan_new_columns(&header, &new, &idx(&[1, 0]));
    // header at row 1, then body rows = df_index + 2 (so 1+2=3, then 0+2=2).
    assert_eq!(
        plan.cells,
        vec![
            PlannedCell { col_idx: 1, row: 1, raw: json!("m") },
            PlannedCell { col_idx: 1, row: 3, raw: json!("r3") },
            PlannedCell { col_idx: 1, row: 2, raw: json!("r2") },
        ]
    );
}
```

- [ ] **Step 2: Run tests to verify they fail (compile error — symbols undefined)**

Run: `cargo test -p colmena_dag_engine --lib position_tests::plan_new_columns 2>&1 | tail -20`
Expected: FAIL — `cannot find type 'PlannedCell'` / `cannot find function 'plan_new_columns'`.

- [ ] **Step 3: Implement the types + helper**

Insert in THE FILE just before `async fn do_update_by_position` (around line 977):

```rust
/// A single cell a new column needs: a header cell (row 1) or a body value.
/// `raw` is the value as it came from the df — formula resolution happens in the
/// caller, where the full column→index map (existing + new) is available.
#[derive(Debug, PartialEq)]
struct PlannedCell {
    col_idx: usize,
    /// 1-based; row 1 is the header.
    row: usize,
    raw: serde_json::Value,
}

/// What `update_by_position` should write for columns present in the returned df
/// but absent from the sheet header.
#[derive(Debug, Default, PartialEq)]
struct NewColumnPlan {
    /// (column name, 0-based column index) for each added column, in df order.
    added: Vec<(String, usize)>,
    /// Header cells (row 1) + body cells, in write order.
    cells: Vec<PlannedCell>,
}

/// For each df column whose name is NOT in `header_cols`, assign the next free
/// column index (appended after the last header column, in df-column order) and
/// emit its header cell plus one body cell per record with a non-null value.
/// A column whose body is entirely null is skipped (no orphan header). The sheet
/// row for record `i` is `df_index[i] + 2` (header is row 1).
fn plan_new_columns(
    header_cols: &[String],
    new_records: &[serde_json::Map<String, serde_json::Value>],
    df_index: &[serde_json::Value],
) -> NewColumnPlan {
    use std::collections::HashSet;
    let existing: HashSet<&str> = header_cols.iter().map(String::as_str).collect();

    // Discover new column names in first-seen df order, excluding the synthetic
    // index and any name already present in the header.
    let mut new_names: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for r in new_records {
        for k in r.keys() {
            if k == "__index__" || existing.contains(k.as_str()) {
                continue;
            }
            if seen.insert(k.clone()) {
                new_names.push(k.clone());
            }
        }
    }

    let mut plan = NewColumnPlan::default();
    let mut next_idx = header_cols.len();
    for name in new_names {
        let mut body: Vec<PlannedCell> = Vec::new();
        for (i, r) in new_records.iter().enumerate() {
            let Some(v) = r.get(&name) else { continue };
            if v.is_null() {
                continue;
            }
            let Some(row_idx) = df_index.get(i).and_then(serde_json::Value::as_u64) else {
                continue;
            };
            body.push(PlannedCell {
                col_idx: next_idx,
                row: row_idx as usize + 2,
                raw: v.clone(),
            });
        }
        if body.is_empty() {
            continue; // entirely-null column → don't create an orphan header.
        }
        plan.cells.push(PlannedCell {
            col_idx: next_idx,
            row: 1,
            raw: serde_json::Value::String(name.clone()),
        });
        plan.cells.extend(body);
        plan.added.push((name, next_idx));
        next_idx += 1;
    }
    plan
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p colmena_dag_engine --lib position_tests::plan_new_columns 2>&1 | tail -20`
Expected: PASS — 5 tests.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gsheets_run_python.rs
git commit -m "feat(gsheets): plan_new_columns helper for update_by_position

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 2: Wire new columns into `do_update_by_position`

**Files:**
- Modify: THE FILE — `do_update_by_position`, the region between the existing diff→`cell_updates` loop (ends ~line 1149) and the response (`let cells = cell_updates.len();` ~line 1151; success branch ~line 1163-1170).

- [ ] **Step 1: Append new-column writes before counting cells**

In THE FILE, find the end of the diff loop (right after the `for chg in &diff.changes { ... }` block closes, before `let cells = cell_updates.len();`). Insert:

```rust
    // Append columns present in the returned df but absent from the sheet header.
    // They get the next free column indices; their formulas may reference other
    // new columns, so resolve against existing + new positions.
    let new_plan = plan_new_columns(&header_cols, &new_records, &df_index);
    let added_columns: Vec<serde_json::Value> = new_plan
        .added
        .iter()
        .map(|(name, i)| serde_json::json!({ "name": name, "column": col_letter(*i) }))
        .collect();
    let new_column_names: Vec<String> =
        new_plan.added.iter().map(|(name, _)| name.clone()).collect();
    if !new_plan.cells.is_empty() {
        let mut resolvable = col_to_index.clone();
        for (name, i) in &new_plan.added {
            resolvable.insert(name.clone(), *i);
        }
        for pc in &new_plan.cells {
            let resolved = match resolve_formula_placeholders(&pc.raw, &resolvable, pc.row) {
                Ok(v) => v,
                Err(e) => return e.to_json(raw_name),
            };
            let addr = a1_addr(pc.col_idx, pc.row);
            formula_log.record(&addr, &resolved);
            cell_updates.push((addr, crate::gsheets::domain::CellValue::from_json(&resolved)));
        }
    }
```

Note: `col_to_index`, `new_records`, `df_index`, `header_cols`, `formula_log`, and `cell_updates` are all already in scope at this point in the function (verify by reading lines 1056-1149 of THE FILE).

- [ ] **Step 2: Surface `added_columns` and the new columns in the success response**

In THE FILE, find the success branch (`Ok(_) =>` around line 1163) where `resp` is built as:

```rust
            let mut resp = serde_json::json!({
                "tab": raw_name, "mode": "update_by_position",
                "changes": {"rows": diff.rows_changed, "cells": cells, "columns": diff.columns_touched},
                "skipped_columns": skipped_columns,
            });
```

Replace that `let mut resp = ...;` statement with:

```rust
            let mut columns_touched = diff.columns_touched.clone();
            columns_touched.extend(new_column_names.iter().cloned());
            let mut resp = serde_json::json!({
                "tab": raw_name, "mode": "update_by_position",
                "changes": {"rows": diff.rows_changed, "cells": cells, "columns": columns_touched},
                "skipped_columns": skipped_columns,
            });
            if !added_columns.is_empty() {
                resp["added_columns"] = serde_json::Value::Array(added_columns.clone());
            }
```

(The zero-change early return at `if cells == 0` stays as-is: when only new columns are added, `cells > 0`, so that branch is not taken.)

- [ ] **Step 3: Build + run the whole gsheets unit suite (existing tests must stay green; deny-warnings must pass)**

Run: `cargo test -p colmena_dag_engine --lib gsheets_run_python 2>&1 | tail -25`
Expected: PASS — all `position_tests` (including the 5 new) and existing module tests green, no warnings (the crate denies warnings).

Run: `cargo clippy -p colmena_dag_engine --lib 2>&1 | tail -15`
Expected: no warnings/errors.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gsheets_run_python.rs
git commit -m "feat(gsheets): update_by_position appends df-only columns

Columns present in the returned DataFrame but absent from the sheet
header are now created (header + body, formulas resolved) in the same
atomic batch, reported via a new additive added_columns field, instead
of being silently dropped.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 3: Docs — make the promised behavior explicit

**Files:**
- Modify: `src/libs/colmena/text/tools/gsheets.yaml` (the `gsheets_run_python` description, Mode 4 / `update_by_position` area, around lines 149-164 and 211-218).
- Modify: `src/libs/colmena/skills/gsheets-editing/references/edit-rows.md` (the "Rules" bullet at line 36-38 that says "Do NOT add rows with this mode").

- [ ] **Step 1: Add a note to the tool description**

In `gsheets.yaml`, in the `update_by_position` "Mode 4" block (after the formula-fill examples, near line 164), add a sentence:

```
    # Adding a NEW column: assign it on the bound df (df['Margen'] = ...). A column
    # NOT already in the sheet header is APPENDED after the last column (header +
    # values written in the same atomic batch) and reported under `added_columns`
    # [{name, column}]. (Filas nuevas NO: este modo edita filas existentes.)
```

- [ ] **Step 2: Fix the misleading rule in edit-rows.md**

In `edit-rows.md`, replace the bullet:

```
- Do NOT add rows with this mode (it edits existing rows only). Columns whose
  header name is empty or duplicated can't be addressed and are reported in
  `skipped_columns`.
```

with:

```
- Do NOT add rows with this mode (it edits existing rows only). You CAN add a
  new COLUMN: assign it on the bound df (`df['Margen'] = '={{Venta}}-{{Costo}}'`)
  and it is appended after the last column (reported under `added_columns`).
  Existing columns whose header name is empty or duplicated can't be addressed
  and are reported in `skipped_columns`.
```

- [ ] **Step 3: Commit**

```bash
git add src/libs/colmena/text/tools/gsheets.yaml src/libs/colmena/skills/gsheets-editing/references/edit-rows.md
git commit -m "docs(gsheets): document update_by_position column-append + added_columns

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 4: E2E verification against the real `products` sheet

This is the acceptance test the user asked for: the ORIGINAL failing flow (no `set_cell` workaround) must now create the `Margen` column.

**Files:**
- Reuse: `/tmp/colmena_e2e/repro_add_column.json` (created during diagnosis; the failing repro graph).

- [ ] **Step 1: Build the release binary in the worktree**

Run: `cargo build --release --bin dag_engine 2>&1 | tail -5`
Expected: `Finished release`. Confirm the binary embeds python@3.14: `otool -L target/release/dag_engine | grep -i python` → shows `python@3.14`.

- [ ] **Step 2: Run the original repro graph against real Google**

Run (from the worktree root; OAuth creds + pandas PYTHONPATH per the local gsheets E2E runbook — `.env`/`.venv` live in the main repo root `/Users/danielgarcia/startti/colmena`):

```bash
WT=$(pwd); MAIN=/Users/danielgarcia/startti/colmena
set -a; source "$MAIN/.env"; set +a
unset ANTHROPIC_BASE_URL
export COLMENA_GOOGLE_SHARE_EMAIL=agents@startti.co
export COLMENA_GOOGLE_OAUTH_CLIENT_ID=$(gcloud secrets versions access latest --secret=colmena-oauth-client-id --project=startti-dev)
export COLMENA_GOOGLE_OAUTH_CLIENT_SECRET=$(gcloud secrets versions access latest --secret=colmena-oauth-client-secret --project=startti-dev)
export COLMENA_GOOGLE_OAUTH_REFRESH_TOKEN=$(gcloud secrets versions access latest --secret=colmena-oauth-refresh-token --project=startti-dev)
export PYTHONPATH="$MAIN/.venv/lib/python3.14/site-packages"
"$WT/target/release/dag_engine" run /tmp/colmena_e2e/repro_add_column.json \
  --agent-session-id repro_fix_$(date +%s) --include-extra-info \
  > /tmp/colmena_e2e/repro_add_column_FIXED.sse 2>&1
echo "exit=$?"
```

(If `gcloud` prompts for reauth, run `gcloud auth login` first — it is interactive.)

- [ ] **Step 3: Verify the column was created**

Run:
```bash
grep -oE '"mode":"update_by_position"[^}]*\}[^}]*\}' /tmp/colmena_e2e/repro_add_column_FIXED.sse | head -3
grep -oE '"added_columns":\[[^]]*\]' /tmp/colmena_e2e/repro_add_column_FIXED.sse | head -1
grep -oE '"formula_cells":\{"H2":"[^"]*"' /tmp/colmena_e2e/repro_add_column_FIXED.sse | head -1
```
Expected:
- `changes` shows `cells > 0` and `"columns"` includes `"Margen"` (NOT `cells:0`).
- `added_columns` contains `{"name":"Margen","column":"H"}`.
- `formula_cells` shows `H2` → `=F2-E2` (price=F, cost=E).
- The agent's final read confirms `Margen` is now a header with formula values.

If `cells:0` / no `added_columns` → the fix did not take effect; STOP and debug before claiming success (per verification-before-completion).

- [ ] **Step 4: Final full test sweep before wrap-up**

Run: `cargo test -p colmena_dag_engine --lib 2>&1 | tail -5`
Expected: all unit tests pass.

---

## Notes for the implementer
- No colmena public-API change → ADP worker unaffected (only the tool-result wire-format gains the additive `added_columns`).
- `update_in_place` is intentionally NOT touched (it already errors clearly on unknown columns).
- Keep everything in one source file; do not restructure `gsheets_run_python.rs`.
