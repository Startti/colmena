//! Parquet part writer for prepared tables (dark behind `COLMENA_LARGE_TABULAR`).
//!
//! Settings are the ones the Phase 0 spike measured: ZSTD level 3,
//! dictionary encoding on, chunk statistics, one row group per part. A part is
//! built in memory and handed to the [`PartSink`] as one buffer with a known
//! length, so no more than one part (about [`PART_MAX_BYTES`] plus one batch)
//! is ever held, however large the table.

use crate::tabular_prepare::manifest::{
    part_path, ColumnInfo, ColumnType, ManifestError, MAX_COLUMN_NAME_CHARS, MAX_PARTS, MAX_TABLES,
};
use crate::tabular_prepare::part_sink::{PartSink, SinkError};
use arrow_array::{Array, ArrayRef, RecordBatch, StringArray};
use arrow_schema::{DataType, SchemaRef, TimeUnit};
use bytes::Bytes;
use parquet::arrow::ArrowWriter;
use parquet::basic::{Compression, ZstdLevel};
use parquet::file::properties::{EnabledStatistics, WriterProperties};
use std::sync::Arc;
use thiserror::Error;

/// A part is closed at this many rows, whichever limit is hit first.
pub const PART_MAX_ROWS: usize = 500_000;

/// A part is closed once its encoded size reaches this. The check runs after
/// each batch, so a part overshoots by at most one batch.
pub const PART_MAX_BYTES: usize = 64 * 1024 * 1024;

/// A batch is written in slices of about this many bytes, so one oversized
/// batch is never encoded whole and a part overshoots its byte limit by at most
/// one slice, whatever the size of the batch handed in.
pub const WRITE_SLICE_BYTES: usize = 8 * 1024 * 1024;

const ZSTD_LEVEL: i32 = 3;

