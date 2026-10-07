//! Typing the cells of a sheet and building Arrow batches from its rows.
//!
//! An xlsx cell already has a type (a number, a boolean, a date, text), so the
//! columns are typed from the kinds of their cells, never by re-reading text:
//! a number stays a number however many digits it has (the CSV rules, which keep
//! a value of more than 15 digits as text, would turn every computed column of a
//! workbook into text), and a text cell that looks like a number stays text, as
//! it is in Excel (a zip code `00501` is never a number).
//!
//! The rules are conservative, as for a CSV: a column whose cells disagree is
//! text.
//! - Only numbers: `int` when every one is a whole number a 64-bit integer holds
//!   exactly (at most 2^53 in absolute value), else `float`.
//! - Only booleans: `bool`. Only dates: `date`; dates with timestamps:
//!   `timestamp`. Times of day, errors and anything else are text.
//! - Any other mix, and a column with no value at all: `string`.
//! - A number that is not finite is text.
//!
//! A later cell that contradicts the type of its column is a conflict (its column
//! index), and the caller makes that column text and reads the sheet again. A
//! text column keeps every value as it would be shown: a number is its shortest
//! exact decimal form (`1.5`, `42`, `0.30000000000000004`), a boolean `TRUE` or
//! `FALSE`, a date `YYYY-MM-DD` or `YYYY-MM-DD HH:MM:SS`, a time `HH:MM:SS`.

use crate::tabular_prepare::csv::{BATCH_BYTES, BATCH_CELLS, BATCH_ROWS};
use crate::tabular_prepare::infer::{InferredColumn, InferredSchema};
use crate::tabular_prepare::manifest::ColumnType;
use crate::tabular_prepare::xlsx_sheet::{Cell, RowCells};
use crate::tabular_prepare::xlsx_styles::Temporal;
use arrow_array::builder::{
    BooleanBuilder, Date32Builder, Float64Builder, Int64Builder, StringBuilder,
    TimestampMicrosecondBuilder,
};
use arrow_array::{ArrayRef, RecordBatch};
use arrow_schema::SchemaRef;
use std::borrow::Cow;
use std::sync::Arc;

const INT: u8 = 1;
const FLOAT: u8 = 2;
const BOOL: u8 = 4;
const DATE: u8 = 8;
const TIMESTAMP: u8 = 16;
const TEXT: u8 = 32;

/// The largest whole number a double holds exactly (2^53).
const EXACT_INT: f64 = 9_007_199_254_740_992.0;

fn is_whole(n: f64) -> bool {
    n.is_finite() && n.fract() == 0.0 && n.abs() <= EXACT_INT
}

fn kind_of(cell: &Cell) -> u8 {
    match cell {
        Cell::Number(n) if is_whole(*n) => INT,
        Cell::Number(n) if n.is_finite() => FLOAT,
        Cell::Bool(_) => BOOL,
        Cell::Temporal(Temporal::Date(_)) => DATE,
        Cell::Temporal(Temporal::Timestamp(_)) => TIMESTAMP,
        _ => TEXT,
    }
}

/// What kinds of cell each column has shown so far.
pub struct Seen(Vec<u8>);

impl Seen {
    pub fn new(width: usize) -> Self {
        Self(vec![0; width])
    }

    /// Notes the kinds of a row's cells. A cell beyond the width is ignored.
    pub fn observe(&mut self, row: &RowCells) {
        for (column, cell) in row {
            if let Some(seen) = self.0.get_mut(*column) {
                *seen |= kind_of(cell);
            }
        }
    }

    pub fn types(&self) -> Vec<ColumnType> {
        self.0.iter().map(|s| resolve(*s)).collect()
    }
}

fn resolve(seen: u8) -> ColumnType {
    match seen {
        0 => ColumnType::String,
        s if s & TEXT != 0 => ColumnType::String,
        s if s == INT => ColumnType::Int,
        s if s & !(INT | FLOAT) == 0 => ColumnType::Float,
        BOOL => ColumnType::Bool,
        DATE => ColumnType::Date,
        s if s & !(DATE | TIMESTAMP) == 0 => ColumnType::Timestamp,
        _ => ColumnType::String,
    }
}

