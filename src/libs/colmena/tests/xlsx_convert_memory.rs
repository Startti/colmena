//! The xlsx conversion's memory does not grow with the workbook.
//!
//! Own test binary because it installs a counting global allocator and measures
//! the peak of live heap bytes while a generated workbook is converted to Parquet
//! parts. Each workbook is written to a local file BEFORE the measurement starts
//! (the generator streams, but the file is the input, not part of what is
//! measured) and is read back through the same spool the job uses, so the spool,
//! the zip reader, the XML parser, the shared-strings table, the batches and the
//! part writer are all inside the measurement.
//!
//! Bounds. The allocator counts requested bytes, not time, so a slower runner
//! changes nothing but the interleaving of the reading and writing halves, which
//! can add at most the batches in flight (one being built, two in the channel,
//! one being written). Each scenario's absolute bound is therefore set from the
//! worst case those add up to for its shape, with room above what a debug build
//! measured, and the proof that the memory is bounded and not merely small is
//! the second assertion of each scenario: the peak of a workbook four times
//! larger is not four times more.

use async_trait::async_trait;
use bytes::Bytes;
use colmena::tabular_prepare::convert::ConvertControl;
use colmena::tabular_prepare::part_sink::{PartSink, SinkError};
use colmena::tabular_prepare::writer::WriterConfig;
use colmena::tabular_prepare::xlsx_convert::{convert_xlsx, XlsxSource};
use colmena::tabular_prepare::xlsx_spool::{spool_stream, Spooled, XlsxError, MAX_XLSX_BYTES};
use std::alloc::{GlobalAlloc, Layout, System};
use std::fs::File;
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
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

/// What a generated workbook's sheet holds.
#[derive(Clone, Copy)]
enum Shape {
    /// `cols` short cells of every kind per row: numbers, dates, booleans, text,
    /// and strings from the shared table.
    Grid { rows: usize, unique_strings: usize },
    /// `cells` inline text cells of `width` bytes per row.
    Wide {
        rows: usize,
        cells: usize,
        width: usize,
    },
}

fn column(i: usize) -> char {
    (b'A' + i as u8) as char
}

/// The sheet XML, row by row, through `emit`.
fn sheet_rows(shape: Shape, mut emit: impl FnMut(&str)) {
    emit("<worksheet><sheetData>");
    match shape {
        Shape::Grid {
            rows,
            unique_strings,
        } => {
            let names = ["id", "ref", "price", "day", "text", "flag", "n", "name"];
            let header: String = names
                .iter()
                .enumerate()
                .map(|(i, n)| {
                    format!(
                        "<c r=\"{}1\" t=\"inlineStr\"><is><t>{n}</t></is></c>",
                        column(i)
                    )
                })
                .collect();
            emit(&format!("<row r=\"1\">{header}</row>"));
            let mut row = String::new();
            for r in 2..=rows + 1 {
                row.clear();
                let (id, s) = (r - 1, (r * 7) % unique_strings.max(1));
                row.push_str(&format!(
                    "<row r=\"{r}\"><c r=\"A{r}\"><v>{id}</v></c>\
                     <c r=\"B{r}\" t=\"s\"><v>{s}</v></c>\
                     <c r=\"C{r}\"><v>{}.{:03}</v></c>\
                     <c r=\"D{r}\" s=\"1\"><v>{}</v></c>\
                     <c r=\"E{r}\" t=\"inlineStr\"><is><t>row {id} of text</t></is></c>\
                     <c r=\"F{r}\" t=\"b\"><v>{}</v></c>\
                     <c r=\"G{r}\"><v>{}</v></c>\
                     <c r=\"H{r}\" t=\"s\"><v>{}</v></c></row>",
                    id % 9973,
                    id % 1000,
                    44_000 + id % 1000,
                    id % 2,
                    (id * 31) % 100_003,
                    (id * 13) % unique_strings.max(1),
                ));
                emit(&row);
            }
        }
        Shape::Wide { rows, cells, width } => {
            let header: String = (0..cells)
                .map(|i| {
                    format!(
                        "<c r=\"{}1\" t=\"inlineStr\"><is><t>w{i}</t></is></c>",
                        column(i)
                    )
                })
                .collect();
            emit(&format!("<row r=\"1\">{header}</row>"));
            let mut text = String::with_capacity(width + 16);
            for r in 2..=rows + 1 {
                let mut row = format!("<row r=\"{r}\">");
                for c in 0..cells {
                    // Varied but compressible text.
                    text.clear();
                    let word = format!("w{r}-{c}-");
                    while text.len() < width {
                        text.push_str(&word);
                    }
                    text.truncate(width);
                    row.push_str(&format!(
                        "<c r=\"{}{r}\" t=\"inlineStr\"><is><t>{text}</t></is></c>",
                        column(c)
                    ));
                }
                row.push_str("</row>");
                emit(&row);
            }
        }
    }
    emit("</sheetData></worksheet>");
}

