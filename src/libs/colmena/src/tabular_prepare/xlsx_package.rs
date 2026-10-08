//! The parts of an xlsx, read as streams through two guards.
//!
//! The archive is checked from its headers ([`crate::tabular_prepare::precheck`])
//! before anything is opened. Under every part are two guards, because that
//! check only reads headers:
//! - [`Limited`] ends a part at the size the archive declared for it. Deflate can
//!   produce more than a header says, and a byte past the declared size is
//!   [`ArchiveError::EntryTooLarge`].
//! - [`Guarded`] bounds what the XML parser can buffer. The parser copies a text
//!   node or a tag whole before it hands it over, so one 1 GiB text node would be
//!   1 GiB of memory; the guard fails the read once more than `max_token_bytes`
//!   were consumed since the last event.

use crate::tabular_prepare::precheck::{
    check_archive_with, ArchiveError, ArchiveLimits, ArchiveSummary,
};
use crate::tabular_prepare::xlsx_spool::{Invalid, Spooled, XlsxError};
use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;
use std::collections::HashMap;
use std::io::{self, BufRead, Read, Seek, SeekFrom};
use thiserror::Error;
use tokio_util::sync::CancellationToken;

/// Sheets a workbook may have. It is the number of tables a manifest holds.
pub const MAX_SHEETS: usize = crate::tabular_prepare::manifest::MAX_TABLES;

/// Times one part may be opened in a job: the sample, the run and at most three
/// restarts (`MAX_RESTARTS`). A sheet that restarts is inflated again each time, and a
/// legitimate sheet may be over a gigabyte of XML, so the limit is on reads, not on a
/// flat total of bytes that its own restarts would use up. The worst case is therefore
/// five reads of each part: at most 5 x 2.5 GiB of XML to parse in all, which the job's
/// 300 s budget (checked every 4,096 events) ends long before at any real speed.
pub const MAX_READS_PER_PART: u32 = 5;

/// Events between two looks at the cancel token, in any part.
pub const CANCEL_CHECK_EVENTS: u64 = 4096;

/// Elements open at once in any part. The deepest part this reads (a rich-text
/// shared string) is seven deep.
pub const MAX_DEPTH: usize = 32;

/// Bytes of an element's name.
pub const MAX_NAME_BYTES: usize = 256;

/// Attributes of a tag this reader will look through.
pub const MAX_ATTRIBUTES: usize = 64;

/// Declared size of the workbook part and its relationships. What is kept of them is a
/// few strings (at most 256 sheets), whatever their size; the cap bounds the time.
pub const MAX_SMALL_PART_BYTES: u64 = 16 * 1024 * 1024;

/// Declared size of the styles part. Real workbooks with style bloat have tens of
/// megabytes of it; it is parsed as a stream and what is kept is one byte for each of at
/// most 65,536 styles and a map of at most 65,536 custom formats (about 4 MiB), so the
/// cap bounds only the time.
pub const MAX_STYLES_PART_BYTES: u64 = 64 * 1024 * 1024;

/// Declared size of a worksheet part: the pre-check's per-entry limit (2 GiB),
/// within the job's running budget of inflated bytes.
pub const MAX_SHEET_PART_BYTES: u64 = crate::tabular_prepare::precheck::MAX_ENTRY_BYTES;

/// Bytes the XML parser may buffer for one event (a tag or a text node).
pub const MAX_TOKEN_BYTES: u64 = 1024 * 1024;

/// The limits of the xlsx reader. The defaults are the constants of this
/// module and of [`crate::tabular_prepare::precheck`]; tests lower them.
#[derive(Debug, Clone, Copy)]
pub struct XlsxLimits {
    pub(crate) archive: ArchiveLimits,
    pub(crate) max_token_bytes: u64,
    pub(crate) max_sheets: usize,
}

impl Default for XlsxLimits {
    fn default() -> Self {
        Self {
            archive: ArchiveLimits::default(),
            max_token_bytes: MAX_TOKEN_BYTES,
            max_sheets: MAX_SHEETS,
        }
    }
}

