//! The driver that prepares one large CSV: claim the registry row, convert the
//! source into Parquet parts on the host's storage, write the manifest LAST and
//! complete the row with it. Dark behind `COLMENA_LARGE_TABULAR`.
//!
//! Order matters. The manifest is the only thing that names the parts, so it is
//! put after every part is stored and the row is completed only after the
//! manifest is stored: a reader that finds a ready row finds every part it
//! lists. The key of every object is recorded before its put (the keys of the
//! parts by [`ConvertControl`], the manifest's here), so the registry always
//! tracks the union of everything any attempt may have written, including what
//! a failed put left. A failure is recorded with `fail_with_blobs` and the
//! tracked objects are then deleted; nothing but the failure reason is
//! user-visible, and it never carries a cell or a storage key.

use crate::storage::domain::OutputStorageRepository;
use crate::tabular_prepare::convert::ConvertedTable;
use crate::tabular_prepare::csv::Encoding;
use crate::tabular_prepare::manifest::{
    unique_table_names, ConversionReport, Manifest, MAX_REPORTED_DEMOTED,
};
use crate::tabular_prepare::prepare::PrepareStartError;
use crate::tabular_prepare::registry::PreparationRegistry;
use crate::tabular_prepare::writer::WriterConfig;
use chrono::{DateTime, Utc};
use std::sync::Arc;
use std::time::Duration;

/// Time a preparation may take before it stops itself.
pub const PREP_TIMEOUT: Duration = Duration::from_secs(300);

/// The reasons a preparation can be recorded as failed. Stable, lowercase
/// snake_case; the host maps them to what the user sees. Each is written
/// together with a fixed detail sentence that carries no cell and no key.
pub mod reason {
    /// The time budget ([`super::PREP_TIMEOUT`]) ran out.
    pub const TIME: &str = "time";
    /// The storage could not be read or written (source read, a part or the
    /// manifest put), or answered with a key outside the prepared layout.
    pub const STORAGE: &str = "storage";
    /// The file cannot be read as a CSV: empty, UTF-16 or binary, an unclosed
    /// quote, a record over the limit, a row longer than the header, or more
    /// columns than the reader accepts.
    pub const UNREADABLE_FILE: &str = "unreadable_file";
    /// The table list does not fit the registry row (more than 64 KiB).
    pub const TABLE_TOO_LARGE: &str = "table_too_large";
    /// Anything else: a defect, never the file's fault.
    pub const INTERNAL: &str = "internal";
}

/// Source of "now". Read once per registry write.
pub type Clock = dyn Fn() -> DateTime<Utc> + Send + Sync;

/// What the driver needs.
pub struct PrepareEnv {
    pub registry: Arc<dyn PreparationRegistry>,
    pub storage: Arc<dyn OutputStorageRepository>,
    pub clock: Arc<Clock>,
    pub budget: Duration,
    pub writer: WriterConfig,
}

impl PrepareEnv {
    pub fn new(
        registry: Arc<dyn PreparationRegistry>,
        storage: Arc<dyn OutputStorageRepository>,
    ) -> Self {
        Self {
            registry,
            storage,
            clock: Arc::new(Utc::now),
            budget: PREP_TIMEOUT,
            writer: WriterConfig::default(),
        }
    }

    pub fn with_clock(mut self, clock: Arc<Clock>) -> Self {
        self.clock = clock;
        self
    }

    pub fn with_budget(mut self, budget: Duration) -> Self {
        self.budget = budget;
        self
    }

    pub fn with_writer(mut self, writer: WriterConfig) -> Self {
        self.writer = writer;
        self
    }
}

/// A table that is ready: what the registry row and the manifest record, and
/// what the conversion reported.
#[derive(Debug)]
pub struct PreparedTable {
    pub manifest: Manifest,
    /// Full storage key of the manifest.
    pub manifest_key: String,
    /// Every object any attempt may have written, parts and manifest, as full
    /// storage keys.
    pub blob_keys: Vec<String>,
    /// The part of `blob_keys` the manifest does not reference (left by an
    /// aborted run): safe to delete.
    pub stale_keys: Vec<String>,
    pub prepared_bytes: u64,
    /// Everything the conversion reported (rows, parts, encoding, replacements,
    /// the UTF-8 counts, blank and padded rows, demoted columns, restarts).
    pub converted: ConvertedTable,
}

/// A failure that was recorded in the registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrepareFailure {
    pub code: &'static str,
    pub detail: String,
}

#[derive(Debug)]
pub enum PrepareOutcome {
    Ready(Box<PreparedTable>),
    /// The storage adapter reports no derived root: nothing was written, not
    /// even a registry row.
    Refused(PrepareStartError),
    /// Someone else holds the source, it is final, or it is ready already.
    NotClaimed,
    /// The row is gone or another job owns it: nothing was recorded.
    Cancelled,
    Failed(PrepareFailure),
}

pub fn encoding_name(e: Encoding) -> &'static str {
    match e {
        Encoding::Utf8 => "utf-8",
        Encoding::Windows1252 => "windows-1252",
    }
}

