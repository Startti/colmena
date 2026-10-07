//! The cells of one worksheet, row by row, as a stream.
//!
//! The sheet part is parsed as it is inflated and handed on one row at a time;
//! nothing but the current row is ever held, however many rows the sheet has.
//! A row arrives as its non-empty cells with their column index (empty cells are
//! not delivered), so a sparse row costs what it holds.
//!
//! Decisions, each with a test.
//! - **Formulas are never evaluated.** A formula cell is read as the value Excel
//!   cached next to it (`v`); one with no cached value is empty. The formula text
//!   (`f`) is skipped, never parsed.
//! - **Merged cells are not expanded.** Excel stores a merged range's value in its
//!   top-left cell and leaves the others empty; that is what is read (the
//!   `mergeCells` element is ignored). Filling the range would invent values.
//! - **Numbers stay numbers.** Strings (shared, inline, or a formula's text) are
//!   text; a boolean is a boolean; an error (`#DIV/0!`) is its text, so it is
//!   visible and makes a numeric column text instead of vanishing; an empty
//!   string is empty, as in a CSV.
//! - **Dates** are numbers whose style says so (see `xlsx_styles`), or an ISO 8601
//!   cell (`t="d"`).
//! - **Rows and columns must be in order.** A row number or a column that does not
//!   increase is a corrupt part.
//! - Blank rows (no non-empty cell, or numbers skipped) are counted, not delivered.

use crate::tabular_prepare::xlsx_package::{attribute, next_event, text_of, Package};
use crate::tabular_prepare::xlsx_spool::{Cap, Invalid, XlsxError};
use crate::tabular_prepare::xlsx_strings::{unescape_ooxml, SharedStrings};
use crate::tabular_prepare::xlsx_styles::{temporal, Styles, Temporal};
use quick_xml::events::Event;

/// Rows of a sheet (Excel's own limit).
pub const MAX_ROWS: u32 = 1_048_576;
/// Columns of a sheet (Excel's own limit, and the CSV's).
pub const MAX_COLUMNS: usize = 16_384;
/// Cells (`c` elements, empty ones too) of a workbook, at most; the same number
/// bounds one sheet. Measured on one generated workbook (spike item 4).
pub const MAX_CELLS: u64 = 50_000_000;
/// Bytes of one cell's text: Excel's 32,767 characters at four bytes each.
pub const MAX_CELL_BYTES: usize = 131_072;
/// Bytes of text in one row: the CSV's record limit.
pub const MAX_ROW_BYTES: usize = crate::tabular_prepare::scan::MAX_RECORD_BYTES;

/// The limits of a sheet read. The defaults are the constants above; tests lower
/// them, nothing outside the crate can (the fields are crate-private).
#[derive(Debug, Clone, Copy)]
pub struct SheetLimits {
    pub(crate) max_rows: u32,
    pub(crate) max_columns: usize,
    pub(crate) max_cells: u64,
    pub(crate) max_cell_bytes: usize,
    pub(crate) max_row_bytes: usize,
}

impl Default for SheetLimits {
    fn default() -> Self {
        Self {
            max_rows: MAX_ROWS,
            max_columns: MAX_COLUMNS,
            max_cells: MAX_CELLS,
            max_cell_bytes: MAX_CELL_BYTES,
            max_row_bytes: MAX_ROW_BYTES,
        }
    }
}

/// One cell's value.
#[derive(Debug, Clone, PartialEq)]
pub enum Cell {
    Text(Box<str>),
    Number(f64),
    Bool(bool),
    Temporal(Temporal),
}

/// The non-empty cells of a row, by column index.
pub type RowCells = Vec<(usize, Cell)>;

/// What the reader calls with each row: its number from 1 and its cells; the
/// answer is whether to go on.
pub(crate) type OnRow<'a> = &'a mut dyn FnMut(u32, &mut RowCells) -> Result<bool, XlsxError>;