#[derive(Debug, Error)]
#[error("a part expands past the size its archive declares")]
struct ExpandsPastDeclared;

#[derive(Debug, Error)]
#[error("an XML element is longer than the limit")]
struct TokenTooLong;

#[derive(Debug, Error)]
#[error("a part is shorter than declared or its checksum is wrong")]
struct BadChecksum;

/// Ends a part at the size its archive declared for it, and checks its CRC-32 at
/// the end: a part that is shorter, longer or different from what the directory
/// says is an error.
pub struct Limited<R> {
    inner: R,
    left: u64,
    crc: flate2::Crc,
    expected: u32,
}

impl<R> Limited<R> {
    pub fn new(inner: R, declared: u64, crc: u32) -> Self {
        Self {
            inner,
            left: declared,
            crc: flate2::Crc::new(),
            expected: crc,
        }
    }
}

impl<R: Read> Read for Limited<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.left == 0 {
            // The declared size is read: anything more is a lie in the header.
            return match self.inner.read(&mut [0u8; 1])? {
                0 if self.crc.sum() == self.expected => Ok(0),
                0 => Err(io::Error::other(BadChecksum)),
                _ => Err(io::Error::other(ExpandsPastDeclared)),
            };
        }
        let want = buf
            .len()
            .min(usize::try_from(self.left).unwrap_or(usize::MAX));
        let n = self.inner.read(&mut buf[..want])?;
        if n == 0 {
            // Shorter than declared.
            return Err(io::Error::other(BadChecksum));
        }
        self.crc.update(&buf[..n]);
        self.left -= n as u64;
        Ok(n)
    }
}

/// A buffered reader that fails once more than `limit` bytes were consumed
/// since [`Guarded::reset`], which the parse loop calls after every event.
pub struct Guarded<R> {
    inner: R,
    buf: Vec<u8>,
    pos: usize,
    filled: usize,
    since: u64,
    limit: u64,
    /// Elements open at this point of the part.
    depth: usize,
    /// Events read so far, to look at the cancel token every [`CANCEL_CHECK_EVENTS`].
    events: u64,
    cancel: Option<CancellationToken>,
}

impl<R: Read> Guarded<R> {
    fn new(inner: R, limit: u64) -> Self {
        Self {
            inner,
            buf: vec![0; 16 * 1024],
            pos: 0,
            filled: 0,
            since: 0,
            limit,
            depth: 0,
            events: 0,
            cancel: None,
        }
    }

    pub fn reset(&mut self) {
        self.since = 0;
    }
}

impl<R: Read> Read for Guarded<R> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let n = {
            let available = self.fill_buf()?;
            let n = available.len().min(out.len());
            out[..n].copy_from_slice(&available[..n]);
            n
        };
        self.consume(n);
        Ok(n)
    }
}

impl<R: Read> BufRead for Guarded<R> {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        if self.pos >= self.filled {
            if self.since > self.limit {
                return Err(io::Error::other(TokenTooLong));
            }
            self.filled = self.inner.read(&mut self.buf)?;
            self.pos = 0;
        }
        Ok(&self.buf[self.pos..self.filled])
    }

    fn consume(&mut self, amt: usize) {
        self.pos = (self.pos + amt).min(self.filled);
        self.since += amt as u64;
    }
}

/// What an I/O error from a guarded part means.
pub fn io_failure(e: &io::Error) -> XlsxError {
    match e.get_ref() {
        Some(inner) if inner.is::<ExpandsPastDeclared>() => {
            XlsxError::Archive(ArchiveError::EntryTooLarge)
        }
        Some(inner) if inner.is::<TokenTooLong>() => XlsxError::Invalid(Invalid::TokenTooLong),
        _ => XlsxError::Invalid(Invalid::Xml),
    }
}

pub fn xml_failure(e: &quick_xml::Error) -> XlsxError {
    match e {
        quick_xml::Error::Io(io) => io_failure(io),
        _ => XlsxError::Invalid(Invalid::Xml),
    }
}

