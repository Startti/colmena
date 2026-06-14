# gsheets expand-merges Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** When reading a Google Sheet, forward-fill merged cells so every cell of a merge returns the anchor's value — automatically, on every read, for both `gsheets_read` and `gsheets_run_python`.

**Architecture:** Put the forward-fill inside `SheetsClient::read_range` (gsheets infrastructure). Both LLM read surfaces call `read_range`, so both inherit the fill with no changes of their own. Switch `read_range` from `spreadsheets.values.get` to `spreadsheets.get` with `includeGridData=true`, which returns cell values **and** merge rectangles in a single request (Approach B) — fresh on every read, no cache. A new pure module `merge_fill.rs` does the rectangle fill. Sub-ranges that cut a merge keep the pre-feature behavior (B1: only anchors present in the returned grid are filled).

**Tech Stack:** Rust (crate `colmena_dag_engine`), `serde_json`, `wiremock` for HTTP unit tests, Google Sheets API v4. Spec: [`docs/superpowers/specs/2026-06-14-gsheets-expand-merges-design.md`](../specs/2026-06-14-gsheets-expand-merges-design.md).

**Build/test commands (this repo):**
- Module unit tests: `cargo test --lib -p colmena_dag_engine <module>`
- Full pre-push: `cargo test --verbose` (CI command — runs unit + integration + doctests)
- Format/lint: `cargo fmt` and `cargo clippy`
- Deny-warnings is ON (`warnings = "deny"`): no unused imports / dead code.
- E2E graph run: `cargo run --bin dag_engine -- run <graph.json>` (see Task 5 for the real-Google runbook).

---

## File Structure

| File | Responsibility | Action |
|---|---|---|
| `src/libs/colmena/src/gsheets/infrastructure/merge_fill.rs` | Pure `MergeRect` + `forward_fill_merges` over a 2-D `serde_json::Value` grid. No I/O. | **Create** |
| `src/libs/colmena/src/gsheets/infrastructure/mod.rs` | Wire `pub mod merge_fill;` | **Modify** |
| `src/libs/colmena/src/gsheets/infrastructure/http_client.rs` | Grid-data parse helpers (`cell_scalar`, `extended_value_to_scalar`, `parse_grid_block`, `parse_merges`) + rewritten `read_range` (Approach B). Update/extend wiremock tests. | **Modify** |
| `src/libs/colmena/text/tools/gsheets.yaml` | Tool descriptions for `gsheets_read` + `gsheets_run_python` note auto-expansion of merged cells. | **Modify** |
| `tests/graphs/agents/gsheets_expand_merges_read.json` | E2E graph: read a real sheet with merges via `gsheets_read`. | **Create** |
| `tests/graphs/agents/gsheets_expand_merges_python.json` | E2E graph: `gsheets_run_python` groupby over a previously-merged column. | **Create** |

`gsheets_run_python.rs` and `gsheets_tools.rs` are **not modified** — they call `read_range`, so they inherit the fill. The E2E in Task 5 proves this end-to-end.

---

## Task 1: Pure forward-fill module (`merge_fill.rs`)

**Files:**
- Create: `src/libs/colmena/src/gsheets/infrastructure/merge_fill.rs`
- Modify: `src/libs/colmena/src/gsheets/infrastructure/mod.rs`

- [ ] **Step 1: Write the module with its failing tests**

Create `src/libs/colmena/src/gsheets/infrastructure/merge_fill.rs`:

