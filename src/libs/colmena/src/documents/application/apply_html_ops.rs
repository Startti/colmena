//! HtmlOpApplier — mutates HtmlIR for each PatchOp (slide-level + block-level).
//!
//! Pure to ports: depends only on IdGenerator from domain.

use crate::documents::domain::artifact::{AssignedIds, OpOutcome};
use crate::documents::domain::ir::html::{Block, HtmlIR, ImageSrc, Slide, SlideLayout, VideoSrc};
use crate::documents::domain::patch::PatchOp;
use crate::documents::domain::{DocumentError, IdGenerator};

pub struct HtmlOpApplier<'a> {
    pub ids: &'a dyn IdGenerator,
}

impl<'a> HtmlOpApplier<'a> {
    pub fn apply(&self, ir: &mut HtmlIR, op: &PatchOp) -> Result<OpOutcome, DocumentError> {
        match op {
            PatchOp::AddSlide {
                layout,
                at_index,
                title,
                subtitle,
            } => self.add_slide(ir, *layout, *at_index, title.clone(), subtitle.clone()),
            PatchOp::DeleteSlide { slide_id } => self.delete_slide(ir, slide_id),
            PatchOp::ReorderSlides { order } => self.reorder_slides(ir, order),
            PatchOp::SetSlideLayout { slide_id, layout } => {
                self.set_slide_layout(ir, slide_id, *layout)
            }
            PatchOp::SetSlideTitle {
                slide_id,
                title,
                subtitle,
            } => self.set_slide_title(ir, slide_id, title.clone(), subtitle.clone()),
            PatchOp::SetSlideNotes { slide_id, notes } => {
                self.set_slide_notes(ir, slide_id, notes.clone())
            }
            PatchOp::InsertHtmlBlock {
                slide_id,
                before,
                after,
                block,
            } => self.insert_html_block(ir, slide_id, before.as_deref(), after.as_deref(), block),
            PatchOp::DeleteHtmlBlock { slide_id, block_id } => {
                self.delete_html_block(ir, slide_id, block_id)
            }
            PatchOp::ReplaceHtmlBlock {
                slide_id,
                block_id,
                block,
            } => self.replace_html_block(ir, slide_id, block_id, block),
            PatchOp::MoveHtmlBlock {
                slide_id,
                block_id,
                after_block_id,
            } => self.move_html_block(ir, slide_id, block_id, after_block_id),
            PatchOp::InsertHtmlTableRow {
                slide_id,
                table_block_id,
                before,
                after,
                cells,
            } => self.insert_html_table_row(
                ir,
                slide_id,
                table_block_id,
                before.as_deref(),
                after.as_deref(),
                cells,
            ),
            PatchOp::DeleteHtmlTableRow {
                slide_id,
                table_block_id,
                row_id,
            } => self.delete_html_table_row(ir, slide_id, table_block_id, row_id),
            PatchOp::UpdateHtmlTableCell {
                slide_id,
                table_block_id,
                row_id,
                col_index,
                cell,
            } => {
                self.update_html_table_cell(ir, slide_id, table_block_id, row_id, *col_index, cell)
            }
            PatchOp::InsertHtmlListItem {
                slide_id,
                list_block_id,
                at_index,
                runs,
            } => self.insert_html_list_item(ir, slide_id, list_block_id, *at_index, runs),
            PatchOp::DeleteHtmlListItem {
                slide_id,
                list_block_id,
                item_id,
            } => self.delete_html_list_item(ir, slide_id, list_block_id, item_id),
            PatchOp::UpdateHtmlListItem {
                slide_id,
                list_block_id,
                item_id,
                runs,
            } => self.update_html_list_item(ir, slide_id, list_block_id, item_id, runs),
            PatchOp::SetTheme { theme } => {
                ir.theme = *theme;
                Ok(OpOutcome::default())
            }
            PatchOp::SetDocProps {
                title,
                author,
                date,
                locale,
            } => {
                if let Some(t) = title.clone() {
                    ir.doc_props.title = Some(t);
                }
                if let Some(a) = author.clone() {
                    ir.doc_props.author = Some(a);
                }
                if let Some(d) = date.clone() {
                    ir.doc_props.date = Some(d);
                }
                if let Some(l) = locale {
                    ir.doc_props.locale = *l;
                }
                Ok(OpOutcome::default())
            }
            PatchOp::SetFooter { footer } => {
                ir.footer = footer.clone();
                Ok(OpOutcome::default())
            }
            _ => Err(DocumentError::InvalidPatchOp {
                reason: "op not applicable to HTML kind".into(),
                op: serde_json::to_value(op).unwrap_or_default(),
            }),
        }
    }

