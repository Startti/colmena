use crate::storage::domain::StorageError;
use crate::tabular_prepare::csv::{CsvError, RawBatches};
use crate::tabular_prepare::infer::{cell_fits, InferredSchema};
use crate::tabular_prepare::manifest::{ColumnType, ManifestError};
use arrow_array::{Array, ArrayRef, RecordBatch, StringArray};
use arrow_cast::cast;
use arrow_schema::{DataType, SchemaRef};
use bytes::Bytes;
use futures::{Stream, StreamExt};
use std::io::{self, Read};
use std::pin::Pin;
use thiserror::Error;
use tokio_util::sync::CancellationToken;

/// A cell that does not fit the type of its column. `row` counts data rows
/// from zero (the header is not a row).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeConflict {
    pub column: usize,
    pub row: u64,
}

#[derive(Debug, Error)]
pub enum ConvertError {
    #[error(transparent)]
    Csv(#[from] CsvError),
    #[error(transparent)]
    Manifest(#[from] ManifestError),
    #[error("column {} does not fit its type at data row {}", .0.column, .0.row + 1)]
    Conflict(TypeConflict),
    #[error("could not convert a column: {0}")]
    Cast(String),
    /// The blocking half of the conversion panicked. Never silent and never a
    /// short table: the run fails with this.
    #[error("the file reader panicked")]
    ReaderPanicked,
}

/// The batches of a CSV with each column in the type of `schema`. The first
/// error ends the iteration.
pub struct TypedBatches {
    raw: RawBatches,
    types: Vec<ColumnType>,
    arrow: SchemaRef,
    rows_done: u64,
    done: bool,
}

impl TypedBatches {
    /// `schema` is the effective schema: the inferred one, with any column the
    /// caller demoted already set to text.
    pub fn new(raw: RawBatches, schema: InferredSchema) -> Self {
        Self {
            raw,
            types: schema.columns.iter().map(|c| c.column_type).collect(),
            arrow: schema.arrow_schema(),
            rows_done: 0,
            done: false,
        }
    }

    fn type_batch(&self, raw: &RecordBatch) -> Result<RecordBatch, ConvertError> {
        let mut first: Option<TypeConflict> = None;
        let mut columns: Vec<ArrayRef> = Vec::with_capacity(self.types.len());
        for (c, (t, field)) in self.types.iter().zip(self.arrow.fields()).enumerate() {
            let col = raw.column(c);
            if *t == ColumnType::String {
                columns.push(col.clone());
                continue;
            }
            let text = col
                .as_any()
                .downcast_ref::<StringArray>()
                .ok_or_else(|| ConvertError::Cast("a column is not text".into()))?;
            match type_column(text, *t, field.data_type()) {
                Ok(typed) => columns.push(typed),
                Err(i) => {
                    let row = self.rows_done + i as u64;
                    if first.as_ref().is_none_or(|f| row < f.row) {
                        first = Some(TypeConflict { column: c, row });
                    }
                }
            }
        }
        if let Some(conflict) = first {
            return Err(ConvertError::Conflict(conflict));
        }
        RecordBatch::try_new(self.arrow.clone(), columns)
            .map_err(|e| ConvertError::Cast(e.to_string()))
    }
}

/// One text column in its type, or the index of the first cell that does not
/// fit.
///
/// A cell fits when the rules that chose the type say so, and the stored value
/// is then checked against the cell: every non-empty cell must come out as a
/// non-null value (Arrow's cast turns what it cannot parse into null), and a
/// float must be exactly the value of its literal. A disagreement between the
/// inference and the cast is a conflict, never a changed value.
fn type_column(text: &StringArray, t: ColumnType, dt: &DataType) -> Result<ArrayRef, usize> {
    if let Some(i) = (0..text.len()).find(|&i| !text.is_null(i) && !cell_fits(text.value(i), t)) {
        return Err(i);
    }
    let typed = cast(text, dt).map_err(|_| 0usize)?;
    if typed.null_count() != text.null_count() {
        return Err((0..text.len())
            .find(|&i| !text.is_null(i) && typed.is_null(i))
            .unwrap_or(0));
    }
    if t == ColumnType::Float {
        let values = typed
            .as_any()
            .downcast_ref::<arrow_array::Float64Array>()
            .ok_or(0usize)?;
        let bad = (0..text.len()).find(|&i| {
            !text.is_null(i)
                && text
                    .value(i)
                    .parse::<f64>()
                    .map_or(true, |v| v.to_bits() != values.value(i).to_bits())
        });
        if let Some(i) = bad {
            return Err(i);
        }
    }
    Ok(typed)
}

impl Iterator for TypedBatches {
    type Item = Result<RecordBatch, ConvertError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        let raw = match self.raw.next()? {
            Ok(b) => b,
            Err(e) => {
                self.done = true;
                return Some(Err(e.into()));
            }
        };
        match self.type_batch(&raw) {
            Ok(b) => {
                self.rows_done += b.num_rows() as u64;
                Some(Ok(b))
            }
            Err(e) => {
                self.done = true;
                Some(Err(e))
            }
        }
    }
}

/// The stream of an object in storage.
pub type ByteStream = Pin<Box<dyn Stream<Item = Result<Bytes, StorageError>> + Send>>;

/// Reads a storage stream as a blocking source, for [`open_csv`]. It must be
/// built inside the runtime and read on a blocking thread (`spawn_blocking`).
/// A storage error becomes an `io::Error` with its text.
///
/// A read that is waiting for the next chunk of a stalled stream is woken by
/// `cancel` and fails with [`CsvError::Cancelled`], so the blocking thread does
/// not outlive a dropped or cancelled conversion. What cannot be interrupted is
/// a read stuck inside the stream's own non-async code; none of this module's
/// readers does that.
///
/// [`open_csv`]: crate::tabular_prepare::csv::open_csv
pub fn stream_reader(stream: ByteStream, cancel: CancellationToken) -> impl Read + Send {
    StreamBridge {
        handle: tokio::runtime::Handle::current(),
        stream,
        chunk: Bytes::new(),
        cancel,
        ended: false,
    }
}

struct StreamBridge {
    handle: tokio::runtime::Handle,
    stream: ByteStream,
    chunk: Bytes,
    cancel: CancellationToken,
    ended: bool,
}

impl Read for StreamBridge {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        while self.chunk.is_empty() {
            if self.ended {
                return Ok(0);
            }
            let (stream, cancel) = (&mut self.stream, &self.cancel);
            let next = self.handle.block_on(async {
                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => Err(CsvError::Cancelled),
                    item = stream.next() => Ok(item),
                }
            });
            match next {
                Err(e) => return Err(e.into_io()),
                Ok(None) => self.ended = true,
                Ok(Some(Ok(chunk))) => self.chunk = chunk,
                Ok(Some(Err(e))) => return Err(io::Error::other(e.to_string())),
            }
        }
        let n = buf.len().min(self.chunk.len());
        buf[..n].copy_from_slice(&self.chunk[..n]);
        self.chunk = self.chunk.slice(n..);
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tabular_prepare::csv::{open_csv, BATCH_ROWS};
    use crate::tabular_prepare::manifest::ColumnType;
    use arrow_array::{
        Array, BooleanArray, Date32Array, Float64Array, Int64Array, StringArray,
        TimestampMicrosecondArray,
    };
    use bytes::Bytes;
    use futures::stream;
    use std::io::{Cursor, Read};
    use tokio_util::sync::CancellationToken;

