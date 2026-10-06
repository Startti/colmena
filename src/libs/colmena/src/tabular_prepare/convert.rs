//! Converting a CSV into Parquet parts (dark behind `COLMENA_LARGE_TABULAR`):
//! typing the batches, restarting when a column turns out to be text after
//! all, and reading a storage stream as a blocking source.
//!
//! The reader hands over text. Each cell is checked against the type of its
//! column by the same rules that chose the type ([`cell_fits`]), never by what
//! a number parser would accept: a late `007` in an integer column must not
//! quietly become `7`. A cell that does not fit is a [`TypeConflict`] naming
//! its column and row; the conversion demotes that column to text and starts
//! over ([`convert_csv_table`]).
//!
//! # Memory
//!
//! Every stage is bounded by constants, never by the file. Records the reader
//! keeps are copied into buffers of their exact size: the parser grows its own
//! buffer by doubling and `ByteRecord::clone` copies all of it, which kept up to
//! twice the text (116 instead of 96 MiB at 700 columns, 54 instead of 39 at
//! 16,384). Per kept record there are also about 100 bytes of structure.
//!
//! - **Reader and sample.** A record is at most `MAX_RECORD_BYTES` = 1 MiB (the
//!   scanner fails at the limit, also inside an unclosed quote). The type sample
//!   keeps at most `SAMPLE_MAX_BYTES` = 16 MiB of text (plus one record), and at
//!   most `SAMPLE_MAX_CELLS` = 2,000,000 fields at 8 bytes of field ends each =
//!   16 MiB, over at most `INFERENCE_ROWS` = 10,000 records (about 1 MiB of
//!   structure), plus about 2 MiB of sniff and parser buffers: 35 MiB, drained
//!   as the first batches are built. A very wide file (up to 16,384 columns)
//!   adds header-sized structures (names, schema, the table-list check), about
//!   8 MiB at most (6 measured): 43 MiB. Such a file never reaches a batch: the
//!   table list (cap 64 KiB, about 900 columns) cannot fit the registry row,
//!   and the run stops right after the sample (measured 32 MiB at 3,000
//!   columns, 39 at 16,384).
//! - **A batch.** At most `BATCH_ROWS` = 8,192 rows, `BATCH_CELLS` = 1,000,000
//!   cells and `BATCH_BYTES` = 8 MiB of text, whatever the row size (a record
//!   that would pass the budget starts the next batch). The reader first holds
//!   the batch's records (text 8 MiB + the held one, up to 1 MiB + 8 bytes of
//!   field ends per cell, 8 MiB + about 1 MiB of structure = 18 MiB), then
//!   builds each column with the exact capacity it needs (text 8 MiB + 4 bytes
//!   of offset per cell, 4 MiB + validity, 0.1 MiB = 12 MiB) and drops the
//!   records: 31 MiB at the peak of building (18 + 12 + 1), 12 MiB after.
//!   Typing adds at most 8 MiB of numbers (text columns are shared, not
//!   copied): 21 MiB. Windows-1252 expansion cannot break it: the budget counts
//!   decoded bytes.
//! - **In flight.** The reader building one batch (31 MiB), the channel holding
//!   `CHANNEL_BATCHES` = 2 and the writer holding one (21 MiB each): 94 MiB.
//! - **The writer.** One part is built in memory: its encoded size is closed at
//!   `PART_MAX_BYTES` = 64 MiB, overshooting by at most one slice (an oversized
//!   batch is cut into slices first), and finishing it holds the encoded row
//!   group and its output at once: 2 x (64 + 21) = 170 MiB.
//!
//! In all, as a ceiling that adds every worst case even though they do not
//! coincide: 94 + 170 + 43 (the sample, drained early) + 2 = 309 MiB at the
//! default settings, against the 4 GiB of the preparation job. With a part
//! limit of 8 MiB (the `tabular_convert_memory` test) it is
//! 94 + 2 x (8 + 21) + 43 + 2 = 197 MiB.
//!
//! The test measures the peak of live heap bytes with a counting allocator, in
//! a debug build, and gives each scenario its own bound, the measured peak plus
//! a stated margin, so that the slack that was removed would fail it: 55 MiB
//! (bound 64) for a 270 MB file of 96 KiB rows, and for one four times smaller;
//! 96 MiB (bound 106) for 700 columns; 32 and 39 MiB (bounds 38 and 46) for
//! 3,000 and 16,384 columns refused after the sample. It fails at 864 MiB when
//! the byte budget is removed. A margin of 10 MiB cannot see the 3.5 MiB that
//! builders sized empty would keep at 700 columns: a unit test of the batch's
//! own size covers that.

use crate::storage::domain::StorageError;
use crate::tabular_prepare::csv::{
    open_csv_with, CsvError, DecodeStats, Encoding, RawBatches, ReadLimits,
};
use crate::tabular_prepare::infer::{cell_fits, InferredSchema};
use crate::tabular_prepare::manifest::{
    min_tables_json_len, part_path, ColumnType, ManifestError, TABLES_JSON_MAX_BYTES,
};
use crate::tabular_prepare::part_sink::{PartSink, SinkError};
use crate::tabular_prepare::scan::ScanStats;
use crate::tabular_prepare::writer::{PartWriter, TableWritten, WriterConfig, WriterError};
use arrow_array::{Array, ArrayRef, RecordBatch, StringArray};
use arrow_cast::cast;
use arrow_schema::{DataType, SchemaRef};
use async_trait::async_trait;
use bytes::Bytes;
use futures::{Stream, StreamExt};
use std::collections::BTreeSet;
use std::io::{self, Read};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use thiserror::Error;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

/// A cell that does not fit the type of its column. `row` counts data rows
/// from zero (the header is not a row).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeConflict {
    pub column: usize,
    pub row: u64,
}

#[derive(Debug, Error)]
pub enum ConvertError {
    #[error(transparent)]
    Csv(#[from] CsvError),
    #[error(transparent)]
    Manifest(#[from] ManifestError),
    #[error("column {} does not fit its type at data row {}", .0.column, .0.row + 1)]
    Conflict(TypeConflict),
    #[error("could not convert a column: {0}")]
    Cast(String),
    /// The blocking half of the conversion panicked. Never silent and never a
    /// short table: the run fails with this.
    #[error("the file reader panicked")]
    ReaderPanicked,
}

/// The batches of a CSV with each column in the type of `schema`. The first
/// error ends the iteration.
pub struct TypedBatches {
    raw: RawBatches,
    types: Vec<ColumnType>,
    arrow: SchemaRef,
    rows_done: u64,
    done: bool,
}

impl TypedBatches {
    /// `schema` is the effective schema: the inferred one, with any column the
    /// caller demoted already set to text.
    pub fn new(raw: RawBatches, schema: InferredSchema) -> Self {
        Self {
            raw,
            types: schema.columns.iter().map(|c| c.column_type).collect(),
            arrow: schema.arrow_schema(),
            rows_done: 0,
            done: false,
        }
    }

    fn type_batch(&self, raw: &RecordBatch) -> Result<RecordBatch, ConvertError> {
        let mut first: Option<TypeConflict> = None;
        let mut columns: Vec<ArrayRef> = Vec::with_capacity(self.types.len());
        for (c, (t, field)) in self.types.iter().zip(self.arrow.fields()).enumerate() {
            let col = raw.column(c);
            if *t == ColumnType::String {
                columns.push(col.clone());
                continue;
            }
            let text = col
                .as_any()
                .downcast_ref::<StringArray>()
                .ok_or_else(|| ConvertError::Cast("a column is not text".into()))?;
            match type_column(text, *t, field.data_type()) {
                Ok(typed) => columns.push(typed),
                Err(i) => {
                    let row = self.rows_done + i as u64;
                    if first.as_ref().is_none_or(|f| row < f.row) {
                        first = Some(TypeConflict { column: c, row });
                    }
                }
            }
        }
        if let Some(conflict) = first {
            return Err(ConvertError::Conflict(conflict));
        }
        RecordBatch::try_new(self.arrow.clone(), columns)
            .map_err(|e| ConvertError::Cast(e.to_string()))
    }
}

/// One text column in its type, or the index of the first cell that does not
/// fit.
///
/// A cell fits when the rules that chose the type say so, and the stored value
/// is then checked against the cell: every non-empty cell must come out as a
/// non-null value (Arrow's cast turns what it cannot parse into null), and a
/// float must be exactly the value of its literal. A disagreement between the
/// inference and the cast is a conflict, never a changed value.
fn type_column(text: &StringArray, t: ColumnType, dt: &DataType) -> Result<ArrayRef, usize> {
    if let Some(i) = (0..text.len()).find(|&i| !text.is_null(i) && !cell_fits(text.value(i), t)) {
        return Err(i);
    }
    let typed = cast(text, dt).map_err(|_| 0usize)?;
    if typed.null_count() != text.null_count() {
        return Err((0..text.len())
            .find(|&i| !text.is_null(i) && typed.is_null(i))
            .unwrap_or(0));
    }
    if t == ColumnType::Float {
        let values = typed
            .as_any()
            .downcast_ref::<arrow_array::Float64Array>()
            .ok_or(0usize)?;
        let bad = (0..text.len()).find(|&i| {
            !text.is_null(i)
                && text
                    .value(i)
                    .parse::<f64>()
                    .map_or(true, |v| v.to_bits() != values.value(i).to_bits())
        });
        if let Some(i) = bad {
            return Err(i);
        }
    }
    Ok(typed)
}

impl Iterator for TypedBatches {
    type Item = Result<RecordBatch, ConvertError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        let raw = match self.raw.next()? {
            Ok(b) => b,
            Err(e) => {
                self.done = true;
                return Some(Err(e.into()));
            }
        };
        match self.type_batch(&raw) {
            Ok(b) => {
                self.rows_done += b.num_rows() as u64;
                Some(Ok(b))
            }
            Err(e) => {
                self.done = true;
                Some(Err(e))
            }
        }
    }
}

/// The stream of an object in storage.
pub type ByteStream = Pin<Box<dyn Stream<Item = Result<Bytes, StorageError>> + Send>>;

/// Reads a storage stream as a blocking source, for [`open_csv`]. It must be
/// built inside the runtime and read on a blocking thread (`spawn_blocking`).
/// A storage error becomes an `io::Error` with its text.
///
/// A read that is waiting for the next chunk of a stalled stream is woken by
/// `cancel` and fails with [`CsvError::Cancelled`], so the blocking thread does
/// not outlive a dropped or cancelled conversion. What cannot be interrupted is
/// a read stuck inside the stream's own non-async code; none of this module's
/// readers does that.
///
/// [`open_csv`]: crate::tabular_prepare::csv::open_csv
pub fn stream_reader(stream: ByteStream, cancel: CancellationToken) -> impl Read + Send {
    StreamBridge {
        handle: tokio::runtime::Handle::current(),
        stream,
        chunk: Bytes::new(),
        cancel,
        ended: false,
    }
}

struct StreamBridge {
    handle: tokio::runtime::Handle,
    stream: ByteStream,
    chunk: Bytes,
    cancel: CancellationToken,
    ended: bool,
}

impl Read for StreamBridge {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        while self.chunk.is_empty() {
            if self.ended {
                return Ok(0);
            }
            let (stream, cancel) = (&mut self.stream, &self.cancel);
            let next = self.handle.block_on(async {
                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => Err(CsvError::Cancelled),
                    item = stream.next() => Ok(item),
                }
            });
            match next {
                Err(e) => return Err(e.into_io()),
                Ok(None) => self.ended = true,
                Ok(Some(Ok(chunk))) => self.chunk = chunk,
                Ok(Some(Err(e))) => return Err(io::Error::other(e.to_string())),
            }
        }
        let n = buf.len().min(self.chunk.len());
        buf[..n].copy_from_slice(&self.chunk[..n]);
        self.chunk = self.chunk.slice(n..);
        Ok(n)
    }
}