```rust
//! Pure forward-fill of merged cells for sheet reads.
//!
//! Google's `spreadsheets.get` returns a merged region's value only in its
//! top-left (anchor) cell; the rest of the rectangle is empty. This module
//! propagates the anchor value across the whole rectangle so a read reflects
//! what the merge visually represents.
//!
//! Spec: `docs/superpowers/specs/2026-06-14-gsheets-expand-merges-design.md`.

use serde_json::Value;

/// A merge rectangle in absolute sheet coordinates (half-open: `start..end`,
/// `end` exclusive — the convention Google's API uses).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MergeRect {
    pub start_row: usize,
    pub end_row: usize,
    pub start_col: usize,
    pub end_col: usize,
}

/// Forward-fill merged cells in `grid` (row-major; `grid[r][c]` is the cell at
/// grid-relative `(r, c)`).
///
/// `row_offset`/`col_offset` are the absolute sheet coordinates of `grid[0][0]`
/// (the `startRow`/`startColumn` of the returned `GridData`). For each merge
/// whose anchor `(start_row, start_col)` — after subtracting the offsets —
/// falls inside the grid, the anchor's value is cloned into every cell of the
/// rectangle, clamped to the grid bounds.
///
/// B1 (per spec): merges whose anchor falls OUTSIDE the grid (e.g. a sub-range
/// that begins partway down a vertical merge) are skipped — those cells keep
/// whatever they had, matching the pre-feature behavior.
pub fn forward_fill_merges(
    grid: &mut [Vec<Value>],
    merges: &[MergeRect],
    row_offset: usize,
    col_offset: usize,
) {
    for m in merges {
        // Anchor before the offset ⇒ outside the returned grid (B1): skip.
        if m.start_row < row_offset || m.start_col < col_offset {
            continue;
        }
        let ar = m.start_row - row_offset; // anchor row, grid-relative
        let ac = m.start_col - col_offset; // anchor col, grid-relative
        let Some(anchor_row) = grid.get(ar) else {
            continue;
        };
        let Some(anchor_val) = anchor_row.get(ac) else {
            continue;
        };
        let anchor_val = anchor_val.clone();
        let r_end = m.end_row.saturating_sub(row_offset).min(grid.len());
        for r in ar..r_end {
            let row = &mut grid[r];
            let c_end = m.end_col.saturating_sub(col_offset).min(row.len());
            for c in ac..c_end {
                row[c] = anchor_val.clone();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn grid(rows: Vec<Vec<Value>>) -> Vec<Vec<Value>> {
        rows
    }

    #[test]
    fn horizontal_merge_fills_to_the_right() {
        // A1:C1 merged, anchor "H". Row width 3.
        let mut g = grid(vec![
            vec![json!("H"), json!(null), json!(null)],
            vec![json!(1), json!(2), json!(3)],
        ]);
        let merges = [MergeRect { start_row: 0, end_row: 1, start_col: 0, end_col: 3 }];
        forward_fill_merges(&mut g, &merges, 0, 0);
        assert_eq!(g[0], vec![json!("H"), json!("H"), json!("H")]);
        assert_eq!(g[1], vec![json!(1), json!(2), json!(3)]); // row 2 untouched
    }

    #[test]
    fn vertical_merge_fills_downward() {
        // A1:A3 merged, anchor "Cat".
        let mut g = grid(vec![
            vec![json!("Cat"), json!(10)],
            vec![json!(null), json!(20)],
            vec![json!(null), json!(30)],
        ]);
        let merges = [MergeRect { start_row: 0, end_row: 3, start_col: 0, end_col: 1 }];
        forward_fill_merges(&mut g, &merges, 0, 0);
        assert_eq!(g[0][0], json!("Cat"));
        assert_eq!(g[1][0], json!("Cat"));
        assert_eq!(g[2][0], json!("Cat"));
        // Column B untouched.
        assert_eq!((g[0][1].clone(), g[1][1].clone(), g[2][1].clone()), (json!(10), json!(20), json!(30)));
    }

    #[test]
    fn block_merge_fills_both_directions() {
        // A1:B2 merged, anchor "X".
        let mut g = grid(vec![
            vec![json!("X"), json!(null)],
            vec![json!(null), json!(null)],
        ]);
        let merges = [MergeRect { start_row: 0, end_row: 2, start_col: 0, end_col: 2 }];
        forward_fill_merges(&mut g, &merges, 0, 0);
        assert_eq!(g, vec![
            vec![json!("X"), json!("X")],
            vec![json!("X"), json!("X")],
        ]);
    }

    #[test]
    fn anchor_outside_grid_is_skipped_b1() {
        // Merge A1:A5 in absolute coords, but the grid starts at row 2 (offset 2):
        // the anchor (row 0) is above the returned grid, so nothing is filled.
        let mut g = grid(vec![
            vec![json!(null)], // abs row 2
            vec![json!(null)], // abs row 3
        ]);
        let merges = [MergeRect { start_row: 0, end_row: 5, start_col: 0, end_col: 1 }];
        forward_fill_merges(&mut g, &merges, 2, 0);
        assert_eq!(g, vec![vec![json!(null)], vec![json!(null)]]); // unchanged
    }

    #[test]
    fn offset_applied_when_anchor_inside_grid() {
        // Grid starts at abs (row 2, col 1). Merge anchor at abs (2,1) → grid (0,0).
        let mut g = grid(vec![
            vec![json!("A"), json!(null)],
            vec![json!(null), json!(null)],
        ]);
        let merges = [MergeRect { start_row: 2, end_row: 4, start_col: 1, end_col: 3 }];
        forward_fill_merges(&mut g, &merges, 2, 1);
        assert_eq!(g, vec![vec![json!("A"), json!("A")], vec![json!("A"), json!("A")]]);
    }

    #[test]
    fn no_merges_is_noop() {
        let original = grid(vec![vec![json!(1), json!(2)], vec![json!(3), json!(4)]]);
        let mut g = original.clone();
        forward_fill_merges(&mut g, &[], 0, 0);
        assert_eq!(g, original);
    }

    #[test]
    fn merge_extending_past_grid_is_clamped() {
        // Merge claims A1:A10 but grid only has 2 rows — no panic, fill what exists.
        let mut g = grid(vec![vec![json!("V")], vec![json!(null)]]);
        let merges = [MergeRect { start_row: 0, end_row: 10, start_col: 0, end_col: 1 }];
        forward_fill_merges(&mut g, &merges, 0, 0);
        assert_eq!(g, vec![vec![json!("V")], vec![json!("V")]]);
    }

    #[test]
    fn empty_anchor_fills_with_null_noop() {
        let mut g = grid(vec![vec![json!(null), json!(null)]]);
        let merges = [MergeRect { start_row: 0, end_row: 1, start_col: 0, end_col: 2 }];
        forward_fill_merges(&mut g, &merges, 0, 0);
        assert_eq!(g, vec![vec![json!(null), json!(null)]]);
    }
}
```

