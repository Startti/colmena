//! Table-cell formatting use case: apply presentable formatting (cell
//! background, borders, vertical alignment, text style, horizontal alignment)
//! to rectangular cell ranges inside a Google Docs table, in one atomic
//! multi-op `batchUpdate`. Mirrors `gsheets_format_range`'s `ops:[...]` shape.
//! See spec `docs/superpowers/specs/2026-06-22-gdocs-table-cell-formatting-design.md`.

use crate::gdocs::application::co_edit_guard::{run_guard_non_blocking, GuardContext};
use crate::gdocs::application::insert::apply_and_finalize;
use crate::gdocs::application::table::{cell_location, find_cell, find_table};
use crate::gdocs::application::util;
use crate::gdocs::domain::{
    ChangeKind, DocsError, DocumentId, EditResult, RgbColor, TabId, TableSnapshot,
};
use schemars::JsonSchema;
use serde::Deserialize;

/// A 0-based, end-EXCLUSIVE rectangle of cells within one table.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct CellRange {
    pub row_start: u32,
    pub row_end: u32,
    pub col_start: u32,
    pub col_end: u32,
}

/// One formatting op: which table, which cell rectangle, what format.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct TableFormatOp {
    pub table_index: u32,
    pub cell_range: CellRange,
    pub format: CellFormat,
}

/// All formatting attributes are optional; only set ones are emitted.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct CellFormat {
    /// Cell background, hex `#RRGGBB`.
    pub background_color: Option<String>,
    /// Vertical alignment of cell content: TOP | MIDDLE | BOTTOM.
    pub vertical_alignment: Option<String>,
    pub borders: Option<CellBorders>,
    pub text: Option<CellTextFormat>,
    /// Paragraph alignment: LEFT | CENTER | RIGHT | JUSTIFIED.
    pub horizontal_alignment: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct CellTextFormat {
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub underline: Option<bool>,
    pub strikethrough: Option<bool>,
    pub font_size: Option<f32>,
    /// Text color, hex `#RRGGBB`.
    pub color: Option<String>,
}

/// Each side optional; applied to every cell in the range (Docs has no
/// inner/outer border concept — every cell carries its own 4 borders).
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct CellBorders {
    pub top: Option<CellBorderSide>,
    pub bottom: Option<CellBorderSide>,
    pub left: Option<CellBorderSide>,
    pub right: Option<CellBorderSide>,
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct CellBorderSide {
    /// SOLID | DOTTED | DASHED. Default SOLID when the side is present.
    pub style: Option<String>,
    /// Border color, hex `#RRGGBB`. Default black.
    pub color: Option<String>,
    /// Border width in points. Default 1.
    pub width_pt: Option<f32>,
}

/// Parse `#RRGGBB` to a normalized `RgbColor` (0.0..=1.0). Rejects anything
/// that is not exactly 6 hex digits after an optional `#`.
pub(crate) fn hex_to_rgb(hex: &str) -> Result<RgbColor, DocsError> {
    let h = hex.trim().strip_prefix('#').unwrap_or(hex.trim());
    if h.len() != 6 || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(DocsError::InvalidArgs(format!(
            "invalid hex color '{hex}' — expected #RRGGBB"
        )));
    }
    let parse = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).unwrap();
    Ok(RgbColor {
        r: parse(0) as f32 / 255.0,
        g: parse(2) as f32 / 255.0,
        b: parse(4) as f32 / 255.0,
    })
}

/// Docs `OptionalColor` shape: `{ "color": { "rgbColor": { r, g, b } } }`.
fn rgb_to_color(c: &RgbColor) -> serde_json::Value {
    serde_json::json!({ "color": { "rgbColor": { "red": c.r, "green": c.g, "blue": c.b } } })
}

/// Validate the range against the table dims (end-exclusive, non-empty).
/// Checked arithmetic — never panics on adversarial LLM input.
pub(crate) fn validate_range(table: &TableSnapshot, r: &CellRange) -> Result<(), DocsError> {
    let bad = |m: String| DocsError::InvalidArgs(m);
    if r.row_start >= r.row_end || r.col_start >= r.col_end {
        return Err(bad(format!(
            "empty cell_range (rows {}..{}, cols {}..{}) — end must be > start",
            r.row_start, r.row_end, r.col_start, r.col_end
        )));
    }
    if r.row_end > table.rows || r.col_end > table.columns {
        return Err(bad(format!(
            "cell_range rows {}..{} cols {}..{} out of bounds for table {} ({}x{})",
            r.row_start,
            r.row_end,
            r.col_start,
            r.col_end,
            table.table_index,
            table.rows,
            table.columns
        )));
    }
    Ok(())
}

