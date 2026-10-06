//! The conversion's memory does not grow with the file.
//!
//! Own test binary because it installs a counting global allocator and
//! measures the peak of live heap bytes while a generated CSV of wide text
//! cells is converted to Parquet parts. The file is several times larger than
//! the bound the conversion states, so a pipeline that held the input (or a
//! batch of 8,192 wide rows) could not pass.

use async_trait::async_trait;
use bytes::Bytes;
use colmena::tabular_prepare::convert::{convert_csv_table, ConvertError, CsvSource};
use colmena::tabular_prepare::part_sink::{PartSink, SinkError};
use colmena::tabular_prepare::writer::WriterConfig;
use std::alloc::{GlobalAlloc, Layout, System};
use std::io::Read;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

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

/// `rows` rows of a wide text cell and a number, generated as they are read.
struct Wide {
    rows: usize,
    width: usize,
    next: usize,
    pending: Vec<u8>,
    pos: usize,
}

impl Wide {
    fn new(rows: usize, width: usize) -> Self {
        Self {
            rows,
            width,
            next: 0,
            pending: b"text,n\n".to_vec(),
            pos: 0,
        }
    }
}

impl Read for Wide {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.pos == self.pending.len() {
            if self.next == self.rows {
                return Ok(0);
            }
            self.pending.clear();
            self.pos = 0;
            // Varied but compressible text, so the parts are real work.
            let word = format!("row{}-", self.next);
            while self.pending.len() < self.width {
                self.pending.extend_from_slice(word.as_bytes());
            }
            self.pending.truncate(self.width);
            self.pending
                .extend_from_slice(format!(",{}\n", self.next).as_bytes());
            self.next += 1;
        }
        let n = buf.len().min(self.pending.len() - self.pos);
        buf[..n].copy_from_slice(&self.pending[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

struct WideSource(usize, usize);

#[async_trait]
impl CsvSource for WideSource {
    async fn open(
        &self,
        _cancel: &tokio_util::sync::CancellationToken,
    ) -> Result<Box<dyn Read + Send>, ConvertError> {
        Ok(Box::new(Wide::new(self.0, self.1)))
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

const MIB: usize = 1024 * 1024;

/// Peak live heap bytes above the starting level while converting.
async fn peak_of(rows: usize, width: usize, part_bytes: usize) -> (usize, usize) {
    let sink = Arc::new(Discard::default());
    let cfg = WriterConfig {
        max_rows: 500_000,
        max_bytes: part_bytes,
    };
    let base = LIVE.load(Ordering::SeqCst);
    PEAK.store(base, Ordering::SeqCst);
    let t = convert_csv_table(&WideSource(rows, width), sink.clone(), 0, cfg)
        .await
        .unwrap_or_else(|f| panic!("conversion failed: {}", f.error));
    assert_eq!(t.written.rows, rows as u64);
    (
        PEAK.load(Ordering::SeqCst) - base,
        sink.0.load(Ordering::SeqCst),
    )
}

/// A grid of `cols` columns and `rows` rows of short numeric cells, generated
/// as it is read. Its header names are `c0`, `c1`, ...
struct Grid {
    cols: usize,
    rows: usize,
    next: usize,
    pending: Vec<u8>,
    pos: usize,
}

impl Grid {
    fn new(cols: usize, rows: usize) -> Self {
        let header = (0..cols)
            .map(|i| format!("c{i}"))
            .collect::<Vec<_>>()
            .join(",");
        Self {
            cols,
            rows,
            next: 0,
            pending: format!("{header}\n").into_bytes(),
            pos: 0,
        }
    }
}

impl Read for Grid {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.pos == self.pending.len() {
            if self.next == self.rows {
                return Ok(0);
            }
            self.pending.clear();
            self.pos = 0;
            for c in 0..self.cols {
                if c > 0 {
                    self.pending.push(b',');
                }
                self.pending
                    .extend_from_slice(format!("{}", 10_000_000 + self.next * 7 + c).as_bytes());
            }
            self.pending.push(b'\n');
            self.next += 1;
        }
        let n = buf.len().min(self.pending.len() - self.pos);
        buf[..n].copy_from_slice(&self.pending[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

struct GridSource(usize, usize);

#[async_trait]
impl CsvSource for GridSource {
    async fn open(
        &self,
        _cancel: &tokio_util::sync::CancellationToken,
    ) -> Result<Box<dyn Read + Send>, ConvertError> {
        Ok(Box::new(Grid::new(self.0, self.1)))
    }
}

/// Peak live heap bytes above the starting level of a conversion that is
/// expected to succeed or fail with the result `check` accepts.
async fn peak_of_grid(cols: usize, rows: usize, part_bytes: usize) -> (usize, Result<u64, String>) {
    let sink = Arc::new(Discard::default());
    let cfg = WriterConfig {
        max_rows: 500_000,
        max_bytes: part_bytes,
    };
    let base = LIVE.load(Ordering::SeqCst);
    PEAK.store(base, Ordering::SeqCst);
    let result = convert_csv_table(&GridSource(cols, rows), sink, 0, cfg).await;
    let peak = PEAK.load(Ordering::SeqCst) - base;
    (
        peak,
        result
            .map(|t| t.written.rows)
            .map_err(|f| f.error.to_string()),
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn memory_is_bounded_whatever_the_size_of_the_file() {
    let part = 8 * MIB;
    // 96 KiB per row, wide enough that 8,192 rows would be 768 MiB.
    let width = 96 * 1024;
    let (small, _) = peak_of(700, width, part).await; // about 67 MB
    let (large, put) = peak_of(2800, width, part).await; // about 270 MB
    eprintln!(
        "peak small {} MiB, large {} MiB, parts {} MiB",
        small / MIB,
        large / MIB,
        put / MIB
    );
    assert!(put > 0);
    // The worst case the `convert` module states for these settings (a part
    // of 8 MiB): 94 in flight + 2 x (8 + 21) the writer + 43 the sample + 2
    // of buffers = 197 MiB. No scenario below reaches it, so each has its own
    // bound: the peak measured (debug build, steady within 1 MiB over repeated
    // runs) plus a margin, small enough that the slack that was removed
    // (doubled record copies, kilobytes per column builder) fails it.
    let ceiling = (94 + 2 * (8 + 21) + 43 + 2) * MIB;
    // 96 KiB rows: measured 55 MiB (a batch is 85 rows, 8 MiB of text), +16%.
    let narrow_bound = 64 * MIB;
    assert!(narrow_bound <= ceiling);
    assert!(
        large <= narrow_bound,
        "peak {} MiB over the {} MiB bound",
        large / MIB,
        narrow_bound / MIB
    );
    // It is the same peak for a file four times larger, not four times more.
    assert!(large <= small + small / 2 + 8 * MIB, "{small} then {large}");

    // A wide table that still fits the registry row (the table list is capped
    // at 64 KiB, which is about 900 columns): 700 columns, 75 MB.
    let (wide, rows) = peak_of_grid(700, 12_000, part).await;
    assert_eq!(rows, Ok(12_000));
    eprintln!("peak wide (700 columns) {} MiB", wide / MIB);
    // Measured 96 MiB: the sample (2,000,000 cells of 8 bytes, kept at their
    // exact size: 33 MiB), a batch of 1,000,000 cells in the reader (records
    // 16 MiB, then columns 12 MiB), two typed batches in the channel and one in
    // the writer (8 MiB of numbers each), and the part writer. How many typed
    // batches wait in the channel at the peak depends on how the reader and
    // the writer are scheduled: a slower runner measured 109 MiB, one batch
    // more. Bound: the measure, that one batch (12 MiB), and a margin.
    let wide_bound = 120 * MIB;
    assert!(wide_bound <= ceiling);
    assert!(
        wide <= wide_bound,
        "wide: peak {} MiB over the {} MiB bound",
        wide / MIB,
        wide_bound / MIB
    );

    // Wider than any table list can record: the conversion stops after the
    // header and the type sample, so what it holds is the sample, whatever the
    // file. 16,384 columns is the widest the reader accepts. The sample is at
    // most 35 MiB (16 MiB of text, 2,000,000 fields at 8 bytes, 2 MiB of
    // buffers, the records' own structs) plus header-sized structures (names,
    // schema, the table-list check) of about 6 MiB at 16,384 columns. Measured
    // 32 and 39 MiB; bounds +18%. Doubled record copies: 44 and 54 MiB.
    for (cols, sample_bound) in [(3_000usize, 38 * MIB), (16_384, 46 * MIB)] {
        let (peak, result) = peak_of_grid(cols, 2_000_000, part).await;
        eprintln!("peak refused ({cols} columns) {} MiB", peak / MIB);
        assert!(
            matches!(&result, Err(e) if e.contains("table list")),
            "{cols} columns: {result:?}"
        );
        assert!(
            peak <= sample_bound,
            "{cols} columns: peak {} MiB over the sample bound",
            peak / MIB
        );
    }
}