/// What a sheet read counted.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SheetStats {
    /// `c` elements read, empty ones included.
    pub cells: u64,
    /// Rows with at least one non-empty cell, as delivered.
    pub rows: u64,
    /// Rows skipped because they hold nothing (absent, or only empty cells).
    pub blank_rows: u64,
}

/// What a read of a workbook's cells needs besides the sheet itself.
pub struct SheetContext<'a> {
    pub strings: &'a SharedStrings,
    pub styles: &'a Styles,
    pub date1904: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Number,
    Shared,
    Str,
    Inline,
    Bool,
    Error,
    IsoDate,
}

/// The column index (from zero) of a cell reference such as `AB12`; `None` when
/// it has no letters.
fn column_of(reference: &str) -> Option<usize> {
    let mut column = 0usize;
    let mut letters = 0;
    for b in reference.bytes().take_while(u8::is_ascii_alphabetic) {
        column = column
            .saturating_mul(26)
            .saturating_add(usize::from(b.to_ascii_uppercase() - b'A') + 1);
        letters += 1;
    }
    (letters > 0).then(|| column - 1)
}

/// An ISO 8601 date or date and time without a zone, as a value.
fn iso_date(text: &str) -> Option<Temporal> {
    use chrono::{NaiveDate, NaiveDateTime};
    let epoch = NaiveDate::from_ymd_opt(1970, 1, 1)?;
    if let Ok(d) = NaiveDate::parse_from_str(text, "%Y-%m-%d") {
        return i32::try_from(d.signed_duration_since(epoch).num_days())
            .ok()
            .map(Temporal::Date);
    }
    NaiveDateTime::parse_from_str(text, "%Y-%m-%dT%H:%M:%S%.f")
        .ok()
        .map(|t| Temporal::Timestamp(t.and_utc().timestamp_micros()))
}

struct Pending {
    column: usize,
    kind: Kind,
    style: usize,
    value: String,
    inline: String,
}

fn finish(p: Pending, ctx: &SheetContext<'_>) -> Result<Option<Cell>, XlsxError> {
    let text = |s: &str| -> Option<Cell> { (!s.is_empty()).then(|| Cell::Text(s.into())) };
    Ok(match p.kind {
        Kind::Inline => text(&p.inline),
        Kind::Str | Kind::Error => text(&p.value),
        Kind::Shared => {
            if p.value.is_empty() {
                return Ok(None);
            }
            let index: usize = p
                .value
                .trim()
                .parse()
                .map_err(|_| XlsxError::Invalid(Invalid::BadCell))?;
            let s = ctx
                .strings
                .get(index)
                .ok_or(XlsxError::Invalid(Invalid::BadCell))?;
            text(s)
        }
        Kind::Bool => {
            (!p.value.is_empty()).then(|| Cell::Bool(matches!(p.value.trim(), "1" | "true")))
        }
        Kind::IsoDate => match iso_date(p.value.trim()) {
            Some(t) => Some(Cell::Temporal(t)),
            None => text(&p.value),
        },
        Kind::Number => {
            if p.value.trim().is_empty() {
                return Ok(None);
            }
            match p.value.trim().parse::<f64>() {
                Ok(n) => match temporal(n, ctx.date1904, ctx.styles.format(p.style)) {
                    Some(t) => Some(Cell::Temporal(t)),
                    None => Some(Cell::Number(n)),
                },
                Err(_) => text(&p.value),
            }
        }
    })
}

/// Reads the sheet in `part`, handing each row that holds a value to `on_row` as
/// `(row number from 1, cells by column)`. `on_row` returns whether to go on; a
/// `false` ends the read without error (the caller has seen enough).
pub fn read_sheet<F>(
    pkg: &mut Package,
    part: &str,
    ctx: &SheetContext<'_>,
    mut on_row: F,
) -> Result<SheetStats, XlsxError>
where
    F: FnMut(u32, &mut RowCells) -> Result<bool, XlsxError>,
{
    read_sheet_with(pkg, part, ctx, &SheetLimits::default(), &mut on_row)
}