#[derive(Debug, Error)]
pub enum WriterError {
    #[error("parquet: {0}")]
    Parquet(String),
    #[error(transparent)]
    Sink(#[from] SinkError),
    #[error(transparent)]
    Manifest(#[from] ManifestError),
    #[error("column type {0} cannot be written")]
    UnsupportedType(String),
    #[error("the batch does not match the table schema")]
    SchemaMismatch,
    #[error("the writer already failed; its output is incomplete")]
    Poisoned,
    #[error("invalid writer settings: {0}")]
    Config(String),
}

/// The encoder's blocking task did not finish (it panicked, or the runtime is
/// shutting down): a typed error, and the writer is closed by it.
fn encoder_stopped(e: tokio::task::JoinError) -> WriterError {
    WriterError::Parquet(if e.is_panic() {
        "the encoder panicked".into()
    } else {
        "the encoder was cancelled".into()
    })
}

impl From<parquet::errors::ParquetError> for WriterError {
    fn from(e: parquet::errors::ParquetError) -> Self {
        WriterError::Parquet(e.to_string())
    }
}

#[derive(Debug, Clone, Copy)]
pub struct WriterConfig {
    pub max_rows: usize,
    pub max_bytes: usize,
}

impl Default for WriterConfig {
    fn default() -> Self {
        Self {
            max_rows: PART_MAX_ROWS,
            max_bytes: PART_MAX_BYTES,
        }
    }
}

/// What a finished table looks like, ready for the manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableWritten {
    pub rows: u64,
    pub parts: u32,
    pub columns: Vec<ColumnInfo>,
}

/// The writer settings of a part. One row group per part: the row limit of
/// the group equals the row limit of the part, and the writer is fed slices
/// that never cross it.
pub fn part_properties(max_rows: usize) -> WriterProperties {
    WriterProperties::builder()
        .set_compression(Compression::ZSTD(
            ZstdLevel::try_new(ZSTD_LEVEL).expect("a valid zstd level"),
        ))
        .set_dictionary_enabled(true)
        .set_statistics_enabled(EnabledStatistics::Chunk)
        .set_max_row_group_row_count(Some(max_rows))
        .build()
}

/// The manifest type of an Arrow type, or an error for one the converter
/// does not produce.
pub fn column_type_of(t: &DataType) -> Result<ColumnType, WriterError> {
    match t {
        DataType::Int64 => Ok(ColumnType::Int),
        DataType::Float64 => Ok(ColumnType::Float),
        DataType::Boolean => Ok(ColumnType::Bool),
        DataType::Utf8 => Ok(ColumnType::String),
        DataType::Date32 => Ok(ColumnType::Date),
        DataType::Timestamp(TimeUnit::Microsecond, None) => Ok(ColumnType::Timestamp),
        other => Err(WriterError::UnsupportedType(other.to_string())),
    }
}

/// What a column slice takes decoded in memory, as Arrow buffers: the values,
/// the offsets of a string column and the validity bits when it has nulls. This
/// is what a reader needs to hold the column, whatever Parquet stored.
fn decoded_bytes(col: &ArrayRef) -> u64 {
    let n = col.len() as u64;
    let validity = if col.null_count() > 0 {
        n.div_ceil(8)
    } else {
        0
    };
    let body = match col.data_type() {
        DataType::Int64 | DataType::Float64 | DataType::Timestamp(..) => 8 * n,
        DataType::Date32 => 4 * n,
        DataType::Boolean => n.div_ceil(8),
        DataType::Utf8 => col.as_any().downcast_ref::<StringArray>().map_or(0, |a| {
            let o = a.value_offsets();
            u64::from((o[o.len() - 1] - o[0]) as u32) + 4 * (n + 1)
        }),
        _ => col.get_array_memory_size() as u64,
    };
    body + validity
}

/// Writes one table as a series of Parquet parts.
pub struct PartWriter {
    sink: Arc<dyn PartSink>,
    table_idx: usize,
    schema: SchemaRef,
    types: Vec<ColumnType>,
    cfg: WriterConfig,
    current: Option<ArrowWriter<Vec<u8>>>,
    rows_in_part: usize,
    parts_done: usize,
    rows_total: u64,
    column_bytes: Vec<u64>,
    memory_bytes: Vec<u64>,
    attempted: Vec<String>,
    poisoned: bool,
}

impl PartWriter {
    pub fn new(
        sink: Arc<dyn PartSink>,
        table_idx: usize,
        schema: SchemaRef,
        cfg: WriterConfig,
    ) -> Result<Self, WriterError> {
        if cfg.max_rows == 0 || cfg.max_bytes == 0 {
            return Err(WriterError::Config("limits must be above zero".into()));
        }
        if table_idx >= MAX_TABLES {
            return Err(WriterError::Config(format!("table index {table_idx}")));
        }
        if schema.fields().is_empty() {
            return Err(WriterError::Config("a table needs a column".into()));
        }
        let mut names = std::collections::HashSet::new();
        for f in schema.fields() {
            let n = f.name();
            if n.is_empty()
                || n.chars().count() > MAX_COLUMN_NAME_CHARS
                || n.chars().any(char::is_control)
                || !names.insert(n.as_str())
            {
                return Err(WriterError::Config(
                    "column names must be unique, non-empty, clean and short".into(),
                ));
            }
        }
        let types = schema
            .fields()
            .iter()
            .map(|f| column_type_of(f.data_type()))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            sink,
            table_idx,
            column_bytes: vec![0; types.len()],
            memory_bytes: vec![0; types.len()],
            types,
            schema,
            cfg,
            current: None,
            rows_in_part: 0,
            parts_done: 0,
            rows_total: 0,
            attempted: Vec::new(),
            poisoned: false,
        })
    }

    /// Estimated bytes held by the part being built (what is encoded plus what
    /// the open row group will encode to). Below `max_bytes` whenever a call to
    /// [`write`](Self::write) returns.
    pub fn buffered_bytes(&self) -> usize {
        self.current
            .as_ref()
            .map_or(0, |w| w.bytes_written() + w.in_progress_size())
    }

    /// Every part path handed to the sink, including one whose put failed (it
    /// may exist partly). The caller removes these when the conversion fails.
    pub fn attempted_paths(&self) -> &[String] {
        &self.attempted
    }

    /// Any error poisons the writer: nothing more is accepted and `finish`
    /// refuses, so a failed table is never reported as written.
    pub async fn write(&mut self, batch: &RecordBatch) -> Result<(), WriterError> {
        if self.poisoned {
            return Err(WriterError::Poisoned);
        }
        if batch.schema() != self.schema {
            return Err(WriterError::SchemaMismatch);
        }
        let result = self.write_batch(batch).await;
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }

    async fn write_batch(&mut self, batch: &RecordBatch) -> Result<(), WriterError> {
        let mut offset = 0;
        // Rows per slice from the average row size of this batch.
        let per_row = (batch.get_array_memory_size() / batch.num_rows().max(1)).max(1);
        let slice_rows = (WRITE_SLICE_BYTES / per_row).max(1);
        while offset < batch.num_rows() {
            let take = (self.cfg.max_rows - self.rows_in_part)
                .min(batch.num_rows() - offset)
                .min(slice_rows);
            if take == 0 {
                // Unreachable while a full part is rolled right away; a loop
                // that cannot advance must fail, never spin.
                return Err(WriterError::Config(
                    "the part limit was reached but not rolled".into(),
                ));
            }
            if self.current.is_none() {
                self.current = Some(self.open_part()?);
            }
            let slice = batch.slice(offset, take);
            for (total, col) in self.memory_bytes.iter_mut().zip(slice.columns()) {
                *total += decoded_bytes(col);
            }
            // Encoding and compressing a slice is CPU work of tens of
            // milliseconds: it runs on the blocking pool, not on the async
            // worker the rest of the service shares.
            let mut writer = self.current.take().expect("opened above");
            let (writer, written) = tokio::task::spawn_blocking(move || {
                let written = writer.write(&slice);
                (writer, written)
            })
            .await
            .map_err(encoder_stopped)?;
            self.current = Some(writer);
            written?;
            self.rows_in_part += take;
            offset += take;
            if self.rows_in_part >= self.cfg.max_rows || self.buffered_bytes() >= self.cfg.max_bytes
            {
                self.roll().await?;
            }
        }
        Ok(())
    }

    fn open_part(&self) -> Result<ArrowWriter<Vec<u8>>, WriterError> {
        // The path is checked when the part opens, so nothing is encoded for a
        // part that could not be named.
        part_path(self.table_idx, self.parts_done)?;
        Ok(ArrowWriter::try_new(
            Vec::new(),
            self.schema.clone(),
            Some(part_properties(self.cfg.max_rows)),
        )?)
    }

    /// Closes the open part and hands it to the sink.
    async fn roll(&mut self) -> Result<(), WriterError> {
        let writer = self.current.take().expect("a part is open");
        let (metadata, data) = tokio::task::spawn_blocking(move || {
            let mut writer = writer;
            let metadata = writer.finish()?;
            Ok::<_, parquet::errors::ParquetError>((
                metadata,
                Bytes::from(std::mem::take(writer.inner_mut())),
            ))
        })
        .await
        .map_err(encoder_stopped)??;
        for rg in metadata.row_groups() {
            for (total, col) in self.column_bytes.iter_mut().zip(rg.columns()) {
                *total += col.uncompressed_size().max(0) as u64;
            }
        }
        let path = part_path(self.table_idx, self.parts_done)?;
        self.attempted.push(path.clone());
        self.sink.put(&path, data).await?;
        self.parts_done += 1;
        self.rows_total += self.rows_in_part as u64;
        self.rows_in_part = 0;
        Ok(())
    }

    /// Writes the last part and returns the table's totals. A table with no
    /// rows still gets one part, so its schema can be read.
    ///
    /// Takes `&mut self` so that `attempted_paths` can still be read when the
    /// last put fails. The writer is closed afterwards, whatever the outcome.
    pub async fn finish(&mut self) -> Result<TableWritten, WriterError> {
        if self.poisoned {
            return Err(WriterError::Poisoned);
        }
        self.poisoned = true;
        if self.current.is_none() && self.parts_done == 0 {
            self.current = Some(self.open_part()?);
        }
        if self.current.is_some() {
            self.roll().await?;
        }
        let columns = self
            .schema
            .fields()
            .iter()
            .zip(&self.types)
            .zip(self.column_bytes.iter().zip(&self.memory_bytes))
            .map(|((f, t), (bytes, memory))| ColumnInfo {
                name: f.name().clone(),
                column_type: *t,
                uncompressed_bytes: *bytes,
                in_memory_bytes: *memory,
            })
            .collect();
        Ok(TableWritten {
            rows: self.rows_total,
            parts: self.parts_done as u32,
            columns,
        })
    }
}