/// A cell as the text a person would see.
pub fn cell_text(cell: &Cell) -> Cow<'_, str> {
    match cell {
        Cell::Text(t) => Cow::Borrowed(t),
        Cell::Number(n) if is_whole(*n) && n.abs() < 1e15 => Cow::Owned(format!("{}", *n as i64)),
        Cell::Number(n) => Cow::Owned(format!("{n}")),
        Cell::Bool(true) => Cow::Borrowed("TRUE"),
        Cell::Bool(false) => Cow::Borrowed("FALSE"),
        Cell::Temporal(t) => Cow::Owned(t.to_text()),
    }
}

enum Column {
    Int(Int64Builder),
    Float(Float64Builder),
    Bool(BooleanBuilder),
    Text(StringBuilder),
    Date(Date32Builder),
    Timestamp(TimestampMicrosecondBuilder),
}

impl Column {
    fn new(t: ColumnType) -> Self {
        match t {
            ColumnType::Int => Self::Int(Int64Builder::new()),
            ColumnType::Float => Self::Float(Float64Builder::new()),
            ColumnType::Bool => Self::Bool(BooleanBuilder::new()),
            ColumnType::String => Self::Text(StringBuilder::new()),
            ColumnType::Date => Self::Date(Date32Builder::new()),
            ColumnType::Timestamp => Self::Timestamp(TimestampMicrosecondBuilder::new()),
        }
    }

    fn append_null(&mut self) {
        match self {
            Self::Int(b) => b.append_null(),
            Self::Float(b) => b.append_null(),
            Self::Bool(b) => b.append_null(),
            Self::Text(b) => b.append_null(),
            Self::Date(b) => b.append_null(),
            Self::Timestamp(b) => b.append_null(),
        }
    }

    /// Appends a cell; the bytes of text it added, or `None` when the cell does
    /// not fit this column's type.
    fn append(&mut self, cell: &Cell) -> Option<usize> {
        match (self, cell) {
            (Self::Text(b), cell) => {
                let text = cell_text(cell);
                b.append_value(&text);
                return Some(text.len());
            }
            (Self::Int(b), Cell::Number(n)) if is_whole(*n) => b.append_value(*n as i64),
            (Self::Float(b), Cell::Number(n)) if n.is_finite() => b.append_value(*n),
            (Self::Bool(b), Cell::Bool(v)) => b.append_value(*v),
            (Self::Date(b), Cell::Temporal(Temporal::Date(d))) => b.append_value(*d),
            (Self::Timestamp(b), Cell::Temporal(Temporal::Timestamp(t))) => b.append_value(*t),
            (Self::Timestamp(b), Cell::Temporal(Temporal::Date(d))) => {
                b.append_value(i64::from(*d) * 86_400_000_000);
            }
            _ => return None,
        }
        Some(0)
    }

    fn finish(&mut self) -> ArrayRef {
        match self {
            Self::Int(b) => Arc::new(b.finish()),
            Self::Float(b) => Arc::new(b.finish()),
            Self::Bool(b) => Arc::new(b.finish()),
            Self::Text(b) => Arc::new(b.finish()),
            Self::Date(b) => Arc::new(b.finish()),
            Self::Timestamp(b) => Arc::new(b.finish()),
        }
    }
}

/// Builds the batches of one sheet from its rows. A batch is closed at
/// `BATCH_ROWS` rows, `BATCH_CELLS` cells or `BATCH_BYTES` bytes of text, the
/// CSV reader's bounds, so a wide or text-heavy sheet gets shorter batches.
pub struct Batcher {
    schema: SchemaRef,
    columns: Vec<Column>,
    rows: usize,
    text_bytes: usize,
}

impl Batcher {
    pub fn new(names: &[String], types: &[ColumnType]) -> Self {
        let schema = InferredSchema {
            columns: names
                .iter()
                .zip(types)
                .map(|(name, t)| InferredColumn {
                    name: name.clone(),
                    column_type: *t,
                })
                .collect(),
        }
        .arrow_schema();
        Self {
            schema,
            columns: types.iter().map(|t| Column::new(*t)).collect(),
            rows: 0,
            text_bytes: 0,
        }
    }