/// Where a read of a sheet is: counters, and the limits it enforces.
struct Reading<'l> {
    limits: &'l SheetLimits,
    stats: SheetStats,
    row_number: u32,
    row_bytes: usize,
    next_column: usize,
}

impl Reading<'_> {
    /// A `row` element begins: its number, which must increase, and the rows
    /// skipped before it.
    fn begin_row(&mut self, e: &quick_xml::events::BytesStart) -> Result<(), XlsxError> {
        let bad = XlsxError::Invalid(Invalid::BadCell);
        let number = match attribute(e, b"r")? {
            Some(r) => r.trim().parse::<u32>().map_err(|_| bad)?,
            None => self.row_number + 1,
        };
        if number <= self.row_number {
            return Err(bad);
        }
        if number > self.limits.max_rows {
            return Err(XlsxError::TooLarge(Cap::Rows));
        }
        self.stats.blank_rows += u64::from(number - self.row_number - 1);
        self.row_number = number;
        (self.next_column, self.row_bytes) = (0, 0);
        Ok(())
    }

    /// A `c` element begins: its column, which must increase, and its type.
    fn begin_cell(&mut self, e: &quick_xml::events::BytesStart) -> Result<Pending, XlsxError> {
        let bad = XlsxError::Invalid(Invalid::BadCell);
        self.stats.cells += 1;
        if self.stats.cells > self.limits.max_cells {
            return Err(XlsxError::TooLarge(Cap::Cells));
        }
        let column = match attribute(e, b"r")?.as_deref().map(column_of) {
            Some(Some(c)) => c,
            Some(None) => return Err(bad),
            None => self.next_column,
        };
        if column < self.next_column {
            return Err(bad);
        }
        if column >= self.limits.max_columns {
            return Err(XlsxError::TooLarge(Cap::Columns));
        }
        self.next_column = column + 1;
        let kind = match attribute(e, b"t")?.as_deref() {
            Some("s") => Kind::Shared,
            Some("str") => Kind::Str,
            Some("inlineStr") => Kind::Inline,
            Some("b") => Kind::Bool,
            Some("e") => Kind::Error,
            Some("d") => Kind::IsoDate,
            _ => Kind::Number,
        };
        let style = attribute(e, b"s")?
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        Ok(Pending {
            column,
            kind,
            style,
            value: String::new(),
            inline: String::new(),
        })
    }
}

