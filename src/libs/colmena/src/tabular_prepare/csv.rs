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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tabular_prepare::scan::MAX_RECORD_BYTES;
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
    fn the_sample_count_counts_sequences_not_bytes_and_a_cut_end_only_when_it_is_the_end() {
        assert_eq!(count_utf8("é日".as_bytes(), true), (2, 0));
        assert_eq!(count_utf8(b"a\xFF\xC3\xA9", true), (1, 1));
        // A character cut by a sample limit is not invalid; at the real end it is.
        assert_eq!(count_utf8(b"ab\xE2\x82", false), (0, 0));
        assert_eq!(count_utf8(b"ab\xE2\x82", true), (0, 1));
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
}
