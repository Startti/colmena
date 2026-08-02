use crate::documents::domain::artifact::{AssignedIds, OpOutcome};
use crate::documents::domain::ir::{Block, ListItem, Run, TableCell, TableRow, WordIR};
use crate::documents::domain::patch::PatchOp;
use crate::documents::domain::{DocumentError, IdGenerator};

pub struct WordOpApplier<'a> {
    pub ids: &'a dyn IdGenerator,
}

impl<'a> WordOpApplier<'a> {
    pub fn apply(&self, ir: &mut WordIR, op: &PatchOp) -> Result<OpOutcome, DocumentError> {
        let mut assigned = AssignedIds::default();
        match op {
            PatchOp::InsertBlock {
                before,
                after,
                block,
            } => {
                let mut new_block: Block = serde_json::from_value(block.clone())
                    .map_err(|e| invalid(op, &format!("bad block: {e}")))?;
                assign_block_ids(&mut new_block, self.ids, &mut assigned);
                let pos = resolve_insert_pos(
                    before.as_deref(),
                    after.as_deref(),
                    |b| ir.block_index(b),
                    ir.document.blocks.len(),
                    op,
                    "before block not found",
                    "after block not found",
                )?;
                ir.document.blocks.insert(pos, new_block);
            }
            PatchOp::DeleteBlock { block_id } => {
                let i = ir
                    .block_index(block_id)
                    .ok_or_else(|| invalid(op, "block not found"))?;
                ir.document.blocks.remove(i);
            }
            PatchOp::ReplaceBlock { block_id, block } => {
                let i = ir
                    .block_index(block_id)
                    .ok_or_else(|| invalid(op, "block not found"))?;
                let mut new_block: Block = serde_json::from_value(block.clone())
                    .map_err(|e| invalid(op, &format!("bad block: {e}")))?;
                set_block_id(&mut new_block, block_id);
                ir.document.blocks[i] = new_block;
            }
            PatchOp::MoveBlock {
                block_id,
                after_block_id,
            } => {
                let src = ir
                    .block_index(block_id)
                    .ok_or_else(|| invalid(op, "block not found"))?;
                let block = ir.document.blocks.remove(src);
                let dst_ref = ir
                    .block_index(after_block_id)
                    .ok_or_else(|| invalid(op, "target not found"))?;
                ir.document.blocks.insert(dst_ref + 1, block);
            }
            PatchOp::SetHeadingLevel { block_id, level } => {
                let b = ir
                    .block_mut(block_id)
                    .ok_or_else(|| invalid(op, "block not found"))?;
                if let Block::Heading { level: l, .. } = b {
                    *l = *level;
                } else {
                    return Err(invalid(op, "not a heading"));
                }
            }
            PatchOp::ReplaceRunText {
                block_id,
                run_id,
                new_text,
            } => {
                let b = ir
                    .block_mut(block_id)
                    .ok_or_else(|| invalid(op, "block not found"))?;
                let run = find_run_mut(b, run_id).ok_or_else(|| invalid(op, "run not found"))?;
                run.text = new_text.clone();
            }
            PatchOp::SetRunStyle {
                block_id,
                run_id,
                style_patch,
            } => {
                let b = ir
                    .block_mut(block_id)
                    .ok_or_else(|| invalid(op, "block not found"))?;
                let run = find_run_mut(b, run_id).ok_or_else(|| invalid(op, "run not found"))?;
                if let Some(obj) = style_patch.as_object() {
                    if let Some(v) = obj.get("bold") {
                        run.bold = v.as_bool();
                    }
                    if let Some(v) = obj.get("italic") {
                        run.italic = v.as_bool();
                    }
                    if let Some(v) = obj.get("underline") {
                        run.underline = v.as_bool();
                    }
                    if let Some(v) = obj.get("size") {
                        run.size = v.as_f64();
                    }
                    if let Some(v) = obj.get("color") {
                        run.color = v.as_str().map(|s| s.to_string());
                    }
                }
            }
            PatchOp::InsertRun {
                block_id,
                at_index,
                run,
            } => {
                let b = ir
                    .block_mut(block_id)
                    .ok_or_else(|| invalid(op, "block not found"))?;
                let mut new_run: Run = serde_json::from_value(run.clone())
                    .map_err(|e| invalid(op, &format!("bad run: {e}")))?;
                assign_fresh_run_id(&mut new_run, self.ids, &mut assigned);
                match b {
                    Block::Paragraph { runs, .. } | Block::Heading { runs, .. } => {
                        let pos = (*at_index as usize).min(runs.len());
                        runs.insert(pos, new_run);
                    }
                    _ => return Err(invalid(op, "block doesn't support runs directly")),
                }
            }
            PatchOp::DeleteRun { block_id, run_id } => {
                let b = ir
                    .block_mut(block_id)
                    .ok_or_else(|| invalid(op, "block not found"))?;
                match b {
                    Block::Paragraph { runs, .. } | Block::Heading { runs, .. } => {
                        let i = runs
                            .iter()
                            .position(|r| &r.id == run_id)
                            .ok_or_else(|| invalid(op, "run not found"))?;
                        runs.remove(i);
                    }
                    _ => return Err(invalid(op, "block doesn't support runs directly")),
                }
            }
            PatchOp::InsertListItem {
                list_block_id,
                at_index,
                runs,
            } => {
                let items = list_items_mut(ir, list_block_id, op)?;
                let new_runs = build_runs_from_json(runs, self.ids, &mut assigned, op)?;
                let pos = (*at_index as usize).min(items.len());
                let item_id = self.ids.new_list_item_id();
                assigned.list_items.push(item_id.clone());
                items.insert(
                    pos,
                    ListItem {
                        id: item_id,
                        runs: new_runs,
                    },
                );
            }
            PatchOp::ReplaceListItem {
                list_block_id,
                item_id,
                runs,
            } => {
                let items = list_items_mut(ir, list_block_id, op)?;
                let it = items
                    .iter_mut()
                    .find(|i| &i.id == item_id)
                    .ok_or_else(|| invalid(op, "item not found"))?;
                it.runs = build_runs_from_json(runs, self.ids, &mut assigned, op)?;
            }
            PatchOp::DeleteListItem {
                list_block_id,
                item_id,
            } => {
                let items = list_items_mut(ir, list_block_id, op)?;
                items.retain(|i| &i.id != item_id);
            }
            PatchOp::InsertTableRow {
                table_block_id,
                before,
                after,
                cells,
            } => {
                let rows = table_rows_mut(ir, table_block_id, op)?;
                let mut new_cells: Vec<TableCell> = Vec::new();
                for c in cells {
                    let mut cell: TableCell = serde_json::from_value(c.clone())
                        .map_err(|e| invalid(op, &format!("bad cell: {e}")))?;
                    for run in cell.runs.iter_mut() {
                        assign_fresh_run_id(run, self.ids, &mut assigned);
                    }
                    new_cells.push(cell);
                }
                let row_id = self.ids.new_row_id();
                assigned.rows.push(row_id.clone());
                let row = TableRow {
                    id: row_id,
                    cells: new_cells,
                };
                let pos = resolve_insert_pos(
                    before.as_deref(),
                    after.as_deref(),
                    |b| rows.iter().position(|r| r.id == b),
                    rows.len(),
                    op,
                    "before row not found",
                    "after row not found",
                )?;
                rows.insert(pos, row);
            }
            PatchOp::DeleteTableRow {
                table_block_id,
                row_id,
            } => {
                let rows = table_rows_mut(ir, table_block_id, op)?;
                rows.retain(|r| &r.id != row_id);
            }
            PatchOp::UpdateTableCell {
                table_block_id,
                row_id,
                col_index,
                runs,
            } => {
                let rows = table_rows_mut(ir, table_block_id, op)?;
                let row = rows
                    .iter_mut()
                    .find(|r| &r.id == row_id)
                    .ok_or_else(|| invalid(op, "row not found"))?;
                let cell = row
                    .cells
                    .get_mut(*col_index as usize)
                    .ok_or_else(|| invalid(op, "column out of range"))?;
                cell.runs = build_runs_from_json(runs, self.ids, &mut assigned, op)?;
            }

            // Excel-only and HTML-only ops are not valid for Word artifacts.
            PatchOp::SetCell { .. }
            | PatchOp::SetRange { .. }
            | PatchOp::ClearRange { .. }
            | PatchOp::InsertRow { .. }
            | PatchOp::DeleteRow { .. }
            | PatchOp::InsertColumn { .. }
            | PatchOp::DeleteColumn { .. }
            | PatchOp::AddSheet { .. }
            | PatchOp::RenameSheet { .. }
            | PatchOp::DeleteSheet { .. }
            | PatchOp::ReorderSheets { .. }
            | PatchOp::CreateTable { .. }
            | PatchOp::ResizeTable { .. }
            | PatchOp::DeleteTable { .. }
            | PatchOp::SetColumnWidth { .. }
            | PatchOp::DefineStyle { .. }
            | PatchOp::AddSlide { .. }
            | PatchOp::DeleteSlide { .. }
            | PatchOp::ReorderSlides { .. }
            | PatchOp::SetSlideLayout { .. }
            | PatchOp::SetSlideTitle { .. }
            | PatchOp::SetSlideNotes { .. }
            | PatchOp::InsertHtmlBlock { .. }
            | PatchOp::DeleteHtmlBlock { .. }
            | PatchOp::ReplaceHtmlBlock { .. }
            | PatchOp::MoveHtmlBlock { .. }
            | PatchOp::InsertHtmlTableRow { .. }
            | PatchOp::DeleteHtmlTableRow { .. }
            | PatchOp::UpdateHtmlTableCell { .. }
            | PatchOp::InsertHtmlListItem { .. }
            | PatchOp::DeleteHtmlListItem { .. }
            | PatchOp::UpdateHtmlListItem { .. }
            | PatchOp::SetTheme { .. }
            | PatchOp::SetDocProps { .. }
            | PatchOp::SetFooter { .. } => {
                return Err(invalid(op, "not a Word op"));
            }
        }
        Ok(OpOutcome {
            assigned_ids: assigned,
        })
    }
}