pub(crate) fn read_sheet_with(
    pkg: &mut Package,
    part: &str,
    ctx: &SheetContext<'_>,
    limits: &SheetLimits,
    on_row: OnRow<'_>,
) -> Result<SheetStats, XlsxError> {
    let mut at = Reading {
        limits,
        stats: SheetStats::default(),
        row_number: 0,
        row_bytes: 0,
        next_column: 0,
    };
    let mut cells = RowCells::new();
    let mut pending: Option<Pending> = None;
    let (mut in_v, mut in_is, mut in_t, mut in_phonetic) = (false, false, false, false);
    let mut reader = pkg.xml(part)?;
    let mut buf = Vec::new();
    loop {
        match next_event(&mut reader, &mut buf)? {
            Event::Eof => break,
            Event::Start(e) if e.local_name().as_ref() == b"row" => at.begin_row(&e)?,
            // A row with no cells at all.
            Event::Empty(e) if e.local_name().as_ref() == b"row" => {
                at.begin_row(&e)?;
                at.stats.blank_rows += 1;
            }
            Event::End(e) if e.local_name().as_ref() == b"row" => {
                if cells.is_empty() {
                    at.stats.blank_rows += 1;
                } else {
                    at.stats.rows += 1;
                    let more = on_row(at.row_number, &mut cells)?;
                    cells.clear();
                    if !more {
                        break;
                    }
                }
            }
            Event::Start(e) if e.local_name().as_ref() == b"c" => {
                pending = Some(at.begin_cell(&e)?);
            }
            // A cell with no value: counted and placed, nothing to deliver.
            Event::Empty(e) if e.local_name().as_ref() == b"c" => {
                at.begin_cell(&e)?;
            }
            Event::End(e) if e.local_name().as_ref() == b"c" => {
                if let Some(p) = pending.take() {
                    let column = p.column;
                    if let Some(cell) = finish(p, ctx)? {
                        if let Cell::Text(t) = &cell {
                            if t.len() > limits.max_cell_bytes {
                                return Err(XlsxError::Invalid(Invalid::CellTooLong));
                            }
                            at.row_bytes += t.len();
                            if at.row_bytes > limits.max_row_bytes {
                                return Err(XlsxError::Invalid(Invalid::RowTooLong));
                            }
                        }
                        cells.push((column, cell));
                    }
                }
            }
            Event::Start(e) => match e.local_name().as_ref() {
                b"v" if pending.is_some() => in_v = true,
                b"is" if pending.is_some() => in_is = true,
                b"t" if in_is => in_t = true,
                b"rPh" if in_is => in_phonetic = true,
                _ => {}
            },
            Event::End(e) => match e.local_name().as_ref() {
                b"v" => in_v = false,
                b"is" => in_is = false,
                b"t" => in_t = false,
                b"rPh" => in_phonetic = false,
                _ => {}
            },
            Event::Text(t) => {
                if let Some(p) = pending.as_mut() {
                    if in_v {
                        p.value.push_str(&text_of(&t)?);
                    } else if in_is && in_t && !in_phonetic {
                        p.inline.push_str(&unescape_ooxml(&text_of(&t)?));
                    }
                }
            }
            _ => {}
        }
    }
    Ok(at.stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tabular_prepare::xlsx_package::XlsxLimits;
    use crate::tabular_prepare::xlsx_spool::spool_stream;
    use crate::tabular_prepare::xlsx_strings::read_shared_strings;
    use crate::tabular_prepare::xlsx_styles::read_styles;
    use crate::tabular_prepare::xlsx_workbook::read_workbook;
    use crate::tabular_prepare::xlsxfix::Wb;
    use bytes::Bytes;
    use std::sync::atomic::AtomicU64;
    use tokio_util::sync::CancellationToken;

    type Rows = Vec<(u32, RowCells)>;

    async fn open_with(bytes: Vec<u8>, limits: &XlsxLimits) -> Package {
        let dir = tempfile::tempdir().unwrap();
        let stream = Box::pin(futures::stream::iter(vec![Ok(Bytes::from(bytes))]));
        let read = AtomicU64::new(0);
        let cancel = CancellationToken::new();
        let spooled = spool_stream(dir.path(), stream, None, 1 << 30, &cancel, &read)
            .await
            .unwrap();
        Package::open_with(spooled, limits).unwrap()
    }

    /// Reads the first sheet of a workbook through the whole stack so far.
    async fn read_with(
        bytes: Vec<u8>,
        xlsx: &XlsxLimits,
        sheet: &SheetLimits,
    ) -> Result<(Rows, SheetStats), XlsxError> {
        let mut pkg = open_with(bytes, xlsx).await;
        let wb = read_workbook(&mut pkg)?;
        let strings = match &wb.shared_strings {
            Some(part) => read_shared_strings(&mut pkg, part)?,
            None => SharedStrings::none(),
        };
        let styles = match &wb.styles {
            Some(part) => read_styles(&mut pkg, part)?,
            None => Styles::none(),
        };
        let ctx = SheetContext {
            strings: &strings,
            styles: &styles,
            date1904: wb.date1904,
        };
        let mut rows: Rows = Vec::new();
        let stats = read_sheet_with(
            &mut pkg,
            &wb.sheets[0].part,
            &ctx,
            sheet,
            &mut |n, cells| {
                rows.push((n, std::mem::take(cells)));
                Ok(true)
            },
        )?;
        Ok((rows, stats))
    }

    async fn read(bytes: Vec<u8>) -> Result<(Rows, SheetStats), XlsxError> {
        read_with(bytes, &XlsxLimits::default(), &SheetLimits::default()).await
    }

    fn text(s: &str) -> Cell {
        Cell::Text(s.into())
    }

    #[tokio::test]
    async fn values_are_read_with_the_type_the_cell_declares() {
        let rows = concat!(
            "<row r=\"1\">",
            "<c r=\"A1\"><v>42</v></c>",
            "<c r=\"B1\" t=\"s\"><v>1</v></c>",
            "<c r=\"C1\" t=\"inlineStr\"><is><r><t>in</t></r><r><t xml:space=\"preserve\">line </t></r><rPh><t>x</t></rPh></is></c>",
            "<c r=\"D1\" t=\"str\"><v>formula text</v></c>",
            "<c r=\"E1\" t=\"b\"><v>1</v></c>",
            "<c r=\"F1\" t=\"b\"><v>0</v></c>",
            "<c r=\"G1\" t=\"e\"><v>#DIV/0!</v></c>",
            "<c r=\"H1\" t=\"s\"><v>0</v></c>",
            "<c r=\"I1\"><v>-1.5E-3</v></c>",
            "</row>"
        );
        let bytes = Wb::new().sheet("A", rows).shared(&["", "shared"]).build();
        let (got, stats) = read(bytes).await.unwrap();
        assert_eq!(
            got,
            vec![(
                1,
                vec![
                    (0, Cell::Number(42.0)),
                    (1, text("shared")),
                    (2, text("inline ")),
                    (3, text("formula text")),
                    (4, Cell::Bool(true)),
                    (5, Cell::Bool(false)),
                    (6, text("#DIV/0!")),
                    // The empty shared string at index 0 is empty, as in a CSV.
                    (8, Cell::Number(-0.0015)),
                ]
            )]
        );
        assert_eq!((stats.cells, stats.rows, stats.blank_rows), (9, 1, 0));
    }

    #[tokio::test]
    async fn formulas_use_the_cached_value_and_are_never_evaluated() {
        let rows = concat!(
            "<row r=\"1\">",
            "<c r=\"A1\"><f>1+1</f><v>42</v></c>",
            "<c r=\"B1\"><f>SUM(A:A)</f></c>",
            "<c r=\"C1\" t=\"str\"><f>HYPERLINK(\"http://evil\",\"x\")</f><v>cached</v></c>",
            "<c r=\"D1\"><f t=\"shared\" si=\"0\"/><v>7</v></c>",
            "<c r=\"E1\"><f>1/0</f><v>not a number</v></c>",
            "</row>"
        );
        let (got, _) = read(Wb::new().sheet("A", rows).build()).await.unwrap();
        assert_eq!(
            got[0].1,
            vec![
                (0, Cell::Number(42.0)),
                // No cached value: empty, not computed.
                (2, text("cached")),
                (3, Cell::Number(7.0)),
                // A cached value that is not a number stays what it says.
                (4, text("not a number")),
            ]
        );
    }

    #[tokio::test]
    async fn a_merged_range_keeps_its_value_in_the_top_left_cell_only() {
        let sheet = concat!(
            "<worksheet><sheetData>",
            "<row r=\"1\"><c r=\"A1\" t=\"inlineStr\"><is><t>merged</t></is></c><c r=\"B1\"/></row>",
            "<row r=\"2\"><c r=\"A2\"/><c r=\"B2\"/></row>",
            "</sheetData><mergeCells count=\"1\"><mergeCell ref=\"A1:B2\"/></mergeCells></worksheet>"
        );
        let (got, stats) = read(Wb::new().sheet("A", sheet).build()).await.unwrap();
        assert_eq!(got, vec![(1, vec![(0, text("merged"))])]);
        assert_eq!((stats.cells, stats.rows, stats.blank_rows), (4, 1, 1));
    }

    #[tokio::test]
    async fn sparse_rows_and_columns_are_read_and_blank_rows_are_counted() {
        let rows = concat!(
            "<row r=\"2\"><c r=\"C2\"><v>1</v></c><c><v>2</v></c><c r=\"F2\"><v>3</v></c></row>",
            "<row r=\"3\"/>",
            "<row r=\"4\"><c r=\"A4\"/></row>",
            "<row r=\"9\"><c r=\"AB9\"><v>4</v></c></row>"
        );
        let (got, stats) = read(Wb::new().sheet("A", rows).build()).await.unwrap();
        assert_eq!(
            got,
            vec![
                (
                    2,
                    vec![
                        (2, Cell::Number(1.0)),
                        (3, Cell::Number(2.0)),
                        (5, Cell::Number(3.0))
                    ]
                ),
                (9, vec![(27, Cell::Number(4.0))]),
            ]
        );
        // Row 1 absent, row 3 empty, row 4 only an empty cell, rows 5 to 8 absent.
        assert_eq!((stats.rows, stats.blank_rows), (2, 1 + 1 + 1 + 4));
    }

    #[tokio::test]
    async fn rows_or_columns_out_of_order_and_bad_references_are_a_corrupt_part() {
        let bad = |rows: &str| read(Wb::new().sheet("A", rows).shared(&["a"]).build());
        let cases = [
            "<row r=\"3\"/><row r=\"2\"/>",
            "<row r=\"2\"/><row r=\"2\"/>",
            "<row r=\"x\"/>",
            "<row r=\"1\"><c r=\"B1\"><v>1</v></c><c r=\"A1\"><v>1</v></c></row>",
            "<row r=\"1\"><c r=\"12\"><v>1</v></c></row>",
            "<row r=\"1\"><c r=\"A1\" t=\"s\"><v>7</v></c></row>",
            "<row r=\"1\"><c r=\"A1\" t=\"s\"><v>x</v></c></row>",
        ];
        for rows in cases {
            let r = bad(rows).await;
            assert_eq!(
                r.unwrap_err(),
                XlsxError::Invalid(Invalid::BadCell),
                "{rows}"
            );
        }
    }

    #[tokio::test]
    async fn the_caller_can_stop_the_read() {
        let rows: String = (1..=100)
            .map(|i| format!("<row r=\"{i}\"><c r=\"A{i}\"><v>{i}</v></c></row>"))
            .collect();
        let mut pkg = open_with(Wb::new().sheet("A", &rows).build(), &XlsxLimits::default()).await;
        let wb = read_workbook(&mut pkg).unwrap();
        let strings = SharedStrings::none();
        let styles = Styles::none();
        let ctx = SheetContext {
            strings: &strings,
            styles: &styles,
            date1904: false,
        };
        let mut seen = 0;
        let stats = read_sheet(&mut pkg, &wb.sheets[0].part, &ctx, |_, _| {
            seen += 1;
            Ok(seen < 3)
        })
        .unwrap();
        assert_eq!((seen, stats.rows), (3, 3));
    }

    #[tokio::test]
    async fn a_number_is_a_date_only_when_its_style_says_so() {
        // Styles: 0 general, 1 a built-in date, 2 a custom date-time, 3 plain custom.
        let rows = concat!(
            "<row r=\"1\">",
            "<c r=\"A1\" s=\"1\"><v>44197</v></c>",
            "<c r=\"B1\" s=\"2\"><v>44197.5</v></c>",
            "<c r=\"C1\" s=\"0\"><v>44197</v></c>",
            "<c r=\"D1\" s=\"3\"><v>44197</v></c>",
            "<c r=\"E1\" s=\"9\"><v>44197</v></c>",
            "<c r=\"F1\" s=\"1\"><v>60</v></c>",
            "<c r=\"G1\" t=\"d\"><v>2021-01-01T12:30:00</v></c>",
            "<c r=\"H1\" t=\"d\"><v>2021-01-01</v></c>",
            "<c r=\"I1\" t=\"d\"><v>not a date</v></c>",
            "</row>"
        );
        let bytes = Wb::new()
            .sheet("A", rows)
            .styles(
                &[0, 14, 164, 165],
                &[(164, "yyyy-mm-dd hh:mm"), (165, "0.00")],
            )
            .build();
        let (got, _) = read(bytes).await.unwrap();
        let cells = &got[0].1;
        let at = |i: usize| cells.iter().find(|(c, _)| *c == i).map(|(_, v)| v.clone());
        let day = |t: Temporal| Some(Cell::Temporal(t));
        assert_eq!(at(0), day(Temporal::Date(18_628)));
        assert_eq!(at(1), day(Temporal::Timestamp(1_609_502_400_000_000)));
        assert_eq!(at(2), Some(Cell::Number(44_197.0)));
        assert_eq!(at(3), Some(Cell::Number(44_197.0)));
        assert_eq!(
            at(4),
            Some(Cell::Number(44_197.0)),
            "a style that does not exist"
        );
        assert_eq!(
            at(5),
            Some(Cell::Number(60.0)),
            "the day that never existed"
        );
        assert_eq!(at(6), day(Temporal::Timestamp(1_609_504_200_000_000)));
        assert_eq!(at(7), day(Temporal::Date(18_628)));
        assert_eq!(at(8), Some(text("not a date")));
    }

    #[tokio::test]
    async fn the_1904_system_moves_every_date() {
        let rows = "<row r=\"1\"><c r=\"A1\" s=\"1\"><v>42735</v></c></row>";
        let bytes = Wb::new()
            .sheet("A", rows)
            .styles(&[0, 14], &[])
            .date1904()
            .build();
        let (got, _) = read(bytes).await.unwrap();
        assert_eq!(got[0].1, vec![(0, Cell::Temporal(Temporal::Date(18_628)))]);
    }

    fn limited(f: impl FnOnce(&mut SheetLimits)) -> SheetLimits {
        let mut l = SheetLimits::default();
        f(&mut l);
        l
    }

    async fn read_limited(rows: &str, sheet: SheetLimits) -> Result<(Rows, SheetStats), XlsxError> {
        read_with(
            Wb::new().sheet("A", rows).build(),
            &XlsxLimits::default(),
            &sheet,
        )
        .await
    }

    #[tokio::test]
    async fn rows_columns_cells_and_text_are_capped_and_the_cap_is_inclusive() {
        let grid = "<row r=\"1\"><c r=\"A1\"><v>1</v></c><c r=\"B1\"><v>2</v></c></row><row r=\"2\"><c r=\"A2\"><v>3</v></c></row>";
        let ok = |l| read_limited(grid, l);
        assert!(ok(limited(|l| l.max_rows = 2)).await.is_ok());
        let r = ok(limited(|l| l.max_rows = 1)).await;
        assert_eq!(r.unwrap_err(), XlsxError::TooLarge(Cap::Rows));
        assert!(ok(limited(|l| l.max_columns = 2)).await.is_ok());
        let r = ok(limited(|l| l.max_columns = 1)).await;
        assert_eq!(r.unwrap_err(), XlsxError::TooLarge(Cap::Columns));
        assert!(ok(limited(|l| l.max_cells = 3)).await.is_ok());
        let r = ok(limited(|l| l.max_cells = 2)).await;
        assert_eq!(r.unwrap_err(), XlsxError::TooLarge(Cap::Cells));
        // Empty cells count: they cost the parser the same.
        let empties = "<row r=\"1\"><c r=\"A1\"/><c r=\"B1\"/><c r=\"C1\"/></row>";
        let r = read_limited(empties, limited(|l| l.max_cells = 2)).await;
        assert_eq!(r.unwrap_err(), XlsxError::TooLarge(Cap::Cells));
        // The defaults are Excel's own limits.
        let excel = SheetLimits::default();
        assert_eq!((excel.max_rows, excel.max_columns), (1_048_576, 16_384));
        let beyond = "<row r=\"1048577\"/>";
        let r = read_limited(beyond, SheetLimits::default()).await;
        assert_eq!(r.unwrap_err(), XlsxError::TooLarge(Cap::Rows));
        let beyond = "<row r=\"1\"><c r=\"XFE1\"><v>1</v></c></row>";
        let r = read_limited(beyond, SheetLimits::default()).await;
        assert_eq!(r.unwrap_err(), XlsxError::TooLarge(Cap::Columns));
    }

    #[tokio::test]
    async fn a_cell_or_a_row_with_too_much_text_is_refused() {
        let row = |n: usize| {
            let cells: String = (0..n)
                .map(|i| {
                    format!(
                        "<c r=\"{}1\" t=\"inlineStr\"><is><t>abcdefghij</t></is></c>",
                        (b'A' + i as u8) as char
                    )
                })
                .collect();
            format!("<row r=\"1\">{cells}</row>")
        };
        let r = read_limited(&row(1), limited(|l| l.max_cell_bytes = 9)).await;
        assert_eq!(r.unwrap_err(), XlsxError::Invalid(Invalid::CellTooLong));
        assert!(read_limited(&row(1), limited(|l| l.max_cell_bytes = 10))
            .await
            .is_ok());
        let r = read_limited(&row(3), limited(|l| l.max_row_bytes = 29)).await;
        assert_eq!(r.unwrap_err(), XlsxError::Invalid(Invalid::RowTooLong));
        assert!(read_limited(&row(3), limited(|l| l.max_row_bytes = 30))
            .await
            .is_ok());
        assert_eq!((MAX_CELL_BYTES, MAX_ROW_BYTES), (131_072, 1_048_576));
    }

    #[tokio::test]
    async fn one_giant_text_node_is_stopped_by_the_xml_guard_before_it_is_buffered() {
        let big = "x".repeat(50_000);
        let rows =
            format!("<row r=\"1\"><c r=\"A1\" t=\"inlineStr\"><is><t>{big}</t></is></c></row>");
        let limits = XlsxLimits {
            max_token_bytes: 4096,
            ..XlsxLimits::default()
        };
        let r = read_with(
            Wb::new().sheet("A", &rows).build(),
            &limits,
            &SheetLimits::default(),
        )
        .await;
        assert_eq!(r.unwrap_err(), XlsxError::Invalid(Invalid::TokenTooLong));
    }

    #[tokio::test]
    async fn a_workbook_written_by_a_real_library_reads_end_to_end() {
        use rust_xlsxwriter::{ExcelDateTime, Format, Formula, Workbook};
        let mut book = Workbook::new();
        let sheet = book.add_worksheet();
        sheet.set_name("Data").unwrap();
        sheet.write_number(0, 0, 1.5).unwrap();
        sheet.write_string(0, 1, "héllo & <co>").unwrap();
        sheet.write_boolean(0, 2, true).unwrap();
        let date = ExcelDateTime::from_ymd(2021, 1, 1).unwrap();
        let format = Format::new().set_num_format("yyyy-mm-dd");
        sheet
            .write_datetime_with_format(0, 3, &date, &format)
            .unwrap();
        sheet
            .write_formula(0, 4, Formula::new("=1+2").set_result("3"))
            .unwrap();
        sheet
            .merge_range(1, 0, 2, 1, "merged", &Format::new())
            .unwrap();
        let (got, _) = read(book.save_to_buffer().unwrap()).await.unwrap();
        assert_eq!(
            got[0].1,
            vec![
                (0, Cell::Number(1.5)),
                (1, text("héllo & <co>")),
                (2, Cell::Bool(true)),
                (3, Cell::Temporal(Temporal::Date(18_628))),
                (4, Cell::Number(3.0)),
            ]
        );
        // The merged value sits in its top-left cell; nothing is filled in.
        assert_eq!(got[1], (2, vec![(0, text("merged"))]));
        assert_eq!(got.len(), 2);
    }
}
