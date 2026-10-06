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
/// Converts one CSV into the parts of table `table_idx`, in one read of the
/// source. Memory is bounded by the reader's batch limits, a channel of
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
/// Converts one CSV into the parts of table `table_idx`, in one read of the
/// source (a late conflict or a wrong encoding guess is a failure here; the
/// restarts are the next slice). Every part key is recorded in `control` before
/// its put, and nothing is reported as written on a failure. Crate private:
/// only tests move the boundaries, and the limits are validated.
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
    let plan = RunPlan {
        force: None,
        text_columns: Vec::new(),
        all_strings: false,
        limits: *limits,
    };
    let failed = |error: TableError| TableFailure {
        error,
        blob_paths: control.paths_of(table_idx),
    };
    match run(source, &sink, &cancel, table_idx, cfg, &plan).await {
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
            Ok(ConvertedTable {
                written: ok.written,
                restarts: 0,
                demoted: ok.demoted,
                all_strings: false,
                encoding: ok.encoding,
                replacements: ok.replacements,
                utf8_valid_multibyte: ok.utf8_valid_multibyte,
                utf8_invalid: ok.utf8_invalid,
                blank_rows: ok.blank_rows,
                blank_dropped: ok.blank_dropped,
                padded_rows: ok.padded_rows,
                blob_paths,
                stale_paths,
            })
        }
        Err(RunEnd::Conflict(c)) => Err(failed(ConvertError::Conflict(c).into())),
        Err(RunEnd::Reencode(right)) => Err(failed(
            ConvertError::Cast(format!(
                "the whole file says {right:?} was the right encoding"
            ))
            .into(),
        )),
        Err(RunEnd::Failed(e)) => Err(failed(e)),
    }
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

    // ---- cancellation ----
}
