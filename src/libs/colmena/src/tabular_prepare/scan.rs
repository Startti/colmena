//! Record scanner: bounds a CSV record and settles blank lines (dark behind
//! `COLMENA_LARGE_TABULAR`).
//!
//! It sits between the decoded text and the CSV parser and follows the
//! parser's own quoting rules (a quote opens a quoted field only at the start
//! of a field, `""` is a quote inside one, a quoted field may hold line
//! breaks), so it knows where a record really ends:
//!
//! - A record longer than [`MAX_RECORD_BYTES`] fails with
//!   [`CsvError::RecordTooLong`] naming the record (never its content). This
//!   holds inside an unclosed quote, where every physical line is short but the
//!   record never ends and a parser would buffer the whole file. It is what
//!   bounds the parser's memory, in the header, in the sample and after it.
//! - Blank lines are never skipped silently. After the header, a blank line in
//!   a one-column file is a null row (written as `""`), because there the empty
//!   line is the empty cell; trailing blank lines at the end of the file are
//!   dropped. In a file with several columns a blank line is dropped. Every
//!   blank line is counted in [`ScanStats`], and blank lines before the header
//!   are dropped.

use crate::tabular_prepare::csv::CsvError;
use std::io::{self, Read};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Longest CSV record, in bytes of decoded text, quotes and delimiters
/// included. A spreadsheet row of more than a mebibyte is not a row; the limit
/// is also what keeps one record from filling a batch (see the batch budget in
/// `csv.rs`).
pub const MAX_RECORD_BYTES: usize = 1024 * 1024;

const CHUNK: usize = 16 * 1024;

/// How many `""` rows are produced per refill when blank lines are owed.
const OWED_PER_REFILL: u64 = 4096;

/// What the scanner saw, shared with the caller.
#[derive(Debug, Default)]
pub struct ScanStats {
    records: AtomicU64,
    blank_dropped: AtomicU64,
    blank_rows: AtomicU64,
    padded_rows: AtomicU64,
}

impl ScanStats {
    /// Records with content (the header included).
    pub fn records(&self) -> u64 {
        self.records.load(Ordering::Relaxed)
    }
    /// Blank lines that were dropped (before the header, trailing, or in a
    /// file with several columns).
    pub fn blank_dropped(&self) -> u64 {
        self.blank_dropped.load(Ordering::Relaxed)
    }
    /// Rows with fewer fields than the header that were padded with nulls
    /// (counted by the batch reader, not the scanner).
    pub fn padded_rows(&self) -> u64 {
        self.padded_rows.load(Ordering::Relaxed)
    }
    /// Blank lines in a one-column file that became null rows.
    pub fn blank_rows(&self) -> u64 {
        self.blank_rows.load(Ordering::Relaxed)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    FieldStart,
    Unquoted,
    Quoted,
    QuoteSeen,
}

/// What a line feed that follows a carriage return does.
#[derive(Clone, Copy, PartialEq, Eq)]
enum AfterCr {
    No,
    /// The carriage return ended a record: pass the line feed on.
    Keep,
    /// The carriage return was a blank line: drop the line feed too.
    Drop,
}

pub struct RecordScanner<R> {
    inner: R,
    delimiter: u8,
    blank_as_null: bool,
    stats: Arc<ScanStats>,
    state: State,
    record_bytes: usize,
    record_no: u64,
    after_cr: AfterCr,
    seen_record: bool,
    /// Blank lines after the header, waiting for the next record to decide
    /// whether they are rows (one column) or trailing.
    pending_blank: u64,
    owed: u64,
    input: Vec<u8>,
    in_pos: usize,
    in_len: usize,
    out: Vec<u8>,
    out_pos: usize,
    eof: bool,
}

impl<R: Read> RecordScanner<R> {
    pub fn new(inner: R, delimiter: u8, blank_as_null: bool, stats: Arc<ScanStats>) -> Self {
        Self {
            inner,
            delimiter,
            blank_as_null,
            stats,
            state: State::FieldStart,
            record_bytes: 0,
            record_no: 0,
            after_cr: AfterCr::No,
            seen_record: false,
            pending_blank: 0,
            owed: 0,
            input: vec![0; CHUNK],
            in_pos: 0,
            in_len: 0,
            out: Vec::with_capacity(CHUNK),
            out_pos: 0,
            eof: false,
        }
    }

    fn end_record(&mut self) {
        self.stats.records.fetch_add(1, Ordering::Relaxed);
        self.record_no += 1;
        self.seen_record = true;
        self.state = State::FieldStart;
        self.record_bytes = 0;
    }

