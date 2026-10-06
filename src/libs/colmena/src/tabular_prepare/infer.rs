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

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::{Array, Float64Array, Int64Array, StringArray};
    use arrow_csv::ReaderBuilder;
    use arrow_schema::DataType;

    fn names(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("c{i}")).collect()
    }

    /// Infers one column from its cells.
    fn infer_col(cells: &[&str]) -> ColumnType {
        let mut inf = SchemaInferer::new(names(1));
        for c in cells {
            inf.observe(&[*c]);
        }
        inf.finish().columns[0].column_type
    }

    #[test]
    fn plain_types_are_recognised() {
        assert_eq!(infer_col(&["1", "-20", "0", "300"]), ColumnType::Int);
        assert_eq!(infer_col(&["1.5", "-0.25", "3.0"]), ColumnType::Float);
        assert_eq!(infer_col(&["1e5", "1.5E-3", "-2e+4"]), ColumnType::Float);
        assert_eq!(infer_col(&["true", "FALSE", "True"]), ColumnType::Bool);
        assert_eq!(infer_col(&["2020-02-29", "1999-12-31"]), ColumnType::Date);
        assert_eq!(
            infer_col(&["2020-01-05T10:20:30", "2020-01-05 10:20:30.123456"]),
            ColumnType::Timestamp
        );
        assert_eq!(infer_col(&["abc", "def"]), ColumnType::String);
    }

    #[test]
    fn leading_zero_integers_stay_strings() {
        for cells in [
            &["00123", "01234", "00501"][..],
            &["0123", "5"][..],
            &["-0123"][..],
            &["00"][..],
        ] {
            assert_eq!(infer_col(cells), ColumnType::String, "{cells:?}");
        }
        // A lone zero and a negative zero are numbers.
        assert_eq!(infer_col(&["0", "-0"]), ColumnType::Int);
    }

    #[test]
    fn more_than_fifteen_digits_is_a_string() {
        assert_eq!(infer_col(&["123456789012345"]), ColumnType::Int);
        assert_eq!(infer_col(&["1234567890123456"]), ColumnType::String);
        assert_eq!(infer_col(&["-1234567890123456"]), ColumnType::String);
        // The same limit protects floats from losing digits.
        assert_eq!(infer_col(&["1234567.12345678"]), ColumnType::Float);
        assert_eq!(infer_col(&["12345678.123456789"]), ColumnType::String);
    }

    #[test]
    fn empty_cells_are_null_and_do_not_decide_the_type() {
        assert_eq!(infer_col(&["1", "", "3"]), ColumnType::Int);
        assert_eq!(infer_col(&["", "2020-01-01", ""]), ColumnType::Date);
        // Nothing to go on: the safe type.
        assert_eq!(infer_col(&["", ""]), ColumnType::String);
        assert_eq!(infer_col(&[]), ColumnType::String);
    }

    #[test]
    fn mixed_cells_fall_back_to_string() {
        assert_eq!(infer_col(&["1", "two", "3"]), ColumnType::String);
        assert_eq!(infer_col(&["1", "true"]), ColumnType::String);
        assert_eq!(
            infer_col(&["2020-01-01", "2020-01-01 10:00:00"]),
            ColumnType::String
        );
        assert_eq!(infer_col(&["2020-01-01", "5"]), ColumnType::String);
        // Integers and floats mix into floats.
        assert_eq!(infer_col(&["1", "2.5"]), ColumnType::Float);
    }

    #[test]
    fn near_misses_are_strings_not_numbers() {
        for cell in [
            "+5",
            " 5",
            "5 ",
            "1_000",
            "1e999",
            "1e-400",
            "4.9e-325",
            "2e-310",
            "2020-01-05 +9:00:00",
            "2020-01-05T09:+0:00",
            "1,5",
            "1.",
            ".5",
            "NaN",
            "inf",
            "-inf",
            "0x10",
            "1e",
            "--1",
            "1.2.3",
            "TRUE1",
            "yes",
            "2021-02-30",
            "2021-13-01",
            "2021-02-29",
            "2020-1-5",
            "2020-01-05T10:20:30Z",
            "2020-01-05T10:20:30+02:00",
            "2020-01-05T25:00:00",
            "٣",
        ] {
            assert_eq!(infer_col(&[cell]), ColumnType::String, "{cell:?}");
        }
    }

    #[test]
    fn only_the_first_ten_thousand_rows_decide() {
        let mut inf = SchemaInferer::new(names(1));
        for i in 0..INFERENCE_ROWS {
            let cell = i.to_string();
            let wants_more = inf.observe(&[cell.as_str()]);
            assert_eq!(wants_more, i + 1 < INFERENCE_ROWS, "row {i}");
        }
        // Later rows are not examined, however odd.
        assert!(!inf.observe(&["N/A"]));
        assert_eq!(inf.rows_seen(), INFERENCE_ROWS);
        assert_eq!(inf.finish().columns[0].column_type, ColumnType::Int);

        // The same value inside the window decides.
        let mut inf = SchemaInferer::new(names(1));
        for _ in 0..INFERENCE_ROWS - 1 {
            inf.observe(&["7"]);
        }
        inf.observe(&["N/A"]);
        assert_eq!(inf.finish().columns[0].column_type, ColumnType::String);
    }

    #[test]
    fn short_rows_count_as_null_and_extra_cells_are_ignored() {
        let mut inf = SchemaInferer::new(names(2));
        inf.observe(&["1"]);
        inf.observe(&["2", "5", "ignored"]);
        let s = inf.finish();
        assert_eq!(s.columns[0].column_type, ColumnType::Int);
        // Null from the short row, then an integer.
        assert_eq!(s.columns[1].column_type, ColumnType::Int);
        assert_eq!(s.columns.len(), 2);
    }

    #[test]
    fn the_arrow_schema_has_the_type_of_each_column() {
        let mut inf = SchemaInferer::new(names(6));
        inf.observe(&["1", "1.5", "true", "x", "2020-01-01", "2020-01-01T00:00:00"]);
        let schema = inf.finish().arrow_schema();
        let types: Vec<_> = schema
            .fields()
            .iter()
            .map(|f| f.data_type().clone())
            .collect();
        assert_eq!(types[0], DataType::Int64);
        assert_eq!(types[1], DataType::Float64);
        assert_eq!(types[2], DataType::Boolean);
        assert_eq!(types[3], DataType::Utf8);
        assert_eq!(types[4], DataType::Date32);
        assert!(matches!(types[5], DataType::Timestamp(_, None)));
        assert!(schema.fields().iter().all(|f| f.is_nullable()));
    }

    /// What the inference calls a type must be something the CSV reader that
    /// uses the schema can actually parse, with the value intact.
    #[test]
    fn the_csv_reader_parses_what_the_inference_accepts() {
        let csv = "id,code,price,sci,day,at,flag\n\
                   7,00123,1.5,1.5E-3,2020-02-29,2020-01-05T10:20:30,TRUE\n\
                   -8,01234,-0.25,1e5,1999-12-31,2020-01-05 10:20:30.123456,false\n\
                   ,,,,,,\n";
        let mut lines = csv.lines();
        let header: Vec<String> = lines.next().unwrap().split(',').map(String::from).collect();
        let rows: Vec<Vec<&str>> = lines.map(|l| l.split(',').collect()).collect();
        let mut inf = SchemaInferer::new(header);
        for r in &rows {
            inf.observe(r);
        }
        let schema = inf.finish().arrow_schema();
        let mut reader = ReaderBuilder::new(schema)
            .with_header(true)
            .build(std::io::Cursor::new(csv))
            .unwrap();
        let batch = reader.next().unwrap().unwrap();
        assert_eq!(batch.num_rows(), 3);
        let ids = batch
            .column(0)
            .as_any()
            .downcast_ref::<Int64Array>()
            .unwrap();
        assert_eq!((ids.value(0), ids.value(1), ids.is_null(2)), (7, -8, true));
        let codes = batch
            .column(1)
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        assert_eq!((codes.value(0), codes.value(1)), ("00123", "01234"));
        let sci = batch
            .column(3)
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        assert_eq!((sci.value(0), sci.value(1)), (0.0015, 100_000.0));
        // Dates, timestamps and booleans are read with their values, and the
        // empty row is null in each.
        let days = batch
            .column(4)
            .as_any()
            .downcast_ref::<arrow_array::Date32Array>()
            .unwrap();
        assert_eq!(
            (days.value(0), days.value(1), days.is_null(2)),
            (18_321, 10_956, true)
        );
        let at = batch
            .column(5)
            .as_any()
            .downcast_ref::<arrow_array::TimestampMicrosecondArray>()
            .unwrap();
        assert_eq!(at.value(0), 1_578_219_630_000_000);
        assert_eq!(at.value(1), 1_578_219_630_123_456);
        assert!(at.is_null(2));
        let flag = batch
            .column(6)
            .as_any()
            .downcast_ref::<arrow_array::BooleanArray>()
            .unwrap();
        assert_eq!(
            (flag.value(0), flag.value(1), flag.is_null(2)),
            (true, false, true)
        );
    }

    #[test]
    fn a_cell_fits_a_type_only_by_the_same_rules_that_chose_it() {
        use ColumnType::*;
        // Empty is null and fits everything.
        for t in [Int, Float, Bool, String, Date, Timestamp] {
            assert!(cell_fits("", t), "{t:?}");
        }
        assert!(cell_fits("anything at all", String));
        assert!(cell_fits("12", Int) && cell_fits("12", Float));
        assert!(cell_fits("1.5", Float) && !cell_fits("1.5", Int));
        assert!(cell_fits("TRUE", Bool) && !cell_fits("yes", Bool) && !cell_fits("1", Bool));
        assert!(cell_fits("2020-02-29", Date) && !cell_fits("2021-02-29", Date));
        assert!(cell_fits("2020-01-05 10:20:30", Timestamp) && !cell_fits("2020-01-05", Timestamp));
        // What a number parser would accept but the inference never would.
        for cell in ["007", "+5", " 5", "1e999", "0123456789012345678"] {
            assert!(!cell_fits(cell, Int), "{cell}");
        }
        assert!(!cell_fits("007.5", Float) && !cell_fits("N/A", Float));
    }
}