/// An opened workbook archive: what it declares and a way to read its parts.
pub struct Package {
    file: Spooled,
    /// Times each part has been opened. A part is inflated once per read, so what
    /// bounds the work is how many reads of one part are allowed.
    reads: HashMap<String, u32>,
    /// Cancelled when the job is: every part read looks at it.
    cancel: CancellationToken,
    summary: ArchiveSummary,
    max_token_bytes: u64,
    max_sheets: usize,
}

impl Package {
    /// Checks the archive against the limits and opens it. Nothing is inflated.
    pub fn open(spooled: Spooled) -> Result<Self, XlsxError> {
        Self::open_with(spooled, &XlsxLimits::default())
    }

    pub(crate) fn open_with(mut spooled: Spooled, limits: &XlsxLimits) -> Result<Self, XlsxError> {
        let summary = check_archive_with(&mut spooled, &limits.archive).map_err(|e| match e {
            ArchiveError::Io => XlsxError::Local,
            other => XlsxError::Archive(other),
        })?;
        Ok(Self {
            file: spooled,
            reads: HashMap::new(),
            cancel: CancellationToken::new(),
            summary,
            max_token_bytes: limits.max_token_bytes,
            max_sheets: limits.max_sheets,
        })
    }

    /// Makes every part read from now on stop when `cancel` is cancelled.
    pub fn set_cancel(&mut self, cancel: CancellationToken) {
        self.cancel = cancel;
    }

    pub fn max_sheets(&self) -> usize {
        self.max_sheets
    }

    pub fn summary(&self) -> &ArchiveSummary {
        &self.summary
    }

    pub fn has(&self, name: &str) -> bool {
        self.summary.entry(name).is_some()
    }

    /// The part `name` as an XML reader, whose reads are bounded by the size the
    /// archive declared for it and by the token limit.
    ///
    /// There is one reading of the archive: the one the pre-check made. The part is
    /// found at the offset it validated (its local header already compared with the
    /// central directory) and only its raw bytes are handed to the decoder, so no
    /// other directory is ever discovered or allocated.
    pub fn xml(&mut self, name: &str, max_declared: u64) -> Result<XmlPart<'_>, XlsxError> {
        let entry = self
            .summary
            .entry(name)
            .ok_or(XlsxError::Invalid(Invalid::Xml))?
            .clone();
        if entry.uncompressed > max_declared {
            return Err(XlsxError::Archive(ArchiveError::EntryTooLarge));
        }
        let reads = self.reads.entry(name.to_string()).or_insert(0);
        *reads += 1;
        if *reads > MAX_READS_PER_PART {
            return Err(XlsxError::Archive(ArchiveError::TotalTooLarge));
        }
        self.file
            .seek(SeekFrom::Start(entry.data_offset))
            .map_err(|_| XlsxError::Local)?;
        let raw = (&mut self.file).take(entry.compressed);
        let inner: Box<dyn Read + '_> = if entry.method == 0 {
            Box::new(raw)
        } else {
            Box::new(flate2::read::DeflateDecoder::new(raw))
        };
        let limited = Limited::new(inner, entry.uncompressed, entry.crc);
        let mut guarded = Guarded::new(limited, self.max_token_bytes);
        guarded.cancel = Some(self.cancel.clone());
        Ok(Reader::from_reader(guarded))
    }
}

