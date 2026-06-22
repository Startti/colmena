//! Pure mapping from a declarative `FormatSpec` to Google Sheets
//! `spreadsheets.batchUpdate` requests. No I/O. See spec
//! `docs/superpowers/specs/2026-06-22-gsheets-cell-formatting-design.md`.

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

/// A 0-based, end-exclusive grid rectangle on one sheet.
#[derive(Debug, Clone, PartialEq)]
pub struct GridRange {
    pub sheet_id: i64,
    pub start_row: u32,
    pub end_row: u32,
    pub start_col: u32,
    pub end_col: u32,
}

#[derive(Debug, Clone, Deserialize, JsonSchema, Default)]
pub struct FormatSpec {
    pub text: Option<TextFormat>,
    /// Hex `#RRGGBB` cell background.
    pub background_color: Option<String>,
    /// LEFT | CENTER | RIGHT
    pub horizontal_alignment: Option<String>,
    /// TOP | MIDDLE | BOTTOM
    pub vertical_alignment: Option<String>,
    pub number_format: Option<NumberFormat>,
    /// OVERFLOW | CLIP | WRAP
    pub wrap: Option<String>,
    pub borders: Option<Borders>,
    /// Pixel width applied to the columns spanned by the range.
    pub column_width_px: Option<u32>,
    /// Pixel height applied to the rows spanned by the range.
    pub row_height_px: Option<u32>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema, Default)]
pub struct TextFormat {
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub underline: Option<bool>,
    pub strikethrough: Option<bool>,
    pub font_size: Option<u32>,
    pub font_family: Option<String>,
    /// Hex `#RRGGBB` text (foreground) color.
    pub color: Option<String>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema, Default)]
pub struct NumberFormat {
    /// NUMBER | CURRENCY | PERCENT | DATE | TIME | DATE_TIME | TEXT | SCIENTIFIC
    pub r#type: String,
    pub pattern: Option<String>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema, Default)]
pub struct Borders {
    pub top: Option<BorderSide>,
    pub bottom: Option<BorderSide>,
    pub left: Option<BorderSide>,
    pub right: Option<BorderSide>,
    pub inner_horizontal: Option<BorderSide>,
    pub inner_vertical: Option<BorderSide>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema, Default)]
pub struct BorderSide {
    /// SOLID | SOLID_MEDIUM | SOLID_THICK | DASHED | DOTTED | DOUBLE
    pub style: String,
    /// Hex `#RRGGBB`. Defaults to black when omitted.
    pub color: Option<String>,
}

/// Error returned by the pure mappers (mapped to InvalidArgs by the dispatcher).
#[derive(Debug, PartialEq)]
pub struct FormatError(pub String);

/// Parse `#RRGGBB` (or `RRGGBB`) → Sheets `RgbColor` JSON with 0.0–1.0 floats.
/// Note: Sheets `RgbColor` is a bare `{red,green,blue}` object, unlike the
/// Docs API `OptionalColor` wrapper.
pub fn hex_to_rgb(hex: &str) -> Result<Value, FormatError> {
    let h = hex.strip_prefix('#').unwrap_or(hex);
    if h.len() != 6 || !h.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(FormatError(format!(
            "invalid hex color: {hex:?} (expected #RRGGBB)"
        )));
    }
    let comp = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).unwrap() as f64 / 255.0;
    Ok(json!({ "red": comp(0), "green": comp(2), "blue": comp(4) }))
}

/// Parse an A1 range against `sheet_id` → 0-based, end-exclusive `GridRange`.
/// Supports "A1", "A1:D5", whole columns "B:D", and whole rows "2:5".
/// For whole-column ranges, row bounds are 0..0 (no row indices emitted);
/// for whole-row ranges, col bounds are 0..0.
pub fn a1_to_grid_range(sheet_id: i64, a1: &str) -> Result<GridRange, FormatError> {
    if a1.trim().is_empty() {
        return Err(FormatError("empty A1 range".into()));
    }
    let (start, end) = match a1.split_once(':') {
        Some((s, e)) => (s, e),
        None => (a1, a1),
    };
    let (sc, sr) = parse_a1_cell(start)?;
    let (ec, er) = parse_a1_cell(end)?;
    let (start_col, end_col) = match (sc, ec) {
        (Some(a), Some(b)) => (a.min(b), a.max(b) + 1),
        _ => (0, 0),
    };
    let (start_row, end_row) = match (sr, er) {
        (Some(a), Some(b)) => (a.min(b), a.max(b) + 1),
        _ => (0, 0),
    };
    Ok(GridRange {
        sheet_id,
        start_row,
        end_row,
        start_col,
        end_col,
    })
}