    pub fn schema(&self) -> SchemaRef {
        self.schema.clone()
    }

    /// Adds a row (non-empty cells by column; the others are null). `Err` is the
    /// column of the first cell that does not fit its type; the row is then
    /// half added and the batcher must be dropped.
    pub fn push(&mut self, row: &RowCells) -> Result<(), usize> {
        let mut cells = row.iter().peekable();
        for (index, column) in self.columns.iter_mut().enumerate() {
            match cells.next_if(|(c, _)| *c == index) {
                Some((_, cell)) => {
                    self.text_bytes += column.append(cell).ok_or(index)?;
                }
                None => column.append_null(),
            }
        }
        self.rows += 1;
        Ok(())
    }

    /// Whether the batch should be closed before another row is added.
    pub fn is_full(&self) -> bool {
        self.rows >= BATCH_ROWS
            || (self.rows + 1) * self.columns.len().max(1) > BATCH_CELLS
            || self.text_bytes >= BATCH_BYTES
    }

    pub fn rows(&self) -> usize {
        self.rows
    }

    /// Closes the batch (`None` when it holds no row) and starts the next.
    pub fn take(&mut self) -> Option<RecordBatch> {
        if self.rows == 0 {
            return None;
        }
        let arrays: Vec<ArrayRef> = self.columns.iter_mut().map(Column::finish).collect();
        (self.rows, self.text_bytes) = (0, 0);
        RecordBatch::try_new(self.schema.clone(), arrays).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::cast::AsArray;
    use arrow_array::types::{Date32Type, Float64Type, Int64Type, TimestampMicrosecondType};

    fn num(n: f64) -> Cell {
        Cell::Number(n)
    }

    fn txt(s: &str) -> Cell {
        Cell::Text(s.into())
    }

    fn types_of(rows: &[RowCells], width: usize) -> Vec<ColumnType> {
        let mut seen = Seen::new(width);
        for r in rows {
            seen.observe(r);
        }
        seen.types()
    }

    #[test]
    fn a_column_takes_the_type_its_cells_agree_on_and_text_when_they_do_not() {
        let d = Cell::Temporal(Temporal::Date(0));
        let t = Cell::Temporal(Temporal::Timestamp(1));
        let rows: Vec<RowCells> = vec![
            vec![
                (0, num(1.0)),
                (1, num(1.0)),
                (2, Cell::Bool(true)),
                (3, d.clone()),
                (4, d.clone()),
                (5, num(1.0)),
            ],
            vec![
                (0, num(2.0)),
                (1, num(2.5)),
                (2, Cell::Bool(false)),
                (3, d.clone()),
                (4, t.clone()),
                (5, txt("N/A")),
            ],
            vec![(0, num(-3.0)), (1, num(1e300)), (3, d)],
        ];
        use ColumnType::*;
        // Column 6 has no value at all; column 7 is a time of day.
        assert_eq!(
            types_of(&rows, 7),
            [Int, Float, Bool, Date, Timestamp, String, String]
        );
        let time = Cell::Temporal(Temporal::Time(60));
        assert_eq!(types_of(&[vec![(0, time)]], 1), [String]);
        // A bool with a number, a number with a date, a number that is not finite.
        assert_eq!(
            types_of(&[vec![(0, Cell::Bool(true))], vec![(0, num(1.0))]], 1),
            [String]
        );
        assert_eq!(types_of(&[vec![(0, num(f64::NAN))]], 1), [String]);
        // 2^53 is whole and exact; one more is a float.
        assert_eq!(
            types_of(&[vec![(0, num(9_007_199_254_740_992.0))]], 1),
            [Int]
        );
        assert_eq!(
            types_of(&[vec![(0, num(9_007_199_254_740_994.0))]], 1),
            [Float]
        );
    }

    #[test]
    fn a_text_cell_that_looks_like_a_number_stays_text_and_a_long_number_stays_a_number() {
        let rows = vec![
            vec![(0, txt("00501")), (1, num(0.1 + 0.2))],
            vec![(0, txt("01234")), (1, num(1.0 / 3.0))],
        ];
        assert_eq!(types_of(&rows, 2), [ColumnType::String, ColumnType::Float]);
    }

    #[test]
    fn text_shows_each_kind_of_cell_as_a_person_would_see_it() {
        let shown = |c: Cell| cell_text(&c).into_owned();
        assert_eq!(shown(num(42.0)), "42");
        assert_eq!(shown(num(-0.0)), "0");
        assert_eq!(shown(num(1.5)), "1.5");
        assert_eq!(shown(num(0.1 + 0.2)), "0.30000000000000004");
        assert_eq!(shown(num(1e21)), "1000000000000000000000");
        assert_eq!(shown(num(1.0e15)), "1000000000000000");
        assert_eq!(shown(num(f64::NAN)), "NaN");
        assert_eq!(shown(Cell::Bool(true)), "TRUE");
        assert_eq!(shown(Cell::Temporal(Temporal::Date(18_628))), "2021-01-01");
        assert_eq!(shown(Cell::Temporal(Temporal::Time(45_000))), "12:30:00");
        assert_eq!(shown(txt("as is")), "as is");
    }

    fn names(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("c{i}")).collect()
    }