    fn add_slide(
        &self,
        ir: &mut HtmlIR,
        layout: SlideLayout,
        at_index: Option<u32>,
        title: Option<String>,
        subtitle: Option<String>,
    ) -> Result<OpOutcome, DocumentError> {
        if matches!(layout, SlideLayout::Title | SlideLayout::SectionDivider)
            && title.as_deref().unwrap_or("").trim().is_empty()
        {
            return Err(DocumentError::IRValidationFailed {
                path: "/op/add_slide".into(),
                reason: format!("layout {:?} requires non-empty title", layout),
            });
        }
        let id = self.ids.new_slide_id();
        let new_slide = Slide {
            id: id.clone(),
            layout,
            title,
            subtitle,
            notes: None,
            blocks: vec![],
        };
        let idx = at_index
            .map(|i| (i as usize).min(ir.slides.len()))
            .unwrap_or(ir.slides.len());
        ir.slides.insert(idx, new_slide);
        Ok(OpOutcome {
            assigned_ids: AssignedIds {
                slide: Some(id),
                ..Default::default()
            },
        })
    }

    fn delete_slide(&self, ir: &mut HtmlIR, slide_id: &str) -> Result<OpOutcome, DocumentError> {
        if ir.slides.len() <= 1 {
            return Err(DocumentError::IRValidationFailed {
                path: "/slides".into(),
                reason: "cannot delete last slide".into(),
            });
        }
        let pos = ir.slides.iter().position(|s| s.id == slide_id);
        match pos {
            Some(i) => {
                ir.slides.remove(i);
                recompute_assets_referenced(ir);
                Ok(OpOutcome::default())
            }
            None => Err(DocumentError::IRValidationFailed {
                path: format!("/slides/{slide_id}"),
                reason: "slide not found".into(),
            }),
        }
    }

    fn reorder_slides(
        &self,
        ir: &mut HtmlIR,
        order: &[String],
    ) -> Result<OpOutcome, DocumentError> {
        if order.len() != ir.slides.len() {
            return Err(DocumentError::IRValidationFailed {
                path: "/op/reorder_slides".into(),
                reason: format!(
                    "order length {} != current slides {}",
                    order.len(),
                    ir.slides.len()
                ),
            });
        }
        let mut new_slides = Vec::with_capacity(order.len());
        for id in order {
            let pos = ir.slides.iter().position(|s| s.id == *id).ok_or_else(|| {
                DocumentError::IRValidationFailed {
                    path: "/op/reorder_slides".into(),
                    reason: format!("unknown slide_id {id}"),
                }
            })?;
            new_slides.push(ir.slides.remove(pos));
        }
        ir.slides = new_slides;
        Ok(OpOutcome::default())
    }

    fn set_slide_layout(
        &self,
        ir: &mut HtmlIR,
        slide_id: &str,
        layout: SlideLayout,
    ) -> Result<OpOutcome, DocumentError> {
        let slide = find_slide_mut(ir, slide_id)?;
        if matches!(layout, SlideLayout::Title | SlideLayout::SectionDivider)
            && slide.title.as_deref().unwrap_or("").trim().is_empty()
        {
            return Err(DocumentError::IRValidationFailed {
                path: format!("/slides/{slide_id}"),
                reason: format!("layout {:?} requires non-empty title", layout),
            });
        }
        slide.layout = layout;
        Ok(OpOutcome::default())
    }