/// The next event of a part. The guard is reset once the event is read, so it
/// bounds each event and not the part.
pub fn next_event<'b, R: Read>(
    reader: &mut Reader<Guarded<R>>,
    buf: &'b mut Vec<u8>,
) -> Result<Event<'b>, XlsxError> {
    buf.clear();
    let event = reader.read_event_into(buf).map_err(|e| xml_failure(&e))?;
    let guard = reader.get_mut();
    guard.reset();
    // A part of millions of comments or of elements that are not cells has no cell
    // to look at the token on: look every so many events.
    guard.events += 1;
    if guard.events.is_multiple_of(CANCEL_CHECK_EVENTS)
        && guard.cancel.as_ref().is_some_and(|c| c.is_cancelled())
    {
        return Err(XlsxError::Cancelled);
    }
    match &event {
        Event::Start(e) | Event::Empty(e) if e.name().as_ref().len() > MAX_NAME_BYTES => {
            return Err(XlsxError::Invalid(Invalid::TokenTooLong));
        }
        Event::Start(_) => {
            // The parser keeps every open element's name until it is closed, so
            // what it holds is bounded by the depth times the name length.
            guard.depth += 1;
            if guard.depth > MAX_DEPTH {
                return Err(XlsxError::Invalid(Invalid::TooDeep));
            }
        }
        Event::End(_) => guard.depth = guard.depth.saturating_sub(1),
        _ => {}
    }
    Ok(event)
}

/// Raw XML bytes as text: UTF-8 (what the format requires) with the predefined
/// and numeric entities resolved. `quick-xml`'s own helpers are not available
/// here because another crate turns its `encoding` feature on.
pub fn text_of(raw: &[u8]) -> Result<std::borrow::Cow<'_, str>, XlsxError> {
    let text = std::str::from_utf8(raw).map_err(|_| XlsxError::Invalid(Invalid::Xml))?;
    quick_xml::escape::unescape(text).map_err(|_| XlsxError::Invalid(Invalid::Xml))
}

/// The value of the attribute whose local name is `local` (`id` finds `r:id`).
pub fn attribute(e: &BytesStart, local: &[u8]) -> Result<Option<String>, XlsxError> {
    // The parser's duplicate-attribute check compares each attribute with every
    // earlier one (quadratic in a tag with tens of thousands); the count is capped
    // instead and the check turned off.
    let mut attributes = e.attributes();
    attributes.with_checks(false);
    for (i, attr) in attributes.enumerate() {
        if i >= MAX_ATTRIBUTES {
            return Err(XlsxError::Invalid(Invalid::TooManyAttributes));
        }
        let attr = attr.map_err(|_| XlsxError::Invalid(Invalid::Xml))?;
        if attr.key.local_name().as_ref() == local {
            return text_of(&attr.value).map(|v| Some(v.into_owned()));
        }
    }
    Ok(None)
}

