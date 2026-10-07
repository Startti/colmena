//! One sheet of a workbook, read and written: the book that is opened once, the
//! short first read that decides a sheet's columns, and one full read that
//! streams its rows, typed, into parts (see `xlsx_convert` for the whole
//! workbook).
//!
//! Reading and typing run on a blocking thread and the writer on the async side,
//! joined by a channel of two batches, as for a CSV. The blocking half stops at
//! the next row when its token is cancelled or the receiving side is gone, and a
//! run that fails cancels it and waits for it, so no thread outlives the run.
//! Memory is bounded by constants, never by the sheet: a row (at most 1 MiB of
//! text over 16,384 cells), a batch (8,192 rows, 1,000,000 cells or 8 MiB of
//! text), two batches in the channel, one part in the writer (64 MiB), the
//! shared-strings table (at most 168 MiB) and the XML parser's buffer for one
//! event (1 MiB). The workbook itself is on disk.

use crate::tabular_prepare::convert::{ConvertError, TableError, TypeConflict, CHANNEL_BATCHES};
use crate::tabular_prepare::csv::column_names;
use crate::tabular_prepare::infer::INFERENCE_ROWS;
use crate::tabular_prepare::manifest::ColumnType;
use crate::tabular_prepare::part_sink::PartSink;
use crate::tabular_prepare::writer::{PartWriter, TableWritten, WriterConfig};
use crate::tabular_prepare::xlsx_columns::{cell_text, Batcher, Seen};
use crate::tabular_prepare::xlsx_package::{Package, XlsxLimits};
use crate::tabular_prepare::xlsx_sheet::{read_sheet_with, SheetContext, SheetLimits, SheetStats};
use crate::tabular_prepare::xlsx_spool::{Invalid, Spooled, XlsxError};
use crate::tabular_prepare::xlsx_strings::{read_shared_strings, SharedStrings};
use crate::tabular_prepare::xlsx_styles::{read_styles, Styles};
use crate::tabular_prepare::xlsx_workbook::{read_workbook, SheetRef};
use arrow_array::RecordBatch;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Everything a read of the workbook needs, opened once.
pub struct Book {
    pub pkg: Package,
    pub sheets: Vec<SheetRef>,
    pub strings: SharedStrings,
    pub styles: Styles,
    pub date1904: bool,
    /// Cells read by every read of this book, for the job's cell limit.
    pub cells: AtomicU64,
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
        cells: AtomicU64::new(0),
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
    // Never indexes past what it allocated, whatever order the cells come in.
    let width = row.iter().map(|(c, _)| c + 1).max().unwrap_or(0);
    let mut raw = vec![String::new(); width];
    for (column, cell) in row {
        if let Some(slot) = raw.get_mut(*column) {
            *slot = cell_text(cell).into_owned();
        }
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
        cells,
        ..
    } = book;
    let ctx = SheetContext {
        strings,
        styles,
        date1904: *date1904,
        cells,
        cancel,
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

/// What the blocking half hands to the writer.
enum Item {
    Batch(RecordBatch),
    Done(SheetStats),
    Conflict(TypeConflict),
    Failed(XlsxError),
    /// A batch that could not be built.
    Broken,
}

/// The blocking half of a run: read every row of the sheet, type it and send the
/// batches. It stops as soon as the receiving side is gone or cancelled.
fn produce(
    book: &Mutex<Book>,
    part: &str,
    names: &[String],
    types: &[ColumnType],
    limits: &SheetLimits,
    cancel: &CancellationToken,
    tx: &mpsc::Sender<Item>,
) {
    let mut book = book.lock().unwrap_or_else(|p| p.into_inner());
    let Book {
        pkg,
        strings,
        styles,
        date1904,
        cells,
        ..
    } = &mut *book;
    let ctx = SheetContext {
        strings,
        styles,
        date1904: *date1904,
        cells,
        cancel,
    };
    let mut batcher = Batcher::new(names, types);
    let mut header_seen = false;
    let mut data_rows = 0u64;
    let mut stopped = false;
    let mut broken = false;
    let mut conflict = None;
    let read = read_sheet_with(pkg, part, &ctx, limits, &mut |_, row| {
        if cancel.is_cancelled() {
            return Err(XlsxError::Cancelled);
        }
        if !header_seen {
            header_seen = true;
            return Ok(true);
        }
        if row.last().is_some_and(|(c, _)| *c >= names.len()) {
            return Err(XlsxError::Invalid(Invalid::BeyondHeader));
        }
        if batcher.is_full() {
            match batcher.take() {
                Ok(Some(batch)) => {
                    if tx.blocking_send(Item::Batch(batch)).is_err() {
                        stopped = true;
                        return Ok(false);
                    }
                }
                Ok(None) => {}
                Err(_) => {
                    broken = true;
                    return Ok(false);
                }
            }
        }
        if let Err(column) = batcher.push(row) {
            conflict = Some(TypeConflict {
                column,
                row: data_rows,
            });
            return Ok(false);
        }
        data_rows += 1;
        Ok(true)
    });
    let item = match (read, conflict) {
        _ if stopped => return,
        (Err(e), _) => Item::Failed(e),
        (Ok(_), _) if broken => Item::Broken,
        (Ok(_), Some(c)) => Item::Conflict(c),
        (Ok(stats), None) => {
            match batcher.take() {
                Ok(Some(batch)) => {
                    if tx.blocking_send(Item::Batch(batch)).is_err() {
                        return;
                    }
                }
                Ok(None) => {}
                Err(_) => {
                    let _ = tx.blocking_send(Item::Broken);
                    return;
                }
            }
            Item::Done(stats)
        }
    };
    let _ = tx.blocking_send(item);
}

/// What a single run ends with, other than success.
pub enum RunEnd {
    Conflict(TypeConflict),
    Failed(TableError),
}

impl From<crate::tabular_prepare::writer::WriterError> for RunEnd {
    fn from(e: crate::tabular_prepare::writer::WriterError) -> Self {
        RunEnd::Failed(e.into())
    }
}

/// What one run of a sheet is asked to do.
pub struct Plan<'a> {
    /// The part holding the sheet's cells.
    pub part: &'a str,
    pub table_idx: usize,
    pub names: &'a [String],
    /// The effective types: the sample's, with any demoted column as text.
    pub types: &'a [ColumnType],
    pub limits: SheetLimits,
    pub cfg: WriterConfig,
}

/// One read of a sheet, written out as parts through `sink`.
pub async fn run_sheet(
    book: &Arc<Mutex<Book>>,
    plan: &Plan<'_>,
    sink: &Arc<dyn PartSink>,
    cancel: &CancellationToken,
) -> Result<(TableWritten, SheetStats), RunEnd> {
    // A token of this run alone: stopping this run's reader must not stop the
    // restarts that may follow.
    let cancel = cancel.child_token();
    let (tx, mut rx) = mpsc::channel(CHANNEL_BATCHES);
    let producer = {
        let book = book.clone();
        let (part, names, types) = (
            plan.part.to_string(),
            plan.names.to_vec(),
            plan.types.to_vec(),
        );
        let (limits, cancel) = (plan.limits, cancel.clone());
        tokio::task::spawn_blocking(move || {
            produce(&book, &part, &names, &types, &limits, &cancel, &tx);
        })
    };
    let schema = Batcher::new(plan.names, plan.types).schema();
    let outcome = async {
        let mut writer = PartWriter::new(sink.clone(), plan.table_idx, schema, plan.cfg)?;
        loop {
            match rx.recv().await {
                Some(Item::Batch(batch)) => writer.write(&batch).await?,
                Some(Item::Done(stats)) => return Ok((writer.finish().await?, stats)),
                Some(Item::Conflict(c)) => return Err(RunEnd::Conflict(c)),
                Some(Item::Failed(e)) => return Err(RunEnd::Failed(e.into())),
                Some(Item::Broken) => {
                    let e = ConvertError::Cast("a batch could not be built".into());
                    return Err(RunEnd::Failed(e.into()));
                }
                None => {
                    let e = ConvertError::Cast("the reader stopped unexpectedly".into());
                    return Err(RunEnd::Failed(e.into()));
                }
            }
        }
    }
    .await;
    // On any outcome but success the reader may still be going: cancel it, close
    // the channel and wait for it, so no blocking thread outlives the run.
    if outcome.is_err() {
        cancel.cancel();
    }
    drop(rx);
    match (outcome, producer.await) {
        (Err(RunEnd::Failed(_)), Err(e)) if e.is_panic() => {
            Err(RunEnd::Failed(ConvertError::ReaderPanicked.into()))
        }
        (outcome, _) => outcome,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tabular_prepare::manifest::part_path;
    use crate::tabular_prepare::part_sink::fake::MemorySink;
    use crate::tabular_prepare::xlsx_spool::{spool_stream, MAX_XLSX_BYTES};
    use crate::tabular_prepare::xlsxfix::Wb;
    use arrow_array::Array;
    use bytes::Bytes;
    use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
    use std::sync::atomic::AtomicU64;

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
    fn rows_of(sink: &MemorySink, parts: u32) -> Vec<Vec<String>> {
        let mut out = Vec::new();
        for p in 0..parts as usize {
            let bytes = sink.get(&part_path(0, p).unwrap()).unwrap();
            for batch in ParquetRecordBatchReaderBuilder::try_new(bytes)
                .unwrap()
                .build()
                .unwrap()
            {
                let batch = batch.unwrap();
                for r in 0..batch.num_rows() {
                    out.push(
                        (0..batch.num_columns())
                            .map(|c| match batch.column(c) {
                                col if col.is_null(r) => "null".to_string(),
                                col => arrow_cast::display::array_value_to_string(col, r).unwrap(),
                            })
                            .collect(),
                    );
                }
            }
        }
        out
    }

    async fn run(
        book: &Arc<Mutex<Book>>,
        types: &[ColumnType],
        names: &[String],
        cfg: WriterConfig,
    ) -> (Result<(TableWritten, SheetStats), RunEnd>, Arc<MemorySink>) {
        let sink = Arc::new(MemorySink::default());
        let part = part_of(book, 0);
        let plan = Plan {
            part: &part,
            table_idx: 0,
            names,
            types,
            limits: SheetLimits::default(),
            cfg,
        };
        let dyn_sink: Arc<dyn PartSink> = sink.clone();
        let result = run_sheet(book, &plan, &dyn_sink, &CancellationToken::new()).await;
        (result, sink)
    }

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

    #[tokio::test]
    async fn a_run_writes_every_row_typed_with_blank_rows_dropped_and_missing_cells_null() {
        let rows = [
            row(1, &[text("A", 1, "n"), text("B", 1, "s")]),
            row(2, &[num("A", 2, 1), text("B", 2, "a")]),
            row(4, &[text("B", 4, "only b")]),
            row(5, &[num("A", 5, 3)]),
        ]
        .concat();
        let book = book_of(Wb::new().sheet("A", &rows).build()).await;
        let sample = sample(&book, 0).unwrap().unwrap();
        let (result, sink) =
            run(&book, &sample.types, &sample.names, WriterConfig::default()).await;
        let (written, stats) = result.ok().unwrap();
        assert_eq!((written.rows, written.parts, stats.blank_rows), (3, 1, 1));
        assert_eq!(
            rows_of(&sink, 1),
            [["1", "a"], ["null", "only b"], ["3", "null"]]
        );
    }

    #[tokio::test]
    async fn a_late_conflict_comes_back_as_a_conflict_naming_column_and_row() {
        let mut rows = vec![row(1, &[text("A", 1, "n")])];
        for r in 2..=20 {
            let cell = if r == 15 {
                text("A", r, "N/A")
            } else {
                num("A", r, r)
            };
            rows.push(row(r, &[cell]));
        }
        let book = book_of(Wb::new().sheet("A", &rows.concat()).build()).await;
        let names = vec!["n".to_string()];
        let (result, _) = run(&book, &[ColumnType::Int], &names, WriterConfig::default()).await;
        match result {
            Err(RunEnd::Conflict(c)) => assert_eq!((c.column, c.row), (0, 13)),
            _ => panic!("expected a conflict"),
        }
        // As text the same sheet converts, with every value as it was.
        let (result, sink) = run(
            &book,
            &[ColumnType::String],
            &names,
            WriterConfig::default(),
        )
        .await;
        let (written, _) = result.ok().unwrap();
        let got = rows_of(&sink, written.parts);
        assert_eq!(
            (got.len(), got[0][0].as_str(), got[13][0].as_str()),
            (19, "2", "N/A")
        );
    }

    #[tokio::test]
    async fn parts_roll_at_the_row_limit() {
        let mut rows = vec![row(1, &[text("A", 1, "n")])];
        rows.extend((2..=2501).map(|r| row(r, &[num("A", r, r)])));
        let book = book_of(Wb::new().sheet("A", &rows.concat()).build()).await;
        let cfg = WriterConfig {
            max_rows: 1000,
            ..WriterConfig::default()
        };
        let names = vec!["n".to_string()];
        let (result, sink) = run(&book, &[ColumnType::Int], &names, cfg).await;
        let (written, _) = result.ok().unwrap();
        assert_eq!((written.rows, written.parts), (2500, 3));
        assert_eq!(rows_of(&sink, 3).len(), 2500);
    }

    #[tokio::test]
    async fn a_value_beyond_the_header_is_refused_without_echoing_it() {
        let rows = [
            row(1, &[text("A", 1, "a")]),
            row(2, &[num("A", 2, 1), text("B", 2, "secret cell text")]),
        ]
        .concat();
        let book = book_of(Wb::new().sheet("A", &rows).build()).await;
        let err = sample(&book, 0).err().unwrap();
        assert_eq!(err, XlsxError::Invalid(Invalid::BeyondHeader));
        assert!(!err.to_string().contains("secret"));
        let names = vec!["a".to_string()];
        let (result, sink) = run(&book, &[ColumnType::Int], &names, WriterConfig::default()).await;
        assert!(matches!(
            result,
            Err(RunEnd::Failed(TableError::Xlsx(XlsxError::Invalid(
                Invalid::BeyondHeader
            ))))
        ));
        assert!(
            sink.paths().is_empty(),
            "nothing is written for a refused sheet"
        );
    }

    #[tokio::test]
    async fn a_cancelled_token_stops_the_read_at_its_first_row() {
        let book = book_of(Wb::new().sheet("A", &row(1, &[text("A", 1, "a")])).build()).await;
        let cancel = CancellationToken::new();
        cancel.cancel();
        let part = part_of(&book, 0);
        let sampled = sample_sheet(
            &mut book.lock().unwrap(),
            &part,
            &SheetLimits::default(),
            &cancel,
        );
        assert_eq!(sampled.err(), Some(XlsxError::Cancelled));
        let sink: Arc<dyn PartSink> = Arc::new(MemorySink::default());
        let names = vec!["a".to_string()];
        let plan = Plan {
            part: &part,
            table_idx: 0,
            names: &names,
            types: &[ColumnType::String],
            limits: SheetLimits::default(),
            cfg: WriterConfig::default(),
        };
        let result = run_sheet(&book, &plan, &sink, &cancel).await;
        assert!(matches!(
            result,
            Err(RunEnd::Failed(TableError::Xlsx(XlsxError::Cancelled)))
        ));
    }

    #[tokio::test]
    async fn a_row_opened_inside_a_row_is_refused_by_the_sample_without_a_panic() {
        // Cells in column F, then (inside the same row) a row with a cell in column A:
        // the header used to be built by indexing with the first of those columns.
        let rows = "<row r=\"1\"><c r=\"F1\"><v>1</v></c><row r=\"2\"><c r=\"A2\"><v>1</v></c></row></row>";
        let book = book_of(Wb::new().sheet("A", rows).build()).await;
        let err = sample(&book, 0).err().unwrap();
        assert_eq!(err, XlsxError::Invalid(Invalid::BadCell));
        // And the header builder itself never indexes past its width.
        use crate::tabular_prepare::xlsx_sheet::Cell;
        let out_of_order = vec![(5, Cell::Number(1.0)), (0, Cell::Number(2.0))];
        assert_eq!(header_of(&out_of_order).len(), 6);
    }
}
