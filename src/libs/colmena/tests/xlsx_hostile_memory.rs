//! Hostile workbooks cost bounded memory to refuse.
//!
//! Own test binary with a counting global allocator: each archive is built before
//! the measurement, then opened and read through the same path the job uses, and
//! the peak of live heap bytes above the starting level is asserted.

use bytes::Bytes;
use colmena::tabular_prepare::precheck::ArchiveError;
use colmena::tabular_prepare::xlsx_package::Package;
use colmena::tabular_prepare::xlsx_sheet::{read_sheet, SheetContext};
use colmena::tabular_prepare::xlsx_spool::{spool_stream, XlsxError, MAX_XLSX_BYTES};
use colmena::tabular_prepare::xlsx_strings::{read_shared_strings, SharedStrings};
use colmena::tabular_prepare::xlsx_styles::{read_styles, Styles};
use colmena::tabular_prepare::xlsx_workbook::read_workbook;
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

/// The counters are global: one test measures at a time.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

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
    let _one_at_a_time = SERIAL.lock().await;
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
    let _one_at_a_time = SERIAL.lock().await;
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

const NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

/// The parts of a small valid workbook; a test replaces the one it attacks.
fn parts() -> Vec<(&'static str, Vec<u8>)> {
    let rels = format!("<Relationships><Relationship Id=\"rId1\" Type=\"{NS}/worksheet\" Target=\"worksheets/sheet1.xml\"/><Relationship Id=\"rId2\" Type=\"{NS}/sharedStrings\" Target=\"sharedStrings.xml\"/><Relationship Id=\"rId3\" Type=\"{NS}/styles\" Target=\"styles.xml\"/></Relationships>");
    vec![
        ("_rels/.rels", format!("<Relationships><Relationship Id=\"r\" Type=\"{NS}/officeDocument\" Target=\"xl/workbook.xml\"/></Relationships>").into_bytes()),
        ("xl/workbook.xml", format!("<workbook xmlns:r=\"{NS}\"><sheets><sheet name=\"A\" r:id=\"rId1\"/></sheets></workbook>").into_bytes()),
        ("xl/_rels/workbook.xml.rels", rels.into_bytes()),
        ("xl/styles.xml", b"<styleSheet><cellXfs><xf numFmtId=\"0\"/></cellXfs></styleSheet>".to_vec()),
        ("xl/sharedStrings.xml", b"<sst><si><t>a</t></si></sst>".to_vec()),
        ("xl/worksheets/sheet1.xml", b"<worksheet><sheetData><row r=\"1\"><c r=\"A1\"><v>1</v></c></row></sheetData></worksheet>".to_vec()),
    ]
}

/// The valid workbook with `part` replaced by `body`, as an archive.
fn book_with(part: &str, body: Vec<u8>) -> Vec<u8> {
    book_with_all(&[(part, body)])
}