fn invalid(op: &PatchOp, reason: &str) -> DocumentError {
    DocumentError::InvalidPatchOp {
        reason: reason.to_string(),
        op: serde_json::to_value(op).unwrap_or(serde_json::Value::Null),
    }
}

// ---- Intra-kind helpers (Stage 2, finding #39 follow-up slice) ----
// These extract 3 patterns repeated across the op arms above: run
// deserialize-assign-push, the List/Table block type-guard, and the
// before/after -> positional-index resolution. Semantics and error text
// are byte-identical to the pre-extraction inline arms — see the parity
// net tests in this module's `tests` block.

/// Assign a fresh server-side run id to `run` and record it in `assigned`.
fn assign_fresh_run_id(run: &mut Run, ids: &dyn IdGenerator, assigned: &mut AssignedIds) {
    run.id = ids.new_run_id();
    assigned.runs.push(run.id.clone());
}

/// Deserialize each raw JSON value in `raw` into a [`Run`], assign it a
/// fresh server-side id, and collect the results in order. Preserves the
/// exact "bad run: {e}" error wording of the original inline arms.
fn build_runs_from_json(
    raw: &[serde_json::Value],
    ids: &dyn IdGenerator,
    assigned: &mut AssignedIds,
    op: &PatchOp,
) -> Result<Vec<Run>, DocumentError> {
    raw.iter()
        .map(|r| {
            let mut run: Run = serde_json::from_value(r.clone())
                .map_err(|e| invalid(op, &format!("bad run: {e}")))?;
            assign_fresh_run_id(&mut run, ids, assigned);
            Ok(run)
        })
        .collect()
}