const _: () = assert!(MAX_PARTS <= u32::MAX as usize);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tabular_prepare::part_sink::fake::MemorySink;
    use arrow_array::{Int64Array, StringArray};
    use arrow_schema::{Field, Schema};
    use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;

    /// What the writer produces is a Parquet file any reader opens: the codec,
    /// the schema and every row are in the bytes the sink received.
    #[tokio::test]
    async fn parquet_smoke() {
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("name", DataType::Utf8, true),
        ]));
        let ids = Int64Array::from_iter_values(0..10);
        let names = StringArray::from_iter((0..10).map(|i| Some(format!("row-{i}"))));
        let batch =
            RecordBatch::try_new(schema.clone(), vec![Arc::new(ids), Arc::new(names)]).unwrap();
        let sink = Arc::new(MemorySink::default());
        let mut w =
            PartWriter::new(sink.clone(), 0, schema.clone(), WriterConfig::default()).unwrap();
        w.write(&batch).await.unwrap();
        w.finish().await.unwrap();

        let builder =
            ParquetRecordBatchReaderBuilder::try_new(sink.get("t0/part-00000.parquet").unwrap())
                .unwrap();
        // The file records the codec but not the level (the part writer's
        // properties pin the level).
        assert!(matches!(
            builder.metadata().row_group(0).column(0).compression(),
            Compression::ZSTD(_)
        ));
        assert_eq!(builder.schema().as_ref(), schema.as_ref());
        let back: Vec<RecordBatch> = builder.build().unwrap().map(|b| b.unwrap()).collect();
        assert_eq!(back, vec![batch]);
    }
}

