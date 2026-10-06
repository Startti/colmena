//! Reading a CSV for preparation (dark behind `COLMENA_LARGE_TABULAR`).
//!
//! [`prepare_input`] turns a blocking byte source into clean UTF-8 text and
//! finds the delimiter, holding no more than one sample ([`SNIFF_BYTES`]) in
//! memory and never reading the rest ahead of the consumer: the file can be
//! far larger than memory.

use std::io::{self};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CsvError {
    #[error("the file is empty")]
    Empty,
    #[error("unsupported encoding: {0}")]
    UnsupportedEncoding(String),
    #[error("record {record} is longer than {limit} bytes")]
    RecordTooLong { record: u64, limit: usize },
    #[error("the file has more than {limit} columns")]
    TooManyColumns { limit: usize },
    #[error("malformed CSV: {0}")]
    Parse(String),
    #[error("the conversion was cancelled")]
    Cancelled,
    #[error("read error: {0}")]
    Io(String),
    #[error("invalid reader limits: {0}")]
    InvalidLimits(String),
}

impl CsvError {
    /// The typed error inside an `io::Error` produced by this module's
    /// readers, or `Io` for any other.
    pub fn from_io(e: io::Error) -> Self {
        Self::typed(&e)
    }

    fn typed(e: &io::Error) -> Self {
        match e.get_ref().and_then(|i| i.downcast_ref::<CsvError>()) {
            Some(inner) => inner.clone(),
            None => CsvError::Io(e.to_string()),
        }
    }

    pub(crate) fn into_io(self) -> io::Error {
        io::Error::new(io::ErrorKind::InvalidData, self)
    }
}
