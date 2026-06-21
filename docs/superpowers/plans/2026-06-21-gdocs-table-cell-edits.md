# Surgical Table-Cell Edits Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the LLM 6 tools to discover and surgically edit Google Docs tables — read tables, set a cell's plain text, and insert/delete rows and columns — over docs the user shared (Docs API only).

**Architecture:** Tables are currently dropped during snapshot parsing. The core change is modeling tables in `DocumentSnapshot` (`parse_tables`), then a new `application/table.rs` that locates a table by 0-based index, builds Docs API `batchUpdate` requests, and finalizes through the existing co-edit/revision machinery. A new non-blocking guard variant treats human edits as soft-warnings. Six synthetic tools wire it to the LLM. All additive — no `DocsClient` trait change, no ADP impact.

**Tech Stack:** Rust (`colmena_dag_engine` crate), hexagonal layers (domain/application/infrastructure), `mockall` for `MockDocsClient`, `serde_json` for request shapes, Google Docs `documents.batchUpdate`.

---

## File Structure

- `src/libs/colmena/src/gdocs/domain/types.rs` — add `TableSnapshot`, `CellSnapshot`; add `tables` field to `TabSnapshot`.
- `src/libs/colmena/src/gdocs/infrastructure/http_client.rs` — add `parse_tables`, wire into `parse_snapshot`; fix the 1 internal `TabSnapshot {…}` constructor.
- `src/libs/colmena/src/gdocs/application/co_edit_guard.rs` — add `run_guard_non_blocking`.
- `src/libs/colmena/src/gdocs/application/insert.rs` — make `apply_and_finalize` `pub(crate)`.
- `src/libs/colmena/src/gdocs/application/table.rs` — **new**: `run_read_tables`, `run_set_table_cell`, `run_insert_table_row`, `run_delete_table_row`, `run_insert_table_column`, `run_delete_table_column`, pure request builders, `TableListing` types.
- `src/libs/colmena/src/gdocs/application/mod.rs` — `pub mod table;`.
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gdocs_tools.rs` — 6 `TOOL_*` consts, 6 Args structs, 6 `tool_*()` defs, 6 `dispatch_*()` fns, add to `build_all_gdocs_tools`.
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs` — re-export the 6 dispatchers + `TOOL_*` consts.
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/toolkit_packages.rs` — add 6 tools to `gdocs`, add `gdocs_read_tables` to `gdocsread`; update counts + sync tests.
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs` — extend `all_gdocs` (29→35) and `gdocs_entries` (29→35) arrays + tool-builder imports.
- `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs` — router: imports, `is_gdocs_tool` arm, 6 match arms.
- `src/libs/colmena/text/tools/gdocs.yaml` — 6 LLM-facing descriptions.
- `src/libs/colmena/tests/gdocs_integration_test.rs` — `#[ignore]` live round-trip.
- `tests/graphs/agents/gdocs_table_edits_e2e.json` — **new** E2E graph.
- `docs/developer_guide/45_gdocs.md`, `docs/CHANGELOG_2026-06.md`, `docs/BACKLOG.md` — docs.

**Two cross-file count contracts (CI-enforced):** `build_all_gdocs_tools().len()` == `all_gdocs.len()` == `gdocs_entries.len()` == `gdocs` toolkit `tools.len()`. All move 29 → **35**. The `gdocsread` toolkit moves 9 → **10**.

---

## Task 1: Domain types — `TableSnapshot`, `CellSnapshot`, `TabSnapshot.tables`

**Files:**
- Modify: `src/libs/colmena/src/gdocs/domain/types.rs` (after `ParagraphSnapshot`, ~line 466; and `TabSnapshot` ~line 451)
- Modify (constructors): `src/libs/colmena/src/gdocs/infrastructure/http_client.rs:237,242`
- Test: inline `#[cfg(test)]` in `types.rs`

- [ ] **Step 1: Add the two new structs** after `ParagraphSnapshot` in `types.rs`:

```rust
/// A table parsed from the document body, with its cells and the UTF-16
/// indices the Docs API uses to address content. Populated by
/// `parse_tables`; consumed by the `gdocs_*_table_*` use cases.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TableSnapshot {
    /// 0-based order of appearance of this table within its tab.
    pub table_index: u32,
    /// Tab the table lives in. `None` for single-tab legacy docs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab_id: Option<TabId>,
    /// `startIndex` of the `table` structural element — what the Docs API
    /// wants as `tableStartLocation.index` for insert/deleteTableRow/Column.
    pub start_index: u32,
    pub rows: u32,
    pub columns: u32,
    /// Row-major grid of cells. A cell merged "away" by a neighbour's span
    /// is absent; `row_span`/`col_span` on the master signal the merge.
    pub cells: Vec<Vec<CellSnapshot>>,
}

/// A single table cell with its plain text and content range.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CellSnapshot {
    pub row: u32,
    pub col: u32,
    /// Plain text of the cell's paragraphs (trailing `\n` trimmed).
    pub text: String,
    /// startIndex of the cell's first paragraph (insertion point).
    pub content_start_index: u32,
    /// endIndex of the cell's last paragraph.
    pub content_end_index: u32,
    /// >1 when this cell is the master of a merge; 1 otherwise.
    pub row_span: u32,
    pub col_span: u32,
}
```

- [ ] **Step 2: Add the `tables` field to `TabSnapshot`**:

```rust
pub struct TabSnapshot {
    pub tab_id: Option<TabId>,
    pub paragraphs: Vec<ParagraphSnapshot>,
    /// Tables in this tab, in order. Additive (`#[serde(default)]`) so
    /// every existing consumer ignores it without change.
    #[serde(default)]
    pub tables: Vec<TableSnapshot>,
}
```

- [ ] **Step 3: Fix the 2 production constructors** in `http_client.rs`. At line ~237:

```rust
            let paragraphs = parse_paragraphs(&body_arr, &mut para_counter);
            let mut table_counter: u32 = 0;
            let tables = parse_tables(&body_arr, &tab_id, &mut table_counter);
            tabs.push(TabSnapshot { tab_id, paragraphs, tables });
```

At line ~242 (legacy single-tab):

```rust
        let paragraphs = parse_paragraphs(body, &mut para_counter);
        let mut table_counter: u32 = 0;
        let tables = parse_tables(body, &None, &mut table_counter);
        tabs.push(TabSnapshot {
            tab_id: None,
            paragraphs,
            tables,
        });
```

(`parse_tables` is implemented in Task 2; this step will not compile until then — that's expected, the two tasks land together. If you need an intermediate compile, temporarily use `tables: Vec::new()` and replace in Task 2.)

- [ ] **Step 4: Add a serde round-trip test** in the `#[cfg(test)] mod tests` of `types.rs`:

```rust
#[test]
fn table_snapshot_round_trips_and_tab_default() {
    // tables field defaults to empty when absent (backward compat).
    let legacy = serde_json::json!({
        "tab_id": null, "paragraphs": []
    });
    let tab: TabSnapshot = serde_json::from_value(legacy).unwrap();
    assert!(tab.tables.is_empty());

    let t = TableSnapshot {
        table_index: 0, tab_id: None, start_index: 5,
        rows: 1, columns: 2,
        cells: vec![vec![
            CellSnapshot { row: 0, col: 0, text: "a".into(),
                content_start_index: 6, content_end_index: 8, row_span: 1, col_span: 1 },
            CellSnapshot { row: 0, col: 1, text: "b".into(),
                content_start_index: 8, content_end_index: 10, row_span: 1, col_span: 1 },
        ]],
    };
    let j = serde_json::to_value(&t).unwrap();
    let back: TableSnapshot = serde_json::from_value(j).unwrap();
    assert_eq!(t, back);
}
```