/// Writes a workbook of one sheet to `path`, streaming; returns its size.
fn write_workbook(path: &Path, shape: Shape, method: CompressionMethod) -> u64 {
    let file = BufWriter::new(File::create(path).unwrap());
    let mut zip = ZipWriter::new(file);
    let stored = FileOptions::default().compression_method(CompressionMethod::Stored);
    let ns = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
    let mut put = |name: &str, body: &str| {
        zip.start_file(name, stored).unwrap();
        zip.write_all(body.as_bytes()).unwrap();
    };
    put(
        "_rels/.rels",
        &format!("<Relationships><Relationship Id=\"r\" Type=\"{ns}/officeDocument\" Target=\"xl/workbook.xml\"/></Relationships>"),
    );
    put(
        "xl/workbook.xml",
        &format!("<workbook xmlns:r=\"{ns}\"><sheets><sheet name=\"Data\" r:id=\"rId1\"/></sheets></workbook>"),
    );
    put(
        "xl/_rels/workbook.xml.rels",
        &format!("<Relationships><Relationship Id=\"rId1\" Type=\"{ns}/worksheet\" Target=\"worksheets/sheet1.xml\"/><Relationship Id=\"rId2\" Type=\"{ns}/sharedStrings\" Target=\"sharedStrings.xml\"/><Relationship Id=\"rId3\" Type=\"{ns}/styles\" Target=\"styles.xml\"/></Relationships>"),
    );
    put(
        "xl/styles.xml",
        "<styleSheet><cellXfs><xf numFmtId=\"0\"/><xf numFmtId=\"14\"/></cellXfs></styleSheet>",
    );
    let unique = match shape {
        Shape::Grid { unique_strings, .. } => unique_strings,
        Shape::Wide { .. } => 0,
    };
    zip.start_file("xl/sharedStrings.xml", stored).unwrap();
    zip.write_all(b"<sst>").unwrap();
    for i in 0..unique {
        zip.write_all(
            format!("<si><t>shared string number {i} with some padding</t></si>").as_bytes(),
        )
        .unwrap();
    }
    zip.write_all(b"</sst>").unwrap();
    zip.start_file(
        "xl/worksheets/sheet1.xml",
        FileOptions::default().compression_method(method),
    )
    .unwrap();
    sheet_rows(shape, |chunk| zip.write_all(chunk.as_bytes()).unwrap());
    zip.finish().unwrap().flush().unwrap();
    std::fs::metadata(path).unwrap().len()
}

/// The workbook file, streamed in 64 KiB chunks through the spool the job uses.
struct FileSource(PathBuf);

#[async_trait]
impl XlsxSource for FileSource {
    async fn spool(&self, cancel: &CancellationToken) -> Result<Spooled, XlsxError> {
        let file = File::open(&self.0).unwrap();
        let stream = futures::stream::unfold(file, |mut f| async move {
            let mut buf = vec![0u8; 64 * 1024];
            match f.read(&mut buf) {
                Ok(0) => None,
                Ok(n) => {
                    buf.truncate(n);
                    Some((Ok(Bytes::from(buf)), f))
                }
                Err(_) => None,
            }
        });
        let read = AtomicU64::new(0);
        spool_stream(
            &std::env::temp_dir(),
            Box::pin(stream),
            None,
            MAX_XLSX_BYTES,
            cancel,
            &read,
        )
        .await
    }
}

/// Drops every part, counting the bytes.
#[derive(Default)]
struct Discard(AtomicUsize);

