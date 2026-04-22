use crate::documents::domain::ir::{CellType, ExcelIR};
use crate::documents::domain::{IRRenderer, RenderError};
use async_trait::async_trait;
use rust_xlsxwriter::{Format, Workbook};

pub struct ExcelRenderer;

impl ExcelRenderer {
    fn render_sync(ir: &ExcelIR) -> Result<Vec<u8>, RenderError> {
        let mut wb = Workbook::new();
        let mut sorted = ir.workbook.sheets.clone();
        sorted.sort_by_key(|s| s.order);

        for sheet in &sorted {
            let ws = wb.add_worksheet();
            ws.set_name(&sheet.name)
                .map_err(|e| RenderError::Failed(format!("set_name: {e}")))?;

            for col in &sheet.columns {
                ws.set_column_width(col.index as u16, col.width)
                    .map_err(|e| RenderError::Failed(format!("set_column_width: {e}")))?;
            }

            for (addr, cell) in &sheet.cells {
                let (row, col) = parse_a1(addr)
                    .ok_or_else(|| RenderError::Failed(format!("invalid address: {addr}")))?;

                let mut fmt = Format::new();
                if let Some(f) = &cell.format {
                    fmt = fmt.set_num_format(f);
                }
                if let Some(sref) = &cell.style_ref {
                    if let Some(style) = ir.workbook.named_styles.get(sref) {
                        if let Some(font) = &style.font {
                            if font.bold.unwrap_or(false) {
                                fmt = fmt.set_bold();
                            }
                            if font.italic.unwrap_or(false) {
                                fmt = fmt.set_italic();
                            }
                            if let Some(sz) = font.size {
                                fmt = fmt.set_font_size(sz);
                            }
                            if let Some(color) = &font.color {
                                if let Ok(c) = u32::from_str_radix(color.trim_start_matches('#'), 16) {
                                    fmt = fmt.set_font_color(rust_xlsxwriter::Color::RGB(c));
                                }
                            }
                        }
                        if let Some(fill) = &style.fill {
                            if let Ok(c) = u32::from_str_radix(fill.trim_start_matches('#'), 16) {
                                fmt = fmt.set_background_color(rust_xlsxwriter::Color::RGB(c));
                            }
                        }
                    }
                }

                let vt = cell
                    .value_type
                    .clone()
                    .unwrap_or_else(|| infer_type(&cell.value));
                write_cell(ws, row, col, &cell.value, vt, &fmt)?;
            }

            for table in &sheet.tables {
                let (first, last) = parse_range(&table.range).ok_or_else(|| {
                    RenderError::Failed(format!("invalid range: {}", table.range))
                })?;
                let mut t = rust_xlsxwriter::Table::new().set_name(&table.name);
                if !table.header_row {
                    t = t.set_header_row(false);
                }
                ws.add_table(first.0, first.1, last.0, last.1, &t)
                    .map_err(|e| RenderError::Failed(format!("add_table: {e}")))?;
            }
        }

        wb.save_to_buffer()
            .map_err(|e| RenderError::Failed(format!("save_to_buffer: {e}")))
    }
}

fn infer_type(value: &serde_json::Value) -> CellType {
    if value.is_number() {
        CellType::Number
    } else if value.is_boolean() {
        CellType::Boolean
    } else if value
        .as_str()
        .map(|s| s.starts_with('='))
        .unwrap_or(false)
    {
        CellType::Formula
    } else {
        CellType::String
    }
}