/// Parse one A1 token ("A", "1", "B12") → (col_index?, row_index?), 0-based.
fn parse_a1_cell(tok: &str) -> Result<(Option<u32>, Option<u32>), FormatError> {
    let letters: String = tok
        .chars()
        .take_while(|c| c.is_ascii_alphabetic())
        .collect();
    let digits: String = tok
        .chars()
        .skip_while(|c| c.is_ascii_alphabetic())
        .collect();
    if letters.is_empty() && digits.is_empty() {
        return Err(FormatError(format!("invalid A1 token: {tok:?}")));
    }
    let col = if letters.is_empty() {
        None
    } else {
        let mut n: u32 = 0;
        for c in letters.chars() {
            n = n
                .checked_mul(26)
                .and_then(|x| x.checked_add(c.to_ascii_uppercase() as u32 - 'A' as u32 + 1))
                .ok_or_else(|| FormatError(format!("column reference too large in {tok:?}")))?;
        }
        Some(n - 1)
    };
    let row = if digits.is_empty() {
        None
    } else {
        Some(
            digits
                .parse::<u32>()
                .map_err(|_| FormatError(format!("invalid row in {tok:?}")))?
                - 1,
        )
    };
    Ok((col, row))
}

fn grid_json(g: &GridRange) -> Value {
    let mut m = serde_json::Map::new();
    m.insert("sheetId".into(), json!(g.sheet_id));
    if g.end_row > g.start_row {
        m.insert("startRowIndex".into(), json!(g.start_row));
        m.insert("endRowIndex".into(), json!(g.end_row));
    }
    if g.end_col > g.start_col {
        m.insert("startColumnIndex".into(), json!(g.start_col));
        m.insert("endColumnIndex".into(), json!(g.end_col));
    }
    Value::Object(m)
}

fn border_json(side: &BorderSide) -> Result<Value, FormatError> {
    let color = match &side.color {
        Some(h) => hex_to_rgb(h)?,
        None => json!({ "red": 0.0, "green": 0.0, "blue": 0.0 }),
    };
    Ok(json!({ "style": side.style, "color": color }))
}