- [ ] **Step 2: Wire the module**

In `src/libs/colmena/src/gsheets/infrastructure/mod.rs`, add after `pub mod http_client;`:

```rust
pub mod merge_fill;
```

- [ ] **Step 3: Run tests to verify they pass**

Run: `cargo test --lib -p colmena_dag_engine merge_fill`
Expected: PASS (8 tests).

- [ ] **Step 4: Lint + format**

Run: `cargo fmt && cargo clippy -p colmena_dag_engine 2>&1 | tail -5`
Expected: no warnings (deny-warnings is on).

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/gsheets/infrastructure/merge_fill.rs src/libs/colmena/src/gsheets/infrastructure/mod.rs
git commit -m "feat(gsheets): pure forward-fill of merged cells (merge_fill.rs)"
```

---

## Task 2: Grid-data parse helpers in `http_client.rs`

These pure helpers convert a `spreadsheets.get` (includeGridData) response into the same `[[scalar,...],...]` 2-D JSON the read pipeline already consumes, honoring the three render options, plus extract the merge rectangles. No HTTP — testable over `serde_json` fixtures.

**Files:**
- Modify: `src/libs/colmena/src/gsheets/infrastructure/http_client.rs`

- [ ] **Step 1: Add imports and helper functions**

At the top of `http_client.rs`, update the domain import to add `ValueRenderOption`, and import the new merge types. Change the existing `use crate::gsheets::domain::{...}` block to include `ValueRenderOption`:

```rust
use crate::gsheets::domain::{
    CellValue, ReadOptions, ReadResponse, SetRangeResponse, SheetId, SheetMeta, SheetsClient,
    SheetsError, SpreadsheetId, SpreadsheetMeta, ValueRenderOption,
};
use crate::gsheets::infrastructure::merge_fill::{forward_fill_merges, MergeRect};
```

Then add these free functions next to `rectangle_to_records` (anywhere in the file's free-function region, e.g. just above `rectangle_to_records` at line ~906):

```rust
/// Google `ExtendedValue` → scalar JSON. `ExtendedValue` is a one-of:
/// `{numberValue | stringValue | boolValue | formulaValue | errorValue}`.
/// Empty / unknown ⇒ `Null`. Error cells surface their `type` string
/// (e.g. `"#REF!"`).
fn extended_value_to_scalar(ev: &Value) -> Value {
    if let Some(n) = ev.get("numberValue") {
        n.clone()
    } else if let Some(s) = ev.get("stringValue") {
        s.clone()
    } else if let Some(b) = ev.get("boolValue") {
        b.clone()
    } else if let Some(f) = ev.get("formulaValue") {
        f.clone()
    } else if let Some(e) = ev.get("errorValue") {
        Value::String(
            e.get("type")
                .and_then(Value::as_str)
                .unwrap_or("ERROR")
                .to_string(),
        )
    } else {
        Value::Null
    }
}

/// Map one grid `CellData` to the scalar JSON the read pipeline expects,
/// honoring the requested render option. Mirrors what `values.get` returned:
/// numbers as JSON numbers, strings as strings, booleans as bools, empty as
/// `Null`.
fn cell_scalar(cell: &Value, render: ValueRenderOption) -> Value {
    match render {
        ValueRenderOption::FormattedValue => {
            cell.get("formattedValue").cloned().unwrap_or(Value::Null)
        }
        ValueRenderOption::UnformattedValue => cell
            .get("effectiveValue")
            .map(extended_value_to_scalar)
            .unwrap_or(Value::Null),
        ValueRenderOption::Formula => match cell.get("userEnteredValue") {
            // Formula cells: the user-entered formula text. Non-formula
            // cells: their literal user-entered value.
            Some(uev) => match uev.get("formulaValue") {
                Some(f) => f.clone(),
                None => extended_value_to_scalar(uev),
            },
            None => Value::Null,
        },
    }
}

/// Parse the first sheet's first `GridData` block from a `spreadsheets.get`
/// response into a RECTANGULAR grid of scalar values plus the block's absolute
/// top-left offset `(row_offset, col_offset)`. Rows are padded to a uniform
/// width so forward-fill can reach every cell of a merge.
fn parse_grid_block(body: &Value, render: ValueRenderOption) -> (Vec<Vec<Value>>, usize, usize) {
    let block = body
        .get("sheets")
        .and_then(|s| s.as_array())
        .and_then(|s| s.first())
        .and_then(|s| s.get("data"))
        .and_then(|d| d.as_array())
        .and_then(|d| d.first());
    let Some(block) = block else {
        return (Vec::new(), 0, 0);
    };
    let row_offset = block.get("startRow").and_then(Value::as_u64).unwrap_or(0) as usize;
    let col_offset = block
        .get("startColumn")
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize;
    let mut grid: Vec<Vec<Value>> = block
        .get("rowData")
        .and_then(|r| r.as_array())
        .map(|rows| {
            rows.iter()
                .map(|row| {
                    row.get("values")
                        .and_then(|v| v.as_array())
                        .map(|cells| cells.iter().map(|c| cell_scalar(c, render)).collect())
                        .unwrap_or_default()
                })
                .collect()
        })
        .unwrap_or_default();
    let width = grid.iter().map(Vec::len).max().unwrap_or(0);
    for row in &mut grid {
        row.resize(width, Value::Null);
    }
    (grid, row_offset, col_offset)
}

