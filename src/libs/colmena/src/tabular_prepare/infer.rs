//! Conservative type inference for CSV columns (dark behind
//! `COLMENA_LARGE_TABULAR`).
//!
//! The type of a column is decided from its first [`INFERENCE_ROWS`] rows. The
//! rules lean toward text: a wrong number type loses data (a zip code `00501`
//! read as `501`), a wrong text type loses nothing. A later cell that does not
//! fit the decided type is not this module's concern: the converter demotes
//! that column to text and restarts.

use crate::tabular_prepare::manifest::ColumnType;
use arrow_schema::{DataType, Field, Schema, SchemaRef, TimeUnit};
use chrono::NaiveDate;
use std::sync::Arc;

/// Rows examined to decide the types. Later rows are never looked at.
pub const INFERENCE_ROWS: usize = 10_000;

/// Digits an integer or a float mantissa may have: what an `f64` holds exactly.
const MAX_DIGITS: usize = 15;

const INT: u8 = 1;
const FLOAT: u8 = 2;
const BOOL: u8 = 4;
const DATE: u8 = 8;
const TIMESTAMP: u8 = 16;
const STRING: u8 = 32;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InferredColumn {
    pub name: String,
    pub column_type: ColumnType,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InferredSchema {
    pub columns: Vec<InferredColumn>,
}

impl InferredSchema {
    /// Arrow schema for the CSV reader and the Parquet writer. Every column is
    /// nullable: an empty cell is null.
    pub fn arrow_schema(&self) -> SchemaRef {
        let fields: Vec<Field> = self
            .columns
            .iter()
            .map(|c| Field::new(&c.name, arrow_type_of(c.column_type), true))
            .collect();
        Arc::new(Schema::new(fields))
    }
}

pub fn arrow_type_of(t: ColumnType) -> DataType {
    match t {
        ColumnType::Int => DataType::Int64,
        ColumnType::Float => DataType::Float64,
        ColumnType::Bool => DataType::Boolean,
        ColumnType::String => DataType::Utf8,
        ColumnType::Date => DataType::Date32,
        ColumnType::Timestamp => DataType::Timestamp(TimeUnit::Microsecond, None),
    }
}

/// Collects the first rows of a CSV and decides the column types.
pub struct SchemaInferer {
    window: usize,
    names: Vec<String>,
    seen: Vec<u8>,
    rows: usize,
}

impl SchemaInferer {
    /// `names` are the column names, already cleaned and unique.
    pub fn new(names: Vec<String>) -> Self {
        Self::with_window(names, INFERENCE_ROWS)
    }

    /// Like [`new`](Self::new) with the types decided from the first `window`
    /// rows instead of [`INFERENCE_ROWS`] (tests move the window to put a
    /// boundary where they want it).
    pub fn with_window(names: Vec<String>, window: usize) -> Self {
        Self {
            window,
            seen: vec![0; names.len()],
            names,
            rows: 0,
        }
    }

    pub fn rows_seen(&self) -> usize {
        self.rows
    }

    /// Looks at one row. A missing cell is null and an extra cell is ignored.
    /// Returns whether more rows are wanted: `false` once the window is full,
    /// after which rows are not examined at all.
    pub fn observe(&mut self, row: &[&str]) -> bool {
        if self.rows >= self.window {
            return false;
        }
        for (i, seen) in self.seen.iter_mut().enumerate() {
            if *seen & STRING == 0 {
                *seen |= classify(row.get(i).copied().unwrap_or(""));
            }
        }
        self.rows += 1;
        self.rows < self.window
    }

    pub fn finish(self) -> InferredSchema {
        let columns = self
            .names
            .into_iter()
            .zip(self.seen)
            .map(|(name, seen)| InferredColumn {
                name,
                column_type: resolve(seen),
            })
            .collect();
        InferredSchema { columns }
    }
}

fn resolve(seen: u8) -> ColumnType {
    match seen {
        INT => ColumnType::Int,
        FLOAT | 3 => ColumnType::Float,
        BOOL => ColumnType::Bool,
        DATE => ColumnType::Date,
        TIMESTAMP => ColumnType::Timestamp,
        // Nothing seen, text seen, or kinds that do not mix.
        _ => ColumnType::String,
    }
}

/// The kind of one cell as a bit (0 for an empty cell, which is null).
fn classify(cell: &str) -> u8 {
    if cell.is_empty() {
        return 0;
    }
    if cell.eq_ignore_ascii_case("true") || cell.eq_ignore_ascii_case("false") {
        return BOOL;
    }
    if is_int(cell) {
        return INT;
    }
    if is_float(cell) {
        return FLOAT;
    }
    if is_date(cell) {
        return DATE;
    }
    if is_timestamp(cell) {
        return TIMESTAMP;
    }
    STRING
}

fn all_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// `-?` digits, at most [`MAX_DIGITS`], no leading zero (`0` alone is fine).
fn is_int(s: &str) -> bool {
    let digits = s.strip_prefix('-').unwrap_or(s);
    all_digits(digits)
        && digits.len() <= MAX_DIGITS
        && !(digits.len() > 1 && digits.starts_with('0'))
}

/// `-?int.frac` or `-?int[.frac]e[+-]?digits`, with the integer-part and
/// mantissa rules of [`is_int`] and a finite value.
fn is_float(s: &str) -> bool {
    let s = s.strip_prefix('-').unwrap_or(s);
    let (mantissa, exp) = match s.split_once(['e', 'E']) {
        Some((m, e)) => (m, Some(e)),
        None => (s, None),
    };
    let (int, frac) = match mantissa.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (mantissa, None),
    };
    if frac.is_none() && exp.is_none() {
        return false;
    }
    let exp_ok = exp.is_none_or(|e| all_digits(e.strip_prefix(['+', '-']).unwrap_or(e)));
    let frac_ok = frac.is_none_or(all_digits);
    let digits = int.len() + frac.map_or(0, str::len);
    all_digits(int)
        && !(int.len() > 1 && int.starts_with('0'))
        && exp_ok
        && frac_ok
        && digits <= MAX_DIGITS
        && s.parse::<f64>().is_ok_and(|v| {
            // Finite, and not a literal that underflows: a non-zero literal must
            // not become zero or a subnormal, whose precision is gone.
            let all_zero = mantissa.bytes().all(|b| b == b'0' || b == b'.');
            v.is_finite() && if all_zero { v == 0.0 } else { v.is_normal() }
        })
}