/// Look up `list_block_id` and assert it is a [`Block::List`], returning a
/// mutable reference to its items. Preserves the exact "list not found" /
/// "not a list" error text of the original inline `let Block::List { .. }
/// = b else { ... }` guards.
fn list_items_mut<'a>(
    ir: &'a mut WordIR,
    list_block_id: &str,
    op: &PatchOp,
) -> Result<&'a mut Vec<ListItem>, DocumentError> {
    let b = ir
        .block_mut(list_block_id)
        .ok_or_else(|| invalid(op, "list not found"))?;
    let Block::List { items, .. } = b else {
        return Err(invalid(op, "not a list"));
    };
    Ok(items)
}

/// Look up `table_block_id` and assert it is a [`Block::Table`], returning
/// a mutable reference to its rows. Preserves the exact "table not found" /
/// "not a table" error text of the original inline `let Block::Table { .. }
/// = b else { ... }` guards.
fn table_rows_mut<'a>(
    ir: &'a mut WordIR,
    table_block_id: &str,
    op: &PatchOp,
) -> Result<&'a mut Vec<TableRow>, DocumentError> {
    let b = ir
        .block_mut(table_block_id)
        .ok_or_else(|| invalid(op, "table not found"))?;
    let Block::Table { rows, .. } = b else {
        return Err(invalid(op, "not a table"));
    };
    Ok(rows)
}