#[async_trait]
impl PartSink for Discard {
    async fn put(&self, _path: &str, data: Bytes) -> Result<(), SinkError> {
        self.0.fetch_add(data.len(), Ordering::SeqCst);
        Ok(())
    }
}

/// The workbook at `path`: the rows it converted and the peak of live heap bytes
/// above the starting level while converting it.
async fn peak_of(path: &Path, rows: usize) -> usize {
    let sink = Arc::new(Discard::default());
    let cfg = WriterConfig {
        max_rows: 500_000,
        max_bytes: 8 * MIB,
    };
    let base = LIVE.load(Ordering::SeqCst);
    PEAK.store(base, Ordering::SeqCst);
    let tables = convert_xlsx(
        &FileSource(path.to_path_buf()),
        sink.clone(),
        cfg,
        &ConvertControl::new(),
    )
    .await
    .unwrap_or_else(|f| panic!("conversion failed: {}", f.error));
    assert_eq!(tables.len(), 1);
    assert_eq!(tables[0].table.written.rows, rows as u64);
    assert!(sink.0.load(Ordering::SeqCst) > 0);
    PEAK.load(Ordering::SeqCst) - base
}

#[tokio::test(flavor = "multi_thread")]
async fn memory_is_bounded_whatever_the_size_of_the_workbook() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("book.xlsx");

    // Many short rows, with a shared-strings table of 50,000 entries.
    let grid = |rows| Shape::Grid {
        rows,
        unique_strings: 50_000,
    };
    let small_size = write_workbook(&path, grid(100_000), CompressionMethod::Stored);
    let small = peak_of(&path, 100_000).await;
    let large_size = write_workbook(&path, grid(400_000), CompressionMethod::Stored);
    let large = peak_of(&path, 400_000).await;
    eprintln!(
        "grid: {} MiB -> peak {} MiB, {} MiB -> peak {} MiB",
        small_size / MIB as u64,
        small / MIB,
        large_size / MIB as u64,
        large / MIB
    );
    assert!(large_size > 3 * small_size);
    assert!(large <= small + small / 4 + 8 * MIB, "{small} then {large}");
    assert!(large <= GRID_BOUND, "peak {} MiB", large / MIB);

    // Wide text cells: four of 16 KiB per row, so a batch is held by its 8 MiB of
    // text and not by its rows.
    let wide = |rows| Shape::Wide {
        rows,
        cells: 4,
        width: 16 * 1024,
    };
    let small_size = write_workbook(&path, wide(300), CompressionMethod::Stored);
    let small = peak_of(&path, 300).await;
    let large_size = write_workbook(&path, wide(1200), CompressionMethod::Stored);
    let large = peak_of(&path, 1200).await;
    eprintln!(
        "wide: {} MiB -> peak {} MiB, {} MiB -> peak {} MiB",
        small_size / MIB as u64,
        small / MIB,
        large_size / MIB as u64,
        large / MIB
    );
    assert!(large_size > 3 * small_size);
    assert!(large <= small + small / 4 + 8 * MIB, "{small} then {large}");
    assert!(large <= WIDE_BOUND, "peak {} MiB", large / MIB);

    // The same grid deflated, so the inflate path is inside the measurement too.
    let deflated = write_workbook(&path, grid(150_000), CompressionMethod::Deflated);
    let peak = peak_of(&path, 150_000).await;
    eprintln!(
        "deflated grid: {} MiB -> peak {} MiB",
        deflated / MIB as u64,
        peak / MIB
    );
    assert!(peak <= GRID_BOUND, "peak {} MiB", peak / MIB);
}

/// Worst case of the grid shape: batches of 8,192 rows of eight short cells (a
/// few hundred KiB each) in flight, the part writer at its 8 MiB part limit
/// (encode and output, twice that), the 50,000-string table (about 3 MiB), the
/// zip central directory and a 64 KiB spool chunk. Debug builds measure well
/// under this; a slow runner changes only the interleaving.
const GRID_BOUND: usize = 64 * MIB;

/// Worst case of the wide shape: a batch is held by its 8 MiB of text (twice
/// while its columns are built), up to four of them in flight, and the part
/// writer at 8 MiB.
const WIDE_BOUND: usize = 112 * MIB;
