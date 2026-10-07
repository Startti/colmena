//! Converting an xlsx into Parquet parts: one table per sheet that holds a value,
//! through the same writer, part sink and tracking as a CSV (dark behind
//! `COLMENA_LARGE_TABULAR`).
//!
//! The workbook is spooled to a local file, checked and opened once (see
//! `xlsx_package`), and then each sheet is converted in turn through
//! `xlsx_run`: a short first read decides its column types and a second streams
//! every row, typed, into parts.
//!
//! Rules.
//! - The first row that holds a value is the header; a sheet with no value is not
//!   a table; a sheet with only a header is a table of no rows; a workbook with no
//!   table is refused (`NoData`). Header names are cleaned like a CSV's.
//! - A value past the last column of the header is refused (`BeyondHeader`), as a
//!   CSV row longer than its header is.
//! - A late cell that contradicts its column's type makes that column text and the
//!   sheet is read again, at most three times; the third makes every column text,
//!   which cannot conflict. Only the sheet that conflicted is read again.
//! - The cell cap is the job's: every cell any read counts, restarts included.

use crate::tabular_prepare::convert::{
    ConvertControl, ConvertError, ConvertedTable, TableError, TableFailure, TrackingSink,
    MAX_RESTARTS,
};
use crate::tabular_prepare::csv::Encoding;
use crate::tabular_prepare::driver::report_of;
use crate::tabular_prepare::manifest::{
    min_tables_json_len, part_path, unique_table_names, ColumnType, Manifest, ManifestError,
    SkippedSheet, TableInfo, MANIFEST_MAX_BYTES, SKIPPED_HEADER_ROW, TABLES_JSON_MAX_BYTES,
};
use crate::tabular_prepare::part_sink::PartSink;
use crate::tabular_prepare::writer::WriterConfig;
use crate::tabular_prepare::xlsx_package::XlsxLimits;
use crate::tabular_prepare::xlsx_run::{open_book, run_sheet, sample_sheet, Plan, RunEnd};
use crate::tabular_prepare::xlsx_sheet::SheetLimits;
use crate::tabular_prepare::xlsx_spool::{Cap, Invalid, Spooled, XlsxError};
use async_trait::async_trait;
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

/// A workbook that can be fetched once: the host's storage gives it as a stream
/// and this puts it in a local file.
#[async_trait]
pub trait XlsxSource: Send + Sync {
    /// The workbook in a local file. `cancel` is cancelled when the conversion is
    /// dropped or cancelled.
    async fn spool(&self, cancel: &CancellationToken) -> Result<Spooled, XlsxError>;
}

/// What a workbook became: a table per sheet that has a value, and the sheets that
/// are not tables for want of a header. Derefs to the tables.
#[derive(Debug)]
pub struct Converted {
    pub tables: Vec<SheetTable>,
    /// Sheets skipped (see [`SKIPPED_HEADER_ROW`]); their names cleaned.
    pub skipped: Vec<SkippedSheet>,
}

impl std::ops::Deref for Converted {
    type Target = [SheetTable];

    fn deref(&self) -> &[SheetTable] {
        &self.tables
    }
}

/// A table that was written, with the name of its sheet.
#[derive(Debug)]
pub struct SheetTable {
    /// The sheet's own name; the manifest's table name is a clean, unique form of it.
    pub sheet: String,
    pub table: ConvertedTable,
}

/// Limits of a conversion: the workbook's and the sheet's. The defaults are the
/// constants of those modules; tests lower them.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Limits {
    pub xlsx: XlsxLimits,
    pub sheet: SheetLimits,
}

