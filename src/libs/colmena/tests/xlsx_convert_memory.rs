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

/// The counters are global: one test measures at a time.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

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
    /// `cols` short numeric cells per row, up to Excel's 16,384 columns.
    Columns { rows: usize, cols: usize },
    /// One numeric column with a text cell after the 10,000 rows the types come from:
    /// the sheet is read again as text.
    Late { rows: usize },
}

/// The name of column `i` (from zero): `A` to `XFD`.
fn column(i: usize) -> String {
    let mut n = i + 1;
    let mut name = Vec::new();
    while n > 0 {
        name.push(b'A' + ((n - 1) % 26) as u8);
        n = (n - 1) / 26;
    }
    name.reverse();
    String::from_utf8(name).unwrap()
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
        Shape::Columns { rows, cols } => {
            for r in 1..=rows {
                let cells: String = (0..cols)
                    .map(|c| format!("<c r=\"{}{r}\"><v>{}</v></c>", column(c), r * 7 + c))
                    .collect();
                emit(&format!("<row r=\"{r}\">{cells}</row>"));
            }
        }
        Shape::Late { rows } => {
            emit("<row r=\"1\"><c r=\"A1\" t=\"inlineStr\"><is><t>n</t></is></c></row>");
            for r in 2..=rows + 1 {
                let cell = if r == 10_006 {
                    format!("<c r=\"A{r}\" t=\"inlineStr\"><is><t>N/A</t></is></c>")
                } else {
                    format!("<c r=\"A{r}\"><v>{r}</v></c>")
                };
                emit(&format!("<row r=\"{r}\">{cell}</row>"));
            }
        }
    }
    emit("</sheetData></worksheet>");
}