- [ ] **Step 5: Update the other 6 `TabSnapshot {…}` test-helper sites** so the crate compiles. In each of these, add `tables: vec![]` to the struct literal (they are all in `#[cfg(test)]` or test-helper code):
  - `src/libs/colmena/src/gdocs/application/_test_helpers.rs:47,75`
  - `src/libs/colmena/src/gdocs/application/scope_resolver.rs:226`
  - `src/libs/colmena/src/gdocs/application/diff.rs:210,302,306,317,321`
  - `src/libs/colmena/src/gdocs/infrastructure/outline_cache.rs:84`
  - `src/libs/colmena/src/gdocs/infrastructure/revision_store.rs:288`

Run `cargo build --lib 2>&1 | grep "missing field \`tables\`"` to find any you missed.

- [ ] **Step 6: Commit** (after Task 2 compiles — or commit Tasks 1+2 together):

```bash
git add src/libs/colmena/src/gdocs/domain/types.rs
git commit -m "feat(gdocs): model tables in DocumentSnapshot (TableSnapshot/CellSnapshot)"
```

---

## Task 2: Infra — `parse_tables`

**Files:**
- Modify: `src/libs/colmena/src/gdocs/infrastructure/http_client.rs` (add `parse_tables` next to `parse_paragraphs`, ~line 276)
- Test: inline `#[cfg(test)]` in `http_client.rs`

- [ ] **Step 1: Write the failing test** in the `#[cfg(test)] mod tests` of `http_client.rs`:

```rust
#[test]
fn parse_tables_single_table_two_cells() {
    let content = serde_json::json!([
        { "table": {
            "rows": 1, "columns": 2,
            "tableRows": [
                { "tableCells": [
                    { "startIndex": 6, "endIndex": 8,
                      "content": [ { "startIndex": 6, "endIndex": 8,
                        "paragraph": { "elements": [ { "textRun": { "content": "a\n" } } ] } } ] },
                    { "startIndex": 8, "endIndex": 10,
                      "content": [ { "startIndex": 8, "endIndex": 10,
                        "paragraph": { "elements": [ { "textRun": { "content": "b\n" } } ] } } ] }
                ] }
            ]
        }, "startIndex": 5 }
    ]);
    let arr = content.as_array().unwrap();
    let mut c = 0u32;
    let tables = parse_tables(arr, &None, &mut c);
    assert_eq!(tables.len(), 1);
    let t = &tables[0];
    assert_eq!((t.table_index, t.start_index, t.rows, t.columns), (0, 5, 1, 2));
    assert_eq!(t.cells[0][0].text, "a");
    assert_eq!(t.cells[0][0].content_start_index, 6);
    assert_eq!(t.cells[0][0].content_end_index, 8);
    assert_eq!(t.cells[0][1].col, 1);
    assert_eq!(t.cells[0][1].text, "b");
}

#[test]
fn parse_tables_reports_merge_spans_and_empty_cell() {
    let content = serde_json::json!([
        { "table": {
            "rows": 1, "columns": 1,
            "tableRows": [
                { "tableCells": [
                    { "startIndex": 2, "endIndex": 3,
                      "tableCellStyle": { "rowSpan": 2, "columnSpan": 3 },
                      "content": [ { "startIndex": 2, "endIndex": 3,
                        "paragraph": { "elements": [ { "textRun": { "content": "\n" } } ] } } ] }
                ] }
            ]
        }, "startIndex": 1 }
    ]);
    let arr = content.as_array().unwrap();
    let mut c = 0u32;
    let t = &parse_tables(arr, &None, &mut c)[0];
    assert_eq!(t.cells[0][0].text, ""); // empty cell → trimmed to ""
    assert_eq!(t.cells[0][0].row_span, 2);
    assert_eq!(t.cells[0][0].col_span, 3);
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --lib parse_tables 2>&1 | tail -20`
Expected: FAIL — `cannot find function parse_tables`.

- [ ] **Step 3: Implement `parse_tables`** right after `parse_paragraphs` (reuse the same text-extraction approach as `paragraph_text_and_range`):

```rust
/// Parse `table` structural elements out of a body `content[]` array.
/// `table_counter` yields the 0-based `table_index` (reset per tab by the
/// caller, mirroring how the agent addresses tables). Indices come straight
/// from the Docs API (UTF-16) — no manual math.
fn parse_tables(
    content: &[serde_json::Value],
    tab_id: &Option<TabId>,
    table_counter: &mut u32,
) -> Vec<TableSnapshot> {
    let mut out = Vec::new();
    for elem in content {
        let Some(table) = elem.get("table") else { continue };
        let start_index = elem.get("startIndex").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
        let rows = table.get("rows").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
        let columns = table.get("columns").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
        let mut cells: Vec<Vec<CellSnapshot>> = Vec::new();
        if let Some(table_rows) = table.get("tableRows").and_then(|v| v.as_array()) {
            for (r, row) in table_rows.iter().enumerate() {
                let mut row_cells = Vec::new();
                if let Some(tcs) = row.get("tableCells").and_then(|v| v.as_array()) {
                    for (c, cell) in tcs.iter().enumerate() {
                        row_cells.push(parse_cell(cell, r as u32, c as u32));
                    }
                }
                cells.push(row_cells);
            }
        }
        out.push(TableSnapshot {
            table_index: *table_counter,
            tab_id: tab_id.clone(),
            start_index,
            rows,
            columns,
            cells,
        });
        *table_counter += 1;
    }
    out
}

fn parse_cell(cell: &serde_json::Value, row: u32, col: u32) -> CellSnapshot {
    // Concatenate text across the cell's content paragraphs; capture the
    // first paragraph's startIndex and the last paragraph's endIndex.
    let mut text = String::new();
    let mut content_start_index = cell.get("startIndex").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    let mut content_end_index = cell.get("endIndex").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    if let Some(paras) = cell.get("content").and_then(|v| v.as_array()) {
        if let Some(first) = paras.first().and_then(|p| p.get("startIndex")).and_then(|v| v.as_u64()) {
            content_start_index = first as u32;
        }
        if let Some(last) = paras.last().and_then(|p| p.get("endIndex")).and_then(|v| v.as_u64()) {
            content_end_index = last as u32;
        }
        for p in paras {
            if let Some(elems) = p.pointer("/paragraph/elements").and_then(|v| v.as_array()) {
                for e in elems {
                    if let Some(t) = e.pointer("/textRun/content").and_then(|v| v.as_str()) {
                        text.push_str(t);
                    }
                }
            }
        }
    }
    let row_span = cell.pointer("/tableCellStyle/rowSpan").and_then(|v| v.as_u64()).unwrap_or(1) as u32;
    let col_span = cell.pointer("/tableCellStyle/columnSpan").and_then(|v| v.as_u64()).unwrap_or(1) as u32;
    CellSnapshot {
        row,
        col,
        text: text.trim_end_matches('\n').to_string(),
        content_start_index,
        content_end_index,
        row_span: row_span.max(1),
        col_span: col_span.max(1),
    }
}
```

Add `TableSnapshot, CellSnapshot` to the `use crate::gdocs::domain::{…}` import block at the top of `http_client.rs`.

- [ ] **Step 4: Run to verify pass**

Run: `cargo test --lib parse_tables 2>&1 | tail -20`
Expected: PASS (2 tests). Also `cargo build --lib` now compiles Task 1's constructors.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/gdocs/infrastructure/http_client.rs \
        src/libs/colmena/src/gdocs/application src/libs/colmena/src/gdocs/infrastructure