    #[test]
    fn rows_become_a_batch_with_nulls_for_the_cells_that_are_not_there() {
        use ColumnType::*;
        let types = [Int, Float, String, Date, Timestamp];
        let mut batcher = Batcher::new(&names(5), &types);
        let day = Cell::Temporal(Temporal::Date(18_628));
        batcher
            .push(&vec![
                (0, num(7.0)),
                (1, num(7.0)),
                (2, num(7.5)),
                (3, day.clone()),
                (4, day),
            ])
            .unwrap();
        batcher.push(&vec![(2, txt("x"))]).unwrap();
        assert_eq!(batcher.rows(), 2);
        let batch = batcher.take().unwrap();
        assert_eq!((batch.num_rows(), batch.num_columns()), (2, 5));
        assert_eq!(
            batch
                .column(0)
                .as_primitive::<Int64Type>()
                .iter()
                .collect::<Vec<_>>(),
            [Some(7), None]
        );
        assert_eq!(
            batch
                .column(1)
                .as_primitive::<Float64Type>()
                .iter()
                .collect::<Vec<_>>(),
            [Some(7.0), None]
        );
        let strings = batch.column(2).as_string::<i32>();
        assert_eq!((strings.value(0), strings.value(1)), ("7.5", "x"));
        assert_eq!(
            batch
                .column(3)
                .as_primitive::<Date32Type>()
                .iter()
                .collect::<Vec<_>>(),
            [Some(18_628), None]
        );
        // A date in a timestamp column is its midnight.
        let micros = batch
            .column(4)
            .as_primitive::<TimestampMicrosecondType>()
            .value(0);
        assert_eq!(micros, 18_628 * 86_400_000_000);
        assert!(batcher.take().is_none(), "a taken batch starts empty");
    }

    #[test]
    fn a_cell_that_does_not_fit_its_column_names_the_column() {
        use ColumnType::*;
        let cases: [(ColumnType, Cell); 5] = [
            (Int, num(1.5)),
            (Float, txt("x")),
            (Bool, num(1.0)),
            (Date, Cell::Temporal(Temporal::Timestamp(5))),
            (Int, num(f64::INFINITY)),
        ];
        for (t, cell) in cases {
            let mut batcher = Batcher::new(&names(2), &[String, t]);
            assert_eq!(
                batcher.push(&vec![(0, txt("ok")), (1, cell.clone())]),
                Err(1),
                "{t:?} {cell:?}"
            );
        }
        // Text takes anything.
        let mut batcher = Batcher::new(&names(1), &[String]);
        for cell in [
            num(1.5),
            Cell::Bool(true),
            Cell::Temporal(Temporal::Time(1)),
        ] {
            assert!(batcher.push(&vec![(0, cell)]).is_ok());
        }
    }
}
