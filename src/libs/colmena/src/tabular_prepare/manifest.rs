//! Manifest of a prepared source: what tables it holds, their columns and
//! where the Parquet parts live. Dark behind `COLMENA_LARGE_TABULAR`.
//!
//! The manifest is read back from storage by a later turn, so everything it
//! says is validated again on the way in ([`Manifest::from_json`]) and every
//! storage path is built from numbers by one function ([`part_path`]) that
//! produces exactly one spelling.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use thiserror::Error;

/// Version of the manifest layout. A reader refuses any other value.
pub const MANIFEST_VERSION: u32 = 2;

/// Path of the manifest, relative to the prepared root of a source.
pub const MANIFEST_PATH: &str = "manifest.json";

/// Cap on `tables_json`, the copy of the table list the registry row keeps.
/// Never truncated: a source that needs more fails with
/// [`ManifestError::ManifestTooLarge`].
pub const TABLES_JSON_MAX_BYTES: usize = 64 * 1024;

/// Cap on a manifest file read back from storage. The table list inside it is
/// already held to [`TABLES_JSON_MAX_BYTES`] when it is written, so anything
/// much larger did not come from this module.
pub const MANIFEST_MAX_BYTES: usize = 128 * 1024;

/// Tables per source (an Excel file has one per non-empty sheet).
pub const MAX_TABLES: usize = 256;

/// Columns per table (the Excel sheet limit).
pub const MAX_COLUMNS: usize = 16_384;

/// Parts per table: `part_path` pads the index to five digits, and a sixth
/// digit would give a second spelling of the same path.
pub const MAX_PARTS: usize = 100_000;

/// Longest table name, in characters.
pub const MAX_NAME_CHARS: usize = 64;

/// Longest column name, in characters.
pub const MAX_COLUMN_NAME_CHARS: usize = 128;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ManifestError {
    #[error("the table list is {bytes} bytes, above the {cap}-byte limit; export fewer columns or sheets")]
    ManifestTooLarge { bytes: usize, cap: usize },
    #[error("invalid manifest: {0}")]
    Invalid(String),
}

/// Column types the converter can write.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ColumnType {
    Int,
    Float,
    Bool,
    String,
    Date,
    Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ColumnInfo {
    pub name: String,
    #[serde(rename = "type")]
    pub column_type: ColumnType,
    /// Uncompressed size of the column across all parts as Parquet encodes it
    /// (a dictionary-encoded string column is far smaller than its values).
    /// For information and for sizing storage; **not** for budgeting memory.
    pub uncompressed_bytes: u64,
    /// The size of the column once decoded into Arrow buffers (values, string
    /// offsets, validity bits), summed over the parts. It is **not** an upper
    /// bound of what a Python reader holds: a boolean is a bit here and a byte
    /// in numpy, a date is 4 bytes here and 8 in pandas, a string is its bytes
    /// plus 4 here and about 57 bytes plus its bytes as Python objects. A reader
    /// starts from this figure and applies its own per-type multipliers (see the
    /// developer guide). It is at least the fixed width of the type times the
    /// rows, which `validate` checks.
    pub in_memory_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TableInfo {
    pub name: String,
    pub rows: u64,
    pub parts: u32,
    pub columns: Vec<ColumnInfo>,
}

/// Most demoted column names a report lists (the count is always exact), so
/// the report stays small however wide the table is.
pub const MAX_REPORTED_DEMOTED: usize = 32;

/// What converting one table did, so a reader or the tool can warn the user
/// about what changed silently. It is not part of the table list the registry
/// row keeps (`tables_json`): that stays within its cap however many columns a
/// table has. Optional in the manifest: a manifest without it is valid.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversionReport {
    /// Name of the table in `tables` this describes.
    pub table: String,
    /// `utf-8` or `windows-1252`: how the file was decoded.
    pub encoding: String,
    /// Invalid UTF-8 sequences replaced by U+FFFD (zero for Windows-1252).
    pub replacements: u64,
    /// Non-ASCII UTF-8 characters and invalid UTF-8 sequences found in the whole
    /// file, whichever encoding was chosen: the evidence of the choice.
    pub utf8_valid_multibyte: u64,
    pub utf8_invalid: u64,
    /// Blank lines that became null rows (a one-column file).
    pub blank_rows: u64,
    /// Blank lines dropped.
    pub blank_dropped: u64,
    /// Rows shorter than the header, padded with nulls.
    pub padded_rows: u64,
    /// Type restarts the conversion needed.
    pub restarts: u32,
    /// Every column is text because the restarts ran out.
    pub all_strings: bool,
    /// Columns typed from the sample that ended as text: how many, and the first
    /// [`MAX_REPORTED_DEMOTED`] names.
    pub demoted_count: u32,
    pub demoted: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub version: u32,
    pub tables: Vec<TableInfo>,
    /// What the conversion did, per table. Absent when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conversion: Vec<ConversionReport>,
    /// Sheets of a workbook that were not turned into a table, and why. Absent when
    /// empty (always, for a CSV).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skipped: Vec<SkippedSheet>,
}

/// The reason a sheet is skipped: its first row with a value is narrower than a row
/// of its first 10,000 data rows (a title above the table, say), so it has no header
/// to name its columns.
pub const SKIPPED_HEADER_ROW: &str = "header_row";

/// A sheet that is not a table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkippedSheet {
    /// The sheet's name, cleaned like a table name.
    pub sheet: String,
    pub reason: String,
}