git commit -m "feat(gdocs): parse tables from documents.get into TabSnapshot.tables"
```

---

## Task 3: Application — non-blocking guard + `apply_and_finalize` visibility

**Files:**
- Modify: `src/libs/colmena/src/gdocs/application/co_edit_guard.rs` (add fn)
- Modify: `src/libs/colmena/src/gdocs/application/insert.rs:455` (`async fn` → `pub(crate) async fn`)
- Test: inline `#[cfg(test)]` in `co_edit_guard.rs`

- [ ] **Step 1: Make `apply_and_finalize` reusable.** In `insert.rs:455` change:

```rust
pub(crate) async fn apply_and_finalize(
```

- [ ] **Step 2: Write the failing test** in `co_edit_guard.rs` tests (model it on the existing guard tests in that file — reuse their `MockDocsClient` / store setup helpers). The behavior to assert: on revision drift with a prior snapshot, `run_guard_non_blocking` returns the detected human changes as `soft_warnings` and never errors.

```rust
#[tokio::test]
async fn non_blocking_guard_returns_human_changes_as_soft_warnings() {
    // Reuse the same fixtures the blocking-guard tests build. A doc whose
    // paragraph 2 changed under the agent's feet must NOT error here.
    let ctx_fixture = build_drifted_fixture_with_paragraph_change(); // existing test helper pattern
    let ok = run_guard_non_blocking(&ctx_fixture.ctx(), &ctx_fixture.doc_id)
        .await
        .expect("non-blocking guard never errors on drift");
    assert!(!ok.soft_warnings.is_empty(), "human change surfaced as soft-warning");
}
```

(If no shared drift fixture exists, build it inline the same way the nearest existing `run_guard` drift test in this file does — same `MockDocsClient` expectations + `RevisionStore` seeded with a prior snapshot whose paragraph 2 text differs.)

- [ ] **Step 3: Run to verify failure**

Run: `cargo test --lib non_blocking_guard 2>&1 | tail -20`
Expected: FAIL — `cannot find function run_guard_non_blocking`.

- [ ] **Step 4: Implement `run_guard_non_blocking`** in `co_edit_guard.rs` (mirrors `run_guard` but never blocks — all detected changes become `soft_warnings`; no scope resolution needed):

```rust
/// Like [`run_guard`] but for edits whose scope can't be expressed as a
/// paragraph range (table cells). Detects human paragraph changes since the
/// agent's cursor and returns them ALL as `soft_warnings` — it never blocks
/// with `HumanChangesPending`. The `resolved_scope` is a doc-wide placeholder.
pub async fn run_guard_non_blocking(
    ctx: &GuardContext<'_>,
    doc_id: &DocumentId,
) -> Result<GuardOk, DocsError> {
    let snapshot = match ctx.cache.get_fresh(ctx.session_id, doc_id) {
        Some(s) => s,
        None => {
            let s = ctx.client.get(doc_id).await?;
            ctx.cache.put(ctx.session_id, doc_id, s.clone());
            s
        }
    };
    let resolved_scope = scope_resolver::resolve(&Scope::All, &snapshot)?;
    let (known, prior_snap) = ctx
        .revisions
        .get_with_snapshot(ctx.session_id, doc_id)
        .await?;
    let soft_warnings = match known {
        Some(k) if k != snapshot.revision_id => prior_snap
            .map(|prior| paragraph_diff(&prior, &snapshot))
            .unwrap_or_default(),
        _ => vec![],
    };
    Ok(GuardOk {
        snapshot,
        resolved_scope,
        soft_warnings,
    })
}
```

- [ ] **Step 5: Run to verify pass**

Run: `cargo test --lib non_blocking_guard 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src/libs/colmena/src/gdocs/application/co_edit_guard.rs src/libs/colmena/src/gdocs/application/insert.rs
git commit -m "feat(gdocs): non-blocking co-edit guard for table edits"
```

---

## Task 4: Application — `table.rs` scaffold + `run_read_tables` + request-builder helpers

**Files:**
- Create: `src/libs/colmena/src/gdocs/application/table.rs`
- Modify: `src/libs/colmena/src/gdocs/application/mod.rs` (add `pub mod table;`)
- Test: inline `#[cfg(test)]` in `table.rs`

- [ ] **Step 1: Register the module.** In `application/mod.rs` add (alphabetical with the others):

```rust
pub mod table;
```

- [ ] **Step 2: Write the file header, imports, listing types, and pure helpers** in `table.rs`:

```rust
//! Surgical table editing use cases: read tables, set a cell's plain text,
//! and insert/delete rows and columns. Addresses tables by 0-based
//! `table_index` (order within a tab). Plain text only — rich/markdown cell
//! content is deferred to v1.1. See spec
//! `docs/superpowers/specs/2026-06-21-gdocs-table-cell-edits-design.md`.

use crate::gdocs::application::co_edit_guard::{run_guard_non_blocking, GuardContext};
use crate::gdocs::application::insert::apply_and_finalize;
use crate::gdocs::domain::{
    CellSnapshot, ChangeKind, DocsClient, DocsError, DocumentId, EditResult, TabId, TableSnapshot,
};
use serde::Serialize;

/// `read_tables` return shape — one entry per table with per-cell previews.
#[derive(Debug, Clone, Serialize)]
pub struct TableListing {
    pub tables: Vec<TableInfo>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TableInfo {
    pub table_index: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tab_id: Option<String>,
    pub rows: u32,
    pub columns: u32,
    pub cells: Vec<TableCellInfo>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TableCellInfo {
    pub row: u32,
    pub col: u32,
    pub text_preview: String,
    pub row_span: u32,
    pub col_span: u32,
}

/// Build a `Location` JSON, attaching `tabId` only for multi-tab docs.
fn location(index: u32, tab_id: Option<&TabId>) -> serde_json::Value {
    let mut loc = serde_json::json!({ "index": index });
    if let Some(t) = tab_id {
        loc["tabId"] = serde_json::json!(t.0);
    }
    loc
}

/// Build a `tableCellLocation` JSON.
fn cell_location(
    table_start: u32,
    row: u32,
    col: u32,
    tab_id: Option<&TabId>,
) -> serde_json::Value {
    serde_json::json!({
        "tableStartLocation": location(table_start, tab_id),
        "rowIndex": row,
        "columnIndex": col,
    })
}

/// Find a table by index within the requested tab (default: first tab).
fn find_table<'a>(
    snap: &'a crate::gdocs::domain::DocumentSnapshot,
    table_index: u32,
    tab_id: Option<&TabId>,
) -> Result<&'a TableSnapshot, DocsError> {
    let tab = match tab_id {
        Some(t) => snap
            .tabs
            .iter()
            .find(|tb| tb.tab_id.as_ref() == Some(t))
            .ok_or_else(|| DocsError::TabNotFound(t.0.clone()))?,
        None => snap
            .tabs
            .first()
            .ok_or_else(|| DocsError::InvalidArgs("document has no tabs".into()))?,
    };
    tab.tables
        .iter()
        .find(|t| t.table_index == table_index)
        .ok_or_else(|| {
            DocsError::InvalidArgs(format!(
                "table_index {} out of range — this tab has {} table(s)",
                table_index,
                tab.tables.len()
            ))
        })
}

/// Locate a cell in a table, erroring clearly when out of range or covered
/// by a merge (slave positions are absent from the parsed grid).
fn find_cell<'a>(t: &'a TableSnapshot, row: u32, col: u32) -> Result<&'a CellSnapshot, DocsError> {
    t.cells
        .get(row as usize)
        .and_then(|r| r.iter().find(|c| c.col == col))
        .ok_or_else(|| {
            DocsError::InvalidArgs(format!(
                "cell ({},{}) not addressable in table {} ({}×{}) — out of range or covered by a merged cell",
                row, col, t.table_index, t.rows, t.columns
            ))
        })
}
```