fn write_cell(
    ws: &mut rust_xlsxwriter::Worksheet,
    row: u32,
    col: u16,
    value: &serde_json::Value,
    ct: CellType,
    fmt: &Format,
) -> Result<(), RenderError> {
    let err = |e: rust_xlsxwriter::XlsxError| RenderError::Failed(format!("write: {e}"));
    match ct {
        CellType::String => {
            let s = value
                .as_str()
                .map(|s| s.to_string())
                .unwrap_or_else(|| value.to_string());
            ws.write_string_with_format(row, col, &s, fmt).map_err(err)?;
        }
        CellType::Number => {
            let n = value.as_f64().unwrap_or(0.0);
            ws.write_number_with_format(row, col, n, fmt).map_err(err)?;
        }
        CellType::Boolean => {
            let b = value.as_bool().unwrap_or(false);
            ws.write_boolean_with_format(row, col, b, fmt).map_err(err)?;
        }
        CellType::Date => {
            let s = value
                .as_str()
                .map(|s| s.to_string())
                .unwrap_or_default();
            ws.write_string_with_format(row, col, &s, fmt).map_err(err)?;
        }
        CellType::Formula => {
            let s = value
                .as_str()
                .map(|s| s.to_string())
                .unwrap_or_default();
            let formula = rust_xlsxwriter::Formula::new(&s);
            ws.write_formula_with_format(row, col, formula, fmt)
                .map_err(err)?;
        }
    }
    Ok(())
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

fn parse_range(range: &str) -> Option<((u32, u16), (u32, u16))> {
    let (a, b) = range.split_once(':')?;
    Some((parse_a1(a)?, parse_a1(b)?))
}

#[async_trait]
impl IRRenderer for ExcelRenderer {
    async fn render(&self, ir: &serde_json::Value) -> Result<Vec<u8>, RenderError> {
        let ir: ExcelIR = serde_json::from_value(ir.clone())
            .map_err(|e| RenderError::Failed(format!("parse IR: {e}")))?;
        Self::render_sync(&ir)
    }
    fn target_extension(&self) -> &'static str {
        "xlsx"
    }
    fn target_mime(&self) -> &'static str {
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::documents::domain::ir::{Cell, Sheet};
    use std::collections::BTreeMap;

    #[tokio::test]
    async fn renders_minimal_xlsx() {
        let mut ir = ExcelIR::empty("art_x", "v1");
        let mut cells = BTreeMap::new();
        cells.insert(
            "A1".into(),
            Cell {
                value: serde_json::json!("Hello"),
                value_type: None,
                format: None,
                style_ref: None,
            },
        );
        cells.insert(
            "B1".into(),
            Cell {
                value: serde_json::json!(42),
                value_type: None,
                format: None,
                style_ref: None,
            },
        );
        ir.workbook.sheets.push(Sheet {
            id: "s1".into(),
            name: "Sheet1".into(),
            order: 0,
            columns: vec![],
            cells,
            tables: vec![],
        });

        let r = ExcelRenderer;
        let bytes = r.render(&serde_json::to_value(&ir).unwrap()).await.unwrap();
        assert!(bytes.len() > 100);
        assert_eq!(&bytes[..2], b"PK");
    }

    #[test]
    fn parses_a1() {
        assert_eq!(parse_a1("A1"), Some((0, 0)));
        assert_eq!(parse_a1("B5"), Some((4, 1)));
        assert_eq!(parse_a1("AA1"), Some((0, 26)));
        assert_eq!(parse_a1("0"), None);
    }

    #[tokio::test]
    async fn calamine_can_reopen_rendered_xlsx() {
        use calamine::{Reader, Xlsx};
        use std::io::Cursor;

        let mut ir = ExcelIR::empty("art_x", "v1");
        let mut cells = BTreeMap::new();
        cells.insert(
            "A1".into(),
            Cell {
                value: serde_json::json!("Producto"),
                value_type: None,
                format: None,
                style_ref: None,
            },
        );
        cells.insert(
            "A2".into(),
            Cell {
                value: serde_json::json!("Widget"),
                value_type: None,
                format: None,
                style_ref: None,
            },
        );
        ir.workbook.sheets.push(Sheet {
            id: "s1".into(),
            name: "Ventas".into(),
            order: 0,
            columns: vec![],
            cells,
            tables: vec![],
        });

        let bytes = ExcelRenderer
            .render(&serde_json::to_value(&ir).unwrap())
            .await
            .unwrap();
        let mut xl: Xlsx<_> = calamine::open_workbook_from_rs(Cursor::new(bytes)).unwrap();
        let range = xl.worksheet_range("Ventas").unwrap();
        assert_eq!(range.get((0, 0)).unwrap().to_string(), "Producto");
        assert_eq!(range.get((1, 0)).unwrap().to_string(), "Widget");
    }
}