    fn typed(csv: &str) -> (InferredSchema, TypedBatches) {
        let o = open_csv(Cursor::new(csv.as_bytes().to_vec()), None).unwrap();
        let schema = o.schema.clone();
        (schema.clone(), TypedBatches::new(o.batches, schema))
    }

    #[test]
    fn cells_become_typed_values_with_empty_as_null() {
        let csv = "i,f,b,s,d,t\n\
                   7,1.5,TRUE,00123,2020-02-29,2020-01-05T10:20:30\n\
                   -8,2,false,x,1999-12-31,2020-01-05 10:20:30.123456\n\
                   ,,,,,\n";
        let (_, mut it) = typed(csv);
        let b = it.next().unwrap().unwrap();
        assert!(it.next().is_none());
        let col = |i: usize| b.column(i).clone();
        let ints = col(0);
        let ints = ints.as_any().downcast_ref::<Int64Array>().unwrap();
        assert_eq!(
            (ints.value(0), ints.value(1), ints.is_null(2)),
            (7, -8, true)
        );
        let f = col(1);
        let f = f.as_any().downcast_ref::<Float64Array>().unwrap();
        assert_eq!((f.value(0), f.value(1)), (1.5, 2.0));
        let bo = col(2);
        let bo = bo.as_any().downcast_ref::<BooleanArray>().unwrap();
        assert_eq!(
            (bo.value(0), bo.value(1), bo.is_null(2)),
            (true, false, true)
        );
        let s = col(3);
        let s = s.as_any().downcast_ref::<StringArray>().unwrap();
        assert_eq!((s.value(0), s.value(1), s.is_null(2)), ("00123", "x", true));
        let d = col(4);
        let d = d.as_any().downcast_ref::<Date32Array>().unwrap();
        assert_eq!(
            (d.value(0), d.value(1), d.is_null(2)),
            (18_321, 10_956, true)
        );
        let t = col(5);
        let t = t
            .as_any()
            .downcast_ref::<TimestampMicrosecondArray>()
            .unwrap();
        assert_eq!(t.value(0), 1_578_219_630_000_000);
        assert_eq!(t.value(1), 1_578_219_630_123_456);
        assert!(t.is_null(2));
    }

