//! A workbook opened once and the short first read of one of its sheets (see
//! `xlsx_convert` for the whole conversion).
//!
//! `open_book` runs the archive pre-check, lists the sheets and loads the shared
//! strings and styles; `Book` keeps them for the reads that follow. `sample_sheet`
//! reads the head of a sheet: the first row that holds a value is the header and
//! the first 10,000 data rows decide the column types from the kinds of their
//! cells. Memory is bounded by constants, never by the sheet: the row being read
//! (at most 1 MiB of text over 16,384 cells), the shared-strings table (at most
//! 168 MiB) and the XML parser's buffer for one event (1 MiB). The workbook
//! itself is on disk.

use crate::tabular_prepare::csv::column_names;
use crate::tabular_prepare::infer::INFERENCE_ROWS;
use crate::tabular_prepare::manifest::ColumnType;
use crate::tabular_prepare::xlsx_columns::{cell_text, Seen};
use crate::tabular_prepare::xlsx_package::{Package, XlsxLimits};
use crate::tabular_prepare::xlsx_sheet::{read_sheet_with, SheetContext, SheetLimits};
use crate::tabular_prepare::xlsx_spool::{Invalid, Spooled, XlsxError};
use crate::tabular_prepare::xlsx_strings::{read_shared_strings, SharedStrings};
use crate::tabular_prepare::xlsx_styles::{read_styles, Styles};
use crate::tabular_prepare::xlsx_workbook::{read_workbook, SheetRef};
use tokio_util::sync::CancellationToken;

/// Everything a read of the workbook needs, opened once.
pub struct Book {
    pub pkg: Package,
    pub sheets: Vec<SheetRef>,
    pub strings: SharedStrings,
    pub styles: Styles,
    pub date1904: bool,
}

pub fn open_book(spooled: Spooled, limits: &XlsxLimits) -> Result<Book, XlsxError> {
    let mut pkg = Package::open_with(spooled, limits)?;
    let workbook = read_workbook(&mut pkg)?;
    let strings = match &workbook.shared_strings {
        Some(part) => read_shared_strings(&mut pkg, part)?,
        None => SharedStrings::none(),
    };
    let styles = match &workbook.styles {
        Some(part) => read_styles(&mut pkg, part)?,
        None => Styles::none(),
    };
    Ok(Book {
        pkg,
        sheets: workbook.sheets,
        strings,
        styles,
        date1904: workbook.date1904,
    })
}

/// What the first read of a sheet decided.
pub struct Sample {
    /// Column names, cleaned and unique.
    pub names: Vec<String>,
    /// The type each column's cells agree on.
    pub types: Vec<ColumnType>,
    /// Data rows the types were decided from.
    pub rows: u64,
}

/// The header of a sheet as text, by column.
fn header_of(row: &[(usize, crate::tabular_prepare::xlsx_sheet::Cell)]) -> Vec<String> {
    let width = row.last().map_or(0, |(c, _)| c + 1);
    let mut raw = vec![String::new(); width];
    for (column, cell) in row {
        raw[*column] = cell_text(cell).into_owned();
    }
    raw
}

