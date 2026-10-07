//! Pre-check of an xlsx archive, from its headers alone. Nothing is inflated.
//!
//! An xlsx is a zip. Before any entry is opened, the end-of-central-directory
//! record, the central directory and the local header of every entry are read
//! and checked against limits (`ArchiveLimits`), so a zip bomb is refused for
//! the cost of a few small reads whatever it would expand to. Reads are bounded
//! by constants: the last 64 KiB plus 22 bytes, a central directory of at most
//! 8 MiB, and one 30-byte header plus a name (at most 512 bytes) per entry.
//!
//! The limits are the measured ones (spike item 6, generated files only):
//! entries up to 2 GiB, 2.5 GiB in all, a compressed/uncompressed ratio of at
//! least 1 % for any entry above 1 MiB (the lowest ratio of a real workbook was
//! 8.2 %, a bomb 0.097 %). Entry count (10,000) and central-directory size are
//! estimates, not prototyped. Headers that disagree are refused: the central
//! directory is what the reader trusts, so a local header that says something
//! else is a lie about the size.
//!
//! Zip64, encryption, several disks and methods other than stored and deflate
//! are refused as unsupported. No workbook under these limits needs zip64: its
//! sizes are above 4 GiB, which the caps forbid.

use std::collections::HashSet;
use std::io::{self, Read, Seek, SeekFrom};
use thiserror::Error;

const GIB: u64 = 1024 * 1024 * 1024;
const MIB: u64 = 1024 * 1024;

/// Entries an archive may have.
pub const MAX_ENTRIES: usize = 10_000;
/// Bytes of central directory read into memory.
pub const MAX_CENTRAL_DIR_BYTES: u64 = 8 * MIB;
/// Bytes of an entry name.
pub const MAX_NAME_BYTES: usize = 512;
/// Uncompressed bytes of one entry.
pub const MAX_ENTRY_BYTES: u64 = 2 * GIB;
/// Uncompressed bytes of all entries together (2.5 GiB).
pub const MAX_TOTAL_BYTES: u64 = 2 * GIB + GIB / 2;
/// An entry up to this size is exempt from the ratio rule.
pub const RATIO_GRACE_BYTES: u64 = MIB;
/// Compressed bytes times this must reach the uncompressed bytes (1 %).
pub const RATIO_INVERSE: u64 = 100;

/// Deflate cannot expand more than about 1032 times.
const MAX_DEFLATE_EXPANSION: u64 = 1032;
const EOCD_LEN: usize = 22;
const CENTRAL_LEN: usize = 46;
const LOCAL_LEN: usize = 30;

/// The limits of the check. The defaults are the constants above; tests lower
/// them, nothing outside the crate can.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ArchiveLimits {
    pub max_entries: usize,
    pub max_central_dir_bytes: u64,
    pub max_entry_bytes: u64,
    pub max_total_bytes: u64,
}

impl Default for ArchiveLimits {
    fn default() -> Self {
        Self {
            max_entries: MAX_ENTRIES,
            max_central_dir_bytes: MAX_CENTRAL_DIR_BYTES,
            max_entry_bytes: MAX_ENTRY_BYTES,
            max_total_bytes: MAX_TOTAL_BYTES,
        }
    }
}

/// Why an archive was refused. The text is fixed: no name, size or byte of the
/// file is echoed.
#[derive(Debug, Error, PartialEq, Eq, Clone, Copy)]
pub enum ArchiveError {
    #[error("the file is not a readable zip archive")]
    NotAnArchive,
    #[error("the archive could not be read from local storage")]
    Io,
    #[error("the archive uses zip64, encryption, several disks or an unsupported method")]
    Unsupported,
    #[error("the archive has too many entries")]
    TooManyEntries,
    #[error("the archive's central directory is too large")]
    CentralDirectoryTooLarge,
    #[error("an entry name is not acceptable")]
    BadName,
    #[error("two entries have the same name")]
    DuplicateName,
    #[error("an entry expands past the per-entry limit")]
    EntryTooLarge,
    #[error("the archive expands past the total limit")]
    TotalTooLarge,
    #[error("an entry is compressed more than the ratio limit allows")]
    RatioTooLow,
    #[error("an entry's sizes are impossible for its compression method")]
    ImpossibleSizes,
    #[error("the archive's headers are inconsistent")]
    InconsistentHeaders,
}