/// Map a `FormatSpec` over `grid` to Sheets batchUpdate requests.
/// Errors if the spec carries no attributes (fail loud, not silent no-op).
pub fn build_format_requests(
    grid: &GridRange,
    spec: &FormatSpec,
) -> Result<Vec<Value>, FormatError> {
    let mut out = Vec::new();

    let mut fmt = serde_json::Map::new();
    let mut fields: Vec<String> = Vec::new();

    if let Some(bg) = &spec.background_color {
        fmt.insert("backgroundColor".into(), hex_to_rgb(bg)?);
        fields.push("backgroundColor".into());
    }
    if let Some(t) = &spec.text {
        let mut tf = serde_json::Map::new();
        if let Some(b) = t.bold {
            tf.insert("bold".into(), json!(b));
            fields.push("textFormat.bold".into());
        }
        if let Some(b) = t.italic {
            tf.insert("italic".into(), json!(b));
            fields.push("textFormat.italic".into());
        }
        if let Some(b) = t.underline {
            tf.insert("underline".into(), json!(b));
            fields.push("textFormat.underline".into());
        }
        if let Some(b) = t.strikethrough {
            tf.insert("strikethrough".into(), json!(b));
            fields.push("textFormat.strikethrough".into());
        }
        if let Some(s) = t.font_size {
            tf.insert("fontSize".into(), json!(s));
            fields.push("textFormat.fontSize".into());
        }
        if let Some(f) = &t.font_family {
            tf.insert("fontFamily".into(), json!(f));
            fields.push("textFormat.fontFamily".into());
        }
        if let Some(c) = &t.color {
            tf.insert("foregroundColor".into(), hex_to_rgb(c)?);
            fields.push("textFormat.foregroundColor".into());
        }
        if !tf.is_empty() {
            fmt.insert("textFormat".into(), Value::Object(tf));
        }
    }
    if let Some(a) = &spec.horizontal_alignment {
        fmt.insert("horizontalAlignment".into(), json!(a));
        fields.push("horizontalAlignment".into());
    }
    if let Some(a) = &spec.vertical_alignment {
        fmt.insert("verticalAlignment".into(), json!(a));
        fields.push("verticalAlignment".into());
    }
    if let Some(nf) = &spec.number_format {
        let mut n = serde_json::Map::new();
        n.insert("type".into(), json!(nf.r#type));
        if let Some(p) = &nf.pattern {
            n.insert("pattern".into(), json!(p));
        }
        fmt.insert("numberFormat".into(), Value::Object(n));
        fields.push("numberFormat".into());
    }
    if let Some(w) = &spec.wrap {
        fmt.insert("wrapStrategy".into(), json!(w));
        fields.push("wrapStrategy".into());
    }
    if !fmt.is_empty() {
        out.push(json!({ "repeatCell": {
            "range": grid_json(grid),
            "cell": { "userEnteredFormat": Value::Object(fmt) },
            "fields": format!("userEnteredFormat({})", fields.join(",")),
        }}));
    }

    if let Some(b) = &spec.borders {
        let mut ub = serde_json::Map::new();
        ub.insert("range".into(), grid_json(grid));
        for (key, side) in [
            ("top", &b.top),
            ("bottom", &b.bottom),
            ("left", &b.left),
            ("right", &b.right),
            ("innerHorizontal", &b.inner_horizontal),
            ("innerVertical", &b.inner_vertical),
        ] {
            if let Some(s) = side {
                ub.insert(key.into(), border_json(s)?);
            }
        }
        if ub.len() > 1 {
            out.push(json!({ "updateBorders": Value::Object(ub) }));
        }
    }

    if let Some(px) = spec.column_width_px {
        out.push(json!({ "updateDimensionProperties": {
            "range": { "sheetId": grid.sheet_id, "dimension": "COLUMNS",
                       "startIndex": grid.start_col, "endIndex": grid.end_col.max(grid.start_col + 1) },
            "properties": { "pixelSize": px },
            "fields": "pixelSize",
        }}));
    }
    if let Some(px) = spec.row_height_px {
        out.push(json!({ "updateDimensionProperties": {
            "range": { "sheetId": grid.sheet_id, "dimension": "ROWS",
                       "startIndex": grid.start_row, "endIndex": grid.end_row.max(grid.start_row + 1) },
            "properties": { "pixelSize": px },
            "fields": "pixelSize",
        }}));
    }

    if out.is_empty() {
        return Err(FormatError("format has no attributes".into()));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_to_rgb_parses_with_and_without_hash() {
        assert_eq!(
            hex_to_rgb("#FFFFFF").unwrap(),
            json!({"red":1.0,"green":1.0,"blue":1.0})
        );
        assert_eq!(
            hex_to_rgb("000000").unwrap(),
            json!({"red":0.0,"green":0.0,"blue":0.0})
        );
        assert!(hex_to_rgb("#ZZZ").is_err());
        assert!(hex_to_rgb("#FFF").is_err()); // only 6-digit supported
    }

    #[test]
    fn a1_single_cell_and_range_and_full_column() {
        assert_eq!(
            a1_to_grid_range(7, "A1").unwrap(),
            GridRange {
                sheet_id: 7,
                start_row: 0,
                end_row: 1,
                start_col: 0,
                end_col: 1
            }
        );
        assert_eq!(
            a1_to_grid_range(7, "A1:D1").unwrap(),
            GridRange {
                sheet_id: 7,
                start_row: 0,
                end_row: 1,
                start_col: 0,
                end_col: 4
            }
        );
        let col = a1_to_grid_range(7, "B:B").unwrap();
        assert_eq!((col.start_col, col.end_col), (1, 2));
        assert!(a1_to_grid_range(7, "").is_err());
    }

    #[test]
    fn a1_multi_letter_and_garbage() {
        let aa = a1_to_grid_range(0, "AA1").unwrap();
        assert_eq!((aa.start_col, aa.end_col), (26, 27));
        assert!(a1_to_grid_range(0, "AAAAAAAA1").is_err()); // overflow -> error, not panic
        assert!(a1_to_grid_range(0, "!!").is_err());
    }

    fn gr() -> GridRange {
        GridRange {
            sheet_id: 0,
            start_row: 0,
            end_row: 1,
            start_col: 0,
            end_col: 4,
        }
    }

    #[test]
    fn background_and_bold_produce_one_repeat_cell_with_composite_mask() {
        let spec = FormatSpec {
            background_color: Some("#000000".into()),
            text: Some(TextFormat {
                bold: Some(true),
                ..Default::default()
            }),
            ..Default::default()
        };
        let reqs = build_format_requests(&gr(), &spec).unwrap();
        assert_eq!(reqs.len(), 1);
        let rc = &reqs[0]["repeatCell"];
        assert_eq!(rc["cell"]["userEnteredFormat"]["textFormat"]["bold"], true);
        assert!(rc["cell"]["userEnteredFormat"]["backgroundColor"].is_object());
        let fields = rc["fields"].as_str().unwrap();
        assert!(fields.contains("backgroundColor"));
        assert!(fields.contains("textFormat.bold"));
    }

    #[test]
    fn borders_and_column_width_produce_separate_requests() {
        let spec = FormatSpec {
            borders: Some(Borders {
                top: Some(BorderSide {
                    style: "SOLID".into(),
                    color: None,
                }),
                ..Default::default()
            }),
            column_width_px: Some(120),
            ..Default::default()
        };
        let reqs = build_format_requests(&gr(), &spec).unwrap();
        assert!(reqs.iter().any(|r| r.get("updateBorders").is_some()));
        let dim = reqs
            .iter()
            .find(|r| r.get("updateDimensionProperties").is_some())
            .unwrap();
        assert_eq!(
            dim["updateDimensionProperties"]["range"]["dimension"],
            "COLUMNS"
        );
        assert_eq!(
            dim["updateDimensionProperties"]["properties"]["pixelSize"],
            120
        );
    }

    #[test]
    fn empty_spec_is_error() {
        assert!(build_format_requests(&gr(), &FormatSpec::default()).is_err());
    }

    #[test]
    fn alignment_number_wrap_and_row_height_fan_out() {
        let spec = FormatSpec {
            horizontal_alignment: Some("CENTER".into()),
            vertical_alignment: Some("MIDDLE".into()),
            number_format: Some(NumberFormat {
                r#type: "CURRENCY".into(),
                pattern: Some("\"$\"#,##0".into()),
            }),
            wrap: Some("WRAP".into()),
            row_height_px: Some(24),
            ..Default::default()
        };
        let reqs = build_format_requests(&gr(), &spec).unwrap();
        let rc = reqs.iter().find(|r| r.get("repeatCell").is_some()).unwrap();
        let uef = &rc["repeatCell"]["cell"]["userEnteredFormat"];
        assert_eq!(uef["horizontalAlignment"], "CENTER");
        assert_eq!(uef["verticalAlignment"], "MIDDLE");
        assert_eq!(uef["numberFormat"]["type"], "CURRENCY");
        assert_eq!(uef["wrapStrategy"], "WRAP");
        let fields = rc["repeatCell"]["fields"].as_str().unwrap();
        assert!(
            fields.contains("horizontalAlignment")
                && fields.contains("numberFormat")
                && fields.contains("wrapStrategy")
        );
        let dim = reqs
            .iter()
            .find(|r| r.get("updateDimensionProperties").is_some())
            .unwrap();
        assert_eq!(
            dim["updateDimensionProperties"]["range"]["dimension"],
            "ROWS"
        );
        assert_eq!(
            dim["updateDimensionProperties"]["properties"]["pixelSize"],
            24
        );
    }

    #[test]
    fn borders_with_explicit_color_and_inner() {
        let spec = FormatSpec {
            borders: Some(Borders {
                top: Some(BorderSide {
                    style: "SOLID".into(),
                    color: Some("#FF0000".into()),
                }),
                inner_horizontal: Some(BorderSide {
                    style: "DASHED".into(),
                    color: None,
                }),
                ..Default::default()
            }),
            ..Default::default()
        };
        let reqs = build_format_requests(&gr(), &spec).unwrap();
        let ub = &reqs
            .iter()
            .find(|r| r.get("updateBorders").is_some())
            .unwrap()["updateBorders"];
        assert_eq!(ub["top"]["style"], "SOLID");
        assert_eq!(ub["top"]["color"]["red"], 1.0);
        assert_eq!(ub["innerHorizontal"]["style"], "DASHED");
    }
}