/// Fails the next read once the token is cancelled.
struct CancelReader<R> {
    inner: R,
    cancel: CancellationToken,
}

impl<R: Read> Read for CancelReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.cancel.is_cancelled() {
            return Err(CsvError::Cancelled.into_io());
        }
        self.inner.read(buf)
    }
}

/// What the caller keeps while a conversion runs: the part keys that may exist
/// in the sink, and the means to stop it. The keys are written *before* each
/// put, here and not in the conversion's own state, so they survive dropping
/// the conversion future (a timeout, a cancelled request): the caller removes
/// them from the sink afterwards.
///
/// Rules for the caller. Wait for the conversion future to finish, or drop it,
/// before deleting anything: a conversion still running can put a key you just
/// deleted. `cancel()` stops the reader but does not interrupt a put in flight,
/// and up to `CHANNEL_BATCHES` batches already read can still be put after it.
/// One control may serve several tables (each result lists only the keys under
/// its own `t<idx>/`), but a table index must not be used twice on one control.
#[derive(Default)]
pub struct ConvertControl {
    paths: Mutex<BTreeSet<String>>,
    cancel: CancellationToken,
}

impl ConvertControl {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Every part key handed to the sink so far, sorted, each once.
    pub fn paths(&self) -> Vec<String> {
        self.paths.lock().map_or_else(
            |poisoned| poisoned.into_inner().iter().cloned().collect(),
            |p| p.iter().cloned().collect(),
        )
    }

    /// Stops the conversion: its reader fails at its next read (or at once if it
    /// is waiting on a stalled stream) and the run ends with
    /// [`CsvError::Cancelled`].
    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    /// The keys of table `table_idx` alone (those under `t<idx>/`): what that
    /// table's result reports, so a control shared by several tables never
    /// lists one table's parts as another's.
    pub fn paths_of(&self, table_idx: usize) -> Vec<String> {
        let prefix = format!("t{table_idx}/");
        self.paths()
            .into_iter()
            .filter(|p| p.starts_with(&prefix))
            .collect()
    }

    fn note(&self, path: &str) {
        match self.paths.lock() {
            Ok(mut p) => p.insert(path.to_string()),
            Err(poisoned) => poisoned.into_inner().insert(path.to_string()),
        };
    }
}

/// Records each path before the put it is for.
struct TrackingSink {
    inner: Arc<dyn PartSink>,
    control: Arc<ConvertControl>,
}

#[async_trait]
impl PartSink for TrackingSink {
    async fn put(&self, path: &str, data: Bytes) -> Result<(), SinkError> {
        self.control.note(path);
        self.inner.put(path, data).await
    }
}

/// Type restarts allowed. The first ones demote the column that conflicted;
/// the last one makes every column text, which cannot conflict. With one more
/// run if the encoding turns out to be Windows-1252, a file costs at most
/// `MAX_RESTARTS + 2` reads.
pub const MAX_RESTARTS: usize = 3;

/// Batches in flight between reading and writing. Together with the batch
/// bounds of the reader this keeps memory fixed whatever the file.
const CHANNEL_BATCHES: usize = 2;

/// A CSV that can be opened from the start, once per run.
#[async_trait]
pub trait CsvSource: Send + Sync {
    /// Opens the source. `cancel` is cancelled when the conversion is dropped or
    /// cancelled; a source whose reads can stall waits on it (see
    /// [`stream_reader`]).
    async fn open(&self, cancel: &CancellationToken) -> Result<Box<dyn Read + Send>, ConvertError>;
}

