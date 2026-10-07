//! Reading a CSV for preparation (dark behind `COLMENA_LARGE_TABULAR`).
//!
//! [`prepare_input`] turns a blocking byte source into clean UTF-8 text and
//! finds the delimiter, holding no more than one sample ([`SNIFF_BYTES`]) in
//! memory and never reading the rest ahead of the consumer: the file can be
//! far larger than memory.

use crate::tabular_prepare::infer::{InferredSchema, SchemaInferer, INFERENCE_ROWS};
use crate::tabular_prepare::manifest::{clean_name, MAX_COLUMNS, MAX_COLUMN_NAME_CHARS};
use crate::tabular_prepare::scan::{RecordScanner, ScanStats, MAX_RECORD_BYTES};
use arrow_array::builder::StringBuilder;
use arrow_array::{ArrayRef, RecordBatch};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{self, Cursor, Read};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use thiserror::Error;

/// Bytes read up front to detect the encoding and the delimiter.
pub const SNIFF_BYTES: usize = 1024 * 1024;

/// Rows per batch handed on, for files of a few columns.
pub const BATCH_ROWS: usize = 8192;

/// Cells (rows times columns) per batch at most: a wide file gets shorter
/// batches so a batch stays small whatever the shape.
pub const BATCH_CELLS: usize = 1_000_000;

/// Bytes of text per batch at most. A record is at most `MAX_RECORD_BYTES`, so
/// one always fits; a batch ends before the record that would pass this.
pub const BATCH_BYTES: usize = 8 * 1024 * 1024;

const _: () = assert!(MAX_RECORD_BYTES <= BATCH_BYTES);

/// Bytes of text kept to decide the types. The sample stops at this size even
/// if it has fewer than [`INFERENCE_ROWS`] rows (very long rows).
pub const SAMPLE_MAX_BYTES: usize = 16 * 1024 * 1024;

/// Cells kept to decide the types: a wide file is sampled over fewer rows (a
/// parsed record costs about eight bytes of bookkeeping per field).
pub const SAMPLE_MAX_CELLS: usize = 2_000_000;

/// The limits of the reader. The defaults are the constants above; a test can
/// lower them to move the sample, batch and part boundaries within a small
/// file. They are crate-private on purpose: no production caller can pass
/// other limits, and [`ReadLimits::validate`] refuses a value that would make
/// the reader read nothing or hold more than the constants allow.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ReadLimits {
    /// Rows the types are decided from.
    pub inference_rows: usize,
    pub sample_bytes: usize,
    pub sample_cells: usize,
    pub batch_rows: usize,
    pub batch_cells: usize,
    pub batch_bytes: usize,
}

impl ReadLimits {
    /// Every limit is at least one and at most its production constant, and a
    /// batch holds at least one row of `columns` cells. A violation is a typed
    /// error, so no limit can yield an empty table that looks successful.
    pub(crate) fn validate(&self, columns: usize) -> Result<(), CsvError> {
        let bad = |name: &str, v: usize, max: usize| {
            Err(CsvError::InvalidLimits(format!(
                "{name} is {v}, it must be between 1 and {max}"
            )))
        };
        for (name, v, max) in [
            ("inference_rows", self.inference_rows, INFERENCE_ROWS),
            ("sample_bytes", self.sample_bytes, SAMPLE_MAX_BYTES),
            ("sample_cells", self.sample_cells, SAMPLE_MAX_CELLS),
            ("batch_rows", self.batch_rows, BATCH_ROWS),
            ("batch_cells", self.batch_cells, BATCH_CELLS),
            ("batch_bytes", self.batch_bytes, BATCH_BYTES),
        ] {
            if v == 0 || v > max {
                return bad(name, v, max);
            }
        }
        if self.batch_cells < columns {
            return Err(CsvError::InvalidLimits(format!(
                "batch_cells is {}, a row of this file has {columns} cells",
                self.batch_cells
            )));
        }
        Ok(())
    }
}

impl Default for ReadLimits {
    fn default() -> Self {
        Self {
            inference_rows: INFERENCE_ROWS,
            sample_bytes: SAMPLE_MAX_BYTES,
            sample_cells: SAMPLE_MAX_CELLS,
            batch_rows: BATCH_ROWS,
            batch_cells: BATCH_CELLS,
            batch_bytes: BATCH_BYTES,
        }
    }
}

/// Records the delimiter vote looks at.
const SNIFF_RECORDS: usize = 50;

const CHUNK: usize = 16 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CsvError {
    #[error("the file is empty")]
    Empty,
    #[error("unsupported encoding: {0}")]
    UnsupportedEncoding(String),
    #[error("record {record} is longer than {limit} bytes")]
    RecordTooLong { record: u64, limit: usize },
    #[error("the file has more than {limit} columns")]
    TooManyColumns { limit: usize },
    #[error("malformed CSV: {0}")]
    Parse(String),
    #[error("the conversion was cancelled")]
    Cancelled,
    #[error("read error: {0}")]
    Io(String),
    #[error("invalid reader limits: {0}")]
    InvalidLimits(String),
}

impl CsvError {
    /// The typed error inside an `io::Error` produced by this module's
    /// readers, or `Io` for any other.
    pub fn from_io(e: io::Error) -> Self {
        Self::typed(&e)
    }

    fn typed(e: &io::Error) -> Self {
        match e.get_ref().and_then(|i| i.downcast_ref::<CsvError>()) {
            Some(inner) => inner.clone(),
            None => CsvError::Io(e.to_string()),
        }
    }

    fn from_csv(e: ::csv::Error) -> Self {
        match e.kind() {
            ::csv::ErrorKind::Io(io) => Self::typed(io),
            _ => CsvError::Parse(e.to_string()),
        }
    }