/// Writes a workbook of one sheet per shape to `path`, streaming; returns its size.
fn write_workbook(path: &Path, shapes: &[Shape], method: CompressionMethod) -> u64 {
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
    let sheets: String = (1..=shapes.len())
        .map(|i| format!("<sheet name=\"Data{i}\" r:id=\"rId{i}\"/>"))
        .collect();
    put(
        "xl/workbook.xml",
        &format!("<workbook xmlns:r=\"{ns}\"><sheets>{sheets}</sheets></workbook>"),
    );
    let mut rels: String = (1..=shapes.len())
        .map(|i| format!("<Relationship Id=\"rId{i}\" Type=\"{ns}/worksheet\" Target=\"worksheets/sheet{i}.xml\"/>"))
        .collect();
    rels.push_str(&format!("<Relationship Id=\"rIdS\" Type=\"{ns}/sharedStrings\" Target=\"sharedStrings.xml\"/><Relationship Id=\"rIdT\" Type=\"{ns}/styles\" Target=\"styles.xml\"/>"));
    put(
        "xl/_rels/workbook.xml.rels",
        &format!("<Relationships>{rels}</Relationships>"),
    );
    put(
        "xl/styles.xml",
        "<styleSheet><cellXfs><xf numFmtId=\"0\"/><xf numFmtId=\"14\"/></cellXfs></styleSheet>",
    );
    let unique = shapes
        .iter()
        .map(|s| match s {
            Shape::Grid { unique_strings, .. } => *unique_strings,
            _ => 0,
        })
        .max()
        .unwrap_or(0);
    zip.start_file("xl/sharedStrings.xml", stored).unwrap();
    zip.write_all(b"<sst>").unwrap();
    for i in 0..unique {
        zip.write_all(
            format!("<si><t>shared string number {i} with some padding</t></si>").as_bytes(),
        )
        .unwrap();
    }
    zip.write_all(b"</sst>").unwrap();
    for (i, shape) in shapes.iter().enumerate() {
        zip.start_file(
            format!("xl/worksheets/sheet{}.xml", i + 1),
            FileOptions::default().compression_method(method),
        )
        .unwrap();
        sheet_rows(*shape, |chunk| zip.write_all(chunk.as_bytes()).unwrap());
    }
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

/// The production writer settings: parts of 500,000 rows or 64 MiB.
fn production() -> WriterConfig {
    WriterConfig::default()
}

/// The 8 MiB parts the first scenarios use, so that their parts close within the file.
fn small_parts() -> WriterConfig {
    WriterConfig {
        max_rows: 500_000,
        max_bytes: 8 * MIB,
    }
}

/// Converts the workbook at `path` and returns the result and the peak of live heap
/// bytes above the starting level.
async fn measure(path: &Path, cfg: WriterConfig) -> (Result<Vec<u64>, String>, usize) {
    let sink = Arc::new(Discard::default());
    let base = LIVE.load(Ordering::SeqCst);
    PEAK.store(base, Ordering::SeqCst);
    let result = convert_xlsx(
        &FileSource(path.to_path_buf()),
        sink.clone(),
        cfg,
        &ConvertControl::new(),
    )
    .await
    .map(|t| t.iter().map(|t| t.table.written.rows).collect())
    .map_err(|f| f.error.to_string());
    (result, PEAK.load(Ordering::SeqCst) - base)
}

/// The workbook at `path` converted to tables of the given row counts, and the peak.
async fn peak_of(path: &Path, rows: &[usize], cfg: WriterConfig) -> usize {
    let (result, peak) = measure(path, cfg).await;
    let want: Vec<u64> = rows.iter().map(|r| *r as u64).collect();
    assert_eq!(
        result.unwrap_or_else(|e| panic!("conversion failed: {e}")),
        want
    );
    peak
}

#[tokio::test(flavor = "multi_thread")]
async fn memory_is_bounded_whatever_the_size_of_the_workbook() {
    let _one_at_a_time = SERIAL.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("book.xlsx");

    // Many short rows, with a shared-strings table of 50,000 entries.
    let grid = |rows| Shape::Grid {
        rows,
        unique_strings: 50_000,
    };
    let small_size = write_workbook(&path, &[grid(100_000)], CompressionMethod::Stored);
    let small = peak_of(&path, &[100_000], small_parts()).await;
    let large_size = write_workbook(&path, &[grid(400_000)], CompressionMethod::Stored);
    let large = peak_of(&path, &[400_000], small_parts()).await;
    eprintln!(
        "grid: {} MiB -> peak {} MiB, {} MiB -> peak {} MiB",
        small_size / MIB as u64,
        small / MIB,
        large_size / MIB as u64,
        large / MIB
    );
    assert!(large_size > 3 * small_size);
    assert!(large <= small + small / 4 + 8 * MIB, "{small} then {large}");
    assert!(large <= grid_bound(8 * MIB), "peak {} MiB", large / MIB);

    // Wide text cells: four of 16 KiB per row, so a batch is held by its 8 MiB of
    // text and not by its rows.
    let wide = |rows| Shape::Wide {
        rows,
        cells: 4,
        width: 16 * 1024,
    };
    let small_size = write_workbook(&path, &[wide(300)], CompressionMethod::Stored);
    let small = peak_of(&path, &[300], small_parts()).await;
    let large_size = write_workbook(&path, &[wide(1200)], CompressionMethod::Stored);
    let large = peak_of(&path, &[1200], small_parts()).await;
    eprintln!(
        "wide: {} MiB -> peak {} MiB, {} MiB -> peak {} MiB",
        small_size / MIB as u64,
        small / MIB,
        large_size / MIB as u64,
        large / MIB
    );
    assert!(large_size > 3 * small_size);
    assert!(large <= small + small / 4 + 8 * MIB, "{small} then {large}");
    assert!(large <= wide_bound(8 * MIB), "peak {} MiB", large / MIB);

    // The same grid deflated, so the inflate path is inside the measurement too.
    let deflated = write_workbook(&path, &[grid(150_000)], CompressionMethod::Deflated);
    let peak = peak_of(&path, &[150_000], small_parts()).await;
    eprintln!(
        "deflated grid: {} MiB -> peak {} MiB",
        deflated / MIB as u64,
        peak / MIB
    );
    assert!(peak <= grid_bound(8 * MIB), "peak {} MiB", peak / MIB);
}

// The bounds are derived from the constants of the design, named here. Each is
// pinned to the production constant by `the_constants_the_bounds_come_from_...`, so a
// batch twice as large changes a constant this test asserts, not only a peak.

/// Bytes of text one batch holds at most (the reader's byte budget).
const BATCH_TEXT: usize = 8 * MIB;
/// Batches alive at once, worst case: one being built (its records, then its
/// columns, twice the text), two in the channel, one being written.
const BATCHES_IN_FLIGHT_TEXT: usize = 2 * BATCH_TEXT + 2 * BATCH_TEXT + BATCH_TEXT;
/// The shared-strings table's reserve for a workbook of this test (50,000 strings of
/// about 50 bytes), four bytes of offset each, rounded up.
const STRINGS: usize = 8 * MIB;
/// What is not a batch, a part or the strings: one XML event (1 MiB), the spool
/// chunk, the central directory, the cells of one row and the buffers of the runtime.
const FIXED: usize = 8 * MIB;
/// The part writer holds an encoded row group and its output: twice the part limit,
/// plus the slice being added (8 MiB).
fn part_cost(part_bytes: usize) -> usize {
    2 * part_bytes + 8 * MIB
}

/// Worst case of the short-row shape: its batches are a few hundred KiB, so only the
/// part writer and the strings count; the batches are covered by `FIXED`.
fn grid_bound(part_bytes: usize) -> usize {
    part_cost(part_bytes) + STRINGS + FIXED + 8 * MIB
}

/// Worst case of the wide-text shape: batches held by their text, in flight.
fn wide_bound(part_bytes: usize) -> usize {
    BATCHES_IN_FLIGHT_TEXT + part_cost(part_bytes) + FIXED
}

#[test]
fn the_constants_the_bounds_come_from_are_the_production_ones() {
    use colmena::tabular_prepare::csv::{BATCH_BYTES, BATCH_CELLS, BATCH_ROWS};
    use colmena::tabular_prepare::writer::{PART_MAX_BYTES, PART_MAX_ROWS};
    use colmena::tabular_prepare::xlsx_package::MAX_TOKEN_BYTES;
    assert_eq!(BATCH_BYTES, BATCH_TEXT);
    assert_eq!((BATCH_ROWS, BATCH_CELLS), (8192, 1_000_000));
    assert_eq!((PART_MAX_BYTES, PART_MAX_ROWS), (64 * MIB, 500_000));
    assert_eq!(MAX_TOKEN_BYTES as usize, MIB);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_production_part_size_a_workbook_of_sheets_and_a_restart_stay_within_their_bounds() {
    let _one_at_a_time = SERIAL.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("book.xlsx");
    let grid = |rows| Shape::Grid {
        rows,
        unique_strings: 50_000,
    };

    // Three sheets in one workbook: the peak is one sheet's, not three.
    let one = {
        write_workbook(&path, &[grid(100_000)], CompressionMethod::Stored);
        peak_of(&path, &[100_000], production()).await
    };
    write_workbook(
        &path,
        &[grid(100_000), grid(100_000), grid(100_000)],
        CompressionMethod::Stored,
    );
    let three = peak_of(&path, &[100_000, 100_000, 100_000], production()).await;
    eprintln!("sheets: one {} MiB, three {} MiB", one / MIB, three / MIB);
    assert!(three <= one + one / 4 + 8 * MIB, "{one} then {three}");
    assert!(three <= grid_bound(64 * MIB), "peak {} MiB", three / MIB);

    // The wide-text shape with the production 64 MiB parts.
    let wide = Shape::Wide {
        rows: 1500,
        cells: 4,
        width: 16 * 1024,
    };
    write_workbook(&path, &[wide], CompressionMethod::Stored);
    let peak = peak_of(&path, &[1500], production()).await;
    eprintln!("wide, production parts: {} MiB", peak / MIB);
    assert!(peak <= wide_bound(64 * MIB), "peak {} MiB", peak / MIB);

    // A sheet read again as text after a late conflict: the second read costs no more.
    write_workbook(
        &path,
        &[Shape::Late { rows: 120_000 }],
        CompressionMethod::Stored,
    );
    let restarted = peak_of(&path, &[120_000], production()).await;
    eprintln!("restart: {} MiB", restarted / MIB);
    assert!(
        restarted <= grid_bound(64 * MIB),
        "peak {} MiB",
        restarted / MIB
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sheet_of_16384_columns_is_refused_after_its_sample_with_bounded_memory() {
    let _one_at_a_time = SERIAL.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("book.xlsx");
    // 16,384 columns by 200 rows: 3.3 million cells. The table list (64 KiB, about 900
    // columns) cannot hold it, so it is refused after the header and the sample.
    write_workbook(
        &path,
        &[Shape::Columns {
            rows: 200,
            cols: 16_384,
        }],
        CompressionMethod::Stored,
    );
    let (result, peak) = measure(&path, production()).await;
    let error = result.unwrap_err();
    assert!(error.contains("table list"), "{error}");
    // The sample of 200 rows of 16,384 cells and the names: well under the batch
    // budget plus the fixed cost.
    assert!(
        peak <= BATCHES_IN_FLIGHT_TEXT + FIXED,
        "peak {} MiB",
        peak / MIB
    );
    eprintln!("16,384 columns: refused at {} MiB", peak / MIB);
}