#[derive(Debug, Error)]
pub enum TableError {
    #[error(transparent)]
    Convert(#[from] ConvertError),
    #[error(transparent)]
    Writer(#[from] WriterError),
}

/// A finished table.
#[derive(Debug)]
pub struct ConvertedTable {
    pub written: TableWritten,
    /// Type restarts (the encoding retry is not counted).
    pub restarts: usize,
    /// Names of every column that was typed from the sample and ended as text
    /// (all of them when `all_strings` is set), as the manifest shows them.
    pub demoted: Vec<String>,
    /// Every column is text because the restarts ran out.
    pub all_strings: bool,
    pub encoding: Encoding,
    /// Invalid UTF-8 sequences replaced by U+FFFD (zero for Windows-1252). A
    /// caller that cares about exact values reports a non-zero count.
    pub replacements: u64,
    /// Non-ASCII UTF-8 characters found in the whole file, whichever encoding
    /// was chosen. With `utf8_invalid` this is the evidence of the choice: a
    /// caller can warn when it was close, or when a file with UTF-8 sequences
    /// was read as Windows-1252.
    pub utf8_valid_multibyte: u64,
    /// Invalid UTF-8 sequences found in the whole file, whichever encoding was
    /// chosen (for Windows-1252 they are what the single bytes looked like).
    pub utf8_invalid: u64,
    /// Blank lines that became null rows (a one-column file).
    pub blank_rows: u64,
    /// Blank lines dropped (several columns, before the header, trailing).
    pub blank_dropped: u64,
    /// Rows with fewer fields than the header, padded with nulls.
    pub padded_rows: u64,
    /// Every part path any run handed to the sink, each once. Runs overwrite
    /// the same keys, so this is the set of objects that may exist.
    pub blob_paths: Vec<String>,
    /// The part of `blob_paths` the manifest does not reference: left by an
    /// aborted run that wrote more parts than the final one. Safe to delete,
    /// and a reader must never list the prefix to find parts.
    pub stale_paths: Vec<String>,
}

impl ConvertedTable {
    /// The keys of the parts of the finished table, in order: the only ones a
    /// manifest or a reader refers to.
    pub fn live_paths(&self, table_idx: usize) -> Vec<String> {
        (0..self.written.parts as usize)
            .filter_map(|i| part_path(table_idx, i).ok())
            .collect()
    }
}

/// A conversion that did not finish. `blob_paths` is what may exist in the
/// sink and has to be removed; nothing is reported as written.
#[derive(Debug)]
pub struct TableFailure {
    pub error: TableError,
    pub blob_paths: Vec<String>,
}

/// What a single run ended with, other than success.
enum RunEnd {
    Conflict(TypeConflict),
    /// The whole file says the other encoding was the right one.
    Reencode(Encoding),
    Failed(TableError),
}

impl From<ConvertError> for RunEnd {
    fn from(e: ConvertError) -> Self {
        match e {
            ConvertError::Conflict(c) => RunEnd::Conflict(c),
            other => RunEnd::Failed(other.into()),
        }
    }
}

impl From<WriterError> for RunEnd {
    fn from(e: WriterError) -> Self {
        RunEnd::Failed(e.into())
    }
}

type Item = Result<Option<RecordBatch>, ConvertError>;

/// What the producer tells the writer before any batch.
struct RunHeader {
    /// The effective schema, with demoted columns as text.
    schema: InferredSchema,
    /// What the sample said, before any demotion.
    inferred: Vec<ColumnType>,
    encoding: Encoding,
    decode: Arc<DecodeStats>,
    scan: Arc<ScanStats>,
}

type Header = Result<RunHeader, ConvertError>;

/// What one finished run reports.
struct RunOk {
    written: TableWritten,
    encoding: Encoding,
    replacements: u64,
    utf8_valid_multibyte: u64,
    utf8_invalid: u64,
    blank_rows: u64,
    blank_dropped: u64,
    padded_rows: u64,
    demoted: Vec<String>,
}

/// The blocking half of a run: parse, type and send batches (`None` marks a
/// clean end). It stops as soon as the receiving side is gone.
fn produce(
    reader: Box<dyn Read + Send>,
    force: Option<Encoding>,
    text_columns: Vec<usize>,
    all_strings: bool,
    limits: ReadLimits,
    schema_tx: oneshot::Sender<Header>,
    tx: mpsc::Sender<Item>,
) {
    let opened = match open_csv_with(reader, force, &limits) {
        Ok(o) => o,
        Err(e) => {
            let _ = schema_tx.send(Err(e.into()));
            return;
        }
    };
    // Fail now, not after the whole file, when even the smallest possible table
    // list cannot fit the registry row.
    let columns: Vec<(&str, ColumnType)> = opened
        .schema
        .columns
        .iter()
        .map(|c| (c.name.as_str(), c.column_type))
        .collect();
    let min = min_tables_json_len(&columns, opened.sample_rows as u64);
    if min > TABLES_JSON_MAX_BYTES {
        let _ = schema_tx.send(Err(ManifestError::ManifestTooLarge {
            bytes: min,
            cap: TABLES_JSON_MAX_BYTES,
        }
        .into()));
        return;
    }
    let inferred: Vec<ColumnType> = opened
        .schema
        .columns
        .iter()
        .map(|c| c.column_type)
        .collect();
    let mut schema = opened.schema.clone();
    for (i, c) in schema.columns.iter_mut().enumerate() {
        if all_strings || text_columns.contains(&i) {
            c.column_type = ColumnType::String;
        }
    }
    if schema_tx
        .send(Ok(RunHeader {
            schema: schema.clone(),
            inferred,
            encoding: opened.encoding,
            decode: opened.decode.clone(),
            scan: opened.stats.clone(),
        }))
        .is_err()
    {
        return;
    }
    for item in TypedBatches::new(opened.batches, schema) {
        if tx.blocking_send(item.map(Some)).is_err() {
            return;
        }
    }
    let _ = tx.blocking_send(Ok(None));
}

/// What one run is asked to do differently from the sample's first guess.
struct RunPlan {
    force: Option<Encoding>,
    text_columns: Vec<usize>,
    all_strings: bool,
    limits: ReadLimits,
}

/// One read of the file, written out as parts through `sink` (which records
/// every key before its put).
async fn run(
    source: &dyn CsvSource,
    sink: &Arc<dyn PartSink>,
    cancel: &CancellationToken,
    table_idx: usize,
    cfg: WriterConfig,
    plan: &RunPlan,
) -> Result<RunOk, RunEnd> {
    let (force, all_strings, limits) = (plan.force, plan.all_strings, plan.limits);
    // A token of this run alone: cancelling it to stop this run's reader must
    // not stop the restarts that may follow.
    let cancel = &cancel.child_token();
    let reader = source.open(cancel).await.map_err(RunEnd::from)?;
    let reader: Box<dyn Read + Send> = Box::new(CancelReader {
        inner: reader,
        cancel: cancel.clone(),
    });
    let (schema_tx, schema_rx) = oneshot::channel();
    let (tx, mut rx) = mpsc::channel(CHANNEL_BATCHES);
    let text_columns = plan.text_columns.clone();
    let producer = tokio::task::spawn_blocking(move || {
        produce(
            reader,
            force,
            text_columns,
            all_strings,
            limits,
            schema_tx,
            tx,
        )
    });

    let outcome = async {
        let header = schema_rx
            .await
            .map_err(|_| ConvertError::Cast("the reader stopped before the header".into()))
            .and_then(|r| r)
            .map_err(RunEnd::from)?;
        let (schema, encoding, decode) = (&header.schema, header.encoding, &header.decode);
        let mut writer = PartWriter::new(sink.clone(), table_idx, schema.arrow_schema(), cfg)?;
        let written = async {
            loop {
                match rx.recv().await {
                    Some(Ok(Some(batch))) => writer.write(&batch).await?,
                    Some(Ok(None)) => {
                        // The file is read to its end: only now do the counts
                        // say whether it was plausibly UTF-8.
                        if force.is_none() {
                            let right = if decode.plausibly_utf8() {
                                Encoding::Utf8
                            } else {
                                Encoding::Windows1252
                            };
                            if right != encoding {
                                return Err(RunEnd::Reencode(right));
                            }
                        }
                        return Ok(writer.finish().await?);
                    }
                    Some(Err(e)) => return Err(RunEnd::from(e)),
                    None => {
                        let e = ConvertError::Cast("the reader stopped unexpectedly".into());
                        return Err(RunEnd::from(e));
                    }
                }
            }
        }
        .await;
        written.map(|w| RunOk {
            encoding,
            // Only UTF-8 replaces; for Windows-1252 the counts are of the bytes
            // as UTF-8, kept for the decision.
            replacements: if encoding == Encoding::Utf8 {
                decode.invalid()
            } else {
                0
            },
            // Final: the file is read to its end.
            utf8_valid_multibyte: decode.valid_multibyte(),
            utf8_invalid: decode.invalid(),
            blank_rows: header.scan.blank_rows(),
            blank_dropped: header.scan.blank_dropped(),
            padded_rows: header.scan.padded_rows(),
            demoted: header
                .schema
                .columns
                .iter()
                .zip(&header.inferred)
                .filter(|(c, was)| {
                    c.column_type == ColumnType::String && **was != ColumnType::String
                })
                .map(|(c, _)| c.name.clone())
                .collect(),
            written: w,
        })
    }
    .await;
    // On any outcome but success the reader may still be going, or parked on a
    // stalled stream: cancel it, close the channel, and only then wait for it, so
    // no blocking thread outlives the run and the failure is reported.
    if outcome.is_err() {
        cancel.cancel();
    }
    drop(rx);
    let joined = producer.await;
    match (&outcome, joined) {
        (Err(RunEnd::Failed(TableError::Convert(ConvertError::Cast(_)))), Err(e))
            if e.is_panic() =>
        {
            Err(RunEnd::Failed(ConvertError::ReaderPanicked.into()))
        }
        _ => outcome,
    }
}

/// Converts one CSV into the parts of table `table_idx`.
///
/// The types come from a sample, so a late cell can contradict them. When one
/// does, that column becomes text and the file is read again from the start;
/// after [`MAX_RESTARTS`] restarts every column is text. Parts are written
/// under deterministic keys, so a restart overwrites what the run before wrote
/// and the result lists each key once. Invalid UTF-8 is replaced and counted
/// (`replacements`); a file whose whole content is not plausibly UTF-8 (more
/// invalid sequences than valid multibyte ones) is read once more as
/// Windows-1252.
///
/// Memory is bounded by the reader's batch limits, a channel of
/// [`CHANNEL_BATCHES`] batches and one part in the writer.
pub async fn convert_csv_table(
    source: &dyn CsvSource,
    sink: Arc<dyn PartSink>,
    table_idx: usize,
    cfg: WriterConfig,
) -> Result<ConvertedTable, TableFailure> {
    convert_csv_table_with(source, sink, table_idx, cfg, &ConvertControl::new()).await
}

/// [`convert_csv_table`] with a caller-owned [`ConvertControl`]: the keys put so
/// far stay readable after the future is dropped, and `control.cancel()` stops
/// the run. Dropping the future cancels the reader too.
pub async fn convert_csv_table_with(
    source: &dyn CsvSource,
    sink: Arc<dyn PartSink>,
    table_idx: usize,
    cfg: WriterConfig,
    control: &Arc<ConvertControl>,
) -> Result<ConvertedTable, TableFailure> {
    convert_csv_table_limits(
        source,
        sink,
        table_idx,
        cfg,
        control,
        &ReadLimits::default(),
    )
    .await
}

/// [`convert_csv_table_with`] with other reader limits (sample, batch). Crate
/// private: only tests move the boundaries, and the limits are validated.
///
/// ```compile_fail
/// use colmena::tabular_prepare::convert::convert_csv_table_limits;
/// ```
pub(crate) async fn convert_csv_table_limits(
    source: &dyn CsvSource,
    sink: Arc<dyn PartSink>,
    table_idx: usize,
    cfg: WriterConfig,
    control: &Arc<ConvertControl>,
    limits: &ReadLimits,
) -> Result<ConvertedTable, TableFailure> {
    // A child of the caller's token, cancelled when this future is dropped.
    let cancel = control.cancel.child_token();
    let _stop_on_drop = cancel.clone().drop_guard();
    let sink: Arc<dyn PartSink> = Arc::new(TrackingSink {
        inner: sink,
        control: control.clone(),
    });
    let mut plan = RunPlan {
        force: None,
        text_columns: Vec::new(),
        all_strings: false,
        limits: *limits,
    };
    let mut restarts = 0;
    let mut failed = None;
    for _ in 0..MAX_RESTARTS + 2 {
        let result = run(source, &sink, &cancel, table_idx, cfg, &plan).await;
        match result {
            Ok(ok) => {
                let blob_paths = control.paths_of(table_idx);
                let live: BTreeSet<String> = (0..ok.written.parts as usize)
                    .filter_map(|i| part_path(table_idx, i).ok())
                    .collect();
                let stale_paths = blob_paths
                    .iter()
                    .filter(|p| !live.contains(*p))
                    .cloned()
                    .collect();
                return Ok(ConvertedTable {
                    written: ok.written,
                    restarts,
                    demoted: ok.demoted,
                    all_strings: plan.all_strings,
                    encoding: ok.encoding,
                    replacements: ok.replacements,
                    utf8_valid_multibyte: ok.utf8_valid_multibyte,
                    utf8_invalid: ok.utf8_invalid,
                    blank_rows: ok.blank_rows,
                    blank_dropped: ok.blank_dropped,
                    padded_rows: ok.padded_rows,
                    blob_paths,
                    stale_paths,
                });
            }
            Err(RunEnd::Conflict(c)) if !plan.all_strings => {
                restarts += 1;
                if restarts >= MAX_RESTARTS {
                    plan.all_strings = true;
                } else {
                    plan.text_columns.push(c.column);
                }
            }
            Err(RunEnd::Reencode(right)) if plan.force.is_none() => {
                plan.force = Some(right);
            }
            Err(RunEnd::Conflict(_)) | Err(RunEnd::Reencode(_)) => {
                let e = ConvertError::Cast("a run that cannot conflict did".into());
                failed = Some(e.into());
                break;
            }
            Err(RunEnd::Failed(e)) => {
                failed = Some(e);
                break;
            }
        }
    }
    let error = failed.unwrap_or_else(|| {
        ConvertError::Cast("the conversion did not settle after its restarts".into()).into()
    });
    Err(TableFailure {
        error,
        blob_paths: control.paths_of(table_idx),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tabular_prepare::csv::{open_csv, BATCH_ROWS};
    use crate::tabular_prepare::manifest::ColumnType;
    use arrow_array::{
        Array, BooleanArray, Date32Array, Float64Array, Int64Array, StringArray,
        TimestampMicrosecondArray,
    };
    use bytes::Bytes;
    use futures::stream;
    use std::io::{Cursor, Read};
    use tokio_util::sync::CancellationToken;

    fn typed(csv: &str) -> (InferredSchema, TypedBatches) {
        let o = open_csv(Cursor::new(csv.as_bytes().to_vec()), None).unwrap();
        let schema = o.schema.clone();
        (schema.clone(), TypedBatches::new(o.batches, schema))
    }

    #[test]
    fn cells_become_typed_values_with_empty_as_null() {
        let csv = "i,f,b,s,d,t\n\
                   7,1.5,TRUE,00123,2020-02-29,2020-01-05T10:20:30\n\
                   -8,2,false,x,1999-12-31,2020-01-05 10:20:30.123456\n\
                   ,,,,,\n";
        let (_, mut it) = typed(csv);
        let b = it.next().unwrap().unwrap();
        assert!(it.next().is_none());
        let col = |i: usize| b.column(i).clone();
        let ints = col(0);
        let ints = ints.as_any().downcast_ref::<Int64Array>().unwrap();
        assert_eq!(
            (ints.value(0), ints.value(1), ints.is_null(2)),
            (7, -8, true)
        );
        let f = col(1);
        let f = f.as_any().downcast_ref::<Float64Array>().unwrap();
        assert_eq!((f.value(0), f.value(1)), (1.5, 2.0));
        let bo = col(2);
        let bo = bo.as_any().downcast_ref::<BooleanArray>().unwrap();
        assert_eq!(
            (bo.value(0), bo.value(1), bo.is_null(2)),
            (true, false, true)
        );
        let s = col(3);
        let s = s.as_any().downcast_ref::<StringArray>().unwrap();
        assert_eq!((s.value(0), s.value(1), s.is_null(2)), ("00123", "x", true));
        let d = col(4);
        let d = d.as_any().downcast_ref::<Date32Array>().unwrap();
        assert_eq!(
            (d.value(0), d.value(1), d.is_null(2)),
            (18_321, 10_956, true)
        );
        let t = col(5);
        let t = t
            .as_any()
            .downcast_ref::<TimestampMicrosecondArray>()
            .unwrap();
        assert_eq!(t.value(0), 1_578_219_630_000_000);
        assert_eq!(t.value(1), 1_578_219_630_123_456);
        assert!(t.is_null(2));
    }

    #[test]
    fn the_output_schema_is_the_effective_schema() {
        let (schema, mut it) = typed("a,b\n1,x\n");
        let b = it.next().unwrap().unwrap();
        assert_eq!(b.schema(), schema.arrow_schema());
    }

    /// An int column in the sample, then `late` far past it.
    fn late_value(late: &str) -> String {
        let mut csv = String::from("id,v\n");
        for i in 0..crate::tabular_prepare::infer::INFERENCE_ROWS + 20_000 {
            csv.push_str(&format!("{i},{i}\n"));
        }
        csv.push_str(&format!("1,{late}\n"));
        csv
    }

    #[test]
    fn a_late_value_that_does_not_fit_is_a_conflict_with_its_column_and_row() {
        let csv = late_value("N/A");
        let rows_before = crate::tabular_prepare::infer::INFERENCE_ROWS + 20_000;
        let (schema, it) = typed(&csv);
        assert_eq!(schema.columns[1].column_type, ColumnType::Int);
        let mut rows = 0;
        let mut conflict = None;
        for item in it {
            match item {
                Ok(b) => rows += b.num_rows(),
                Err(e) => conflict = Some(e),
            }
        }
        // Whole batches before the offending one arrive typed.
        assert!(
            rows > 0 && rows <= rows_before && rows % BATCH_ROWS == 0,
            "{rows}"
        );
        match conflict.unwrap() {
            ConvertError::Conflict(c) => {
                assert_eq!(
                    c,
                    TypeConflict {
                        column: 1,
                        row: rows_before as u64
                    }
                );
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn with_two_conflicts_in_one_batch_the_earlier_row_is_reported() {
        let mut csv = String::from("a,b\n");
        for i in 0..crate::tabular_prepare::infer::INFERENCE_ROWS {
            csv.push_str(&format!("{i},{i}\n"));
        }
        // Column a goes wrong at the second late row, column b at the first.
        csv.push_str("1,N/A\nN/A,2\n");
        let (_, it) = typed(&csv);
        let conflict = it.filter_map(Result::err).next().unwrap();
        match conflict {
            ConvertError::Conflict(c) => assert_eq!(
                c,
                TypeConflict {
                    column: 1,
                    row: crate::tabular_prepare::infer::INFERENCE_ROWS as u64
                }
            ),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn values_a_number_parser_would_accept_are_still_conflicts() {
        // Each of these parses as a number or a bool somewhere, and none is
        // what the inference accepted: reading them would change the value.
        for late in ["007", "+5", "1e3", "1.5"] {
            let (_, it) = typed(&late_value(late));
            let err = it.filter_map(Result::err).next();
            assert!(
                matches!(err, Some(ConvertError::Conflict(_))),
                "{late}: {err:?}"
            );
        }
    }

    #[test]
    fn a_column_forced_to_text_keeps_every_value_verbatim() {
        let csv = late_value("N/A");
        let o = open_csv(Cursor::new(csv.into_bytes()), None).unwrap();
        let mut schema = o.schema.clone();
        schema.columns[1].column_type = ColumnType::String;
        let mut seen = 0usize;
        for b in TypedBatches::new(o.batches, schema) {
            let b = b.unwrap();
            let v = b.column(1).as_any().downcast_ref::<StringArray>().unwrap();
            for r in 0..b.num_rows() {
                // Every value is the text of its row, verbatim; the late one is "N/A".
                let want = if seen + r == crate::tabular_prepare::infer::INFERENCE_ROWS + 20_000 {
                    "N/A".to_string()
                } else {
                    (seen + r).to_string()
                };
                assert_eq!(v.value(r), want, "row {}", seen + r);
            }
            seen += b.num_rows();
        }
        assert_eq!(seen, crate::tabular_prepare::infer::INFERENCE_ROWS + 20_001);
    }

    #[test]
    fn a_failure_of_the_source_comes_out_typed_and_ends_the_iteration() {
        let mut csv = String::from("a,b\n");
        for _ in 0..crate::tabular_prepare::infer::INFERENCE_ROWS + 100 {
            csv.push_str("1,2\n");
        }
        csv.push_str(&"x".repeat(crate::tabular_prepare::scan::MAX_RECORD_BYTES + 10));
        let (_, it) = typed(&csv);
        let items: Vec<_> = it.take(1000).collect();
        assert!(matches!(
            items.last().unwrap(),
            Err(ConvertError::Csv(
                crate::tabular_prepare::csv::CsvError::RecordTooLong { .. }
            ))
        ));
        assert_eq!(items.iter().filter(|i| i.is_err()).count(), 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_storage_stream_reads_as_a_blocking_reader_in_order() {
        let chunks: Vec<Result<Bytes, StorageError>> = (0..1000)
            .map(|i| Ok(Bytes::from(format!("{i},row\n"))))
            .collect();
        let expected: String = (0..1000).map(|i| format!("{i},row\n")).collect();
        let reader = stream_reader(Box::pin(stream::iter(chunks)), CancellationToken::new());
        let text = tokio::task::spawn_blocking(move || {
            let mut out = String::new();
            let mut r = reader;
            r.read_to_string(&mut out).map(|_| out)
        })
        .await
        .unwrap()
        .unwrap();
        assert_eq!(text, expected);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_storage_error_in_the_stream_is_an_io_error_with_its_text() {
        let chunks: Vec<Result<Bytes, StorageError>> = vec![
            Ok(Bytes::from_static(b"a,b\n1,2\n")),
            Err(StorageError::BackendUnavailable("connection reset".into())),
        ];
        let reader = stream_reader(Box::pin(stream::iter(chunks)), CancellationToken::new());
        let err = tokio::task::spawn_blocking(move || {
            let mut r = reader;
            r.read_to_end(&mut Vec::new()).unwrap_err()
        })
        .await
        .unwrap();
        assert!(err.to_string().contains("connection reset"), "{err}");
    }

    // ---- the restart loop ----

    use crate::tabular_prepare::csv::{CsvError, Encoding};
    use crate::tabular_prepare::infer::INFERENCE_ROWS;
    use crate::tabular_prepare::manifest::ManifestError;
    use crate::tabular_prepare::part_sink::fake::MemorySink;
    use crate::tabular_prepare::writer::{WriterConfig, WriterError};
    use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// A CSV in memory that counts how often it is opened.
    struct MemSource {
        bytes: Vec<u8>,
        opens: AtomicUsize,
    }

    impl MemSource {
        fn new(bytes: Vec<u8>) -> Self {
            Self {
                bytes,
                opens: AtomicUsize::new(0),
            }
        }
        fn opens(&self) -> usize {
            self.opens.load(Ordering::SeqCst)
        }
    }

    #[async_trait::async_trait]
    impl CsvSource for MemSource {
        async fn open(
            &self,
            _cancel: &CancellationToken,
        ) -> Result<Box<dyn Read + Send>, ConvertError> {
            self.opens.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(Cursor::new(self.bytes.clone())))
        }
    }

    async fn convert(
        src: &MemSource,
        sink: &Arc<MemorySink>,
        cfg: WriterConfig,
    ) -> Result<ConvertedTable, TableFailure> {
        convert_csv_table(src, sink.clone(), 0, cfg).await
    }

    fn types(t: &ConvertedTable) -> Vec<ColumnType> {
        t.written.columns.iter().map(|c| c.column_type).collect()
    }

    /// Reads the column `col` of every part, as text.
    fn read_column_text(sink: &MemorySink, parts: u32, col: usize) -> Vec<Option<String>> {
        let mut out = Vec::new();
        for p in 0..parts {
            let bytes = sink.get(&format!("t0/part-{p:05}.parquet")).unwrap();
            for b in ParquetRecordBatchReaderBuilder::try_new(bytes)
                .unwrap()
                .build()
                .unwrap()
            {
                let b = b.unwrap();
                let a = b
                    .column(col)
                    .as_any()
                    .downcast_ref::<StringArray>()
                    .unwrap();
                out.extend((0..a.len()).map(|i| (!a.is_null(i)).then(|| a.value(i).to_string())));
            }
        }
        out
    }

    /// `n` rows of "id,v" where v is an integer, then the given late rows.
    fn int_csv(n: usize, late: &str) -> Vec<u8> {
        let mut csv = String::from("id,v\n");
        for i in 0..n {
            csv.push_str(&format!("{i},{i}\n"));
        }
        csv.push_str(late);
        csv.into_bytes()
    }

    #[tokio::test]
    async fn a_clean_file_is_converted_in_one_pass() {
        let src = MemSource::new(b"a,b,c\n1,x,2020-01-01\n2,y,2020-01-02\n".to_vec());
        let sink = Arc::new(MemorySink::default());
        let t = convert(&src, &sink, WriterConfig::default()).await.unwrap();
        assert_eq!(
            (t.written.rows, t.written.parts, t.restarts, src.opens()),
            (2, 1, 0, 1)
        );
        assert_eq!(
            types(&t),
            vec![ColumnType::Int, ColumnType::String, ColumnType::Date]
        );
        assert!(t.demoted.is_empty() && !t.all_strings);
        assert_eq!(t.blob_paths, vec!["t0/part-00000.parquet".to_string()]);
    }

    #[tokio::test]
    async fn a_million_ints_and_one_late_na_become_a_text_column_with_every_value_verbatim() {
        let src = MemSource::new(int_csv(1_000_000, "1000000,N/A\n"));
        let sink = Arc::new(MemorySink::default());
        let t = convert(&src, &sink, WriterConfig::default()).await.unwrap();
        assert_eq!((t.restarts, src.opens()), (1, 2));
        assert_eq!(t.written.rows, 1_000_001);
        // The exposed schema shows the fallback type; the other column keeps its own.
        assert_eq!(types(&t), vec![ColumnType::Int, ColumnType::String]);
        assert_eq!(t.demoted, vec!["v".to_string()]);
        let v = read_column_text(&sink, t.written.parts, 1);
        assert_eq!(v.len(), 1_000_001);
        assert_eq!(v[0].as_deref(), Some("0"));
        assert_eq!(v[999_999].as_deref(), Some("999999"));
        assert_eq!(v[1_000_000].as_deref(), Some("N/A"));
    }

    /// Five integer columns; column k goes wrong at its own late row.
    fn five_columns(bad: &[(usize, usize)]) -> Vec<u8> {
        let rows = INFERENCE_ROWS + 3000;
        let mut csv = String::from("c0,c1,c2,c3,c4\n");
        for r in 0..rows {
            let cells: Vec<String> = (0..5)
                .map(
                    |c| match bad.iter().find(|(col, row)| *col == c && *row == r) {
                        Some(_) => "N/A".to_string(),
                        None => r.to_string(),
                    },
                )
                .collect();
            csv.push_str(&cells.join(","));
            csv.push('\n');
        }
        csv.into_bytes()
    }

    #[tokio::test]
    async fn after_three_restarts_every_column_is_text() {
        let late = INFERENCE_ROWS + 100;
        // Three columns fail one after another, at rows far enough apart to
        // land in different batches, so each run finds one.
        let src = MemSource::new(five_columns(&[
            (0, late),
            (1, late + 1000),
            (2, late + 2000),
        ]));
        let sink = Arc::new(MemorySink::default());
        let t = convert(&src, &sink, WriterConfig::default()).await.unwrap();
        assert_eq!((t.restarts, src.opens()), (3, 4));
        assert!(t.all_strings);
        assert!(
            types(&t).iter().all(|t| *t == ColumnType::String),
            "{:?}",
            types(&t)
        );
        let c3 = read_column_text(&sink, t.written.parts, 3);
        assert_eq!(c3[late].as_deref(), Some(late.to_string().as_str()));
    }

    #[tokio::test]
    async fn two_conflicting_columns_demote_only_those_two() {
        let late = INFERENCE_ROWS + 100;
        let src = MemSource::new(five_columns(&[(1, late), (3, late + 1000)]));
        let sink = Arc::new(MemorySink::default());
        let t = convert(&src, &sink, WriterConfig::default()).await.unwrap();
        assert_eq!((t.restarts, src.opens()), (2, 3));
        assert!(!t.all_strings);
        let expect = [
            ColumnType::Int,
            ColumnType::String,
            ColumnType::Int,
            ColumnType::String,
            ColumnType::Int,
        ];
        assert_eq!(types(&t), expect);
        assert_eq!(t.demoted, vec!["c1".to_string(), "c3".to_string()]);
    }

    #[tokio::test]
    async fn restarts_overwrite_the_same_keys_and_the_tracked_paths_are_the_union() {
        // Parts of 5,000 rows; the conflict is past the first part, so the
        // first run leaves part 0 behind and the second one replaces it.
        let cfg = WriterConfig {
            max_rows: 5000,
            max_bytes: usize::MAX,
        };
        let src = MemSource::new(int_csv(INFERENCE_ROWS + 3000, "1,N/A\n"));
        let sink = Arc::new(MemorySink::default());
        let t = convert(&src, &sink, cfg).await.unwrap();
        assert_eq!(t.restarts, 1);
        let puts = sink.paths();
        let first = "t0/part-00000.parquet";
        assert_eq!(puts.iter().filter(|p| *p == first).count(), 2, "{puts:?}");
        // The tracked paths are each key once, and cover every put.
        let mut unique = puts.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(t.blob_paths, unique);
        // The surviving part 0 is the text version.
        let bytes = sink.get(first).unwrap();
        let schema = ParquetRecordBatchReaderBuilder::try_new(bytes)
            .unwrap()
            .schema()
            .clone();
        assert_eq!(schema.field(1).data_type(), &arrow_schema::DataType::Utf8);
    }

    #[tokio::test]
    async fn a_sink_failure_reports_every_path_tried_across_the_restart() {
        let cfg = WriterConfig {
            max_rows: 5000,
            max_bytes: usize::MAX,
        };
        let src = MemSource::new(int_csv(INFERENCE_ROWS + 3000, "1,N/A\n"));
        // Run 1 puts part 0; run 2 puts part 0 again (put 2) and then fails on put 3.
        let sink = Arc::new(MemorySink::failing_from(2));
        let failure = convert(&src, &sink, cfg).await.unwrap_err();
        assert!(
            matches!(failure.error, TableError::Writer(WriterError::Sink(_))),
            "{:?}",
            failure.error
        );
        assert_eq!(
            failure.blob_paths.first().map(String::as_str),
            Some("t0/part-00000.parquet")
        );
        assert!(failure.blob_paths.len() >= 2, "{:?}", failure.blob_paths);
    }

    #[tokio::test]
    async fn invalid_utf8_after_the_sample_restarts_once_as_windows_1252() {
        let mut csv = b"a,b\n".to_vec();
        for _ in 0..(crate::tabular_prepare::csv::SNIFF_BYTES / 4 + 100) {
            csv.extend_from_slice(b"1,x\n");
        }
        csv.extend_from_slice(b"2,caf\xE9\n");
        let src = MemSource::new(csv);
        let sink = Arc::new(MemorySink::default());
        let t = convert(&src, &sink, WriterConfig::default()).await.unwrap();
        assert_eq!(
            (t.encoding, src.opens(), t.restarts),
            (Encoding::Windows1252, 2, 0)
        );
        let b = read_column_text(&sink, t.written.parts, 1);
        assert_eq!(b.last().unwrap().as_deref(), Some("café"));
    }

    #[tokio::test]
    async fn the_number_of_runs_is_bounded() {
        // Three type restarts and one encoding restart is the most any file costs.
        let late = INFERENCE_ROWS + 100;
        let mut csv = five_columns(&[(0, late), (1, late + 1000), (2, late + 2000)]);
        for _ in 0..(crate::tabular_prepare::csv::SNIFF_BYTES / 4) {
            csv.extend_from_slice(b"1,2,3,4,5\n");
        }
        csv.extend_from_slice(b"1,2,3,4,caf\xE9\n");
        let src = MemSource::new(csv);
        let sink = Arc::new(MemorySink::default());
        let t = convert(&src, &sink, WriterConfig::default()).await.unwrap();
        assert_eq!(src.opens(), 5);
        assert_eq!((t.restarts, t.encoding), (3, Encoding::Windows1252));
        assert_eq!(src.opens(), MAX_RESTARTS + 2);
    }

    #[tokio::test]
    async fn a_source_error_that_no_restart_fixes_fails_at_once() {
        let mut csv = b"a,b\n".to_vec();
        for _ in 0..INFERENCE_ROWS + 100 {
            csv.extend_from_slice(b"1,2\n");
        }
        csv.extend(std::iter::repeat_n(
            b'x',
            crate::tabular_prepare::scan::MAX_RECORD_BYTES + 10,
        ));
        let src = MemSource::new(csv);
        let sink = Arc::new(MemorySink::default());
        let failure = convert(&src, &sink, WriterConfig::default())
            .await
            .unwrap_err();
        assert!(matches!(
            failure.error,
            TableError::Convert(ConvertError::Csv(CsvError::RecordTooLong { .. }))
        ));
        assert_eq!(src.opens(), 1);
    }

    #[tokio::test]
    async fn an_empty_file_and_an_unopenable_source_fail_without_paths() {
        let sink = Arc::new(MemorySink::default());
        let src = MemSource::new(Vec::new());
        let failure = convert(&src, &sink, WriterConfig::default())
            .await
            .unwrap_err();
        assert!(matches!(
            failure.error,
            TableError::Convert(ConvertError::Csv(CsvError::Empty))
        ));
        assert!(failure.blob_paths.is_empty() && sink.paths().is_empty());

        struct Broken;
        #[async_trait::async_trait]
        impl CsvSource for Broken {
            async fn open(
                &self,
                _cancel: &CancellationToken,
            ) -> Result<Box<dyn Read + Send>, ConvertError> {
                Err(ConvertError::Csv(CsvError::Io("gone".into())))
            }
        }
        let failure = convert_csv_table(&Broken, sink.clone(), 0, WriterConfig::default())
            .await
            .unwrap_err();
        assert!(failure.blob_paths.is_empty());
    }

    #[tokio::test]
    async fn a_reader_that_dies_midway_is_a_failure_never_a_short_table() {
        /// Serves a header and rows, then panics well past the sample, as a bug
        /// in a source would.
        struct Dies {
            data: Vec<u8>,
            sent: usize,
        }
        impl Read for Dies {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                if self.sent >= 2_000_000 {
                    panic!("source blew up");
                }
                let n = buf.len().min(self.data.len() - self.sent).min(16 * 1024);
                buf[..n].copy_from_slice(&self.data[self.sent..self.sent + n]);
                self.sent += n;
                Ok(n)
            }
        }
        struct DiesSource;
        #[async_trait::async_trait]
        impl CsvSource for DiesSource {
            async fn open(
                &self,
                _cancel: &CancellationToken,
            ) -> Result<Box<dyn Read + Send>, ConvertError> {
                Ok(Box::new(Dies {
                    data: int_csv(250_000, ""),
                    sent: 0,
                }))
            }
        }
        let sink = Arc::new(MemorySink::default());
        let failure = convert_csv_table(&DiesSource, sink.clone(), 0, WriterConfig::default())
            .await
            .unwrap_err();
        assert!(
            matches!(
                failure.error,
                TableError::Convert(ConvertError::ReaderPanicked)
            ),
            "{}",
            failure.error
        );
    }

    #[tokio::test]
    async fn a_sink_failure_reports_every_path_tried_and_no_table() {
        let cfg = WriterConfig {
            max_rows: 1000,
            max_bytes: usize::MAX,
        };
        let src = MemSource::new(int_csv(3500, ""));
        // Part 0 is put, part 1 is tried and fails.
        let sink = Arc::new(MemorySink::failing_from(1));
        let failure = convert(&src, &sink, cfg).await.unwrap_err();
        assert!(
            matches!(failure.error, TableError::Writer(WriterError::Sink(_))),
            "{:?}",
            failure.error
        );
        assert_eq!(
            failure.blob_paths,
            vec!["t0/part-00000.parquet", "t0/part-00001.parquet"]
        );
    }

    #[tokio::test]
    async fn a_failure_stops_the_reader_instead_of_letting_it_finish_the_file() {
        /// Three million rows, generated as they are read, and a count of what was pulled.
        struct Endless {
            next: u64,
            pending: Vec<u8>,
            pulled: Arc<AtomicUsize>,
        }
        impl Read for Endless {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                while self.pending.len() < buf.len().min(32 * 1024) && self.next < 3_000_000 {
                    self.pending
                        .extend_from_slice(format!("{},x\n", self.next).as_bytes());
                    self.next += 1;
                }
                let n = buf.len().min(self.pending.len());
                buf[..n].copy_from_slice(&self.pending[..n]);
                self.pending.drain(..n);
                self.pulled.fetch_add(n, Ordering::SeqCst);
                Ok(n)
            }
        }
        struct EndlessSource(Arc<AtomicUsize>);
        #[async_trait::async_trait]
        impl CsvSource for EndlessSource {
            async fn open(
                &self,
                _cancel: &CancellationToken,
            ) -> Result<Box<dyn Read + Send>, ConvertError> {
                Ok(Box::new(Endless {
                    next: 0,
                    pending: b"id,v\n".to_vec(),
                    pulled: self.0.clone(),
                }))
            }
        }
        let pulled = Arc::new(AtomicUsize::new(0));
        let cfg = WriterConfig {
            max_rows: 1000,
            max_bytes: usize::MAX,
        };
        // The first part put fails.
        let sink = Arc::new(MemorySink::failing_from(0));
        let failure = convert_csv_table(&EndlessSource(pulled.clone()), sink, 0, cfg)
            .await
            .unwrap_err();
        assert!(matches!(failure.error, TableError::Writer(_)));
        // About 40 MB were on offer; the reader stopped within a few batches.
        assert!(
            pulled.load(Ordering::SeqCst) < 8 * 1024 * 1024,
            "{}",
            pulled.load(Ordering::SeqCst)
        );
    }

    #[tokio::test]
    async fn a_stray_byte_in_a_utf8_file_is_replaced_and_reported_not_reread_as_1252() {
        let mut csv = b"a,b\n".to_vec();
        for _ in 0..crate::tabular_prepare::csv::SNIFF_BYTES / 8 {
            csv.extend_from_slice("é,ñ\n".as_bytes());
        }
        csv.extend_from_slice(b"x\xFF,y\n");
        let src = MemSource::new(csv);
        let sink = Arc::new(MemorySink::default());
        let t = convert(&src, &sink, WriterConfig::default()).await.unwrap();
        assert_eq!(
            (t.encoding, t.replacements, src.opens()),
            (Encoding::Utf8, 1, 1)
        );
        let a = read_column_text(&sink, t.written.parts, 0);
        assert_eq!(a.first().unwrap().as_deref(), Some("é"));
        assert_eq!(a.last().unwrap().as_deref(), Some("x\u{FFFD}"));
    }

    #[tokio::test]
    async fn a_reader_that_panics_before_the_header_is_a_typed_error_too() {
        struct Boom;
        impl Read for Boom {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                panic!("source blew up at once");
            }
        }
        struct BoomSource;
        #[async_trait::async_trait]
        impl CsvSource for BoomSource {
            async fn open(
                &self,
                _cancel: &CancellationToken,
            ) -> Result<Box<dyn Read + Send>, ConvertError> {
                Ok(Box::new(Boom))
            }
        }
        let sink = Arc::new(MemorySink::default());
        let failure = convert_csv_table(&BoomSource, sink, 0, WriterConfig::default())
            .await
            .unwrap_err();
        assert!(matches!(
            failure.error,
            TableError::Convert(ConvertError::ReaderPanicked)
        ));
    }

    // ---- no value changes silently ----

    fn column_of(late: &str, kind_rows: &[&str]) -> Result<RecordBatch, ConvertError> {
        let mut csv = String::from("v\n");
        for r in kind_rows {
            csv.push_str(&format!("{r}\n"));
        }
        // Keep the type decided by the sample, then put the odd value far
        // past it.
        for _ in 0..crate::tabular_prepare::infer::INFERENCE_ROWS {
            csv.push_str(&format!("{}\n", kind_rows[0]));
        }
        csv.push_str(&format!("{late}\n"));
        let (_, it) = typed(&csv);
        let mut last = None;
        for b in it {
            last = Some(b?);
        }
        Ok(last.unwrap())
    }

    #[test]
    fn a_timestamp_the_inference_accepts_is_never_stored_as_null() {
        // `u32::parse` accepts a plus sign, so "+9:00:00" looked like a clock.
        for late in [
            "2020-01-05 +9:00:00",
            "2020-01-05 09:+0:00",
            "2020-01-05T09:00:+1",
        ] {
            let r = column_of(late, &["2020-01-05 10:20:30"]);
            assert!(
                matches!(r, Err(ConvertError::Conflict(_))),
                "{late} was not refused: {r:?}"
            );
        }
    }

    #[test]
    fn a_float_that_underflows_is_not_stored_as_zero() {
        for late in ["1e-400", "4.9e-325", "-1e-999", "2e-310"] {
            let r = column_of(late, &["1.5"]);
            assert!(
                matches!(r, Err(ConvertError::Conflict(_))),
                "{late} was not refused: {r:?}"
            );
        }
        // Zero written as zero is fine, so is a tiny normal number.
        for ok in ["0.0", "0e0", "-0.0", "1e-300", "2.5e-100"] {
            let b = column_of(ok, &["1.5"]).unwrap();
            let v = b.column(0).as_any().downcast_ref::<Float64Array>().unwrap();
            let got = v.value(v.len() - 1);
            assert_eq!(got.to_bits(), ok.parse::<f64>().unwrap().to_bits(), "{ok}");
        }
    }

    /// Strings that look like the type they are fed to, and mutations of them.
    fn corpus(seed: u64, templates: &[&str], alphabet: &str, n: usize) -> Vec<String> {
        let mut x = seed;
        let mut next = move || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        let chars: Vec<char> = alphabet.chars().collect();
        let mut out = Vec::new();
        for _ in 0..n {
            let t = templates[(next() % templates.len() as u64) as usize];
            let s: String = t
                .chars()
                .map(|c| {
                    if c == '#' {
                        chars[(next() % chars.len() as u64) as usize]
                    } else {
                        c
                    }
                })
                .collect();
            out.push(s);
        }
        out
    }

    /// Either the type refuses the string, or the stored value is the value of
    /// the literal: never null, never another number.
    fn check(s: &str, t: ColumnType) {
        use arrow_array::{BooleanArray, Date32Array, TimestampMicrosecondArray};
        use chrono::{NaiveDate, NaiveDateTime};
        let text = StringArray::from(vec![Some(s)]);
        let dt = crate::tabular_prepare::infer::arrow_type_of(t);
        let Ok(col) = type_column(&text, t, &dt) else {
            return;
        };
        assert!(!col.is_null(0), "{s:?} stored as null in {t:?}");
        let any = col.as_any();
        match t {
            ColumnType::Int => {
                let v = any.downcast_ref::<Int64Array>().unwrap().value(0);
                assert_eq!(v, s.parse::<i64>().unwrap(), "{s:?}");
            }
            ColumnType::Float => {
                let v = any.downcast_ref::<Float64Array>().unwrap().value(0);
                let want = s.parse::<f64>().unwrap();
                assert!(
                    v.is_finite() && (v != 0.0 || want == 0.0),
                    "{s:?} became {v}"
                );
                assert_eq!(v.to_bits(), want.to_bits(), "{s:?}");
            }
            ColumnType::Bool => {
                let v = any.downcast_ref::<BooleanArray>().unwrap().value(0);
                assert_eq!(v, s.eq_ignore_ascii_case("true"), "{s:?}");
            }
            ColumnType::Date => {
                let v = any.downcast_ref::<Date32Array>().unwrap().value(0);
                let d = NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap();
                assert_eq!(
                    v,
                    d.signed_duration_since(NaiveDate::from_ymd_opt(1970, 1, 1).unwrap())
                        .num_days() as i32,
                    "{s:?}"
                );
            }
            ColumnType::Timestamp => {
                let v = any
                    .downcast_ref::<TimestampMicrosecondArray>()
                    .unwrap()
                    .value(0);
                let fmt = if s.as_bytes()[10] == b'T' {
                    "%Y-%m-%dT%H:%M:%S%.f"
                } else {
                    "%Y-%m-%d %H:%M:%S%.f"
                };
                let d = NaiveDateTime::parse_from_str(s, fmt).unwrap();
                assert_eq!(v, d.and_utc().timestamp_micros(), "{s:?}");
            }
            ColumnType::String => {}
        }
    }

    #[test]
    fn near_miss_strings_are_either_refused_or_stored_as_their_own_value() {
        let digits = "0123456789+-.eE: TtZ_x٣";
        let sets: [(ColumnType, &[&str], &str); 5] = [
            (
                ColumnType::Int,
                &["#", "##", "###", "-##", "#####", "0#", "-0", "#.#", "+#"],
                digits,
            ),
            (
                ColumnType::Float,
                &[
                    "#.#",
                    "#.##",
                    "-#.#e#",
                    "#e-##",
                    "##.#e##",
                    "#e+#",
                    "0.#",
                    "-0.#",
                    "#.#e-###",
                    "##.##e-###",
                ],
                "0123456789-+eE.",
            ),
            (
                ColumnType::Bool,
                &["true", "TRUE", "tRuE", "false", "FALSE", "#rue", "fals#"],
                "tfTF01e",
            ),
            (
                ColumnType::Date,
                &[
                    "20##-##-##",
                    "2020-0#-#1",
                    "####-##-##",
                    "2020-#-##",
                    "2020-02-#9",
                ],
                "0123456789-+ ",
            ),
            (
                ColumnType::Timestamp,
                &[
                    "2020-01-05 ##:##:##",
                    "2020-01-05T##:##:##",
                    "2020-01-05 ##:##:##.#",
                    "2020-01-05 #:##:##",
                    "2020-01-05 ##:#+:##",
                    "2020-02-29 2#:5#:5#.######",
                    "2020-01-05 ##:##:##Z",
                ],
                "0123456789+- :.TZ",
            ),
        ];
        let mut accepted = 0;
        for (t, templates, alphabet) in sets {
            for s in corpus(0x9E37_79B9_7F4A_7C15, templates, alphabet, 40_000) {
                if crate::tabular_prepare::infer::cell_fits(&s, t) {
                    accepted += 1;
                }
                check(&s, t);
            }
        }
        assert!(
            accepted > 20_000,
            "the corpus hardly exercised the types: {accepted}"
        );
    }

    // ---- what the result reports ----

    #[tokio::test]
    async fn the_result_reports_blank_lines_and_padded_rows() {
        let sink = Arc::new(MemorySink::default());
        // One column: blank lines inside are null rows, trailing ones dropped.
        let src = MemSource::new(b"name\nann\n\nbob\n\n\ncy\n\n".to_vec());
        let t = convert(&src, &sink, WriterConfig::default()).await.unwrap();
        assert_eq!(
            (t.written.rows, t.blank_rows, t.blank_dropped, t.padded_rows),
            (6, 3, 1, 0)
        );
        // Several columns: blank lines dropped, short rows padded, both counted.
        let src = MemSource::new(b"a,b,c\n1,2,3\n\n4,5\n6\n7,8,9\n\n".to_vec());
        let sink = Arc::new(MemorySink::default());
        let t = convert(&src, &sink, WriterConfig::default()).await.unwrap();
        assert_eq!(
            (t.written.rows, t.blank_rows, t.blank_dropped, t.padded_rows),
            (4, 0, 2, 2)
        );
    }

    #[tokio::test]
    async fn demoted_lists_every_demoted_column_also_when_all_are_text() {
        let late = INFERENCE_ROWS + 100;
        let src = MemSource::new(five_columns(&[
            (0, late),
            (1, late + 1000),
            (2, late + 2000),
        ]));
        let sink = Arc::new(MemorySink::default());
        let t = convert(&src, &sink, WriterConfig::default()).await.unwrap();
        assert!(t.all_strings);
        assert_eq!(t.demoted, vec!["c0", "c1", "c2", "c3", "c4"]);
        // A column that was text from the start is not "demoted".
        let src = MemSource::new(b"a,b\nx,1\ny,2\n".to_vec());
        let sink = Arc::new(MemorySink::default());
        assert!(convert(&src, &sink, WriterConfig::default())
            .await
            .unwrap()
            .demoted
            .is_empty());
    }

    #[tokio::test]
    async fn a_table_list_that_cannot_fit_the_registry_fails_before_the_conversion() {
        // 3,000 columns of an endless file: even with the shortest type and
        // zero bytes the table list is over 64 KiB, so nothing is converted.
        struct Endless(Arc<AtomicUsize>, bool);
        impl Read for Endless {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                let header = (0..3000)
                    .map(|i| format!("c{i}"))
                    .collect::<Vec<_>>()
                    .join(",");
                let row = vec!["1"; 3000].join(",");
                let line = if self.1 {
                    format!("{row}\n")
                } else {
                    self.1 = true;
                    format!("{header}\n")
                };
                let n = line.len().min(buf.len());
                buf[..n].copy_from_slice(&line.as_bytes()[..n]);
                self.0.fetch_add(n, Ordering::SeqCst);
                Ok(n)
            }
        }
        struct EndlessSource(Arc<AtomicUsize>);
        #[async_trait::async_trait]
        impl CsvSource for EndlessSource {
            async fn open(
                &self,
                _cancel: &CancellationToken,
            ) -> Result<Box<dyn Read + Send>, ConvertError> {
                Ok(Box::new(Endless(self.0.clone(), false)))
            }
        }
        let pulled = Arc::new(AtomicUsize::new(0));
        let sink = Arc::new(MemorySink::default());
        let failure = convert_csv_table(
            &EndlessSource(pulled.clone()),
            sink.clone(),
            0,
            WriterConfig::default(),
        )
        .await
        .unwrap_err();
        assert!(matches!(
            failure.error,
            TableError::Convert(ConvertError::Manifest(
                ManifestError::ManifestTooLarge { .. }
            ))
        ));
        assert!(sink.paths().is_empty());
        // Only the sample was read, not the (endless) file.
        assert!(
            pulled.load(Ordering::SeqCst) < 40 * 1024 * 1024,
            "{}",
            pulled.load(Ordering::SeqCst)
        );
    }

    #[tokio::test]
    async fn a_wide_table_that_fits_is_converted() {
        let header = (0..700)
            .map(|i| format!("c{i}"))
            .collect::<Vec<_>>()
            .join(",");
        let row = vec!["1"; 700].join(",");
        let src = MemSource::new(format!("{header}\n{row}\n{row}\n").into_bytes());
        let sink = Arc::new(MemorySink::default());
        let t = convert(&src, &sink, WriterConfig::default()).await.unwrap();
        assert_eq!(t.written.columns.len(), 700);
    }

    // ---- cancellation ----

    use tokio::sync::Notify;

    /// How long a test waits for something that must happen by itself. Only a
    /// guard so a regression fails instead of hanging; nothing is asserted
    /// about timing.
    const GUARD: std::time::Duration = std::time::Duration::from_secs(60);

    /// A CSV of `rows` short rows. It counts what it was asked for, can cancel
    /// a control once enough was pulled, and says when the reader is dropped
    /// (the blocking thread letting go of it).
    struct Counted {
        rows: usize,
        pulled: Arc<AtomicUsize>,
        cancel_after: Option<(usize, Arc<ConvertControl>)>,
        gone: Arc<Notify>,
    }

    impl Counted {
        fn new(rows: usize) -> Self {
            Self {
                rows,
                pulled: Arc::new(AtomicUsize::new(0)),
                cancel_after: None,
                gone: Arc::new(Notify::new()),
            }
        }
    }

    struct CountedReader {
        next: usize,
        rows: usize,
        pending: Vec<u8>,
        pulled: Arc<AtomicUsize>,
        cancel_after: Option<(usize, Arc<ConvertControl>)>,
        gone: Arc<Notify>,
    }

    impl Read for CountedReader {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            while self.pending.len() < buf.len().min(16 * 1024) && self.next < self.rows {
                self.pending
                    .extend_from_slice(format!("{},x\n", self.next).as_bytes());
                self.next += 1;
            }
            let n = buf.len().min(self.pending.len());
            buf[..n].copy_from_slice(&self.pending[..n]);
            self.pending.drain(..n);
            let total = self.pulled.fetch_add(n, Ordering::SeqCst) + n;
            if let Some((limit, control)) = &self.cancel_after {
                if total >= *limit {
                    control.cancel();
                }
            }
            Ok(n)
        }
    }

    impl Drop for CountedReader {
        fn drop(&mut self) {
            self.gone.notify_one();
        }
    }

    #[async_trait::async_trait]
    impl CsvSource for Counted {
        async fn open(
            &self,
            _cancel: &CancellationToken,
        ) -> Result<Box<dyn Read + Send>, ConvertError> {
            Ok(Box::new(CountedReader {
                next: 0,
                rows: self.rows,
                pending: b"id,v\n".to_vec(),
                pulled: self.pulled.clone(),
                cancel_after: self.cancel_after.clone(),
                gone: self.gone.clone(),
            }))
        }
    }

    /// Stores into a memory sink and says once `after` puts have been made.
    struct Signals {
        inner: MemorySink,
        after: usize,
        seen: AtomicUsize,
        reached: Arc<Notify>,
    }

    #[async_trait::async_trait]
    impl PartSink for Signals {
        async fn put(&self, path: &str, data: bytes::Bytes) -> Result<(), SinkError> {
            self.inner.put(path, data).await?;
            if self.seen.fetch_add(1, Ordering::SeqCst) + 1 == self.after {
                self.reached.notify_one();
            }
            Ok(())
        }
    }

    #[tokio::test]
    async fn dropping_the_future_keeps_the_keys_already_put_and_stops_the_reader() {
        let src = Counted::new(30_000_000);
        let reached = Arc::new(Notify::new());
        let sink = Arc::new(Signals {
            inner: MemorySink::default(),
            after: 3,
            seen: AtomicUsize::new(0),
            reached: reached.clone(),
        });
        let control = ConvertControl::new();
        let cfg = WriterConfig {
            max_rows: 2000,
            max_bytes: usize::MAX,
        };
        let run = convert_csv_table_with(&src, sink.clone(), 0, cfg, &control);
        // Dropped the moment the third part has been put: `select!` drops the
        // conversion when the other branch is ready.
        tokio::select! {
            _ = run => panic!("30 million rows cannot be done yet"),
            _ = reached.notified() => {}
        }
        let kept = control.paths();
        assert!(kept.len() >= 3, "{kept:?}");
        assert_eq!(kept[0], "t0/part-00000.parquet");
        // Every key that reached the sink is in the set the caller still holds.
        for p in sink.inner.paths() {
            assert!(kept.contains(&p), "{p} lost");
        }
        // The blocking reader lets go: it is dropped, long before the end.
        tokio::time::timeout(GUARD, src.gone.notified())
            .await
            .expect("the reader went on after the future was dropped");
        assert!(src.pulled.load(Ordering::SeqCst) < 100 * 1024 * 1024);
    }

    #[tokio::test]
    async fn a_key_is_recorded_before_its_put_so_a_put_that_never_returns_is_still_known() {
        struct Hangs(Arc<Notify>);
        #[async_trait::async_trait]
        impl PartSink for Hangs {
            async fn put(&self, _: &str, _: bytes::Bytes) -> Result<(), SinkError> {
                self.0.notify_one();
                std::future::pending().await
            }
        }
        let src = MemSource::new(int_csv(3000, ""));
        let control = ConvertControl::new();
        let cfg = WriterConfig {
            max_rows: 1000,
            max_bytes: usize::MAX,
        };
        let started = Arc::new(Notify::new());
        let run = convert_csv_table_with(&src, Arc::new(Hangs(started.clone())), 0, cfg, &control);
        tokio::select! {
            _ = run => panic!("a put that never returns cannot finish"),
            _ = started.notified() => {}
        }
        assert_eq!(control.paths(), vec!["t0/part-00000.parquet"]);
    }

    #[tokio::test]
    async fn cancelling_the_control_ends_the_run_with_a_typed_error() {
        let control = ConvertControl::new();
        let mut src = Counted::new(30_000_000);
        // The reader cancels the control itself once it has served 1 MiB.
        src.cancel_after = Some((1024 * 1024, control.clone()));
        let sink = Arc::new(MemorySink::default());
        let failure = convert_csv_table_with(&src, sink, 0, WriterConfig::default(), &control)
            .await
            .unwrap_err();
        assert!(
            matches!(
                failure.error,
                TableError::Convert(ConvertError::Csv(CsvError::Cancelled))
            ),
            "{:?}",
            failure.error
        );
        assert!(src.pulled.load(Ordering::SeqCst) < 200 * 1024 * 1024);
    }

    /// A stream that gives `first` and then never yields again. It says when it
    /// is first left waiting, and when it is dropped.
    struct Stalls {
        first: Option<bytes::Bytes>,
        waiting: Arc<Notify>,
        dropped: Arc<Notify>,
    }

    impl Stalls {
        fn new(first: &'static [u8]) -> Self {
            Self {
                first: Some(bytes::Bytes::from_static(first)),
                waiting: Arc::new(Notify::new()),
                dropped: Arc::new(Notify::new()),
            }
        }
    }

    impl Stream for Stalls {
        type Item = Result<bytes::Bytes, StorageError>;
        fn poll_next(
            mut self: Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Option<Self::Item>> {
            match self.first.take() {
                Some(chunk) => std::task::Poll::Ready(Some(Ok(chunk))),
                None => {
                    self.waiting.notify_one();
                    std::task::Poll::Pending
                }
            }
        }
    }

    impl Drop for Stalls {
        fn drop(&mut self) {
            self.dropped.notify_one();
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_stalled_storage_read_is_interrupted_by_the_token() {
        // A stream that never yields: a blocking read on it cannot return on
        // its own.
        let token = CancellationToken::new();
        let stalls = Stalls::new(b"");
        let waiting = stalls.waiting.clone();
        let mut reader = stream_reader(Box::pin(stalls), token.clone());
        let handle = tokio::task::spawn_blocking(move || reader.read(&mut [0u8; 16]));
        // Cancel only once the read is parked on the stream.
        tokio::time::timeout(GUARD, waiting.notified())
            .await
            .unwrap();
        token.cancel();
        let r = tokio::time::timeout(GUARD, handle)
            .await
            .expect("the blocked read was not interrupted")
            .unwrap();
        assert_eq!(CsvError::from_io(r.unwrap_err()), CsvError::Cancelled);
    }

    struct StallSource(Arc<Notify>, Arc<Notify>);

    #[async_trait::async_trait]
    impl CsvSource for StallSource {
        async fn open(
            &self,
            cancel: &CancellationToken,
        ) -> Result<Box<dyn Read + Send>, ConvertError> {
            let mut stalls = Stalls::new(b"id,v\n1,a\n2,b\n");
            stalls.waiting = self.0.clone();
            stalls.dropped = self.1.clone();
            Ok(Box::new(stream_reader(Box::pin(stalls), cancel.clone())))
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn dropping_the_future_frees_a_reader_stalled_on_the_storage_stream() {
        let (waiting, dropped) = (Arc::new(Notify::new()), Arc::new(Notify::new()));
        let source = StallSource(waiting.clone(), dropped.clone());
        let sink = Arc::new(MemorySink::default());
        let run = convert_csv_table(&source, sink, 0, WriterConfig::default());
        // Dropped once the reader is parked on the stalled stream.
        tokio::select! {
            _ = run => panic!("a stalled stream cannot finish"),
            _ = waiting.notified() => {}
        }
        // The blocking thread was woken and ended; the stream is gone.
        tokio::time::timeout(GUARD, dropped.notified())
            .await
            .expect("the blocking reader is still stuck on the stalled stream");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_failing_put_is_reported_even_when_the_reader_is_stalled_on_storage() {
        use std::task::Poll;
        /// Many rows, then a stream that never yields again.
        struct ThenStalls(bool);
        impl Stream for ThenStalls {
            type Item = Result<bytes::Bytes, StorageError>;
            fn poll_next(
                mut self: Pin<&mut Self>,
                _: &mut std::task::Context<'_>,
            ) -> Poll<Option<Self::Item>> {
                if !self.0 {
                    self.0 = true;
                    // More rows than the 1 MiB sniff, the sample and one batch hold, so a batch is written (and
                    // its put fails) while the reader waits for more.
                    let mut rows = String::from("id,v\n");
                    for i in 0..11_000 {
                        rows.push_str(&format!("{i},{}\n", "a".repeat(100)));
                    }
                    return Poll::Ready(Some(Ok(bytes::Bytes::from(rows))));
                }
                Poll::Pending
            }
        }
        struct StallSource;
        #[async_trait::async_trait]
        impl CsvSource for StallSource {
            async fn open(
                &self,
                cancel: &CancellationToken,
            ) -> Result<Box<dyn Read + Send>, ConvertError> {
                let stream: ByteStream = Box::pin(ThenStalls(false));
                Ok(Box::new(stream_reader(stream, cancel.clone())))
            }
        }
        // Parts of two rows, and the first put fails: the writer fails while
        // the blocking reader is parked on the stream.
        let cfg = WriterConfig {
            max_rows: 2,
            max_bytes: usize::MAX,
        };
        let sink = Arc::new(MemorySink::failing_from(0));
        let run = convert_csv_table(&StallSource, sink, 0, cfg);
        // The timeout is only a guard: the failure must come back by itself.
        let failure = tokio::time::timeout(std::time::Duration::from_secs(20), run)
            .await
            .expect("the failure was never reported: the reader was not cancelled")
            .unwrap_err();
        assert!(
            matches!(failure.error, TableError::Writer(WriterError::Sink(_))),
            "{:?}",
            failure.error
        );
    }

    // ---- the encoding is decided from the whole file ----

    /// Rows "1,<word>" repeated, `n` of them.
    fn rows_of_word(word: impl AsRef<[u8]>, n: usize) -> Vec<u8> {
        let mut row = b"1,".to_vec();
        row.extend_from_slice(word.as_ref());
        row.push(b'\n');
        row.repeat(n)
    }

    #[tokio::test]
    async fn ascii_and_one_stray_byte_in_the_sample_then_utf8_later_is_read_as_utf8() {
        // The sample alone says "0 valid, 1 invalid": not plausibly UTF-8. The
        // rest of the file is UTF-8 with accents.
        let mut csv = b"a,b\n1,plain\n1,x\xFFy\n".to_vec();
        csv.extend(rows_of_word(
            "ascii",
            crate::tabular_prepare::csv::SNIFF_BYTES / 8,
        ));
        csv.extend(rows_of_word("café ñandú", 2000));
        let src = MemSource::new(csv);
        let sink = Arc::new(MemorySink::default());
        let t = convert(&src, &sink, WriterConfig::default()).await.unwrap();
        assert_eq!(
            (t.encoding, t.replacements, src.opens()),
            (Encoding::Utf8, 1, 2)
        );
        let b = read_column_text(&sink, t.written.parts, 1);
        assert_eq!(b[1].as_deref(), Some("x\u{FFFD}y"));
        assert_eq!(b.last().unwrap().as_deref(), Some("café ñandú"));
        assert!(b.iter().flatten().all(|v| !v.contains('Ã')), "mojibake");
    }

    #[tokio::test]
    async fn one_valid_and_one_invalid_sequence_is_utf8_with_one_replacement() {
        let src = MemSource::new(b"a,b\n1,caf\xC3\xA9\n1,bad\xFFbyte\n".to_vec());
        let sink = Arc::new(MemorySink::default());
        let t = convert(&src, &sink, WriterConfig::default()).await.unwrap();
        assert_eq!((t.encoding, t.replacements), (Encoding::Utf8, 1));
        let b = read_column_text(&sink, t.written.parts, 1);
        assert_eq!(b[0].as_deref(), Some("café"));
    }

    #[tokio::test]
    async fn a_real_windows_1252_file_stays_windows_1252_whether_accents_come_early_or_late() {
        for late in [false, true] {
            let mut csv = b"a,b\n".to_vec();
            if late {
                csv.extend(rows_of_word(
                    "ascii",
                    crate::tabular_prepare::csv::SNIFF_BYTES / 8,
                ));
            }
            csv.extend(rows_of_word(b"caf\xE9 \xF1and\xFA", 300));
            let src = MemSource::new(csv);
            let sink = Arc::new(MemorySink::default());
            let t = convert(&src, &sink, WriterConfig::default()).await.unwrap();
            assert_eq!(
                (t.encoding, t.replacements),
                (Encoding::Windows1252, 0),
                "late {late}"
            );
            let b = read_column_text(&sink, t.written.parts, 1);
            assert_eq!(b.last().unwrap().as_deref(), Some("café ñandú"));
        }
    }

    #[tokio::test]
    async fn a_utf8_file_with_one_binary_looking_cell_keeps_its_accents() {
        let mut csv = b"a,b\n".to_vec();
        csv.extend(rows_of_word("café ñandú", 500));
        csv.extend_from_slice(b"1,\x01\x02\xFF\xFE\x80\n");
        csv.extend(rows_of_word("naïve", 500));
        let src = MemSource::new(csv);
        let sink = Arc::new(MemorySink::default());
        let t = convert(&src, &sink, WriterConfig::default()).await.unwrap();
        assert_eq!((t.encoding, src.opens()), (Encoding::Utf8, 1));
        assert_eq!(t.replacements, 3); // 0xFF, 0xFE, 0x80
        let b = read_column_text(&sink, t.written.parts, 1);
        assert_eq!(b[0].as_deref(), Some("café ñandú"));
        assert_eq!(b.last().unwrap().as_deref(), Some("naïve"));
    }

    #[tokio::test]
    async fn the_utf8_evidence_is_reported_whatever_the_encoding_chosen() {
        // UTF-8 with one stray byte: one valid sequence, one invalid, replaced.
        let src = MemSource::new(b"a,b\n1,caf\xC3\xA9\n1,bad\xFFbyte\n".to_vec());
        let sink = Arc::new(MemorySink::default());
        let t = convert(&src, &sink, WriterConfig::default()).await.unwrap();
        assert_eq!(
            (
                t.encoding,
                t.replacements,
                t.utf8_valid_multibyte,
                t.utf8_invalid
            ),
            (Encoding::Utf8, 1, 1, 1)
        );
        // Windows-1252: nothing replaced, and the counts say what the bytes
        // looked like as UTF-8 (three single accented bytes in each row).
        let src = MemSource::new(rows_of_word(b"caf\xE9 \xF1and\xFA", 300));
        let t = convert(&src, &sink, WriterConfig::default()).await.unwrap();
        assert_eq!(
            (
                t.encoding,
                t.replacements,
                t.utf8_valid_multibyte,
                t.utf8_invalid
            ),
            (Encoding::Windows1252, 0, 0, 900)
        );
    }

    #[tokio::test]
    async fn the_two_known_misreads_leave_their_evidence_in_the_result() {
        let sink = Arc::new(MemorySink::default());
        // A Windows-1252 file whose accents are an uppercase letter followed by
        // a byte 0x80-0xBF ("Ã©" is the bytes C3 A9): valid UTF-8, read as UTF-8.
        let src = MemSource::new(rows_of_word(b"Caf\xC3\xA9", 50));
        let t = convert(&src, &sink, WriterConfig::default()).await.unwrap();
        assert_eq!(
            (t.encoding, t.utf8_valid_multibyte, t.utf8_invalid),
            (Encoding::Utf8, 50, 0)
        );
        // A UTF-8 file with many sequences cut mid-character (each lost its
        // last byte): more invalid than half the valid ones, read as 1252.
        let mut csv = rows_of_word("caf\u{e9}", 10);
        csv.splice(0..0, b"a,b\n".iter().copied());
        csv.extend(rows_of_word(b"\xE2\x82 cut", 100));
        let src = MemSource::new(csv);
        let t = convert(&src, &sink, WriterConfig::default()).await.unwrap();
        assert_eq!(
            (t.encoding, t.utf8_valid_multibyte, t.utf8_invalid),
            (Encoding::Windows1252, 10, 100)
        );
    }

    #[tokio::test]
    async fn limits_that_would_read_nothing_fail_the_conversion_instead_of_succeeding_empty() {
        use crate::tabular_prepare::csv::ReadLimits;
        let d = ReadLimits::default();
        for limits in [
            ReadLimits { batch_rows: 0, ..d },
            ReadLimits {
                batch_cells: 1,
                ..d
            },
            ReadLimits {
                batch_bytes: crate::tabular_prepare::csv::BATCH_BYTES + 1,
                ..d
            },
        ] {
            let src = MemSource::new(b"a,b\n1,2\n3,4\n".to_vec());
            let sink = Arc::new(MemorySink::default());
            let r = convert_csv_table_limits(
                &src,
                sink,
                0,
                WriterConfig::default(),
                &ConvertControl::new(),
                &limits,
            )
            .await;
            let Err(failure) = r else {
                panic!("an invalid limit is never a success")
            };
            assert!(
                matches!(
                    failure.error,
                    TableError::Convert(ConvertError::Csv(CsvError::InvalidLimits(_)))
                ),
                "{:?}",
                failure.error
            );
        }
    }

    #[tokio::test]
    async fn a_control_shared_by_two_tables_never_lists_one_tables_parts_as_the_others() {
        let sink = Arc::new(MemorySink::default());
        let control = ConvertControl::new();
        let cfg = WriterConfig {
            max_rows: 1000,
            max_bytes: usize::MAX,
        };
        let a = MemSource::new(int_csv(2500, ""));
        let t0 = convert_csv_table_with(&a, sink.clone(), 0, cfg, &control)
            .await
            .unwrap();
        // Table 1 restarts and ends with fewer parts, so it has stale keys of its own.
        struct Changing(AtomicUsize);
        #[async_trait::async_trait]
        impl CsvSource for Changing {
            async fn open(
                &self,
                _c: &CancellationToken,
            ) -> Result<Box<dyn Read + Send>, ConvertError> {
                let n = self.0.fetch_add(1, Ordering::SeqCst);
                let bytes = if n == 0 {
                    int_csv(INFERENCE_ROWS + 3000, "1,N/A\n")
                } else {
                    int_csv(1500, "")
                };
                Ok(Box::new(Cursor::new(bytes)))
            }
        }
        let t1 = convert_csv_table_with(
            &Changing(AtomicUsize::new(0)),
            sink.clone(),
            1,
            cfg,
            &control,
        )
        .await
        .unwrap();
        // Each result lists only its own table's keys, whatever else the control saw.
        assert!(
            t0.blob_paths.iter().all(|p| p.starts_with("t0/")),
            "{:?}",
            t0.blob_paths
        );
        assert!(
            t1.blob_paths.iter().all(|p| p.starts_with("t1/")),
            "{:?}",
            t1.blob_paths
        );
        assert!(t1.stale_paths.iter().all(|p| p.starts_with("t1/")));
        assert!(!t1.stale_paths.is_empty());
        // Deleting what table 1 reports as stale cannot touch table 0's live parts.
        for p in &t1.stale_paths {
            assert!(!t0.live_paths(0).contains(p));
        }
        // The control still knows everything, for a caller that wants it.
        assert_eq!(
            control.paths().len(),
            t0.blob_paths.len() + t1.blob_paths.len()
        );
        // A failed table reports its own keys too.
        let bad = MemSource::new(int_csv(INFERENCE_ROWS + 3000, "1,N/A\n"));
        let failing = Arc::new(MemorySink::failing_from(0));
        let f = convert_csv_table_with(&bad, failing, 2, cfg, &control)
            .await
            .unwrap_err();
        assert!(
            f.blob_paths.iter().all(|p| p.starts_with("t2/")),
            "{:?}",
            f.blob_paths
        );
    }

    // ---- the early check on the table list ----

    fn grid(cols: usize, rows: usize) -> Vec<u8> {
        let header = (0..cols)
            .map(|i| format!("c{i}"))
            .collect::<Vec<_>>()
            .join(",");
        let row = vec!["12345"; cols].join(",");
        format!("{header}\n{}", format!("{row}\n").repeat(rows)).into_bytes()
    }

    #[tokio::test]
    async fn the_early_minimum_never_exceeds_the_table_list_that_is_written() {
        use crate::tabular_prepare::csv::open_csv;
        use crate::tabular_prepare::manifest::Manifest;
        for (cols, rows) in [(1usize, 5usize), (3, 40), (40, 2000), (200, 300)] {
            let bytes = grid(cols, rows);
            let o = open_csv(Cursor::new(bytes.clone()), None).unwrap();
            let pairs: Vec<(&str, ColumnType)> = o
                .schema
                .columns
                .iter()
                .map(|c| (c.name.as_str(), c.column_type))
                .collect();
            let min = min_tables_json_len(&pairs, o.sample_rows as u64);
            let sink = Arc::new(MemorySink::default());
            let t = convert(&MemSource::new(bytes), &sink, WriterConfig::default())
                .await
                .unwrap();
            let real = Manifest::new(vec![crate::tabular_prepare::manifest::TableInfo {
                name: "t".into(),
                rows: t.written.rows,
                parts: t.written.parts,
                columns: t.written.columns.clone(),
            }])
            .tables_json()
            .unwrap()
            .len();
            assert!(min <= real, "{cols} columns: {min} > {real}");
        }
    }

    #[tokio::test]
    async fn a_table_the_looser_check_let_through_is_now_refused_after_the_sample() {
        use crate::tabular_prepare::csv::SAMPLE_MAX_CELLS;
        // Columns for which the minimum with zero rows fits the cap but the one
        // with the rows of the sample does not.
        let names = |n: usize| (0..n).map(|i| format!("c{i}")).collect::<Vec<_>>();
        let min_for = |n: usize, rows: u64| {
            let ns = names(n);
            let pairs: Vec<(&str, ColumnType)> =
                ns.iter().map(|s| (s.as_str(), ColumnType::Int)).collect();
            min_tables_json_len(&pairs, rows)
        };
        let cols = (300..2000)
            .find(|&n| {
                let rows = (SAMPLE_MAX_CELLS / n).min(10_000) as u64;
                min_for(n, 0) <= TABLES_JSON_MAX_BYTES && min_for(n, rows) > TABLES_JSON_MAX_BYTES
            })
            .expect("a width between the two bounds");
        // An endless file of that width: the run must stop after the sample.
        struct EndlessSource(usize, Arc<AtomicUsize>);
        #[async_trait::async_trait]
        impl CsvSource for EndlessSource {
            async fn open(
                &self,
                _c: &CancellationToken,
            ) -> Result<Box<dyn Read + Send>, ConvertError> {
                // The header once, then the same row for ever.
                let all = grid(self.0, 1);
                let (header, row) = all.split_at(all.iter().position(|b| *b == b'\n').unwrap() + 1);
                struct Loop {
                    head: Vec<u8>,
                    row: Vec<u8>,
                    sent: usize,
                    pulled: Arc<AtomicUsize>,
                }
                impl Read for Loop {
                    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                        let total = self.head.len();
                        let n = if self.sent < total {
                            let n = buf.len().min(total - self.sent);
                            buf[..n].copy_from_slice(&self.head[self.sent..self.sent + n]);
                            n
                        } else {
                            let at = (self.sent - total) % self.row.len();
                            let n = buf.len().min(self.row.len() - at);
                            buf[..n].copy_from_slice(&self.row[at..at + n]);
                            n
                        };
                        self.sent += n;
                        self.pulled.fetch_add(n, Ordering::SeqCst);
                        Ok(n)
                    }
                }
                Ok(Box::new(Loop {
                    head: header.to_vec(),
                    row: row.to_vec(),
                    sent: 0,
                    pulled: self.1.clone(),
                }))
            }
        }
        let pulled = Arc::new(AtomicUsize::new(0));
        let sink = Arc::new(MemorySink::default());
        let failure = convert_csv_table(
            &EndlessSource(cols, pulled.clone()),
            sink.clone(),
            0,
            WriterConfig::default(),
        )
        .await
        .unwrap_err();
        assert!(
            matches!(
                failure.error,
                TableError::Convert(ConvertError::Manifest(
                    ManifestError::ManifestTooLarge { .. }
                ))
            ),
            "{:?}",
            failure.error
        );
        assert!(sink.paths().is_empty());
        assert!(pulled.load(Ordering::SeqCst) < 40 * 1024 * 1024);
    }
}
