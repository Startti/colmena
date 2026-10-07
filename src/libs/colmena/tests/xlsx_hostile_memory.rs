//! Hostile workbooks cost bounded memory to refuse.
//!
//! Own test binary with a counting global allocator: each archive is built before
//! the measurement, then opened and read through the same path the job uses, and
//! the peak of live heap bytes above the starting level is asserted.

use bytes::Bytes;
use colmena::tabular_prepare::precheck::ArchiveError;
use colmena::tabular_prepare::xlsx_package::Package;
use colmena::tabular_prepare::xlsx_spool::{spool_stream, XlsxError, MAX_XLSX_BYTES};
use std::alloc::{GlobalAlloc, Layout, System};
use std::io::{Cursor, Write};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use tokio_util::sync::CancellationToken;
use zip::write::FileOptions;
use zip::{CompressionMethod, ZipWriter};

struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let p = System.alloc(l);
        if !p.is_null() {
            let now = LIVE.fetch_add(l.size(), Ordering::SeqCst) + l.size();
            PEAK.fetch_max(now, Ordering::SeqCst);
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        System.dealloc(p, l);
        LIVE.fetch_sub(l.size(), Ordering::SeqCst);
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

const MIB: usize = 1024 * 1024;

pub fn stored() -> FileOptions {
    FileOptions::default().compression_method(CompressionMethod::Stored)
}

/// Spools `bytes` and opens it as a package; returns the result and the peak of
/// live heap bytes above the level before the call.
async fn open(bytes: Vec<u8>) -> (Result<Package, XlsxError>, usize) {
    let dir = tempfile::tempdir().unwrap();
    let base = LIVE.load(Ordering::SeqCst);
    PEAK.store(base, Ordering::SeqCst);
    let stream = Box::pin(futures::stream::iter(vec![Ok(Bytes::from(bytes))]));
    let read = AtomicU64::new(0);
    let spooled = spool_stream(
        dir.path(),
        stream,
        None,
        MAX_XLSX_BYTES,
        &CancellationToken::new(),
        &read,
    )
    .await
    .unwrap();
    let result = Package::open(spooled);
    // What the input itself held is not the package's cost: subtract the bytes.
    (result, PEAK.load(Ordering::SeqCst) - base)
}

/// An archive of one stored entry, with `comment` as its comment (raw bytes).
fn archive_with_comment(comment: &[u8]) -> Vec<u8> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    zip.start_file("a.xml", stored()).unwrap();
    zip.write_all(b"<a/>").unwrap();
    let mut out = zip.finish().unwrap().into_inner();
    let n = out.len();
    out[n - 2..].copy_from_slice(&(comment.len() as u16).to_le_bytes());
    out.extend_from_slice(comment);
    out
}

#[tokio::test]
async fn a_forged_end_record_in_the_comment_is_refused_before_any_directory_is_read() {
    // The forged record claims 65,535 entries and a directory the real one never had.
    let mut forged = b"PK\x05\x06".to_vec();
    // Disks, 65,535 entries, a directory size and offset, and a comment length that
    // does not reach the end of the file (so a check that wants the record whose
    // comment ends the file would never pick it).
    forged.extend_from_slice(&[0, 0, 0, 0, 0xFF, 0xFF, 0xFF, 0xFF]);
    forged.extend_from_slice(&[0, 0, 0, 1, 0, 0, 0, 0, 1, 0]);
    let bytes = archive_with_comment(&forged);
    // Sanity: the archive is otherwise a valid one.
    let (ok, _) = open(archive_with_comment(b"a plain comment")).await;
    assert!(ok.is_ok());
    let (result, peak) = open(bytes).await;
    assert!(matches!(
        result,
        Err(XlsxError::Archive(ArchiveError::InconsistentHeaders))
    ));
    assert!(peak < 2 * MIB, "peak {peak} bytes");
}

#[tokio::test]
async fn a_gap_before_the_end_record_is_refused_before_any_directory_is_read() {
    let mut bytes = archive_with_comment(b"");
    let n = bytes.len();
    bytes.splice(n - 22..n - 22, vec![0u8; 64]);
    let (result, peak) = open(bytes).await;
    assert!(matches!(
        result,
        Err(XlsxError::Archive(ArchiveError::InconsistentHeaders))
    ));
    assert!(peak < 2 * MIB, "peak {peak} bytes");
}