- [ ] **Step 3: Write the failing test** for `run_read_tables` + `find_table`/`find_cell` (uses `MockDocsClient`):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::gdocs::domain::{DocumentSnapshot, MockDocsClient, ParagraphSnapshot, RevisionId, TabSnapshot};

    fn snap_with_one_table() -> DocumentSnapshot {
        DocumentSnapshot {
            doc_id: DocumentId("d".into()),
            revision_id: RevisionId("r1".into()),
            title: "t".into(),
            tabs: vec![TabSnapshot {
                tab_id: None,
                paragraphs: vec![ParagraphSnapshot {
                    n: 1, kind: crate::gdocs::domain::ParagraphKind::Paragraph,
                    text: "hi".into(), start_index: 1, end_index: 3,
                }],
                tables: vec![TableSnapshot {
                    table_index: 0, tab_id: None, start_index: 5, rows: 1, columns: 2,
                    cells: vec![vec![
                        CellSnapshot { row: 0, col: 0, text: "a".into(),
                            content_start_index: 6, content_end_index: 8, row_span: 1, col_span: 1 },
                        CellSnapshot { row: 0, col: 1, text: "b".into(),
                            content_start_index: 8, content_end_index: 10, row_span: 1, col_span: 1 },
                    ]],
                }],
            }],
        }
    }

    #[tokio::test]
    async fn read_tables_lists_cells_with_previews() {
        let mut client = MockDocsClient::new();
        client.expect_get().returning(|_| Ok(snap_with_one_table()));
        let listing = run_read_tables(&client, &DocumentId("d".into()), None).await.unwrap();
        assert_eq!(listing.tables.len(), 1);
        assert_eq!(listing.tables[0].columns, 2);
        assert_eq!(listing.tables[0].cells[1].text_preview, "b");
    }

    #[test]
    fn find_cell_rejects_out_of_range() {
        let s = snap_with_one_table();
        let t = find_table(&s, 0, None).unwrap();
        assert!(find_cell(t, 0, 5).is_err());
    }
}
```

- [ ] **Step 4: Implement `run_read_tables`**:

```rust
/// Read every table in the doc (optionally scoped to a tab) with per-cell
/// previews so the agent can address cells by `(table_index, row, col)`.
pub async fn run_read_tables(
    client: &dyn DocsClient,
    doc_id: &DocumentId,
    tab_id: Option<&TabId>,
) -> Result<TableListing, DocsError> {
    let snap = client.get(doc_id).await?;
    let mut tables = Vec::new();
    for tab in &snap.tabs {
        if let Some(want) = tab_id {
            if tab.tab_id.as_ref() != Some(want) {
                continue;
            }
        }
        for t in &tab.tables {
            tables.push(TableInfo {
                table_index: t.table_index,
                tab_id: t.tab_id.as_ref().map(|x| x.0.clone()),
                rows: t.rows,
                columns: t.columns,
                cells: t
                    .cells
                    .iter()
                    .flatten()
                    .map(|c| TableCellInfo {
                        row: c.row,
                        col: c.col,
                        text_preview: c.text.chars().take(80).collect(),
                        row_span: c.row_span,
                        col_span: c.col_span,
                    })
                    .collect(),
            });
        }
    }
    Ok(TableListing { tables })
}
```

- [ ] **Step 5: Run to verify pass**

Run: `cargo test --lib gdocs::application::table 2>&1 | tail -20`
Expected: PASS (2 tests).

- [ ] **Step 6: Commit**

```bash
git add src/libs/colmena/src/gdocs/application/table.rs src/libs/colmena/src/gdocs/application/mod.rs
git commit -m "feat(gdocs): table.rs scaffold + run_read_tables + cell addressing"
```

---

## Task 5: Application — `run_set_table_cell`

**Files:**
- Modify: `src/libs/colmena/src/gdocs/application/table.rs`
- Test: inline `#[cfg(test)]` in `table.rs`

- [ ] **Step 1: Write the failing test** (add to `table.rs` tests):

```rust
#[test]
fn build_set_cell_requests_deletes_then_inserts() {
    let cell = CellSnapshot { row: 0, col: 0, text: "ab".into(),
        content_start_index: 6, content_end_index: 9, row_span: 1, col_span: 1 };
    let reqs = build_set_cell_requests(&cell, "X", None);
    assert_eq!(reqs.len(), 2);
    // delete [6, 8) preserves the trailing newline at 8
    assert_eq!(reqs[0]["deleteContentRange"]["range"]["startIndex"], 6);
    assert_eq!(reqs[0]["deleteContentRange"]["range"]["endIndex"], 8);
    assert_eq!(reqs[1]["insertText"]["location"]["index"], 6);
    assert_eq!(reqs[1]["insertText"]["text"], "X");
}

#[test]
fn build_set_cell_requests_empty_cell_skips_delete() {
    let cell = CellSnapshot { row: 0, col: 0, text: "".into(),
        content_start_index: 6, content_end_index: 7, row_span: 1, col_span: 1 };
    let reqs = build_set_cell_requests(&cell, "X", None);
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0]["insertText"]["text"], "X");
}

#[test]
fn set_cell_rejects_merged_slave_master() {
    // A merged master (col_span 2) is still addressable; a non-existent
    // slave position errors. Covered by find_cell test in Task 4; here we
    // assert set on a master cell with text builds requests.
    let cell = CellSnapshot { row: 0, col: 0, text: "hi".into(),
        content_start_index: 6, content_end_index: 9, row_span: 2, col_span: 2 };
    assert_eq!(build_set_cell_requests(&cell, "x", None).len(), 2);
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --lib build_set_cell 2>&1 | tail -20`
Expected: FAIL — `cannot find function build_set_cell_requests`.

- [ ] **Step 3: Implement the builder + the use case**:

```rust
/// Build the requests to replace a cell's text with `text`. Deletes the
/// existing content while preserving the cell's mandatory trailing newline
/// (`content_end_index - 1`), then inserts at the cell content start.
fn build_set_cell_requests(
    cell: &CellSnapshot,
    text: &str,
    tab_id: Option<&TabId>,
) -> Vec<serde_json::Value> {
    let mut reqs = Vec::new();
    let del_end = cell.content_end_index.saturating_sub(1);
    if del_end > cell.content_start_index {
        let mut range = location(cell.content_start_index, tab_id);
        range["endIndex"] = serde_json::json!(del_end);
        range["startIndex"] = serde_json::json!(cell.content_start_index);
        // `location` set "index"; drop it — a range uses start/endIndex.
        range.as_object_mut().unwrap().remove("index");
        reqs.push(serde_json::json!({ "deleteContentRange": { "range": range } }));
    }
    reqs.push(serde_json::json!({
        "insertText": {
            "location": location(cell.content_start_index, tab_id),
            "text": text,
        }
    }));
    reqs
}

/// Replace the plain text of a single cell. One cell per call: re-fetches a
/// fresh snapshot so indices are correct.
pub async fn run_set_table_cell(
    ctx: &GuardContext<'_>,
    doc_id: &DocumentId,
    table_index: u32,
    row: u32,
    col: u32,
    text: &str,
    tab_id: Option<TabId>,
) -> Result<EditResult, DocsError> {
    let guard = run_guard_non_blocking(ctx, doc_id).await?;
    let snap = &guard.snapshot;
    let table = find_table(snap, table_index, tab_id.as_ref())?;
    let cell = find_cell(table, row, col)?;
    let requests = build_set_cell_requests(cell, text, tab_id.as_ref());
    apply_and_finalize(
        ctx,
        doc_id,
        snap,
        requests,
        ChangeKind::Replace,
        0, // tables aren't paragraphs — paragraph N/A
        &format!("table[{}] cell({},{}) = {}", table_index, row, col, text),
        tab_id,
        vec![],
        guard.soft_warnings,
    )
    .await
}
```

