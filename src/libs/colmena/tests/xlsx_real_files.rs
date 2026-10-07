//! Workbooks written by real writers (openpyxl, LibreOffice), converted and compared.
//!
//! `committed_fixtures_convert_to_the_values_recorded_with_them` converts every
//! `tests/fixtures/xlsx/*.xlsx` and compares what it makes with the `.expected.json`
//! beside it, which `tests/fixtures/xlsx/compare.py` has verified cell by cell against
//! what openpyxl reads back from the same file. `dump_a_directory` is the ignored
//! harness that does the conversion for any directory of workbooks:
//!
//! ```text
//! XLSX_REAL_DIR=dir XLSX_REAL_OUT=out cargo test --test xlsx_real_files dump_a_directory -- --ignored
//! ```

use async_trait::async_trait;
use bytes::Bytes;
use colmena::tabular_prepare::convert::ConvertControl;
use colmena::tabular_prepare::manifest::{part_path, unique_table_names};
use colmena::tabular_prepare::part_sink::{PartSink, SinkError};
use colmena::tabular_prepare::writer::WriterConfig;
use colmena::tabular_prepare::xlsx_convert::{convert_xlsx, XlsxSource};
use colmena::tabular_prepare::xlsx_spool::{spool_stream, Spooled, XlsxError, MAX_XLSX_BYTES};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

struct FileSource(PathBuf);

#[async_trait]
impl XlsxSource for FileSource {
    async fn spool(&self, cancel: &CancellationToken) -> Result<Spooled, XlsxError> {
        let bytes = std::fs::read(&self.0).unwrap();
        let stream = Box::pin(futures::stream::iter(vec![Ok(Bytes::from(bytes))]));
        let read = AtomicU64::new(0);
        spool_stream(
            &std::env::temp_dir(),
            stream,
            None,
            MAX_XLSX_BYTES,
            cancel,
            &read,
        )
        .await
    }
}

#[derive(Default)]
struct Capture(Mutex<BTreeMap<String, Bytes>>);

#[async_trait]
impl PartSink for Capture {
    async fn put(&self, path: &str, data: Bytes) -> Result<(), SinkError> {
        self.0.lock().unwrap().insert(path.to_string(), data);
        Ok(())
    }
}

/// What converting the workbook at `path` makes, as JSON: per table its sheet, name,
/// columns (name and type) and every row as text (null for a null); or the error.
async fn dump(path: &Path) -> Value {
    let capture = Arc::new(Capture::default());
    let control = ConvertControl::new();
    let result = convert_xlsx(
        &FileSource(path.to_path_buf()),
        capture.clone(),
        WriterConfig::default(),
        &control,
    )
    .await;
    let converted = match result {
        Ok(c) => c,
        Err(f) => return json!({ "error": f.error.to_string() }),
    };
    let parts = capture.0.lock().unwrap();
    let tables: Vec<Value> = converted
        .tables
        .iter()
        .enumerate()
        .map(|(idx, t)| {
            let columns: Vec<Value> = t
                .table
                .written
                .columns
                .iter()
                .map(|c| json!({ "name": c.name, "type": format!("{:?}", c.column_type).to_lowercase() }))
                .collect();
            let mut rows: Vec<Value> = Vec::new();
            for p in 0..t.table.written.parts as usize {
                let bytes = parts[&part_path(idx, p).unwrap()].clone();
                for batch in ParquetRecordBatchReaderBuilder::try_new(bytes).unwrap().build().unwrap() {
                    let batch = batch.unwrap();
                    for r in 0..batch.num_rows() {
                        let row: Vec<Value> = (0..batch.num_columns())
                            .map(|c| {
                                let col = batch.column(c);
                                if col.is_null(r) {
                                    Value::Null
                                } else {
                                    Value::String(arrow_cast::display::array_value_to_string(col, r).unwrap())
                                }
                            })
                            .collect();
                        rows.push(Value::Array(row));
                    }
                }
            }
            json!({ "sheet": t.sheet, "name": converted.table_names[idx], "columns": columns, "rows": rows })
        })
        .collect();
    let skipped: Vec<Value> = converted
        .skipped
        .iter()
        .map(|s| json!({ "sheet": s.sheet, "reason": s.reason }))
        .collect();
    let _ = unique_table_names;
    json!({ "tables": tables, "skipped": skipped })
}

fn workbooks(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "xlsx"))
        .collect();
    v.sort();
    v
}

#[tokio::test]
#[ignore = "harness: set XLSX_REAL_DIR and XLSX_REAL_OUT"]
async fn dump_a_directory() {
    let dir = std::env::var("XLSX_REAL_DIR").expect("XLSX_REAL_DIR");
    let out = PathBuf::from(std::env::var("XLSX_REAL_OUT").expect("XLSX_REAL_OUT"));
    std::fs::create_dir_all(&out).unwrap();
    for path in workbooks(Path::new(&dir)) {
        let name = path.file_stem().unwrap().to_string_lossy().into_owned();
        let value = dump(&path).await;
        std::fs::write(
            out.join(format!("{name}.json")),
            serde_json::to_string(&value).unwrap(),
        )
        .unwrap();
    }
}

#[tokio::test]
async fn committed_fixtures_convert_to_the_values_recorded_with_them() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/xlsx");
    let files = workbooks(&dir);
    assert!(!files.is_empty(), "no fixtures in {dir:?}");
    for path in files {
        let expected_path = path.with_extension("expected.json");
        let expected: Value =
            serde_json::from_slice(&std::fs::read(&expected_path).unwrap()).unwrap();
        let got = dump(&path).await;
        assert_eq!(got, expected, "{}", path.display());
    }
}