/// A character that shows nothing, or that makes a name read as something else, and
/// that no name needs: bidirectional controls, overrides and isolates, zero-width and
/// other invisible format characters (category Cf), the byte order mark, tag characters,
/// the line and paragraph separators, and the characters that render blank though they are
/// letters or symbols (combining grapheme joiner, Hangul and Braille fillers, Khmer inherent
/// vowels, Mongolian free variation selectors, variation selectors). The zero-width
/// non-joiner and joiner (U+200C, U+200D) are not here: see [`clean_name`].
fn is_invisible(c: char) -> bool {
    matches!(c,
        '\u{AD}' | '\u{34F}' | '\u{600}'..='\u{605}' | '\u{61C}' | '\u{6DD}' | '\u{70F}'
        | '\u{890}'..='\u{891}' | '\u{8E2}' | '\u{115F}' | '\u{1160}' | '\u{17B4}' | '\u{17B5}'
        | '\u{180B}'..='\u{180E}' | '\u{200B}' | '\u{200E}'..='\u{200F}' | '\u{2028}'..='\u{202E}'
        | '\u{2060}'..='\u{206F}' | '\u{2800}' | '\u{3164}' | '\u{FE00}'..='\u{FE0F}' | '\u{FEFF}'
        | '\u{FFA0}' | '\u{FFF9}'..='\u{FFFB}' | '\u{110BD}' | '\u{110CD}'
        | '\u{13430}'..='\u{1343F}' | '\u{1BCA0}'..='\u{1BCA3}' | '\u{1D173}'..='\u{1D17A}'
        | '\u{E0001}' | '\u{E0020}'..='\u{E007F}' | '\u{E0100}'..='\u{E01EF}')
}

/// A space other than the ASCII one (no-break, the en/em family, narrow no-break,
/// medium mathematical, ideographic, Ogham): read as a plain space.
fn is_odd_space(c: char) -> bool {
    matches!(
        c,
        '\u{A0}' | '\u{1680}' | '\u{2000}'..='\u{200A}' | '\u{202F}' | '\u{205F}' | '\u{3000}'
    )
}

/// Whether a character may be part of a table or column name: not a control character,
/// not invisible (see [`clean_name`]) and not a space other than the ASCII one.
pub fn is_clean_char(c: char) -> bool {
    !c.is_control() && !is_invisible(c) && !is_odd_space(c)
}

/// A name as the manifest keeps it. Control characters become `_`; invisible characters
/// are removed (so two names that differ only by them are the same name, and the usual
/// `_2` suffix tells them apart); other spaces become a plain one; and the zero-width
/// non-joiner and joiner, which Persian, Indic scripts and emoji sequences need, are kept
/// only between two visible characters. The result is trimmed; an empty one is for the
/// caller to name (`columnN`, `sheetN`).
pub fn clean_name(raw: &str) -> String {
    const ZWNJ: char = '\u{200C}';
    const ZWJ: char = '\u{200D}';
    let chars: Vec<char> = raw
        .chars()
        .filter_map(|c| {
            if c.is_control() {
                Some('_')
            } else if is_odd_space(c) {
                Some(' ')
            } else if is_invisible(c) {
                None
            } else {
                Some(c)
            }
        })
        .collect();
    let visible = |c: char| !c.is_whitespace() && c != ZWNJ && c != ZWJ;
    let mut out = String::with_capacity(raw.len());
    for (i, c) in chars.iter().enumerate() {
        let joiner = *c == ZWNJ || *c == ZWJ;
        let between =
            i > 0 && visible(chars[i - 1]) && chars.get(i + 1).is_some_and(|n| visible(*n));
        if !joiner || between {
            out.push(*c);
        }
    }
    out.trim().to_string()
}

impl Manifest {
    pub fn new(tables: Vec<TableInfo>) -> Self {
        Self {
            version: MANIFEST_VERSION,
            tables,
            conversion: Vec::new(),
            skipped: Vec::new(),
        }
    }

    pub fn with_skipped(mut self, skipped: Vec<SkippedSheet>) -> Self {
        self.skipped = skipped;
        self
    }

    pub fn with_conversion(mut self, conversion: Vec<ConversionReport>) -> Self {
        self.conversion = conversion;
        self
    }

    /// The table list as the registry row stores it. Exactly the `tables`
    /// array of the manifest, refused when it passes
    /// [`TABLES_JSON_MAX_BYTES`].
    pub fn tables_json(&self) -> Result<String, ManifestError> {
        let json = serde_json::to_string(&self.tables)
            .map_err(|e| ManifestError::Invalid(e.to_string()))?;
        if json.len() > TABLES_JSON_MAX_BYTES {
            return Err(ManifestError::ManifestTooLarge {
                bytes: json.len(),
                cap: TABLES_JSON_MAX_BYTES,
            });
        }
        Ok(json)
    }

    /// The manifest file. Refused under the same rule as `tables_json`, so a
    /// manifest that could not be recorded in the row is never written.
    pub fn to_json(&self) -> Result<String, ManifestError> {
        // Everything a reader will check is checked before writing, so a
        // manifest that was written can always be read.
        self.validate()?;
        self.tables_json()?;
        let json =
            serde_json::to_string(self).map_err(|e| ManifestError::Invalid(e.to_string()))?;
        if json.len() > MANIFEST_MAX_BYTES {
            return Err(ManifestError::Invalid(format!(
                "manifest is {} bytes, above the {MANIFEST_MAX_BYTES}-byte limit",
                json.len()
            )));
        }
        Ok(json)
    }

    /// Parses and validates a manifest read from storage. The size is checked
    /// before parsing; counts, names and the version are checked after.
    pub fn from_json(bytes: &[u8]) -> Result<Self, ManifestError> {
        if bytes.len() > MANIFEST_MAX_BYTES {
            return Err(ManifestError::Invalid(format!(
                "manifest is {} bytes, above the {MANIFEST_MAX_BYTES}-byte limit",
                bytes.len()
            )));
        }
        // The version first: a manifest of another version is refused as such,
        // not as a parse error about whichever field changed.
        let probe: serde_json::Value =
            serde_json::from_slice(bytes).map_err(|e| ManifestError::Invalid(e.to_string()))?;
        if let Some(v) = probe.get("version").and_then(serde_json::Value::as_u64) {
            if v != u64::from(MANIFEST_VERSION) {
                return Err(ManifestError::Invalid(format!(
                    "unsupported manifest version {v} (this reader reads {MANIFEST_VERSION})"
                )));
            }
        }
        let m: Manifest =
            serde_json::from_slice(bytes).map_err(|e| ManifestError::Invalid(e.to_string()))?;
        m.validate()?;
        Ok(m)
    }