/// Resolve a `before`/`after` anchor pair to a positional index: `before`
/// resolves to the target's own index, `after` resolves to the index right
/// after the target, and the default (neither provided) appends at `len`.
/// Preserves the exact not-found error text of the original inline
/// if/else-if arms — callers supply their own wording (e.g. "before block
/// not found" vs. "before row not found") since it differs per call site.
fn resolve_insert_pos(
    before: Option<&str>,
    after: Option<&str>,
    find: impl Fn(&str) -> Option<usize>,
    len: usize,
    op: &PatchOp,
    before_not_found: &str,
    after_not_found: &str,
) -> Result<usize, DocumentError> {
    if let Some(b) = before {
        find(b).ok_or_else(|| invalid(op, before_not_found))
    } else if let Some(a) = after {
        let i = find(a).ok_or_else(|| invalid(op, after_not_found))?;
        Ok(i + 1)
    } else {
        Ok(len)
    }
}

fn assign_block_ids(block: &mut Block, ids: &dyn IdGenerator, out: &mut AssignedIds) {
    match block {
        Block::Heading { id, runs, .. } | Block::Paragraph { id, runs } => {
            *id = ids.new_block_id();
            out.block = Some(id.clone());
            for r in runs {
                r.id = ids.new_run_id();
                out.runs.push(r.id.clone());
            }
        }
        Block::List { id, items, .. } => {
            *id = ids.new_block_id();
            out.block = Some(id.clone());
            for it in items {
                it.id = ids.new_list_item_id();
                out.list_items.push(it.id.clone());
                for r in it.runs.iter_mut() {
                    r.id = ids.new_run_id();
                    out.runs.push(r.id.clone());
                }
            }
        }
        Block::Table { id, rows } => {
            *id = ids.new_block_id();
            out.block = Some(id.clone());
            for row in rows {
                row.id = ids.new_row_id();
                out.rows.push(row.id.clone());
                for cell in row.cells.iter_mut() {
                    for r in cell.runs.iter_mut() {
                        r.id = ids.new_run_id();
                        out.runs.push(r.id.clone());
                    }
                }
            }
        }
    }
}

fn set_block_id(block: &mut Block, new_id: &str) {
    match block {
        Block::Heading { id, .. }
        | Block::Paragraph { id, .. }
        | Block::List { id, .. }
        | Block::Table { id, .. } => *id = new_id.to_string(),
    }
}