impl ArchiveError {
    /// The file is not an archive at all (as opposed to one over a limit).
    pub fn is_not_an_archive(&self) -> bool {
        matches!(self, Self::NotAnArchive)
    }
}

/// One entry as the central directory declares it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryInfo {
    pub name: String,
    pub method: u16,
    pub compressed: u64,
    pub uncompressed: u64,
}

/// What a passing archive declares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveSummary {
    pub entries: Vec<EntryInfo>,
    pub total_uncompressed: u64,
}

impl ArchiveSummary {
    pub fn entry(&self, name: &str) -> Option<&EntryInfo> {
        self.entries.iter().find(|e| e.name == name)
    }
}

fn le16(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn le32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn io_error(e: io::Error) -> ArchiveError {
    if e.kind() == io::ErrorKind::UnexpectedEof {
        ArchiveError::NotAnArchive
    } else {
        ArchiveError::Io
    }
}

/// Checks the archive in `r` against the default limits.
pub fn check_archive<R: Read + Seek>(r: &mut R) -> Result<ArchiveSummary, ArchiveError> {
    check_archive_with(r, &ArchiveLimits::default())
}

pub(crate) fn check_archive_with<R: Read + Seek>(
    r: &mut R,
    limits: &ArchiveLimits,
) -> Result<ArchiveSummary, ArchiveError> {
    let len = r.seek(SeekFrom::End(0)).map_err(io_error)?;
    let (eocd, eocd_at) = read_end_record(r, len)?;
    // Zip64 locator right before the end record, or a marker in a field.
    let marker = le16(&eocd, 10) == 0xFFFF
        || le32(&eocd, 12) == 0xFFFF_FFFF
        || le32(&eocd, 16) == 0xFFFF_FFFF
        || (eocd_at >= 20 && locator_at(r, eocd_at - 20)?);
    let entries_total = usize::from(le16(&eocd, 10));
    if le16(&eocd, 4) != 0 || le16(&eocd, 6) != 0 || le16(&eocd, 8) != le16(&eocd, 10) || marker {
        return Err(ArchiveError::Unsupported);
    }
    let cd_size = u64::from(le32(&eocd, 12));
    let cd_offset = u64::from(le32(&eocd, 16));
    if entries_total > limits.max_entries {
        return Err(ArchiveError::TooManyEntries);
    }
    if cd_size > limits.max_central_dir_bytes {
        return Err(ArchiveError::CentralDirectoryTooLarge);
    }
    if cd_size < (entries_total * CENTRAL_LEN) as u64 || cd_offset + cd_size > eocd_at {
        return Err(ArchiveError::InconsistentHeaders);
    }
    let mut central = vec![0u8; cd_size as usize];
    r.seek(SeekFrom::Start(cd_offset)).map_err(io_error)?;
    r.read_exact(&mut central).map_err(io_error)?;

    let mut entries = Vec::with_capacity(entries_total);
    let mut spans: Vec<(u64, u64)> = Vec::with_capacity(entries_total);
    let mut names: HashSet<Vec<u8>> = HashSet::with_capacity(entries_total);
    let mut total: u64 = 0;
    let mut at = 0usize;
    for _ in 0..entries_total {
        let h = central
            .get(at..at + CENTRAL_LEN)
            .filter(|h| h.starts_with(b"PK\x01\x02"))
            .ok_or(ArchiveError::InconsistentHeaders)?;
        let (flags, method) = (le16(h, 8), le16(h, 10));
        let (crc, csize32, usize32) = (le32(h, 16), le32(h, 20), le32(h, 24));
        let (name_len, extra_len, comment_len) = (
            usize::from(le16(h, 28)),
            usize::from(le16(h, 30)),
            usize::from(le16(h, 32)),
        );
        let (disk, local_offset) = (le16(h, 34), u64::from(le32(h, 42)));
        let record_len = CENTRAL_LEN + name_len + extra_len + comment_len;
        let name = central
            .get(at + CENTRAL_LEN..at + CENTRAL_LEN + name_len)
            .filter(|_| at + record_len <= central.len())
            .ok_or(ArchiveError::InconsistentHeaders)?;
        at += record_len;
        if flags & 0x0001 != 0
            || !matches!(method, 0 | 8)
            || disk != 0
            || csize32 == 0xFFFF_FFFF
            || usize32 == 0xFFFF_FFFF
            || local_offset == 0xFFFF_FFFF
        {
            return Err(ArchiveError::Unsupported);
        }
        if !valid_name(name) {
            return Err(ArchiveError::BadName);
        }
        if !names.insert(name.to_vec()) {
            return Err(ArchiveError::DuplicateName);
        }
        let (csize, usize_) = (u64::from(csize32), u64::from(usize32));
        total = total.saturating_add(usize_);
        if usize_ > limits.max_entry_bytes {
            return Err(ArchiveError::EntryTooLarge);
        }
        if total > limits.max_total_bytes {
            return Err(ArchiveError::TotalTooLarge);
        }
        let impossible = match method {
            0 => csize != usize_,
            _ => usize_ > csize.saturating_mul(MAX_DEFLATE_EXPANSION) + MAX_DEFLATE_EXPANSION,
        };
        if impossible || (name.ends_with(b"/") && usize_ != 0) {
            return Err(ArchiveError::ImpossibleSizes);
        }
        if usize_ > RATIO_GRACE_BYTES && csize.saturating_mul(RATIO_INVERSE) < usize_ {
            return Err(ArchiveError::RatioTooLow);
        }
        let local_len = check_local(
            r,
            name,
            local_offset,
            (flags, method, crc, csize32, usize32),
        )?;
        spans.push((local_offset, local_offset + local_len + csize));
        entries.push(EntryInfo {
            name: String::from_utf8_lossy(name).into_owned(),
            method,
            compressed: csize,
            uncompressed: usize_,
        });
    }
    if at != central.len() {
        return Err(ArchiveError::InconsistentHeaders);
    }
    // Every entry's bytes lie before the central directory and none overlaps
    // another: sorted by position, each ends before the next begins.
    spans.sort_unstable();
    let mut floor = 0u64;
    for (start, end) in spans {
        if start < floor || end > cd_offset {
            return Err(ArchiveError::InconsistentHeaders);
        }
        floor = end;
    }
    Ok(ArchiveSummary {
        entries,
        total_uncompressed: total,
    })
}

/// The end-of-central-directory record and where it starts: the last record
/// whose comment runs exactly to the end of the file.
fn read_end_record<R: Read + Seek>(
    r: &mut R,
    len: u64,
) -> Result<([u8; EOCD_LEN], u64), ArchiveError> {
    if len < EOCD_LEN as u64 {
        return Err(ArchiveError::NotAnArchive);
    }
    let tail_len = len.min((EOCD_LEN + usize::from(u16::MAX)) as u64);
    let mut tail = vec![0u8; tail_len as usize];
    r.seek(SeekFrom::Start(len - tail_len)).map_err(io_error)?;
    r.read_exact(&mut tail).map_err(io_error)?;
    for pos in (0..=tail.len() - EOCD_LEN).rev() {
        if tail[pos..].starts_with(b"PK\x05\x06")
            && pos + EOCD_LEN + usize::from(le16(&tail, pos + 20)) == tail.len()
        {
            let mut record = [0u8; EOCD_LEN];
            record.copy_from_slice(&tail[pos..pos + EOCD_LEN]);
            return Ok((record, len - tail_len + pos as u64));
        }
    }
    Err(ArchiveError::NotAnArchive)
}

fn locator_at<R: Read + Seek>(r: &mut R, at: u64) -> Result<bool, ArchiveError> {
    let mut sig = [0u8; 4];
    r.seek(SeekFrom::Start(at)).map_err(io_error)?;
    r.read_exact(&mut sig).map_err(io_error)?;
    Ok(&sig == b"PK\x06\x07")
}

/// Reads the local header at `offset` and compares it with the central
/// directory. Returns the length of the header with its name and extra field.
/// With a data descriptor (bit 3) the local CRC and sizes are zero by design
/// and are not compared.
fn check_local<R: Read + Seek>(
    r: &mut R,
    name: &[u8],
    offset: u64,
    (flags, method, crc, csize, usize_): (u16, u16, u32, u32, u32),
) -> Result<u64, ArchiveError> {
    let mut h = [0u8; LOCAL_LEN];
    r.seek(SeekFrom::Start(offset))
        .map_err(|_| ArchiveError::InconsistentHeaders)?;
    r.read_exact(&mut h).map_err(|e| match e.kind() {
        io::ErrorKind::UnexpectedEof => ArchiveError::InconsistentHeaders,
        _ => ArchiveError::Io,
    })?;
    let (name_len, extra_len) = (usize::from(le16(&h, 26)), u64::from(le16(&h, 28)));
    let sizes_agree = flags & 0x0008 != 0
        || (le32(&h, 14) == crc && le32(&h, 18) == csize && le32(&h, 22) == usize_);
    if !h.starts_with(b"PK\x03\x04")
        || le16(&h, 6) != flags
        || le16(&h, 8) != method
        || !sizes_agree
        || name_len != name.len()
    {
        return Err(ArchiveError::InconsistentHeaders);
    }
    let mut local_name = vec![0u8; name_len];
    r.read_exact(&mut local_name).map_err(|e| match e.kind() {
        io::ErrorKind::UnexpectedEof => ArchiveError::InconsistentHeaders,
        _ => ArchiveError::Io,
    })?;
    if local_name != name {
        return Err(ArchiveError::InconsistentHeaders);
    }
    Ok((LOCAL_LEN + name_len) as u64 + extra_len)
}

/// A name that cannot escape a directory or be mistaken for another: UTF-8,
/// not empty, at most [`MAX_NAME_BYTES`], no control character, backslash,
/// leading slash, drive letter or `..` segment.
fn valid_name(name: &[u8]) -> bool {
    let Ok(s) = std::str::from_utf8(name) else {
        return false;
    };
    !s.is_empty()
        && s.len() <= MAX_NAME_BYTES
        && !s.chars().any(|c| c.is_control() || c == '\\')
        && !s.starts_with('/')
        && s.as_bytes().get(1) != Some(&b':')
        && !s.split('/').any(|segment| segment == "..")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tabular_prepare::zipfix::{build, Entry};
    use std::io::Cursor;

    fn check(bytes: Vec<u8>) -> Result<ArchiveSummary, ArchiveError> {
        check_archive(&mut Cursor::new(bytes))
    }

    fn deflated(name: &str, data: &[u8], csize: u32, usize_: u32) -> Entry {
        Entry::stored(name, data).claim(8, csize, usize_)
    }

    #[test]
    fn a_well_formed_archive_passes_and_reports_what_it_declares() {
        let bytes = build(&[
            Entry::stored("[Content_Types].xml", b"<Types/>"),
            Entry::stored("xl/workbook.xml", b"<workbook/>"),
        ]);
        let summary = check(bytes).unwrap();
        assert_eq!(summary.entries.len(), 2);
        assert_eq!(summary.total_uncompressed, 8 + 11);
        let wb = summary.entry("xl/workbook.xml").unwrap();
        assert_eq!((wb.compressed, wb.uncompressed, wb.method), (11, 11, 0));
        assert!(summary.entry("nope").is_none());
    }

    #[test]
    fn an_entry_over_the_per_entry_limit_is_refused() {
        // Claims 3 GiB with a plausible ratio; nothing is inflated to find out.
        let big = deflated(
            "xl/worksheets/sheet1.xml",
            b"x",
            1_500_000_000,
            3_000_000_000,
        );
        assert_eq!(check(build(&[big])), Err(ArchiveError::EntryTooLarge));
    }

    #[test]
    fn the_total_over_the_limit_is_refused_even_when_each_entry_is_fine() {
        let one = |n: &str| deflated(n, b"x", 900_000_000, 1_800_000_000);
        let bytes = build(&[one("a"), one("b")]);
        assert_eq!(check(bytes), Err(ArchiveError::TotalTooLarge));
    }

    #[test]
    fn the_ratio_rule_is_one_percent_above_a_one_mib_grace() {
        let data = vec![7u8; 11_000];
        // Exactly 1 %: accepted. One byte less: refused.
        let at = deflated("a", &data, 11_000, 1_100_000);
        assert!(check(build(&[at])).is_ok());
        let under = deflated("a", &data[..10_999], 10_999, 1_100_000);
        assert_eq!(check(build(&[under])), Err(ArchiveError::RatioTooLow));
        // The lowest ratio of a real workbook (8.2 %) passes.
        let real = deflated("a", &vec![1u8; 90_200], 90_200, 1_100_000);
        assert!(check(build(&[real])).is_ok());
        // Up to 1 MiB the ratio is not checked (a long run of one value).
        let small = deflated("a", &vec![1u8; 1024], 1024, 1_048_576);
        assert!(check(build(&[small])).is_ok());
    }

    #[test]
    fn sizes_deflate_cannot_produce_are_refused() {
        let e = deflated("a", b"0123456789", 10, 2_000_000);
        assert_eq!(check(build(&[e])), Err(ArchiveError::ImpossibleSizes));
        // A stored entry is as long as it is stored.
        let s = Entry::stored("a", b"abc").claim(0, 3, 4);
        assert_eq!(check(build(&[s])), Err(ArchiveError::ImpossibleSizes));
    }

    #[test]
    fn a_local_header_that_disagrees_with_the_central_directory_is_refused() {
        // The central directory says 1000 bytes, the local header 1 GiB.
        let mut e = Entry::stored("a", &vec![0u8; 1000]).lie_central(1000, 1000);
        e.local.usize_ = 1 << 30;
        assert_eq!(check(build(&[e])), Err(ArchiveError::InconsistentHeaders));
        // A different name in the local header.
        let mut e = Entry::stored("a", b"x");
        e.local_name = Some(b"b".to_vec());
        assert_eq!(check(build(&[e])), Err(ArchiveError::InconsistentHeaders));
        // A method the local header does not share is a disagreement too: patch it.
        let mut bytes = build(&[Entry::stored("a", b"xyz")]);
        bytes[8] = 8;
        assert_eq!(check(bytes), Err(ArchiveError::InconsistentHeaders));
    }

    #[test]
    fn data_outside_the_file_or_overlapping_the_next_entry_is_refused() {
        // Both headers claim 5,000 bytes; the file holds three.
        let e = Entry::stored("a", b"abc").claim(0, 5000, 5000);
        assert_eq!(check(build(&[e])), Err(ArchiveError::InconsistentHeaders));
        // The first entry claims more than it holds: its data would run over the
        // local header of the second.
        let a = Entry::stored("a", b"abc").claim(0, 40, 40);
        let b = Entry::stored("b", &[9u8; 100]);
        assert_eq!(
            check(build(&[a, b])),
            Err(ArchiveError::InconsistentHeaders)
        );
        // Two central records for one local header.
        let a = Entry::stored("a", b"abc");
        let mut b = Entry::stored("b", b"abc");
        b.offset_override = Some(0);
        assert_eq!(
            check(build(&[a, b])),
            Err(ArchiveError::InconsistentHeaders)
        );
    }

    #[test]
    fn a_data_descriptor_leaves_the_local_sizes_at_zero() {
        let mut e = Entry::stored("a", b"hello");
        e.flags = 0x0008;
        assert!(check(build(&[e])).is_ok());
    }

    #[test]
    fn names_that_could_escape_or_confuse_are_refused() {
        for bad in [
            "../evil.xml",
            "xl/../../evil.xml",
            "/abs.xml",
            "a\\b.xml",
            "C:evil.xml",
            "bad\u{1}name",
            "nul\u{0}name",
            "",
        ] {
            let e = Entry::stored(bad, b"x");
            assert_eq!(check(build(&[e])), Err(ArchiveError::BadName), "{bad:?}");
        }
        let bytes = build(&[Entry::stored("a.xml", b"1"), Entry::stored("a.xml", b"2")]);
        assert_eq!(check(bytes), Err(ArchiveError::DuplicateName));
        let long = "x".repeat(MAX_NAME_BYTES + 1);
        assert_eq!(
            check(build(&[Entry::stored(&long, b"x")])),
            Err(ArchiveError::BadName)
        );
    }

    #[test]
    fn entry_count_and_central_directory_size_are_bounded() {
        let three = [
            Entry::stored("a", b"1"),
            Entry::stored("b", b"2"),
            Entry::stored("c", b"3"),
        ];
        let limits = ArchiveLimits {
            max_entries: 2,
            ..ArchiveLimits::default()
        };
        let r = check_archive_with(&mut Cursor::new(build(&three)), &limits);
        assert_eq!(r, Err(ArchiveError::TooManyEntries));
        let limits = ArchiveLimits {
            max_central_dir_bytes: 100,
            ..ArchiveLimits::default()
        };
        let r = check_archive_with(&mut Cursor::new(build(&three)), &limits);
        assert_eq!(r, Err(ArchiveError::CentralDirectoryTooLarge));
    }

    #[test]
    fn zip64_encryption_and_unknown_methods_are_unsupported() {
        let mut bytes = build(&[Entry::stored("a", b"x")]);
        let n = bytes.len();
        // Entry count 0xFFFF in the end record: the zip64 marker.
        bytes[n - 14..n - 10].copy_from_slice(&[0xFF; 4]);
        assert_eq!(check(bytes), Err(ArchiveError::Unsupported));
        let e = Entry::stored("a", b"x").lie_central(0xFFFF_FFFF, 1);
        assert_eq!(check(build(&[e])), Err(ArchiveError::Unsupported));
        let mut enc = Entry::stored("a", b"x");
        enc.flags = 0x0001;
        assert_eq!(check(build(&[enc])), Err(ArchiveError::Unsupported));
        let m = Entry::stored("a", b"x").claim(99, 1, 1);
        assert_eq!(check(build(&[m])), Err(ArchiveError::Unsupported));
    }

    #[test]
    fn what_is_not_an_archive_is_told_apart_from_what_is_over_a_limit() {
        assert_eq!(check(Vec::new()), Err(ArchiveError::NotAnArchive));
        assert_eq!(check(vec![0u8; 5000]), Err(ArchiveError::NotAnArchive));
        let bytes = build(&[Entry::stored("a", b"hello world")]);
        let cut = bytes[..bytes.len() - 5].to_vec();
        let err = check(cut).unwrap_err();
        assert!(err.is_not_an_archive());
        assert!(!ArchiveError::EntryTooLarge.is_not_an_archive());
    }

    /// Counts what the check reads.
    struct Counting<R> {
        inner: R,
        read: u64,
        biggest: usize,
    }

    impl<R: Read> Read for Counting<R> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let n = self.inner.read(buf)?;
            self.read += n as u64;
            self.biggest = self.biggest.max(buf.len());
            Ok(n)
        }
    }

    impl<R: Seek> Seek for Counting<R> {
        fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
            self.inner.seek(pos)
        }
    }

    #[test]
    fn only_headers_are_read_however_large_the_entries_are() {
        let payload = vec![0u8; 20 * 1024 * 1024];
        let bytes = build(&[
            Entry::stored("xl/worksheets/sheet1.xml", &payload),
            Entry::stored("xl/workbook.xml", b"<workbook/>"),
        ]);
        let mut counting = Counting {
            inner: Cursor::new(bytes),
            read: 0,
            biggest: 0,
        };
        // The 20 MiB of zeros are stored, so the ratio is 1: it passes.
        check_archive(&mut counting).unwrap();
        // The end of the file (64 KiB at most) and a few small headers.
        assert!(counting.read < 128 * 1024, "read {} bytes", counting.read);
        assert!(counting.biggest <= 65_536 + EOCD_LEN);
    }
}
