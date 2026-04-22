use crate::documents::domain::ir::{Block, ListItem, Run, TableCell, TableRow, WordIR};
use crate::documents::domain::patch::PatchOp;
use crate::documents::domain::{DocumentError, IdGenerator};

pub struct WordOpApplier<'a> {
    pub ids: &'a dyn IdGenerator,
}

impl<'a> WordOpApplier<'a> {
    pub fn apply(&self, ir: &mut WordIR, op: &PatchOp) -> Result<(), DocumentError> {
        match op {
            PatchOp::InsertBlock {
                before,
                after,
                block,
            } => {
                let mut new_block: Block = serde_json::from_value(block.clone())
                    .map_err(|e| invalid(op, &format!("bad block: {e}")))?;
                assign_block_ids(&mut new_block, self.ids);
                let pos = if let Some(b) = before {
                    ir.block_index(b)
                        .ok_or_else(|| invalid(op, "before block not found"))?
                } else if let Some(a) = after {
                    let i = ir
                        .block_index(a)
                        .ok_or_else(|| invalid(op, "after block not found"))?;
                    i + 1
                } else {
                    ir.document.blocks.len()
                };
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
                new_run.id = self.ids.new_run_id();
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
                let b = ir
                    .block_mut(list_block_id)
                    .ok_or_else(|| invalid(op, "list not found"))?;
                let Block::List { items, .. } = b else {
                    return Err(invalid(op, "not a list"));
                };
                let mut new_runs: Vec<Run> = Vec::new();
                for r in runs {
                    let mut run: Run = serde_json::from_value(r.clone())
                        .map_err(|e| invalid(op, &format!("bad run: {e}")))?;
                    run.id = self.ids.new_run_id();
                    new_runs.push(run);
                }
                let pos = (*at_index as usize).min(items.len());
                items.insert(
                    pos,
                    ListItem {
                        id: self.ids.new_list_item_id(),
                        runs: new_runs,
                    },
                );
            }
            PatchOp::ReplaceListItem {
                list_block_id,
                item_id,
                runs,
            } => {
                let b = ir
                    .block_mut(list_block_id)
                    .ok_or_else(|| invalid(op, "list not found"))?;
                let Block::List { items, .. } = b else {
                    return Err(invalid(op, "not a list"));
                };
                let it = items
                    .iter_mut()
                    .find(|i| &i.id == item_id)
                    .ok_or_else(|| invalid(op, "item not found"))?;
                let mut new_runs: Vec<Run> = Vec::new();
                for r in runs {
                    let mut run: Run = serde_json::from_value(r.clone())
                        .map_err(|e| invalid(op, &format!("bad run: {e}")))?;
                    run.id = self.ids.new_run_id();
                    new_runs.push(run);
                }
                it.runs = new_runs;
            }
            PatchOp::DeleteListItem {
                list_block_id,
                item_id,
            } => {
                let b = ir
                    .block_mut(list_block_id)
                    .ok_or_else(|| invalid(op, "list not found"))?;
                let Block::List { items, .. } = b else {
                    return Err(invalid(op, "not a list"));
                };
                items.retain(|i| &i.id != item_id);
            }
            PatchOp::InsertTableRow {
                table_block_id,
                before,
                after,
                cells,
            } => {
                let b = ir
                    .block_mut(table_block_id)
                    .ok_or_else(|| invalid(op, "table not found"))?;
                let Block::Table { rows, .. } = b else {
                    return Err(invalid(op, "not a table"));
                };
                let mut new_cells: Vec<TableCell> = Vec::new();
                for c in cells {
                    let mut cell: TableCell = serde_json::from_value(c.clone())
                        .map_err(|e| invalid(op, &format!("bad cell: {e}")))?;
                    for run in cell.runs.iter_mut() {
                        run.id = self.ids.new_run_id();
                    }
                    new_cells.push(cell);
                }
                let row = TableRow {
                    id: self.ids.new_row_id(),
                    cells: new_cells,
                };
                let pos = if let Some(b) = before {
                    rows.iter()
                        .position(|r| &r.id == b)
                        .ok_or_else(|| invalid(op, "before row not found"))?
                } else if let Some(a) = after {
                    let i = rows
                        .iter()
                        .position(|r| &r.id == a)
                        .ok_or_else(|| invalid(op, "after row not found"))?;
                    i + 1
                } else {
                    rows.len()
                };
                rows.insert(pos, row);
            }
            PatchOp::DeleteTableRow {
                table_block_id,
                row_id,
            } => {
                let b = ir
                    .block_mut(table_block_id)
                    .ok_or_else(|| invalid(op, "table not found"))?;
                let Block::Table { rows, .. } = b else {
                    return Err(invalid(op, "not a table"));
                };
                rows.retain(|r| &r.id != row_id);
            }
            PatchOp::UpdateTableCell {
                table_block_id,
                row_id,
                col_index,
                runs,
            } => {
                let b = ir
                    .block_mut(table_block_id)
                    .ok_or_else(|| invalid(op, "table not found"))?;
                let Block::Table { rows, .. } = b else {
                    return Err(invalid(op, "not a table"));
                };
                let row = rows
                    .iter_mut()
                    .find(|r| &r.id == row_id)
                    .ok_or_else(|| invalid(op, "row not found"))?;
                let cell = row
                    .cells
                    .get_mut(*col_index as usize)
                    .ok_or_else(|| invalid(op, "column out of range"))?;
                let mut new_runs: Vec<Run> = Vec::new();
                for r in runs {
                    let mut run: Run = serde_json::from_value(r.clone())
                        .map_err(|e| invalid(op, &format!("bad run: {e}")))?;
                    run.id = self.ids.new_run_id();
                    new_runs.push(run);
                }
                cell.runs = new_runs;
            }

            other => return Err(invalid(other, "not a Word op")),
        }
        Ok(())
    }
}

fn invalid(op: &PatchOp, reason: &str) -> DocumentError {
    DocumentError::InvalidPatchOp {
        reason: reason.to_string(),
        op: serde_json::to_value(op).unwrap_or(serde_json::Value::Null),
    }
}

fn assign_block_ids(block: &mut Block, ids: &dyn IdGenerator) {
    match block {
        Block::Heading { id, runs, .. } | Block::Paragraph { id, runs } => {
            *id = ids.new_block_id();
            for r in runs {
                r.id = ids.new_run_id();
            }
        }
        Block::List { id, items, .. } => {
            *id = ids.new_block_id();
            for it in items {
                it.id = ids.new_list_item_id();
                for r in it.runs.iter_mut() {
                    r.id = ids.new_run_id();
                }
            }
        }
        Block::Table { id, rows } => {
            *id = ids.new_block_id();
            for row in rows {
                row.id = ids.new_row_id();
                for cell in row.cells.iter_mut() {
                    for r in cell.runs.iter_mut() {
                        r.id = ids.new_run_id();
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
        applier
            .apply(
                &mut ir,
                &PatchOp::ReplaceRunText {
                    block_id: "b1".into(),
                    run_id: "r1".into(),
                    new_text: "world".into(),
                },
            )
            .unwrap();
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
        applier
            .apply(
                &mut ir,
                &PatchOp::InsertBlock {
                    before: None,
                    after: Some("b1".into()),
                    block: serde_json::json!({
                        "type": "paragraph",
                        "id": "CLIENT_SHOULD_IGNORE",
                        "runs": [{"id": "ignored", "text": "new"}]
                    }),
                },
            )
            .unwrap();
        assert_eq!(ir.document.blocks.len(), 2);
        assert!(ir.document.blocks[1].id().starts_with("blk_"));
    }
}