/// A part of the workbook, parsed as XML.
pub type XmlPart<'a> = Reader<Guarded<Limited<Box<dyn Read + 'a>>>>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::domain::StorageError;
    use crate::tabular_prepare::xlsx_spool::{spool_stream, MAX_XLSX_BYTES};
    use crate::tabular_prepare::zipfix::{build, Entry};
    use bytes::Bytes;
    use quick_xml::events::Event;
    use std::io::Cursor;
    use std::path::Path;
    use std::sync::atomic::AtomicU64;
    use tokio_util::sync::CancellationToken;

    async fn spool(
        dir: &Path,
        chunks: Vec<Result<Bytes, StorageError>>,
        declared: Option<u64>,
        cap: u64,
    ) -> Result<Spooled, XlsxError> {
        let read = AtomicU64::new(0);
        let stream = Box::pin(futures::stream::iter(chunks));
        spool_stream(dir, stream, declared, cap, &CancellationToken::new(), &read).await
    }

    #[test]
    fn a_part_ends_at_the_size_its_archive_declared() {
        use crate::tabular_prepare::zipfix::crc32;
        let ones = |n: usize| Cursor::new(vec![1u8; n]);
        let crc = crc32(&[1u8; 10]);
        let mut exact = Limited::new(ones(10), 10, crc);
        let mut out = Vec::new();
        exact.read_to_end(&mut out).unwrap();
        assert_eq!(out.len(), 10);
        // One byte more than declared is a lie, whatever the reads look like.
        let e = Limited::new(ones(11), 10, crc)
            .read_to_end(&mut out)
            .unwrap_err();
        assert_eq!(
            io_failure(&e),
            XlsxError::Archive(ArchiveError::EntryTooLarge)
        );
        // Shorter than declared, or with the wrong checksum, is a corrupt part.
        let e = Limited::new(ones(9), 10, crc)
            .read_to_end(&mut out)
            .unwrap_err();
        assert_eq!(io_failure(&e), XlsxError::Invalid(Invalid::Xml));
        let e = Limited::new(ones(10), 10, crc ^ 1)
            .read_to_end(&mut out)
            .unwrap_err();
        assert_eq!(io_failure(&e), XlsxError::Invalid(Invalid::Xml));
    }

    #[test]
    fn the_xml_guard_fails_a_token_over_the_limit_and_passes_many_small_ones() {
        let small = format!("<a>{}</a>", "<b>x</b>".repeat(5000));
        let mut reader = Reader::from_reader(Guarded::new(Cursor::new(small.into_bytes()), 100));
        let mut buf = Vec::new();
        let mut events = 0;
        loop {
            match reader.read_event_into(&mut buf).unwrap() {
                Event::Eof => break,
                _ => events += 1,
            }
            reader.get_mut().reset();
            buf.clear();
        }
        assert!(events > 10_000);
        // One text node of 50 KiB with a limit of 4 KiB: the parser never buffers it.
        let big = format!("<a>{}</a>", "x".repeat(50_000));
        let mut reader = Reader::from_reader(Guarded::new(Cursor::new(big.into_bytes()), 4096));
        let mut buf = Vec::new();
        let err = loop {
            match reader.read_event_into(&mut buf) {
                Err(e) => break e,
                Ok(Event::Eof) => panic!("the big node was read"),
                Ok(_) => {}
            }
            reader.get_mut().reset();
            buf.clear();
        };
        assert_eq!(xml_failure(&err), XlsxError::Invalid(Invalid::TokenTooLong));
        assert!(buf.len() < 4096 + 16 * 1024);
    }

    async fn package_of(bytes: Vec<u8>, limits: &XlsxLimits) -> Result<Package, XlsxError> {
        let dir = tempfile::tempdir().unwrap();
        let chunks = vec![Ok(Bytes::from(bytes))];
        let spooled = spool(dir.path(), chunks, None, MAX_XLSX_BYTES).await?;
        Package::open_with(spooled, limits)
    }

    #[tokio::test]
    async fn a_package_opens_after_its_pre_check_and_refuses_what_the_pre_check_refuses() {
        let limits = XlsxLimits::default();
        let good = build(&[Entry::stored("a.xml", b"<a><b>hi</b></a>")]);
        let mut pkg = package_of(good, &limits).await.unwrap();
        assert!(pkg.has("a.xml") && !pkg.has("b.xml"));
        let mut part = pkg.xml("a.xml", MAX_SMALL_PART_BYTES).unwrap();
        let mut buf = Vec::new();
        assert!(matches!(
            part.read_event_into(&mut buf).unwrap(),
            Event::Start(_)
        ));
        drop(part);
        assert!(pkg.xml("b.xml", MAX_SMALL_PART_BYTES).is_err());
        let bad = build(&[Entry::stored("../a.xml", b"x")]);
        let r = package_of(bad, &limits).await;
        assert!(matches!(r, Err(XlsxError::Archive(ArchiveError::BadName))));
        let r = package_of(b"not a zip at all".to_vec(), &limits).await;
        assert!(matches!(
            r,
            Err(XlsxError::Archive(ArchiveError::NotAnArchive))
        ));
    }

    #[tokio::test]
    async fn a_part_that_inflates_past_its_declared_size_fails_when_it_is_read() {
        // A deflated part of 100,000 identical bytes whose headers (both of them,
        // so the pre-check agrees) declare 50,000.
        let mut zip_bytes = Vec::new();
        {
            let mut w = zip::ZipWriter::new(Cursor::new(&mut zip_bytes));
            let options = zip::write::FileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            w.start_file("a.xml", options).unwrap();
            std::io::Write::write_all(&mut w, format!("<a>{}</a>", "x".repeat(99_990)).as_bytes())
                .unwrap();
            w.finish().unwrap();
        }
        let local = zip_bytes
            .windows(4)
            .position(|w| w == b"PK\x03\x04")
            .unwrap();
        let central = zip_bytes
            .windows(4)
            .position(|w| w == b"PK\x01\x02")
            .unwrap();
        zip_bytes[local + 22..local + 26].copy_from_slice(&50_000u32.to_le_bytes());
        zip_bytes[central + 24..central + 28].copy_from_slice(&50_000u32.to_le_bytes());
        let mut pkg = package_of(zip_bytes, &XlsxLimits::default()).await.unwrap();
        let mut part = pkg.xml("a.xml", MAX_SMALL_PART_BYTES).unwrap();
        let mut buf = Vec::new();
        let err = loop {
            match part.read_event_into(&mut buf) {
                Err(e) => break e,
                Ok(Event::Eof) => panic!("the lie was not caught"),
                Ok(_) => {}
            }
            part.get_mut().reset();
            buf.clear();
        };
        // The parser buffers the text node first; the declared size stops it.
        assert!(matches!(
            xml_failure(&err),
            XlsxError::Archive(ArchiveError::EntryTooLarge)
                | XlsxError::Invalid(Invalid::TokenTooLong)
        ));
    }

    #[tokio::test]
    async fn a_part_may_be_read_five_times_the_sample_the_run_and_three_restarts() {
        let bytes = build(&[Entry::stored("a.xml", b"<a/>")]);
        let mut pkg = package_of(bytes, &XlsxLimits::default()).await.unwrap();
        for _ in 0..MAX_READS_PER_PART {
            assert!(pkg.xml("a.xml", MAX_SMALL_PART_BYTES).is_ok());
        }
        assert_eq!(
            pkg.xml("a.xml", MAX_SMALL_PART_BYTES).err(),
            Some(XlsxError::Archive(ArchiveError::TotalTooLarge))
        );
        // Another part has reads of its own.
        let bytes = build(&[
            Entry::stored("a.xml", b"<a/>"),
            Entry::stored("b.xml", b"<b/>"),
        ]);
        let mut pkg = package_of(bytes, &XlsxLimits::default()).await.unwrap();
        for _ in 0..MAX_READS_PER_PART {
            pkg.xml("a.xml", MAX_SMALL_PART_BYTES).unwrap();
        }
        assert!(pkg.xml("b.xml", MAX_SMALL_PART_BYTES).is_ok());
        // The reads a conversion makes of one sheet (a sample, a run, three restarts) fit.
        assert_eq!(
            MAX_READS_PER_PART as usize,
            2 + crate::tabular_prepare::convert::MAX_RESTARTS
        );
    }

    #[tokio::test]
    async fn a_part_of_millions_of_comments_stops_within_a_bounded_number_of_events() {
        let body = format!("<a>{}</a>", "<!-- -->".repeat(50_000));
        let bytes = build(&[Entry::stored("a.xml", body.as_bytes())]);
        let drain = |pkg: &mut Package| -> Result<u64, XlsxError> {
            let mut part = pkg.xml("a.xml", MAX_SMALL_PART_BYTES)?;
            let mut buf = Vec::new();
            let mut events = 0u64;
            loop {
                if matches!(next_event(&mut part, &mut buf)?, Event::Eof) {
                    return Ok(events);
                }
                events += 1;
            }
        };
        let mut pkg = package_of(bytes.clone(), &XlsxLimits::default())
            .await
            .unwrap();
        assert_eq!(drain(&mut pkg).unwrap(), 50_002);
        // Cancelled, no cell and no row in the part to look at the token on: it stops at
        // the first check.
        let mut pkg = package_of(bytes, &XlsxLimits::default()).await.unwrap();
        let cancel = CancellationToken::new();
        cancel.cancel();
        pkg.set_cancel(cancel);
        assert_eq!(drain(&mut pkg), Err(XlsxError::Cancelled));
    }
}