/// Parse the first sheet's `merges` array into `MergeRect`s. Absent ⇒ empty.
fn parse_merges(body: &Value) -> Vec<MergeRect> {
    body.get("sheets")
        .and_then(|s| s.as_array())
        .and_then(|s| s.first())
        .and_then(|s| s.get("merges"))
        .and_then(|m| m.as_array())
        .map(|arr| {
            arr.iter()
                .map(|m| MergeRect {
                    start_row: m.get("startRowIndex").and_then(Value::as_u64).unwrap_or(0) as usize,
                    end_row: m.get("endRowIndex").and_then(Value::as_u64).unwrap_or(0) as usize,
                    start_col: m
                        .get("startColumnIndex")
                        .and_then(Value::as_u64)
                        .unwrap_or(0) as usize,
                    end_col: m.get("endColumnIndex").and_then(Value::as_u64).unwrap_or(0) as usize,
                })
                .collect()
        })
        .unwrap_or_default()
}
```

- [ ] **Step 2: Add unit tests for the parse helpers**

Inside the existing `#[cfg(test)] mod tests` block in `http_client.rs` (after `read_range_returns_2d_array_for_unformatted`), add:

```rust
    fn sample_grid_body() -> serde_json::Value {
        // Two rows: header "Cat"/"N", then a row where Cat is empty (a merge
        // gap) and N=5. A merges entry covers A1:A2 (the Cat column).
        serde_json::json!({
            "sheets": [{
                "data": [{
                    "startRow": 0,
                    "startColumn": 0,
                    "rowData": [
                        {"values": [
                            {"effectiveValue": {"stringValue": "Cat"}, "formattedValue": "Cat",
                             "userEnteredValue": {"stringValue": "Cat"}},
                            {"effectiveValue": {"stringValue": "N"}, "formattedValue": "N",
                             "userEnteredValue": {"stringValue": "N"}}
                        ]},
                        {"values": [
                            {},
                            {"effectiveValue": {"numberValue": 5.0}, "formattedValue": "5",
                             "userEnteredValue": {"numberValue": 5.0}}
                        ]}
                    ]
                }],
                "merges": [
                    {"startRowIndex": 0, "endRowIndex": 2, "startColumnIndex": 0, "endColumnIndex": 1}
                ]
            }]
        })
    }

    #[test]
    fn parse_grid_block_unformatted_maps_effective_value() {
        let body = sample_grid_body();
        let (grid, ro, co) = parse_grid_block(&body, ValueRenderOption::UnformattedValue);
        assert_eq!(ro, 0);
        assert_eq!(co, 0);
        assert_eq!(
            grid,
            vec![
                vec![serde_json::json!("Cat"), serde_json::json!("N")],
                vec![serde_json::json!(null), serde_json::json!(5.0)],
            ]
        );
    }

    #[test]
    fn parse_grid_block_formatted_uses_formatted_value() {
        let body = sample_grid_body();
        let (grid, _, _) = parse_grid_block(&body, ValueRenderOption::FormattedValue);
        // Number 5 surfaces as its formatted string "5".
        assert_eq!(grid[1][1], serde_json::json!("5"));
    }

    #[test]
    fn parse_grid_block_formula_prefers_formula_value() {
        let body = serde_json::json!({
            "sheets": [{
                "data": [{"startRow": 0, "startColumn": 0, "rowData": [
                    {"values": [
                        {"userEnteredValue": {"formulaValue": "=SUM(B:B)"},
                         "effectiveValue": {"numberValue": 7.0}, "formattedValue": "7"}
                    ]}
                ]}],
                "merges": []
            }]
        });
        let (grid, _, _) = parse_grid_block(&body, ValueRenderOption::Formula);
        assert_eq!(grid[0][0], serde_json::json!("=SUM(B:B)"));
    }

    #[test]
    fn parse_merges_reads_rectangles() {
        let body = sample_grid_body();
        let merges = parse_merges(&body);
        assert_eq!(merges.len(), 1);
        assert_eq!(merges[0], MergeRect { start_row: 0, end_row: 2, start_col: 0, end_col: 1 });
    }

    #[test]
    fn parse_grid_block_no_merges_is_plain_rectangle() {
        // Regression guard: a body with no merges parses to the same logical
        // 2-D values the old values.get path produced.
        let body = serde_json::json!({
            "sheets": [{
                "data": [{"startRow": 0, "startColumn": 0, "rowData": [
                    {"values": [
                        {"effectiveValue": {"stringValue": "x"}},
                        {"effectiveValue": {"numberValue": 1.0}}
                    ]},
                    {"values": [
                        {"effectiveValue": {"stringValue": "y"}},
                        {"effectiveValue": {"numberValue": 2.0}}
                    ]}
                ]}],
                "merges": []
            }]
        });
        let (grid, _, _) = parse_grid_block(&body, ValueRenderOption::UnformattedValue);
        assert_eq!(
            grid,
            vec![
                vec![serde_json::json!("x"), serde_json::json!(1.0)],
                vec![serde_json::json!("y"), serde_json::json!(2.0)],
            ]
        );
        assert!(parse_merges(&body).is_empty());
    }
```