    fn validate(&self) -> Result<(), ManifestError> {
        let bad = |msg: String| Err(ManifestError::Invalid(msg));
        if self.version != MANIFEST_VERSION {
            return bad(format!("unsupported manifest version {}", self.version));
        }
        if self.tables.is_empty() || self.tables.len() > MAX_TABLES {
            return bad(format!(
                "{} tables (1 to {MAX_TABLES} allowed)",
                self.tables.len()
            ));
        }
        let mut seen = HashSet::new();
        for t in &self.tables {
            if t.name.trim().is_empty()
                || t.name.chars().count() > MAX_NAME_CHARS
                || !t.name.chars().all(is_clean_char)
            {
                return bad(format!("invalid table name {:?}", t.name));
            }
            if !seen.insert(t.name.to_lowercase()) {
                return bad(format!("duplicate table name {:?}", t.name));
            }
            if t.parts == 0 || t.parts as usize > MAX_PARTS {
                return bad(format!("table {:?} has {} parts", t.name, t.parts));
            }
            // Every part holds a row, except the one empty part of an empty table.
            if u64::from(t.parts) > t.rows.max(1) {
                return bad(format!(
                    "table {:?} has {} parts for {} rows",
                    t.name, t.parts, t.rows
                ));
            }
            if t.columns.is_empty() || t.columns.len() > MAX_COLUMNS {
                return bad(format!(
                    "table {:?} has {} columns",
                    t.name,
                    t.columns.len()
                ));
            }
            let mut columns = HashSet::new();
            for c in &t.columns {
                if c.name.trim().is_empty()
                    || c.name.chars().count() > MAX_COLUMN_NAME_CHARS
                    || !c.name.chars().all(is_clean_char)
                {
                    return bad(format!("invalid column name in table {:?}", t.name));
                }
                if !columns.insert(c.name.as_str()) {
                    return bad(format!("duplicate column name in table {:?}", t.name));
                }
                if c.in_memory_bytes < min_in_memory_bytes(c.column_type, t.rows) {
                    return bad(format!(
                        "a column of table {:?} is smaller in memory than its rows allow",
                        t.name
                    ));
                }
            }
        }
        if self.skipped.len() > MAX_TABLES
            || self.skipped.iter().any(|s| {
                s.sheet.is_empty()
                    || s.sheet.chars().count() > MAX_NAME_CHARS
                    || !s.sheet.chars().all(is_clean_char)
                    || s.reason != SKIPPED_HEADER_ROW
            })
        {
            return bad("invalid list of skipped sheets".to_string());
        }
        let mut reported = HashSet::new();
        for r in &self.conversion {
            if !self.tables.iter().any(|t| t.name == r.table) || !reported.insert(&r.table) {
                return bad(format!(
                    "conversion report for unknown or repeated table {:?}",
                    r.table
                ));
            }
            if r.encoding != "utf-8" && r.encoding != "windows-1252" {
                return bad(format!(
                    "unknown encoding in the report of table {:?}",
                    r.table
                ));
            }
            if r.demoted.len() > MAX_REPORTED_DEMOTED
                || (r.demoted.len() as u64) > u64::from(r.demoted_count)
                || r.demoted.iter().any(|n| {
                    n.is_empty()
                        || n.chars().count() > MAX_COLUMN_NAME_CHARS
                        || !n.chars().all(is_clean_char)
                })
            {
                return bad(format!(
                    "invalid demoted list in the report of table {:?}",
                    r.table
                ));
            }
        }
        Ok(())
    }
}

/// The least a column of `rows` rows takes decoded in Arrow buffers: the fixed
/// width of its type per row, four bytes of string offset per row, a bit per
/// boolean. The writer's `in_memory_bytes` is never below it. Saturating: the
/// row count of an untrusted manifest can be any value.
pub fn min_in_memory_bytes(t: ColumnType, rows: u64) -> u64 {
    match t {
        ColumnType::Int | ColumnType::Float | ColumnType::Timestamp => rows.saturating_mul(8),
        ColumnType::Date | ColumnType::String => rows.saturating_mul(4),
        ColumnType::Bool => rows.div_ceil(8),
    }
}

/// The smallest the table list of a one-table source can be, given what is
/// known before the conversion ends: the columns as the sample typed them and a
/// lower bound of the rows (the rows of the sample). It is a true lower bound,
/// so a source whose minimum is over [`TABLES_JSON_MAX_BYTES`] cannot be
/// recorded however it converts, and a caller can refuse it before reading the
/// rest of the file; one under the cap may still turn out too large.
///
/// Per column, whatever the type ends as (the sample's type, or text after a
/// late conflict): the shorter of the two type names, the least decoded size
/// either type allows for the rows ([`min_in_memory_bytes`]), zero stored
/// bytes; one part, and a one-letter table name.
pub fn min_tables_json_len(columns: &[(&str, ColumnType)], min_rows: u64) -> usize {
    let infos = columns
        .iter()
        .map(|(name, inferred)| {
            let shorter = [*inferred, ColumnType::String]
                .into_iter()
                .min_by_key(|t| type_name_len(*t))
                .unwrap_or(ColumnType::String);
            let in_memory = [*inferred, ColumnType::String]
                .into_iter()
                .map(|t| min_in_memory_bytes(t, min_rows))
                .min()
                .unwrap_or(0);
            ColumnInfo {
                name: name.to_string(),
                column_type: shorter,
                uncompressed_bytes: 0,
                in_memory_bytes: in_memory,
            }
        })
        .collect();
    let table = TableInfo {
        name: "t".into(),
        rows: min_rows,
        parts: 1,
        columns: infos,
    };
    serde_json::to_string(&[table]).map_or(usize::MAX, |j| j.len())
}

fn type_name_len(t: ColumnType) -> usize {
    match t {
        ColumnType::Int => 3,
        ColumnType::Bool | ColumnType::Date => 4,
        ColumnType::Float => 5,
        ColumnType::String => 6,
        ColumnType::Timestamp => 9,
    }
}

/// Storage path of one part, relative to the prepared root:
/// `t<table>/part-NNNNN.parquet`. The only place such a path is built.
pub fn part_path(table_idx: usize, part_idx: usize) -> Result<String, ManifestError> {
    if table_idx >= MAX_TABLES || part_idx >= MAX_PARTS {
        return Err(ManifestError::Invalid(format!(
            "part t{table_idx}/{part_idx} is outside the limits"
        )));
    }
    Ok(format!("t{table_idx}/part-{part_idx:05}.parquet"))
}