(Confirm `ChangeKind::Replace` exists; if the variant is named differently — e.g. `ChangeKind::Edit` — use the existing one. Check `domain/types.rs` `enum ChangeKind`.)

- [ ] **Step 4: Run to verify pass**

Run: `cargo test --lib build_set_cell 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/gdocs/application/table.rs
git commit -m "feat(gdocs): run_set_table_cell (plain-text cell replace)"
```

---

## Task 6: Application — row + column insert/delete

**Files:**
- Modify: `src/libs/colmena/src/gdocs/application/table.rs`
- Test: inline `#[cfg(test)]` in `table.rs`

- [ ] **Step 1: Write the failing tests**:

```rust
#[test]
fn build_row_and_column_requests() {
    let r = build_insert_row_request(5, 2, true, None);
    assert_eq!(r["insertTableRow"]["tableCellLocation"]["tableStartLocation"]["index"], 5);
    assert_eq!(r["insertTableRow"]["tableCellLocation"]["rowIndex"], 2);
    assert_eq!(r["insertTableRow"]["insertBelow"], true);

    let d = build_delete_row_request(5, 1, None);
    assert_eq!(d["deleteTableRow"]["tableCellLocation"]["rowIndex"], 1);

    let c = build_insert_column_request(5, 0, false, None);
    assert_eq!(c["insertTableColumn"]["tableCellLocation"]["columnIndex"], 0);
    assert_eq!(c["insertTableColumn"]["insertRight"], false);

    let dc = build_delete_column_request(5, 3, Some(&TabId("plan".into())));
    assert_eq!(dc["deleteTableColumn"]["tableCellLocation"]["tableStartLocation"]["tabId"], "plan");
    assert_eq!(dc["deleteTableColumn"]["tableCellLocation"]["columnIndex"], 3);
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --lib build_row_and_column 2>&1 | tail -20`
Expected: FAIL — builders not found.

- [ ] **Step 3: Implement the 4 builders + 4 use cases**:

```rust
fn build_insert_row_request(table_start: u32, at_row: u32, insert_below: bool, tab_id: Option<&TabId>) -> serde_json::Value {
    serde_json::json!({ "insertTableRow": {
        "tableCellLocation": cell_location(table_start, at_row, 0, tab_id),
        "insertBelow": insert_below } })
}
fn build_delete_row_request(table_start: u32, row: u32, tab_id: Option<&TabId>) -> serde_json::Value {
    serde_json::json!({ "deleteTableRow": {
        "tableCellLocation": cell_location(table_start, row, 0, tab_id) } })
}
fn build_insert_column_request(table_start: u32, at_col: u32, insert_right: bool, tab_id: Option<&TabId>) -> serde_json::Value {
    serde_json::json!({ "insertTableColumn": {
        "tableCellLocation": cell_location(table_start, 0, at_col, tab_id),
        "insertRight": insert_right } })
}
fn build_delete_column_request(table_start: u32, col: u32, tab_id: Option<&TabId>) -> serde_json::Value {
    serde_json::json!({ "deleteTableColumn": {
        "tableCellLocation": cell_location(table_start, 0, col, tab_id) } })
}

pub async fn run_insert_table_row(ctx: &GuardContext<'_>, doc_id: &DocumentId, table_index: u32, at_row: u32, insert_below: bool, tab_id: Option<TabId>) -> Result<EditResult, DocsError> {
    let guard = run_guard_non_blocking(ctx, doc_id).await?;
    let table = find_table(&guard.snapshot, table_index, tab_id.as_ref())?;
    let req = build_insert_row_request(table.start_index, at_row, insert_below, tab_id.as_ref());
    apply_and_finalize(ctx, doc_id, &guard.snapshot, vec![req], ChangeKind::Insert, 0,
        &format!("table[{}] insert row at {} (below={})", table_index, at_row, insert_below),
        tab_id, vec![], guard.soft_warnings).await
}
pub async fn run_delete_table_row(ctx: &GuardContext<'_>, doc_id: &DocumentId, table_index: u32, row: u32, tab_id: Option<TabId>) -> Result<EditResult, DocsError> {
    let guard = run_guard_non_blocking(ctx, doc_id).await?;
    let table = find_table(&guard.snapshot, table_index, tab_id.as_ref())?;
    let req = build_delete_row_request(table.start_index, row, tab_id.as_ref());
    apply_and_finalize(ctx, doc_id, &guard.snapshot, vec![req], ChangeKind::Delete, 0,
        &format!("table[{}] delete row {}", table_index, row), tab_id, vec![], guard.soft_warnings).await
}
pub async fn run_insert_table_column(ctx: &GuardContext<'_>, doc_id: &DocumentId, table_index: u32, at_col: u32, insert_right: bool, tab_id: Option<TabId>) -> Result<EditResult, DocsError> {
    let guard = run_guard_non_blocking(ctx, doc_id).await?;
    let table = find_table(&guard.snapshot, table_index, tab_id.as_ref())?;
    let req = build_insert_column_request(table.start_index, at_col, insert_right, tab_id.as_ref());
    apply_and_finalize(ctx, doc_id, &guard.snapshot, vec![req], ChangeKind::Insert, 0,
        &format!("table[{}] insert column at {} (right={})", table_index, at_col, insert_right),
        tab_id, vec![], guard.soft_warnings).await
}
pub async fn run_delete_table_column(ctx: &GuardContext<'_>, doc_id: &DocumentId, table_index: u32, col: u32, tab_id: Option<TabId>) -> Result<EditResult, DocsError> {
    let guard = run_guard_non_blocking(ctx, doc_id).await?;
    let table = find_table(&guard.snapshot, table_index, tab_id.as_ref())?;
    let req = build_delete_column_request(table.start_index, col, tab_id.as_ref());
    apply_and_finalize(ctx, doc_id, &guard.snapshot, vec![req], ChangeKind::Delete, 0,
        &format!("table[{}] delete column {}", table_index, col), tab_id, vec![], guard.soft_warnings).await
}
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test --lib gdocs::application::table 2>&1 | tail -20`
Expected: PASS (all table tests).

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/gdocs/application/table.rs
git commit -m "feat(gdocs): insert/delete table row + column use cases"
```

---

## Task 7: Synthetic tools — consts, Args, defs, dispatchers

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gdocs_tools.rs`

- [ ] **Step 1: Add the 6 `TOOL_*` consts** after `TOOL_RESOLVE_COMMENT` (~line 67):

```rust
pub const TOOL_READ_TABLES: &str = "gdocs_read_tables";
pub const TOOL_SET_TABLE_CELL: &str = "gdocs_set_table_cell";
pub const TOOL_INSERT_TABLE_ROW: &str = "gdocs_insert_table_row";
pub const TOOL_DELETE_TABLE_ROW: &str = "gdocs_delete_table_row";
pub const TOOL_INSERT_TABLE_COLUMN: &str = "gdocs_insert_table_column";
pub const TOOL_DELETE_TABLE_COLUMN: &str = "gdocs_delete_table_column";
```

- [ ] **Step 2: Add the 6 Args structs** near the other `*Args` (~line 489):

