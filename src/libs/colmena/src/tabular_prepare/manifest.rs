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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub version: u32,
    pub tables: Vec<TableInfo>,
}

impl Manifest {
    pub fn new(tables: Vec<TableInfo>) -> Self {
        Self {
            version: MANIFEST_VERSION,
            tables,
        }
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
        serde_json::to_string(self).map_err(|e| ManifestError::Invalid(e.to_string()))
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
            if t.name.is_empty()
                || t.name.chars().count() > MAX_NAME_CHARS
                || t.name.chars().any(char::is_control)
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
                if c.name.is_empty()
                    || c.name.chars().count() > MAX_COLUMN_NAME_CHARS
                    || c.name.chars().any(char::is_control)
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
        let cleaned: String = name
            .trim()
            .chars()
            .map(|c| if c.is_control() { '_' } else { c })
            .take(MAX_NAME_CHARS)
            .collect();
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
}