/// Whether what the manifest would hold so far fits: the table list in the registry
/// row (64 KiB), then the whole manifest file (128 KiB), which also carries a conversion
/// report per table and the skipped sheets. Names are the unique ones the manifest will
/// use. Checked after every sheet, so 256 sheets fail at the sheet that overflows.
fn manifest_fits(tables: &[SheetTable], skipped: &[SkippedSheet]) -> Result<(), Cap> {
    let raw: Vec<&str> = tables.iter().map(|t| t.sheet.as_str()).collect();
    let names = unique_table_names(&raw);
    let infos: Vec<TableInfo> = tables
        .iter()
        .zip(&names)
        .map(|(t, name)| TableInfo {
            name: name.clone(),
            rows: t.table.written.rows,
            parts: t.table.written.parts,
            columns: t.table.written.columns.clone(),
        })
        .collect();
    let reports = tables
        .iter()
        .zip(&names)
        .map(|(t, name)| report_of(name, &t.table))
        .collect();
    let manifest = Manifest::new(infos)
        .with_conversion(reports)
        .with_skipped(skipped.to_vec());
    if manifest.tables_json().is_err() {
        return Err(Cap::TableList);
    }
    match serde_json::to_string(&manifest) {
        Ok(json) if json.len() <= MANIFEST_MAX_BYTES => Ok(()),
        _ => Err(Cap::Manifest),
    }
}

/// Converts the workbook into the parts of one table per sheet that has a value,
/// in the order of the workbook.
pub async fn convert_xlsx(
    source: &dyn XlsxSource,
    sink: Arc<dyn PartSink>,
    cfg: WriterConfig,
    control: &Arc<ConvertControl>,
) -> Result<Converted, TableFailure> {
    convert_xlsx_limits(source, sink, cfg, control, &Limits::default()).await
}