    fn blank_line(&mut self) {
        if self.seen_record && self.blank_as_null {
            self.pending_blank += 1;
        } else {
            self.stats.blank_dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Consumes input into `out` until `out` is full, the input is used up or
    /// blank lines are owed first.
    fn process(&mut self) -> io::Result<()> {
        while self.in_pos < self.in_len && self.out.len() < CHUNK {
            let b = self.input[self.in_pos];
            let terminator = b == b'\n' || b == b'\r';
            // A line feed completing a CRLF belongs to the record before it.
            if b == b'\n' && self.after_cr != AfterCr::No {
                if self.after_cr == AfterCr::Keep {
                    self.out.push(b);
                }
                self.after_cr = AfterCr::No;
                self.in_pos += 1;
                continue;
            }
            self.after_cr = AfterCr::No;
            let at_start = self.state == State::FieldStart && self.record_bytes == 0;
            if at_start && !terminator && self.pending_blank > 0 {
                self.owed = self.pending_blank;
                self.pending_blank = 0;
                return Ok(());
            }
            self.in_pos += 1;
            if terminator && self.state != State::Quoted {
                if at_start {
                    self.blank_line();
                    if b == b'\r' {
                        self.after_cr = AfterCr::Drop;
                    }
                } else {
                    self.out.push(b);
                    if b == b'\r' {
                        self.after_cr = AfterCr::Keep;
                    }
                    self.end_record();
                }
                continue;
            }
            self.out.push(b);
            self.record_bytes += 1;
            if self.record_bytes > MAX_RECORD_BYTES {
                return Err(CsvError::RecordTooLong {
                    record: self.record_no + 1,
                    limit: MAX_RECORD_BYTES,
                }
                .into_io());
            }
            self.state = match (self.state, b) {
                (State::FieldStart, b'"') => State::Quoted,
                (State::Quoted, b'"') => State::QuoteSeen,
                (State::QuoteSeen, b'"') => State::Quoted,
                (State::Quoted, _) => State::Quoted,
                (_, d) if d == self.delimiter => State::FieldStart,
                _ => State::Unquoted,
            };
            if terminator {
                // A line break inside quotes: still the same record.
            }
        }
        Ok(())
    }
}

impl<R: Read> Read for RecordScanner<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            if self.out_pos < self.out.len() {
                let n = buf.len().min(self.out.len() - self.out_pos);
                buf[..n].copy_from_slice(&self.out[self.out_pos..self.out_pos + n]);
                self.out_pos += n;
                return Ok(n);
            }
            self.out.clear();
            self.out_pos = 0;
            if self.owed > 0 {
                let k = self.owed.min(OWED_PER_REFILL);
                for _ in 0..k {
                    self.out.extend_from_slice(b"\"\"\n");
                }
                self.stats.blank_rows.fetch_add(k, Ordering::Relaxed);
                self.owed -= k;
                continue;
            }
            if self.in_pos < self.in_len {
                self.process()?;
                continue;
            }
            if self.eof {
                return Ok(0);
            }
            let n = self.inner.read(&mut self.input)?;
            self.in_pos = 0;
            self.in_len = n;
            if n == 0 {
                self.eof = true;
                // An unterminated last record still counts; blank lines owed
                // at the end are trailing and dropped.
                if self.record_bytes > 0 {
                    self.end_record();
                }
                let trailing = std::mem::take(&mut self.pending_blank);
                self.stats
                    .blank_dropped
                    .fetch_add(trailing, Ordering::Relaxed);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Read};

    fn scan_with(bytes: &[u8], delimiter: u8, blank_as_null: bool) -> (Vec<u8>, Arc<ScanStats>) {
        let stats = Arc::new(ScanStats::default());
        let mut s = RecordScanner::new(
            Cursor::new(bytes.to_vec()),
            delimiter,
            blank_as_null,
            stats.clone(),
        );
        let mut out = Vec::new();
        s.read_to_end(&mut out).unwrap();
        (out, stats)
    }

    fn scan(bytes: &[u8], delimiter: u8) -> (Vec<u8>, Arc<ScanStats>) {
        scan_with(bytes, delimiter, false)
    }

    fn text(bytes: &[u8], delimiter: u8) -> String {
        String::from_utf8(scan(bytes, delimiter).0).unwrap()
    }

    #[test]
    fn ordinary_text_passes_through_unchanged() {
        let csv = "a,b\r\n1,\"x,\"\"y\"\"\nz\"\n3,4\rlone cr,5\n";
        assert_eq!(text(csv.as_bytes(), b','), csv);
    }

    #[test]
    fn records_are_counted_without_blank_lines() {
        let csv = "a,b\r\n1,\"x,\"\"y\"\"\nz\"\n3,4\rlone cr,5\n";
        assert_eq!(scan(csv.as_bytes(), b',').1.records(), 4);
        assert_eq!(scan(b"a\n\n\r\nb\n\n", b',').1.records(), 2);
    }

    #[test]
    fn an_escaped_quote_does_not_end_the_quoted_field() {
        // `""` inside a quoted field is a quote, so the field stays open and
        // the limit still counts the lines that follow.
        let mut csv = b"a\n\"x\"\"y\n".to_vec();
        for _ in 0..MAX_RECORD_BYTES / 10 + 10 {
            csv.extend_from_slice(b"some text\n");
        }
        let mut s = RecordScanner::new(Cursor::new(csv), b',', false, Arc::default());
        assert!(s.read_to_end(&mut Vec::new()).is_err());
    }

    #[test]
    fn a_quote_inside_an_unquoted_field_is_literal_like_the_parser_treats_it() {
        // Many lines with a 5" in the middle of a field: none opens a quoted
        // field, so no record is longer than a line.
        let line = "a 5\" pipe,b\n";
        let csv = line.repeat(2 * MAX_RECORD_BYTES / line.len());
        let (out, _) = scan(csv.as_bytes(), b',');
        assert_eq!(out.len(), csv.len());
    }

    #[test]
    fn a_record_over_the_limit_is_an_error_naming_the_record() {
        let mut csv = b"a,b\n1,2\n3,\"".to_vec();
        csv.extend(std::iter::repeat_n(b'x', MAX_RECORD_BYTES + 1));
        let mut s = RecordScanner::new(Cursor::new(csv), b',', false, Arc::default());
        let err = s.read_to_end(&mut Vec::new()).unwrap_err();
        assert_eq!(
            CsvError::from_io(err),
            CsvError::RecordTooLong {
                record: 3,
                limit: MAX_RECORD_BYTES
            }
        );
    }

    #[test]
    fn the_limit_holds_inside_an_unclosed_quote_across_many_lines() {
        // `a,b\n"` then newline-terminated lines for ever: every physical line
        // is short, but the record never ends.
        struct Lines(usize);
        impl Read for Lines {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                let head: &[u8] = if self.0 == 0 { b"a,b\n\"" } else { b"" };
                let mut n = 0;
                for &b in head {
                    buf[n] = b;
                    n += 1;
                }
                while n + 10 <= buf.len().min(16 * 1024) {
                    buf[n..n + 10].copy_from_slice(b"some text\n");
                    n += 10;
                }
                self.0 += n;
                Ok(n)
            }
        }
        let mut s = RecordScanner::new(Lines(0), b',', false, Arc::default());
        let mut sink = vec![0u8; 8192];
        let mut read = 0usize;
        let err = loop {
            match s.read(&mut sink) {
                Ok(0) => panic!("no end expected"),
                Ok(n) => {
                    read += n;
                    assert!(read < 64 * 1024 * 1024, "the limit never fired");
                }
                Err(e) => break e,
            }
        };
        assert!(matches!(
            CsvError::from_io(err),
            CsvError::RecordTooLong { record: 2, .. }
        ));
        // Nothing past the limit plus a read buffer was let through.
        assert!(read <= MAX_RECORD_BYTES + 32 * 1024, "{read}");
    }