    pub(crate) fn into_io(self) -> io::Error {
        io::Error::new(io::ErrorKind::InvalidData, self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    Utf8,
    Windows1252,
}

/// What decoding found, shared with the caller. Counts are of sequences:
/// `valid_multibyte` is how many non-ASCII UTF-8 characters were read and
/// `invalid` how many invalid sequences were replaced by U+FFFD. Both are final
/// once the reader is exhausted, and both stay zero for Windows-1252.
#[derive(Debug, Default)]
pub struct DecodeStats {
    pub(crate) valid_multibyte: AtomicU64,
    pub(crate) invalid: AtomicU64,
}

impl DecodeStats {
    pub fn valid_multibyte(&self) -> u64 {
        self.valid_multibyte.load(Ordering::Relaxed)
    }
    pub fn invalid(&self) -> u64 {
        self.invalid.load(Ordering::Relaxed)
    }
    /// Whether the content is plausibly UTF-8: no invalid sequence, or at least
    /// half as many valid multibyte sequences as invalid ones. Real Windows-1252
    /// text has almost no valid UTF-8 multibyte sequences (an accented letter is
    /// one byte that is not a valid start of one), so its valid count is near
    /// zero against many invalid ones; UTF-8 with stray bytes has the opposite.
    /// Ties and near-ties go to UTF-8: a counted replacement character is
    /// visible, mojibake is not.
    pub fn plausibly_utf8(&self) -> bool {
        self.invalid() == 0 || self.valid_multibyte().saturating_mul(2) >= self.invalid()
    }
}

/// Counts the multibyte sequences and invalid sequences of `bytes`. An
/// incomplete sequence at the very end is invalid only when `ends` is set (the
/// input really ends there, rather than being cut by a limit).
fn count_utf8(bytes: &[u8], ends: bool) -> (u64, u64) {
    let (mut valid, mut invalid) = (0u64, 0u64);
    let mut rest = bytes;
    loop {
        match std::str::from_utf8(rest) {
            Ok(s) => {
                valid += s.bytes().filter(|b| *b >= 0xC0).count() as u64;
                return (valid, invalid);
            }
            Err(e) => {
                let ok = &rest[..e.valid_up_to()];
                valid += ok.iter().filter(|b| **b >= 0xC0).count() as u64;
                match e.error_len() {
                    Some(n) => {
                        invalid += 1;
                        rest = &rest[e.valid_up_to() + n..];
                    }
                    None => {
                        invalid += u64::from(ends);
                        return (valid, invalid);
                    }
                }
            }
        }
    }
}

/// A source ready to be parsed: UTF-8, no byte order mark, record-bounded.
pub struct Prepared {
    pub reader: Box<dyn Read + Send>,
    pub delimiter: u8,
    pub encoding: Encoding,
    /// What the record scanner saw: records and blank lines (see `scan.rs`).
    pub stats: Arc<ScanStats>,
    /// What decoding found (see [`DecodeStats`]).
    pub decode: Arc<DecodeStats>,
}

/// Fills `buf` as far as the source allows.
fn read_up_to(input: &mut impl Read, limit: usize) -> io::Result<Vec<u8>> {
    let mut buf = vec![0u8; limit];
    let mut len = 0;
    while len < limit {
        match input.read(&mut buf[len..]) {
            Ok(0) => break,
            Ok(n) => len += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    buf.truncate(len);
    Ok(buf)
}

/// Detects the encoding and delimiter from the first [`SNIFF_BYTES`] and
/// returns a reader over clean UTF-8.
///
/// The encoding is UTF-8 when the sample is plausibly UTF-8 (see
/// [`DecodeStats::plausibly_utf8`]), otherwise Windows-1252; `force` overrides
/// the guess. In UTF-8 an invalid sequence is never an error and never decides
/// the encoding on its own: it becomes U+FFFD and is counted in
/// [`Prepared::decode`], so a stray byte in a UTF-8 file costs one character and
/// is reported, instead of turning every accent into mojibake. A file whose
/// sample was ASCII but whose later content is not plausibly UTF-8 is detected
/// from the final counts, and the caller reads it again with `force`.
pub fn prepare_input<R: Read + Send + 'static>(
    mut input: R,
    force: Option<Encoding>,
) -> Result<Prepared, CsvError> {
    let mut head = read_up_to(&mut input, SNIFF_BYTES).map_err(CsvError::from_io)?;
    let truncated = head.len() == SNIFF_BYTES;
    if head.starts_with(&[0xFF, 0xFE]) || head.starts_with(&[0xFE, 0xFF]) {
        return Err(CsvError::UnsupportedEncoding("UTF-16".into()));
    }
    if head.starts_with(&[0xEF, 0xBB, 0xBF]) {
        head.drain(..3);
    }
    if head.contains(&0) {
        return Err(CsvError::UnsupportedEncoding(
            "binary data, or UTF-16 without a byte order mark".into(),
        ));
    }
    if !truncated && head.iter().all(u8::is_ascii_whitespace) {
        return Err(CsvError::Empty);
    }
    let (valid, invalid) = count_utf8(&head, !truncated);
    let head_stats = DecodeStats::default();
    head_stats.valid_multibyte.store(valid, Ordering::Relaxed);
    head_stats.invalid.store(invalid, Ordering::Relaxed);
    let encoding = force.unwrap_or(if head_stats.plausibly_utf8() {
        Encoding::Utf8
    } else {
        Encoding::Windows1252
    });
    let decode = Arc::new(DecodeStats::default());
    let sniffed = sniff_delimiter(&head, truncated);
    let delimiter = sniffed.unwrap_or(b',');
    let source = Cursor::new(head).chain(input);
    let text: Box<dyn Read + Send> = match encoding {
        Encoding::Utf8 => Box::new(Utf8Lossy::new(source, decode.clone())),
        // The bytes are also counted as UTF-8, so the whole-file decision can be
        // taken from either encoding.
        Encoding::Windows1252 => Box::new(Transcoder::new(Utf8Count::new(source, decode.clone()))),
    };
    // No delimiter evidence means one column, where a blank line is an empty cell.
    let stats = Arc::new(ScanStats::default());
    let scanner = RecordScanner::new(text, delimiter, sniffed.is_none(), stats.clone());
    Ok(Prepared {
        reader: Box::new(scanner),
        delimiter,
        encoding,
        stats,
        decode,
    })
}

/// The candidate whose records agree most on a field count above one (ties go
/// to the larger count, then to the order comma, semicolon, tab, pipe). The
/// record cut by the sample limit is left out. `None` when no candidate gives
/// two columns: the file is read as one column, with a comma.
fn sniff_delimiter(sample: &[u8], truncated: bool) -> Option<u8> {
    let mut best: Option<((usize, usize), u8)> = None;
    for d in [b',', b';', b'\t', b'|'] {
        let mut reader = ::csv::ReaderBuilder::new()
            .delimiter(d)
            .has_headers(false)
            .flexible(true)
            .from_reader(sample);
        let mut counts: Vec<usize> = Vec::new();
        for record in reader.byte_records().take(SNIFF_RECORDS + 1) {
            match record {
                Ok(r) => counts.push(r.len()),
                Err(_) => break,
            }
        }
        if counts.len() > SNIFF_RECORDS {
            counts.truncate(SNIFF_RECORDS);
        } else if truncated {
            counts.pop();
        }
        let mut votes: HashMap<usize, usize> = HashMap::new();
        for c in counts.into_iter().filter(|c| *c >= 2) {
            *votes.entry(c).or_default() += 1;
        }
        if let Some((k, n)) = votes.into_iter().max_by_key(|(k, n)| (*n, *k)) {
            if best.is_none_or(|(score, _)| (n, k) > score) {
                best = Some(((n, k), d));
            }
        }
    }
    best.map(|(_, d)| d)
}

/// Passes UTF-8 through and replaces each invalid sequence with U+FFFD,
/// counting both in [`DecodeStats`]. A sequence cut by a read boundary is held
/// until the next read; one cut by the end of the stream is invalid.
struct Utf8Lossy<R> {
    inner: R,
    stats: Arc<DecodeStats>,
    carry: Vec<u8>,
    input: Vec<u8>,
    output: Vec<u8>,
    pos: usize,
    finished: bool,
}

impl<R> Utf8Lossy<R> {
    fn new(inner: R, stats: Arc<DecodeStats>) -> Self {
        Self {
            inner,
            stats,
            carry: Vec::new(),
            input: vec![0; CHUNK],
            output: Vec::new(),
            pos: 0,
            finished: false,
        }
    }

    /// Appends `data` to the output, replacing invalid sequences. Returns the
    /// incomplete tail if `last` is not set.
    fn convert(&mut self, data: &[u8], last: bool) -> Vec<u8> {
        let mut rest = data;
        loop {
            match std::str::from_utf8(rest) {
                Ok(s) => {
                    self.take_valid(s.as_bytes());
                    return Vec::new();
                }
                Err(e) => {
                    self.take_valid(&rest[..e.valid_up_to()]);
                    match e.error_len() {
                        Some(n) => {
                            self.replace();
                            rest = &rest[e.valid_up_to() + n..];
                        }
                        None if last => {
                            self.replace();
                            return Vec::new();
                        }
                        None => return rest[e.valid_up_to()..].to_vec(),
                    }
                }
            }
        }
    }

    fn take_valid(&mut self, valid: &[u8]) {
        let multibyte = valid.iter().filter(|b| **b >= 0xC0).count() as u64;
        self.stats
            .valid_multibyte
            .fetch_add(multibyte, Ordering::Relaxed);
        self.output.extend_from_slice(valid);
    }

    fn replace(&mut self) {
        self.stats.invalid.fetch_add(1, Ordering::Relaxed);
        self.output.extend_from_slice("\u{FFFD}".as_bytes());
    }
}

impl<R: Read> Read for Utf8Lossy<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        while self.pos == self.output.len() {
            if self.finished {
                return Ok(0);
            }
            self.output.clear();
            self.pos = 0;
            let n = self.inner.read(&mut self.input)?;
            let mut data = std::mem::take(&mut self.carry);
            data.extend_from_slice(&self.input[..n]);
            self.finished = n == 0;
            self.carry = self.convert(&data, n == 0);
        }
        let take = buf.len().min(self.output.len() - self.pos);
        buf[..take].copy_from_slice(&self.output[self.pos..self.pos + take]);
        self.pos += take;
        Ok(take)
    }
}

/// Passes bytes through unchanged and counts them as UTF-8 in [`DecodeStats`]:
/// valid multibyte sequences and invalid sequences, a sequence cut by a read
/// boundary counted once, one cut by the end of the stream invalid.
struct Utf8Count<R> {
    inner: R,
    stats: Arc<DecodeStats>,
    carry: Vec<u8>,
}

impl<R> Utf8Count<R> {
    fn new(inner: R, stats: Arc<DecodeStats>) -> Self {
        Self {
            inner,
            stats,
            carry: Vec::new(),
        }
    }
}

impl<R: Read> Read for Utf8Count<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        let last = n == 0;
        let mut data = std::mem::take(&mut self.carry);
        data.extend_from_slice(&buf[..n]);
        let (valid, invalid, tail) = count_chunk(&data, last);
        self.stats
            .valid_multibyte
            .fetch_add(valid, Ordering::Relaxed);
        self.stats.invalid.fetch_add(invalid, Ordering::Relaxed);
        self.carry = tail;
        Ok(n)
    }
}

/// Valid multibyte sequences and invalid sequences of `data`, and the
/// incomplete sequence at its end to carry to the next chunk (none when `last`:
/// it is counted invalid).
fn count_chunk(data: &[u8], last: bool) -> (u64, u64, Vec<u8>) {
    let (mut valid, mut invalid) = (0u64, 0u64);
    let mut rest = data;
    loop {
        match std::str::from_utf8(rest) {
            Ok(s) => {
                valid += s.bytes().filter(|b| *b >= 0xC0).count() as u64;
                return (valid, invalid, Vec::new());
            }
            Err(e) => {
                valid += rest[..e.valid_up_to()]
                    .iter()
                    .filter(|b| **b >= 0xC0)
                    .count() as u64;
                match e.error_len() {
                    Some(n) => {
                        invalid += 1;
                        rest = &rest[e.valid_up_to() + n..];
                    }
                    None if last => return (valid, invalid + 1, Vec::new()),
                    None => return (valid, invalid, rest[e.valid_up_to()..].to_vec()),
                }
            }
        }
    }
}