- [ ] **Step 3: Run tests to verify they pass**

Run: `cargo test --lib -p colmena_dag_engine http_client`
Expected: the 5 new `parse_*` tests PASS. (The existing `read_range_returns_2d_array_for_unformatted` still uses the old mock and will be updated in Task 3 — it may still pass here since `read_range` is unchanged so far.)

- [ ] **Step 4: Lint + format**

Run: `cargo fmt && cargo clippy -p colmena_dag_engine 2>&1 | tail -5`
Expected: no warnings.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/gsheets/infrastructure/http_client.rs
git commit -m "feat(gsheets): grid-data parse helpers for spreadsheets.get (cell_scalar, parse_grid_block, parse_merges)"
```

---

## Task 3: Rewrite `read_range` to Approach B + wire the fill

**Files:**
- Modify: `src/libs/colmena/src/gsheets/infrastructure/http_client.rs` (the `read_range` impl at lines ~320-359, and the wiremock test at ~962-989)

- [ ] **Step 1: Update the existing wiremock test to the new request/response shape (make it fail first)**

Replace the body of `read_range_returns_2d_array_for_unformatted` (lines ~962-989) with a `spreadsheets.get` mock. The path is now `/abc` (sheet-level get) with `includeGridData`; assert the 2-D array result is unchanged:

```rust
    #[tokio::test]
    async fn read_range_returns_2d_array_for_unformatted() {
        let (server, client) = setup_mock().await;

        // Approach B: read goes through spreadsheets.get (path = /{id}), not
        // /values/{range}. Returns grid data + (here, empty) merges.
        Mock::given(method("GET"))
            .and(path_regex(r"/abc$"))
            .and(header("authorization", "Bearer fake-bearer-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "sheets": [{
                    "data": [{"startRow": 0, "startColumn": 0, "rowData": [
                        {"values": [
                            {"effectiveValue": {"stringValue": "x"}},
                            {"effectiveValue": {"numberValue": 1.0}}
                        ]},
                        {"values": [
                            {"effectiveValue": {"stringValue": "y"}},
                            {"effectiveValue": {"numberValue": 2.0}}
                        ]}
                    ]}],
                    "merges": []
                }]
            })))
            .mount(&server)
            .await;

        let resp = client
            .read_range(
                &SpreadsheetId("abc".into()),
                "Sheet1",
                Some("A1:B2"),
                ReadOptions::default(),
            )
            .await
            .expect("read ok");
        assert_eq!(resp.sheet, "Sheet1");
        assert_eq!(resp.range, "Sheet1!A1:B2");
        assert_eq!(resp.values, serde_json::json!([["x", 1.0], ["y", 2.0]]));
    }
```

- [ ] **Step 2: Add a new wiremock test that proves forward-fill end-to-end**

Add right after the test above:

```rust
    #[tokio::test]
    async fn read_range_forward_fills_merged_cells() {
        let (server, client) = setup_mock().await;

        // Column A (Cat) merged A1:A3 with anchor "Fruit"; rows 2-3 empty in A.
        Mock::given(method("GET"))
            .and(path_regex(r"/abc$"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "sheets": [{
                    "data": [{"startRow": 0, "startColumn": 0, "rowData": [
                        {"values": [
                            {"effectiveValue": {"stringValue": "Fruit"}},
                            {"effectiveValue": {"numberValue": 10.0}}
                        ]},
                        {"values": [
                            {},
                            {"effectiveValue": {"numberValue": 20.0}}
                        ]},
                        {"values": [
                            {},
                            {"effectiveValue": {"numberValue": 30.0}}
                        ]}
                    ]}],
                    "merges": [
                        {"startRowIndex": 0, "endRowIndex": 3, "startColumnIndex": 0, "endColumnIndex": 1}
                    ]
                }]
            })))
            .mount(&server)
            .await;

        let resp = client
            .read_range(&SpreadsheetId("abc".into()), "Sheet1", None, ReadOptions::default())
            .await
            .expect("read ok");
        // The anchor "Fruit" is propagated down the whole merge.
        assert_eq!(
            resp.values,
            serde_json::json!([["Fruit", 10.0], ["Fruit", 20.0], ["Fruit", 30.0]])
        );
    }
