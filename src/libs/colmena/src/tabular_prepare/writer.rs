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