/// Windows-1252 to UTF-8, a chunk at a time.
struct Transcoder<R> {
    inner: R,
    decoder: encoding_rs::Decoder,
    input: Vec<u8>,
    output: Vec<u8>,
    pos: usize,
    len: usize,
    finished: bool,
}

impl<R> Transcoder<R> {
    fn new(inner: R) -> Self {
        let decoder = encoding_rs::WINDOWS_1252.new_decoder_without_bom_handling();
        let out = decoder.max_utf8_buffer_length(CHUNK).unwrap_or(4 * CHUNK);
        Self {
            inner,
            decoder,
            input: vec![0; CHUNK],
            output: vec![0; out],
            pos: 0,
            len: 0,
            finished: false,
        }
    }
}

impl<R: Read> Read for Transcoder<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        while self.pos == self.len {
            // The decoder panics if used after its last call.
            if self.finished {
                return Ok(0);
            }
            let n = self.inner.read(&mut self.input)?;
            let (_, _, written, _) =
                self.decoder
                    .decode_to_utf8(&self.input[..n], &mut self.output, n == 0);
            self.pos = 0;
            self.len = written;
            self.finished = n == 0;
            if n == 0 && written == 0 {
                return Ok(0);
            }
        }
        let take = buf.len().min(self.len - self.pos);
        buf[..take].copy_from_slice(&self.output[self.pos..self.pos + take]);
        self.pos += take;
        Ok(take)
    }
}

/// Unique, deterministic column names from the header cells. Control
/// characters become `_`, a name is cut at [`MAX_COLUMN_NAME_CHARS`], an empty
/// one becomes `column<N>` (1-based) and a repeat gets `_2`, `_3`, ... until it
/// is free. Names are compared as written (Parquet columns are case-sensitive).
pub fn column_names(raw: &[String]) -> Vec<String> {
    column_names_counted(raw).0
}

/// [`column_names`] and the number of candidate names it tried. Each base name
/// remembers the next suffix to try, so the work is linear in the number of
/// columns even when all of them are the same (16,384 identical headers).
fn column_names_counted(raw: &[String]) -> (Vec<String>, usize) {
    let mut taken: HashSet<String> = HashSet::new();
    let mut next_suffix: HashMap<String, usize> = HashMap::new();
    let mut tried = 0;
    let mut out = Vec::with_capacity(raw.len());
    for (i, name) in raw.iter().enumerate() {
        let cleaned: String = clean_name(name)
            .chars()
            .take(MAX_COLUMN_NAME_CHARS)
            .collect();
        let base = if cleaned.trim().is_empty() {
            format!("column{}", i + 1)
        } else {
            cleaned
        };
        tried += 1;
        let mut candidate = base.clone();
        if !taken.insert(candidate.clone()) {
            let mut n = next_suffix.get(&base).copied().unwrap_or(2);
            loop {
                let suffix = format!("_{n}");
                let keep = MAX_COLUMN_NAME_CHARS.saturating_sub(suffix.chars().count());
                candidate = format!("{}{suffix}", base.chars().take(keep).collect::<String>());
                tried += 1;
                n += 1;
                if taken.insert(candidate.clone()) {
                    break;
                }
            }
            next_suffix.insert(base, n);
        }
        out.push(candidate);
    }
    (out, tried)
}

/// A CSV ready to read: the types decided from the sample, and the rows as
/// batches of text.
pub struct OpenedCsv {
    pub schema: InferredSchema,
    pub delimiter: u8,
    pub encoding: Encoding,
    /// Rows the types were decided from (below [`INFERENCE_ROWS`] only when
    /// [`SAMPLE_MAX_BYTES`] or [`SAMPLE_MAX_CELLS`] was reached first).
    pub sample_rows: usize,
    /// Bytes of text in the sample.
    pub sample_bytes: usize,
    /// Records, blank lines and padded rows seen so far; final once `batches`
    /// is exhausted.
    pub stats: Arc<ScanStats>,
    /// What decoding found; final once `batches` is exhausted.
    pub decode: Arc<DecodeStats>,
    pub batches: RawBatches,
}

/// Rows as all-text Arrow batches, empty cells as null, every cell verbatim.
/// A batch holds at most [`BATCH_ROWS`] rows, [`BATCH_CELLS`] cells and
/// [`BATCH_BYTES`] bytes of text, whatever the shape of the file: that is what
/// keeps its memory bounded. A short row is padded with nulls and counted in
/// [`ScanStats::padded_rows`]; a row with more fields than the header is an
/// error. A failure ends the iteration after the rows read before it.
pub struct RawBatches {
    reader: ::csv::Reader<Box<dyn Read + Send>>,
    pending: VecDeque<::csv::ByteRecord>,
    held: Option<::csv::ByteRecord>,
    /// The record the parser reads into; what is kept is a copy of its exact size.
    scratch: ::csv::ByteRecord,
    schema: SchemaRef,
    limits: ReadLimits,
    stats: Arc<ScanStats>,
    rows_read: u64,
    failed: Option<CsvError>,
    done: bool,
}

/// A copy of `record` that holds exactly its bytes. The parser grows its
/// buffer by doubling and `ByteRecord::clone` copies the whole buffer, so a
/// kept clone costs up to twice its text; the sample and the records of a batch
/// are kept, so they are copied field by field into a buffer of the right size.
fn exact(record: &::csv::ByteRecord) -> ::csv::ByteRecord {
    let mut copy = ::csv::ByteRecord::with_capacity(record.as_slice().len(), record.len());
    for field in record.iter() {
        copy.push_field(field);
    }
    copy
}

fn utf8(field: &[u8]) -> Result<&str, CsvError> {
    std::str::from_utf8(field).map_err(|_| CsvError::Parse("a field is not valid UTF-8".into()))
}

impl RawBatches {
    fn next_record(&mut self) -> Result<Option<::csv::ByteRecord>, CsvError> {
        if let Some(r) = self.held.take().or_else(|| self.pending.pop_front()) {
            return Ok(Some(r));
        }
        match self.reader.read_byte_record(&mut self.scratch) {
            Ok(true) => Ok(Some(exact(&self.scratch))),
            Ok(false) => Ok(None),
            Err(e) => Err(CsvError::from_csv(e)),
        }
    }

    /// Reads the records of one batch (bounded by rows, cells and bytes), then
    /// builds each column with the exact capacity its values need: a builder
    /// started empty reserves a few KiB per column that it keeps, which is
    /// nothing for a handful of columns and megabytes per batch for hundreds.
    fn build(&mut self) -> Option<RecordBatch> {
        let columns = self.schema.fields().len();
        let mut records: Vec<::csv::ByteRecord> = Vec::new();
        let mut bytes = 0usize;
        let limits = self.limits;
        while records.len() < limits.batch_rows
            && (records.len() + 1) * columns.max(1) <= limits.batch_cells
        {
            let record = match self.next_record() {
                Ok(Some(r)) => r,
                Ok(None) => {
                    self.done = true;
                    break;
                }
                Err(e) => {
                    self.failed = Some(e);
                    break;
                }
            };
            let len = record.as_slice().len();
            if !records.is_empty() && bytes + len > limits.batch_bytes {
                self.held = Some(record);
                break;
            }
            self.rows_read += 1;
            if record.len() > columns {
                self.failed = Some(CsvError::Parse(format!(
                    "row {} has more fields than the header",
                    self.rows_read
                )));
                break;
            }
            if record.len() < columns {
                self.stats.note_padded();
            }
            bytes += len;
            records.push(record);
        }
        if records.is_empty() {
            if !self.done && self.failed.is_none() {
                // No record fits the limits: never the end of the data.
                self.failed = Some(CsvError::InvalidLimits(format!(
                    "no row of {columns} cells fits a batch of {} rows and {} cells",
                    limits.batch_rows, limits.batch_cells
                )));
            }
            return None;
        }
        let mut column_bytes = vec![0usize; columns];
        for r in &records {
            for (total, f) in column_bytes.iter_mut().zip(r.iter()) {
                *total += f.len();
            }
        }
        let mut builders: Vec<StringBuilder> = column_bytes
            .iter()
            .map(|b| StringBuilder::with_capacity(records.len(), *b))
            .collect();
        for r in &records {
            for (i, b) in builders.iter_mut().enumerate() {
                match r.get(i).map(utf8).transpose() {
                    Ok(Some(f)) if !f.is_empty() => b.append_value(f),
                    Ok(_) => b.append_null(),
                    Err(e) => {
                        self.failed = Some(e);
                        return None;
                    }
                }
            }
        }
        let arrays: Vec<ArrayRef> = builders
            .iter_mut()
            .map(|b| Arc::new(b.finish()) as ArrayRef)
            .collect();
        match RecordBatch::try_new(self.schema.clone(), arrays) {
            Ok(batch) => Some(batch),
            Err(e) => {
                // Never end the table silently.
                self.failed = Some(CsvError::Parse(format!("could not build a batch: {e}")));
                None
            }
        }
    }
}

