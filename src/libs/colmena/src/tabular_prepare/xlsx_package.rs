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
use std::io::{self, BufRead, Read, Seek, SeekFrom};
use thiserror::Error;
use zip::ZipArchive;

/// Sheets a workbook may have. It is the number of tables a manifest holds.
pub const MAX_SHEETS: usize = crate::tabular_prepare::manifest::MAX_TABLES;

/// Bytes the XML parser may buffer for one event (a tag or a text node).
pub const MAX_TOKEN_BYTES: u64 = 1024 * 1024;

/// The limits of the xlsx reader. The defaults are the constants of this
/// module and of [`crate::tabular_prepare::precheck`]; tests lower them.
#[derive(Debug, Clone, Copy)]
pub(crate) struct XlsxLimits {
    pub archive: ArchiveLimits,
    pub max_token_bytes: u64,
    pub max_sheets: usize,
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

/// Ends a part at the size its archive declared for it.
pub struct Limited<R> {
    inner: R,
    left: u64,
}

impl<R: Read> Read for Limited<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.left == 0 {
            // The declared size is read: anything more is a lie in the header.
            return match self.inner.read(&mut [0u8; 1])? {
                0 => Ok(0),
                _ => Err(io::Error::other(ExpandsPastDeclared)),
            };
        }
        let want = buf
            .len()
            .min(usize::try_from(self.left).unwrap_or(usize::MAX));
        let n = self.inner.read(&mut buf[..want])?;
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
    archive: ZipArchive<Spooled>,
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
        spooled
            .seek(SeekFrom::Start(0))
            .map_err(|_| XlsxError::Local)?;
        let archive = ZipArchive::new(spooled).map_err(|_| XlsxError::Invalid(Invalid::Xml))?;
        Ok(Self {
            archive,
            summary,
            max_token_bytes: limits.max_token_bytes,
            max_sheets: limits.max_sheets,
        })
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
    pub fn xml(&mut self, name: &str) -> Result<XmlPart<'_>, XlsxError> {
        let declared = self
            .summary
            .entry(name)
            .ok_or(XlsxError::Invalid(Invalid::Xml))?
            .uncompressed;
        let file = self
            .archive
            .by_name(name)
            .map_err(|_| XlsxError::Invalid(Invalid::Xml))?;
        let limited = Limited {
            inner: file,
            left: declared,
        };
        Ok(Reader::from_reader(Guarded::new(
            limited,
            self.max_token_bytes,
        )))
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
    reader.get_mut().reset();
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
    for attr in e.attributes() {
        let attr = attr.map_err(|_| XlsxError::Invalid(Invalid::Xml))?;
        if attr.key.local_name().as_ref() == local {
            return text_of(&attr.value).map(|v| Some(v.into_owned()));
        }
    }
    Ok(None)
}

/// A part of the workbook, parsed as XML.
pub type XmlPart<'a> = Reader<Guarded<Limited<zip::read::ZipFile<'a>>>>;

#[cfg(test)]
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
        let mut exact = Limited {
            inner: Cursor::new(vec![1u8; 10]),
            left: 10,
        };
        let mut out = Vec::new();
        exact.read_to_end(&mut out).unwrap();
        assert_eq!(out.len(), 10);
        // One byte more than declared is a lie, whatever the reads look like.
        let mut over = Limited {
            inner: Cursor::new(vec![1u8; 11]),
            left: 10,
        };
        let e = over.read_to_end(&mut out).unwrap_err();
        assert_eq!(
            io_failure(&e),
            XlsxError::Archive(ArchiveError::EntryTooLarge)
        );
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
        let mut part = pkg.xml("a.xml").unwrap();
        let mut buf = Vec::new();
        assert!(matches!(
            part.read_event_into(&mut buf).unwrap(),
            Event::Start(_)
        ));
        drop(part);
        assert!(pkg.xml("b.xml").is_err());
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
        let mut part = pkg.xml("a.xml").unwrap();
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
}