/// Build the `batchUpdate` requests for ONE op against an already-located
/// table. Emits (at most): 1 `updateTableCellStyle` over the cell range,
/// N `updateTextStyle` + N `updateParagraphStyle` (one each per cell).
pub(crate) fn build_format_table_requests(
    table: &TableSnapshot,
    range: &CellRange,
    format: &CellFormat,
    tab_id: Option<&TabId>,
) -> Result<Vec<serde_json::Value>, DocsError> {
    let mut reqs = Vec::new();

    // ── cell-level: one updateTableCellStyle over the rectangle ──
    let mut cell_style = serde_json::Map::new();
    let mut fields: Vec<&str> = Vec::new();
    if let Some(bg) = &format.background_color {
        cell_style.insert("backgroundColor".into(), rgb_to_color(&hex_to_rgb(bg)?));
        fields.push("backgroundColor");
    }
    if let Some(va) = &format.vertical_alignment {
        cell_style.insert("contentAlignment".into(), serde_json::json!(va));
        fields.push("contentAlignment");
    }
    if let Some(b) = &format.borders {
        for (side_opt, key) in [
            (&b.top, "borderTop"),
            (&b.bottom, "borderBottom"),
            (&b.left, "borderLeft"),
            (&b.right, "borderRight"),
        ] {
            if let Some(side) = side_opt {
                let color = match &side.color {
                    Some(c) => hex_to_rgb(c)?,
                    None => RgbColor {
                        r: 0.0,
                        g: 0.0,
                        b: 0.0,
                    },
                };
                cell_style.insert(
                    key.to_string(),
                    serde_json::json!({
                        "color": rgb_to_color(&color),
                        "width": { "magnitude": side.width_pt.unwrap_or(1.0), "unit": "PT" },
                        "dashStyle": side.style.clone().unwrap_or_else(|| "SOLID".into()),
                    }),
                );
                fields.push(key);
            }
        }
    }
    if !fields.is_empty() {
        reqs.push(serde_json::json!({
            "updateTableCellStyle": {
                "tableRange": {
                    "tableCellLocation": cell_location(table.start_index, range.row_start, range.col_start, tab_id),
                    "rowSpan": range.row_end - range.row_start,
                    "columnSpan": range.col_end - range.col_start,
                },
                "tableCellStyle": serde_json::Value::Object(cell_style),
                "fields": fields.join(","),
            }
        }));
    }

    // ── text-level + paragraph-level: per cell in the range ──
    let (text_map, text_fields) = build_text_style(format.text.as_ref());
    for row in range.row_start..range.row_end {
        for col in range.col_start..range.col_end {
            let cell = find_cell(table, row, col)?;
            let content = util::range(
                cell.content_start_index,
                cell.content_end_index,
                &tab_id.cloned(),
            );
            if !text_fields.is_empty() {
                reqs.push(serde_json::json!({
                    "updateTextStyle": {
                        "range": content.clone(),
                        "textStyle": serde_json::Value::Object(text_map.clone()),
                        "fields": text_fields.join(","),
                    }
                }));
            }
            if let Some(align) = &format.horizontal_alignment {
                reqs.push(serde_json::json!({
                    "updateParagraphStyle": {
                        "range": content,
                        "paragraphStyle": { "alignment": align },
                        "fields": "alignment",
                    }
                }));
            }
        }
    }
    Ok(reqs)
}