#[cfg(test)]
mod part_writer {
    use super::*;
    use crate::tabular_prepare::manifest::ColumnType;
    use crate::tabular_prepare::part_sink::fake::MemorySink;
    use arrow_array::{
        BooleanArray, Date32Array, Float64Array, Int64Array, StringArray, TimestampMicrosecondArray,
    };
    use arrow_schema::{Field, Schema, TimeUnit};
    use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
    use parquet::file::metadata::ParquetMetaData;
    use parquet::schema::types::ColumnPath;

    fn small(max_rows: usize) -> WriterConfig {
        WriterConfig {
            max_rows,
            max_bytes: usize::MAX,
        }
    }

    fn int_schema() -> SchemaRef {
        Arc::new(Schema::new(vec![Field::new("id", DataType::Int64, false)]))
    }

    fn int_batch(range: std::ops::Range<i64>) -> RecordBatch {
        RecordBatch::try_new(
            int_schema(),
            vec![Arc::new(Int64Array::from_iter_values(range))],
        )
        .unwrap()
    }

    fn metadata_of(bytes: Bytes) -> ParquetMetaData {
        ParquetRecordBatchReaderBuilder::try_new(bytes)
            .unwrap()
            .metadata()
            .as_ref()
            .clone()
    }

    fn read_all(bytes: Bytes) -> Vec<RecordBatch> {
        ParquetRecordBatchReaderBuilder::try_new(bytes)
            .unwrap()
            .build()
            .unwrap()
            .map(|b| b.unwrap())
            .collect()
    }