/// Reads the head of a sheet: the header and the first rows, to decide the column
/// types. `None` for a sheet with no value.
pub fn sample_sheet(
    book: &mut Book,
    part: &str,
    limits: &SheetLimits,
    cancel: &CancellationToken,
) -> Result<Option<Sample>, XlsxError> {
    let Book {
        pkg,
        strings,
        styles,
        date1904,
        ..
    } = book;
    let ctx = SheetContext {
        strings,
        styles,
        date1904: *date1904,
    };
    let mut header: Option<Vec<String>> = None;
    let mut seen = Seen::new(0);
    let mut rows = 0u64;
    read_sheet_with(pkg, part, &ctx, limits, &mut |_, row| {
        if cancel.is_cancelled() {
            return Err(XlsxError::Cancelled);
        }
        let Some(names) = &header else {
            let raw = header_of(row);
            seen = Seen::new(raw.len());
            header = Some(raw);
            return Ok(true);
        };
        if row.last().is_some_and(|(c, _)| *c >= names.len()) {
            return Err(XlsxError::Invalid(Invalid::BeyondHeader));
        }
        seen.observe(row);
        rows += 1;
        Ok(rows < INFERENCE_ROWS as u64)
    })?;
    Ok(header.map(|raw| Sample {
        names: column_names(&raw),
        types: seen.types(),
        rows,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tabular_prepare::xlsx_spool::{spool_stream, MAX_XLSX_BYTES};
    use crate::tabular_prepare::xlsxfix::Wb;
    use bytes::Bytes;
    use std::sync::atomic::AtomicU64;
    use std::sync::{Arc, Mutex};

    async fn book_of(bytes: Vec<u8>) -> Arc<Mutex<Book>> {
        let dir = tempfile::tempdir().unwrap();
        let stream = Box::pin(futures::stream::iter(vec![Ok(Bytes::from(bytes))]));
        let read = AtomicU64::new(0);
        let cancel = CancellationToken::new();
        let spooled = spool_stream(dir.path(), stream, None, MAX_XLSX_BYTES, &cancel, &read)
            .await
            .unwrap();
        Arc::new(Mutex::new(
            open_book(spooled, &XlsxLimits::default()).unwrap(),
        ))
    }

    fn part_of(book: &Arc<Mutex<Book>>, sheet: usize) -> String {
        book.lock().unwrap().sheets[sheet].part.clone()
    }

    fn sample(book: &Arc<Mutex<Book>>, sheet: usize) -> Result<Option<Sample>, XlsxError> {
        let part = part_of(book, sheet);
        let mut book = book.lock().unwrap();
        sample_sheet(
            &mut book,
            &part,
            &SheetLimits::default(),
            &CancellationToken::new(),
        )
    }

    fn text(col: &str, row: usize, t: &str) -> String {
        format!("<c r=\"{col}{row}\" t=\"inlineStr\"><is><t>{t}</t></is></c>")
    }

    fn num(col: &str, row: usize, n: impl std::fmt::Display) -> String {
        format!("<c r=\"{col}{row}\"><v>{n}</v></c>")
    }

    fn row(r: usize, cells: &[String]) -> String {
        format!("<row r=\"{r}\">{}</row>", cells.concat())
    }

    /// Every row of table 0, each value as text (`null` for a null).

    #[tokio::test]
    async fn the_header_names_and_types_come_from_the_first_row_and_the_first_cells() {
        let rows = [
            // Row 1 is blank, so the header is row 2: an empty name, a repeat, a number.
            row(
                2,
                &[text("A", 2, "id"), text("C", 2, "id"), num("D", 2, 2021)],
            ),
            row(3, &[num("A", 3, 1), text("C", 3, "x"), num("D", 3, 1.5)]),
            row(4, &[num("A", 4, 2), text("C", 4, "y"), num("D", 4, 2)]),
        ]
        .concat();
        let book = book_of(Wb::new().sheet("A", &rows).sheet("Blank", "").build()).await;
        let sample = sample(&book, 0).unwrap().unwrap();
        assert_eq!(sample.names, ["id", "column2", "id_2", "2021"]);
        use ColumnType::*;
        assert_eq!(sample.types, [Int, String, String, Float]);
        assert_eq!(sample.rows, 2);
        assert!(
            self::sample(&book, 1).unwrap().is_none(),
            "no value, no table"
        );
        let header_only = Wb::new()
            .sheet("H", &row(1, &[text("A", 1, "only")]))
            .build();
        let sample = self::sample(&book_of(header_only).await, 0)
            .unwrap()
            .unwrap();
        assert_eq!(
            (sample.names, sample.types, sample.rows),
            (vec!["only".to_string()], vec![String], 0)
        );
    }

    #[tokio::test]
    async fn only_the_first_ten_thousand_rows_decide_the_types() {
        let mut rows = vec![row(1, &[text("A", 1, "n")])];
        for r in 2..=10_050 {
            let cell = if r == 10_040 {
                text("A", r, "late")
            } else {
                num("A", r, r)
            };
            rows.push(row(r, &[cell]));
        }
        let book = book_of(Wb::new().sheet("A", &rows.concat()).build()).await;
        let sample = sample(&book, 0).unwrap().unwrap();
        assert_eq!(sample.rows, INFERENCE_ROWS as u64);
        assert_eq!(
            sample.types,
            [ColumnType::Int],
            "the late text was never seen"
        );
    }
}