/// Build the textStyle map + fields list from the optional text format.
///
/// NOTE: a bad text `color` hex is swallowed silently here (the `if let Ok`).
/// This is intentional asymmetry to avoid the borrow/`?` issue inside a helper
/// that returns no `Result`; the cell-level `background_color`/`border.color`
/// paths in `build_format_table_requests` DO validate via `?`. Kept simple for v1.
fn build_text_style(
    t: Option<&CellTextFormat>,
) -> (
    serde_json::Map<String, serde_json::Value>,
    Vec<&'static str>,
) {
    let mut m = serde_json::Map::new();
    let mut fields: Vec<&'static str> = Vec::new();
    let Some(t) = t else { return (m, fields) };
    if let Some(b) = t.bold {
        m.insert("bold".into(), b.into());
        fields.push("bold");
    }
    if let Some(i) = t.italic {
        m.insert("italic".into(), i.into());
        fields.push("italic");
    }
    if let Some(u) = t.underline {
        m.insert("underline".into(), u.into());
        fields.push("underline");
    }
    if let Some(s) = t.strikethrough {
        m.insert("strikethrough".into(), s.into());
        fields.push("strikethrough");
    }
    if let Some(sz) = t.font_size {
        m.insert(
            "fontSize".into(),
            serde_json::json!({ "magnitude": sz, "unit": "PT" }),
        );
        fields.push("fontSize");
    }
    if let Some(c) = &t.color {
        if let Ok(rgb) = hex_to_rgb(c) {
            m.insert(
                "foregroundColor".into(),
                serde_json::json!({ "color": { "rgbColor": { "red": rgb.r, "green": rgb.g, "blue": rgb.b } } }),
            );
            fields.push("foregroundColor");
        }
    }
    (m, fields)
}