/// The valid workbook with several parts replaced.
fn book_with_all(replacements: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    for (name, bytes) in parts() {
        zip.start_file(name, stored()).unwrap();
        let body = replacements
            .iter()
            .find(|(n, _)| *n == name)
            .map_or(&bytes, |(_, b)| b);
        zip.write_all(body).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

/// Millions of elements that are never closed, each three bytes.
fn unclosed(prefix: &str, count: usize) -> Vec<u8> {
    let mut v = prefix.as_bytes().to_vec();
    v.extend_from_slice("<a>".repeat(count).as_bytes());
    v
}

/// The peak, above the level before `read` ran, of reading the part `name` of the
/// package with the function that reads that kind of part.
fn read_part(pkg: &mut Package, name: &str) -> Result<(), XlsxError> {
    match name {
        "xl/workbook.xml" | "xl/_rels/workbook.xml.rels" => read_workbook(pkg).map(|_| ()),
        "xl/styles.xml" => read_styles(pkg, name).map(|_| ()),
        "xl/sharedStrings.xml" => read_shared_strings(pkg, name).map(|_| ()),
        _ => {
            let (strings, styles) = (SharedStrings::none(), Styles::none());
            let (cells, cancel) = (AtomicU64::new(0), CancellationToken::new());
            let ctx = SheetContext {
                strings: &strings,
                styles: &styles,
                date1904: false,
                cells: &cells,
                cancel: &cancel,
            };
            read_sheet(pkg, name, &ctx, |_, _| Ok(true)).map(|_| ())
        }
    }
}

#[tokio::test]
async fn unbounded_nesting_in_any_kind_of_part_is_refused_with_bounded_memory() {
    let _one_at_a_time = SERIAL.lock().await;
    for name in [
        "xl/workbook.xml",
        "xl/_rels/workbook.xml.rels",
        "xl/styles.xml",
        "xl/sharedStrings.xml",
        "xl/worksheets/sheet1.xml",
    ] {
        // 3,000,000 unclosed elements: 9 MB of input that the parser would hold as
        // nine bytes of open-element bookkeeping per tag.
        let body = unclosed("<root>", 3_000_000);
        // The shared-strings table reserves its worst case up front from the declared
        // size (the text, and four bytes for each possible string of five bytes):
        // that is the one cost allowed above the parser's own.
        let reserved = if name == "xl/sharedStrings.xml" {
            body.len() + 4 * (body.len() / 5 + 1)
        } else {
            0
        };
        let (pkg, _) = open(book_with(name, body)).await;
        let mut pkg = pkg.unwrap_or_else(|e| panic!("{name}: {e}"));
        let base = LIVE.load(Ordering::SeqCst);
        PEAK.store(base, Ordering::SeqCst);
        let result = read_part(&mut pkg, name);
        let peak = PEAK.load(Ordering::SeqCst) - base;
        assert!(
            matches!(
                result,
                Err(XlsxError::Invalid(
                    colmena::tabular_prepare::xlsx_spool::Invalid::TooDeep
                ))
            ),
            "{name}: {result:?}"
        );
        assert!(peak < reserved + 2 * MIB, "{name}: peak {peak} bytes");
    }
}

#[tokio::test]
async fn a_part_over_its_own_small_cap_is_refused_before_it_is_read() {
    let _one_at_a_time = SERIAL.lock().await;
    // Whitespace after a valid part, one byte past the cap its kind has: 16 MiB for the
    // workbook, 64 MiB for the styles.
    for (name, over) in [("xl/styles.xml", 65), ("xl/workbook.xml", 17)] {
        let mut body = parts().into_iter().find(|(n, _)| *n == name).unwrap().1;
        body.extend(std::iter::repeat_n(b' ', over * MIB));
        let (pkg, _) = open(book_with(name, body)).await;
        let mut pkg = pkg.unwrap();
        let base = LIVE.load(Ordering::SeqCst);
        PEAK.store(base, Ordering::SeqCst);
        let result = read_part(&mut pkg, name);
        assert!(
            matches!(result, Err(XlsxError::Archive(ArchiveError::EntryTooLarge))),
            "{name}: {result:?}"
        );
        assert!(PEAK.load(Ordering::SeqCst) - base < MIB);
    }
}

#[tokio::test]
async fn a_tag_with_tens_of_thousands_of_attributes_is_refused_in_linear_time() {
    let _one_at_a_time = SERIAL.lock().await;
    let attributes: String = (0..20_000).map(|i| format!(" a{i}=\"x\"")).collect();
    let sheet = format!("<worksheet><sheetData><row r=\"1\"><c r=\"A1\"{attributes}><v>1</v></c></row></sheetData></worksheet>");
    let (pkg, _) = open(book_with("xl/worksheets/sheet1.xml", sheet.into_bytes())).await;
    let mut pkg = pkg.unwrap();
    let started = std::time::Instant::now();
    let result = read_part(&mut pkg, "xl/worksheets/sheet1.xml");
    assert!(matches!(
        result,
        Err(XlsxError::Invalid(
            colmena::tabular_prepare::xlsx_spool::Invalid::TooManyAttributes
        ))
    ));
    // The duplicate-attribute check of the parser would compare each attribute with
    // every earlier one: about 2 x 10^8 comparisons, seconds in a debug build.
    assert!(
        started.elapsed() < std::time::Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn one_cell_made_of_millions_of_tiny_tokens_is_cut_at_the_cell_cap() {
    let _one_at_a_time = SERIAL.lock().await;
    // 3,000,000 comments between one-byte texts: each token is tiny and passes the
    // per-token guard, but the cell they build would be 3 MB.
    let value = format!("1{}", "<!-- -->1".repeat(3_000_000));
    let sheet = format!(
        "<worksheet><sheetData><row r=\"1\"><c r=\"A1\" t=\"str\"><v>{value}</v></c></row></sheetData></worksheet>"
    );
    let (pkg, _) = open(book_with("xl/worksheets/sheet1.xml", sheet.into_bytes())).await;
    let mut pkg = pkg.unwrap();
    let base = LIVE.load(Ordering::SeqCst);
    PEAK.store(base, Ordering::SeqCst);
    let result = read_part(&mut pkg, "xl/worksheets/sheet1.xml");
    let peak = PEAK.load(Ordering::SeqCst) - base;
    assert!(matches!(
        result,
        Err(XlsxError::Invalid(
            colmena::tabular_prepare::xlsx_spool::Invalid::CellTooLong
        ))
    ));
    // The cell cap is 128 KiB; the rest is the reader's own buffers.
    assert!(peak < MIB, "peak {peak} bytes");
}

/// Reads the sheet of a package with its own shared strings.
fn read_with_strings(pkg: &mut Package) -> Result<(), XlsxError> {
    let strings = read_shared_strings(pkg, "xl/sharedStrings.xml")?;
    let styles = Styles::none();
    let (cells, cancel) = (AtomicU64::new(0), CancellationToken::new());
    let ctx = SheetContext {
        strings: &strings,
        styles: &styles,
        date1904: false,
        cells: &cells,
        cancel: &cancel,
    };
    read_sheet(pkg, "xl/worksheets/sheet1.xml", &ctx, |_, _| Ok(true)).map(|_| ())
}

#[tokio::test]
async fn rows_opened_inside_an_open_row_are_refused_not_accumulated() {
    let _one_at_a_time = SERIAL.lock().await;
    use colmena::tabular_prepare::xlsx_spool::Invalid;
    // A shared string of exactly 128 KiB, the cell cap; eight cells of it fill the row
    // cap (1 MiB). Then, without ever closing the outer row, an empty `row` element and
    // eight more such cells, again and again: each empty row used to reset the row's
    // counters while its cells stayed held, so memory grew by a megabyte per 250 bytes.
    let sst = format!("<sst><si><t>{}</t></si></sst>", "x".repeat(131_072));
    let group: String = (1..=8)
        .map(|c| {
            format!(
                "<c r=\"{}1\" t=\"s\"><v>0</v></c>",
                (b'A' + c as u8 - 1) as char
            )
        })
        .collect();
    let mut rows = String::from("<row r=\"1\">");
    for n in 2..2000 {
        rows.push_str(&group);
        rows.push_str(&format!("<row r=\"{n}\"/>"));
    }
    let sheet = format!("<worksheet><sheetData>{rows}</sheetData></worksheet>");
    let bytes = book_with_all(&[
        ("xl/sharedStrings.xml", sst.into_bytes()),
        ("xl/worksheets/sheet1.xml", sheet.into_bytes()),
    ]);
    let (pkg, _) = open(bytes).await;
    let mut pkg = pkg.unwrap();
    let base = LIVE.load(Ordering::SeqCst);
    PEAK.store(base, Ordering::SeqCst);
    let result = read_with_strings(&mut pkg);
    let peak = PEAK.load(Ordering::SeqCst) - base;
    assert!(
        matches!(result, Err(XlsxError::Invalid(Invalid::BadCell))),
        "{result:?}"
    );
    // The table (128 KiB twice) and at most one row of eight cells (1 MiB), not 2,000 rows.
    assert!(peak < 4 * MIB, "peak {peak} bytes");
}

#[tokio::test]
async fn a_nested_row_start_and_the_header_that_used_to_index_past_its_width_are_refused() {
    let _one_at_a_time = SERIAL.lock().await;
    use colmena::tabular_prepare::xlsx_spool::Invalid;
    for rows in [
        "<row r=\"1\"><c r=\"A1\"><v>1</v></c><row r=\"2\"><c r=\"A2\"><v>1</v></c></row></row>",
        // The reviewer's: a cell in column F, then a row whose cell is in column A.
        "<row r=\"1\"><c r=\"F1\"><v>1</v></c><row r=\"2\"><c r=\"A2\"><v>1</v></c></row></row>",
        "<row r=\"1\"><c r=\"A1\"><c r=\"B1\"><v>1</v></c></c></row>",
        "<row r=\"1\"/><sheetData/>",
        "<row r=\"1\"><c r=\"A1\"><v>1</v></c>",
    ] {
        let sheet = format!("<worksheet><sheetData>{rows}</sheetData></worksheet>");
        let (pkg, _) = open(book_with("xl/worksheets/sheet1.xml", sheet.into_bytes())).await;
        let mut pkg = pkg.unwrap();
        let result = read_part(&mut pkg, "xl/worksheets/sheet1.xml");
        assert!(
            matches!(
                result,
                Err(XlsxError::Invalid(Invalid::BadCell | Invalid::Xml))
            ),
            "{rows}: {result:?}"
        );
    }
}