    #[test]
    fn a_record_of_exactly_the_limit_is_accepted() {
        let mut csv = b"a\n\"".to_vec();
        csv.extend(std::iter::repeat_n(b'x', MAX_RECORD_BYTES - 2));
        csv.extend_from_slice(b"\"\nb\n");
        assert_eq!(scan(&csv, b',').0, csv);
        let mut over = b"a\n\"".to_vec();
        over.extend(std::iter::repeat_n(b'x', MAX_RECORD_BYTES - 1));
        over.extend_from_slice(b"\"\nb\n");
        let mut s = RecordScanner::new(Cursor::new(over), b',', false, Arc::default());
        assert!(s.read_to_end(&mut Vec::new()).is_err());
    }

    #[test]
    fn the_output_does_not_depend_on_how_the_input_is_chunked() {
        let csv = "\nh1,h2\r\n\"a\"\"b\",\"c\nd\"\r\n\r\n5\" x,é\rlone\n\n\"\"\n\n".as_bytes();
        for blank_as_null in [false, true] {
            let (whole, whole_stats) = scan_with(csv, b',', blank_as_null);
            struct Dribble<'a>(&'a [u8], usize);
            impl Read for Dribble<'_> {
                fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                    let n = self.1.min(self.0.len()).min(buf.len());
                    buf[..n].copy_from_slice(&self.0[..n]);
                    self.0 = &self.0[n..];
                    Ok(n)
                }
            }
            for step in [1usize, 2, 3, 7] {
                let mut s =
                    RecordScanner::new(Dribble(csv, step), b',', blank_as_null, Arc::default());
                let mut out = Vec::new();
                s.read_to_end(&mut out).unwrap();
                assert_eq!(out, whole, "step {step}, blank_as_null {blank_as_null}");
            }
            for cut in 0..csv.len() {
                let (a, b) = csv.split_at(cut);
                let stats = Arc::new(ScanStats::default());
                let src = Cursor::new(a.to_vec()).chain(Cursor::new(b.to_vec()));
                let mut s = RecordScanner::new(src, b',', blank_as_null, stats.clone());
                let mut out = Vec::new();
                s.read_to_end(&mut out).unwrap();
                assert_eq!(out, whole, "cut {cut}, blank_as_null {blank_as_null}");
                assert_eq!(
                    (stats.records(), stats.blank_rows(), stats.blank_dropped()),
                    (
                        whole_stats.records(),
                        whole_stats.blank_rows(),
                        whole_stats.blank_dropped()
                    ),
                    "cut {cut}"
                );
            }
        }
    }

    #[test]
    fn a_different_delimiter_changes_what_starts_a_quote() {
        // With a semicolon separator a quote after a comma is mid-field.
        let csv = "a;b\n1,\"x;2\ny;z\n";
        assert_eq!(text(csv.as_bytes(), b';'), csv);
        // With a comma separator the same quote opens a field that swallows lines.
        let stats = scan(csv.as_bytes(), b',').1;
        assert_eq!(stats.records(), 2);
    }

    #[test]
    fn blank_lines_are_dropped_and_counted_in_a_multi_column_file() {
        let (out, stats) = scan(b"\n\na,b\n1,2\n\n3,4\r\n\r\n5,6\n\n\n", b',');
        assert_eq!(out, b"a,b\n1,2\n3,4\r\n5,6\n");
        assert_eq!(stats.blank_dropped(), 6);
        assert_eq!(stats.blank_rows(), 0);
    }

    #[test]
    fn a_blank_line_in_a_one_column_file_is_a_null_row_and_trailing_ones_are_not() {
        let (out, stats) = scan_with(b"\nname\nann\n\nbob\n\n\ncy\n\n\n", b',', true);
        // Three blank lines inside the data become explicit empty cells; the
        // leading blank line and the trailing ones are dropped.
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "name\nann\n\"\"\nbob\n\"\"\n\"\"\ncy\n"
        );
        assert_eq!(stats.blank_rows(), 3);
        assert_eq!(stats.blank_dropped(), 3);
    }

    #[test]
    fn a_blank_line_inside_quotes_is_data_not_a_blank_line() {
        let csv = "a\n\"x\n\ny\"\n\"q\"\"\n\nr\"\nb\n";
        let (out, stats) = scan_with(csv.as_bytes(), b',', true);
        assert_eq!(String::from_utf8(out).unwrap(), csv);
        assert_eq!((stats.blank_rows(), stats.blank_dropped()), (0, 0));
    }

    #[test]
    fn many_blank_lines_do_not_buffer() {
        // Fifty thousand blank lines between two rows: the output is produced
        // in small pieces while reading, never as one allocation.
        let csv = format!("a\n1\n{}2\n", "\n".repeat(50_000));
        let stats = Arc::new(ScanStats::default());
        let mut s = RecordScanner::new(Cursor::new(csv.into_bytes()), b',', true, stats.clone());
        let mut buf = [0u8; 4096];
        let (mut total, mut biggest, mut held) = (0, 0, 0);
        loop {
            let n = s.read(&mut buf).unwrap();
            if n == 0 {
                break;
            }
            total += n;
            biggest = biggest.max(n);
            held = held.max(s.out.capacity());
        }
        // The internal buffer stays near one refill, not one per blank line.
        assert!(held <= 64 * 1024, "held {held} bytes");
        assert_eq!(total, "a\n1\n".len() + 50_000 * 3 + "2\n".len());
        assert!(biggest <= 4096);
        assert_eq!(stats.blank_rows(), 50_000);
    }
}