    fn set_slide_title(
        &self,
        ir: &mut HtmlIR,
        slide_id: &str,
        title: Option<String>,
        subtitle: Option<String>,
    ) -> Result<OpOutcome, DocumentError> {
        let slide = find_slide_mut(ir, slide_id)?;
        slide.title = title;
        slide.subtitle = subtitle;
        Ok(OpOutcome::default())
    }

    fn set_slide_notes(
        &self,
        ir: &mut HtmlIR,
        slide_id: &str,
        notes: Option<String>,
    ) -> Result<OpOutcome, DocumentError> {
        let slide = find_slide_mut(ir, slide_id)?;
        slide.notes = notes;
        Ok(OpOutcome::default())
    }

    fn insert_html_block(
        &self,
        ir: &mut HtmlIR,
        slide_id: &str,
        before: Option<&str>,
        after: Option<&str>,
        block_json: &serde_json::Value,
    ) -> Result<OpOutcome, DocumentError> {
        if before.is_some() && after.is_some() {
            return Err(DocumentError::InvalidPatchOp {
                reason: "exactly one of before/after must be provided".into(),
                op: serde_json::json!({}),
            });
        }
        let mut block_json = block_json.clone();
        let new_id = self.ids.new_block_id();
        if let Some(obj) = block_json.as_object_mut() {
            obj.insert("id".into(), serde_json::json!(new_id));
            self.assign_nested_ids(obj);
        }
        let block: Block =
            serde_json::from_value(block_json).map_err(|e| DocumentError::IRValidationFailed {
                path: "/op/insert_html_block/block".into(),
                reason: e.to_string(),
            })?;
        let slide = find_slide_mut(ir, slide_id)?;
        let pos = match (before, after) {
            (Some(b), None) => slide.blocks.iter().position(|x| block_id_of(x) == b),
            (None, Some(a)) => slide
                .blocks
                .iter()
                .position(|x| block_id_of(x) == a)
                .map(|i| i + 1),
            _ => Some(slide.blocks.len()),
        }
        .ok_or_else(|| DocumentError::IRValidationFailed {
            path: format!("/slides/{slide_id}/blocks"),
            reason: "before/after block_id not found".into(),
        })?;
        slide.blocks.insert(pos, block);
        recompute_assets_referenced(ir);
        let mut ids = crate::documents::domain::artifact::AssignedIds::default();
        ids.block = Some(new_id);
        Ok(OpOutcome { assigned_ids: ids })
    }

    fn delete_html_block(
        &self,
        ir: &mut HtmlIR,
        slide_id: &str,
        block_id: &str,
    ) -> Result<OpOutcome, DocumentError> {
        let slide = find_slide_mut(ir, slide_id)?;
        let pos = slide
            .blocks
            .iter()
            .position(|x| block_id_of(x) == block_id)
            .ok_or_else(|| DocumentError::IRValidationFailed {
                path: format!("/slides/{slide_id}/blocks/{block_id}"),
                reason: "block not found".into(),
            })?;
        slide.blocks.remove(pos);
        recompute_assets_referenced(ir);
        Ok(OpOutcome::default())
    }

    fn replace_html_block(
        &self,
        ir: &mut HtmlIR,
        slide_id: &str,
        block_id: &str,
        block_json: &serde_json::Value,
    ) -> Result<OpOutcome, DocumentError> {
        let mut block_json = block_json.clone();
        if let Some(obj) = block_json.as_object_mut() {
            obj.insert("id".into(), serde_json::json!(block_id));
            self.assign_nested_ids(obj);
        }
        let new_block: Block =
            serde_json::from_value(block_json).map_err(|e| DocumentError::IRValidationFailed {
                path: "/op/replace_html_block/block".into(),
                reason: e.to_string(),
            })?;
        let slide = find_slide_mut(ir, slide_id)?;
        let pos = slide
            .blocks
            .iter()
            .position(|x| block_id_of(x) == block_id)
            .ok_or_else(|| DocumentError::IRValidationFailed {
                path: format!("/slides/{slide_id}/blocks/{block_id}"),
                reason: "block not found".into(),
            })?;
        slide.blocks[pos] = new_block;
        recompute_assets_referenced(ir);
        Ok(OpOutcome::default())
    }