    #[test]
    fn the_output_schema_is_the_effective_schema() {
        let (schema, mut it) = typed("a,b\n1,x\n");
        let b = it.next().unwrap().unwrap();
        assert_eq!(b.schema(), schema.arrow_schema());
    }

    /// An int column in the sample, then `late` far past it.
    fn late_value(late: &str) -> String {
        let mut csv = String::from("id,v\n");
        for i in 0..crate::tabular_prepare::infer::INFERENCE_ROWS + 20_000 {
            csv.push_str(&format!("{i},{i}\n"));
        }
        csv.push_str(&format!("1,{late}\n"));
        csv
    }

    #[test]
    fn a_late_value_that_does_not_fit_is_a_conflict_with_its_column_and_row() {
        let csv = late_value("N/A");
        let rows_before = crate::tabular_prepare::infer::INFERENCE_ROWS + 20_000;
        let (schema, it) = typed(&csv);
        assert_eq!(schema.columns[1].column_type, ColumnType::Int);
        let mut rows = 0;
        let mut conflict = None;
        for item in it {
            match item {
                Ok(b) => rows += b.num_rows(),
                Err(e) => conflict = Some(e),
            }
        }
        // Whole batches before the offending one arrive typed.
        assert!(
            rows > 0 && rows <= rows_before && rows % BATCH_ROWS == 0,
            "{rows}"
        );
        match conflict.unwrap() {
            ConvertError::Conflict(c) => {
                assert_eq!(
                    c,
                    TypeConflict {
                        column: 1,
                        row: rows_before as u64
                    }
                );
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn with_two_conflicts_in_one_batch_the_earlier_row_is_reported() {
        let mut csv = String::from("a,b\n");
        for i in 0..crate::tabular_prepare::infer::INFERENCE_ROWS {
            csv.push_str(&format!("{i},{i}\n"));
        }
        // Column a goes wrong at the second late row, column b at the first.
        csv.push_str("1,N/A\nN/A,2\n");
        let (_, it) = typed(&csv);
        let conflict = it.filter_map(Result::err).next().unwrap();
        match conflict {
            ConvertError::Conflict(c) => assert_eq!(
                c,
                TypeConflict {
                    column: 1,
                    row: crate::tabular_prepare::infer::INFERENCE_ROWS as u64
                }
            ),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn values_a_number_parser_would_accept_are_still_conflicts() {
        // Each of these parses as a number or a bool somewhere, and none is
        // what the inference accepted: reading them would change the value.
        for late in ["007", "+5", "1e3", "1.5"] {
            let (_, it) = typed(&late_value(late));
            let err = it.filter_map(Result::err).next();
            assert!(
                matches!(err, Some(ConvertError::Conflict(_))),
                "{late}: {err:?}"
            );
        }
    }

    #[test]
    fn a_column_forced_to_text_keeps_every_value_verbatim() {
        let csv = late_value("N/A");
        let o = open_csv(Cursor::new(csv.into_bytes()), None).unwrap();
        let mut schema = o.schema.clone();
        schema.columns[1].column_type = ColumnType::String;
        let mut seen = 0usize;
        for b in TypedBatches::new(o.batches, schema) {
            let b = b.unwrap();
            let v = b.column(1).as_any().downcast_ref::<StringArray>().unwrap();
            for r in 0..b.num_rows() {
                // Every value is the text of its row, verbatim; the late one is "N/A".
                let want = if seen + r == crate::tabular_prepare::infer::INFERENCE_ROWS + 20_000 {
                    "N/A".to_string()
                } else {
                    (seen + r).to_string()
                };
                assert_eq!(v.value(r), want, "row {}", seen + r);
            }
            seen += b.num_rows();
        }
        assert_eq!(seen, crate::tabular_prepare::infer::INFERENCE_ROWS + 20_001);
    }

    #[test]
    fn a_failure_of_the_source_comes_out_typed_and_ends_the_iteration() {
        let mut csv = String::from("a,b\n");
        for _ in 0..crate::tabular_prepare::infer::INFERENCE_ROWS + 100 {
            csv.push_str("1,2\n");
        }
        csv.push_str(&"x".repeat(crate::tabular_prepare::scan::MAX_RECORD_BYTES + 10));
        let (_, it) = typed(&csv);
        let items: Vec<_> = it.take(1000).collect();
        assert!(matches!(
            items.last().unwrap(),
            Err(ConvertError::Csv(
                crate::tabular_prepare::csv::CsvError::RecordTooLong { .. }
            ))
        ));
        assert_eq!(items.iter().filter(|i| i.is_err()).count(), 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_storage_stream_reads_as_a_blocking_reader_in_order() {
        let chunks: Vec<Result<Bytes, StorageError>> = (0..1000)
            .map(|i| Ok(Bytes::from(format!("{i},row\n"))))
            .collect();
        let expected: String = (0..1000).map(|i| format!("{i},row\n")).collect();
        let reader = stream_reader(Box::pin(stream::iter(chunks)), CancellationToken::new());
        let text = tokio::task::spawn_blocking(move || {
            let mut out = String::new();
            let mut r = reader;
            r.read_to_string(&mut out).map(|_| out)
        })
        .await
        .unwrap()
        .unwrap();
        assert_eq!(text, expected);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_storage_error_in_the_stream_is_an_io_error_with_its_text() {
        let chunks: Vec<Result<Bytes, StorageError>> = vec![
            Ok(Bytes::from_static(b"a,b\n1,2\n")),
            Err(StorageError::BackendUnavailable("connection reset".into())),
        ];
        let reader = stream_reader(Box::pin(stream::iter(chunks)), CancellationToken::new());
        let err = tokio::task::spawn_blocking(move || {
            let mut r = reader;
            r.read_to_end(&mut Vec::new()).unwrap_err()
        })
        .await
        .unwrap();
        assert!(err.to_string().contains("connection reset"), "{err}");
    }

    // ---- the restart loop ----

    // ---- cancellation ----
}