impl Iterator for RawBatches {
    type Item = Result<RecordBatch, CsvError>;

    fn next(&mut self) -> Option<Self::Item> {
        if !self.done && self.failed.is_none() {
            if let Some(batch) = self.build() {
                return Some(Ok(batch));
            }
        }
        // Nothing more to build: the failure that stopped it, once, then the end.
        self.done = true;
        self.failed.take().map(Err)
    }
}

/// Opens a CSV: prepares the input, reads the header and a bounded sample to
/// decide the types, then returns the rows (the sample included) as batches
/// bounded by rows, cells and bytes. The limits are the constants of this
/// module; the public API takes no other (naming either of these outside the
/// crate does not compile).
///
/// ```compile_fail
/// use colmena::tabular_prepare::csv::ReadLimits;
/// ```
///
/// ```compile_fail
/// use colmena::tabular_prepare::csv::open_csv_with;
/// ```
pub fn open_csv<R: Read + Send + 'static>(
    input: R,
    force: Option<Encoding>,
) -> Result<OpenedCsv, CsvError> {
    open_csv_with(input, force, &ReadLimits::default())
}

/// [`open_csv`] with other limits: crate-private, for tests that move the
/// boundaries. The limits are validated; see [`ReadLimits::validate`].
pub(crate) fn open_csv_with<R: Read + Send + 'static>(
    input: R,
    force: Option<Encoding>,
    limits: &ReadLimits,
) -> Result<OpenedCsv, CsvError> {
    limits.validate(1)?;
    let prepared = prepare_input(input, force)?;
    let mut reader = ::csv::ReaderBuilder::new()
        .delimiter(prepared.delimiter)
        .has_headers(false)
        .flexible(true)
        .from_reader(prepared.reader);
    let mut record = ::csv::ByteRecord::new();
    if !reader
        .read_byte_record(&mut record)
        .map_err(CsvError::from_csv)?
    {
        return Err(CsvError::Empty);
    }
    let columns = record.len();
    if columns > MAX_COLUMNS {
        return Err(CsvError::TooManyColumns { limit: MAX_COLUMNS });
    }
    limits.validate(columns)?;
    let raw_names = record
        .iter()
        .map(|f| utf8(f).map(str::to_string))
        .collect::<Result<Vec<_>, _>>()?;
    let mut inferer = SchemaInferer::with_window(column_names(&raw_names), limits.inference_rows);
    let mut pending = VecDeque::new();
    let mut row = ::csv::ByteRecord::new();
    let (mut sample_bytes, mut sample_cells) = (0usize, 0usize);
    while inferer.rows_seen() < limits.inference_rows
        && sample_bytes < limits.sample_bytes
        && sample_cells < limits.sample_cells
    {
        if !reader
            .read_byte_record(&mut row)
            .map_err(CsvError::from_csv)?
        {
            break;
        }
        sample_bytes += row.as_slice().len();
        sample_cells += row.len().max(columns);
        let cells = row.iter().map(utf8).collect::<Result<Vec<_>, _>>()?;
        inferer.observe(&cells);
        pending.push_back(exact(&row));
    }
    let sample_rows = inferer.rows_seen();
    let schema = inferer.finish();
    let text_schema = Arc::new(Schema::new(
        schema
            .columns
            .iter()
            .map(|c| Field::new(&c.name, DataType::Utf8, true))
            .collect::<Vec<_>>(),
    ));
    Ok(OpenedCsv {
        schema,
        delimiter: prepared.delimiter,
        encoding: prepared.encoding,
        sample_rows,
        sample_bytes,
        stats: prepared.stats.clone(),
        decode: prepared.decode,
        batches: RawBatches {
            reader,
            pending,
            held: None,
            scratch: ::csv::ByteRecord::new(),
            schema: text_schema,
            limits: *limits,
            stats: prepared.stats,
            rows_read: 0,
            failed: None,
            done: false,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tabular_prepare::manifest::ColumnType;
    use crate::tabular_prepare::scan::MAX_RECORD_BYTES;
    use arrow_array::{Array, StringArray};
    use std::io::{Cursor, Read};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    fn prepared(bytes: &[u8]) -> Prepared {
        prepare_input(Cursor::new(bytes.to_vec()), None).unwrap()
    }

    fn text_of(mut p: Prepared) -> String {
        let mut out = String::new();
        p.reader.read_to_string(&mut out).unwrap();
        out
    }

    #[test]
    fn the_delimiter_is_sniffed_among_comma_semicolon_tab_and_pipe() {
        for (d, name) in [
            (b',', "comma"),
            (b';', "semicolon"),
            (b'\t', "tab"),
            (b'|', "pipe"),
        ] {
            let sep = (d as char).to_string();
            let csv = format!("a{sep}b{sep}c\n1{sep}2{sep}3\n4{sep}5{sep}6\n");
            assert_eq!(prepared(csv.as_bytes()).delimiter, d, "{name}");
        }
    }

    #[test]
    fn a_delimiter_inside_quotes_does_not_decide() {
        // Commas everywhere inside quoted cells; the real separator is a semicolon.
        let csv = "name;note\n\"a,b,c\";\"x,y,z\"\n\"d,e,f\";\"u,v,w\"\n";
        assert_eq!(prepared(csv.as_bytes()).delimiter, b';');
        // Decimal commas with a semicolon separator.
        let csv = "id;price\n1;1,5\n2;2,5\n3;10,25\n";
        assert_eq!(prepared(csv.as_bytes()).delimiter, b';');
    }

    #[test]
    fn a_single_column_or_header_only_file_falls_back_to_comma() {
        assert_eq!(prepared(b"name\nann\nbob\n").delimiter, b',');
        assert_eq!(prepared(b"a;b;c\n").delimiter, b';');
        assert_eq!(prepared(b"only_header_no_newline").delimiter, b',');
    }

    #[test]
    fn the_sniff_ignores_the_row_cut_by_the_sample_limit() {
        // Rows of 3 semicolon fields, long enough that the 1 MiB sample ends
        // inside a row; the half row must not break the vote.
        let row = format!(
            "{};{};{}\n",
            "x".repeat(1000),
            "y".repeat(1000),
            "z".repeat(1000)
        );
        let csv = format!("a;b;c\n{}", row.repeat(2000));
        assert!(csv.len() > 2 * SNIFF_BYTES);
        assert_eq!(prepared(csv.as_bytes()).delimiter, b';');
    }

    #[test]
    fn a_utf8_bom_is_removed_and_the_text_is_kept() {
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice("name,city\nZoë,São Paulo\n".as_bytes());
        let p = prepared(&bytes);
        assert_eq!(p.encoding, Encoding::Utf8);
        assert_eq!(text_of(p), "name,city\nZoë,São Paulo\n");
    }

    #[test]
    fn utf16_and_binary_input_are_refused_with_a_typed_error() {
        let utf16le = [0xFF, 0xFE, b'a', 0, b',', 0, b'b', 0];
        let utf16be = [0xFE, 0xFF, 0, b'a', 0, b',', 0, b'b'];
        for bytes in [&utf16le[..], &utf16be[..], b"a,b\n1,\0\n2,3\n"] {
            let err = prepare_input(Cursor::new(bytes.to_vec()), None)
                .err()
                .unwrap();
            assert!(matches!(err, CsvError::UnsupportedEncoding(_)), "{err:?}");
        }
    }

    #[test]
    fn windows_1252_is_transcoded_to_utf8() {
        // 0xE9 is "é" and 0x80 is the euro sign in Windows-1252.
        let p = prepared(b"name,price\ncaf\xE9,\x805\n");
        assert_eq!(p.encoding, Encoding::Windows1252);
        assert_eq!(p.delimiter, b',');
        assert_eq!(text_of(p), "name,price\ncafé,€5\n");
    }

    #[test]
    fn transcoding_is_correct_across_internal_chunk_boundaries() {
        let mut bytes = b"c\n".to_vec();
        for _ in 0..50_000 {
            bytes.extend_from_slice(b"caf\xE9\x80\n");
        }
        let text = text_of(prepared(&bytes));
        assert_eq!(text, format!("c\n{}", "café€\n".repeat(50_000)));
    }

    #[test]
    fn utf8_text_passes_through_byte_for_byte_even_across_the_sample_cut() {
        // A multibyte character straddling the sample limit must not make the
        // file look like Windows-1252.
        let mut s = String::from("a,b\n");
        while s.len() < SNIFF_BYTES - 1 {
            s.push('x');
        }
        s.push_str("é,ñ\nsecond,row\n");
        let p = prepared(s.as_bytes());
        assert_eq!(p.encoding, Encoding::Utf8);
        assert_eq!(text_of(p), s);
    }

    #[test]
    fn empty_input_is_a_typed_error_and_header_only_is_not() {
        for bytes in [&b""[..], &[0xEF, 0xBB, 0xBF][..], b"\n\r\n  \n"] {
            let err = prepare_input(Cursor::new(bytes.to_vec()), None)
                .err()
                .unwrap();
            assert_eq!(err, CsvError::Empty, "{bytes:?}");
        }
        assert_eq!(text_of(prepared(b"a,b,c")), "a,b,c");
    }

    #[test]
    fn the_input_is_not_read_ahead_of_the_sample() {
        struct Counting {
            produced: Arc<AtomicUsize>,
            row: Vec<u8>,
            pos: usize,
        }
        impl Read for Counting {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                let n = buf.len().min(16 * 1024);
                for b in &mut buf[..n] {
                    *b = self.row[self.pos % self.row.len()];
                    self.pos += 1;
                }
                self.produced.fetch_add(n, Ordering::SeqCst);
                Ok(n)
            }
        }
        let produced = Arc::new(AtomicUsize::new(0));
        let src = Counting {
            produced: produced.clone(),
            row: b"1,2,3\n".to_vec(),
            pos: 0,
        };
        let mut p = prepare_input(src, None).unwrap();
        let mut first = [0u8; 100];
        p.reader.read_exact(&mut first).unwrap();
        // A 64 MiB file that is not touched beyond the sample and one chunk.
        assert!(produced.load(Ordering::SeqCst) <= SNIFF_BYTES + 32 * 1024);
    }

    #[test]
    fn a_tie_between_delimiters_goes_to_the_first_in_the_fixed_order() {
        // Two fields with a comma and two with a semicolon on every row.
        assert_eq!(prepared(b"a,b;c\nd,e;f\ng,h;i\n").delimiter, b',');
    }

    #[test]
    fn a_record_cut_by_the_sample_limit_gets_no_vote() {
        // The header says semicolon; the only other record is cut by the
        // sample limit and, as far as it goes, looks like eleven columns of
        // commas.
        let csv = format!("a;b\n,,,,,,,,,,{}", "x".repeat(2 * SNIFF_BYTES));
        assert_eq!(prepared(csv.as_bytes()).delimiter, b';');
    }

    #[test]
    fn a_line_without_a_newline_is_cut_off_at_the_limit() {
        struct Counting {
            produced: Arc<AtomicUsize>,
        }
        impl Read for Counting {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                let n = buf.len().min(64 * 1024);
                buf[..n].fill(b'x');
                self.produced.fetch_add(n, Ordering::SeqCst);
                Ok(n)
            }
        }
        let produced = Arc::new(AtomicUsize::new(0));
        // An endless line: never a newline, never an end.
        let mut p = prepare_input(
            Counting {
                produced: produced.clone(),
            },
            None,
        )
        .unwrap();
        let mut sink = vec![0u8; 8192];
        let err = loop {
            match p.reader.read(&mut sink) {
                Ok(0) => panic!("the stream has no end"),
                Ok(_) => {}
                Err(e) => break e,
            }
        };
        assert!(matches!(
            CsvError::from_io(err),
            CsvError::RecordTooLong {
                limit: MAX_RECORD_BYTES,
                ..
            }
        ));
        // Nothing past the limit plus the sample and a chunk was pulled.
        assert!(
            produced.load(Ordering::SeqCst) <= SNIFF_BYTES + MAX_RECORD_BYTES + 128 * 1024,
            "pulled {} bytes",
            produced.load(Ordering::SeqCst)
        );
    }

    #[test]
    fn a_line_of_exactly_the_limit_is_accepted_and_one_byte_more_is_not() {
        let line = |n: usize| format!("{}\nb\n", "x".repeat(n)).into_bytes();
        let mut ok = prepare_input(Cursor::new(line(MAX_RECORD_BYTES)), None).unwrap();
        assert!(ok.reader.read_to_end(&mut Vec::new()).is_ok());
        let mut bad = prepare_input(Cursor::new(line(MAX_RECORD_BYTES + 1)), None).unwrap();
        let err = bad.reader.read_to_end(&mut Vec::new()).unwrap_err();
        assert!(matches!(
            CsvError::from_io(err),
            CsvError::RecordTooLong {
                limit: MAX_RECORD_BYTES,
                ..
            }
        ));
    }

    #[test]
    fn carriage_return_line_endings_do_not_count_as_one_long_line() {
        let row = format!("{},{}\r", "x".repeat(50), "y".repeat(50));
        let csv = format!("a,b\r{}", row.repeat(100_000));
        assert!(csv.len() > MAX_RECORD_BYTES);
        let mut p = prepared(csv.as_bytes());
        assert!(p.reader.read_to_end(&mut Vec::new()).is_ok());
    }

    #[test]
    fn column_names_are_cleaned_and_unique() {
        let raw: Vec<String> = ["id", "", "id", " name ", "ID", "\u{0}x", "  "]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            column_names(&raw),
            // A name is trimmed, as a table's is (a deliberate change with the invisible-
            // character rules: " name " used to keep its spaces).
            vec!["id", "column2", "id_2", "name", "ID", "_x", "column7"]
        );
        // Deterministic.
        assert_eq!(column_names(&raw), column_names(&raw));
        // A long name is cut, and a suffix still fits inside the limit.
        let long = "é".repeat(MAX_COLUMN_NAME_CHARS + 20);
        let cut = column_names(&[long.clone(), long]);
        assert!(cut
            .iter()
            .all(|n| n.chars().count() <= MAX_COLUMN_NAME_CHARS));
        assert_ne!(cut[0], cut[1]);
        // A suffixed name never collides with a later real one.
        let raw: Vec<String> = ["a", "a", "a_2"].iter().map(|s| s.to_string()).collect();
        assert_eq!(column_names(&raw), vec!["a", "a_2", "a_2_2"]);
    }

    #[test]
    fn a_finished_windows_1252_stream_keeps_answering_end_of_file() {
        // The decoder panics when used after its last call; a parser that
        // reads again after the end must get Ok(0), not a panic.
        let mut p = prepared(b"a,b\ncaf\xE9,1\n");
        let mut sink = Vec::new();
        p.reader.read_to_end(&mut sink).unwrap();
        let mut buf = [0u8; 16];
        for _ in 0..3 {
            assert_eq!(p.reader.read(&mut buf).unwrap(), 0);
        }
    }

    fn open(bytes: &[u8]) -> OpenedCsv {
        open_csv(Cursor::new(bytes.to_vec()), None).unwrap()
    }

    fn rows_of(opened: OpenedCsv) -> Vec<Vec<Option<String>>> {
        let mut out = Vec::new();
        for batch in opened.batches {
            let batch = batch.unwrap();
            for r in 0..batch.num_rows() {
                out.push(
                    (0..batch.num_columns())
                        .map(|c| {
                            let col = batch
                                .column(c)
                                .as_any()
                                .downcast_ref::<StringArray>()
                                .unwrap();
                            (!col.is_null(r)).then(|| col.value(r).to_string())
                        })
                        .collect(),
                );
            }
        }
        out
    }

    fn cell(s: &str) -> Option<String> {
        Some(s.to_string())
    }

    #[test]
    fn the_header_is_row_one_and_the_types_come_from_the_sample() {
        let o = open(b"id,code,price\n1,00123,1.5\n2,01234,2.5\n");
        let cols: Vec<_> = o
            .schema
            .columns
            .iter()
            .map(|c| (c.name.as_str(), c.column_type))
            .collect();
        assert_eq!(
            cols,
            vec![
                ("id", ColumnType::Int),
                ("code", ColumnType::String),
                ("price", ColumnType::Float)
            ]
        );
        assert_eq!(o.delimiter, b',');
        assert_eq!(o.sample_rows, 2);
        assert_eq!(rows_of(o).len(), 2);
    }

    #[test]
    fn cells_come_out_verbatim_with_empty_as_null() {
        let csv = "a;b;c\r\n\"x;y\";\"line1\nline2\";\"say \"\"hi\"\"\"\r\n007;;\r\n";
        let rows = rows_of(open(csv.as_bytes()));
        assert_eq!(
            rows[0],
            vec![cell("x;y"), cell("line1\nline2"), cell("say \"hi\"")]
        );
        assert_eq!(rows[1], vec![cell("007"), None, None]);
    }

    #[test]
    fn a_bom_and_windows_1252_are_handled_end_to_end() {
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice("name,city\nZoë,São Paulo\n".as_bytes());
        assert_eq!(
            rows_of(open(&bytes))[0],
            vec![cell("Zoë"), cell("São Paulo")]
        );
        let o = open(b"name,note\ncaf\xE9,\x80\n");
        assert_eq!(o.encoding, Encoding::Windows1252);
        assert_eq!(rows_of(o)[0], vec![cell("café"), cell("€")]);
    }

    #[test]
    fn a_header_only_file_has_a_schema_and_no_rows() {
        let o = open(b"a,b,c\n");
        assert_eq!(o.schema.columns.len(), 3);
        assert!(o
            .schema
            .columns
            .iter()
            .all(|c| c.column_type == ColumnType::String));
        assert_eq!(rows_of(o).len(), 0);
    }

    #[test]
    fn a_short_row_is_null_padded_and_a_long_row_is_an_error() {
        let rows = rows_of(open(b"a,b,c\n1,2,3\n4,5\n"));
        assert_eq!(rows[1], vec![cell("4"), cell("5"), None]);
        let o = open(b"a,b\n1,2\n3,4,5\n");
        let err = o.batches.last().unwrap().unwrap_err();
        assert!(matches!(err, CsvError::Parse(_)), "{err:?}");
    }

    #[test]
    fn empty_input_and_too_many_columns_are_typed_errors() {
        assert_eq!(
            open_csv(Cursor::new(Vec::new()), None).err().unwrap(),
            CsvError::Empty
        );
        let header = (0..=MAX_COLUMNS)
            .map(|i| format!("c{i}"))
            .collect::<Vec<_>>()
            .join(",");
        let err = open_csv(Cursor::new(format!("{header}\n1\n").into_bytes()), None)
            .err()
            .unwrap();
        assert_eq!(err, CsvError::TooManyColumns { limit: MAX_COLUMNS });
        let header = (0..MAX_COLUMNS)
            .map(|i| format!("c{i}"))
            .collect::<Vec<_>>()
            .join(",");
        assert!(open_csv(Cursor::new(format!("{header}\n").into_bytes()), None).is_ok());
    }

    /// Generates `rows` rows of "i,x" lazily and counts what was pulled.
    struct Rows {
        next: u64,
        rows: u64,
        pending: Vec<u8>,
        pulled: Arc<AtomicUsize>,
    }

    impl Rows {
        fn new(rows: u64, pulled: Arc<AtomicUsize>) -> Self {
            Self {
                next: 0,
                rows,
                pending: b"id,v\n".to_vec(),
                pulled,
            }
        }
    }

    impl Read for Rows {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            while self.pending.len() < buf.len().min(32 * 1024) && self.next < self.rows {
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

    #[test]
    fn five_million_rows_stream_in_bounded_batches_without_reading_ahead() {
        let pulled = Arc::new(AtomicUsize::new(0));
        let o = open_csv(Rows::new(5_000_000, pulled.clone()), None).unwrap();
        assert_eq!(o.sample_rows, INFERENCE_ROWS);
        let (mut rows, mut largest, mut worst_ahead) = (0usize, 0usize, 0usize);
        for batch in o.batches {
            let batch = batch.unwrap();
            largest = largest.max(batch.num_rows());
            rows += batch.num_rows();
            // A row is at most 12 bytes here. Whatever has been pulled is what
            // was consumed plus the sample and a few batches, never the file.
            let consumed = rows * 12;
            worst_ahead = worst_ahead.max(pulled.load(Ordering::SeqCst).saturating_sub(consumed));
        }
        assert_eq!(rows, 5_000_000);
        assert!(largest <= BATCH_ROWS, "a batch of {largest} rows");
        assert!(
            worst_ahead <= SNIFF_BYTES + 4 * BATCH_ROWS * 12,
            "read {worst_ahead} bytes ahead"
        );
    }

    #[test]
    fn batches_shrink_for_wide_files() {
        let cols = 2000;
        let header = (0..cols)
            .map(|i| format!("c{i}"))
            .collect::<Vec<_>>()
            .join(",");
        let row = vec!["1"; cols].join(",");
        let csv = format!("{header}\n{}", format!("{row}\n").repeat(1500));
        let o = open(csv.as_bytes());
        let mut total = 0;
        for b in o.batches {
            let b = b.unwrap();
            assert!(
                b.num_rows() * cols <= BATCH_CELLS,
                "{} cells",
                b.num_rows() * cols
            );
            total += b.num_rows();
        }
        assert_eq!(total, 1500);
    }

    #[test]
    fn the_sample_stops_at_its_byte_cap_and_every_row_still_comes_out() {
        // 5,000 rows of about 4 KiB: the sample (16 MiB) ends near row 4,000.
        let big = "x".repeat(4000);
        let csv = format!("a,b\n{}", format!("1,{big}\n").repeat(5000));
        let o = open(csv.as_bytes());
        assert!(
            o.sample_rows < INFERENCE_ROWS && o.sample_rows > 3000,
            "{}",
            o.sample_rows
        );
        assert!(
            o.sample_bytes < SAMPLE_MAX_BYTES + 64 * 1024,
            "{}",
            o.sample_bytes
        );
        let rows = rows_of(o);
        assert_eq!(rows.len(), 5000);
        assert!(rows
            .iter()
            .all(|r| r[0] == cell("1") && r[1].as_deref() == Some(big.as_str())));
    }

    #[test]
    fn guard_errors_surface_typed_from_the_batches() {
        // A line past the limit after the sample.
        let mut csv = b"a,b\n".to_vec();
        for _ in 0..INFERENCE_ROWS + 100 {
            csv.extend_from_slice(b"1,2\n");
        }
        csv.extend(std::iter::repeat_n(b'x', MAX_RECORD_BYTES + 10));
        let results: Vec<_> = open(&csv).batches.take(1000).collect();
        // The rows before the long line arrive, then exactly one error ends it.
        assert_eq!(results.iter().filter(|r| r.is_err()).count(), 1);
        let err = results.last().unwrap().as_ref().unwrap_err();
        assert!(matches!(
            *err,
            CsvError::RecordTooLong {
                limit: MAX_RECORD_BYTES,
                ..
            }
        ));
    }

    #[test]
    fn a_line_guard_error_during_the_sample_is_typed_too() {
        let mut csv = b"a,b\n".to_vec();
        csv.extend(std::iter::repeat_n(b'x', MAX_RECORD_BYTES + 10));
        let err = open_csv(Cursor::new(csv), None).err().unwrap();
        assert!(matches!(
            err,
            CsvError::RecordTooLong {
                limit: MAX_RECORD_BYTES,
                ..
            }
        ));
    }

    #[test]
    fn a_one_column_file_keeps_its_empty_cells_as_null_rows() {
        let o = open(b"name\nann\n\nbob\n\n\ncy\n\n");
        let stats = o.stats.clone();
        let rows: Vec<_> = rows_of(o).into_iter().map(|r| r[0].clone()).collect();
        assert_eq!(
            rows,
            vec![cell("ann"), None, cell("bob"), None, None, cell("cy")]
        );
        assert_eq!((stats.blank_rows(), stats.blank_dropped()), (3, 1));
    }

    #[test]
    fn blank_lines_in_a_multi_column_file_are_dropped_and_reported() {
        let o = open(b"a,b\n1,2\n\n3,4\n\n\n");
        let stats = o.stats.clone();
        let rows = rows_of(o);
        assert_eq!(
            rows,
            vec![vec![cell("1"), cell("2")], vec![cell("3"), cell("4")]]
        );
        assert_eq!((stats.blank_rows(), stats.blank_dropped()), (0, 3));
    }

    /// Accented UTF-8 text with one stray byte in the middle.
    fn mostly_utf8_with_a_stray_byte() -> Vec<u8> {
        let mut bytes = b"name,city\n".to_vec();
        for _ in 0..200 {
            bytes.extend_from_slice("José,São Paulo\n".as_bytes());
        }
        bytes.extend_from_slice(b"ab\xFFcd,x\n");
        for _ in 0..200 {
            bytes.extend_from_slice("Zoë,Köln\n".as_bytes());
        }
        bytes
    }

    #[test]
    fn one_stray_byte_does_not_turn_a_utf8_file_into_mojibake() {
        let bytes = mostly_utf8_with_a_stray_byte();
        let p = prepared(&bytes);
        assert_eq!(p.encoding, Encoding::Utf8);
        let stats = p.decode.clone();
        let out = text_of(p);
        // Every accent survives; the stray byte became one replacement character.
        assert!(out.contains("José,São Paulo\n") && out.contains("Zoë,Köln\n"));
        assert!(!out.contains('Ã'), "mojibake in the output");
        assert!(out.contains("ab\u{FFFD}cd,x\n"));
        assert_eq!((stats.invalid(), stats.valid_multibyte() > 0), (1, true));
    }

    #[test]
    fn a_real_windows_1252_file_is_not_mistaken_for_utf8() {
        // Accents are single bytes that are not valid UTF-8 sequences.
        let p = prepared(b"name,city\nJos\xE9,S\xE3o Paulo\nZo\xEB,K\xF6ln\n");
        assert_eq!(p.encoding, Encoding::Windows1252);
        assert_eq!(text_of(p), "name,city\nJosé,São Paulo\nZoë,Köln\n");
    }

    #[test]
    fn the_plausibility_rule_prefers_utf8_for_ties_and_near_ties() {
        let stats = |valid, invalid| {
            let d = DecodeStats::default();
            d.valid_multibyte.store(valid, Ordering::Relaxed);
            d.invalid.store(invalid, Ordering::Relaxed);
            d.plausibly_utf8()
        };
        // No invalid sequence; more valid than invalid; a tie; a near tie (half).
        assert!(stats(0, 0) && stats(5, 0) && stats(2, 1) && stats(100, 99));
        assert!(stats(1, 1) && stats(3, 5) && stats(3, 6));
        // Mostly invalid: ASCII plus a stray byte, or real Windows-1252.
        assert!(!stats(0, 1) && !stats(1, 3) && !stats(3, 7) && !stats(0, 400));
    }

    #[test]
    fn counting_is_the_same_however_the_stream_is_split_and_a_cut_end_is_one_replacement() {
        let mut bytes = "é,ñ,ü\n".as_bytes().to_vec();
        bytes.extend_from_slice(b"a\xFFb\xC3\n");
        bytes.extend_from_slice("日本\n".as_bytes());
        bytes.push(0xE2); // a character cut by the end of the stream
        let whole = prepared(&bytes);
        let (valid, invalid) = (whole.decode.clone(), whole.decode.clone());
        let expect = text_of(whole);
        let mut p = prepared(&bytes);
        let (mut out, mut one) = (Vec::new(), [0u8; 1]);
        while p.reader.read(&mut one).unwrap() == 1 {
            out.push(one[0]);
        }
        assert_eq!(String::from_utf8(out).unwrap(), expect);
        // 0xFF, the lone 0xC3 and the cut 0xE2 are three invalid sequences;
        // é ñ ü 日 本 are five valid multibyte ones.
        assert_eq!((p.decode.invalid(), p.decode.valid_multibyte()), (3, 5));
        assert_eq!((valid.invalid(), invalid.valid_multibyte()), (3, 5));
    }

    #[test]
    fn replacement_characters_cannot_overflow_the_output() {
        // Every byte invalid: three output bytes per input byte.
        let mut bytes = b"a\n".to_vec();
        bytes.extend(std::iter::repeat_n(0xFF, 100_000));
        let p = prepare_input(Cursor::new(bytes), Some(Encoding::Utf8)).unwrap();
        let stats = p.decode.clone();
        let out = text_of(p);
        assert_eq!(out.matches('\u{FFFD}').count(), 100_000);
        assert_eq!(stats.invalid(), 100_000);
    }

    #[test]
    fn a_late_stray_byte_after_the_sample_is_replaced_and_reported_through_the_batches() {
        let mut bytes = b"a,b\n".to_vec();
        for _ in 0..SNIFF_BYTES / 8 {
            bytes.extend_from_slice("é,ñ\n".as_bytes());
        }
        bytes.extend_from_slice(b"x\xFF,y\n");
        let o = open(&bytes);
        let (stats, enc) = (o.decode.clone(), o.encoding);
        let rows = rows_of(o);
        assert_eq!(enc, Encoding::Utf8);
        assert_eq!(rows.last().unwrap()[0].as_deref(), Some("x\u{FFFD}"));
        assert_eq!((stats.invalid(), stats.plausibly_utf8()), (1, true));
    }

    #[test]
    fn an_ascii_sample_followed_by_late_accents_is_not_plausibly_utf8() {
        let mut bytes = b"a,b\n".to_vec();
        for _ in 0..SNIFF_BYTES / 4 + 100 {
            bytes.extend_from_slice(b"1,x\n");
        }
        bytes.extend_from_slice(b"caf\xE9,2\nna\xEFve,3\n");
        let o = open(&bytes);
        assert_eq!(o.encoding, Encoding::Utf8);
        let stats = o.decode.clone();
        let _ = rows_of(o);
        assert_eq!((stats.invalid(), stats.valid_multibyte()), (2, 0));
        assert!(!stats.plausibly_utf8());
    }

    #[test]
    fn the_sample_count_counts_sequences_not_bytes_and_a_cut_end_only_when_it_is_the_end() {
        assert_eq!(count_utf8("é日".as_bytes(), true), (2, 0));
        assert_eq!(count_utf8(b"a\xFF\xC3\xA9", true), (1, 1));
        // A character cut by a sample limit is not invalid; at the real end it is.
        assert_eq!(count_utf8(b"ab\xE2\x82", false), (0, 0));
        assert_eq!(count_utf8(b"ab\xE2\x82", true), (0, 1));
    }

    /// The most memory one raw batch may hold: its text with the builder's
    /// capacity doubling, plus offsets and validity for the cells.
    fn raw_batch_bound() -> usize {
        2 * BATCH_BYTES + 5 * 1024 * 1024
    }

    fn largest_batch(o: OpenedCsv) -> (usize, usize) {
        let (mut rows, mut worst) = (0, 0);
        for b in o.batches {
            let b = b.unwrap();
            rows += b.num_rows();
            worst = worst.max(b.get_array_memory_size());
        }
        (rows, worst)
    }

    #[test]
    fn rows_of_wide_text_are_batched_by_bytes_not_only_by_rows() {
        // 400 rows of 100 KiB: 40 MB of text in few rows. Before the byte
        // budget this was one batch of 52 MB (and 8,192 such rows, 1 GiB).
        let cell = "x".repeat(100 * 1024);
        let row = format!("{cell},1\n");
        let csv = format!("t,n\n{}", row.repeat(400));
        let (rows, worst) = largest_batch(open(csv.as_bytes()));
        assert_eq!(rows, 400);
        assert!(worst <= raw_batch_bound(), "a batch of {worst} bytes");
    }

    #[test]
    fn rows_at_the_record_limit_fill_a_batch_with_a_few_rows_and_still_all_arrive() {
        let cell = "y".repeat(MAX_RECORD_BYTES - 10);
        let csv = format!("t,n\n{}", format!("{cell},1\n").repeat(40));
        let (rows, worst) = largest_batch(open(csv.as_bytes()));
        assert_eq!(rows, 40);
        assert!(worst <= raw_batch_bound(), "a batch of {worst} bytes");
    }

    #[test]
    fn transcoding_that_triples_the_size_cannot_overflow_a_batch() {
        // 0x80 is the euro sign in Windows-1252: one byte in, three bytes out.
        // 150 rows of 100 KiB is 15 MB in the file and 45 MB as text.
        let mut csv = b"t,n\n".to_vec();
        for _ in 0..150 {
            csv.extend(std::iter::repeat_n(0x80u8, 100 * 1024));
            csv.extend_from_slice(b",1\n");
        }
        let o = open(&csv);
        assert_eq!(o.encoding, Encoding::Windows1252);
        let (rows, worst) = largest_batch(o);
        assert_eq!(rows, 150);
        assert!(worst <= raw_batch_bound(), "a batch of {worst} bytes");
    }

    #[test]
    fn a_wide_file_is_sampled_over_fewer_rows_and_still_arrives_whole() {
        let cols = 4000;
        let header = (0..cols)
            .map(|i| format!("c{i}"))
            .collect::<Vec<_>>()
            .join(",");
        let row = vec!["1"; cols].join(",");
        let csv = format!("{header}\n{}", format!("{row}\n").repeat(1200));
        let o = open(csv.as_bytes());
        // 2,000,000 cells at 4,000 per row: 500 rows decide the types.
        assert_eq!(o.sample_rows, SAMPLE_MAX_CELLS / cols);
        assert_eq!(largest_batch(o).0, 1200);
    }

    #[test]
    fn short_rows_are_padded_and_counted_and_long_rows_are_an_error_naming_the_row() {
        let o = open(b"a,b,c\n1,2,3\n4,5\n6\n7,8,9\n");
        let stats = o.stats.clone();
        let rows = rows_of(o);
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[1], vec![cell("4"), cell("5"), None]);
        assert_eq!(stats.padded_rows(), 2);

        let o = open(b"a,b\n1,2\n3,4,5\n");
        let err = o.batches.last().unwrap().unwrap_err();
        assert_eq!(
            err,
            CsvError::Parse("row 2 has more fields than the header".into())
        );
    }

    #[test]
    fn identical_header_names_are_resolved_in_linear_work() {
        let raw = vec!["id".to_string(); MAX_COLUMNS];
        let (names, tried) = column_names_counted(&raw);
        assert_eq!(names.len(), MAX_COLUMNS);
        // One probe for the first, one per repeat: not 134 million.
        assert!(tried <= 2 * MAX_COLUMNS, "{tried} probes");
        let unique: HashSet<_> = names.iter().collect();
        assert_eq!(unique.len(), MAX_COLUMNS);
        assert_eq!(names[1], "id_2");
        assert_eq!(names[MAX_COLUMNS - 1], format!("id_{MAX_COLUMNS}"));
        // A real header named like a generated one is still not lost.
        let raw: Vec<String> = ["a", "a", "a_3", "a", "a"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(column_names(&raw), vec!["a", "a_2", "a_3", "a_4", "a_5"]);
    }

    /// `head`, then newline-terminated short lines until `total` bytes were
    /// produced (so a missing guard ends in `Ok`, not in a hang), counting what
    /// was pulled.
    struct Dirty {
        head: Vec<u8>,
        sent: usize,
        total: usize,
        pulled: Arc<AtomicUsize>,
    }

    impl Read for Dirty {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let mut n = 0;
            while self.sent < self.head.len() && n < buf.len() {
                buf[n] = self.head[self.sent];
                n += 1;
                self.sent += 1;
            }
            while self.sent < self.total && n + 10 <= buf.len().min(16 * 1024) {
                buf[n..n + 10].copy_from_slice(b"some text\n");
                n += 10;
                self.sent += 10;
            }
            self.pulled.fetch_add(n, Ordering::SeqCst);
            Ok(n)
        }
    }

    fn dirty(head: Vec<u8>) -> (Dirty, Arc<AtomicUsize>) {
        let pulled = Arc::new(AtomicUsize::new(0));
        let d = Dirty {
            head,
            sent: 0,
            total: 64 * 1024 * 1024,
            pulled: pulled.clone(),
        };
        (d, pulled)
    }

    /// What may be pulled before the record limit has to have fired.
    fn pull_bound() -> usize {
        SNIFF_BYTES + MAX_RECORD_BYTES + 64 * 1024
    }

    #[test]
    fn an_unclosed_quote_in_the_header_fails_fast_with_the_record_number() {
        let (src, pulled) = dirty(b"\"".to_vec());
        let err = open_csv(src, None)
            .err()
            .expect("a 64 MiB header must be refused");
        assert_eq!(
            err,
            CsvError::RecordTooLong {
                record: 1,
                limit: MAX_RECORD_BYTES
            }
        );
        assert!(pulled.load(Ordering::SeqCst) <= pull_bound());
    }

    #[test]
    fn an_unclosed_quote_inside_the_sample_fails_fast_with_the_record_number() {
        let (src, pulled) = dirty(b"a,b\n1,2\n3,4\n5,\"".to_vec());
        let err = open_csv(src, None)
            .err()
            .expect("a 64 MiB record must be refused");
        assert_eq!(
            err,
            CsvError::RecordTooLong {
                record: 4,
                limit: MAX_RECORD_BYTES
            }
        );
        assert!(pulled.load(Ordering::SeqCst) <= pull_bound());
    }

    #[test]
    fn an_unclosed_quote_after_the_sample_fails_fast_from_the_batches() {
        let mut head = b"a,b\n".to_vec();
        for _ in 0..INFERENCE_ROWS + 5 {
            head.extend_from_slice(b"1,2\n");
        }
        head.extend_from_slice(b"9,\"");
        let (src, pulled) = dirty(head);
        let results: Vec<_> = open_csv(src, None).unwrap().batches.take(1000).collect();
        let err = results.last().unwrap().as_ref().unwrap_err();
        assert!(matches!(
            err,
            CsvError::RecordTooLong {
                limit: MAX_RECORD_BYTES,
                ..
            }
        ));
        assert_eq!(results.iter().filter(|r| r.is_err()).count(), 1);
        assert!(pulled.load(Ordering::SeqCst) <= pull_bound() + 128 * 1024);
    }

    #[test]
    fn a_quoted_field_with_many_line_breaks_below_the_limit_reads_back_exactly() {
        let field: String = (0..9000).map(|i| format!("line {i}\n")).collect();
        assert!(field.len() > 60_000 && field.len() < MAX_RECORD_BYTES);
        let csv = format!("id,note\n1,\"{field}\"\n2,plain\n");
        let rows = rows_of(open(csv.as_bytes()));
        assert_eq!(rows[0], vec![cell("1"), Some(field)]);
        assert_eq!(rows[1], vec![cell("2"), cell("plain")]);
    }

    #[test]
    fn a_wide_batch_holds_what_its_cells_need_and_not_kilobytes_per_column() {
        // 700 columns and 5 rows: 3,500 cells of 3 bytes. A builder started
        // empty keeps about 5 KiB per column (3.5 MiB here).
        let cols = 700;
        let header = (0..cols)
            .map(|i| format!("c{i}"))
            .collect::<Vec<_>>()
            .join(",");
        let row = vec!["abc"; cols].join(",");
        let csv = format!("{header}\n{}", format!("{row}\n").repeat(5));
        let b = open(csv.as_bytes()).batches.next().unwrap().unwrap();
        let held = b.get_array_memory_size();
        assert!(held < 400 * 1024, "{held} bytes for 3,500 three-byte cells");
    }

    #[test]
    fn a_windows_1252_stream_is_also_counted_as_utf8_and_its_bytes_are_unchanged() {
        let p = prepared(b"name,city\nJos\xE9,S\xE3o\nZo\xEB,K\xF6ln\n");
        assert_eq!(p.encoding, Encoding::Windows1252);
        let stats = p.decode.clone();
        let _ = text_of(p);
        // Four accented single bytes: four invalid sequences, no valid one.
        assert_eq!((stats.invalid(), stats.valid_multibyte()), (4, 0));
        assert!(!stats.plausibly_utf8());
    }

    // ---- the limits cannot be wrong ----

    fn rows_csv() -> Cursor<Vec<u8>> {
        Cursor::new(b"a,b,c\n1,2,3\n4,5,6\n".to_vec())
    }

    #[test]
    fn limits_that_read_nothing_or_exceed_the_constants_are_typed_errors() {
        let d = ReadLimits::default();
        let bad = [
            ReadLimits {
                inference_rows: 0,
                ..d
            },
            ReadLimits {
                sample_bytes: 0,
                ..d
            },
            ReadLimits {
                sample_cells: 0,
                ..d
            },
            ReadLimits { batch_rows: 0, ..d },
            ReadLimits {
                batch_cells: 0,
                ..d
            },
            ReadLimits {
                batch_bytes: 0,
                ..d
            },
            // Fewer cells than a row of this file (three columns).
            ReadLimits {
                batch_cells: 2,
                ..d
            },
            ReadLimits {
                inference_rows: INFERENCE_ROWS + 1,
                ..d
            },
            ReadLimits {
                sample_bytes: SAMPLE_MAX_BYTES + 1,
                ..d
            },
            ReadLimits {
                sample_cells: SAMPLE_MAX_CELLS + 1,
                ..d
            },
            ReadLimits {
                batch_rows: BATCH_ROWS + 1,
                ..d
            },
            ReadLimits {
                batch_cells: BATCH_CELLS + 1,
                ..d
            },
            ReadLimits {
                batch_bytes: BATCH_BYTES + 1,
                ..d
            },
            ReadLimits {
                batch_bytes: usize::MAX,
                ..d
            },
        ];
        for (i, limits) in bad.iter().enumerate() {
            let err = open_csv_with(rows_csv(), None, limits).err();
            assert!(
                matches!(err, Some(CsvError::InvalidLimits(_))),
                "limits {i}: {err:?}"
            );
        }
        // The edge: exactly the constants, and exactly one row of cells.
        assert!(open_csv_with(rows_csv(), None, &d).is_ok());
        let one_row = ReadLimits {
            batch_cells: 3,
            ..d
        };
        assert!(open_csv_with(rows_csv(), None, &one_row).is_ok());
    }

    #[test]
    fn a_batch_that_fits_no_row_is_an_error_never_the_end_of_the_data() {
        // The limits are validated on opening; if they were ever changed after
        // that, no row fitting must fail, not read as an empty table.
        for tweak in [0, 1] {
            let mut o = open_csv_with(rows_csv(), None, &ReadLimits::default()).unwrap();
            if tweak == 0 {
                o.batches.limits.batch_rows = 0;
            } else {
                o.batches.limits.batch_cells = 2;
            }
            let first = o.batches.next();
            assert!(
                matches!(first, Some(Err(CsvError::InvalidLimits(_)))),
                "{tweak}: {first:?}"
            );
            assert!(o.batches.next().is_none());
        }
        // The same file with sane limits has its two rows.
        let o = open_csv_with(rows_csv(), None, &ReadLimits::default()).unwrap();
        let rows: usize = o.batches.map(|b| b.unwrap().num_rows()).sum();
        assert_eq!(rows, 2);
    }

    #[test]
    fn invisible_characters_in_a_header_are_removed_and_a_blank_name_is_named() {
        let raw = vec![
            "id\u{202E}".to_string(),
            "\u{200B}".to_string(),
            "ok".to_string(),
        ];
        assert_eq!(column_names(&raw), ["id", "column2", "ok"]);
    }

    #[test]
    fn a_persian_header_keeps_its_zero_width_non_joiner() {
        let name = "\u{645}\u{6CC}\u{200C}\u{62E}\u{648}\u{627}\u{647}\u{645}".to_string();
        assert_eq!(column_names(std::slice::from_ref(&name)), [name.as_str()]);
    }

    #[test]
    fn headers_that_differ_only_by_invisible_characters_get_the_suffix() {
        let raw: Vec<String> = ["name", "na\u{200B}me", "name\u{A0}", "\u{2800}"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(column_names(&raw), ["name", "name_2", "name_3", "column4"]);
    }
}