/// Apply formatting ops to a doc's tables in one atomic batchUpdate.
///
/// Body wired in Task 2. The `let _ = (...)` fn-pointer references below keep
/// the Task-1 pure builder/validator + the Task-2 imports alive under the
/// crate's `deny(warnings)` dead-code lint until the use case is implemented.
pub async fn run_format_table(
    _ctx: &GuardContext<'_>,
    _doc_id: &DocumentId,
    _ops: &[TableFormatOp],
    _tab_id: Option<TabId>,
) -> Result<EditResult, DocsError> {
    let _ = (
        run_guard_non_blocking,
        apply_and_finalize,
        find_table,
        validate_range,
        build_format_table_requests,
        ChangeKind::Style,
    );
    todo!("task 2")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gdocs::domain::{CellSnapshot, TableSnapshot};

    fn cell(row: u32, col: u32, start: u32, end: u32) -> CellSnapshot {
        CellSnapshot {
            row,
            col,
            text: "x".into(),
            content_start_index: start,
            content_end_index: end,
            row_span: 1,
            col_span: 1,
        }
    }

    /// A 2x2 table: cells at content indices 10/20/30/40, table starts at 5.
    fn table_2x2() -> TableSnapshot {
        TableSnapshot {
            table_index: 0,
            tab_id: None,
            start_index: 5,
            rows: 2,
            columns: 2,
            cells: vec![
                vec![cell(0, 0, 10, 12), cell(0, 1, 20, 22)],
                vec![cell(1, 0, 30, 32), cell(1, 1, 40, 42)],
            ],
        }
    }

    #[test]
    fn hex_parses_and_rejects() {
        let c = hex_to_rgb("#FF8000").unwrap();
        assert!((c.r - 1.0).abs() < 1e-6);
        assert!((c.g - 128.0 / 255.0).abs() < 1e-6);
        assert!(hex_to_rgb("#FFF").is_err());
        assert!(hex_to_rgb("nothex!").is_err());
    }

    #[test]
    fn validate_range_rejects_empty_and_oob() {
        let t = table_2x2();
        assert!(validate_range(
            &t,
            &CellRange {
                row_start: 0,
                row_end: 0,
                col_start: 0,
                col_end: 1
            }
        )
        .is_err());
        assert!(validate_range(
            &t,
            &CellRange {
                row_start: 0,
                row_end: 3,
                col_start: 0,
                col_end: 1
            }
        )
        .is_err());
        assert!(validate_range(
            &t,
            &CellRange {
                row_start: 0,
                row_end: 1,
                col_start: 0,
                col_end: 2
            }
        )
        .is_ok());
    }

    #[test]
    fn cell_level_only_emits_one_table_cell_style() {
        let t = table_2x2();
        let fmt = CellFormat {
            background_color: Some("#1F4E78".into()),
            ..Default::default()
        };
        let reqs = build_format_table_requests(
            &t,
            &CellRange {
                row_start: 0,
                row_end: 1,
                col_start: 0,
                col_end: 2,
            },
            &fmt,
            None,
        )
        .unwrap();
        assert_eq!(
            reqs.len(),
            1,
            "background-only → single updateTableCellStyle"
        );
        let r = &reqs[0]["updateTableCellStyle"];
        assert!(r.is_object());
        assert_eq!(r["fields"].as_str().unwrap(), "backgroundColor");
        // tableRange covers 1 row x 2 cols starting at (0,0)
        let tr = &r["tableRange"];
        assert_eq!(tr["rowSpan"].as_u64().unwrap(), 1);
        assert_eq!(tr["columnSpan"].as_u64().unwrap(), 2);
    }

    #[test]
    fn text_only_emits_text_and_no_cell_style() {
        let t = table_2x2();
        let fmt = CellFormat {
            text: Some(CellTextFormat {
                bold: Some(true),
                ..Default::default()
            }),
            ..Default::default()
        };
        // header row: (0,0)..(1,2) = 2 cells
        let reqs = build_format_table_requests(
            &t,
            &CellRange {
                row_start: 0,
                row_end: 1,
                col_start: 0,
                col_end: 2,
            },
            &fmt,
            None,
        )
        .unwrap();
        assert!(
            reqs.iter().all(|r| r.get("updateTableCellStyle").is_none()),
            "no cell-level attr → no updateTableCellStyle"
        );
        let text_reqs: Vec<_> = reqs
            .iter()
            .filter(|r| r.get("updateTextStyle").is_some())
            .collect();
        assert_eq!(text_reqs.len(), 2, "one updateTextStyle per cell in range");
        assert_eq!(
            text_reqs[0]["updateTextStyle"]["fields"].as_str().unwrap(),
            "bold"
        );
        // its range is the first cell's content range 10..12
        assert_eq!(
            text_reqs[0]["updateTextStyle"]["range"]["startIndex"]
                .as_u64()
                .unwrap(),
            10
        );
        assert_eq!(
            text_reqs[0]["updateTextStyle"]["range"]["endIndex"]
                .as_u64()
                .unwrap(),
            12
        );
    }

    #[test]
    fn horizontal_alignment_emits_paragraph_style_per_cell() {
        let t = table_2x2();
        let fmt = CellFormat {
            horizontal_alignment: Some("CENTER".into()),
            ..Default::default()
        };
        let reqs = build_format_table_requests(
            &t,
            &CellRange {
                row_start: 0,
                row_end: 2,
                col_start: 0,
                col_end: 1,
            },
            &fmt,
            None,
        )
        .unwrap();
        let para: Vec<_> = reqs
            .iter()
            .filter(|r| r.get("updateParagraphStyle").is_some())
            .collect();
        assert_eq!(
            para.len(),
            2,
            "one updateParagraphStyle per cell in the column range"
        );
        assert_eq!(
            para[0]["updateParagraphStyle"]["paragraphStyle"]["alignment"]
                .as_str()
                .unwrap(),
            "CENTER"
        );
        assert_eq!(
            para[0]["updateParagraphStyle"]["fields"].as_str().unwrap(),
            "alignment"
        );
    }

    #[test]
    fn borders_set_all_requested_sides_on_cell_style() {
        let t = table_2x2();
        let fmt = CellFormat {
            borders: Some(CellBorders {
                top: Some(CellBorderSide {
                    style: Some("SOLID".into()),
                    ..Default::default()
                }),
                bottom: Some(CellBorderSide::default()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let reqs = build_format_table_requests(
            &t,
            &CellRange {
                row_start: 0,
                row_end: 1,
                col_start: 0,
                col_end: 2,
            },
            &fmt,
            None,
        )
        .unwrap();
        let cs = &reqs[0]["updateTableCellStyle"];
        let fields = cs["fields"].as_str().unwrap();
        assert!(fields.contains("borderTop"));
        assert!(fields.contains("borderBottom"));
        assert!(!fields.contains("borderLeft"));
        assert!(
            cs["tableCellStyle"]["borderTop"]["width"]["magnitude"]
                .as_f64()
                .unwrap()
                == 1.0,
            "default width 1pt"
        );
        // border color is the Docs OptionalColor shape: { color: { rgbColor: {..} } }
        assert!(
            cs["tableCellStyle"]["borderTop"]["color"]["color"]["rgbColor"].is_object(),
            "border color must be an OptionalColor with rgbColor; got {}",
            cs["tableCellStyle"]["borderTop"]["color"]
        );
    }
}