```rust
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReadTablesArgs {
    pub doc_id: String,
    pub tab_id: Option<String>,
}
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SetTableCellArgs {
    pub doc_id: String,
    pub table_index: u32,
    pub row: u32,
    pub col: u32,
    pub text: String,
    pub tab_id: Option<String>,
    pub mode: Option<String>,
}
#[derive(Debug, Deserialize, JsonSchema)]
pub struct InsertTableRowArgs {
    pub doc_id: String,
    pub table_index: u32,
    pub at_row: u32,
    #[serde(default = "default_true")]
    pub insert_below: bool,
    pub tab_id: Option<String>,
    pub mode: Option<String>,
}
#[derive(Debug, Deserialize, JsonSchema)]
pub struct DeleteTableRowArgs {
    pub doc_id: String,
    pub table_index: u32,
    pub row: u32,
    pub tab_id: Option<String>,
    pub mode: Option<String>,
}
#[derive(Debug, Deserialize, JsonSchema)]
pub struct InsertTableColumnArgs {
    pub doc_id: String,
    pub table_index: u32,
    pub at_col: u32,
    #[serde(default = "default_true")]
    pub insert_right: bool,
    pub tab_id: Option<String>,
    pub mode: Option<String>,
}
#[derive(Debug, Deserialize, JsonSchema)]
pub struct DeleteTableColumnArgs {
    pub doc_id: String,
    pub table_index: u32,
    pub col: u32,
    pub tab_id: Option<String>,
    pub mode: Option<String>,
}

fn default_true() -> bool { true }
```