    fn move_html_block(
        &self,
        ir: &mut HtmlIR,
        slide_id: &str,
        block_id: &str,
        after_block_id: &str,
    ) -> Result<OpOutcome, DocumentError> {
        let slide = find_slide_mut(ir, slide_id)?;
        let from = slide
            .blocks
            .iter()
            .position(|x| block_id_of(x) == block_id)
            .ok_or_else(|| DocumentError::IRValidationFailed {
                path: format!("/slides/{slide_id}/blocks/{block_id}"),
                reason: "block not found".into(),
            })?;
        let removed = slide.blocks.remove(from);
        let after_pos = slide
            .blocks
            .iter()
            .position(|x| block_id_of(x) == after_block_id);
        let insert_at = match after_pos {
            Some(i) => i + 1,
            None => {
                slide.blocks.insert(from, removed);
                return Err(DocumentError::IRValidationFailed {
                    path: format!("/slides/{slide_id}/blocks/{after_block_id}"),
                    reason: "after_block_id not found".into(),
                });
            }
        };
        slide.blocks.insert(insert_at, removed);
        Ok(OpOutcome::default())
    }

    fn insert_html_table_row(
        &self,
        ir: &mut HtmlIR,
        slide_id: &str,
        table_block_id: &str,
        before: Option<&str>,
        after: Option<&str>,
        cells: &[serde_json::Value],
    ) -> Result<OpOutcome, DocumentError> {
        let row_id = self.ids.new_row_id();
        let parsed_cells: Vec<_> = cells
            .iter()
            .map(|c| serde_json::from_value(c.clone()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| DocumentError::IRValidationFailed {
                path: "/op/insert_html_table_row/cells".into(),
                reason: e.to_string(),
            })?;
        let new_row = crate::documents::domain::ir::html::TableRow {
            id: row_id.clone(),
            cells: parsed_cells,
        };
        let slide = find_slide_mut(ir, slide_id)?;
        let block = slide
            .blocks
            .iter_mut()
            .find(|b| block_id_of(b) == table_block_id)
            .ok_or_else(|| DocumentError::IRValidationFailed {
                path: format!("/slides/{slide_id}/blocks/{table_block_id}"),
                reason: "table block not found".into(),
            })?;
        if let Block::Table { rows, .. } = block {
            let pos = match (before, after) {
                (Some(b), None) => rows.iter().position(|r| r.id == b),
                (None, Some(a)) => rows.iter().position(|r| r.id == a).map(|i| i + 1),
                _ => Some(rows.len()),
            }
            .ok_or_else(|| DocumentError::IRValidationFailed {
                path: "/op/insert_html_table_row".into(),
                reason: "row_id not found".into(),
            })?;
            rows.insert(pos, new_row);
            let mut ids = crate::documents::domain::artifact::AssignedIds::default();
            ids.rows = vec![row_id];
            Ok(OpOutcome { assigned_ids: ids })
        } else {
            Err(DocumentError::IRValidationFailed {
                path: format!("/slides/{slide_id}/blocks/{table_block_id}"),
                reason: "not a table block".into(),
            })
        }
    }

    fn delete_html_table_row(
        &self,
        ir: &mut HtmlIR,
        slide_id: &str,
        table_block_id: &str,
        row_id: &str,
    ) -> Result<OpOutcome, DocumentError> {
        let slide = find_slide_mut(ir, slide_id)?;
        let block = slide
            .blocks
            .iter_mut()
            .find(|b| block_id_of(b) == table_block_id)
            .ok_or_else(|| DocumentError::IRValidationFailed {
                path: format!("/slides/{slide_id}/blocks/{table_block_id}"),
                reason: "table block not found".into(),
            })?;
        if let Block::Table { rows, .. } = block {
            let pos = rows.iter().position(|r| r.id == row_id).ok_or_else(|| {
                DocumentError::IRValidationFailed {
                    path: format!("/slides/{slide_id}/blocks/{table_block_id}/rows/{row_id}"),
                    reason: "row not found".into(),
                }
            })?;
            rows.remove(pos);
            Ok(OpOutcome::default())
        } else {
            Err(DocumentError::IRValidationFailed {
                path: format!("/slides/{slide_id}/blocks/{table_block_id}"),
                reason: "not a table block".into(),
            })
        }
    }

    fn update_html_table_cell(
        &self,
        ir: &mut HtmlIR,
        slide_id: &str,
        table_block_id: &str,
        row_id: &str,
        col_index: u32,
        cell_json: &serde_json::Value,
    ) -> Result<OpOutcome, DocumentError> {
        let cell: crate::documents::domain::ir::html::TableCell =
            serde_json::from_value(cell_json.clone()).map_err(|e| {
                DocumentError::IRValidationFailed {
                    path: "/op/update_html_table_cell/cell".into(),
                    reason: e.to_string(),
                }
            })?;
        let slide = find_slide_mut(ir, slide_id)?;
        let block = slide
            .blocks
            .iter_mut()
            .find(|b| block_id_of(b) == table_block_id)
            .ok_or_else(|| DocumentError::IRValidationFailed {
                path: format!("/slides/{slide_id}/blocks/{table_block_id}"),
                reason: "table block not found".into(),
            })?;
        if let Block::Table { rows, .. } = block {
            let row = rows.iter_mut().find(|r| r.id == row_id).ok_or_else(|| {
                DocumentError::IRValidationFailed {
                    path: format!("/slides/{slide_id}/blocks/{table_block_id}/rows/{row_id}"),
                    reason: "row not found".into(),
                }
            })?;
            let idx = col_index as usize;
            if idx >= row.cells.len() {
                return Err(DocumentError::IRValidationFailed {
                    path: format!("/.../rows/{row_id}/cells/{col_index}"),
                    reason: format!("col_index out of range (row has {} cells)", row.cells.len()),
                });
            }
            row.cells[idx] = cell;
            Ok(OpOutcome::default())
        } else {
            Err(DocumentError::IRValidationFailed {
                path: format!("/slides/{slide_id}/blocks/{table_block_id}"),
                reason: "not a table block".into(),
            })
        }
    }

    fn insert_html_list_item(
        &self,
        ir: &mut HtmlIR,
        slide_id: &str,
        list_block_id: &str,
        at_index: u32,
        runs: &[serde_json::Value],
    ) -> Result<OpOutcome, DocumentError> {
        let item_id = self.ids.new_list_item_id();
        let parsed_runs: Vec<_> = runs
            .iter()
            .map(|r| {
                let mut rv = r.clone();
                if let Some(o) = rv.as_object_mut() {
                    o.insert("id".into(), serde_json::json!(self.ids.new_run_id()));
                }
                serde_json::from_value::<crate::documents::domain::ir::html::Run>(rv)
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| DocumentError::IRValidationFailed {
                path: "/op/insert_html_list_item/runs".into(),
                reason: e.to_string(),
            })?;
        let new_item = crate::documents::domain::ir::html::ListItem {
            id: item_id.clone(),
            runs: parsed_runs,
        };
        let slide = find_slide_mut(ir, slide_id)?;
        let block = slide
            .blocks
            .iter_mut()
            .find(|b| block_id_of(b) == list_block_id)
            .ok_or_else(|| DocumentError::IRValidationFailed {
                path: format!("/slides/{slide_id}/blocks/{list_block_id}"),
                reason: "list block not found".into(),
            })?;
        if let Block::List { items, .. } = block {
            let pos = (at_index as usize).min(items.len());
            items.insert(pos, new_item);
            let mut ids = crate::documents::domain::artifact::AssignedIds::default();
            ids.list_items = vec![item_id];
            Ok(OpOutcome { assigned_ids: ids })
        } else {
            Err(DocumentError::IRValidationFailed {
                path: format!("/slides/{slide_id}/blocks/{list_block_id}"),
                reason: "not a list block".into(),
            })
        }
    }

    fn delete_html_list_item(
        &self,
        ir: &mut HtmlIR,
        slide_id: &str,
        list_block_id: &str,
        item_id: &str,
    ) -> Result<OpOutcome, DocumentError> {
        let slide = find_slide_mut(ir, slide_id)?;
        let block = slide
            .blocks
            .iter_mut()
            .find(|b| block_id_of(b) == list_block_id)
            .ok_or_else(|| DocumentError::IRValidationFailed {
                path: format!("/slides/{slide_id}/blocks/{list_block_id}"),
                reason: "list block not found".into(),
            })?;
        if let Block::List { items, .. } = block {
            let pos = items.iter().position(|i| i.id == item_id).ok_or_else(|| {
                DocumentError::IRValidationFailed {
                    path: format!("/.../{list_block_id}/items/{item_id}"),
                    reason: "item not found".into(),
                }
            })?;
            items.remove(pos);
            Ok(OpOutcome::default())
        } else {
            Err(DocumentError::IRValidationFailed {
                path: format!("/slides/{slide_id}/blocks/{list_block_id}"),
                reason: "not a list block".into(),
            })
        }
    }

    fn update_html_list_item(
        &self,
        ir: &mut HtmlIR,
        slide_id: &str,
        list_block_id: &str,
        item_id: &str,
        runs: &[serde_json::Value],
    ) -> Result<OpOutcome, DocumentError> {
        let parsed_runs: Vec<_> = runs
            .iter()
            .map(|r| {
                let mut rv = r.clone();
                if let Some(o) = rv.as_object_mut() {
                    if !o.contains_key("id") {
                        o.insert("id".into(), serde_json::json!(self.ids.new_run_id()));
                    }
                }
                serde_json::from_value::<crate::documents::domain::ir::html::Run>(rv)
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| DocumentError::IRValidationFailed {
                path: "/op/update_html_list_item/runs".into(),
                reason: e.to_string(),
            })?;
        let slide = find_slide_mut(ir, slide_id)?;
        let block = slide
            .blocks
            .iter_mut()
            .find(|b| block_id_of(b) == list_block_id)
            .ok_or_else(|| DocumentError::IRValidationFailed {
                path: format!("/slides/{slide_id}/blocks/{list_block_id}"),
                reason: "list block not found".into(),
            })?;
        if let Block::List { items, .. } = block {
            let item = items.iter_mut().find(|i| i.id == item_id).ok_or_else(|| {
                DocumentError::IRValidationFailed {
                    path: format!("/.../{list_block_id}/items/{item_id}"),
                    reason: "item not found".into(),
                }
            })?;
            item.runs = parsed_runs;
            Ok(OpOutcome::default())
        } else {
            Err(DocumentError::IRValidationFailed {
                path: format!("/slides/{slide_id}/blocks/{list_block_id}"),
                reason: "not a list block".into(),
            })
        }
    }

    fn assign_nested_ids(&self, obj: &mut serde_json::Map<String, serde_json::Value>) {
        if let Some(serde_json::Value::Array(runs)) = obj.get_mut("runs") {
            for r in runs {
                if let Some(o) = r.as_object_mut() {
                    if !o.contains_key("id") {
                        o.insert("id".into(), serde_json::json!(self.ids.new_run_id()));
                    }
                }
            }
        }
        if let Some(serde_json::Value::Array(items)) = obj.get_mut("items") {
            for i in items {
                if let Some(o) = i.as_object_mut() {
                    if !o.contains_key("id") {
                        o.insert("id".into(), serde_json::json!(self.ids.new_list_item_id()));
                    }
                    self.assign_nested_ids(o);
                }
            }
        }
        if let Some(serde_json::Value::Array(rows)) = obj.get_mut("rows") {
            for r in rows {
                if let Some(o) = r.as_object_mut() {
                    if !o.contains_key("id") {
                        o.insert("id".into(), serde_json::json!(self.ids.new_row_id()));
                    }
                }
            }
        }
    }
}

fn block_id_of(b: &Block) -> &str {
    match b {
        Block::Heading { id, .. }
        | Block::Paragraph { id, .. }
        | Block::List { id, .. }
        | Block::Blockquote { id, .. }
        | Block::Code { id, .. }
        | Block::Table { id, .. }
        | Block::Chart { id, .. }
        | Block::KpiCard { id, .. }
        | Block::KpiGrid { id, .. }
        | Block::Image { id, .. }
        | Block::TwoColumns { id, .. }
        | Block::ThreeColumns { id, .. }
        | Block::Comparison { id, .. }
        | Block::Callout { id, .. }
        | Block::Divider { id }
        | Block::Video { id, .. }
        | Block::AutoToc { id, .. } => id,
    }
}

fn find_slide_mut<'b>(ir: &'b mut HtmlIR, id: &str) -> Result<&'b mut Slide, DocumentError> {
    ir.slides
        .iter_mut()
        .find(|s| s.id == id)
        .ok_or_else(|| DocumentError::IRValidationFailed {
            path: format!("/slides/{id}"),
            reason: "slide not found".into(),
        })
}

pub(crate) fn recompute_assets_referenced(ir: &mut HtmlIR) {
    let mut refs = std::collections::HashSet::new();
    for slide in &ir.slides {
        walk_block_assets(&slide.blocks, &mut refs);
    }
    let mut v: Vec<String> = refs.into_iter().collect();
    v.sort();
    ir.assets_referenced = v;
}

fn walk_block_assets(blocks: &[Block], out: &mut std::collections::HashSet<String>) {
    for b in blocks {
        match b {
            Block::Image {
                src: ImageSrc::Asset { asset_id },
                ..
            }
            | Block::Video {
                src: VideoSrc::Asset { asset_id },
                ..
            } => {
                out.insert(asset_id.clone());
            }
            Block::TwoColumns { left, right, .. } => {
                walk_block_assets(left, out);
                walk_block_assets(right, out);
            }
            Block::ThreeColumns {
                left,
                middle,
                right,
                ..
            } => {
                walk_block_assets(left, out);
                walk_block_assets(middle, out);
                walk_block_assets(right, out);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::documents::domain::ir::html::{DocProps, FooterConfig, LayoutMode, Locale, Theme};
    use crate::documents::infrastructure::ids::CountingIdGenerator;

    fn fresh() -> HtmlIR {
        HtmlIR {
            kind: "html".into(),
            artifact_id: "art_x".into(),
            version_id: "v1".into(),
            schema_version: "1.0.0".into(),
            doc_props: DocProps {
                title: None,
                author: None,
                date: None,
                locale: Locale::En,
            },
            theme: Theme::Executive,
            layout_mode: LayoutMode::Slides,
            footer: FooterConfig {
                enabled: false,
                page_numbers: false,
                custom_text: None,
            },
            slides: vec![Slide {
                id: "sl_existing".into(),
                layout: SlideLayout::Blank,
                title: None,
                subtitle: None,
                notes: None,
                blocks: vec![],
            }],
            assets_referenced: vec![],
        }
    }

    #[test]
    fn add_slide_appends_and_returns_id() {
        let mut ir = fresh();
        let ids = CountingIdGenerator::default();
        let applier = HtmlOpApplier { ids: &ids };
        let out = applier
            .apply(
                &mut ir,
                &PatchOp::AddSlide {
                    layout: SlideLayout::Content,
                    at_index: None,
                    title: Some("New".into()),
                    subtitle: None,
                },
            )
            .unwrap();
        assert_eq!(ir.slides.len(), 2);
        assert_eq!(out.assigned_ids.slide.as_deref(), Some("sl_01"));
    }

    #[test]
    fn add_title_slide_without_title_rejected() {
        let mut ir = fresh();
        let ids = CountingIdGenerator::default();
        let applier = HtmlOpApplier { ids: &ids };
        let err = applier
            .apply(
                &mut ir,
                &PatchOp::AddSlide {
                    layout: SlideLayout::Title,
                    at_index: None,
                    title: Some("   ".into()),
                    subtitle: None,
                },
            )
            .unwrap_err();
        assert!(matches!(err, DocumentError::IRValidationFailed { .. }));
    }

    #[test]
    fn delete_last_slide_rejected() {
        let mut ir = fresh();
        let ids = CountingIdGenerator::default();
        let applier = HtmlOpApplier { ids: &ids };
        let err = applier
            .apply(
                &mut ir,
                &PatchOp::DeleteSlide {
                    slide_id: "sl_existing".into(),
                },
            )
            .unwrap_err();
        assert!(err.to_string().contains("cannot delete last"));
    }

    #[test]
    fn reorder_validates_length_and_ids() {
        let mut ir = fresh();
        ir.slides.push(Slide {
            id: "sl_b".into(),
            layout: SlideLayout::Blank,
            title: None,
            subtitle: None,
            notes: None,
            blocks: vec![],
        });
        let ids = CountingIdGenerator::default();
        let applier = HtmlOpApplier { ids: &ids };
        applier
            .apply(
                &mut ir,
                &PatchOp::ReorderSlides {
                    order: vec!["sl_b".into(), "sl_existing".into()],
                },
            )
            .unwrap();
        assert_eq!(ir.slides[0].id, "sl_b");
    }

    #[test]
    fn insert_html_block_assigns_id_and_appends() {
        let mut ir = fresh();
        let ids = CountingIdGenerator::default();
        let applier = HtmlOpApplier { ids: &ids };
        let out = applier
            .apply(
                &mut ir,
                &PatchOp::InsertHtmlBlock {
                    slide_id: "sl_existing".into(),
                    before: None,
                    after: None,
                    block: serde_json::json!({
                        "kind":"paragraph",
                        "runs":[{"id":"will_be_overwritten","text":"x","bold":false,"italic":false,"underline":false,"code":false}]
                    }),
                },
            )
            .unwrap();
        assert!(out.assigned_ids.block.is_some(), "expected block id");
        assert_eq!(ir.slides[0].blocks.len(), 1);
    }

    #[test]
    fn delete_html_block_removes_by_id() {
        let mut ir = fresh();
        ir.slides[0].blocks = vec![Block::Divider { id: "blk_d".into() }];
        let ids = CountingIdGenerator::default();
        let applier = HtmlOpApplier { ids: &ids };
        applier
            .apply(
                &mut ir,
                &PatchOp::DeleteHtmlBlock {
                    slide_id: "sl_existing".into(),
                    block_id: "blk_d".into(),
                },
            )
            .unwrap();
        assert!(ir.slides[0].blocks.is_empty());
    }

    #[test]
    fn set_theme_changes_ir_theme() {
        use crate::documents::domain::ir::html::Theme;
        let mut ir = fresh();
        let ids = CountingIdGenerator::default();
        let applier = HtmlOpApplier { ids: &ids };
        applier
            .apply(&mut ir, &PatchOp::SetTheme { theme: Theme::Dark })
            .unwrap();
        assert_eq!(ir.theme, Theme::Dark);
    }

    #[test]
    fn insert_image_block_with_asset_updates_assets_referenced() {
        let mut ir = fresh();
        let ids = CountingIdGenerator::default();
        let applier = HtmlOpApplier { ids: &ids };
        applier
            .apply(
                &mut ir,
                &PatchOp::InsertHtmlBlock {
                    slide_id: "sl_existing".into(),
                    before: None,
                    after: None,
                    block: serde_json::json!({
                        "kind":"image",
                        "src":{"kind":"asset","asset_id":"asset_logo"},
                        "alt":"x","caption":null,"position":"inline"
                    }),
                },
            )
            .unwrap();
        assert!(ir.assets_referenced.contains(&"asset_logo".to_string()));
    }
}