```

- [ ] **Step 3: Run both tests to verify they FAIL**

Run: `cargo test --lib -p colmena_dag_engine read_range_returns_2d_array_for_unformatted read_range_forward_fills_merged_cells`
Expected: FAIL — the old `read_range` still hits `/values/...`, so the new `/abc` mocks don't match and the calls error / return wrong shape.

- [ ] **Step 4: Rewrite `read_range` (Approach B)**

Replace the `read_range` method body (lines ~320-359) with:

```rust
    async fn read_range(
        &self,
        id: &SpreadsheetId,
        sheet: &str,
        range: Option<&str>,
        opts: ReadOptions,
    ) -> Result<ReadResponse, SheetsError> {
        let quoted_sheet = quote_sheet_for_range(sheet);
        let full_range = match range {
            Some(r) => format!("{quoted_sheet}!{r}"),
            None => quoted_sheet,
        };
        // Approach B: one spreadsheets.get with includeGridData brings the cell
        // values AND the merge rectangles together — so we forward-fill merged
        // cells without a second round-trip and without a stale cache. See
        // docs/superpowers/specs/2026-06-14-gsheets-expand-merges-design.md.
        let fields = "sheets(data(startRow,startColumn,rowData(values(\
            formattedValue,effectiveValue,userEnteredValue))),merges)";
        let url = format!(
            "{}/{}?ranges={}&includeGridData=true&fields={}",
            self.sheets_base,
            id.0,
            urlencoding::encode(&full_range),
            urlencoding::encode(fields),
        );
        let body = self.get_json(&url).await?;

        let (mut grid, row_offset, col_offset) = parse_grid_block(&body, opts.value_render);
        let merges = parse_merges(&body);
        forward_fill_merges(&mut grid, &merges, row_offset, col_offset);

        // Re-shape into the [[...]] rectangle the downstream pipeline expects.
        let raw_values = Value::Array(grid.into_iter().map(Value::Array).collect());
        let values = if opts.as_records {
            rectangle_to_records(&raw_values)
        } else {
            raw_values
        };
        Ok(ReadResponse {
            sheet: sheet.to_string(),
            // Echo the requested range. For an explicit range this matches the
            // old behavior (e.g. "Sheet1!A1:B2"); for a whole-sheet read it is
            // the sheet name — the data extent is still surfaced via the
            // caller's `dimensions` (computed from `values`).
            range: full_range,
            values,
        })
    }
```

- [ ] **Step 5: Run the tests to verify they PASS**

Run: `cargo test --lib -p colmena_dag_engine read_range`
Expected: both `read_range_returns_2d_array_for_unformatted` and `read_range_forward_fills_merged_cells` PASS, plus the existing error-path read tests (404/429/401) — those mock `get_json` failures and are render-path-agnostic, so they still pass.

- [ ] **Step 6: Full module test + lint**

Run: `cargo test --lib -p colmena_dag_engine gsheets && cargo fmt && cargo clippy -p colmena_dag_engine 2>&1 | tail -5`
Expected: all gsheets tests pass, no warnings.

- [ ] **Step 7: Commit**

```bash
git add src/libs/colmena/src/gsheets/infrastructure/http_client.rs
git commit -m "feat(gsheets): read_range uses spreadsheets.get + forward-fills merged cells (Approach B)"
```

---

## Task 4: LLM-facing tool descriptions

Tell the model that merged cells are auto-expanded so it doesn't misread repeated values as duplicates.

**Files:**
- Modify: `src/libs/colmena/text/tools/gsheets.yaml`

- [ ] **Step 1: Update `gsheets_read` description**

In `gsheets.yaml`, inside the `gsheets_read:` entry's `description: |` block, append this sentence as a new paragraph at the end of the block (keep existing indentation):

```yaml
    MERGED CELLS: merged cells are auto-expanded on read — every cell of a merge
    returns the merge's value (the top-left anchor), not a blank. Repeated values
    down a column or across a row may therefore reflect a merged region, not
    duplicated data.
```

- [ ] **Step 2: Update `gsheets_run_python` description**

In the `gsheets_run_python:` entry's `description: |` block, append:

```yaml
    MERGED CELLS: sheets are loaded with merged cells already forward-filled, so
    a column that was visually merged (e.g. a category spanning several rows)
    arrives fully populated — groupby/join/compare work directly, with no NaN
    gaps to fill yourself.
```

- [ ] **Step 3: Verify the YAML still parses (build + any text-registry test)**

Run: `cargo build -p colmena_dag_engine 2>&1 | tail -5`
Expected: builds clean (the text registry is loaded via `include_str!`; a malformed YAML would surface in tests that parse it).

Run: `cargo test --lib -p colmena_dag_engine gsheets 2>&1 | tail -5`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/text/tools/gsheets.yaml
git commit -m "docs(gsheets): tool descriptions note auto-expansion of merged cells"
```

---

## Task 5: Meticulous E2E verification against real Google Sheets