pub(crate) async fn convert_xlsx_limits(
    source: &dyn XlsxSource,
    sink: Arc<dyn PartSink>,
    cfg: WriterConfig,
    control: &Arc<ConvertControl>,
    limits: &Limits,
) -> Result<Converted, TableFailure> {
    // A child of the caller's token, cancelled when this future is dropped.
    let cancel = control.token().child_token();
    let _stop_on_drop = cancel.clone().drop_guard();
    let sink: Arc<dyn PartSink> = Arc::new(TrackingSink {
        inner: sink,
        control: control.clone(),
    });
    let fail = |e: TableError| TableFailure {
        error: e,
        blob_paths: control.paths(),
    };
    let panicked = |_| fail(ConvertError::ReaderPanicked.into());
    let spooled = source.spool(&cancel).await.map_err(|e| fail(e.into()))?;
    let xlsx = limits.xlsx;
    let mut book = tokio::task::spawn_blocking(move || open_book(spooled, &xlsx))
        .await
        .map_err(panicked)?
        .map_err(|e| fail(e.into()))?;
    let sheets = book.sheets.clone();
    book.pkg.set_cancel(cancel.clone());
    let book = Arc::new(Mutex::new(book));

    let mut tables: Vec<SheetTable> = Vec::new();
    let mut skipped: Vec<SkippedSheet> = Vec::new();
    for sheet in &sheets {
        // The cell limit is the job's: the book counts every cell any read of it
        // reads, so sampling, runs, restarts and skipped sheets all count.
        let sheet_limits = limits.sheet;
        let sample = {
            let (book, part, cancel) = (book.clone(), sheet.part.clone(), cancel.clone());
            tokio::task::spawn_blocking(move || {
                let mut book = book.lock().unwrap_or_else(|p| p.into_inner());
                sample_sheet(&mut book, &part, &sheet_limits, &cancel)
            })
            .await
            .map_err(panicked)?
        };
        let sample = match sample {
            Ok(sample) => sample,
            // A header narrower than a row of the sample: this sheet has no names
            // for its columns, but the others still do. Skipped, and said so.
            Err(XlsxError::Invalid(Invalid::BeyondHeader)) => {
                let name = unique_table_names(&[sheet.name.as_str()]).remove(0);
                skipped.push(SkippedSheet {
                    sheet: name,
                    reason: SKIPPED_HEADER_ROW.to_string(),
                });
                continue;
            }
            Err(e) => return Err(fail(e.into())),
        };
        let Some(sample) = sample else { continue };
        // Fail now, not after the whole sheet, when even the smallest possible
        // table list cannot fit the registry row.
        let columns: Vec<(&str, ColumnType)> = sample
            .names
            .iter()
            .map(String::as_str)
            .zip(sample.types.iter().copied())
            .collect();
        let min = min_tables_json_len(&columns, sample.rows);
        if min > TABLES_JSON_MAX_BYTES {
            let e = ManifestError::ManifestTooLarge {
                bytes: min,
                cap: TABLES_JSON_MAX_BYTES,
            };
            return Err(fail(ConvertError::from(e).into()));
        }
        let table_idx = tables.len();
        let mut text_columns: BTreeSet<usize> = BTreeSet::new();
        let (mut restarts, mut all_strings) = (0usize, false);
        let done = loop {
            let types: Vec<ColumnType> = sample
                .types
                .iter()
                .enumerate()
                .map(|(i, t)| {
                    if all_strings || text_columns.contains(&i) {
                        ColumnType::String
                    } else {
                        *t
                    }
                })
                .collect();
            let plan = Plan {
                part: &sheet.part,
                table_idx,
                names: &sample.names,
                types: &types,
                limits: sheet_limits,
                cfg,
            };
            let run = run_sheet(&book, &plan, &sink, &cancel).await;
            match run {
                Ok(written) => break (written, types),
                Err(RunEnd::Conflict(c)) if !all_strings => {
                    restarts += 1;
                    if restarts >= MAX_RESTARTS {
                        all_strings = true;
                    } else {
                        text_columns.insert(c.column);
                    }
                }
                Err(RunEnd::Conflict(_)) => {
                    let e = ConvertError::Cast("a run that cannot conflict did".into());
                    return Err(fail(e.into()));
                }
                Err(RunEnd::Failed(e)) => return Err(fail(e)),
            }
        };
        let ((written, stats), types) = done;
        let blob_paths = control.paths_of(table_idx);
        let live: BTreeSet<String> = (0..written.parts as usize)
            .filter_map(|i| part_path(table_idx, i).ok())
            .collect();
        let stale_paths = blob_paths
            .iter()
            .filter(|p| !live.contains(*p))
            .cloned()
            .collect();
        let demoted = sample
            .names
            .iter()
            .zip(sample.types.iter().zip(&types))
            .filter(|(_, (inferred, now))| {
                **now == ColumnType::String && **inferred != ColumnType::String
            })
            .map(|(name, _)| name.clone())
            .collect();
        tables.push(SheetTable {
            sheet: sheet.name.clone(),
            table: ConvertedTable {
                written,
                restarts,
                demoted,
                all_strings,
                encoding: Encoding::Utf8,
                replacements: 0,
                utf8_valid_multibyte: 0,
                utf8_invalid: 0,
                blank_rows: 0,
                blank_dropped: stats.blank_rows,
                padded_rows: 0,
                blob_paths,
                stale_paths,
            },
        });
        // The table lists of all the sheets share one registry row, and the manifest
        // (tables, conversion reports, skipped sheets) one file: say so now, with their
        // own sentences, not after the last sheet.
        if let Err(cap) = manifest_fits(&tables, &skipped) {
            return Err(fail(XlsxError::TooLarge(cap).into()));
        }
    }
    if tables.is_empty() {
        // Every sheet with a value was skipped: say why the first one was.
        let why = if skipped.is_empty() {
            Invalid::NoData
        } else {
            Invalid::BeyondHeader
        };
        return Err(fail(XlsxError::Invalid(why).into()));
    }
    Ok(Converted { tables, skipped })
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

    struct BytesSource(Vec<u8>);

    #[async_trait]
    impl XlsxSource for BytesSource {
        async fn spool(&self, cancel: &CancellationToken) -> Result<Spooled, XlsxError> {
            let dir = tempfile::tempdir().unwrap();
            let stream = Box::pin(futures::stream::iter(vec![Ok(Bytes::from(self.0.clone()))]));
            let read = AtomicU64::new(0);
            spool_stream(dir.path(), stream, None, MAX_XLSX_BYTES, cancel, &read).await
        }
    }

    type Outcome = Result<Converted, TableFailure>;

    async fn convert(bytes: Vec<u8>) -> (Outcome, Arc<MemorySink>) {
        convert_with(bytes, WriterConfig::default(), &Limits::default()).await
    }

    async fn convert_with(
        bytes: Vec<u8>,
        cfg: WriterConfig,
        limits: &Limits,
    ) -> (Outcome, Arc<MemorySink>) {
        let sink = Arc::new(MemorySink::default());
        let control = ConvertControl::new();
        let result = convert_xlsx_limits(
            &BytesSource(bytes),
            sink.clone() as Arc<dyn PartSink>,
            cfg,
            &control,
            limits,
        )
        .await;
        (result, sink)
    }

    /// Every row of table `idx`, each value as text (`null` for a null).
    fn rows_of(sink: &MemorySink, idx: usize, parts: u32) -> Vec<Vec<String>> {
        let mut out = Vec::new();
        for p in 0..parts as usize {
            let bytes = sink.get(&part_path(idx, p).unwrap()).unwrap();
            for batch in ParquetRecordBatchReaderBuilder::try_new(bytes)
                .unwrap()
                .build()
                .unwrap()
            {
                let batch = batch.unwrap();
                for row in 0..batch.num_rows() {
                    out.push(
                        (0..batch.num_columns())
                            .map(|c| {
                                let col = batch.column(c);
                                if col.is_null(row) {
                                    "null".to_string()
                                } else {
                                    arrow_cast::display::array_value_to_string(col, row).unwrap()
                                }
                            })
                            .collect(),
                    );
                }
            }
        }
        out
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

    fn types(t: &SheetTable) -> Vec<ColumnType> {
        t.table
            .written
            .columns
            .iter()
            .map(|c| c.column_type)
            .collect()
    }

    #[tokio::test]
    async fn a_workbook_becomes_one_table_per_sheet_that_has_a_value() {
        let sales = [
            row(1, &[text("A", 1, "id"), text("B", 1, "name")]),
            row(2, &[num("A", 2, 1), text("B", 2, "a")]),
            row(3, &[num("A", 3, 2), text("B", 3, "b")]),
        ]
        .concat();
        let notes = [
            row(1, &[text("A", 1, "note")]),
            row(2, &[text("A", 2, "hidden")]),
        ]
        .concat();
        let bytes = Wb::new()
            .sheet("Sales", &sales)
            .sheet("Empty", "")
            .sheet("Notes", &notes)
            .hidden(2)
            .build();
        let (result, sink) = convert(bytes).await;
        let tables = result.unwrap();
        let names: Vec<_> = tables.iter().map(|t| t.sheet.as_str()).collect();
        assert_eq!(
            names,
            ["Sales", "Notes"],
            "the empty sheet is not a table, the hidden one is"
        );
        let sales = &tables[0];
        assert_eq!(
            (sales.table.written.rows, sales.table.written.parts),
            (2, 1)
        );
        assert_eq!(types(sales), [ColumnType::Int, ColumnType::String]);
        let columns: Vec<_> = sales
            .table
            .written
            .columns
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        assert_eq!(columns, ["id", "name"]);
        assert_eq!(rows_of(&sink, 0, 1), [["1", "a"], ["2", "b"]]);
        assert_eq!(rows_of(&sink, 1, 1), [["hidden"]]);
        let parts = ["t0/part-00000.parquet", "t1/part-00000.parquet"].map(String::from);
        assert_eq!(
            sink.paths(),
            parts,
            "no manifest is written here, only parts"
        );
        assert_eq!(tables[1].table.live_paths(1), ["t1/part-00000.parquet"]);
    }

    #[tokio::test]
    async fn a_late_conflict_makes_the_column_text_and_keeps_every_value() {
        // 10,010 rows: whole numbers, and a text cell after the 10,000 the types come from.
        let mut rows = vec![row(1, &[text("A", 1, "n")])];
        for r in 2..=10_011 {
            let cell = if r == 10_006 {
                text("A", r, "N/A")
            } else {
                num("A", r, r - 1)
            };
            rows.push(row(r, &[cell]));
        }
        let (result, sink) = convert(Wb::new().sheet("Data", &rows.concat()).build()).await;
        let tables = result.unwrap();
        let t = &tables[0].table;
        assert_eq!((t.restarts, t.all_strings), (1, false));
        assert_eq!(t.demoted, ["n"]);
        assert_eq!(types(&tables[0]), [ColumnType::String]);
        let got = rows_of(&sink, 0, t.written.parts);
        assert_eq!(got.len(), 10_010);
        assert_eq!(got[0], ["1"]);
        assert_eq!(got[10_004], ["N/A"]);
        assert_eq!(got[10_009], ["10010"]);
    }

    #[tokio::test]
    async fn a_workbook_from_a_real_library_is_typed_by_the_kind_of_each_cell() {
        use rust_xlsxwriter::{ExcelDateTime, Format, Workbook};
        let mut book = Workbook::new();
        let sheet = book.add_worksheet();
        sheet.set_name("Data").unwrap();
        for (c, name) in ["n", "f", "b", "d", "zip", "mixed"].iter().enumerate() {
            sheet.write_string(0, c as u16, *name).unwrap();
        }
        let date = Format::new().set_num_format("yyyy-mm-dd");
        for i in 0..3u32 {
            let r = i + 1;
            sheet.write_number(r, 0, f64::from(i) + 1.0).unwrap();
            sheet.write_number(r, 1, 0.1 + f64::from(i)).unwrap();
            sheet.write_boolean(r, 2, i % 2 == 0).unwrap();
            let when = ExcelDateTime::from_ymd(2021, 1, (i + 1) as u8).unwrap();
            sheet
                .write_datetime_with_format(r, 3, &when, &date)
                .unwrap();
            sheet.write_string(r, 4, format!("0{}501", i)).unwrap();
            if i == 1 {
                sheet.write_string(r, 5, "N/A").unwrap();
            } else {
                sheet.write_number(r, 5, 42.0).unwrap();
            }
        }
        let (result, sink) = convert(book.save_to_buffer().unwrap()).await;
        let tables = result.unwrap();
        use ColumnType::*;
        assert_eq!(types(&tables[0]), [Int, Float, Bool, Date, String, String]);
        let got = rows_of(&sink, 0, 1);
        assert_eq!(got[0], ["1", "0.1", "true", "2021-01-01", "00501", "42"]);
        assert_eq!(got[1], ["2", "1.1", "false", "2021-01-02", "01501", "N/A"]);
        assert_eq!(
            got[2][4], "02501",
            "text that looks like a number keeps its zeros"
        );
        assert_eq!(tables[0].table.restarts, 0);
    }

    #[tokio::test]
    async fn three_conflicts_make_every_column_text_and_keep_every_value() {
        // Three numeric columns that each get a text cell after the 10,000 rows the
        // types come from, one after another: three restarts, the last all text.
        let mut rows = vec![row(
            1,
            &[text("A", 1, "a"), text("B", 1, "b"), text("C", 1, "c")],
        )];
        for r in 2..=10_010 {
            let cell = |col: &str, at: usize, t: &str| {
                if r == at {
                    text(col, r, t)
                } else {
                    num(col, r, r)
                }
            };
            rows.push(row(
                r,
                &[
                    cell("A", 10_003, "x"),
                    cell("B", 10_004, "y"),
                    cell("C", 10_005, "z"),
                ],
            ));
        }
        let (result, sink) = convert(Wb::new().sheet("Data", &rows.concat()).build()).await;
        let tables = result.unwrap();
        let t = &tables[0].table;
        assert_eq!((t.restarts, t.all_strings), (3, true));
        assert_eq!(t.demoted, ["a", "b", "c"]);
        assert_eq!(types(&tables[0]), [ColumnType::String; 3]);
        let got = rows_of(&sink, 0, t.written.parts);
        assert_eq!(got.len(), 10_009);
        assert_eq!(got[10_001], ["x", "10003", "10003"]);
        assert_eq!(got[10_002], ["10004", "y", "10004"]);
        assert_eq!(got[10_003], ["10005", "10005", "z"]);
        assert_eq!(got[0], ["2", "2", "2"]);
    }

    #[tokio::test]
    async fn a_workbook_with_no_value_is_refused_and_a_header_only_sheet_is_a_table() {
        let (result, sink) = convert(Wb::new().sheet("A", "").sheet("B", "").build()).await;
        let failure = result.err().unwrap();
        assert!(matches!(
            failure.error,
            TableError::Xlsx(XlsxError::Invalid(Invalid::NoData))
        ));
        assert!(sink.paths().is_empty());
        let header = row(1, &[text("A", 1, "only"), text("B", 1, "header")]);
        let (result, sink) = convert(Wb::new().sheet("H", &header).build()).await;
        let tables = result.unwrap();
        assert_eq!(
            (tables[0].table.written.rows, tables[0].table.written.parts),
            (0, 1)
        );
        assert_eq!(rows_of(&sink, 0, 1), Vec::<Vec<String>>::new());
        assert_eq!(types(&tables[0]), [ColumnType::String; 2]);
    }

    #[tokio::test]
    async fn the_cell_cap_counts_the_whole_workbook_not_each_sheet() {
        let sheet = [
            row(1, &[text("A", 1, "n")]),
            row(2, &[num("A", 2, 1)]),
            row(3, &[num("A", 3, 2)]),
        ]
        .concat();
        let book = || Wb::new().sheet("One", &sheet).sheet("Two", &sheet).build();
        let mut limits = Limits::default();
        // Each sheet is read twice (sampled, then written): 2 x 2 x 3 = 12 cells.
        limits.sheet.max_cells = 12;
        let (result, _) = convert_with(book(), WriterConfig::default(), &limits).await;
        assert_eq!(result.unwrap().len(), 2, "twelve cells fit exactly");
        limits.sheet.max_cells = 11;
        let (result, _) = convert_with(book(), WriterConfig::default(), &limits).await;
        let failure = result.err().unwrap();
        assert!(matches!(
            failure.error,
            TableError::Xlsx(XlsxError::TooLarge(
                crate::tabular_prepare::xlsx_spool::Cap::Cells
            ))
        ));
    }

    /// A sink that cancels the conversion at its first put.
    struct CancelOnPut {
        inner: MemorySink,
        control: Arc<ConvertControl>,
    }

    #[async_trait]
    impl PartSink for CancelOnPut {
        async fn put(
            &self,
            path: &str,
            data: Bytes,
        ) -> Result<(), crate::tabular_prepare::part_sink::SinkError> {
            self.inner.put(path, data).await?;
            self.control.cancel();
            Ok(())
        }
    }

    #[tokio::test]
    async fn cancelling_stops_the_read_and_reports_what_may_exist() {
        let mut rows = vec![row(1, &[text("A", 1, "n")])];
        rows.extend((2..=60_000).map(|r| row(r, &[num("A", r, r)])));
        let control = ConvertControl::new();
        let sink = Arc::new(CancelOnPut {
            inner: MemorySink::default(),
            control: control.clone(),
        });
        let cfg = WriterConfig {
            max_rows: 100,
            ..WriterConfig::default()
        };
        let source = BytesSource(Wb::new().sheet("Data", &rows.concat()).build());
        let limits = Limits::default();
        let done = convert_xlsx_limits(&source, sink.clone(), cfg, &control, &limits);
        let result = tokio::time::timeout(std::time::Duration::from_secs(60), done)
            .await
            .expect("a cancelled conversion ends");
        let failure = result.err().unwrap();
        assert!(matches!(
            failure.error,
            TableError::Xlsx(XlsxError::Cancelled)
        ));
        assert!(failure
            .blob_paths
            .contains(&"t0/part-00000.parquet".to_string()));
        // It stopped before writing all 600 parts.
        assert!(
            sink.inner.paths().len() < 600,
            "{} parts",
            sink.inner.paths().len()
        );
    }

    #[tokio::test]
    async fn a_refused_workbook_says_nothing_about_its_cells_or_sheets() {
        let rows = [
            row(1, &[text("A", 1, "a")]),
            row(2, &[num("A", 2, 1), text("B", 2, "secret cell text")]),
        ]
        .concat();
        let (result, sink) = convert(Wb::new().sheet("Secret sheet", &rows).build()).await;
        let failure = result.err().unwrap();
        let said = failure.error.to_string();
        assert!(
            !said.contains("secret") && !said.contains("Secret"),
            "{said}"
        );
        assert!(matches!(
            failure.error,
            TableError::Xlsx(XlsxError::Invalid(Invalid::BeyondHeader))
        ));
        assert!(failure.blob_paths.is_empty() && sink.paths().is_empty());
    }

    #[tokio::test]
    async fn the_cell_cap_counts_every_read_of_a_sheet_restarts_included() {
        // 10,011 rows with a text cell after the 10,000 the types come from: the
        // sample (10,001 cells), the run that conflicts (10,006) and the run that
        // does not (10,011) read about 30,000 cells of a sheet that holds 10,011.
        let mut rows = vec![row(1, &[text("A", 1, "n")])];
        for r in 2..=10_011 {
            let cell = if r == 10_006 {
                text("A", r, "N/A")
            } else {
                num("A", r, r)
            };
            rows.push(row(r, &[cell]));
        }
        let book = Wb::new().sheet("Data", &rows.concat()).build();
        let mut limits = Limits::default();
        limits.sheet.max_cells = 31_000;
        let (result, _) = convert_with(book.clone(), WriterConfig::default(), &limits).await;
        assert_eq!(result.unwrap()[0].table.restarts, 1);
        limits.sheet.max_cells = 25_000;
        let (result, _) = convert_with(book, WriterConfig::default(), &limits).await;
        let failure = result.err().unwrap();
        assert!(matches!(
            failure.error,
            TableError::Xlsx(XlsxError::TooLarge(
                crate::tabular_prepare::xlsx_spool::Cap::Cells
            ))
        ));
    }

    #[tokio::test]
    async fn a_17_digit_id_is_stored_exactly_and_never_as_a_rounded_float() {
        let rows = [
            row(1, &[text("A", 1, "id"), text("B", 1, "mixed")]),
            row(
                2,
                &[
                    num("A", 2, "12345678901234567"),
                    num("B", 2, "12345678901234567"),
                ],
            ),
            row(3, &[num("A", 3, "12345678901234568"), num("B", 3, "1.5")]),
        ]
        .concat();
        let (result, sink) = convert(Wb::new().sheet("Data", &rows).build()).await;
        let tables = result.unwrap();
        // The id column is 64-bit integers; the column that also has a fraction is
        // text, because a float would have rounded the id.
        assert_eq!(types(&tables[0]), [ColumnType::Int, ColumnType::String]);
        assert_eq!(
            rows_of(&sink, 0, 1),
            [
                ["12345678901234567", "12345678901234567"],
                ["12345678901234568", "1.5"]
            ]
        );
    }

    /// A sheet with a title above its table: the first row with a value has one cell
    /// and the next has two.
    fn titled() -> String {
        [
            row(1, &[text("A", 1, "Quarterly report")]),
            row(2, &[text("A", 2, "id"), text("B", 2, "name")]),
            row(3, &[num("A", 3, 1), text("B", 3, "a")]),
        ]
        .concat()
    }

    fn plain() -> String {
        [
            row(1, &[text("A", 1, "id"), text("B", 1, "name")]),
            row(2, &[num("A", 2, 1), text("B", 2, "a")]),
        ]
        .concat()
    }

    #[tokio::test]
    async fn a_sheet_without_a_header_is_skipped_and_the_others_survive() {
        let book = Wb::new()
            .sheet("Title sheet", &titled())
            .sheet("Data", &plain())
            .build();
        let (result, sink) = convert(book).await;
        let converted = result.unwrap();
        let names: Vec<_> = converted.iter().map(|t| t.sheet.as_str()).collect();
        assert_eq!(names, ["Data"]);
        assert_eq!(converted.skipped.len(), 1);
        assert_eq!(converted.skipped[0].sheet, "Title sheet");
        assert_eq!(converted.skipped[0].reason, "header_row");
        // The skipped sheet wrote nothing: the table that survived is table 0.
        assert_eq!(sink.paths(), ["t0/part-00000.parquet".to_string()]);
        // Every sheet with a value skipped: the file is refused, saying why.
        let (result, _) = convert(Wb::new().sheet("Only", &titled()).build()).await;
        let failure = result.err().unwrap();
        assert!(matches!(
            failure.error,
            TableError::Xlsx(XlsxError::Invalid(Invalid::BeyondHeader))
        ));
        // A ragged row after the 10,000 the sample reads is not skipped: parts of the
        // sheet exist by then, so the whole file fails (the stated limit of the rule).
        let mut rows = vec![row(1, &[text("A", 1, "n")])];
        rows.extend((2..=10_010).map(|r| row(r, &[num("A", r, r)])));
        rows.push(row(
            10_011,
            &[num("A", 10_011, 1), text("B", 10_011, "stray")],
        ));
        let (result, _) = convert(Wb::new().sheet("Late", &rows.concat()).build()).await;
        assert!(matches!(
            result.err().unwrap().error,
            TableError::Xlsx(XlsxError::Invalid(Invalid::BeyondHeader))
        ));
    }

    #[tokio::test]
    async fn the_table_lists_of_many_sheets_fail_early_with_their_own_error() {
        // 24 sheets of 40 columns: about 3.5 KiB of table list each, so the 64 KiB of
        // the registry row runs out around the 19th sheet, not at the end.
        let sheet = |n: usize| {
            let header: String = (0..40)
                .map(|c| {
                    text(
                        &format!(
                            "{}{}",
                            (b'A' + (c / 26) as u8) as char,
                            (b'A' + (c % 26) as u8) as char
                        ),
                        1,
                        &format!("column {n} {c}"),
                    )
                })
                .collect();
            let data: String = (0..40)
                .map(|c| {
                    num(
                        &format!(
                            "{}{}",
                            (b'A' + (c / 26) as u8) as char,
                            (b'A' + (c % 26) as u8) as char
                        ),
                        2,
                        c,
                    )
                })
                .collect();
            format!("<row r=\"1\">{header}</row><row r=\"2\">{data}</row>")
        };
        let mut book = Wb::new();
        for n in 0..24 {
            book = book.sheet(&format!("S{n}"), &sheet(n));
        }
        let (result, sink) = convert(book.build()).await;
        let failure = result.err().unwrap();
        assert!(matches!(
            failure.error,
            TableError::Xlsx(XlsxError::TooLarge(
                crate::tabular_prepare::xlsx_spool::Cap::TableList
            ))
        ));
        assert!(
            sink.paths().len() < 24,
            "{} sheets written",
            sink.paths().len()
        );
    }

    #[test]
    fn the_whole_manifest_is_checked_after_every_sheet_not_only_the_table_list() {
        use crate::tabular_prepare::manifest::ColumnInfo;
        use crate::tabular_prepare::writer::TableWritten;
        // A table whose conversion report lists 32 demoted columns with 128-character
        // names: about 4.4 KiB of report for a 200-byte table list.
        let sheet = |i: usize| SheetTable {
            sheet: format!("S{i}"),
            table: ConvertedTable {
                written: TableWritten {
                    rows: 1,
                    parts: 1,
                    columns: vec![ColumnInfo {
                        name: "a".into(),
                        column_type: ColumnType::String,
                        uncompressed_bytes: 1,
                        in_memory_bytes: 40,
                    }],
                },
                restarts: 3,
                demoted: (0..32).map(|c| format!("{c:0>128}")).collect(),
                all_strings: true,
                encoding: Encoding::Utf8,
                replacements: 0,
                utf8_valid_multibyte: 0,
                utf8_invalid: 0,
                blank_rows: 0,
                blank_dropped: 0,
                padded_rows: 0,
                blob_paths: Vec::new(),
                stale_paths: Vec::new(),
            },
        };
        let tables: Vec<SheetTable> = (0..40).map(sheet).collect();
        // The table list is tiny: 40 tables do not come near 64 KiB.
        assert_eq!(manifest_fits(&tables[..20], &[]), Ok(()));
        // The reports are what overflows the 128 KiB file, between 20 and 40 sheets.
        assert_eq!(manifest_fits(&tables, &[]), Err(Cap::Manifest));
        let first_bad = (21..=40)
            .find(|n| manifest_fits(&tables[..*n], &[]).is_err())
            .unwrap();
        assert!(manifest_fits(&tables[..first_bad - 1], &[]).is_ok());
        // Skipped sheets count too.
        let skipped: Vec<SkippedSheet> = (0..256)
            .map(|i| SkippedSheet {
                sheet: format!("{i:0>60}"),
                reason: SKIPPED_HEADER_ROW.into(),
            })
            .collect();
        assert!(manifest_fits(&tables[..first_bad - 1], &skipped).is_err());
    }
}
