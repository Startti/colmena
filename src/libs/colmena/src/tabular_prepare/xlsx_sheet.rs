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
/// them, nothing outside the crate can.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SheetLimits {
    pub max_rows: u32,
    pub max_columns: usize,
    pub max_cells: u64,
    pub max_cell_bytes: usize,
    pub max_row_bytes: usize,
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
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Number,
    Shared,
    Str,
    Inline,
    Bool,
    Error,
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

struct Pending {
    column: usize,
    kind: Kind,
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
        Kind::Number => {
            if p.value.trim().is_empty() {
                return Ok(None);
            }
            match p.value.trim().parse::<f64>() {
                Ok(n) => Some(Cell::Number(n)),
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
            _ => Kind::Number,
        };
        Ok(Pending {
            column,
            kind,
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
        let ctx = SheetContext { strings: &strings };
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
}