/// Whether a cell may be stored in a column of type `t`: by the rules that
/// chose the types, so a value is read as a number only if the inference
/// would have called it one. An empty cell is null and fits everything.
pub fn cell_fits(cell: &str, t: ColumnType) -> bool {
    let kind = classify(cell);
    kind == 0
        || match t {
            ColumnType::String => true,
            ColumnType::Int => kind == INT,
            ColumnType::Float => kind == INT || kind == FLOAT,
            ColumnType::Bool => kind == BOOL,
            ColumnType::Date => kind == DATE,
            ColumnType::Timestamp => kind == TIMESTAMP,
        }
}

/// Exactly `YYYY-MM-DD`, a real calendar day, year 1000 to 9999.
fn is_date(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return false;
    }
    let (y, m, d) = (&s[0..4], &s[5..7], &s[8..10]);
    if !(all_digits(y) && all_digits(m) && all_digits(d)) {
        return false;
    }
    let (y, m, d): (i32, u32, u32) = (y.parse().unwrap(), m.parse().unwrap(), d.parse().unwrap());
    y >= 1000 && NaiveDate::from_ymd_opt(y, m, d).is_some()
}

/// A date, `T` or a space, `HH:MM:SS` and up to six fraction digits. No zone:
/// a cell with `Z` or an offset stays text rather than be shifted silently.
fn is_timestamp(s: &str) -> bool {
    let (date, time) = match s.get(..10).zip(s.get(11..)) {
        Some(parts) if matches!(s.as_bytes()[10], b'T' | b' ') => parts,
        _ => return false,
    };
    let (clock, frac) = match time.split_once('.') {
        Some((c, f)) => (c, Some(f)),
        None => (time, None),
    };
    let b = clock.as_bytes();
    if !is_date(date)
        || b.len() != 8
        || b[2] != b':'
        || b[5] != b':'
        || !frac.is_none_or(|f| all_digits(f) && f.len() <= 6)
    {
        return false;
    }
    // Digits only: `u32::parse` would accept a plus sign.
    let part = |r: std::ops::Range<usize>| {
        let p = &clock[r];
        all_digits(p).then(|| p.parse::<u32>().ok()).flatten()
    };
    match (part(0..2), part(3..5), part(6..8)) {
        (Some(h), Some(m), Some(sec)) => h < 24 && m < 60 && sec < 60,
        _ => false,
    }
}