pub fn table_name(filename: &str) -> String {
    let stem = std::path::Path::new(filename)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    unique_table_names(&[stem]).remove(0)
}

pub fn report_of(table: &str, t: &ConvertedTable) -> ConversionReport {
    ConversionReport {
        table: table.to_string(),
        encoding: encoding_name(t.encoding).to_string(),
        replacements: t.replacements,
        utf8_valid_multibyte: t.utf8_valid_multibyte,
        utf8_invalid: t.utf8_invalid,
        blank_rows: t.blank_rows,
        blank_dropped: t.blank_dropped,
        padded_rows: t.padded_rows,
        restarts: t.restarts as u32,
        all_strings: t.all_strings,
        demoted_count: t.demoted.len() as u32,
        demoted: t
            .demoted
            .iter()
            .take(MAX_REPORTED_DEMOTED)
            .cloned()
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tabular_prepare::manifest::{ColumnInfo, ColumnType, MAX_NAME_CHARS};
    use crate::tabular_prepare::writer::TableWritten;

    #[test]
    fn the_table_is_named_after_the_file_without_its_extension() {
        assert_eq!(table_name("sales.csv"), "sales");
        assert_eq!(table_name("Q3 report.final.CSV"), "Q3 report.final");
        assert_eq!(table_name("data"), "data");
        // A name that is empty or only whitespace gets the generic one.
        assert_eq!(table_name(""), "sheet1");
        assert_eq!(table_name("  .csv"), "sheet1");
        assert_eq!(table_name(".csv"), ".csv");
        // Control characters are replaced and the length is bounded.
        assert_eq!(table_name("a\u{7}b.csv"), "a_b");
        assert_eq!(
            table_name(&format!("{}.csv", "x".repeat(200)))
                .chars()
                .count(),
            MAX_NAME_CHARS
        );
        // What comes out always passes the manifest's own check.
        for raw in ["a\u{0}b.csv", "\u{202e}x.csv", "../../etc/passwd.csv"] {
            let name = table_name(raw);
            assert!(
                !name.is_empty() && !name.chars().any(char::is_control),
                "{raw}"
            );
        }
    }

    fn converted(demoted: usize) -> ConvertedTable {
        ConvertedTable {
            written: TableWritten {
                rows: 10,
                parts: 1,
                columns: vec![ColumnInfo {
                    name: "a".into(),
                    column_type: ColumnType::String,
                    uncompressed_bytes: 1,
                    in_memory_bytes: 40,
                }],
            },
            restarts: 2,
            demoted: (0..demoted).map(|i| format!("c{i}")).collect(),
            all_strings: demoted > 0,
            encoding: Encoding::Windows1252,
            replacements: 0,
            utf8_valid_multibyte: 3,
            utf8_invalid: 90,
            blank_rows: 1,
            blank_dropped: 2,
            padded_rows: 4,
            blob_paths: Vec::new(),
            stale_paths: Vec::new(),
        }
    }

    #[test]
    fn the_report_carries_every_count_the_conversion_gave() {
        let r = report_of("sales", &converted(2));
        assert_eq!(r.table, "sales");
        assert_eq!(r.encoding, "windows-1252");
        assert_eq!(
            (r.replacements, r.utf8_valid_multibyte, r.utf8_invalid),
            (0, 3, 90)
        );
        assert_eq!((r.blank_rows, r.blank_dropped, r.padded_rows), (1, 2, 4));
        assert_eq!((r.restarts, r.all_strings), (2, true));
        assert_eq!((r.demoted_count, r.demoted.len()), (2, 2));
        let mut utf8 = converted(0);
        utf8.encoding = Encoding::Utf8;
        assert_eq!(report_of("t", &utf8).encoding, "utf-8");
    }

    #[test]
    fn a_wide_table_reports_the_exact_count_and_only_the_first_names() {
        let r = report_of("wide", &converted(5000));
        assert_eq!(r.demoted_count, 5000);
        assert_eq!(r.demoted.len(), MAX_REPORTED_DEMOTED);
        assert_eq!(r.demoted[0], "c0");
        assert_eq!(r.demoted[MAX_REPORTED_DEMOTED - 1], "c31");
        // The manifest accepts what the report produces.
        let manifest = Manifest::new(vec![crate::tabular_prepare::manifest::TableInfo {
            name: "wide".into(),
            rows: 10,
            parts: 1,
            columns: converted(0).written.columns,
        }])
        .with_conversion(vec![r]);
        assert!(manifest.to_json().is_ok());
    }

    #[test]
    fn the_time_budget_is_the_design_value_and_the_reasons_are_stable_snake_case() {
        assert_eq!(PREP_TIMEOUT, Duration::from_secs(300));
        let all = [
            reason::TIME,
            reason::STORAGE,
            reason::UNREADABLE_FILE,
            reason::TABLE_TOO_LARGE,
            reason::INTERNAL,
        ];
        for r in all {
            assert!(
                r.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'),
                "{r}"
            );
        }
        let mut sorted = all.to_vec();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), all.len());
        assert_eq!(reason::TIME, "time");
    }
}
