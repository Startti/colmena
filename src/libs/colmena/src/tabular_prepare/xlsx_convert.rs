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
//! - Cells are capped over the whole workbook: each sheet may use what the sheets
//!   before it left of the 50,000,000.

use crate::tabular_prepare::convert::{
    ConvertControl, ConvertError, ConvertedTable, TableError, TableFailure, TrackingSink,
    MAX_RESTARTS,
};
use crate::tabular_prepare::csv::Encoding;
use crate::tabular_prepare::manifest::{
    min_tables_json_len, part_path, ColumnType, ManifestError, TABLES_JSON_MAX_BYTES,
};
use crate::tabular_prepare::part_sink::PartSink;
use crate::tabular_prepare::writer::WriterConfig;
use crate::tabular_prepare::xlsx_package::XlsxLimits;
use crate::tabular_prepare::xlsx_run::{open_book, run_sheet, sample_sheet, Plan, RunEnd};
use crate::tabular_prepare::xlsx_sheet::SheetLimits;
use crate::tabular_prepare::xlsx_spool::{Invalid, Spooled, XlsxError};
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

/// Converts the workbook into the parts of one table per sheet that has a value,
/// in the order of the workbook.
pub async fn convert_xlsx(
    source: &dyn XlsxSource,
    sink: Arc<dyn PartSink>,
    cfg: WriterConfig,
    control: &Arc<ConvertControl>,
) -> Result<Vec<SheetTable>, TableFailure> {
    convert_xlsx_limits(source, sink, cfg, control, &Limits::default()).await
}

pub(crate) async fn convert_xlsx_limits(
    source: &dyn XlsxSource,
    sink: Arc<dyn PartSink>,
    cfg: WriterConfig,
    control: &Arc<ConvertControl>,
    limits: &Limits,
) -> Result<Vec<SheetTable>, TableFailure> {
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
    let book = tokio::task::spawn_blocking(move || open_book(spooled, &xlsx))
        .await
        .map_err(panicked)?
        .map_err(|e| fail(e.into()))?;
    let sheets = book.sheets.clone();
    let book = Arc::new(Mutex::new(book));

    let mut tables: Vec<SheetTable> = Vec::new();
    let mut cells_used = 0u64;
    for sheet in &sheets {
        // Each sheet may use what the ones before it left of the workbook's cells.
        let mut sheet_limits = limits.sheet;
        sheet_limits.max_cells = limits.sheet.max_cells.saturating_sub(cells_used);
        let sample = {
            let (book, part, cancel) = (book.clone(), sheet.part.clone(), cancel.clone());
            tokio::task::spawn_blocking(move || {
                let mut book = book.lock().unwrap_or_else(|p| p.into_inner());
                sample_sheet(&mut book, &part, &sheet_limits, &cancel)
            })
            .await
            .map_err(panicked)?
            .map_err(|e| fail(e.into()))?
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
        cells_used += stats.cells;
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
    }
    if tables.is_empty() {
        return Err(fail(XlsxError::Invalid(Invalid::NoData).into()));
    }
    Ok(tables)
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

    type Converted = Result<Vec<SheetTable>, TableFailure>;

    async fn convert(bytes: Vec<u8>) -> (Converted, Arc<MemorySink>) {
        convert_with(bytes, WriterConfig::default(), &Limits::default()).await
    }

    async fn convert_with(
        bytes: Vec<u8>,
        cfg: WriterConfig,
        limits: &Limits,
    ) -> (Converted, Arc<MemorySink>) {
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
}