fn find_run_mut<'a>(block: &'a mut Block, run_id: &str) -> Option<&'a mut Run> {
    match block {
        Block::Heading { runs, .. } | Block::Paragraph { runs, .. } => {
            runs.iter_mut().find(|r| r.id == run_id)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::documents::domain::ir::{Block, Run};
    use crate::documents::infrastructure::ids::CountingIdGenerator;

    fn base_ir() -> WordIR {
        let mut ir = WordIR::empty("x", "v1");
        ir.document.blocks.push(Block::Paragraph {
            id: "b1".into(),
            runs: vec![Run {
                id: "r1".into(),
                text: "hello".into(),
                bold: None,
                italic: None,
                underline: None,
                size: None,
                color: None,
            }],
        });
        ir
    }

    #[test]
    fn replace_run_text_updates() {
        let ids = CountingIdGenerator::default();
        let applier = WordOpApplier { ids: &ids };
        let mut ir = base_ir();
        let out = applier
            .apply(
                &mut ir,
                &PatchOp::ReplaceRunText {
                    block_id: "b1".into(),
                    run_id: "r1".into(),
                    new_text: "world".into(),
                },
            )
            .unwrap();
        assert!(out.assigned_ids.is_empty());
        if let Block::Paragraph { runs, .. } = &ir.document.blocks[0] {
            assert_eq!(runs[0].text, "world");
        } else {
            panic!();
        }
    }

    #[test]
    fn insert_block_assigns_server_id() {
        let ids = CountingIdGenerator::default();
        let applier = WordOpApplier { ids: &ids };
        let mut ir = base_ir();
        let out = applier
            .apply(
                &mut ir,
                &PatchOp::InsertBlock {
                    before: None,
                    after: Some("b1".into()),
                    block: serde_json::json!({
                        "type": "paragraph",
                        "runs": [{"text": "new"}, {"text": "second"}]
                    }),
                },
            )
            .unwrap();
        assert_eq!(ir.document.blocks.len(), 2);
        assert!(ir.document.blocks[1].id().starts_with("blk_"));
        assert_eq!(out.assigned_ids.block.as_deref(), Some("blk_01"));
        assert_eq!(out.assigned_ids.runs, vec!["run_01", "run_02"]);
    }

    #[test]
    fn insert_list_item_reports_ids() {
        let ids = CountingIdGenerator::default();
        let applier = WordOpApplier { ids: &ids };
        let mut ir = base_ir();
        ir.document.blocks.push(Block::List {
            id: "lst".into(),
            style: crate::documents::domain::ir::ListStyle::Bullet,
            items: vec![],
        });
        let out = applier
            .apply(
                &mut ir,
                &PatchOp::InsertListItem {
                    list_block_id: "lst".into(),
                    at_index: 0,
                    runs: vec![serde_json::json!({"text": "item"})],
                },
            )
            .unwrap();
        assert_eq!(out.assigned_ids.list_items, vec!["li_01"]);
        assert_eq!(out.assigned_ids.runs, vec!["run_01"]);
    }

    // ---- Phase 0 parity net (finding #39, follow-up slice) ----
    // Characterization tests pinning the exact `invalid(op, ...)` error text
    // and the before/after -> positional-index resolution BEFORE the
    // intra-kind helper extraction (Stage 2). Must pass unmodified against
    // both pre- and post-extraction code.

    fn table_ir_with_rows(row_ids: &[&str]) -> WordIR {
        let mut ir = base_ir();
        ir.document.blocks.push(Block::Table {
            id: "tbl".into(),
            rows: row_ids
                .iter()
                .map(|id| TableRow {
                    id: (*id).to_string(),
                    cells: vec![],
                })
                .collect(),
        });
        ir
    }

    // -- List/Table block type-guard rejections --

    #[test]
    fn insert_list_item_on_non_list_block_returns_not_a_list() {
        let ids = CountingIdGenerator::default();
        let applier = WordOpApplier { ids: &ids };
        let mut ir = base_ir(); // "b1" is a Paragraph, not a List
        let err = applier
            .apply(
                &mut ir,
                &PatchOp::InsertListItem {
                    list_block_id: "b1".into(),
                    at_index: 0,
                    runs: vec![serde_json::json!({"text": "item"})],
                },
            )
            .unwrap_err();
        match err {
            DocumentError::InvalidPatchOp { reason, .. } => assert_eq!(reason, "not a list"),
            other => panic!("expected InvalidPatchOp, got {other:?}"),
        }
    }

    #[test]
    fn delete_list_item_on_non_list_block_returns_not_a_list() {
        let ids = CountingIdGenerator::default();
        let applier = WordOpApplier { ids: &ids };
        let mut ir = base_ir();
        let err = applier
            .apply(
                &mut ir,
                &PatchOp::DeleteListItem {
                    list_block_id: "b1".into(),
                    item_id: "li_1".into(),
                },
            )
            .unwrap_err();
        match err {
            DocumentError::InvalidPatchOp { reason, .. } => assert_eq!(reason, "not a list"),
            other => panic!("expected InvalidPatchOp, got {other:?}"),
        }
    }

    #[test]
    fn insert_table_row_on_non_table_block_returns_not_a_table() {
        let ids = CountingIdGenerator::default();
        let applier = WordOpApplier { ids: &ids };
        let mut ir = base_ir(); // "b1" is a Paragraph, not a Table
        let err = applier
            .apply(
                &mut ir,
                &PatchOp::InsertTableRow {
                    table_block_id: "b1".into(),
                    before: None,
                    after: None,
                    cells: vec![],
                },
            )
            .unwrap_err();
        match err {
            DocumentError::InvalidPatchOp { reason, .. } => assert_eq!(reason, "not a table"),
            other => panic!("expected InvalidPatchOp, got {other:?}"),
        }
    }

    #[test]
    fn update_table_cell_on_non_table_block_returns_not_a_table() {
        let ids = CountingIdGenerator::default();
        let applier = WordOpApplier { ids: &ids };
        let mut ir = base_ir();
        let err = applier
            .apply(
                &mut ir,
                &PatchOp::UpdateTableCell {
                    table_block_id: "b1".into(),
                    row_id: "r1".into(),
                    col_index: 0,
                    runs: vec![],
                },
            )
            .unwrap_err();
        match err {
            DocumentError::InvalidPatchOp { reason, .. } => assert_eq!(reason, "not a table"),
            other => panic!("expected InvalidPatchOp, got {other:?}"),
        }
    }

    // -- before/after -> positional-index resolution: InsertBlock --

    #[test]
    fn insert_block_before_target_positions_new_block_at_target_index() {
        let ids = CountingIdGenerator::default();
        let applier = WordOpApplier { ids: &ids };
        let mut ir = base_ir(); // single block "b1"
        applier
            .apply(
                &mut ir,
                &PatchOp::InsertBlock {
                    before: Some("b1".into()),
                    after: None,
                    block: serde_json::json!({"type": "paragraph", "runs": [{"text": "new"}]}),
                },
            )
            .unwrap();
        assert_eq!(ir.document.blocks.len(), 2);
        assert!(ir.document.blocks[0].id().starts_with("blk_"));
        assert_eq!(ir.document.blocks[1].id(), "b1");
    }

    #[test]
    fn insert_block_after_target_positions_new_block_right_after() {
        let ids = CountingIdGenerator::default();
        let applier = WordOpApplier { ids: &ids };
        let mut ir = base_ir();
        applier
            .apply(
                &mut ir,
                &PatchOp::InsertBlock {
                    before: None,
                    after: Some("b1".into()),
                    block: serde_json::json!({"type": "paragraph", "runs": [{"text": "new"}]}),
                },
            )
            .unwrap();
        assert_eq!(ir.document.blocks.len(), 2);
        assert_eq!(ir.document.blocks[0].id(), "b1");
        assert!(ir.document.blocks[1].id().starts_with("blk_"));
    }

    #[test]
    fn insert_block_default_appends_at_document_end() {
        let ids = CountingIdGenerator::default();
        let applier = WordOpApplier { ids: &ids };
        let mut ir = base_ir();
        ir.document.blocks.push(Block::Paragraph {
            id: "b2".into(),
            runs: vec![],
        });
        applier
            .apply(
                &mut ir,
                &PatchOp::InsertBlock {
                    before: None,
                    after: None,
                    block: serde_json::json!({"type": "paragraph", "runs": [{"text": "new"}]}),
                },
            )
            .unwrap();
        assert_eq!(ir.document.blocks.len(), 3);
        assert_eq!(ir.document.blocks[0].id(), "b1");
        assert_eq!(ir.document.blocks[1].id(), "b2");
        assert!(ir.document.blocks[2].id().starts_with("blk_"));
    }

    #[test]
    fn insert_block_before_missing_target_returns_exact_reason() {
        let ids = CountingIdGenerator::default();
        let applier = WordOpApplier { ids: &ids };
        let mut ir = base_ir();
        let err = applier
            .apply(
                &mut ir,
                &PatchOp::InsertBlock {
                    before: Some("missing".into()),
                    after: None,
                    block: serde_json::json!({"type": "paragraph", "runs": []}),
                },
            )
            .unwrap_err();
        match err {
            DocumentError::InvalidPatchOp { reason, .. } => {
                assert_eq!(reason, "before block not found")
            }
            other => panic!("expected InvalidPatchOp, got {other:?}"),
        }
    }

    // -- before/after -> positional-index resolution: InsertTableRow --

    #[test]
    fn insert_table_row_before_target_positions_row_at_target_index() {
        let ids = CountingIdGenerator::default();
        let applier = WordOpApplier { ids: &ids };
        let mut ir = table_ir_with_rows(&["r1", "r2"]);
        applier
            .apply(
                &mut ir,
                &PatchOp::InsertTableRow {
                    table_block_id: "tbl".into(),
                    before: Some("r2".into()),
                    after: None,
                    cells: vec![],
                },
            )
            .unwrap();
        let Block::Table { rows, .. } = &ir.document.blocks[1] else {
            panic!("expected table block");
        };
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].id, "r1");
        assert!(rows[1].id.starts_with("row_"));
        assert_eq!(rows[2].id, "r2");
    }

    #[test]
    fn insert_table_row_after_target_positions_row_right_after() {
        let ids = CountingIdGenerator::default();
        let applier = WordOpApplier { ids: &ids };
        let mut ir = table_ir_with_rows(&["r1", "r2"]);
        applier
            .apply(
                &mut ir,
                &PatchOp::InsertTableRow {
                    table_block_id: "tbl".into(),
                    before: None,
                    after: Some("r1".into()),
                    cells: vec![],
                },
            )
            .unwrap();
        let Block::Table { rows, .. } = &ir.document.blocks[1] else {
            panic!("expected table block");
        };
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].id, "r1");
        assert!(rows[1].id.starts_with("row_"));
        assert_eq!(rows[2].id, "r2");
    }

    #[test]
    fn insert_table_row_default_appends_at_table_end() {
        let ids = CountingIdGenerator::default();
        let applier = WordOpApplier { ids: &ids };
        let mut ir = table_ir_with_rows(&["r1", "r2"]);
        applier
            .apply(
                &mut ir,
                &PatchOp::InsertTableRow {
                    table_block_id: "tbl".into(),
                    before: None,
                    after: None,
                    cells: vec![],
                },
            )
            .unwrap();
        let Block::Table { rows, .. } = &ir.document.blocks[1] else {
            panic!("expected table block");
        };
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].id, "r1");
        assert_eq!(rows[1].id, "r2");
        assert!(rows[2].id.starts_with("row_"));
    }

    #[test]
    fn insert_table_row_after_missing_target_returns_exact_reason() {
        let ids = CountingIdGenerator::default();
        let applier = WordOpApplier { ids: &ids };
        let mut ir = table_ir_with_rows(&["r1"]);
        let err = applier
            .apply(
                &mut ir,
                &PatchOp::InsertTableRow {
                    table_block_id: "tbl".into(),
                    before: None,
                    after: Some("missing".into()),
                    cells: vec![],
                },
            )
            .unwrap_err();
        match err {
            DocumentError::InvalidPatchOp { reason, .. } => {
                assert_eq!(reason, "after row not found")
            }
            other => panic!("expected InvalidPatchOp, got {other:?}"),
        }
    }
}