    #[tokio::test]
    async fn every_supported_type_round_trips_with_nulls() {
        let schema = Arc::new(Schema::new(vec![
            Field::new("i", DataType::Int64, true),
            Field::new("f", DataType::Float64, true),
            Field::new("b", DataType::Boolean, true),
            Field::new("s", DataType::Utf8, true),
            Field::new("d", DataType::Date32, true),
            Field::new("t", DataType::Timestamp(TimeUnit::Microsecond, None), true),
            Field::new("n", DataType::Utf8, true),
        ]));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(Int64Array::from(vec![Some(1), None, Some(-3)])),
                Arc::new(Float64Array::from(vec![Some(1.5), Some(f64::MAX), None])),
                Arc::new(BooleanArray::from(vec![None, Some(true), Some(false)])),
                Arc::new(StringArray::from(vec![Some("00123"), Some(""), None])),
                Arc::new(Date32Array::from(vec![Some(0), None, Some(19_000)])),
                Arc::new(TimestampMicrosecondArray::from(vec![
                    None,
                    Some(1_700_000_000_000_000),
                    Some(-1),
                ])),
                Arc::new(StringArray::from(vec![None::<&str>, None, None])),
            ],
        )
        .unwrap();
        let sink = Arc::new(MemorySink::default());
        let mut w = PartWriter::new(sink.clone(), 0, schema, WriterConfig::default()).unwrap();
        w.write(&batch).await.unwrap();
        let done = w.finish().await.unwrap();

        assert_eq!((done.rows, done.parts), (3, 1));
        let types: Vec<_> = done.columns.iter().map(|c| c.column_type).collect();
        assert_eq!(
            types,
            vec![
                ColumnType::Int,
                ColumnType::Float,
                ColumnType::Bool,
                ColumnType::String,
                ColumnType::Date,
                ColumnType::Timestamp,
                ColumnType::String
            ]
        );
        let back = read_all(sink.get("t0/part-00000.parquet").unwrap());
        assert_eq!(back, vec![batch]);
    }

    #[test]
    fn part_properties_pin_zstd_3_dictionary_and_chunk_statistics() {
        let props = part_properties(1000);
        let col = ColumnPath::from("anything");
        assert_eq!(
            props.compression(&col),
            Compression::ZSTD(ZstdLevel::try_new(3).unwrap())
        );
        assert!(props.dictionary_enabled(&col));
        assert_eq!(props.statistics_enabled(&col), EnabledStatistics::Chunk);
        assert_eq!(props.max_row_group_row_count(), Some(1000));
    }

    #[tokio::test]
    async fn a_written_part_has_dictionary_statistics_and_one_row_group() {
        let schema = Arc::new(Schema::new(vec![Field::new("c", DataType::Utf8, false)]));
        let values: Vec<String> = (0..5000).map(|i| format!("v{}", i % 7)).collect();
        let batch = RecordBatch::try_new(schema.clone(), vec![Arc::new(StringArray::from(values))])
            .unwrap();
        let sink = Arc::new(MemorySink::default());
        let mut w = PartWriter::new(sink.clone(), 0, schema, WriterConfig::default()).unwrap();
        w.write(&batch).await.unwrap();
        w.finish().await.unwrap();
        let md = metadata_of(sink.get("t0/part-00000.parquet").unwrap());
        assert_eq!(md.num_row_groups(), 1);
        let chunk = md.row_group(0).column(0);
        assert!(
            chunk.dictionary_page_offset().is_some(),
            "no dictionary page"
        );
        assert!(chunk.statistics().is_some(), "no chunk statistics");
    }

    #[tokio::test]
    async fn parts_roll_at_max_rows_with_one_row_group_each() {
        let sink = Arc::new(MemorySink::default());
        let mut w = PartWriter::new(sink.clone(), 3, int_schema(), small(100)).unwrap();
        // One batch that crosses two boundaries, then one that tops up.
        w.write(&int_batch(0..250)).await.unwrap();
        w.write(&int_batch(250..260)).await.unwrap();
        let done = w.finish().await.unwrap();

        assert_eq!((done.rows, done.parts), (260, 3));
        assert_eq!(
            sink.paths(),
            vec![
                "t3/part-00000.parquet",
                "t3/part-00001.parquet",
                "t3/part-00002.parquet"
            ]
        );
        let mut next = 0i64;
        for (path, rows) in sink.paths().iter().zip([100usize, 100, 60]) {
            let bytes = sink.get(path).unwrap();
            let md = metadata_of(bytes.clone());
            assert_eq!(md.num_row_groups(), 1, "{path}");
            assert_eq!(md.file_metadata().num_rows() as usize, rows, "{path}");
            for b in read_all(bytes) {
                let ids = b.column(0).as_any().downcast_ref::<Int64Array>().unwrap();
                for v in ids.iter() {
                    assert_eq!(v, Some(next));
                    next += 1;
                }
            }
        }
        assert_eq!(next, 260);
    }

    #[tokio::test]
    async fn column_bytes_are_the_sum_over_parts() {
        let sink = Arc::new(MemorySink::default());
        let mut w = PartWriter::new(sink.clone(), 0, int_schema(), small(100)).unwrap();
        w.write(&int_batch(0..300)).await.unwrap();
        let done = w.finish().await.unwrap();
        let expect: i64 = sink
            .paths()
            .iter()
            .map(|p| {
                metadata_of(sink.get(p).unwrap())
                    .row_group(0)
                    .column(0)
                    .uncompressed_size()
            })
            .sum();
        assert_eq!(done.parts, 3);
        assert_eq!(done.columns[0].uncompressed_bytes as i64, expect);
        assert!(expect > 0);
    }

    #[tokio::test]
    async fn a_table_without_rows_still_writes_one_readable_part() {
        let sink = Arc::new(MemorySink::default());
        let mut w =
            PartWriter::new(sink.clone(), 0, int_schema(), WriterConfig::default()).unwrap();
        let done = w.finish().await.unwrap();
        assert_eq!((done.rows, done.parts), (0, 1));
        let md = metadata_of(sink.get("t0/part-00000.parquet").unwrap());
        assert_eq!(md.file_metadata().num_rows(), 0);
        assert_eq!(md.file_metadata().schema_descr().num_columns(), 1);
    }

    #[test]
    fn construction_refuses_unsupported_types_and_zero_limits() {
        let sink = Arc::new(MemorySink::default());
        let bad_type = Arc::new(Schema::new(vec![Field::new("x", DataType::Int32, true)]));
        assert!(matches!(
            PartWriter::new(sink.clone(), 0, bad_type, WriterConfig::default()),
            Err(WriterError::UnsupportedType(_))
        ));
        let empty = Arc::new(Schema::empty());
        assert!(PartWriter::new(sink.clone(), 0, empty, WriterConfig::default()).is_err());
        assert!(PartWriter::new(sink.clone(), 0, int_schema(), small(0)).is_err());
        let no_bytes = WriterConfig {
            max_rows: 10,
            max_bytes: 0,
        };
        assert!(PartWriter::new(sink.clone(), 0, int_schema(), no_bytes).is_err());
        assert!(PartWriter::new(sink, MAX_TABLES, int_schema(), WriterConfig::default()).is_err());
    }

    #[tokio::test]
    async fn parts_roll_at_max_bytes_and_the_buffer_stays_bounded() {
        let schema = Arc::new(Schema::new(vec![Field::new("s", DataType::Utf8, false)]));
        let max_bytes = 100 * 1024;
        let cfg = WriterConfig {
            max_rows: usize::MAX / 2,
            max_bytes,
        };
        let sink = Arc::new(MemorySink::default());
        let mut w = PartWriter::new(sink.clone(), 0, schema.clone(), cfg).unwrap();
        let mut batch_raw = 0usize;
        for k in 0..400 {
            let values: Vec<String> = (0..500)
                .map(|i| format!("row-{k}-{i}-{}", i * 7919))
                .collect();
            batch_raw = values.iter().map(|v| v.len() + 4).sum();
            let batch =
                RecordBatch::try_new(schema.clone(), vec![Arc::new(StringArray::from(values))])
                    .unwrap();
            w.write(&batch).await.unwrap();
            // After a write returns, the open part is below the limit: a part
            // that reached it was rolled out.
            assert!(w.buffered_bytes() < max_bytes, "buffer at batch {k}");
        }
        let done = w.finish().await.unwrap();
        assert_eq!(done.rows, 400 * 500);
        assert!(done.parts > 3, "only {} parts", done.parts);
        // A part is the buffer at the moment it rolled: below the limit plus
        // one batch, plus the footer.
        let footer = 8 * 1024;
        for path in sink.paths() {
            let len = sink.get(&path).unwrap().len();
            assert!(len <= max_bytes + batch_raw + footer, "{path}: {len} bytes");
        }
    }

    #[tokio::test]
    async fn a_sink_failure_poisons_the_writer_and_reports_what_was_attempted() {
        let sink = Arc::new(MemorySink::failing_from(1));
        let mut w = PartWriter::new(sink.clone(), 0, int_schema(), small(10)).unwrap();
        // Part 0 (10 rows) is put, part 1 is attempted and fails.
        let err = w.write(&int_batch(0..25)).await.unwrap_err();
        assert!(matches!(err, WriterError::Sink(_)), "{err:?}");
        assert_eq!(
            w.attempted_paths(),
            ["t0/part-00000.parquet", "t0/part-00001.parquet"]
        );
        // Nothing more is accepted and nothing is reported as complete.
        assert!(matches!(
            w.write(&int_batch(0..1)).await,
            Err(WriterError::Poisoned)
        ));
        assert!(matches!(w.finish().await, Err(WriterError::Poisoned)));
        assert_eq!(sink.paths(), vec!["t0/part-00000.parquet"]);
    }

    #[tokio::test]
    async fn a_batch_with_another_schema_is_refused() {
        let sink = Arc::new(MemorySink::default());
        let mut w = PartWriter::new(sink, 0, int_schema(), WriterConfig::default()).unwrap();
        let other = Arc::new(Schema::new(vec![Field::new(
            "other",
            DataType::Int64,
            false,
        )]));
        let batch = RecordBatch::try_new(other, vec![Arc::new(Int64Array::from_iter_values(0..3))])
            .unwrap();
        assert!(matches!(
            w.write(&batch).await,
            Err(WriterError::SchemaMismatch)
        ));
    }

    #[tokio::test]
    async fn the_part_index_is_bounded() {
        // Rolling past MAX_PARTS parts would need a sixth digit in the name.
        let sink = Arc::new(MemorySink::default());
        let mut w = PartWriter::new(sink, 0, int_schema(), small(1)).unwrap();
        w.parts_done = MAX_PARTS; // as if that many parts were already written
        let err = w.write(&int_batch(0..1)).await.unwrap_err();
        assert!(matches!(err, WriterError::Manifest(_)), "{err:?}");
    }

    #[tokio::test]
    async fn an_oversized_batch_is_written_in_slices_and_rolls_inside_it() {
        // One batch of 64 rows of 1 MiB (64 MiB), a part limit of 4 MiB: it must
        // become many parts, none holding more rows than a slice plus the limit.
        let schema = Arc::new(Schema::new(vec![Field::new("s", DataType::Utf8, false)]));
        // Pseudo-random text, which does not compress away.
        let mut x = 0x9E37_79B9_7F4A_7C15u64;
        let values: Vec<String> = (0..64)
            .map(|_| {
                (0..1024 * 1024)
                    .map(|_| {
                        x ^= x << 13;
                        x ^= x >> 7;
                        x ^= x << 17;
                        (b'a' + (x % 26) as u8) as char
                    })
                    .collect()
            })
            .collect();
        let batch = RecordBatch::try_new(schema.clone(), vec![Arc::new(StringArray::from(values))])
            .unwrap();
        let cfg = WriterConfig {
            max_rows: usize::MAX / 2,
            max_bytes: 4 * 1024 * 1024,
        };
        let sink = Arc::new(MemorySink::default());
        let mut w = PartWriter::new(sink.clone(), 0, schema, cfg).unwrap();
        w.write(&batch).await.unwrap();
        // Right after the write the open part is under the limit.
        assert!(w.buffered_bytes() < cfg.max_bytes);
        let done = w.finish().await.unwrap();
        assert_eq!(done.rows, 64);
        // Without slicing the whole batch would be one part.
        assert!(done.parts >= 8, "{} parts", done.parts);
        for path in sink.paths() {
            let md = metadata_of(sink.get(&path).unwrap());
            let rows = md.file_metadata().num_rows();
            // The slice is 8 rows of 1 MiB, the limit 4 MiB: at most 12 rows.
            assert!(rows <= 12, "{path} has {rows} rows");
        }
    }

    #[tokio::test]
    async fn the_memory_estimate_counts_values_offsets_and_validity_not_the_parquet_size() {
        // Strings that dictionary-encode to almost nothing still take memory.
        let schema = Arc::new(Schema::new(vec![
            Field::new("s", DataType::Utf8, true),
            Field::new("n", DataType::Int64, false),
        ]));
        let values: Vec<Option<String>> = (0..1000)
            .map(|i| (i % 10 != 0).then(|| format!("a fairly long repeated value {}", i % 3)))
            .collect();
        let value_bytes: usize = values.iter().flatten().map(|v| v.len()).sum();
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(StringArray::from(values)),
                Arc::new(Int64Array::from_iter_values(0..1000)),
            ],
        )
        .unwrap();
        let sink = Arc::new(MemorySink::default());
        let mut w = PartWriter::new(sink, 0, schema, small(400)).unwrap();
        w.write(&batch).await.unwrap();
        let done = w.finish().await.unwrap();
        let (s, n) = (&done.columns[0], &done.columns[1]);
        // Values plus 4 bytes of offset per row, plus validity: more than the
        // text alone, and not wildly more.
        let floor = (value_bytes + 4 * 1000) as u64;
        assert!(
            s.in_memory_bytes >= floor && s.in_memory_bytes <= floor * 2,
            "{}",
            s.in_memory_bytes
        );
        // The Parquet size is the dictionary, far below what is in memory.
        assert!(
            s.uncompressed_bytes * 3 < s.in_memory_bytes,
            "{} vs {}",
            s.uncompressed_bytes,
            s.in_memory_bytes
        );
        // Eight bytes per integer.
        assert_eq!(n.in_memory_bytes, 8 * 1000);
    }

    #[test]
    fn column_names_must_be_unique_non_empty_clean_and_short() {
        let sink = Arc::new(MemorySink::default());
        let mk = |names: &[&str]| {
            Arc::new(Schema::new(
                names
                    .iter()
                    .map(|n| Field::new(*n, DataType::Int64, true))
                    .collect::<Vec<_>>(),
            ))
        };
        for bad in [vec!["a", "a"], vec![""], vec!["a\u{7}"], vec!["a\nb"]] {
            assert!(
                PartWriter::new(sink.clone(), 0, mk(&bad), WriterConfig::default()).is_err(),
                "{bad:?}"
            );
        }
        let long = "x".repeat(MAX_COLUMN_NAME_CHARS + 1);
        assert!(PartWriter::new(sink.clone(), 0, mk(&[&long]), WriterConfig::default()).is_err());
        assert!(PartWriter::new(sink, 0, mk(&["a", "A", "é"]), WriterConfig::default()).is_ok());
    }

    #[tokio::test]
    async fn a_failing_final_put_keeps_its_path_and_no_table_is_reported() {
        // One part is written (put 0), then the last part's put fails.
        let sink = Arc::new(MemorySink::failing_from(1));
        let mut w = PartWriter::new(sink.clone(), 0, int_schema(), small(10)).unwrap();
        w.write(&int_batch(0..15)).await.unwrap();
        let err = w.finish().await.unwrap_err();
        assert!(matches!(err, WriterError::Sink(_)), "{err:?}");
        // The path of the put that failed is known, so the caller can remove it.
        assert_eq!(
            w.attempted_paths(),
            ["t0/part-00000.parquet", "t0/part-00001.parquet"]
        );
        assert!(matches!(w.finish().await, Err(WriterError::Poisoned)));
    }

    #[tokio::test]
    async fn encoding_does_not_block_the_async_worker() {
        // A single-threaded runtime: a probe task yields in a loop while the
        // writer encodes 8 MiB of text. If the encoder ran on the runtime
        // thread, the write would finish in its first poll and the probe would
        // never run; with the blocking pool the write is pending while the
        // thread encodes, and the probe runs. Nothing depends on how long that
        // takes.
        let schema = Arc::new(Schema::new(vec![Field::new("s", DataType::Utf8, false)]));
        let mut x = 0x2545_F491_4F6C_DD1Du64;
        let values: Vec<String> = (0..8)
            .map(|_| {
                (0..1024 * 1024)
                    .map(|_| {
                        x ^= x << 13;
                        x ^= x >> 7;
                        x ^= x << 17;
                        (b'a' + (x % 26) as u8) as char
                    })
                    .collect()
            })
            .collect();
        let batch = RecordBatch::try_new(schema.clone(), vec![Arc::new(StringArray::from(values))])
            .unwrap();
        let sink = Arc::new(MemorySink::default());
        let mut w = PartWriter::new(sink, 0, schema, WriterConfig::default()).unwrap();
        let ticks = std::sync::atomic::AtomicUsize::new(0);
        let done = std::sync::atomic::AtomicBool::new(false);
        let probe = async {
            while !done.load(std::sync::atomic::Ordering::SeqCst) {
                ticks.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                tokio::task::yield_now().await;
            }
        };
        let mut during_write = 0;
        let work = async {
            w.write(&batch).await.unwrap();
            during_write = ticks.load(std::sync::atomic::Ordering::SeqCst);
            w.finish().await.unwrap();
            done.store(true, std::sync::atomic::Ordering::SeqCst);
        };
        tokio::join!(work, probe);
        // `join!` polls `work` first. Encoding 8 MiB takes milliseconds, so when
        // it runs on the blocking pool the write is pending at that first poll
        // and the probe runs before the write is polled again; when it runs
        // inline the write finishes in its first poll and the probe has not run.
        assert!(
            during_write >= 1,
            "another task ran {during_write} times while the slices were encoded"
        );
        // The flush of the part is not asserted this way: the pages were already
        // compressed while the slices were written, so closing the part can end
        // before the next poll and a tick count between the two would depend on
        // the machine. `roll` hands the close to `spawn_blocking` the same way.
    }

    #[tokio::test]
    async fn a_panic_or_cancel_of_the_encoder_task_is_a_typed_error() {
        let panicked = tokio::task::spawn_blocking(|| panic!("encoder bug"))
            .await
            .unwrap_err();
        assert!(
            matches!(encoder_stopped(panicked), WriterError::Parquet(m) if m.contains("panicked"))
        );
        let handle = tokio::spawn(std::future::pending::<()>());
        handle.abort();
        let cancelled = handle.await.unwrap_err();
        assert!(
            matches!(encoder_stopped(cancelled), WriterError::Parquet(m) if m.contains("cancelled"))
        );
    }
}