(If a `default_true` helper already exists in this file, reuse it — grep first; don't define twice.)

- [ ] **Step 3: Add the 6 `tool_*()` builder fns** near `tool_read_outline`/`tool_insert_image_after_text` (~line 687-725). Use the same `build_synthetic_tool::<T>` / `build_synthetic_tool_with_summary::<T>` helper the neighbouring builders use (match the exact form used by `tool_replace_text`):

```rust
pub fn tool_read_tables() -> ToolDefinition {
    super::build_synthetic_tool::<ReadTablesArgs>(
        TOOL_READ_TABLES,
        "List every table in a Google Doc with per-cell text previews, so you can address cells by (table_index, row, col).",
    )
}
pub fn tool_set_table_cell() -> ToolDefinition {
    super::build_synthetic_tool::<SetTableCellArgs>(
        TOOL_SET_TABLE_CELL,
        "Replace the plain text of one table cell, addressed by table_index (from gdocs_read_tables), row, and col (all 0-based).",
    )
}
pub fn tool_insert_table_row() -> ToolDefinition {
    super::build_synthetic_tool::<InsertTableRowArgs>(
        TOOL_INSERT_TABLE_ROW,
        "Insert a row into a table, relative to an existing row (at_row) above or below (insert_below).",
    )
}
pub fn tool_delete_table_row() -> ToolDefinition {
    super::build_synthetic_tool::<DeleteTableRowArgs>(
        TOOL_DELETE_TABLE_ROW,
        "Delete a row (0-based) from a table.",
    )
}
pub fn tool_insert_table_column() -> ToolDefinition {
    super::build_synthetic_tool::<InsertTableColumnArgs>(
        TOOL_INSERT_TABLE_COLUMN,
        "Insert a column into a table, relative to an existing column (at_col) left or right (insert_right).",
    )
}
pub fn tool_delete_table_column() -> ToolDefinition {
    super::build_synthetic_tool::<DeleteTableColumnArgs>(
        TOOL_DELETE_TABLE_COLUMN,
        "Delete a column (0-based) from a table.",
    )
}
```

(Grep `tool_read_outline` to confirm whether it uses `build_synthetic_tool` or `build_synthetic_tool_with_summary`, and copy that exact form — the description text lives in `gdocs.yaml` and overrides these in Task 9.)

- [ ] **Step 4: Add the 6 builders to `build_all_gdocs_tools()`** (after `tool_resolve_comment()`, ~line 843):

```rust
        // Subsystem G v1.1 (2026-06-21): surgical table edits.
        tool_read_tables(),
        tool_set_table_cell(),
        tool_insert_table_row(),
        tool_delete_table_row(),
        tool_insert_table_column(),
        tool_delete_table_column(),
```

- [ ] **Step 5: Add the 6 dispatchers** in the dispatcher section. `read_tables` is read-only (no guard ctx); the 5 writers build a `GuardContext` exactly like `dispatch_insert_after_text`:

```rust
pub async fn dispatch_read_tables(args: serde_json::Value, _session_id: &str) -> serde_json::Value {
    let parsed: ReadTablesArgs = match serde_json::from_value(args) {
        Ok(a) => a, Err(e) => return invalid_args(e),
    };
    let client = match shared_client().await { Ok(c) => c, Err(e) => return e };
    let tab = parsed.tab_id.map(TabId);
    match table::run_read_tables(client.as_ref(), &DocumentId(parsed.doc_id), tab.as_ref()).await {
        Ok(listing) => serde_json::json!({ "ok": true, "tables": listing.tables }),
        Err(e) => error_to_json(e),
    }
}

pub async fn dispatch_set_table_cell(args: serde_json::Value, session_id: &str) -> serde_json::Value {
    let parsed: SetTableCellArgs = match serde_json::from_value(args) {
        Ok(a) => a, Err(e) => return invalid_args(e),
    };
    let client = match shared_client().await { Ok(c) => c, Err(e) => return e };
    let cache = shared_cache().await;
    let revisions = match shared_revs().await { Ok(r) => r, Err(e) => return e };
    let sa = std::env::var("COLMENA_GDOCS_SA_EMAIL").ok();
    let ctx = co_edit_guard::GuardContext {
        client: client.as_ref(), cache: cache.as_ref(), revisions: revisions.as_ref(),
        session_id, sa_email: sa.as_deref(),
    };
    match table::run_set_table_cell(&ctx, &DocumentId(parsed.doc_id), parsed.table_index,
        parsed.row, parsed.col, &parsed.text, parsed.tab_id.map(TabId)).await {
        Ok(r) => edit_result_to_json(r), Err(e) => error_to_json(e),
    }
}
```

Add the 4 remaining writers (`dispatch_insert_table_row`, `dispatch_delete_table_row`, `dispatch_insert_table_column`, `dispatch_delete_table_column`) with the identical ctx-building preamble, calling the matching `table::run_*` with their args (`at_row`/`insert_below`, `row`, `at_col`/`insert_right`, `col`). Add `use crate::gdocs::application::table;` to the dispatcher imports if not already imported (the module already imports `co_edit_guard`, `insert`, etc. near the top — add `table` there).

- [ ] **Step 6: Build**

Run: `cargo build --lib 2>&1 | tail -20`
Expected: compiles (tools defined but not yet wired to llm.rs/router — that's Task 8).

- [ ] **Step 7: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gdocs_tools.rs
git commit -m "feat(gdocs): 6 table-edit synthetic tools (defs + dispatchers)"
```

---

## Task 8: Wiring — re-exports, toolkit packages, llm.rs arrays, router

**Files:**
- Modify: `.../llm_synthetic_tools/mod.rs`
- Modify: `.../llm_synthetic_tools/toolkit_packages.rs`
- Modify: `.../nodes/llm.rs`
- Modify: `.../infrastructure/dag_tool_executor.rs`

- [ ] **Step 1: Re-export in `mod.rs`.** Add to the `gdocs_tools::{…}` re-export block (near line 372-424) the 6 dispatchers (aliased `dispatch_gdocs_*`) and 6 `TOOL_*` consts (aliased `GDOCS_*_TOOL`), following the existing pattern:

```rust
    dispatch_read_tables as dispatch_gdocs_read_tables,
    dispatch_set_table_cell as dispatch_gdocs_set_table_cell,
    dispatch_insert_table_row as dispatch_gdocs_insert_table_row,
    dispatch_delete_table_row as dispatch_gdocs_delete_table_row,
    dispatch_insert_table_column as dispatch_gdocs_insert_table_column,
    dispatch_delete_table_column as dispatch_gdocs_delete_table_column,
    // ...
    TOOL_READ_TABLES as GDOCS_READ_TABLES_TOOL,
    TOOL_SET_TABLE_CELL as GDOCS_SET_TABLE_CELL_TOOL,
    TOOL_INSERT_TABLE_ROW as GDOCS_INSERT_TABLE_ROW_TOOL,
    TOOL_DELETE_TABLE_ROW as GDOCS_DELETE_TABLE_ROW_TOOL,
    TOOL_INSERT_TABLE_COLUMN as GDOCS_INSERT_TABLE_COLUMN_TOOL,
    TOOL_DELETE_TABLE_COLUMN as GDOCS_DELETE_TABLE_COLUMN_TOOL,
```

Also export the builder fns if the existing pattern re-exports `gdocs_tool_*` (grep `gdocs_tool_read_outline` in mod.rs to confirm the alias form, then add the 6 `tool_* as gdocs_tool_*`).

- [ ] **Step 2: Toolkit packages.** In `toolkit_packages.rs`:
  - `gdocs` alias: bump description `(29 tools)` → `(35 tools)`; append the 6 names after `gdocs_resolve_comment`:

```rust
            // Subsystem G v1.1 (2026-06-21): surgical table edits
            "gdocs_read_tables",
            "gdocs_set_table_cell",
            "gdocs_insert_table_row",
            "gdocs_delete_table_row",
            "gdocs_insert_table_column",
            "gdocs_delete_table_column",
```

  - `gdocsread` alias: bump description `(9 tools …)` → `(10 tools …)`; add `"gdocs_read_tables",` to its tools list.
  - Update the in-file sync tests (`gdocs_full_package_matches_builder` ~line 158, `gdocsread_readonly_package_subset` ~line 199) if they assert exact counts/membership.

- [ ] **Step 3: llm.rs arrays.** In `llm.rs`:
  - Add the 6 `GDOCS_*_TOOL` consts and 6 `gdocs_tool_*` builders to the `use …::{…}` block (~line 2560-2604).
  - Change `let all_gdocs: [&str; 29]` → `[&str; 35]` and append the 6 consts (after `GDOCS_RESOLVE_COMMENT_TOOL`).
  - Change `let gdocs_entries: [(…); 29]` → `[(…); 35]` and append the 6 `(GDOCS_*_TOOL, gdocs_tool_*)` pairs.
  - Update the contract comment "Count must equal …" (it references 29 implicitly; the `[&str; 35]` is the real guard).

- [ ] **Step 4: Router in `dag_tool_executor.rs`.** In the `use …llm_synthetic_tools::{…}` block (~line 1342):
  - Import `dispatch_gdocs_read_tables, dispatch_gdocs_set_table_cell, dispatch_gdocs_insert_table_row, dispatch_gdocs_delete_table_row, dispatch_gdocs_insert_table_column, dispatch_gdocs_delete_table_column` and the 6 `GDOCS_*_TOOL` consts.
  - Add 6 `|| n == GDOCS_*_TOOL` clauses to `is_gdocs_tool` (~line 1399).
  - Add 6 match arms (read_tables takes `(args, session_id)`; the 5 writers also `(args, session_id)`):

```rust
                    n if n == GDOCS_READ_TABLES_TOOL => {
                        dispatch_gdocs_read_tables(args, session_id).await
                    }
                    n if n == GDOCS_SET_TABLE_CELL_TOOL => {
                        dispatch_gdocs_set_table_cell(args, session_id).await
                    }
                    n if n == GDOCS_INSERT_TABLE_ROW_TOOL => {
                        dispatch_gdocs_insert_table_row(args, session_id).await
                    }
                    n if n == GDOCS_DELETE_TABLE_ROW_TOOL => {
                        dispatch_gdocs_delete_table_row(args, session_id).await
                    }
                    n if n == GDOCS_INSERT_TABLE_COLUMN_TOOL => {
                        dispatch_gdocs_insert_table_column(args, session_id).await
                    }
                    n if n == GDOCS_DELETE_TABLE_COLUMN_TOOL => {
                        dispatch_gdocs_delete_table_column(args, session_id).await
                    }
```

- [ ] **Step 5: Build + run the cross-file contract tests**

Run: `cargo test --lib 2>&1 | tail -30`
Expected: PASS. In particular any `*_count` / `*_matches_builder` / toolkit sync tests must be green. If a test asserts `build_all_gdocs_tools().len() == 29`, update it to 35.

- [ ] **Step 6: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs \
        src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/toolkit_packages.rs \
        src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs \
        src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs
git commit -m "feat(gdocs): wire 6 table tools (mod re-exports, toolkit, llm.rs arrays, router)"
```

---

## Task 9: LLM-facing descriptions (`gdocs.yaml`)

**Files:**
- Modify: `src/libs/colmena/text/tools/gdocs.yaml`

- [ ] **Step 1: Add 6 entries** keyed by tool name (match the existing YAML shape — grep `gdocs_insert_image_after_text:` to copy the exact key/field structure). Content:

```yaml
gdocs_read_tables:
  description: >
    Lista todas las tablas de un Google Doc con un preview de texto por celda.
    Devuelve, por tabla: table_index (0-based, orden en el tab), rows, columns,
    y las celdas con row/col (0-based), text_preview, row_span y col_span.
    Úsalo SIEMPRE antes de editar una tabla para obtener el table_index y las
    coordenadas. Funciona sobre docs compartidos (usa la Docs API).

gdocs_set_table_cell:
  description: >
    Reemplaza el TEXTO PLANO de una sola celda de tabla. Direcciona con
    table_index (de gdocs_read_tables) + row + col (todos 0-based). Una celda
    por llamada. No acepta markdown ni estilo: solo texto. Para celdas
    fusionadas (merged), solo la celda master es editable.

gdocs_insert_table_row:
  description: >
    Inserta una fila en una tabla, relativa a una fila existente (at_row),
    arriba o abajo según insert_below (default true = abajo). La nueva fila
    queda vacía.

gdocs_delete_table_row:
  description: >
    Borra la fila row (0-based) de una tabla. No se puede borrar la última
    fila restante.

gdocs_insert_table_column:
  description: >
    Inserta una columna en una tabla, relativa a una columna existente
    (at_col), a la izquierda o derecha según insert_right (default true).

gdocs_delete_table_column:
  description: >
    Borra la columna col (0-based) de una tabla. No se puede borrar la última
    columna restante.
```

- [ ] **Step 2: Verify the YAML loads** (the text registry is validated at build/test):

Run: `cargo test --lib text 2>&1 | tail -15`
Expected: PASS (no missing-key / parse errors).

- [ ] **Step 3: Commit**

```bash
git add src/libs/colmena/text/tools/gdocs.yaml
git commit -m "docs(gdocs): LLM-facing descriptions for 6 table tools"
```

---

## Task 10: Integration `#[ignore]` live round-trip

**Files:**
- Modify: `src/libs/colmena/tests/gdocs_integration_test.rs`

- [ ] **Step 1: Add an `#[ignore]` test** modelled on the existing live tests in this file (copy the auth/client setup from `image_upload_public_insert_delete_roundtrip`). Flow: create a doc from markdown containing a table → `read_tables` (assert dims) → `set_table_cell` → `insert_table_row` → `delete_table_column` → `read_tables` again and assert the final shape.

```rust
#[tokio::test]
#[ignore = "requires Google OAuth creds — run with `cargo test -- --ignored`"]
async fn table_read_set_insert_delete_roundtrip() {
    // ... build the HTTP client exactly like the other live tests ...
    // 1. create_from_markdown with: "| A | B |\n|---|---|\n| 1 | 2 |\n"
    // 2. run_read_tables → assert tables[0].columns == 2 && rows == 2
    // 3. run_set_table_cell(table 0, row 1, col 0, "EDITED")
    // 4. run_insert_table_row(table 0, at_row 1, insert_below true)
    // 5. run_delete_table_column(table 0, col 1)
    // 6. run_read_tables → assert columns == 1 && a cell text == "EDITED"
    // 7. cleanup: delete the doc
}
```

(Fill in the client construction from the sibling test verbatim — do not invent a new auth path.)

- [ ] **Step 2: Build the test (compile only — do not run live here)**

Run: `cargo test --no-run 2>&1 | tail -15`
Expected: compiles.

- [ ] **Step 3: Commit**

```bash
git add src/libs/colmena/tests/gdocs_integration_test.rs
git commit -m "test(gdocs): live #[ignore] table edit round-trip"
```

---

## Task 11: E2E LLM-in-the-loop graph

**Files:**
- Create: `tests/graphs/agents/gdocs_table_edits_e2e.json`

- [ ] **Step 1: Write the graph** in `nodes`-as-map form (trigger_webhook + test_payload + llm_call agent with `enabled_tools: ["gdocs"]`). Use placeholder `<YOUR_DOC_ID>`; default LLM stack (google/gemini-2.5-flash, `connection_url: ${DATABASE_URL}`). The prompt instructs the agent to: read tables, fill a specific cell, add a row, and report the verbatim tool results.

```json
{
  "_comment": "E2E (manual, live): agent reads a table in an operator-provided Google Doc, sets a cell, and inserts a row, IN ONE TURN. Replace <YOUR_DOC_ID>. Needs GEMINI_API_KEY + DATABASE_URL + OAuth creds in-memory + --agent-session-id. Uses Docs API (read_tables/set/insert) so a SHARED doc works.",
  "nodes": {
    "trigger": {
      "type": "trigger_webhook",
      "config": {
        "path": "/gdocs-table-edits",
        "method": "POST",
        "test_payload": {
          "prompt": "On Google Doc id <YOUR_DOC_ID>, do in order: 1) call gdocs_read_tables and report the first table's dimensions and cell previews. 2) call gdocs_set_table_cell on table_index 0, row 1, col 0 with text 'EDITED'. 3) call gdocs_insert_table_row on table_index 0, at_row 1, insert_below true. 4) Report the verbatim changes/revision_id_after of each step."
        }
      }
    },
    "agent": {
      "type": "llm_call",
      "config": {
        "provider": "google",
        "api_key": "${GEMINI_API_KEY}",
        "model": "gemini-2.5-flash",
        "stream": false,
        "max_iterations": 10,
        "connection_url": "${DATABASE_URL}",
        "system_message": "You edit Google Docs tables. Always call gdocs_read_tables first to get table_index and coordinates. Report tool results verbatim.",
        "enabled_tools": ["gdocs"]
      }
    },
    "log": { "type": "log" }
  },
  "edges": [
    { "from": "trigger", "to": "agent" },
    { "from": "agent", "to": "log" }
  ]
}
```

- [ ] **Step 2: Commit**

```bash
git add tests/graphs/agents/gdocs_table_edits_e2e.json
git commit -m "test(gdocs): E2E LLM-in-the-loop table edits graph"
```

---

## Task 12: Docs + sweep + push (controller-run live verify)

**Files:**
- Modify: `docs/developer_guide/45_gdocs.md`, `docs/CHANGELOG_2026-06.md`, `docs/BACKLOG.md`

- [ ] **Step 1: Dev guide §45.** Add a "Table edits" table to the tools listing (the 6 tools with arg summaries), and a one-line note in the `drive.file` scope box that table tools use the Docs API → work on shared docs. Note plain-text-only and one-cell-per-call.

- [ ] **Step 2: CHANGELOG.** Add a new section (next number after the latest §, currently §45 → use §46) summarizing: 6 table tools, snapshot now models tables, non-blocking guard, additive/no ADP impact, count 29→35 / gdocsread 9→10.

- [ ] **Step 3: BACKLOG.** Mark `[ ] Surgical table-cell edits` → `[x]` with "SHIPPED 2026-06-21" + commit pointer; update the prioritization table row (line ~55) to strikethrough/shipped. Note columns were INCLUDED (not deferred).

- [ ] **Step 4: Commit docs**

```bash
git add docs/developer_guide/45_gdocs.md docs/CHANGELOG_2026-06.md docs/BACKLOG.md
git commit -m "docs(gdocs): table-cell edits — dev guide, CHANGELOG, BACKLOG"
```

- [ ] **Step 5: Full sweep (CI parity)**

Run: `cargo fmt && cargo test --verbose 2>&1 | tail -40`
Expected: all green (unit + integration non-ignored + doctests). `--verbose` (not `--lib`) per CLAUDE.md.

- [ ] **Step 6: ADP breaking-change sweep.** Confirm additive-only (new domain types with `#[serde(default)]`, new module, new tools — no `EngineConfig`/`ColmenaEngine`/exported-trait signature change). Grep the ADP worker only if any public signature changed; expected: nothing to do.

- [ ] **Step 7: Live E2E (CONTROLLER step — handles secrets in-memory).** Per memory `feedback_always_real_e2e` + `ref_local_gsheets_e2e_runbook`: inject OAuth creds from Secret Manager in-memory (no echo/file/commit), run `tests/graphs/agents/gdocs_table_edits_e2e.json` against a real shared doc with `--agent-session-id`, save SSE to `/tmp/colmena_e2e/gdocs_table_edits.sse`, and present a friendly report. Also run the `#[ignore]` integration test: `source .env && cargo test -- --ignored table_read_set_insert_delete_roundtrip`.

- [ ] **Step 8: Push + watch CI**

```bash
git push origin develop
gh run watch
```

---

## Self-Review Notes (filled by author)

- **Spec coverage:** read_tables (T4), set_table_cell (T5), insert/delete row+column (T6), plain-text-only (T5 builder), 0-based index addressing (T4 `find_table`/`find_cell`), doc-level non-blocking guard (T3), one-cell-per-call fresh fetch (T5/T6 each call `run_guard_non_blocking` → fresh snapshot), merged-cell + out-of-range handling (T4 `find_cell`, T5 test), multi-tab tabId (T4 `location`), Docs-API/shared-doc scope (T12 docs), 6 synthetic tools + toolkits (T7/T8), testing layers (T2/T4/T5/T6 unit, T10 integration, T11 E2E). All spec sections map to a task.
- **Type consistency:** `TableSnapshot`/`CellSnapshot` field names identical across T1 (def), T2 (parse), T4-T6 (consume). `run_*` signatures fixed in T4-T6 and called identically in T7 dispatchers. Builder fns (`build_set_cell_requests`, `build_*_row_request`, `build_*_column_request`) named consistently T5/T6. Count 29→35 applied uniformly in T8 across `build_all_gdocs_tools`, `all_gdocs`, `gdocs_entries`, toolkit.
- **Open verification during impl:** confirm `ChangeKind` variant names (`Replace`/`Insert`/`Delete`) in `domain/types.rs` (T5 note); confirm `build_synthetic_tool` vs `_with_summary` form for tool defs (T7 note); confirm `default_true` helper not already defined (T7 note). These are "use the existing name" checks, not placeholders.
