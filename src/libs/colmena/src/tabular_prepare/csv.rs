//! Reading a CSV for preparation (dark behind `COLMENA_LARGE_TABULAR`).
//!
//! [`prepare_input`] turns a blocking byte source into clean UTF-8 text and
//! finds the delimiter, holding no more than one sample ([`SNIFF_BYTES`]) in
//! memory and never reading the rest ahead of the consumer: the file can be
//! far larger than memory.

use crate::tabular_prepare::scan::{RecordScanner, ScanStats};
use std::collections::HashMap;
use std::io::{self, Cursor, Read};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use thiserror::Error;

/// Bytes read up front to detect the encoding and the delimiter.
pub const SNIFF_BYTES: usize = 1024 * 1024;

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