/// Inverse of [`part_path`]: `Some` only for the exact canonical spelling, so a
/// key read from the registry or the network can be trusted as a part path
/// (no `..`, no leading zeros in the table index, no other width).
pub fn parse_part_path(path: &str) -> Option<(usize, usize)> {
    let rest = path.strip_prefix('t')?;
    let (table, file) = rest.split_once('/')?;
    let part = file.strip_prefix("part-")?.strip_suffix(".parquet")?;
    let all_digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if !all_digits(table) || !all_digits(part) || table.len() > 3 || part.len() > 5 {
        return None;
    }
    let (t, p) = (table.parse().ok()?, part.parse().ok()?);
    (part_path(t, p).ok().as_deref() == Some(path)).then_some((t, p))
}

/// Unique, deterministic table names from the raw ones (Excel sheet names),
/// in order. A name is trimmed, control characters become `_`, and it is cut
/// to [`MAX_NAME_CHARS`]; an empty result becomes `sheet<N>` (1-based). A name
/// that repeats an earlier one, ignoring case, gets `_2`, `_3`, ... until it is
/// free.
pub fn unique_table_names(raw: &[&str]) -> Vec<String> {
    let mut taken: HashSet<String> = HashSet::new();
    let mut out = Vec::with_capacity(raw.len());
    for (i, name) in raw.iter().enumerate() {
        let cleaned: String = clean_name(name).chars().take(MAX_NAME_CHARS).collect();
        let base = if cleaned.trim().is_empty() {
            format!("sheet{}", i + 1)
        } else {
            cleaned
        };
        let mut candidate = base.clone();
        let mut n = 2;
        while !taken.insert(candidate.to_lowercase()) {
            let suffix = format!("_{n}");
            let keep = MAX_NAME_CHARS.saturating_sub(suffix.chars().count());
            candidate = format!("{}{suffix}", base.chars().take(keep).collect::<String>());
            n += 1;
        }
        out.push(candidate);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn col(name: &str, t: ColumnType, bytes: u64) -> ColumnInfo {
        ColumnInfo {
            in_memory_bytes: bytes * 2 + 100_000_000,
            name: name.to_string(),
            column_type: t,
            uncompressed_bytes: bytes,
        }
    }

    fn sample() -> Manifest {
        Manifest::new(vec![
            TableInfo {
                name: "Sales".into(),
                rows: 1_200_000,
                parts: 3,
                columns: vec![
                    col("id", ColumnType::Int, 9_600_000),
                    col("amount", ColumnType::Float, 9_600_000),
                    col("code", ColumnType::String, 4_000_000),
                    col("day", ColumnType::Date, 4_800_000),
                    col("at", ColumnType::Timestamp, 9_600_000),
                    col("paid", ColumnType::Bool, 1_200_000),
                ],
            },
            TableInfo {
                name: "Notes".into(),
                rows: 10,
                parts: 1,
                columns: vec![col("text", ColumnType::String, 100)],
            },
        ])
    }

    #[test]
    fn json_round_trip_keeps_names_rows_types_and_bytes() {
        let m = sample();
        let back = Manifest::from_json(m.to_json().unwrap().as_bytes()).unwrap();
        assert_eq!(back, m);
        assert_eq!(back.tables[0].columns[1].column_type, ColumnType::Float);
        assert_eq!(back.tables[0].columns[0].uncompressed_bytes, 9_600_000);
    }

    #[test]
    fn column_types_have_stable_wire_names() {
        let json = sample().to_json().unwrap();
        for t in ["int", "float", "string", "date", "timestamp", "bool"] {
            assert!(json.contains(&format!("\"type\":\"{t}\"")), "{t} in {json}");
        }
    }

    /// A manifest whose table list is exactly `len` bytes: valid short columns
    /// up to the length, one more with a name padded to land on it.
    fn manifest_with_tables_json_len(len: usize) -> Manifest {
        let mut m = Manifest::new(vec![TableInfo {
            name: "t".into(),
            rows: 1,
            parts: 1,
            columns: vec![col("c0", ColumnType::Int, 1)],
        }]);
        let mut i = 1;
        while len - m.tables_json().unwrap().len() > 100 {
            m.tables[0]
                .columns
                .push(col(&format!("c{i}"), ColumnType::Int, 1));
            i += 1;
        }
        let base = {
            m.tables[0].columns.push(col("p", ColumnType::Int, 1));
            m.tables_json().unwrap().len()
        };
        let last = m.tables[0].columns.last_mut().unwrap();
        last.name = "p".to_string() + &"x".repeat(len - base);
        m
    }

    #[test]
    fn tables_json_is_refused_one_byte_above_the_cap_and_never_truncated() {
        let at_cap = manifest_with_tables_json_len(TABLES_JSON_MAX_BYTES);
        assert_eq!(at_cap.tables_json().unwrap().len(), TABLES_JSON_MAX_BYTES);
        assert!(at_cap.to_json().is_ok());

        let over = manifest_with_tables_json_len(TABLES_JSON_MAX_BYTES + 1);
        let err = over.tables_json().unwrap_err();
        assert_eq!(
            err,
            ManifestError::ManifestTooLarge {
                bytes: TABLES_JSON_MAX_BYTES + 1,
                cap: TABLES_JSON_MAX_BYTES
            }
        );
        // The manifest file obeys the same rule, so it is never written
        // when the registry row could not hold its table list.
        assert!(matches!(
            over.to_json(),
            Err(ManifestError::ManifestTooLarge { .. })
        ));
    }

    #[test]
    fn part_paths_are_deterministic_and_zero_padded() {
        assert_eq!(part_path(0, 0).unwrap(), "t0/part-00000.parquet");
        assert_eq!(part_path(12, 345).unwrap(), "t12/part-00345.parquet");
        assert_eq!(part_path(255, 99_999).unwrap(), "t255/part-99999.parquet");
        assert_eq!(MANIFEST_PATH, "manifest.json");
    }

    #[test]
    fn part_path_refuses_indices_that_would_need_a_second_spelling() {
        assert!(part_path(MAX_TABLES, 0).is_err());
        assert!(part_path(0, MAX_PARTS).is_err());
    }

    #[test]
    fn parse_part_path_accepts_only_the_canonical_spelling() {
        assert_eq!(parse_part_path("t3/part-00007.parquet"), Some((3, 7)));
        for bad in [
            "../t0/part-00000.parquet",
            "t0/../part-00000.parquet",
            "/t0/part-00000.parquet",
            "t0/part-0.parquet",
            "t0/part-000000.parquet",
            "t00/part-00000.parquet",
            "t0/part-00000.parquet/",
            "t0/part-00000.PARQUET",
            "t0/part-+0001.parquet",
            "t999/part-00000.parquet",
            "t256/part-00000.parquet",
            "manifest.json",
            "",
        ] {
            assert_eq!(parse_part_path(bad), None, "{bad}");
        }
    }

    #[test]
    fn duplicate_sheet_names_get_unique_deterministic_names() {
        let names = unique_table_names(&["Sales", "sales", "SALES", "Sales_2"]);
        assert_eq!(names, vec!["Sales", "sales_2", "SALES_3", "Sales_2_2"]);
        // Same input, same output.
        assert_eq!(
            names,
            unique_table_names(&["Sales", "sales", "SALES", "Sales_2"])
        );
    }

    #[test]
    fn invalid_sheet_names_are_cleaned() {
        let long = "é".repeat(100);
        let names = unique_table_names(&["", "  ", "a\u{0}b\nc", &long, "  padded  "]);
        assert_eq!(names[0], "sheet1");
        assert_eq!(names[1], "sheet2");
        assert_eq!(names[2], "a_b_c");
        assert_eq!(names[3].chars().count(), MAX_NAME_CHARS);
        assert_eq!(names[4], "padded");
    }

    #[test]
    fn suffixed_long_names_stay_within_the_limit_and_unique() {
        let long = "x".repeat(MAX_NAME_CHARS);
        let names = unique_table_names(&[&long, &long, &long]);
        assert!(names.iter().all(|n| n.chars().count() <= MAX_NAME_CHARS));
        let lower: HashSet<_> = names.iter().map(|n| n.to_lowercase()).collect();
        assert_eq!(lower.len(), 3);
    }

    #[test]
    fn from_json_refuses_oversized_input_before_parsing() {
        let big = vec![b' '; MANIFEST_MAX_BYTES + 1];
        let err = Manifest::from_json(&big).unwrap_err();
        assert!(err.to_string().contains("above the"), "{err}");
    }

    #[test]
    fn from_json_refuses_bad_manifests() {
        let mut wrong_version = sample();
        wrong_version.version = MANIFEST_VERSION + 1;
        let mut no_tables = sample();
        no_tables.tables.clear();
        let mut too_many = sample();
        let t = too_many.tables[1].clone();
        too_many.tables = (0..=MAX_TABLES)
            .map(|i| TableInfo {
                name: format!("t{i}"),
                ..t.clone()
            })
            .collect();
        let mut dup = sample();
        dup.tables[1].name = "SALES".into();
        let mut bad_name = sample();
        bad_name.tables[0].name = "a\nb".into();
        let mut no_cols = sample();
        no_cols.tables[0].columns.clear();
        let mut many_parts = sample();
        many_parts.tables[0].parts = MAX_PARTS as u32 + 1;
        for (label, m) in [
            ("version", wrong_version),
            ("no tables", no_tables),
            ("too many tables", too_many),
            ("duplicate", dup),
            ("name", bad_name),
            ("no columns", no_cols),
            ("parts", many_parts),
        ] {
            let json = serde_json::to_vec(&m).unwrap();
            assert!(Manifest::from_json(&json).is_err(), "{label} was accepted");
        }
        assert!(Manifest::from_json(b"{not json").is_err());
        // A valid manifest with a field this reader does not know is refused.
        let mut v: serde_json::Value = serde_json::to_value(sample()).unwrap();
        v["extra"] = serde_json::json!(1);
        assert!(Manifest::from_json(&serde_json::to_vec(&v).unwrap()).is_err());
    }

    #[test]
    fn the_minimum_table_list_never_exceeds_the_real_one() {
        let mut seed = 0x1234_5678_9ABC_DEF1u64;
        let mut next = move |m: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % m
        };
        let types = [
            ColumnType::Int,
            ColumnType::Float,
            ColumnType::Bool,
            ColumnType::String,
            ColumnType::Date,
            ColumnType::Timestamp,
        ];
        for _ in 0..2000 {
            let n = 1 + next(6) as usize;
            let names: Vec<String> = (0..n).map(|i| format!("col{}_{}", i, next(1000))).collect();
            let inferred: Vec<ColumnType> = (0..n).map(|_| types[next(6) as usize]).collect();
            // Rows where eight bytes a row has more digits than four.
            let sample_rows = [0u64, 7, 1250, 1999, 2500, 12_500, 20_000][next(7) as usize];
            let rows = sample_rows + next(3);
            // Whatever the conversion ends as: each column keeps its type or is
            // text, with any sizes the validation allows.
            let columns = (0..n)
                .map(|i| {
                    let t = if next(3) == 0 {
                        ColumnType::String
                    } else {
                        inferred[i]
                    };
                    ColumnInfo {
                        name: names[i].clone(),
                        column_type: t,
                        uncompressed_bytes: next(3),
                        // As small as the validation allows, so the bound is tested close.
                        in_memory_bytes: min_in_memory_bytes(t, rows) + next(3),
                    }
                })
                .collect();
            let table = TableInfo {
                name: "t".into(),
                rows,
                parts: 1,
                columns,
            };
            let real = serde_json::to_string(&[table]).unwrap().len();
            let pairs: Vec<(&str, ColumnType)> =
                names.iter().map(String::as_str).zip(inferred).collect();
            let min = min_tables_json_len(&pairs, sample_rows);
            assert!(min <= real, "{min} > {real}");
        }
    }

    #[test]
    fn the_minimum_is_exact_for_the_smallest_possible_table() {
        let tiny = Manifest::new(vec![TableInfo {
            name: "t".into(),
            rows: 0,
            parts: 1,
            columns: vec![ColumnInfo {
                name: "a".into(),
                column_type: ColumnType::Int,
                uncompressed_bytes: 0,
                in_memory_bytes: 0,
            }],
        }]);
        assert_eq!(
            min_tables_json_len(&[("a", ColumnType::Int)], 0),
            tiny.tables_json().unwrap().len()
        );
        // More rows in the sample make the digits of the sizes longer.
        assert!(
            min_tables_json_len(&[("a", ColumnType::Int)], 10_000)
                > min_tables_json_len(&[("a", ColumnType::Int)], 0)
        );
    }

    fn invalid_manifests() -> Vec<(&'static str, Manifest)> {
        let mut dup_col = sample();
        dup_col.tables[0].columns[1].name = "id".into();
        let mut empty_col = sample();
        empty_col.tables[0].columns[0].name = String::new();
        let mut ctrl_col = sample();
        ctrl_col.tables[0].columns[0].name = "a\tb".into();
        let mut long_col = sample();
        long_col.tables[0].columns[0].name = "x".repeat(MAX_COLUMN_NAME_CHARS + 1);
        let mut no_parts = sample();
        no_parts.tables[0].parts = 0;
        let mut too_many_parts = sample();
        too_many_parts.tables[1].parts = 5; // 10 rows in 5 parts is fine
        too_many_parts.tables[1].rows = 3; // 3 rows in 5 parts is not
        let mut wrong_version = sample();
        wrong_version.version = MANIFEST_VERSION + 1;
        let mut no_tables = sample();
        no_tables.tables.clear();
        let mut dup_table = sample();
        dup_table.tables[1].name = "SALES".into();
        let mut bad_table_name = sample();
        bad_table_name.tables[0].name = "a\nb".into();
        let mut no_cols = sample();
        no_cols.tables[0].columns.clear();
        let mut many_parts = sample();
        many_parts.tables[0].parts = MAX_PARTS as u32 + 1;
        vec![
            ("duplicate column", dup_col),
            ("empty column name", empty_col),
            ("control character in a column name", ctrl_col),
            ("long column name", long_col),
            ("no parts", no_parts),
            ("more parts than rows", too_many_parts),
            ("version", wrong_version),
            ("no tables", no_tables),
            ("duplicate table", dup_table),
            ("table name", bad_table_name),
            ("no columns", no_cols),
            ("parts limit", many_parts),
        ]
    }

    #[test]
    fn to_json_refuses_everything_from_json_refuses() {
        // A manifest that could be written and then not read back would fail
        // the preparation late, at the first reader.
        for (label, m) in invalid_manifests() {
            assert!(m.to_json().is_err(), "{label} was written");
            let json = serde_json::to_vec(&m).unwrap();
            assert!(Manifest::from_json(&json).is_err(), "{label} was read");
        }
    }

    #[test]
    fn whatever_to_json_accepts_from_json_accepts_and_gives_back() {
        let mut seed = 0x2545_F491_4F6C_DD1Du64;
        let mut next = move |m: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % m
        };
        let names = ["a", "b", "id", "Id", "x y", "", "q\u{7}", "long", "é"];
        let types = [
            ColumnType::Int,
            ColumnType::Float,
            ColumnType::Bool,
            ColumnType::String,
            ColumnType::Date,
            ColumnType::Timestamp,
        ];
        let mut written = 0;
        for _ in 0..3000 {
            let tables = (0..1 + next(3))
                .map(|_| TableInfo {
                    name: names[next(names.len() as u64) as usize].to_string(),
                    rows: next(5),
                    parts: next(4) as u32,
                    columns: (0..next(4))
                        .map(|_| {
                            col(
                                names[next(names.len() as u64) as usize],
                                types[next(6) as usize],
                                next(1000),
                            )
                        })
                        .collect(),
                })
                .collect();
            let m = Manifest::new(tables);
            if let Ok(json) = m.to_json() {
                written += 1;
                assert_eq!(Manifest::from_json(json.as_bytes()).unwrap(), m);
            }
        }
        assert!(
            written > 20,
            "the generator hardly produced valid manifests: {written}"
        );
    }

    #[test]
    fn a_column_has_both_the_stored_size_and_an_estimate_of_its_size_in_memory() {
        let c = ColumnInfo {
            name: "t".into(),
            column_type: ColumnType::String,
            uncompressed_bytes: 100,
            in_memory_bytes: 9_000,
        };
        let json = serde_json::to_string(&c).unwrap();
        assert!(json.contains("\"in_memory_bytes\":9000"), "{json}");
        let back: ColumnInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(back, c);
    }

    #[test]
    fn a_manifest_of_another_version_is_refused_as_such() {
        // Version 1 had no in_memory_bytes; it must be refused by its version,
        // not by a parse error about a missing field.
        let v1 = r#"{"version":1,"tables":[{"name":"t","rows":1,"parts":1,"columns":[{"name":"a","type":"int","uncompressed_bytes":9}]}]}"#;
        let err = Manifest::from_json(v1.as_bytes()).unwrap_err().to_string();
        assert!(err.contains("unsupported manifest version 1"), "{err}");
        let v3 = sample()
            .to_json()
            .unwrap()
            .replace("\"version\":2", "\"version\":3");
        assert!(Manifest::from_json(v3.as_bytes())
            .unwrap_err()
            .to_string()
            .contains("version 3"));
        assert_eq!(sample().version, MANIFEST_VERSION);
    }

    fn report(table: &str) -> ConversionReport {
        ConversionReport {
            table: table.to_string(),
            encoding: "windows-1252".into(),
            replacements: 0,
            utf8_valid_multibyte: 3,
            utf8_invalid: 90,
            blank_rows: 1,
            blank_dropped: 2,
            padded_rows: 4,
            restarts: 2,
            all_strings: false,
            demoted_count: 2,
            demoted: vec!["a".into(), "b".into()],
        }
    }

    #[test]
    fn a_conversion_report_round_trips_and_stays_out_of_the_table_list() {
        let plain = sample();
        let name = plain.tables[0].name.clone();
        let with = plain.clone().with_conversion(vec![report(&name)]);
        let back = Manifest::from_json(with.to_json().unwrap().as_bytes()).unwrap();
        assert_eq!(back, with);
        // The registry row keeps the same table list with or without it.
        assert_eq!(with.tables_json().unwrap(), plain.tables_json().unwrap());
        // Without a report the manifest has no such key (it is optional).
        assert!(!plain.to_json().unwrap().contains("conversion"));
        assert!(Manifest::from_json(plain.to_json().unwrap().as_bytes()).is_ok());
    }

    #[test]
    fn a_conversion_report_that_does_not_fit_its_tables_is_refused() {
        let base = sample();
        let name = base.tables[0].name.clone();
        let bad = |r: ConversionReport| base.clone().with_conversion(vec![r]);
        let mut unknown = report(&name);
        unknown.table = "nope".into();
        let mut encoding = report(&name);
        encoding.encoding = "latin1".into();
        let mut too_many = report(&name);
        too_many.demoted = (0..=MAX_REPORTED_DEMOTED)
            .map(|i| format!("c{i}"))
            .collect();
        too_many.demoted_count = 1000;
        let mut over_count = report(&name);
        over_count.demoted_count = 1;
        let mut control = report(&name);
        control.demoted = vec!["a\nb".into()];
        for (why, m) in [
            ("unknown table", bad(unknown)),
            ("unknown encoding", bad(encoding)),
            ("too many names", bad(too_many)),
            ("more names than the count", bad(over_count)),
            ("control character", bad(control)),
        ] {
            assert!(m.to_json().is_err(), "{why}");
            let json = serde_json::to_vec(&m).unwrap();
            assert!(Manifest::from_json(&json).is_err(), "{why} (read back)");
        }
        let twice = base
            .clone()
            .with_conversion(vec![report(&name), report(&name)]);
        assert!(twice.to_json().is_err());
    }

    #[test]
    fn a_manifest_over_the_file_limit_is_never_written() {
        // A table list at its cap leaves less than the file limit for a report:
        // names at their longest still fit, and a manifest that would not be
        // readable back is refused when written.
        let m = manifest_with_tables_json_len(TABLES_JSON_MAX_BYTES);
        let name = m.tables[0].name.clone();
        let mut r = report(&name);
        r.demoted_count = MAX_REPORTED_DEMOTED as u32;
        r.demoted = (0..MAX_REPORTED_DEMOTED)
            .map(|i| format!("{i:0>128}"))
            .collect();
        let json = m.with_conversion(vec![r]).to_json().unwrap();
        assert!(json.len() <= MANIFEST_MAX_BYTES, "{}", json.len());
        assert!(Manifest::from_json(json.as_bytes()).is_ok());
    }

    #[test]
    fn many_reports_cannot_push_the_manifest_past_what_a_reader_accepts() {
        // Many tables each carrying a full report: the table list is within its
        // cap, the file is not, and it is refused instead of written unreadable.
        let base = sample();
        let tables: Vec<TableInfo> = (0..100)
            .map(|i| TableInfo {
                name: format!("t{i}"),
                ..base.tables[0].clone()
            })
            .collect();
        let reports = tables
            .iter()
            .map(|t| {
                let mut r = report(&t.name);
                r.demoted_count = MAX_REPORTED_DEMOTED as u32;
                r.demoted = (0..MAX_REPORTED_DEMOTED)
                    .map(|i| format!("{i:0>128}"))
                    .collect();
                r
            })
            .collect();
        let m = Manifest::new(tables).with_conversion(reports);
        assert!(m.tables_json().is_ok());
        let err = m.to_json().unwrap_err().to_string();
        assert!(err.contains("above the"), "{err}");
    }

    #[test]
    fn the_minimum_size_saturates_for_an_untrusted_row_count() {
        for t in [
            ColumnType::Int,
            ColumnType::Float,
            ColumnType::Timestamp,
            ColumnType::Date,
            ColumnType::String,
        ] {
            assert_eq!(min_in_memory_bytes(t, u64::MAX), u64::MAX, "{t:?}");
            assert_eq!(min_in_memory_bytes(t, u64::MAX / 2 + 1), u64::MAX, "{t:?}");
        }
        assert_eq!(min_in_memory_bytes(ColumnType::Bool, u64::MAX), 1 << 61);
        // A manifest that claims that many rows is refused, not a panic.
        let m = Manifest::new(vec![TableInfo {
            name: "t".into(),
            rows: u64::MAX,
            parts: 1,
            columns: vec![ColumnInfo {
                name: "a".into(),
                column_type: ColumnType::Int,
                uncompressed_bytes: 1,
                in_memory_bytes: 1,
            }],
        }]);
        assert!(Manifest::from_json(&serde_json::to_vec(&m).unwrap()).is_err());
    }

    #[test]
    fn a_column_cannot_be_smaller_in_memory_than_its_rows_allow() {
        for (t, rows, floor) in [
            (ColumnType::Int, 1000u64, 8000u64),
            (ColumnType::Float, 1000, 8000),
            (ColumnType::Timestamp, 1000, 8000),
            (ColumnType::Date, 1000, 4000),
            (ColumnType::String, 1000, 4000),
            (ColumnType::Bool, 1000, 125),
        ] {
            let mk = |bytes| {
                Manifest::new(vec![TableInfo {
                    name: "t".into(),
                    rows,
                    parts: 1,
                    columns: vec![ColumnInfo {
                        name: "a".into(),
                        column_type: t,
                        uncompressed_bytes: 1,
                        in_memory_bytes: bytes,
                    }],
                }])
            };
            assert!(mk(floor).to_json().is_ok(), "{t:?} at the floor");
            assert!(mk(floor - 1).to_json().is_err(), "{t:?} under the floor");
            let json = serde_json::to_vec(&mk(floor - 1)).unwrap();
            assert!(
                Manifest::from_json(&json).is_err(),
                "{t:?} read under the floor"
            );
        }
    }

    #[test]
    fn format_characters_are_stripped_from_names_and_refused_in_a_manifest() {
        // A right-to-left override, a zero-width space and the byte order mark can
        // make a name read as something else.
        let raw = ["Report\u{202E}fdp.exe", "a\u{200B}b", "\u{FEFF}sales"];
        let names = unique_table_names(&raw);
        assert_eq!(names, ["Reportfdp.exe", "ab", "sales"]);
        for c in ['\u{202E}', '\u{200B}', '\u{FEFF}', '\u{2066}', '\u{E0041}'] {
            assert!(!is_clean_char(c), "{c:?}");
        }
        assert!(is_clean_char('é') && is_clean_char('日') && is_clean_char(' '));
        let mut m = Manifest::new(vec![TableInfo {
            name: "sales".into(),
            rows: 1,
            parts: 1,
            columns: vec![ColumnInfo {
                name: "a\u{202E}b".into(),
                column_type: ColumnType::String,
                uncompressed_bytes: 1,
                in_memory_bytes: 40,
            }],
        }]);
        assert!(m.to_json().is_err(), "a format character in a column name");
        m.tables[0].columns[0].name = "ab".into();
        m.tables[0].name = "\u{200B}x".into();
        assert!(m.to_json().is_err(), "and in a table name");
    }

    #[test]
    fn skipped_sheets_round_trip_and_nothing_else_is_accepted() {
        let table = TableInfo {
            name: "Data".into(),
            rows: 1,
            parts: 1,
            columns: vec![ColumnInfo {
                name: "a".into(),
                column_type: ColumnType::String,
                uncompressed_bytes: 1,
                in_memory_bytes: 40,
            }],
        };
        let skip = |sheet: &str, reason: &str| SkippedSheet {
            sheet: sheet.into(),
            reason: reason.into(),
        };
        let ok = Manifest::new(vec![table.clone()])
            .with_skipped(vec![skip("Title", SKIPPED_HEADER_ROW)]);
        let json = ok.to_json().unwrap();
        assert!(json.contains("\"skipped\""));
        assert_eq!(Manifest::from_json(json.as_bytes()).unwrap(), ok);
        // Absent when empty: a CSV's manifest is what it was.
        assert!(!Manifest::new(vec![table.clone()])
            .to_json()
            .unwrap()
            .contains("skipped"));
        for bad in [
            skip("", SKIPPED_HEADER_ROW),
            skip("a\u{0}b", SKIPPED_HEADER_ROW),
            skip("Title", "because"),
        ] {
            let m = Manifest::new(vec![table.clone()]).with_skipped(vec![bad]);
            assert!(m.to_json().is_err());
        }
    }

    #[test]
    fn the_joiners_that_spell_persian_and_emoji_survive_and_the_rest_of_the_format_characters_do_not(
    ) {
        // A Persian header with the zero-width non-joiner, and an emoji sequence with joiners.
        let persian = "\u{645}\u{6CC}\u{200C}\u{62E}\u{648}\u{627}\u{647}\u{645}";
        let family = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}";
        assert!(is_clean_char('\u{200C}') && is_clean_char('\u{200D}'));
        assert_eq!(unique_table_names(&[persian, family]), [persian, family]);
        for c in [
            '\u{890}',
            '\u{891}',
            '\u{110CD}',
            '\u{13430}',
            '\u{1BCA0}',
            '\u{2028}',
            '\u{2029}',
            '\u{200B}',
            '\u{200E}',
            '\u{202E}',
            '\u{2066}',
            '\u{FEFF}',
            '\u{E0041}',
        ] {
            assert!(!is_clean_char(c), "{c:?}");
        }
    }

    #[test]
    fn invisible_and_blank_characters_are_removed_and_spaces_normalised() {
        // Every character that shows nothing, however it is spelled.
        for c in [
            '\u{34F}',
            '\u{115F}',
            '\u{1160}',
            '\u{3164}',
            '\u{FFA0}',
            '\u{17B4}',
            '\u{17B5}',
            '\u{180B}',
            '\u{180C}',
            '\u{180D}',
            '\u{2800}',
            '\u{2065}',
            '\u{FE00}',
            '\u{FE0F}',
            '\u{E0100}',
            '\u{E01EF}',
        ] {
            assert!(!is_clean_char(c), "{c:?}");
            assert_eq!(clean_name(&format!("a{c}b")), "ab", "{c:?}");
        }
        // Other spaces are a plain space, and trimmed at the ends.
        assert_eq!(clean_name("\u{A0}a\u{2003}b\u{3000}"), "a b");
        for c in [
            '\u{A0}', '\u{2000}', '\u{200A}', '\u{202F}', '\u{205F}', '\u{3000}',
        ] {
            assert!(!is_clean_char(c), "{c:?}");
        }
        // A name made only of invisible characters, or only of joiners, is blank and is
        // named like an empty one.
        let blanks = [
            "\u{2800}\u{3164}",
            "\u{200C}",
            "\u{200D}\u{200C}",
            "\u{FE0F}",
            "\u{A0}\u{3000}",
        ];
        assert_eq!(
            unique_table_names(&blanks),
            ["sheet1", "sheet2", "sheet3", "sheet4", "sheet5"]
        );
        // Joiners stay between visible characters and go at the ends and beside spaces.
        assert_eq!(clean_name("\u{200C}a\u{200C}b\u{200C}"), "a\u{200C}b");
        assert_eq!(clean_name("a \u{200D} b"), "a  b");
        // Names that differ only by what was removed or normalised are the same name.
        let same = ["Sales", "Sa\u{200B}les", "Sales\u{A0}", "sales\u{2800}"];
        assert_eq!(
            unique_table_names(&same),
            ["Sales", "Sales_2", "Sales_3", "sales_4"]
        );
        // A manifest refuses a blank name and one with an invisible character.
        let mut m = Manifest::new(vec![TableInfo {
            name: "ok".into(),
            rows: 1,
            parts: 1,
            columns: vec![ColumnInfo {
                name: "a".into(),
                column_type: ColumnType::String,
                uncompressed_bytes: 1,
                in_memory_bytes: 40,
            }],
        }]);
        assert!(m.to_json().is_ok());
        for bad in [" ", "\u{A0}", "a\u{3164}"] {
            m.tables[0].columns[0].name = bad.into();
            assert!(m.to_json().is_err(), "{bad:?}");
        }
    }
}