**This task is mandatory and cannot be skipped or mocked.** Per repo rule, the feature is not "done" until verified end-to-end against the live Google Sheets API. The point of this feature is correctness on real merged sheets — unit tests with hand-written JSON cannot prove the real `spreadsheets.get` payload shape matches our parser. Both read surfaces must be exercised.

**Files:**
- Create: `tests/graphs/agents/gsheets_expand_merges_read.json`
- Create: `tests/graphs/agents/gsheets_expand_merges_python.json`

**Prerequisites (from the local gsheets E2E runbook in memory):**
- OAuth creds come from GCP Secret Manager (`startti-dev`), injected in-memory only — never written to disk, committed, or printed. Required env vars: `COLMENA_GOOGLE_OAUTH_CLIENT_ID`, `COLMENA_GOOGLE_OAUTH_CLIENT_SECRET`, `COLMENA_GOOGLE_OAUTH_REFRESH_TOKEN`, `COLMENA_GOOGLE_SHARE_EMAIL`.
- `gcloud auth` may need re-auth; pandas in `gsheets_run_python` needs the `PYTHONPATH=python3.14` workaround per the runbook.
- `GEMINI_API_KEY` from repo `.env` for the LLM.

- [ ] **Step 1: Create a real test spreadsheet with merges (one-time, manual or scripted)**

Create (or reuse) a Google Sheet shared with the agent account (`COLMENA_GOOGLE_SHARE_EMAIL`, as Editor) named e.g. `colmena_expand_merges_e2e`, with a tab `Ventas` shaped like:

| Categoria (A) | Producto (B) | Monto (C) |
|---|---|---|
| **Frutas** (merged A2:A4) | Manzana | 100 |
| | Banana | 200 |
| | Pera | 50 |
| **Verduras** (merged A5:A6) | Lechuga | 30 |
| | Tomate | 70 |

Row 1 = headers. Column A has two vertical merges (`A2:A4` = "Frutas", `A5:A6` = "Verduras"). Record the spreadsheet ID.

> If creating by hand is impractical, the implementer may script the setup with a throwaway `gsheets_run_python`/`set_range` + a `batchUpdate` mergeCells call, but the canonical artifact is a real sheet with real merges. Document the spreadsheet ID used in the run report.

- [ ] **Step 2: Create the `gsheets_read` E2E graph**

Create `tests/graphs/agents/gsheets_expand_merges_read.json`. Replace `<SPREADSHEET_ID>` with the real ID from Step 1. Pattern matches `tests/graphs/agents/gsheets_markdown_read.json` — the spreadsheet ID goes **verbatim in the prompt** (not in a `context` block), and the toolkit is flag-activated:

```json
{
  "comment": "E2E: read a real sheet with vertical merges in column A via gsheets_read. The agent must report the Categoria for the 'Pera' row. Pre-feature, A4 would be blank (merge gap) and the agent could not answer; with expand-merges the read returns 'Frutas' in every row of the merge, so the agent answers 'Frutas'.",
  "nodes": {
    "trigger": {
      "type": "trigger_webhook",
      "config": { "path": "/merges_read", "method": "POST", "test_payload": {
        "prompt": "Use this exact spreadsheet_id (copy character-by-character): <SPREADSHEET_ID>\nEn la hoja 'Ventas' de esa planilla, leé la tabla completa con gsheets_read (omití range) y decime exactamente a qué Categoria pertenece el producto 'Pera'. Respondé solo con el nombre de la categoria."
      }}
    },
    "agent": {
      "type": "llm_call",
      "config": {
        "provider": "google",
        "api_key": "${GEMINI_API_KEY}",
        "model": "gemini-2.5-flash",
        "temperature": 0,
        "system_message": "Sos un asistente que lee Google Sheets con las herramientas gsheets. Copiá el spreadsheet_id verbatim. No inventes datos; respondé en base a lo que leés.",
        "enabled_tools": ["gsheets"]
      }
    },
    "log": { "type": "log" }
  },
  "edges": [ { "from": "trigger", "to": "agent" }, { "from": "agent", "to": "log" } ]
}
```

> `enabled_tools: ["gsheets"]` activates the toolkit by flag (creds from process env).

- [ ] **Step 3: Run the read E2E and verify**

```bash
set -a; source .env; set +a
# inject OAuth creds from Secret Manager in-memory per the runbook (do not print values)
mkdir -p /tmp/colmena_e2e
cargo run --bin dag_engine -- run tests/graphs/agents/gsheets_expand_merges_read.json \
  --agent-session-id agent_merges_read_001 2>&1 | tee /tmp/colmena_e2e/gsheets_expand_merges_read.sse
```

Expected: the agent's final answer is **"Frutas"**. Verify in the SSE that the `gsheets_read` tool result shows column A populated as `Frutas` on the Pera row (not blank). If the answer is blank/wrong, the fill is not reaching markdown — debug before proceeding.

- [ ] **Step 4: Create the `gsheets_run_python` E2E graph**

Create `tests/graphs/agents/gsheets_expand_merges_python.json`. This proves the shared-layer claim — the pandas path inherits the fill with zero code changes. Same prompt-verbatim-ID pattern. Replace `<SPREADSHEET_ID>`:

```json
{
  "comment": "E2E: gsheets_run_python loads 'Ventas' into a DataFrame and groups by Categoria summing Monto. Pre-feature, the merged 'Frutas'/'Verduras' cells arrive as NaN for rows 2-3 of each group, so a groupby('Categoria') drops those rows and the totals are wrong. With expand-merges the column is fully populated and the totals are Frutas=350, Verduras=100. The agent must report both totals.",
  "nodes": {
    "trigger": {
      "type": "trigger_webhook",
      "config": { "path": "/merges_python", "method": "POST", "test_payload": {
        "prompt": "Use this exact spreadsheet_id (copy character-by-character): <SPREADSHEET_ID>\nEn la hoja 'Ventas' de esa planilla, sumá el Monto por Categoria usando código. Llamá gsheets_run_python con un binding SHEET {var:'v', spreadsheet_id:'<SPREADSHEET_ID>', sheet:'Ventas'} y código: import pandas as pd; df=pd.DataFrame(v); output=df.groupby('Categoria')['Monto'].sum().to_dict(). Decime el total de cada categoria."
      }}
    },
    "agent": {
      "type": "llm_call",
      "config": {
        "provider": "google",
        "api_key": "${GEMINI_API_KEY}",
        "model": "gemini-2.5-flash",
        "temperature": 0,
        "system_message": "Sos un analista de datos. Copiá el spreadsheet_id verbatim. Para procesar la hoja 'Ventas' usá gsheets_run_python: cargá la hoja como DataFrame, agrupá por 'Categoria' y sumá 'Monto'. Respondé con el total de cada categoria.",
        "enabled_tools": ["gsheets"]
      }
    },
    "log": { "type": "log" }
  },
  "edges": [ { "from": "trigger", "to": "agent" }, { "from": "agent", "to": "log" } ]
}
```

- [ ] **Step 5: Run the python E2E and verify the totals**

```bash
cargo run --bin dag_engine -- run tests/graphs/agents/gsheets_expand_merges_python.json \
  --agent-session-id agent_merges_python_001 2>&1 | tee /tmp/colmena_e2e/gsheets_expand_merges_python.sse
```

Expected: the agent reports **Frutas = 350** (100+200+50) and **Verduras = 100** (30+70). These totals are only correct if the merged Categoria column was forward-filled before pandas saw it. If a category total is missing rows (e.g. Frutas=100), the fill is NOT reaching the pandas path — STOP and debug; the shared-layer assumption is broken.

- [ ] **Step 6: Present the friendly E2E report**

Per repo rule (don't paste full SSE): for each of the two runs, present input prompt, the key tool payload (the relevant slice of the gsheets tool result), token usage, and the final answer. Confirm both expected results (Frutas/Verduras read = "Frutas"; totals 350/100).

- [ ] **Step 7: Commit the graphs**

```bash
git add tests/graphs/agents/gsheets_expand_merges_read.json tests/graphs/agents/gsheets_expand_merges_python.json
git commit -m "test(gsheets): E2E graphs for expand-merges (read + pandas groupby)"
```

---

## Final verification (after all tasks)

- [ ] **Full CI-equivalent test run:** `cargo test --verbose 2>&1 | tail -30` — unit + integration + doctests green (not just `--lib`).
- [ ] **Lint clean:** `cargo clippy -p colmena_dag_engine 2>&1 | tail -5` — no warnings.
- [ ] **ADP sweep:** confirm no ADP code references break — `read_range`/`ReadOptions`/`ReadResponse` signatures are unchanged (internal-only change), so the worker is unaffected. Note in the PR.
- [ ] **Update docs/CHANGELOG_2026-06.md** with a new section summarizing expand-merges (Approach B, always-on, B1 sub-range limitation, both surfaces).
- [ ] Hand off to `superpowers:finishing-a-development-branch` — push + PR only after both E2E runs above pass (per repo rule: push/PR conditional on verified E2E success).

---

## Self-Review notes (author)

- **Spec coverage:** §3 architecture → Tasks 1-3; §4 endpoint+mapping → Tasks 2-3; §5 forward_fill → Task 1; §6 regression risk → `parse_grid_block_no_merges_is_plain_rectangle` + the unchanged-output assertion in Task 3 Step 1; §7 ADP → final sweep; §8 testing → Tasks 1-2 (unit), Task 5 (E2E both surfaces). All covered.
- **B1** is exercised by `anchor_outside_grid_is_skipped_b1` (Task 1).
- **Type consistency:** `MergeRect` fields and `forward_fill_merges` signature are identical across Tasks 1, 2, 3. The grid is `Vec<Vec<serde_json::Value>>` throughout (no `CellValue` round-trip), eliminating the type-shift regression risk.
- **Known minor behavior change:** `ReadResponse.range` for whole-sheet reads is now the sheet name rather than Google's A1 extent; extent is preserved via the caller's `dimensions`. Documented in Task 3 Step 4.
