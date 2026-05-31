use crate::documents::domain::artifact::{AssignedIds, OpOutcome};
use crate::documents::domain::ir::{Cell, CellType, ExcelIR, NamedStyle, NamedTable, Sheet};
use crate::documents::domain::patch::PatchOp;
use crate::documents::domain::{DocumentError, IdGenerator};
use std::collections::BTreeMap;

pub struct ExcelOpApplier<'a> {
    pub ids: &'a dyn IdGenerator,
}

impl<'a> ExcelOpApplier<'a> {
    pub fn apply(&self, ir: &mut ExcelIR, op: &PatchOp) -> Result<OpOutcome, DocumentError> {
        let mut assigned = AssignedIds::default();
        match op {
            PatchOp::SetCell {
                sheet_id,
                address,
                value,
                value_type,
                format,
                style_ref,
            } => {
                let sheet =
                    ir.sheet_mut(sheet_id)
                        .ok_or_else(|| DocumentError::InvalidPatchOp {
                            reason: format!("sheet not found: {sheet_id}"),
                            op: serde_json::to_value(op).unwrap(),
                        })?;
                let ct = value_type.as_deref().and_then(parse_cell_type);
                sheet.cells.insert(
                    address.to_ascii_uppercase(),
                    Cell {
                        value: value.clone(),
                        value_type: ct,
                        format: format.clone(),
                        style_ref: style_ref.clone(),
                    },
                );
            }
            PatchOp::SetRange {
                sheet_id,
                range,
                values,
                value_types,
            } => {
                let sheet = ir
                    .sheet_mut(sheet_id)
                    .ok_or_else(|| invalid(op, "sheet not found"))?;
                let (first, _last) =
                    parse_range(range).ok_or_else(|| invalid(op, "invalid range"))?;
                for (r, row) in values.iter().enumerate() {
                    for (c, v) in row.iter().enumerate() {
                        if v.is_null() {
                            continue;
                        }
                        let addr = to_a1(first.0 + r as u32, first.1 + c as u16);
                        let ct = value_types
                            .as_ref()
                            .and_then(|m| m.get(r))
                            .and_then(|row| row.get(c))
                            .and_then(|o| o.as_deref())
                            .and_then(parse_cell_type);
                        sheet.cells.insert(
                            addr,
                            Cell {
                                value: v.clone(),
                                value_type: ct,
                                format: None,
                                style_ref: None,
                            },
                        );
                    }
                }
            }
            PatchOp::ClearRange { sheet_id, range } => {
                let sheet = ir
                    .sheet_mut(sheet_id)
                    .ok_or_else(|| invalid(op, "sheet not found"))?;
                let (first, last) =
                    parse_range(range).ok_or_else(|| invalid(op, "invalid range"))?;
                let addrs: Vec<String> = sheet
                    .cells
                    .keys()
                    .filter(|a| in_range(a, first, last))
                    .cloned()
                    .collect();
                for a in addrs {
                    sheet.cells.remove(&a);
                }
            }
            PatchOp::InsertRow {
                sheet_id,
                before_row,
                values,
            } => {
                let sheet = ir
                    .sheet_mut(sheet_id)
                    .ok_or_else(|| invalid(op, "sheet not found"))?;
                shift_rows(&mut sheet.cells, *before_row, 1);
                if let Some(vs) = values {
                    for (c, v) in vs.iter().enumerate() {
                        if v.is_null() {
                            continue;
                        }
                        let addr = to_a1(*before_row - 1, c as u16);
                        sheet.cells.insert(
                            addr,
                            Cell {
                                value: v.clone(),
                                value_type: None,
                                format: None,
                                style_ref: None,
                            },
                        );
                    }
                }
            }
            PatchOp::DeleteRow {
                sheet_id,
                row_index,
            } => {
                let sheet = ir
                    .sheet_mut(sheet_id)
                    .ok_or_else(|| invalid(op, "sheet not found"))?;
                let to_remove: Vec<String> = sheet
                    .cells
                    .keys()
                    .filter(|a| cell_row(a).map(|r| r == *row_index).unwrap_or(false))
                    .cloned()
                    .collect();
                for a in to_remove {
                    sheet.cells.remove(&a);
                }
                shift_rows(&mut sheet.cells, *row_index + 1, -1);
            }
            PatchOp::InsertColumn {
                sheet_id,
                before_col,
                values,
            } => {
                let sheet = ir
                    .sheet_mut(sheet_id)
                    .ok_or_else(|| invalid(op, "sheet not found"))?;
                shift_cols(&mut sheet.cells, *before_col, 1);
                if let Some(vs) = values {
                    for (r, v) in vs.iter().enumerate() {
                        if v.is_null() {
                            continue;
                        }
                        let addr = to_a1(r as u32, *before_col as u16);
                        sheet.cells.insert(
                            addr,
                            Cell {
                                value: v.clone(),
                                value_type: None,
                                format: None,
                                style_ref: None,
                            },
                        );
                    }
                }
            }
            PatchOp::DeleteColumn {
                sheet_id,
                col_index,
            } => {
                let sheet = ir
                    .sheet_mut(sheet_id)
                    .ok_or_else(|| invalid(op, "sheet not found"))?;
                let to_remove: Vec<String> = sheet
                    .cells
                    .keys()
                    .filter(|a| cell_col(a).map(|c| c == *col_index as u16).unwrap_or(false))
                    .cloned()
                    .collect();
                for a in to_remove {
                    sheet.cells.remove(&a);
                }
                shift_cols(&mut sheet.cells, *col_index + 1, -1);
            }
            PatchOp::AddSheet { name, at_index } => {
                let id = self.ids.new_sheet_id();
                assigned.sheet = Some(id.clone());
                let order = at_index.unwrap_or(ir.workbook.sheets.len() as u32);
                for s in ir.workbook.sheets.iter_mut() {
                    if s.order >= order {
                        s.order += 1;
                    }
                }
                ir.workbook.sheets.push(Sheet {
                    id,
                    name: name.clone(),
                    order,
                    columns: vec![],
                    cells: BTreeMap::new(),
                    tables: vec![],
                });
            }
            PatchOp::RenameSheet { sheet_id, new_name } => {
                let sheet = ir
                    .sheet_mut(sheet_id)
                    .ok_or_else(|| invalid(op, "sheet not found"))?;
                sheet.name = new_name.clone();
            }
            PatchOp::DeleteSheet { sheet_id } => {
                let Some(pos) = ir.workbook.sheets.iter().position(|s| &s.id == sheet_id) else {
                    return Err(invalid(op, "sheet not found"));
                };
                let removed_order = ir.workbook.sheets[pos].order;
                ir.workbook.sheets.remove(pos);
                for s in ir.workbook.sheets.iter_mut() {
                    if s.order > removed_order {
                        s.order -= 1;
                    }
                }
            }
            PatchOp::ReorderSheets { order } => {
                for (i, sid) in order.iter().enumerate() {
                    if let Some(s) = ir.sheet_mut(sid) {
                        s.order = i as u32;
                    }
                }
            }
            PatchOp::CreateTable {
                sheet_id,
                range,
                name,
                header_row,
                style_preset,
            } => {
                let id = self.ids.new_table_id();
                assigned.table = Some(id.clone());
                let sheet = ir
                    .sheet_mut(sheet_id)
                    .ok_or_else(|| invalid(op, "sheet not found"))?;
                sheet.tables.push(NamedTable {
                    id,
                    name: name.clone(),
                    range: range.clone(),
                    header_row: *header_row,
                    style_preset: style_preset.clone(),
                });
            }
            PatchOp::ResizeTable {
                table_id,
                new_range,
            } => {
                for sheet in ir.workbook.sheets.iter_mut() {
                    if let Some(t) = sheet.tables.iter_mut().find(|t| &t.id == table_id) {
                        t.range = new_range.clone();
                        return Ok(OpOutcome {
                            assigned_ids: assigned,
                        });
                    }
                }
                return Err(invalid(op, "table not found"));
            }
            PatchOp::DeleteTable { table_id } => {
                for sheet in ir.workbook.sheets.iter_mut() {
                    sheet.tables.retain(|t| &t.id != table_id);
                }
            }
            PatchOp::SetColumnWidth {
                sheet_id,
                col,
                width,
            } => {
                let sheet = ir
                    .sheet_mut(sheet_id)
                    .ok_or_else(|| invalid(op, "sheet not found"))?;
                if let Some(c) = sheet.columns.iter_mut().find(|c| c.index == *col) {
                    c.width = *width;
                } else {
                    sheet
                        .columns
                        .push(crate::documents::domain::ir::ColumnSpec {
                            index: *col,
                            width: *width,
                        });
                }
            }
            PatchOp::DefineStyle {
                style_ref,
                definition,
            } => {
                let style: NamedStyle = serde_json::from_value(definition.clone())
                    .map_err(|e| invalid(op, &format!("bad style: {e}")))?;
                ir.workbook.named_styles.insert(style_ref.clone(), style);
            }
            // Word-only and HTML-only ops are not valid for Excel artifacts.
            PatchOp::InsertBlock { .. }
            | PatchOp::DeleteBlock { .. }
            | PatchOp::ReplaceBlock { .. }
            | PatchOp::MoveBlock { .. }
            | PatchOp::SetHeadingLevel { .. }
            | PatchOp::ReplaceRunText { .. }
            | PatchOp::SetRunStyle { .. }
            | PatchOp::InsertRun { .. }
            | PatchOp::DeleteRun { .. }
            | PatchOp::InsertListItem { .. }
            | PatchOp::ReplaceListItem { .. }
            | PatchOp::DeleteListItem { .. }
            | PatchOp::InsertTableRow { .. }
            | PatchOp::DeleteTableRow { .. }
            | PatchOp::UpdateTableCell { .. }
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
                return Err(invalid(op, "Word/HTML op not applicable to Excel artifact"));
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

fn parse_cell_type(s: &str) -> Option<CellType> {
    match s {
        "string" => Some(CellType::String),
        "number" => Some(CellType::Number),
        "boolean" => Some(CellType::Boolean),
        "date" => Some(CellType::Date),
        "formula" => Some(CellType::Formula),
        _ => None,
    }
}

fn parse_a1(addr: &str) -> Option<(u32, u16)> {
    let addr = addr.to_ascii_uppercase();
    let split = addr.find(|c: char| c.is_ascii_digit())?;
    let (col_str, row_str) = addr.split_at(split);
    let row: u32 = row_str.parse().ok()?;
    if row == 0 {
        return None;
    }
    let mut col: u32 = 0;
    for c in col_str.chars() {
        if !c.is_ascii_alphabetic() {
            return None;
        }
        col = col * 26 + (c as u32 - 'A' as u32 + 1);
    }
    Some((row - 1, (col - 1) as u16))
}

fn parse_range(r: &str) -> Option<((u32, u16), (u32, u16))> {
    let (a, b) = r.split_once(':')?;
    Some((parse_a1(a)?, parse_a1(b)?))
}

fn to_a1(row: u32, col: u16) -> String {
    let mut n = col as u32 + 1;
    let mut s = String::new();
    while n > 0 {
        let rem = ((n - 1) % 26) as u8;
        s.insert(0, (b'A' + rem) as char);
        n = (n - 1) / 26;
    }
    format!("{}{}", s, row + 1)
}

fn cell_row(addr: &str) -> Option<u32> {
    parse_a1(addr).map(|(r, _)| r + 1)
}
fn cell_col(addr: &str) -> Option<u16> {
    parse_a1(addr).map(|(_, c)| c)
}

fn in_range(addr: &str, first: (u32, u16), last: (u32, u16)) -> bool {
    let Some((r, c)) = parse_a1(addr) else {
        return false;
    };
    r >= first.0 && r <= last.0 && c >= first.1 && c <= last.1
}

fn shift_rows(cells: &mut BTreeMap<String, Cell>, from_row: u32, delta: i32) {
    let mut moves: Vec<(String, String, Cell)> = Vec::new();
    for (addr, cell) in cells.iter() {
        if let Some((r, c)) = parse_a1(addr) {
            if r + 1 >= from_row {
                let new_r = (r as i32 + delta) as u32;
                let new_addr = to_a1(new_r, c);
                moves.push((addr.clone(), new_addr, cell.clone()));
            }
        }
    }
    for (old, _, _) in &moves {
        cells.remove(old);
    }
    for (_, new, cell) in moves {
        cells.insert(new, cell);
    }
}

fn shift_cols(cells: &mut BTreeMap<String, Cell>, from_col: u32, delta: i32) {
    let mut moves: Vec<(String, String, Cell)> = Vec::new();
    for (addr, cell) in cells.iter() {
        if let Some((r, c)) = parse_a1(addr) {
            if c as u32 >= from_col {
                let new_c = (c as i32 + delta) as u16;
                let new_addr = to_a1(r, new_c);
                moves.push((addr.clone(), new_addr, cell.clone()));
            }
        }
    }
    for (old, _, _) in &moves {
        cells.remove(old);
    }
    for (_, new, cell) in moves {
        cells.insert(new, cell);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::documents::infrastructure::ids::CountingIdGenerator;

    fn sheet_ir() -> ExcelIR {
        let mut ir = ExcelIR::empty("art_x", "v1");
        ir.workbook.sheets.push(Sheet {
            id: "s1".into(),
            name: "Hoja1".into(),
            order: 0,
            columns: vec![],
            cells: BTreeMap::new(),
            tables: vec![],
        });
        ir
    }

    #[test]
    fn set_cell_inserts() {
        let ids = CountingIdGenerator::default();
        let applier = ExcelOpApplier { ids: &ids };
        let mut ir = sheet_ir();
        applier
            .apply(
                &mut ir,
                &PatchOp::SetCell {
                    sheet_id: "s1".into(),
                    address: "B5".into(),
                    value: serde_json::json!(42),
                    value_type: None,
                    format: None,
                    style_ref: None,
                },
            )
            .unwrap();
        assert_eq!(
            ir.workbook.sheets[0].cells["B5"].value,
            serde_json::json!(42)
        );
    }

    #[test]
    fn insert_row_shifts_cells_down() {
        let ids = CountingIdGenerator::default();
        let applier = ExcelOpApplier { ids: &ids };
        let mut ir = sheet_ir();
        ir.workbook.sheets[0].cells.insert(
            "A1".into(),
            Cell {
                value: serde_json::json!("top"),
                value_type: None,
                format: None,
                style_ref: None,
            },
        );
        ir.workbook.sheets[0].cells.insert(
            "A5".into(),
            Cell {
                value: serde_json::json!("fifth"),
                value_type: None,
                format: None,
                style_ref: None,
            },
        );
        applier
            .apply(
                &mut ir,
                &PatchOp::InsertRow {
                    sheet_id: "s1".into(),
                    before_row: 3,
                    values: None,
                },
            )
            .unwrap();
        assert!(ir.workbook.sheets[0].cells.contains_key("A1"));
        assert!(ir.workbook.sheets[0].cells.contains_key("A6"));
        assert!(!ir.workbook.sheets[0].cells.contains_key("A5"));
    }

    #[test]
    fn add_sheet_allocates_id() {
        let ids = CountingIdGenerator::default();
        let applier = ExcelOpApplier { ids: &ids };
        let mut ir = sheet_ir();
        let out = applier
            .apply(
                &mut ir,
                &PatchOp::AddSheet {
                    name: "Costos".into(),
                    at_index: None,
                },
            )
            .unwrap();
        assert_eq!(ir.workbook.sheets.len(), 2);
        assert_eq!(ir.workbook.sheets[1].id, "sheet_01");
        assert_eq!(out.assigned_ids.sheet.as_deref(), Some("sheet_01"));
    }

    #[test]
    fn create_table_reports_table_id() {
        let ids = CountingIdGenerator::default();
        let applier = ExcelOpApplier { ids: &ids };
        let mut ir = sheet_ir();
        let out = applier
            .apply(
                &mut ir,
                &PatchOp::CreateTable {
                    sheet_id: "s1".into(),
                    range: "A1:B3".into(),
                    name: "T".into(),
                    header_row: true,
                    style_preset: None,
                },
            )
            .unwrap();
        assert_eq!(out.assigned_ids.table.as_deref(), Some("tbl_01"));
    }

    #[test]
    fn to_a1_roundtrip() {
        assert_eq!(to_a1(0, 0), "A1");
        assert_eq!(to_a1(4, 1), "B5");
        assert_eq!(to_a1(0, 26), "AA1");
    }
}
