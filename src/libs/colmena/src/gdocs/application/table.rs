//! Surgical table editing use cases: read tables, set a cell's plain text,
//! and insert/delete rows and columns. Addresses tables by 0-based
//! `table_index` (order within a tab). Plain text only — rich/markdown cell
//! content is deferred to v1.1. See spec
//! `docs/superpowers/specs/2026-06-21-gdocs-table-cell-edits-design.md`.

use crate::gdocs::application::co_edit_guard::{run_guard_non_blocking, GuardContext};
use crate::gdocs::application::insert::apply_and_finalize;
use crate::gdocs::application::util;
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

// ── Helpers ────────────────────────────────────────────────────────

/// Build a `tableCellLocation` JSON.
fn cell_location(
    table_start: u32,
    row: u32,
    col: u32,
    tab_id: Option<&TabId>,
) -> serde_json::Value {
    serde_json::json!({
        "tableStartLocation": util::location(table_start, &tab_id.cloned()),
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
fn find_cell(t: &TableSnapshot, row: u32, col: u32) -> Result<&CellSnapshot, DocsError> {
    t.cells
        .get(row as usize)
        .and_then(|r| r.iter().find(|c| c.col == col))
        .ok_or_else(|| {
            DocsError::InvalidArgs(format!(
                "cell ({},{}) not addressable in table {} ({}x{}) — out of range or covered by a merged cell",
                row, col, t.table_index, t.rows, t.columns
            ))
        })
}

// ── read_tables ────────────────────────────────────────────────────

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

// ── set_table_cell ─────────────────────────────────────────────────

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
        reqs.push(serde_json::json!({
            "deleteContentRange": {
                "range": util::range(cell.content_start_index, del_end, &tab_id.cloned())
            }
        }));
    }
    reqs.push(serde_json::json!({
        "insertText": {
            "location": util::location(cell.content_start_index, &tab_id.cloned()),
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
        0,
        &format!("table[{}] cell({},{}) = {}", table_index, row, col, text),
        tab_id,
        vec![],
        guard.soft_warnings,
    )
    .await
}

// ── row / column insert + delete ───────────────────────────────────

fn build_insert_row_request(
    table_start: u32,
    at_row: u32,
    insert_below: bool,
    tab_id: Option<&TabId>,
) -> serde_json::Value {
    serde_json::json!({ "insertTableRow": {
        "tableCellLocation": cell_location(table_start, at_row, 0, tab_id),
        "insertBelow": insert_below } })
}

fn build_delete_row_request(
    table_start: u32,
    row: u32,
    tab_id: Option<&TabId>,
) -> serde_json::Value {
    serde_json::json!({ "deleteTableRow": {
        "tableCellLocation": cell_location(table_start, row, 0, tab_id) } })
}

fn build_insert_column_request(
    table_start: u32,
    at_col: u32,
    insert_right: bool,
    tab_id: Option<&TabId>,
) -> serde_json::Value {
    serde_json::json!({ "insertTableColumn": {
        "tableCellLocation": cell_location(table_start, 0, at_col, tab_id),
        "insertRight": insert_right } })
}

fn build_delete_column_request(
    table_start: u32,
    col: u32,
    tab_id: Option<&TabId>,
) -> serde_json::Value {
    serde_json::json!({ "deleteTableColumn": {
        "tableCellLocation": cell_location(table_start, 0, col, tab_id) } })
}

/// Insert a row above/below `at_row` in the addressed table.
pub async fn run_insert_table_row(
    ctx: &GuardContext<'_>,
    doc_id: &DocumentId,
    table_index: u32,
    at_row: u32,
    insert_below: bool,
    tab_id: Option<TabId>,
) -> Result<EditResult, DocsError> {
    let guard = run_guard_non_blocking(ctx, doc_id).await?;
    let snap = &guard.snapshot;
    let table = find_table(snap, table_index, tab_id.as_ref())?;
    let request =
        build_insert_row_request(table.start_index, at_row, insert_below, tab_id.as_ref());
    apply_and_finalize(
        ctx,
        doc_id,
        snap,
        vec![request],
        ChangeKind::Insert,
        0,
        &format!(
            "table[{}] insert row at {} (below={})",
            table_index, at_row, insert_below
        ),
        tab_id,
        vec![],
        guard.soft_warnings,
    )
    .await
}

/// Delete `row` in the addressed table.
pub async fn run_delete_table_row(
    ctx: &GuardContext<'_>,
    doc_id: &DocumentId,
    table_index: u32,
    row: u32,
    tab_id: Option<TabId>,
) -> Result<EditResult, DocsError> {
    let guard = run_guard_non_blocking(ctx, doc_id).await?;
    let snap = &guard.snapshot;
    let table = find_table(snap, table_index, tab_id.as_ref())?;
    let request = build_delete_row_request(table.start_index, row, tab_id.as_ref());
    apply_and_finalize(
        ctx,
        doc_id,
        snap,
        vec![request],
        ChangeKind::Delete,
        0,
        &format!("table[{}] delete row {}", table_index, row),
        tab_id,
        vec![],
        guard.soft_warnings,
    )
    .await
}

/// Insert a column left/right of `at_col` in the addressed table.
pub async fn run_insert_table_column(
    ctx: &GuardContext<'_>,
    doc_id: &DocumentId,
    table_index: u32,
    at_col: u32,
    insert_right: bool,
    tab_id: Option<TabId>,
) -> Result<EditResult, DocsError> {
    let guard = run_guard_non_blocking(ctx, doc_id).await?;
    let snap = &guard.snapshot;
    let table = find_table(snap, table_index, tab_id.as_ref())?;
    let request =
        build_insert_column_request(table.start_index, at_col, insert_right, tab_id.as_ref());
    apply_and_finalize(
        ctx,
        doc_id,
        snap,
        vec![request],
        ChangeKind::Insert,
        0,
        &format!(
            "table[{}] insert column at {} (right={})",
            table_index, at_col, insert_right
        ),
        tab_id,
        vec![],
        guard.soft_warnings,
    )
    .await
}

/// Delete `col` in the addressed table.
pub async fn run_delete_table_column(
    ctx: &GuardContext<'_>,
    doc_id: &DocumentId,
    table_index: u32,
    col: u32,
    tab_id: Option<TabId>,
) -> Result<EditResult, DocsError> {
    let guard = run_guard_non_blocking(ctx, doc_id).await?;
    let snap = &guard.snapshot;
    let table = find_table(snap, table_index, tab_id.as_ref())?;
    let request = build_delete_column_request(table.start_index, col, tab_id.as_ref());
    apply_and_finalize(
        ctx,
        doc_id,
        snap,
        vec![request],
        ChangeKind::Delete,
        0,
        &format!("table[{}] delete column {}", table_index, col),
        tab_id,
        vec![],
        guard.soft_warnings,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gdocs::application::_test_helpers::TestRig;
    use crate::gdocs::application::co_edit_guard::GuardContext;
    use crate::gdocs::domain::traits::MockDocsClient;
    use crate::gdocs::domain::{
        BatchUpdateResult, DocumentSnapshot, ParagraphKind, ParagraphSnapshot, RevisionId,
        TabSnapshot,
    };
    use std::sync::{Arc, Mutex};

    fn snap_with_one_table() -> DocumentSnapshot {
        DocumentSnapshot {
            doc_id: DocumentId("d".into()),
            revision_id: RevisionId("r1".into()),
            title: "t".into(),
            tabs: vec![TabSnapshot {
                tab_id: None,
                paragraphs: vec![ParagraphSnapshot {
                    n: 1,
                    kind: ParagraphKind::Paragraph,
                    text: "hi".into(),
                    start_index: 1,
                    end_index: 3,
                }],
                tables: vec![TableSnapshot {
                    table_index: 0,
                    tab_id: None,
                    start_index: 5,
                    rows: 1,
                    columns: 2,
                    cells: vec![vec![
                        CellSnapshot {
                            row: 0,
                            col: 0,
                            text: "a".into(),
                            content_start_index: 6,
                            content_end_index: 8,
                            row_span: 1,
                            col_span: 1,
                        },
                        CellSnapshot {
                            row: 0,
                            col: 1,
                            text: "b".into(),
                            content_start_index: 8,
                            content_end_index: 10,
                            row_span: 1,
                            col_span: 1,
                        },
                    ]],
                }],
            }],
        }
    }

    #[tokio::test]
    async fn read_tables_lists_cells_with_previews() {
        let mut client = MockDocsClient::new();
        client.expect_get().returning(|_| Ok(snap_with_one_table()));
        let listing = run_read_tables(&client, &DocumentId("d".into()), None)
            .await
            .unwrap();
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

    #[test]
    fn build_set_cell_requests_deletes_then_inserts() {
        let cell = CellSnapshot {
            row: 0,
            col: 0,
            text: "ab".into(),
            content_start_index: 6,
            content_end_index: 9,
            row_span: 1,
            col_span: 1,
        };
        let reqs = build_set_cell_requests(&cell, "X", None);
        assert_eq!(reqs.len(), 2);
        assert_eq!(reqs[0]["deleteContentRange"]["range"]["startIndex"], 6);
        assert_eq!(reqs[0]["deleteContentRange"]["range"]["endIndex"], 8);
        assert_eq!(reqs[1]["insertText"]["location"]["index"], 6);
        assert_eq!(reqs[1]["insertText"]["text"], "X");
    }

    #[test]
    fn build_set_cell_requests_empty_cell_skips_delete() {
        let cell = CellSnapshot {
            row: 0,
            col: 0,
            text: "".into(),
            content_start_index: 6,
            content_end_index: 7,
            row_span: 1,
            col_span: 1,
        };
        let reqs = build_set_cell_requests(&cell, "X", None);
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0]["insertText"]["text"], "X");
    }

    #[test]
    fn build_row_and_column_requests() {
        let r = build_insert_row_request(5, 2, true, None);
        assert_eq!(
            r["insertTableRow"]["tableCellLocation"]["tableStartLocation"]["index"],
            5
        );
        assert_eq!(r["insertTableRow"]["tableCellLocation"]["rowIndex"], 2);
        assert_eq!(r["insertTableRow"]["insertBelow"], true);

        let d = build_delete_row_request(5, 1, None);
        assert_eq!(d["deleteTableRow"]["tableCellLocation"]["rowIndex"], 1);

        let c = build_insert_column_request(5, 0, false, None);
        assert_eq!(
            c["insertTableColumn"]["tableCellLocation"]["columnIndex"],
            0
        );
        assert_eq!(c["insertTableColumn"]["insertRight"], false);

        let dc = build_delete_column_request(5, 3, Some(&TabId("plan".into())));
        assert_eq!(
            dc["deleteTableColumn"]["tableCellLocation"]["tableStartLocation"]["tabId"],
            "plan"
        );
        assert_eq!(
            dc["deleteTableColumn"]["tableCellLocation"]["columnIndex"],
            3
        );
    }

    /// End-to-end orchestration: `run_set_table_cell` should drive the guard,
    /// emit a `deleteContentRange` + `insertText` for the addressed cell, and
    /// finalize. Asserts the requests handed to `batch_update` wire the cell's
    /// content indices correctly.
    #[tokio::test]
    async fn run_set_table_cell_wires_delete_and_insert() {
        let mut rig = TestRig::new();
        // Pre- and post-write snapshots; apply_and_finalize re-fetches after.
        let pre = snap_with_one_table();
        let mut post = snap_with_one_table();
        post.revision_id = RevisionId("r2".into());
        let queue = Arc::new(Mutex::new(vec![pre, post]));
        rig.client.expect_get().returning(move |_| {
            let mut q = queue.lock().unwrap();
            let s = if q.len() > 1 {
                q.remove(0)
            } else {
                q[0].clone()
            };
            Ok(s)
        });
        // Assert the requests contain deleteContentRange + insertText for the
        // addressed cell (content_start_index 6, content_end_index 8 → del_end 7).
        rig.client
            .expect_batch_update()
            .returning(move |_, requests, _| {
                assert_eq!(requests.len(), 2);
                assert_eq!(requests[0]["deleteContentRange"]["range"]["startIndex"], 6);
                assert_eq!(requests[0]["deleteContentRange"]["range"]["endIndex"], 7);
                assert_eq!(requests[1]["insertText"]["location"]["index"], 6);
                assert_eq!(requests[1]["insertText"]["text"], "X");
                Ok(BatchUpdateResult {
                    revision_id_after: RevisionId("r2".into()),
                    replies: vec![],
                })
            });
        let ctx = GuardContext {
            client: &rig.client,
            cache: &rig.cache,
            revisions: &rig.revisions,
            session_id: "s1",
            sa_email: None,
        };
        let result = run_set_table_cell(&ctx, &DocumentId("d".into()), 0, 0, 0, "X", None)
            .await
            .unwrap();
        assert_eq!(result.changes.len(), 1);
        assert_eq!(result.changes[0].kind, ChangeKind::Replace);
    }
}
