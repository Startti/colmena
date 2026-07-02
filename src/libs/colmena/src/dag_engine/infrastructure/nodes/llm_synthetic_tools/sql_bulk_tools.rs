//! Synthetic tools for bulk-loading large structured attachments (CSV / XLSX)
//! into Postgres without making the LLM read every row.
//!
//! Two tools ship together; the LLM workflow is **always inspect → insert**:
//!
//! 1. [`sql_inspect_attachment`](SQL_INSPECT_ATTACHMENT_TOOL_NAME) — opens the
//!    attachment, returns headers + first N sample rows + inferred types +
//!    total-row count (~200 tokens of LLM-visible payload regardless of file
//!    size). If the optional `target_table` arg is set, the response also
//!    includes the destination table's column list and SQL types so the LLM
//!    can build a correct `column_mapping` without a separate
//!    `information_schema` round-trip.
//!
//! 2. [`sql_bulk_insert_from_attachment`](SQL_BULK_INSERT_TOOL_NAME) —
//!    streams the attachment bytes via the shared attachment plumbing
//!    (see `DagToolExecutor::fetch_attachment_stream`) and performs a
//!    Postgres `COPY FROM STDIN` so the worker process never holds the full
//!    payload in RAM. `column_mapping` is **required** (per `daniel@startti.co`
//!    decision, 2026-06-09) — the LLM must explicitly map CSV columns to DB
//!    columns even when names match identically; this prevents silent
//!    misalignment errors that are hard to debug after the fact.
//!
//! ## Backend support
//!
//! v1 is **Postgres-only**, matching the existing `sql_query` node which is
//! also Postgres-only (see `PgPoolAdapter`). Extending to SQLite/MySQL would
//! require parallel adapters in `sql_pool_adapter.rs` and is tracked
//! separately as a backlog item — adding bulk-only multi-backend would leave
//! a confusing surface where the LLM can bulk-insert into SQLite but then
//! cannot `sql_query` it back.
//!
//! ## Error handling
//!
//! Bulk insert defaults to [`OnErrorPolicy::FailFast`]: any row error rolls
//! back the entire transaction. Operators may opt in to `SkipRows` (continue
//! past row-level errors, report them) or `PartialCommit` (commit whatever
//! succeeds, surface what failed) per call. The default mirrors the safety
//! posture of `sql_query` mutations (transactional, atomic).
//!
//! ## Caps
//!
//! - Max attachment size: **50 MB** (hardcoded ceiling).
//! - Max rows inserted in a single call: **100 000** (hardcoded ceiling).
//! - Max sample rows returned by inspect: **20**.
//!
//! Caps are hardcoded for v1; making them configurable is tracked in the
//! backlog under "SQL bulk insert v1.1".

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Tool name surfaced to the LLM. Constant so other crates (text registry,
/// dispatcher router) can reference it without typo-prone string literals.
pub const SQL_INSPECT_ATTACHMENT_TOOL_NAME: &str = "sql_inspect_attachment";

/// Tool name surfaced to the LLM for the bulk insert.
pub const SQL_BULK_INSERT_TOOL_NAME: &str = "sql_bulk_insert_from_attachment";

/// Hardcoded ceilings — see module docs. Operators that need higher limits
/// can configure them through a future `runtime_limits.max_bulk_*` knob
/// (tracked in backlog).
pub const MAX_ATTACHMENT_BYTES: u64 = 50 * 1024 * 1024;
pub const MAX_BULK_INSERT_ROWS: u64 = 100_000;
pub const MAX_INSPECT_SAMPLE_ROWS: u32 = 20;
pub const DEFAULT_INSPECT_SAMPLE_ROWS: u32 = 5;
pub const DEFAULT_HEADER_ROW: u32 = 1;

// ─────────────────────────────────────────────────────────────────────────────
// Type inference
// ─────────────────────────────────────────────────────────────────────────────

/// Inferred SQL-ish type for a CSV/XLSX column, derived from the first N
/// sampled rows. The LLM sees these strings in the inspect response and uses
/// them to decide the `column_mapping`. Names line up loosely with Postgres
/// types so the model has familiar anchors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum InferredType {
    /// All sampled values parsed as i64 (or were empty).
    Integer,
    /// All sampled values parsed as f64 (or were empty), with at least one
    /// containing a decimal point.
    Numeric,
    /// All sampled values were "true" / "false" (case-insensitive) or empty.
    Bool,
    /// All sampled values matched ISO 8601 date (`YYYY-MM-DD`) — no time.
    Date,
    /// All sampled values matched ISO 8601 timestamp (with `T<time>` portion).
    Timestamp,
    /// Inference failed for at least one value, or all values were empty.
    /// Default-safe — Postgres can cast text into most things.
    Text,
}

impl InferredType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Integer => "integer",
            Self::Numeric => "numeric",
            Self::Bool => "bool",
            Self::Date => "date",
            Self::Timestamp => "timestamp",
            Self::Text => "text",
        }
    }
}

/// Source format of the attachment, derived from `mime_type` (preferred) or
/// the file extension (fallback). Emitted in the inspect response so the LLM
/// can adapt its strategy (CSV may have delimiter quirks; XLSX may have
/// multiple sheets).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum FileFormat {
    Csv,
    Xlsx,
}

impl FileFormat {
    /// Map a MIME type to the format. Returns `None` for unsupported types
    /// (PDF, image, etc.) — the dispatcher surfaces a friendly error.
    pub fn from_mime(mime: &str) -> Option<Self> {
        match mime.to_ascii_lowercase().as_str() {
            "text/csv" | "application/csv" | "text/plain" => Some(Self::Csv),
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
            | "application/vnd.ms-excel" => Some(Self::Xlsx),
            _ => None,
        }
    }

    /// Map a filename extension to a format. Used as a fallback when the
    /// `mime_type` reported by storage is generic
    /// (`application/octet-stream`) — local storage adapters do not preserve
    /// the original mime, so the catalog's filename is more reliable.
    pub fn from_filename(filename: &str) -> Option<Self> {
        let lower = filename.to_ascii_lowercase();
        if lower.ends_with(".csv") || lower.ends_with(".tsv") {
            Some(Self::Csv)
        } else if lower.ends_with(".xlsx") || lower.ends_with(".xlsm") || lower.ends_with(".xls") {
            Some(Self::Xlsx)
        } else {
            None
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// On-error policy
// ─────────────────────────────────────────────────────────────────────────────

/// What to do when a row fails to insert during the bulk operation.
///
/// `FailFast` (default) rolls back the entire transaction at the first error.
/// Operators that need partial progress can opt in to `SkipRows`
/// (transaction commits the rows that succeed) or `PartialCommit` (same as
/// SkipRows but with detailed per-row error reporting).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum OnErrorPolicy {
    /// Roll back the entire transaction at the first row error. **Default.**
    #[default]
    FailFast,
    /// Continue past row-level errors. Successful rows commit. Skipped row
    /// numbers (no per-row error details) are returned to the LLM.
    SkipRows,
    /// Same as `SkipRows` but each failed row's error message is included in
    /// the response. More expensive in tokens; useful for debugging dirty
    /// CSVs.
    PartialCommit,
}

// ─────────────────────────────────────────────────────────────────────────────
// `sql_inspect_attachment` — args + response
// ─────────────────────────────────────────────────────────────────────────────

/// Arguments accepted by the [`SQL_INSPECT_ATTACHMENT_TOOL_NAME`] tool.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct InspectArgs {
    /// `document_id` of an attachment registered in the conversation catalog.
    /// The dispatcher resolves it via
    /// `DagToolExecutor::fetch_attachment_stream` so it must already be
    /// visible in the catalog block of the system message.
    pub attachment_id: String,

    /// How many sample rows to return. Defaults to 5; clamped to the
    /// `MAX_INSPECT_SAMPLE_ROWS` ceiling.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample_rows: Option<u32>,

    /// CSV delimiter override. When omitted, the dispatcher attempts
    /// auto-detection (comma → semicolon → tab). Ignored for XLSX.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delimiter: Option<String>,

    /// XLSX sheet name. When omitted, the dispatcher uses the first sheet.
    /// Ignored for CSV.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sheet_name: Option<String>,

    /// 1-indexed row where the column headers live. Defaults to 1 (first row
    /// is the header). Set to 2 for files with a title row before headers
    /// (common in exported XLSX reports).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header_row: Option<u32>,

    /// Optional schema-qualified destination table (e.g. `public.products`).
    /// When set, the inspect response also includes the table's columns + SQL
    /// types so the LLM can build a correct `column_mapping` without a
    /// separate `information_schema` round-trip.
    ///
    /// Validated against `permissions.allowed_schemas` by the dispatcher;
    /// returns a structured error if the schema is not whitelisted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_table: Option<String>,
}

/// Response shape returned by [`SQL_INSPECT_ATTACHMENT_TOOL_NAME`]. The
/// dispatcher serializes this as the tool result and the LLM receives it.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct InspectResponse {
    /// Column names as they appear in the source file's header row.
    pub columns: Vec<String>,

    /// Inferred type per column. Same order + length as `columns`.
    pub inferred_types: Vec<InferredType>,

    /// First N rows after the header, parsed as JSON objects keyed by column
    /// name. Empty cells become JSON `null`.
    pub sample: Vec<serde_json::Map<String, serde_json::Value>>,

    /// Total number of data rows in the file (excluding header). Computed by
    /// streaming through the entire attachment without buffering rows.
    pub total_rows: u64,

    /// Source format (`csv` or `xlsx`).
    pub format: FileFormat,

    /// CSV delimiter that was used. Echoes the override or the auto-detected
    /// value. Omitted for XLSX.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delimiter: Option<String>,

    /// Encoding detected for CSV. Always `"utf-8"` for now. Omitted for XLSX.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encoding: Option<String>,

    /// XLSX sheet name that was read. Omitted for CSV.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sheet_name: Option<String>,

    /// Destination-table schema, populated when `target_table` was set in the
    /// args. The LLM consumes this to build the `column_mapping` for the
    /// follow-up `sql_bulk_insert_from_attachment` call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_table_schema: Option<TargetTableSchema>,
}

/// Destination-table schema description, embedded in the inspect response
/// when the LLM passes `target_table`. Built from `information_schema.columns`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TargetTableSchema {
    /// Fully-qualified table name as the LLM provided it (e.g.
    /// `"public.products"`).
    pub table: String,

    /// Columns of the destination table, in the order Postgres reports them.
    pub columns: Vec<TargetTableColumn>,
}

/// One column of a destination table — name, SQL type, nullability,
/// default. Mirrors `information_schema.columns` fields the LLM cares about.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TargetTableColumn {
    pub name: String,
    /// Postgres data type (`integer`, `text`, `numeric(10,2)`, etc.).
    pub data_type: String,
    pub is_nullable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column_default: Option<String>,
}

// ─────────────────────────────────────────────────────────────────────────────
// `sql_bulk_insert_from_attachment` — args + response
// ─────────────────────────────────────────────────────────────────────────────

/// Arguments accepted by the [`SQL_BULK_INSERT_TOOL_NAME`] tool.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct BulkInsertArgs {
    /// `document_id` of the attachment to stream into the destination table.
    /// Must be the same id the LLM passed to `sql_inspect_attachment` (or
    /// any id visible in the catalog).
    pub attachment_id: String,

    /// Schema-qualified destination table. Validated against
    /// `permissions.allowed_schemas` and the read-only/sandbox rules in
    /// `sql_ast`. Examples: `"public.products"`, `"reporting.sales"`.
    pub table: String,

    /// **Required.** Mapping from source-file column → destination-table
    /// column. The LLM must pass this explicitly even when names match
    /// identically. Decision rationale: identity-style mappings (5 keys) are
    /// cheap in tokens, and the explicit pass-through prevents silent
    /// misalignment when CSV columns and DB columns drift in name or order.
    pub column_mapping: std::collections::BTreeMap<String, String>,

    /// What to do on row-level errors. Defaults to [`OnErrorPolicy::FailFast`].
    #[serde(default)]
    pub on_error: OnErrorPolicy,

    /// CSV-specific overrides (delimiter + header row). Optional — defaults
    /// matter the way `sql_inspect_attachment` already detected them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delimiter: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header_row: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sheet_name: Option<String>,
}

/// Response shape returned by [`SQL_BULK_INSERT_TOOL_NAME`]. The dispatcher
/// serializes this as the tool result.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct BulkInsertResponse {
    /// Number of rows that were successfully inserted.
    pub rows_inserted: u64,

    /// Number of rows that were skipped due to errors (only > 0 for
    /// `SkipRows` and `PartialCommit` policies; always 0 for `FailFast`
    /// since the whole batch rolls back on any failure).
    pub rows_skipped: u64,

    /// End-to-end wall-clock duration of the COPY operation in milliseconds.
    pub duration_ms: u64,

    /// Backend method used to perform the bulk insert. `"copy_from_stdin"`
    /// for Postgres. Reserved for future backends (e.g.
    /// `"batched_insert_1000"` for SQLite).
    pub method: String,

    /// Per-row errors. Populated only when `on_error` is `SkipRows` or
    /// `PartialCommit` AND at least one row failed. Empty for `FailFast`
    /// (which would have rolled back instead).
    #[serde(default)]
    pub errors: Vec<BulkInsertRowError>,
}

/// Detail of one row that failed during a `SkipRows` or `PartialCommit`
/// bulk insert. The `row_index` is 1-indexed and excludes the header row.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct BulkInsertRowError {
    pub row_index: u64,
    pub message: String,
}

// ─────────────────────────────────────────────────────────────────────────────
// Tabular attachment auto-summary (option B, 2026-06-10)
// ─────────────────────────────────────────────────────────────────────────────

/// Default sample rows included in the tabular auto-summary. Three rows keeps
/// the catalog block compact (~50 tokens for typical 5-column tables) while
/// still showing enough variety for the LLM to recognise type patterns.
pub const DEFAULT_TABULAR_SUMMARY_SAMPLE_ROWS: u32 = 3;

/// Build a structured one-paragraph summary for a tabular attachment
/// (CSV/XLSX) so the LLM can see the schema + a few sample rows in the
/// catalog block of the system message **without any tool call**.
///
/// Returns:
/// - `Some(text)` when `mime` (or the filename extension) resolves to CSV
///   or XLSX AND the bytes parse cleanly. The text is the structured
///   summary the caller should persist to the attachment's `description`
///   field.
/// - `None` when the mime is not tabular OR the bytes exceed
///   `MAX_ATTACHMENT_BYTES` (50 MB) OR the parser errors. The caller
///   should then fall back to the LLM-based summary path.
///
/// **Cost:** zero LLM tokens. Pure local parse via `parse_inspect_bytes`.
pub fn build_tabular_summary(mime: &str, filename: &str, bytes: &[u8]) -> Option<String> {
    if bytes.len() as u64 > MAX_ATTACHMENT_BYTES {
        return None;
    }
    // Only fire for tabular mimes / extensions. Plain text and PDFs go
    // through the existing LLM-based summarizer.
    let format = match (
        FileFormat::from_mime(mime),
        FileFormat::from_filename(filename),
    ) {
        (Some(f), _) => f,
        (None, Some(f)) => f,
        (None, None) => return None,
    };
    // Plain text mime (`text/plain`) is too ambiguous — we don't want every
    // .txt upload to be treated as CSV. Only fire when the mime is
    // explicitly tabular OR the filename extension is `.csv`/`.xlsx`.
    let is_tabular_mime = matches!(
        FileFormat::from_mime(mime),
        Some(FileFormat::Csv) | Some(FileFormat::Xlsx)
    ) && !matches!(mime.to_ascii_lowercase().as_str(), "text/plain");
    let is_tabular_extension = FileFormat::from_filename(filename).is_some();
    if !is_tabular_mime && !is_tabular_extension {
        return None;
    }

    let args = InspectArgs {
        attachment_id: String::new(), // not used by parse path
        sample_rows: Some(DEFAULT_TABULAR_SUMMARY_SAMPLE_ROWS),
        delimiter: None,
        sheet_name: None,
        header_row: None,
        target_table: None,
    };
    let resp = parse_inspect_bytes_with_filename(bytes, mime, filename, &args).ok()?;

    Some(format_tabular_summary(&resp, format))
}

/// Render the inspect response as a compact one-paragraph summary suitable
/// for the catalog block. The format is stable and self-describing so the
/// LLM can recognize column names + types + sample rows without further
/// parsing.
fn format_tabular_summary(resp: &InspectResponse, format: FileFormat) -> String {
    let col_count = resp.columns.len();
    let row_count = resp.total_rows;

    // Header line: format, dimensions, optional sheet name.
    let format_str = match format {
        FileFormat::Csv => "CSV",
        FileFormat::Xlsx => "XLSX",
    };
    let mut header = format!("{format_str}, {col_count} cols × {row_count} rows");
    if let Some(sheet) = resp.sheet_name.as_deref() {
        header.push_str(&format!(" (sheet \"{sheet}\")"));
    }
    if let Some(delim) = resp.delimiter.as_deref() {
        let visible = if delim == "\t" { "\\t" } else { delim };
        header.push_str(&format!(" (delimiter '{visible}')"));
    }

    // Schema line: column names with inferred types.
    let mut schema = String::from("schema: ");
    for (i, (col, t)) in resp
        .columns
        .iter()
        .zip(resp.inferred_types.iter())
        .enumerate()
    {
        if i > 0 {
            schema.push_str(", ");
        }
        schema.push_str(&format!("{col} ({})", t.as_str()));
    }

    // Sample rows: render each row as a CSV-style line of values, truncating
    // long strings so a single huge cell doesn't blow up the catalog token cost.
    let mut sample = String::from("sample rows:");
    for row in &resp.sample {
        sample.push_str("\n  ");
        for (i, col) in resp.columns.iter().enumerate() {
            if i > 0 {
                sample.push_str(", ");
            }
            let raw = row.get(col).and_then(|v| v.as_str()).unwrap_or("");
            let truncated = if raw.chars().count() > 40 {
                let head: String = raw.chars().take(37).collect();
                format!("{head}...")
            } else {
                raw.to_string()
            };
            sample.push_str(&truncated);
        }
    }

    format!("{header}\n{schema}\n{sample}")
}

// ─────────────────────────────────────────────────────────────────────────────
// Pure attachment parsing — `parse_inspect_bytes`
// ─────────────────────────────────────────────────────────────────────────────

/// Parse the attachment bytes and produce the LLM-facing inspect summary.
///
/// **Pure** — does NOT touch storage, network, or any database. Test fixtures
/// pass raw CSV/XLSX bytes and assert on the returned `InspectResponse`.
///
/// Behavior contract:
/// - Format is derived from `mime_type` first; on `None`, falls back to
///   trying CSV (the common case for `text/plain` uploads).
/// - `args.sample_rows` is clamped to `[0, MAX_INSPECT_SAMPLE_ROWS]`;
///   the default is `DEFAULT_INSPECT_SAMPLE_ROWS`.
/// - `args.header_row` (1-indexed) skips rows above. Default 1.
/// - The returned `InspectResponse` does NOT include `target_table_schema`
///   — that lookup requires a SQL connection and is added at the dispatcher
///   layer (T3). Callers that need it merge it after calling this function.
///
/// Errors are returned as plain strings so the dispatcher can wrap them in
/// the structured tool error envelope the LLM expects.
pub fn parse_inspect_bytes(
    bytes: &[u8],
    mime_type: &str,
    args: &InspectArgs,
) -> Result<InspectResponse, String> {
    parse_inspect_bytes_with_filename(bytes, mime_type, "", args)
}

/// Same as [`parse_inspect_bytes`] but also takes the source `filename` so
/// the dispatcher can fall back to the file extension when the storage
/// adapter returns a generic `mime_type` (e.g. `application/octet-stream`).
/// Local in-memory storage adapters frequently drop the original mime.
pub fn parse_inspect_bytes_with_filename(
    bytes: &[u8],
    mime_type: &str,
    filename: &str,
    args: &InspectArgs,
) -> Result<InspectResponse, String> {
    if bytes.len() as u64 > MAX_ATTACHMENT_BYTES {
        return Err(format!(
            "attachment too large: {} bytes > limit {} bytes ({} MB)",
            bytes.len(),
            MAX_ATTACHMENT_BYTES,
            MAX_ATTACHMENT_BYTES / 1024 / 1024
        ));
    }
    let format = FileFormat::from_mime(mime_type)
        .or_else(|| FileFormat::from_filename(filename))
        .ok_or_else(|| {
            format!(
                "unsupported mime_type '{mime_type}' (filename '{filename}') for bulk inspect. \
                 Supported: text/csv, application/csv, text/plain, \
                 application/vnd.openxmlformats-officedocument.spreadsheetml.sheet, \
                 application/vnd.ms-excel."
            )
        })?;
    match format {
        FileFormat::Csv => parse_csv_bytes(bytes, args),
        FileFormat::Xlsx => parse_xlsx_bytes(bytes, args),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Records-extraction path (attachment_run_python, 2026-06-10)
// ─────────────────────────────────────────────────────────────────────────────

/// `(columns, rows)` returned by [`parse_attachment_to_records`]. Each row
/// is a JSON `{column_name: value_or_null}` map ready to feed into
/// `pd.DataFrame(records)` in the Python sandbox.
pub type AttachmentRecords = (Vec<String>, Vec<serde_json::Map<String, serde_json::Value>>);

/// Load every row of a tabular attachment as a `Vec` of `{column: value}`
/// JSON records — the shape pandas accepts via `pd.DataFrame(records)`.
///
/// Used by `attachment_run_python` to inject the full DataFrame into the
/// Python sandbox in one shot (the LLM's code then runs against `df`).
///
/// Returns `Err` if:
/// - bytes exceed `MAX_ATTACHMENT_BYTES` (50 MB)
/// - row count exceeds `MAX_BULK_INSERT_ROWS` (100 K) — the helper would
///   otherwise stream all 1M+ rows into Python memory unbounded
/// - mime is not CSV/XLSX (caller falls back to the LLM-error envelope)
///
/// Values are emitted as JSON strings (CSV native shape); pandas re-infers
/// types on `pd.DataFrame(records)`. Empty cells become `null`.
pub fn parse_attachment_to_records(
    bytes: &[u8],
    mime_type: &str,
    filename: &str,
    delimiter: Option<&str>,
    sheet_name: Option<&str>,
    header_row: Option<u32>,
) -> Result<AttachmentRecords, String> {
    if bytes.len() as u64 > MAX_ATTACHMENT_BYTES {
        return Err(format!(
            "attachment too large: {} bytes > limit {} MB",
            bytes.len(),
            MAX_ATTACHMENT_BYTES / 1024 / 1024
        ));
    }
    let format = FileFormat::from_mime(mime_type)
        .or_else(|| FileFormat::from_filename(filename))
        .ok_or_else(|| {
            format!(
                "unsupported mime_type '{mime_type}' (filename '{filename}') for run_python. \
                 Supported: text/csv, application/csv, text/plain (.csv), \
                 application/vnd.openxmlformats-officedocument.spreadsheetml.sheet, \
                 application/vnd.ms-excel."
            )
        })?;
    match format {
        FileFormat::Csv => parse_csv_to_records(bytes, delimiter, header_row),
        FileFormat::Xlsx => parse_xlsx_to_records(bytes, sheet_name, header_row),
    }
}

fn parse_csv_to_records(
    bytes: &[u8],
    delimiter: Option<&str>,
    header_row: Option<u32>,
) -> Result<AttachmentRecords, String> {
    let header_row = header_row.unwrap_or(DEFAULT_HEADER_ROW).max(1) as usize;
    let delimiter_byte = match delimiter {
        Some(s) => {
            let trimmed = s.trim();
            if trimmed.len() != 1 {
                return Err(format!(
                    "delimiter must be a single character, got '{trimmed}'"
                ));
            }
            trimmed.bytes().next().unwrap()
        }
        None => {
            let first_line = std::str::from_utf8(bytes)
                .map_err(|e| format!("CSV bytes are not valid UTF-8: {e}"))?
                .lines()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("");
            detect_csv_delimiter(first_line)
        }
    };
    let mut rdr = csv::ReaderBuilder::new()
        .delimiter(delimiter_byte)
        .has_headers(false)
        .flexible(true)
        .from_reader(bytes);
    let mut iter = rdr.records();
    for _ in 1..header_row {
        if iter.next().is_none() {
            return Err(format!("CSV ended before reaching header_row={header_row}"));
        }
    }
    let header_record = iter
        .next()
        .ok_or_else(|| "CSV is empty — no header row".to_string())?
        .map_err(|e| format!("CSV header parse error: {e}"))?;
    let columns: Vec<String> = header_record.iter().map(|s| s.to_string()).collect();
    if columns.is_empty() {
        return Err("CSV header has zero columns".into());
    }

    let mut records: Vec<serde_json::Map<String, serde_json::Value>> = Vec::new();
    for rec in iter {
        let rec =
            rec.map_err(|e| format!("CSV row parse error at row {}: {e}", records.len() + 1))?;
        if records.len() as u64 >= MAX_BULK_INSERT_ROWS {
            return Err(format!(
                "CSV exceeds {MAX_BULK_INSERT_ROWS} rows — run_python cannot load the full DataFrame. \
                 Use sql_bulk_insert_from_attachment for large files, or filter the data before upload."
            ));
        }
        let values: Vec<String> = rec.iter().map(|s| s.to_string()).collect();
        let mut row = serde_json::Map::new();
        for (i, col) in columns.iter().enumerate() {
            let v = values.get(i).cloned().unwrap_or_default();
            let json_v = if v.is_empty() {
                serde_json::Value::Null
            } else {
                serde_json::Value::String(v)
            };
            row.insert(col.clone(), json_v);
        }
        records.push(row);
    }
    Ok((columns, records))
}

fn parse_xlsx_to_records(
    bytes: &[u8],
    sheet_name: Option<&str>,
    header_row: Option<u32>,
) -> Result<AttachmentRecords, String> {
    use calamine::{open_workbook_from_rs, Reader, Xlsx};
    use std::io::Cursor;

    let header_row = header_row.unwrap_or(DEFAULT_HEADER_ROW).max(1) as usize;
    let mut wb: Xlsx<_> = open_workbook_from_rs(Cursor::new(bytes.to_vec()))
        .map_err(|e| format!("XLSX parse error: {e}"))?;
    let sheet_names = wb.sheet_names();
    if sheet_names.is_empty() {
        return Err("XLSX has no sheets".into());
    }
    let chosen_sheet = match sheet_name {
        Some(name) => {
            if !sheet_names.iter().any(|s| s == name) {
                return Err(format!(
                    "sheet '{name}' not found. Available: {sheet_names:?}"
                ));
            }
            name.to_string()
        }
        None => sheet_names[0].clone(),
    };
    let range = wb
        .worksheet_range(&chosen_sheet)
        .map_err(|e| format!("XLSX worksheet read error for '{chosen_sheet}': {e}"))?;

    let height = range.height();
    if height < header_row {
        return Err(format!(
            "XLSX sheet '{chosen_sheet}' has only {height} rows; header_row={header_row} is out of range"
        ));
    }

    let header_row_idx = header_row - 1;
    let columns: Vec<String> = (0..range.width())
        .map(|c| {
            range
                .get((header_row_idx, c))
                .map(cell_to_string)
                .unwrap_or_default()
        })
        .collect();
    if columns.iter().all(|c| c.is_empty()) {
        return Err(format!(
            "XLSX header row {header_row} on sheet '{chosen_sheet}' has no column names"
        ));
    }

    let data_row_count = height - header_row;
    if data_row_count as u64 > MAX_BULK_INSERT_ROWS {
        return Err(format!(
            "XLSX exceeds {MAX_BULK_INSERT_ROWS} rows — run_python cannot load the full DataFrame. \
             Filter the data before upload."
        ));
    }
    let mut records: Vec<serde_json::Map<String, serde_json::Value>> =
        Vec::with_capacity(data_row_count);
    for r_offset in 0..data_row_count {
        let r = header_row + r_offset;
        let mut row = serde_json::Map::new();
        for (i, col) in columns.iter().enumerate() {
            let raw = range.get((r, i)).map(cell_to_string).unwrap_or_default();
            let v = if raw.is_empty() {
                serde_json::Value::Null
            } else {
                serde_json::Value::String(raw)
            };
            row.insert(col.clone(), v);
        }
        records.push(row);
    }
    Ok((columns, records))
}

/// Detect the CSV delimiter by sniffing the first non-empty line. Tries
/// comma, semicolon, tab in order; returns the one that produces the most
/// fields. Defaults to comma when the line has no separator candidates.
fn detect_csv_delimiter(first_line: &str) -> u8 {
    let candidates = [b',', b';', b'\t'];
    let counts: Vec<(u8, usize)> = candidates
        .iter()
        .map(|&c| (c, first_line.bytes().filter(|b| *b == c).count()))
        .collect();
    counts
        .iter()
        .max_by_key(|(_, n)| *n)
        .filter(|(_, n)| *n > 0)
        .map(|(c, _)| *c)
        .unwrap_or(b',')
}

/// CSV branch of [`parse_inspect_bytes`]. Streams the file via the `csv`
/// crate so total-row counting does not require buffering all rows.
fn parse_csv_bytes(bytes: &[u8], args: &InspectArgs) -> Result<InspectResponse, String> {
    let sample_target = args
        .sample_rows
        .unwrap_or(DEFAULT_INSPECT_SAMPLE_ROWS)
        .min(MAX_INSPECT_SAMPLE_ROWS) as usize;
    let header_row = args.header_row.unwrap_or(DEFAULT_HEADER_ROW).max(1) as usize;

    // Resolve delimiter: explicit override > sniff first non-empty line.
    let delimiter_byte = match args.delimiter.as_deref() {
        Some(s) => {
            let trimmed = s.trim();
            if trimmed.len() != 1 {
                return Err(format!(
                    "delimiter must be a single character, got '{trimmed}'"
                ));
            }
            trimmed.bytes().next().unwrap()
        }
        None => {
            // First non-empty line.
            let first_line = std::str::from_utf8(bytes)
                .map_err(|e| format!("CSV bytes are not valid UTF-8: {e}"))?
                .lines()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("");
            detect_csv_delimiter(first_line)
        }
    };
    let delimiter_str = (delimiter_byte as char).to_string();

    let mut rdr = csv::ReaderBuilder::new()
        .delimiter(delimiter_byte)
        .has_headers(false) // we handle the header row manually so header_row > 1 works
        .flexible(true)
        .from_reader(bytes);

    let mut iter = rdr.records();
    // Skip rows BEFORE the header row.
    for _ in 1..header_row {
        if iter.next().is_none() {
            return Err(format!(
                "CSV ended before reaching header_row={header_row}; the file may have fewer rows than expected"
            ));
        }
    }
    // Read the header row.
    let header_record = iter
        .next()
        .ok_or_else(|| "CSV is empty — no header row found".to_string())?
        .map_err(|e| format!("CSV header parse error: {e}"))?;
    let columns: Vec<String> = header_record.iter().map(|s| s.to_string()).collect();
    if columns.is_empty() {
        return Err("CSV header has zero columns".into());
    }

    // Stream remaining rows: collect first N samples + count total.
    let mut sample: Vec<serde_json::Map<String, serde_json::Value>> =
        Vec::with_capacity(sample_target);
    let mut sample_raw: Vec<Vec<String>> = Vec::with_capacity(sample_target);
    let mut total_rows: u64 = 0;
    for rec in iter {
        let rec = rec.map_err(|e| format!("CSV row parse error at row {}: {e}", total_rows + 1))?;
        let values: Vec<String> = rec.iter().map(|s| s.to_string()).collect();
        if sample.len() < sample_target {
            let mut map = serde_json::Map::new();
            for (i, col) in columns.iter().enumerate() {
                let v = values.get(i).cloned().unwrap_or_default();
                let json_v = if v.is_empty() {
                    serde_json::Value::Null
                } else {
                    serde_json::Value::String(v)
                };
                map.insert(col.clone(), json_v);
            }
            sample.push(map);
            sample_raw.push(values);
        }
        total_rows += 1;
    }

    let inferred_types = infer_column_types(&sample_raw, columns.len());

    Ok(InspectResponse {
        columns,
        inferred_types,
        sample,
        total_rows,
        format: FileFormat::Csv,
        delimiter: Some(delimiter_str),
        encoding: Some("utf-8".to_string()),
        sheet_name: None,
        target_table_schema: None,
    })
}

/// XLSX branch of [`parse_inspect_bytes`]. Uses `calamine` to read the
/// requested sheet; total-row count is `worksheet_height - header_row`.
fn parse_xlsx_bytes(bytes: &[u8], args: &InspectArgs) -> Result<InspectResponse, String> {
    use calamine::{open_workbook_from_rs, Data, Reader, Xlsx};
    use std::io::Cursor;

    let sample_target = args
        .sample_rows
        .unwrap_or(DEFAULT_INSPECT_SAMPLE_ROWS)
        .min(MAX_INSPECT_SAMPLE_ROWS) as usize;
    let header_row = args.header_row.unwrap_or(DEFAULT_HEADER_ROW).max(1) as usize;

    let mut wb: Xlsx<_> = open_workbook_from_rs(Cursor::new(bytes.to_vec()))
        .map_err(|e| format!("XLSX parse error: {e}"))?;
    let sheet_names = wb.sheet_names();
    if sheet_names.is_empty() {
        return Err("XLSX has no sheets".into());
    }
    let chosen_sheet = match args.sheet_name.as_deref() {
        Some(name) => {
            if !sheet_names.iter().any(|s| s == name) {
                return Err(format!(
                    "sheet '{name}' not found in workbook. Available sheets: {sheet_names:?}"
                ));
            }
            name.to_string()
        }
        None => sheet_names[0].clone(),
    };
    let range = wb
        .worksheet_range(&chosen_sheet)
        .map_err(|e| format!("XLSX worksheet read error for '{chosen_sheet}': {e}"))?;

    let height = range.height();
    if height < header_row {
        return Err(format!(
            "XLSX sheet '{chosen_sheet}' has only {height} rows; header_row={header_row} is out of range"
        ));
    }

    // Read the header row (0-indexed in calamine; header_row is 1-indexed).
    let header_row_idx = header_row - 1;
    let columns: Vec<String> = (0..range.width())
        .map(|c| {
            range
                .get((header_row_idx, c))
                .map(cell_to_string)
                .unwrap_or_default()
        })
        .collect();
    if columns.is_empty() || columns.iter().all(|c| c.is_empty()) {
        return Err(format!(
            "XLSX header row {header_row} on sheet '{chosen_sheet}' has no column names"
        ));
    }

    let total_rows = (height - header_row) as u64;
    let mut sample = Vec::with_capacity(sample_target);
    let mut sample_raw: Vec<Vec<String>> = Vec::with_capacity(sample_target);
    let data_row_count = std::cmp::min(sample_target, height - header_row);
    for r_offset in 0..data_row_count {
        let r = header_row + r_offset; // 0-indexed in calamine
        let raw_row: Vec<String> = (0..columns.len())
            .map(|c| range.get((r, c)).map(cell_to_string).unwrap_or_default())
            .collect();
        let mut map = serde_json::Map::new();
        for (i, col) in columns.iter().enumerate() {
            let v = raw_row.get(i).cloned().unwrap_or_default();
            let json_v = if v.is_empty() {
                serde_json::Value::Null
            } else {
                serde_json::Value::String(v)
            };
            map.insert(col.clone(), json_v);
        }
        sample.push(map);
        sample_raw.push(raw_row);
    }

    let _ = Data::Empty; // keep `Data` import live across xlsx/calamine API drift
    let inferred_types = infer_column_types(&sample_raw, columns.len());

    Ok(InspectResponse {
        columns,
        inferred_types,
        sample,
        total_rows,
        format: FileFormat::Xlsx,
        delimiter: None,
        encoding: None,
        sheet_name: Some(chosen_sheet),
        target_table_schema: None,
    })
}

/// Convert a `calamine::Data` cell to a string consistently. Empty cells →
/// empty string; numbers preserve fractional digits via `Display`.
fn cell_to_string(cell: &calamine::Data) -> String {
    use calamine::Data;
    match cell {
        Data::Empty => String::new(),
        Data::String(s) => s.clone(),
        Data::Float(f) => {
            // Avoid losing precision for integer-valued floats (1.0 → "1").
            if f.fract() == 0.0 && f.abs() < 1e15 {
                format!("{}", *f as i64)
            } else {
                format!("{f}")
            }
        }
        Data::Int(i) => format!("{i}"),
        Data::Bool(b) => b.to_string(),
        Data::DateTime(dt) => format!("{dt}"),
        Data::DateTimeIso(s) | Data::DurationIso(s) => s.clone(),
        Data::Error(e) => format!("#ERROR({e:?})"),
    }
}

/// Infer per-column types from the sampled rows.
///
/// Algorithm: for each column, walk all sampled values. Track which
/// type-categories each non-empty value satisfies. The final type is the
/// most-specific category that matched **every** non-empty sample.
/// Ordering of specificity (narrow → broad):
///
///   1. `Bool` (exactly "true"/"false", case-insensitive)
///   2. `Integer` (`^-?\d+$`)
///   3. `Numeric` (`^-?\d+\.\d+$` OR plain integer — superset of Integer)
///   4. `Date` (`YYYY-MM-DD`)
///   5. `Timestamp` (`YYYY-MM-DDTHH:MM(:SS)(...)`)
///   6. `Text` (default — anything that didn't match a stricter type)
///
/// Columns whose every sampled value was empty default to `Text` (safer
/// than `Unknown` because Postgres accepts text into most columns).
pub fn infer_column_types(sample_raw: &[Vec<String>], col_count: usize) -> Vec<InferredType> {
    let mut out = Vec::with_capacity(col_count);
    for c in 0..col_count {
        let values: Vec<&str> = sample_raw
            .iter()
            .filter_map(|row| row.get(c).map(|s| s.as_str()))
            .filter(|s| !s.is_empty())
            .collect();
        if values.is_empty() {
            out.push(InferredType::Text);
            continue;
        }
        let all_bool = values
            .iter()
            .all(|v| matches!(v.to_ascii_lowercase().as_str(), "true" | "false"));
        if all_bool {
            out.push(InferredType::Bool);
            continue;
        }
        let all_int = values.iter().all(|v| {
            let trimmed = v.trim_start_matches('-');
            !trimmed.is_empty() && trimmed.bytes().all(|b| b.is_ascii_digit())
        });
        if all_int {
            out.push(InferredType::Integer);
            continue;
        }
        let all_numeric = values.iter().all(|v| is_numeric(v));
        if all_numeric {
            out.push(InferredType::Numeric);
            continue;
        }
        let all_timestamp = values.iter().all(|v| is_timestamp(v));
        if all_timestamp {
            out.push(InferredType::Timestamp);
            continue;
        }
        let all_date = values.iter().all(|v| is_date(v));
        if all_date {
            out.push(InferredType::Date);
            continue;
        }
        out.push(InferredType::Text);
    }
    out
}

fn is_numeric(v: &str) -> bool {
    // Accept ints and decimals.
    let s = v.trim_start_matches('-');
    if s.is_empty() {
        return false;
    }
    let mut seen_dot = false;
    for b in s.bytes() {
        match b {
            b'0'..=b'9' => {}
            b'.' if !seen_dot => seen_dot = true,
            _ => return false,
        }
    }
    true
}

fn is_date(v: &str) -> bool {
    // YYYY-MM-DD strict.
    if v.len() != 10 {
        return false;
    }
    let bytes = v.as_bytes();
    bytes[..4].iter().all(|b| b.is_ascii_digit())
        && bytes[4] == b'-'
        && bytes[5..7].iter().all(|b| b.is_ascii_digit())
        && bytes[7] == b'-'
        && bytes[8..10].iter().all(|b| b.is_ascii_digit())
}

// ─────────────────────────────────────────────────────────────────────────────
// SQL side — target_table_schema lookup + Postgres COPY bulk insert (T3)
// ─────────────────────────────────────────────────────────────────────────────

/// Parse a `schema.table` (or `table` — defaults to `public`) into its parts.
/// The dispatcher uses the schema part to validate against `allowed_schemas`.
pub fn split_qualified_table(full: &str) -> (String, String) {
    let trimmed = full.trim();
    if let Some((schema, table)) = trimmed.split_once('.') {
        (schema.to_string(), table.to_string())
    } else {
        ("public".to_string(), trimmed.to_string())
    }
}

/// Validate that the target table lives under a permitted schema.
/// Returns the (schema, table) tuple on success.
pub fn validate_table_against_allowlist(
    full_table: &str,
    allowed_schemas: &[String],
) -> Result<(String, String), String> {
    let (schema, table) = split_qualified_table(full_table);
    if allowed_schemas.is_empty() {
        return Err(
            "permissions.allowed_schemas is empty — cannot bulk-insert anywhere. \
             Operator must configure allowed_schemas in the tool's fixed_config."
                .to_string(),
        );
    }
    if !allowed_schemas.iter().any(|s| s == &schema) {
        return Err(format!(
            "schema '{schema}' is not in allowed_schemas {allowed_schemas:?}. \
             Either pick a table in a permitted schema, or ask the operator \
             to widen allowed_schemas."
        ));
    }
    if table.is_empty() {
        return Err("table name is empty".into());
    }
    // Defensive: identifier sanity — only [A-Za-z0-9_].
    let ok = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');
    if !ok(&schema) || !ok(&table) {
        return Err(format!(
            "schema/table identifier contains invalid characters: schema='{schema}', table='{table}'. \
             Allowed: A-Z, a-z, 0-9, underscore."
        ));
    }
    Ok((schema, table))
}

/// Build a short-lived `PgPool` for ad-hoc SQL calls from the synthetic
/// tools. Single connection (no pooling reuse across calls) — these tools
/// are not on the hot path of the agent loop so the constant ~10ms connect
/// overhead is acceptable.
async fn build_short_lived_pool(connection_url: &str) -> Result<sqlx::PgPool, String> {
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(std::time::Duration::from_secs(5))
        .connect(connection_url)
        .await
        .map_err(|e| format!("Postgres connect failed: {e}"))
}

/// Fetch the destination table's column list + SQL types from
/// `information_schema.columns`. Used by `sql_inspect_attachment` when the
/// LLM passes `target_table` — saves an extra round-trip vs the LLM having
/// to issue its own `sql_query` against `information_schema`.
pub async fn fetch_target_table_schema(
    connection_url: &str,
    full_table: &str,
    allowed_schemas: &[String],
) -> Result<TargetTableSchema, String> {
    let (schema, table) = validate_table_against_allowlist(full_table, allowed_schemas)?;
    let pool = build_short_lived_pool(connection_url).await?;
    let rows = sqlx::query_as::<_, (String, String, String, Option<String>)>(
        r#"SELECT column_name::text, data_type::text, is_nullable::text, column_default::text
           FROM information_schema.columns
           WHERE table_schema = $1 AND table_name = $2
           ORDER BY ordinal_position"#,
    )
    .bind(&schema)
    .bind(&table)
    .fetch_all(&pool)
    .await
    .map_err(|e| format!("information_schema query failed: {e}"))?;
    if rows.is_empty() {
        return Err(format!(
            "table '{schema}.{table}' has no columns in information_schema — \
             does it exist? Use sql_query to introspect."
        ));
    }
    let columns = rows
        .into_iter()
        .map(
            |(name, data_type, is_nullable, column_default)| TargetTableColumn {
                name,
                data_type,
                is_nullable: is_nullable.eq_ignore_ascii_case("YES"),
                column_default,
            },
        )
        .collect();
    Ok(TargetTableSchema {
        table: format!("{schema}.{table}"),
        columns,
    })
}

/// Quote a Postgres identifier safely. The values we receive come from
/// `validate_table_against_allowlist` (alphanumeric + underscore only), so
/// double-quoting is sufficient — but we still defensively escape embedded
/// `"` so a future widening of the allowlist regex doesn't introduce
/// injection.
fn quote_ident(ident: &str) -> String {
    format!("\"{}\"", ident.replace('"', "\"\""))
}

/// Execute the bulk insert: stream `csv_bytes` into Postgres via
/// `COPY <schema>.<table>(<dest_cols>) FROM STDIN CSV HEADER`.
///
/// Only [`OnErrorPolicy::FailFast`] is supported in v1 — `COPY` is atomic
/// by design, so any row failure rolls back the whole batch. `SkipRows`
/// and `PartialCommit` require row-by-row INSERT and are tracked for v1.1.
pub async fn execute_bulk_insert_csv_postgres_copy(
    connection_url: &str,
    args: &BulkInsertArgs,
    allowed_schemas: &[String],
    csv_bytes: &[u8],
) -> Result<BulkInsertResponse, String> {
    if !matches!(args.on_error, OnErrorPolicy::FailFast) {
        return Err(format!(
            "on_error='{}' is not yet supported by sql_bulk_insert_from_attachment v1. \
             Only 'fail_fast' (default) is implemented. SkipRows / PartialCommit are \
             tracked for v1.1.",
            serde_json::to_string(&args.on_error)
                .unwrap_or_default()
                .trim_matches('"')
        ));
    }
    if args.column_mapping.is_empty() {
        return Err("column_mapping is empty — the LLM must explicitly map at least one source column to a destination column.".into());
    }
    if csv_bytes.is_empty() {
        return Err("attachment is empty — nothing to insert.".into());
    }
    if csv_bytes.len() as u64 > MAX_ATTACHMENT_BYTES {
        return Err(format!(
            "attachment too large: {} bytes > limit {} MB",
            csv_bytes.len(),
            MAX_ATTACHMENT_BYTES / 1024 / 1024
        ));
    }

    let (schema, table) = validate_table_against_allowlist(&args.table, allowed_schemas)?;

    // Build destination column list in source-file order. The COPY command's
    // column list MUST match the order of fields in the streamed CSV. Since
    // column_mapping is a BTreeMap (ordered by source key) the iteration is
    // deterministic but in *alphabetical* order of source keys — not the
    // order they appear in the CSV. We must reorder using the CSV header.
    // For v1 we require the LLM to have provided a mapping for every CSV
    // column that exists in the file; we extract the CSV header to do the
    // ordering, and we project just the columns mentioned in the mapping.
    let csv_header_line = std::str::from_utf8(csv_bytes)
        .map_err(|e| format!("CSV bytes are not valid UTF-8: {e}"))?
        .lines()
        .find(|l| !l.trim().is_empty())
        .ok_or_else(|| "CSV is empty — no header row".to_string())?;
    let delimiter_byte = match args.delimiter.as_deref() {
        Some(s) if s.len() == 1 => s.bytes().next().unwrap(),
        Some(s) => {
            return Err(format!("delimiter must be a single character, got '{s}'"));
        }
        None => detect_csv_delimiter(csv_header_line),
    };
    let header_cols: Vec<String> = {
        let mut rdr = csv::ReaderBuilder::new()
            .delimiter(delimiter_byte)
            .has_headers(false)
            .flexible(true)
            .from_reader(csv_header_line.as_bytes());
        rdr.records()
            .next()
            .transpose()
            .map_err(|e| format!("CSV header parse error: {e}"))?
            .ok_or_else(|| "CSV header missing".to_string())?
            .iter()
            .map(|s| s.to_string())
            .collect()
    };

    // For each CSV column, find the destination column from the mapping.
    // The mapping must cover every CSV column whose name appears in the file
    // header — extra mapping entries are tolerated (and ignored) so the LLM
    // can pass a superset.
    let mut dest_cols: Vec<String> = Vec::with_capacity(header_cols.len());
    for src in &header_cols {
        match args.column_mapping.get(src) {
            Some(dest) => dest_cols.push(dest.clone()),
            None => {
                return Err(format!(
                    "column_mapping is missing source column '{src}' (present in CSV header). \
                     Add an entry for it or remove the column from the CSV before insert."
                ));
            }
        }
    }
    let dest_cols_sql = dest_cols
        .iter()
        .map(|c| quote_ident(c))
        .collect::<Vec<_>>()
        .join(", ");
    let copy_sql = format!(
        "COPY {qschema}.{qtable} ({cols}) FROM STDIN WITH (FORMAT csv, HEADER true)",
        qschema = quote_ident(&schema),
        qtable = quote_ident(&table),
        cols = dest_cols_sql,
    );

    let pool = build_short_lived_pool(connection_url).await?;
    let mut conn = pool
        .acquire()
        .await
        .map_err(|e| format!("Postgres acquire failed: {e}"))?;

    let started = std::time::Instant::now();
    // BEGIN / COMMIT explicit so we can rollback on COPY failure cleanly.
    sqlx::query("BEGIN")
        .execute(&mut *conn)
        .await
        .map_err(|e| format!("BEGIN failed: {e}"))?;
    let mut copy = conn
        .copy_in_raw(&copy_sql)
        .await
        .map_err(|e| format!("COPY init failed: {e}"))?;
    if let Err(e) = copy.send(csv_bytes).await {
        let _ = copy.abort(format!("send failed: {e}")).await;
        return Err(format!("COPY send failed: {e}"));
    }
    let rows_inserted = match copy.finish().await {
        Ok(n) => n,
        Err(e) => {
            // COPY auto-aborts on finish error; explicitly rollback the TX too.
            let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
            return Err(format!("COPY finish failed: {e}"));
        }
    };
    if rows_inserted > MAX_BULK_INSERT_ROWS {
        let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
        return Err(format!(
            "bulk insert produced {rows_inserted} rows > cap {MAX_BULK_INSERT_ROWS}; \
             rolled back. Operator can raise the cap via a v1.1 runtime_limits knob."
        ));
    }
    sqlx::query("COMMIT")
        .execute(&mut *conn)
        .await
        .map_err(|e| format!("COMMIT failed: {e}"))?;
    let duration_ms = started.elapsed().as_millis() as u64;

    Ok(BulkInsertResponse {
        rows_inserted,
        rows_skipped: 0,
        duration_ms,
        method: "copy_from_stdin".into(),
        errors: Vec::new(),
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// LLM-facing tool definitions
// ─────────────────────────────────────────────────────────────────────────────

/// Build the `ToolDefinition` for [`SQL_INSPECT_ATTACHMENT_TOOL_NAME`].
/// The schema is derived from `InspectArgs` via `schemars` so the LLM sees
/// the exact accepted argument shape with descriptions inline.
pub fn build_sql_inspect_attachment_tool_definition() -> crate::llm::domain::tools::ToolDefinition {
    super::build_synthetic_tool_with_summary::<InspectArgs>(
        SQL_INSPECT_ATTACHMENT_TOOL_NAME,
        crate::text::tool_description(SQL_INSPECT_ATTACHMENT_TOOL_NAME),
        crate::text::tool_summary(SQL_INSPECT_ATTACHMENT_TOOL_NAME),
    )
}

/// Build the `ToolDefinition` for [`SQL_BULK_INSERT_TOOL_NAME`].
pub fn build_sql_bulk_insert_tool_definition() -> crate::llm::domain::tools::ToolDefinition {
    super::build_synthetic_tool_with_summary::<BulkInsertArgs>(
        SQL_BULK_INSERT_TOOL_NAME,
        crate::text::tool_description(SQL_BULK_INSERT_TOOL_NAME),
        crate::text::tool_summary(SQL_BULK_INSERT_TOOL_NAME),
    )
}

// ─────────────────────────────────────────────────────────────────────────────
// Dispatchers (T4) — bridge from DagToolExecutor::execute to the pure logic
// ─────────────────────────────────────────────────────────────────────────────

use crate::llm::domain::{LlmError, ToolCall, ToolResult};
use std::collections::HashMap as StdHashMap;

/// Resolve `${ENV_VAR}` placeholders in a string. Mirrors the behavior of
/// `SqlNode::resolve_env_vars` so the synthetic SQL tools accept the same
/// `connection_url: "${DATABASE_URL}"` pattern operators already use for
/// the `sql_query` node.
pub(super) fn resolve_env_vars(input: &str) -> Result<String, String> {
    let mut result = String::new();
    let mut last_end = 0;
    while let Some(start) = input[last_end..].find("${") {
        let absolute_start = last_end + start;
        result.push_str(&input[last_end..absolute_start]);
        if let Some(end) = input[absolute_start..].find('}') {
            let absolute_end = absolute_start + end;
            let var_name = &input[absolute_start + 2..absolute_end];
            let val =
                std::env::var(var_name).map_err(|_| format!("Env var {var_name} not found"))?;
            result.push_str(&val);
            last_end = absolute_end + 1;
        } else {
            result.push_str(&input[absolute_start..]);
            break;
        }
    }
    result.push_str(&input[last_end..]);
    Ok(result)
}

/// Extract `connection_url` (required) and `permissions.allowed_schemas`
/// (required) from a tool's `fixed_config`. Surfaces a structured error if
/// either is missing so the operator sees a clear "tool not configured"
/// signal in the LLM trace.
fn extract_connection_config(
    fixed_config: &StdHashMap<String, serde_json::Value>,
) -> Result<(String, Vec<String>), String> {
    let raw_connection_url = fixed_config
        .get("connection_url")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            "tool fixed_config is missing 'connection_url'. \
             Operator must set tool_configurations.<tool>.fixed_config.connection_url \
             to a Postgres connection string."
                .to_string()
        })?;
    // Resolve ${ENV_VAR} placeholders so operators can pass
    // "connection_url": "${DATABASE_URL}" (same pattern as sql_query node).
    let connection_url = resolve_env_vars(raw_connection_url)
        .map_err(|e| format!("connection_url env resolution failed: {e}"))?;
    let allowed_schemas = fixed_config
        .get("permissions")
        .and_then(|p| p.get("allowed_schemas"))
        .and_then(|s| s.as_array())
        .ok_or_else(|| {
            "tool fixed_config is missing 'permissions.allowed_schemas'. \
             Operator must set a non-empty allowed_schemas array."
                .to_string()
        })?
        .iter()
        .filter_map(|v| v.as_str().map(|s| s.to_string()))
        .collect::<Vec<_>>();
    if allowed_schemas.is_empty() {
        return Err(
            "permissions.allowed_schemas is empty — at least one schema must be permitted."
                .to_string(),
        );
    }
    Ok((connection_url, allowed_schemas))
}

/// Wrap an `Err(String)` into the tool error envelope the LLM expects:
/// `{"error": "...", "source": "execution"}`. Mirrors the shape the existing
/// `sql_query` node uses so the LLM can apply the same recovery logic
/// documented in the `error_recovery` skill reference.
///
/// We use `ToolResult::success` (not `failure`) deliberately: `failure`
/// emits `output: ""` which breaks Gemini's "messages cannot be empty"
/// validation on the next turn. By returning the error envelope as a
/// successful-shape output, the LLM sees the error verbatim in the tool
/// response and can apply recovery logic.
fn err_envelope(call_id: &str, msg: String) -> ToolResult {
    let v = serde_json::json!({ "error": msg, "source": "execution" });
    ToolResult::success(call_id.to_string(), v.to_string())
}

/// Dispatcher for `sql_inspect_attachment`. Called from
/// `DagToolExecutor::execute` via the trampoline in this module.
pub async fn dispatch_sql_inspect_attachment_via_executor(
    executor: &crate::dag_engine::infrastructure::dag_tool_executor::DagToolExecutor,
    tool_call: &ToolCall,
    fixed_config: &StdHashMap<String, serde_json::Value>,
) -> Result<ToolResult, LlmError> {
    let call_id = &tool_call.id;

    // Parse args from the LLM.
    let args: InspectArgs = match serde_json::from_str(&tool_call.function.arguments) {
        Ok(a) => a,
        Err(e) => {
            return Ok(err_envelope(
                call_id,
                format!("failed to parse sql_inspect_attachment args: {e}"),
            ))
        }
    };

    // Fetch attachment bytes via the shared plumbing.
    let stored = match executor.fetch_attachment_bytes(&args.attachment_id).await {
        Ok(b) => b,
        Err(e) => return Ok(err_envelope(call_id, e)),
    };

    // Prefer mime/filename from the catalog (authoritative) over the storage
    // adapter's reported values — local storage strips the original mime
    // (defaults to application/octet-stream) and renames to a UUID-based
    // storage_key. The catalog row preserves what the user uploaded.
    let (effective_mime, effective_filename) = executor
        .lookup_attachment_meta(&args.attachment_id)
        .unwrap_or_else(|| (stored.mime_type.clone(), stored.filename.clone()));

    // Parse the bytes (pure). With the catalog mime + filename, this resolves
    // correctly even when storage strips the original metadata.
    let mut response = match parse_inspect_bytes_with_filename(
        &stored.bytes,
        &effective_mime,
        &effective_filename,
        &args,
    ) {
        Ok(r) => r,
        Err(e) => return Ok(err_envelope(call_id, e)),
    };

    // Optional target_table_schema lookup. Only fires when the LLM opted in.
    if let Some(target_table) = &args.target_table {
        match extract_connection_config(fixed_config) {
            Ok((conn_url, allowed)) => {
                match fetch_target_table_schema(&conn_url, target_table, &allowed).await {
                    Ok(s) => response.target_table_schema = Some(s),
                    Err(e) => {
                        return Ok(err_envelope(
                            call_id,
                            format!("target_table lookup failed: {e}"),
                        ))
                    }
                }
            }
            Err(e) => {
                // No connection configured — the user wanted target_table but
                // we can't reach the DB. Surface the config gap clearly.
                return Ok(err_envelope(
                    call_id,
                    format!("cannot fetch target_table schema: {e}"),
                ));
            }
        }
    }

    let v = serde_json::to_value(&response).map_err(|e| LlmError::ToolExecutionFailed {
        message: format!("serialize inspect response: {e}"),
    })?;
    Ok(ToolResult::success(call_id.to_string(), v.to_string()))
}

/// Dispatcher for `sql_bulk_insert_from_attachment`.
pub async fn dispatch_sql_bulk_insert_from_attachment_via_executor(
    executor: &crate::dag_engine::infrastructure::dag_tool_executor::DagToolExecutor,
    tool_call: &ToolCall,
    fixed_config: &StdHashMap<String, serde_json::Value>,
) -> Result<ToolResult, LlmError> {
    let call_id = &tool_call.id;

    let args: BulkInsertArgs = match serde_json::from_str(&tool_call.function.arguments) {
        Ok(a) => a,
        Err(e) => {
            return Ok(err_envelope(
                call_id,
                format!("failed to parse sql_bulk_insert_from_attachment args: {e}"),
            ))
        }
    };

    let (connection_url, allowed_schemas) = match extract_connection_config(fixed_config) {
        Ok(t) => t,
        Err(e) => return Ok(err_envelope(call_id, e)),
    };

    // Fetch attachment bytes. v1 buffers the full payload before COPY —
    // streaming via sqlx::copy_in_raw works on `&[u8]` slices but not on
    // a generic `Stream`, so we materialize first. The 50 MB cap keeps RAM
    // bounded.
    let stored = match executor.fetch_attachment_bytes(&args.attachment_id).await {
        Ok(b) => b,
        Err(e) => return Ok(err_envelope(call_id, e)),
    };

    // Prefer catalog meta over storage meta (see inspect dispatcher comment).
    let (effective_mime, effective_filename) = executor
        .lookup_attachment_meta(&args.attachment_id)
        .unwrap_or_else(|| (stored.mime_type.clone(), stored.filename.clone()));

    // Detect format. XLSX bulk insert is deferred to v1.1 (calamine doesn't
    // stream — it materializes the workbook, which couples bytes-per-row to
    // memory unpredictably). Fallback to filename for generic mimes.
    let format = match FileFormat::from_mime(&effective_mime)
        .or_else(|| FileFormat::from_filename(&effective_filename))
    {
        Some(f) => f,
        None => {
            return Ok(err_envelope(
                call_id,
                format!(
                    "unsupported mime_type '{effective_mime}' (filename \
                     '{effective_filename}') for bulk insert. \
                     v1 supports CSV only (text/csv, text/plain, or *.csv). \
                     XLSX is tracked for v1.1."
                ),
            ))
        }
    };
    if !matches!(format, FileFormat::Csv) {
        return Ok(err_envelope(
            call_id,
            "sql_bulk_insert_from_attachment v1 supports CSV only. XLSX bulk insert is \
             tracked for v1.1 — convert your XLSX to CSV before insert."
                .to_string(),
        ));
    }

    match execute_bulk_insert_csv_postgres_copy(
        &connection_url,
        &args,
        &allowed_schemas,
        &stored.bytes,
    )
    .await
    {
        Ok(r) => {
            let v = serde_json::to_value(&r).map_err(|e| LlmError::ToolExecutionFailed {
                message: format!("serialize bulk_insert response: {e}"),
            })?;
            Ok(ToolResult::success(call_id.to_string(), v.to_string()))
        }
        Err(e) => Ok(err_envelope(call_id, e)),
    }
}

fn is_timestamp(v: &str) -> bool {
    // YYYY-MM-DDTHH:MM(:SS)(...) — basic check.
    if v.len() < 16 {
        return false;
    }
    let (date_part, time_part) = v.split_at(10);
    if !is_date(date_part) {
        return false;
    }
    let bytes = time_part.as_bytes();
    if bytes[0] != b'T' && bytes[0] != b' ' {
        return false;
    }
    bytes[1..3].iter().all(|b| b.is_ascii_digit())
        && bytes[3] == b':'
        && bytes[4..6].iter().all(|b| b.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inferred_type_str_roundtrip() {
        for v in [
            InferredType::Integer,
            InferredType::Numeric,
            InferredType::Bool,
            InferredType::Date,
            InferredType::Timestamp,
            InferredType::Text,
        ] {
            let s = v.as_str();
            let back: InferredType = serde_json::from_value(serde_json::json!(s)).unwrap();
            assert_eq!(back, v, "round-trip failed for {s}");
        }
    }

    #[test]
    fn file_format_from_csv_mime() {
        assert_eq!(FileFormat::from_mime("text/csv"), Some(FileFormat::Csv));
        assert_eq!(
            FileFormat::from_mime("application/csv"),
            Some(FileFormat::Csv)
        );
        assert_eq!(FileFormat::from_mime("text/plain"), Some(FileFormat::Csv));
        // Case-insensitive.
        assert_eq!(FileFormat::from_mime("Text/CSV"), Some(FileFormat::Csv));
    }

    #[test]
    fn file_format_from_xlsx_mime() {
        assert_eq!(
            FileFormat::from_mime(
                "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
            ),
            Some(FileFormat::Xlsx)
        );
        assert_eq!(
            FileFormat::from_mime("application/vnd.ms-excel"),
            Some(FileFormat::Xlsx)
        );
    }

    #[test]
    fn file_format_from_unsupported_mime() {
        assert_eq!(FileFormat::from_mime("application/pdf"), None);
        assert_eq!(FileFormat::from_mime("image/png"), None);
        assert_eq!(FileFormat::from_mime(""), None);
    }

    #[test]
    fn on_error_default_is_fail_fast() {
        let p = OnErrorPolicy::default();
        assert_eq!(p, OnErrorPolicy::FailFast);
        // Wire format: snake_case.
        let s = serde_json::to_string(&p).unwrap();
        assert_eq!(s, "\"fail_fast\"");
    }

    #[test]
    fn inspect_args_minimal_deserialize() {
        // The LLM may pass only `attachment_id` — every other field is
        // optional with a sensible default.
        let args: InspectArgs = serde_json::from_str(r#"{"attachment_id":"doc-x"}"#).unwrap();
        assert_eq!(args.attachment_id, "doc-x");
        assert!(args.sample_rows.is_none());
        assert!(args.delimiter.is_none());
        assert!(args.target_table.is_none());
    }

    #[test]
    fn inspect_args_with_target_table_deserialize() {
        let body = r#"{
            "attachment_id":"doc-x",
            "sample_rows":7,
            "target_table":"public.products"
        }"#;
        let args: InspectArgs = serde_json::from_str(body).unwrap();
        assert_eq!(args.sample_rows, Some(7));
        assert_eq!(args.target_table.as_deref(), Some("public.products"));
    }

    #[test]
    fn bulk_insert_args_require_column_mapping() {
        // column_mapping is a non-optional BTreeMap, so omitting it must
        // cause deserialization to fail.
        let body = r#"{"attachment_id":"doc-x","table":"public.t"}"#;
        let err = serde_json::from_str::<BulkInsertArgs>(body).unwrap_err();
        assert!(
            err.to_string().contains("column_mapping"),
            "expected error to mention missing column_mapping, got: {err}"
        );
    }

    #[test]
    fn bulk_insert_args_minimal_with_mapping_ok() {
        let body = r#"{
            "attachment_id":"doc-x",
            "table":"public.products",
            "column_mapping":{"product_id":"id","name":"name"}
        }"#;
        let args: BulkInsertArgs = serde_json::from_str(body).unwrap();
        assert_eq!(args.table, "public.products");
        assert_eq!(args.column_mapping.len(), 2);
        // Default on_error is FailFast.
        assert_eq!(args.on_error, OnErrorPolicy::FailFast);
    }

    #[test]
    fn bulk_insert_response_serializes_method_and_counts() {
        let r = BulkInsertResponse {
            rows_inserted: 1487,
            rows_skipped: 0,
            duration_ms: 230,
            method: "copy_from_stdin".into(),
            errors: Vec::new(),
        };
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["rows_inserted"], 1487);
        assert_eq!(v["method"], "copy_from_stdin");
        // Empty `errors` should serialize as an empty array (not omitted)
        // so the LLM sees a stable shape across runs.
        assert!(v["errors"].is_array() && v["errors"].as_array().unwrap().is_empty());
    }

    // ─────────────────────────────────────────────────────────────────────
    // CSV / XLSX parsing tests (T2)
    // ─────────────────────────────────────────────────────────────────────

    fn args_default(id: &str) -> InspectArgs {
        InspectArgs {
            attachment_id: id.into(),
            sample_rows: None,
            delimiter: None,
            sheet_name: None,
            header_row: None,
            target_table: None,
        }
    }

    #[test]
    fn csv_parses_simple_3_column_4_row_fixture() {
        let csv = b"product_id,sku,price\n1,A001,9.99\n2,A002,14.50\n3,A003,7.25\n4,A004,21.00\n";
        let r = parse_inspect_bytes(csv, "text/csv", &args_default("x")).unwrap();
        assert_eq!(r.columns, vec!["product_id", "sku", "price"]);
        assert_eq!(r.total_rows, 4);
        assert_eq!(r.format, FileFormat::Csv);
        assert_eq!(r.delimiter.as_deref(), Some(","));
        assert_eq!(r.encoding.as_deref(), Some("utf-8"));
        // Default sample_rows = 5 → all 4 rows fit.
        assert_eq!(r.sample.len(), 4);
        assert_eq!(r.sample[0]["product_id"], "1");
        assert_eq!(r.sample[0]["sku"], "A001");
    }

    #[test]
    fn csv_sample_rows_clamps_to_max() {
        // 25 data rows > MAX_INSPECT_SAMPLE_ROWS (20), so the clamp truly bites.
        let mut csv = String::from("a,b\n");
        for i in 1..=25 {
            csv.push_str(&format!("{i},x\n"));
        }
        let mut args = args_default("x");
        args.sample_rows = Some(99); // would-be huge → clamp to MAX
        let r = parse_inspect_bytes(csv.as_bytes(), "text/csv", &args).unwrap();
        assert_eq!(r.sample.len(), MAX_INSPECT_SAMPLE_ROWS as usize);
        assert_eq!(r.total_rows, 25);
    }

    #[test]
    fn csv_explicit_delimiter_semicolon_works() {
        let csv = b"a;b;c\n1;2;3\n";
        let mut args = args_default("x");
        args.delimiter = Some(";".into());
        let r = parse_inspect_bytes(csv, "text/csv", &args).unwrap();
        assert_eq!(r.columns, vec!["a", "b", "c"]);
        assert_eq!(r.delimiter.as_deref(), Some(";"));
    }

    #[test]
    fn csv_delimiter_must_be_single_char() {
        let csv = b"a,b\n1,2\n";
        let mut args = args_default("x");
        args.delimiter = Some("||".into());
        let err = parse_inspect_bytes(csv, "text/csv", &args).unwrap_err();
        assert!(err.contains("single character"));
    }

    #[test]
    fn csv_auto_detects_tab_delimiter() {
        let csv = b"a\tb\tc\n1\t2\t3\n";
        let r = parse_inspect_bytes(csv, "text/csv", &args_default("x")).unwrap();
        assert_eq!(r.columns, vec!["a", "b", "c"]);
        assert_eq!(r.delimiter.as_deref(), Some("\t"));
    }

    #[test]
    fn csv_header_row_2_skips_title_row() {
        let csv = b"Reporte Q3 2026,,\nproduct_id,sku,price\n1,A001,9.99\n";
        let mut args = args_default("x");
        args.header_row = Some(2);
        let r = parse_inspect_bytes(csv, "text/csv", &args).unwrap();
        assert_eq!(r.columns, vec!["product_id", "sku", "price"]);
        assert_eq!(r.total_rows, 1);
    }

    #[test]
    fn csv_rejects_empty_input() {
        let err = parse_inspect_bytes(b"", "text/csv", &args_default("x")).unwrap_err();
        assert!(err.contains("empty"));
    }

    #[test]
    fn parse_rejects_unsupported_mime() {
        let err = parse_inspect_bytes(b"data", "application/pdf", &args_default("x")).unwrap_err();
        assert!(err.contains("unsupported mime_type"));
    }

    #[test]
    fn parse_rejects_oversized_payload() {
        // Forge a buffer that *says* it's larger than the cap without actually
        // allocating 51 MB — by passing a thin slice but checking the length
        // path. We simulate via a Vec<u8> resized to just over the limit.
        let big = vec![b'a'; (MAX_ATTACHMENT_BYTES + 1) as usize];
        let err = parse_inspect_bytes(&big, "text/csv", &args_default("x")).unwrap_err();
        assert!(err.contains("attachment too large"));
    }

    #[test]
    fn empty_cell_becomes_json_null_in_sample() {
        let csv = b"a,b,c\n1,,3\n";
        let r = parse_inspect_bytes(csv, "text/csv", &args_default("x")).unwrap();
        assert_eq!(r.sample[0]["a"], "1");
        assert_eq!(r.sample[0]["b"], serde_json::Value::Null);
        assert_eq!(r.sample[0]["c"], "3");
    }

    // ─────────────────────────────────────────────────────────────────────
    // Type inference tests
    // ─────────────────────────────────────────────────────────────────────

    fn rows(v: Vec<Vec<&str>>) -> Vec<Vec<String>> {
        v.into_iter()
            .map(|row| row.into_iter().map(|s| s.to_string()).collect())
            .collect()
    }

    #[test]
    fn infer_all_integer_column() {
        let r = rows(vec![vec!["1"], vec!["42"], vec!["-7"]]);
        assert_eq!(infer_column_types(&r, 1), vec![InferredType::Integer]);
    }

    #[test]
    fn infer_numeric_column_when_any_decimal() {
        let r = rows(vec![vec!["1.0"], vec!["42"], vec!["3.14"]]);
        assert_eq!(infer_column_types(&r, 1), vec![InferredType::Numeric]);
    }

    #[test]
    fn infer_bool_column() {
        let r = rows(vec![vec!["true"], vec!["false"], vec!["TRUE"]]);
        assert_eq!(infer_column_types(&r, 1), vec![InferredType::Bool]);
    }

    #[test]
    fn infer_date_column() {
        let r = rows(vec![vec!["2026-01-15"], vec!["2026-06-09"]]);
        assert_eq!(infer_column_types(&r, 1), vec![InferredType::Date]);
    }

    #[test]
    fn infer_timestamp_column() {
        let r = rows(vec![
            vec!["2026-01-15T10:23:00Z"],
            vec!["2026-06-09T14:00:00+00:00"],
        ]);
        assert_eq!(infer_column_types(&r, 1), vec![InferredType::Timestamp]);
    }

    #[test]
    fn infer_text_when_mixed() {
        let r = rows(vec![vec!["1"], vec!["abc"]]);
        assert_eq!(infer_column_types(&r, 1), vec![InferredType::Text]);
    }

    #[test]
    fn infer_text_when_all_empty() {
        let r = rows(vec![vec![""], vec![""]]);
        assert_eq!(infer_column_types(&r, 1), vec![InferredType::Text]);
    }

    #[test]
    fn infer_multi_column_independence() {
        let r = rows(vec![
            vec!["1", "true", "2026-01-15", "hello"],
            vec!["2", "false", "2026-06-09", "world"],
        ]);
        assert_eq!(
            infer_column_types(&r, 4),
            vec![
                InferredType::Integer,
                InferredType::Bool,
                InferredType::Date,
                InferredType::Text,
            ]
        );
    }

    #[test]
    fn infer_empty_values_dont_invalidate_other_rows() {
        // Column where some rows are empty but all non-empty are integers.
        let r = rows(vec![vec!["1"], vec![""], vec!["42"]]);
        assert_eq!(infer_column_types(&r, 1), vec![InferredType::Integer]);
    }

    // ─────────────────────────────────────────────────────────────────────
    // Tabular auto-summary tests (option B, 2026-06-10)
    // ─────────────────────────────────────────────────────────────────────

    #[test]
    fn tabular_summary_for_simple_csv_includes_header_types_and_sample() {
        let csv = b"product_id,sku,price\n1,A001,9.99\n2,A002,14.50\n3,A003,7.25\n4,A004,21.00\n";
        let s = build_tabular_summary("text/csv", "products.csv", csv).unwrap();
        // Header line: format + dims + delimiter.
        assert!(s.contains("CSV, 3 cols × 4 rows"), "missing dims, got: {s}");
        assert!(s.contains("delimiter ','"), "missing delimiter, got: {s}");
        // Schema line: columns with inferred types.
        assert!(
            s.contains("product_id (integer)"),
            "missing product_id type"
        );
        assert!(s.contains("sku (text)"), "missing sku type");
        assert!(s.contains("price (numeric)"), "missing price type");
        // Sample lines: at most DEFAULT_TABULAR_SUMMARY_SAMPLE_ROWS rows.
        let sample_lines: Vec<&str> = s
            .lines()
            .filter(|l| {
                l.trim_start().starts_with('1')
                    || l.trim_start().starts_with('2')
                    || l.trim_start().starts_with('3')
            })
            .collect();
        assert_eq!(
            sample_lines.len(),
            DEFAULT_TABULAR_SUMMARY_SAMPLE_ROWS as usize
        );
    }

    #[test]
    fn tabular_summary_returns_none_for_non_tabular_mime() {
        let pdf_like = b"%PDF-1.4\nfake pdf content\n";
        assert!(build_tabular_summary("application/pdf", "doc.pdf", pdf_like).is_none());
    }

    #[test]
    fn tabular_summary_returns_none_for_plain_text_mime_without_csv_extension() {
        // text/plain alone is ambiguous; we don't want every .txt to look like CSV.
        // No .csv/.xlsx extension either → must skip.
        let text = b"this is just\nsome text\nwithout commas\n";
        assert!(build_tabular_summary("text/plain", "readme.txt", text).is_none());
    }

    #[test]
    fn tabular_summary_fires_when_text_plain_but_filename_says_csv() {
        // Common: signed-URL providers send text/plain for .csv. We rely on
        // the filename extension to still recognise it.
        let csv = b"a,b\n1,2\n3,4\n";
        let s = build_tabular_summary("text/plain", "data.csv", csv);
        assert!(s.is_some(), "should resolve via .csv extension");
        assert!(s.unwrap().contains("CSV"));
    }

    #[test]
    fn tabular_summary_returns_none_for_oversized_payload() {
        let too_big = vec![b'a'; (MAX_ATTACHMENT_BYTES + 1) as usize];
        assert!(build_tabular_summary("text/csv", "huge.csv", &too_big).is_none());
    }

    #[test]
    fn tabular_summary_truncates_very_long_cells() {
        // A cell of 200 chars should be truncated to ~40 in the rendered sample.
        let long_cell = "x".repeat(200);
        let csv = format!("name,desc\nrow1,{long_cell}\n");
        let s = build_tabular_summary("text/csv", "data.csv", csv.as_bytes()).unwrap();
        // Long string should be truncated with `...` marker.
        assert!(s.contains("..."));
        // And the original 200-char run must not survive verbatim.
        assert!(!s.contains(&"x".repeat(100)));
    }

    #[test]
    fn tabular_summary_handles_csv_with_only_header() {
        // 0 data rows is a degenerate but valid case. Summary should render
        // `0 rows` without panicking and without producing "sample rows:" garbage.
        let csv = b"a,b,c\n";
        let s = build_tabular_summary("text/csv", "empty.csv", csv).unwrap();
        assert!(s.contains("3 cols × 0 rows"));
    }

    // ─────────────────────────────────────────────────────────────────────
    // T3 — pure validation tests (no DB)
    // ─────────────────────────────────────────────────────────────────────

    #[test]
    fn split_qualified_table_with_schema() {
        assert_eq!(
            split_qualified_table("reporting.sales"),
            ("reporting".into(), "sales".into())
        );
    }

    #[test]
    fn split_qualified_table_without_schema_defaults_to_public() {
        assert_eq!(
            split_qualified_table("products"),
            ("public".into(), "products".into())
        );
    }

    #[test]
    fn validate_table_rejects_schema_not_in_allowlist() {
        let allowed = vec!["public".to_string()];
        let err = validate_table_against_allowlist("secrets.users", &allowed).unwrap_err();
        assert!(err.contains("not in allowed_schemas"));
    }

    #[test]
    fn validate_table_rejects_empty_allowlist() {
        let err = validate_table_against_allowlist("public.products", &[]).unwrap_err();
        assert!(err.contains("allowed_schemas is empty"));
    }

    #[test]
    fn validate_table_accepts_default_public() {
        let allowed = vec!["public".to_string()];
        let ok = validate_table_against_allowlist("products", &allowed).unwrap();
        assert_eq!(ok, ("public".into(), "products".into()));
    }

    #[test]
    fn validate_table_rejects_special_chars_in_identifier() {
        let allowed = vec!["public".to_string()];
        let err = validate_table_against_allowlist("public.tab\"le", &allowed).unwrap_err();
        assert!(err.contains("invalid characters"));
    }

    #[test]
    fn quote_ident_doubles_embedded_quotes() {
        assert_eq!(quote_ident(r#"col"with"quotes"#), r#""col""with""quotes""#);
    }

    #[tokio::test]
    async fn bulk_insert_rejects_unsupported_on_error_policy() {
        // SkipRows / PartialCommit are v1.1; v1 must surface a clear error.
        let mut args: BulkInsertArgs = serde_json::from_str(
            r#"{"attachment_id":"x","table":"public.t","column_mapping":{"a":"a"}}"#,
        )
        .unwrap();
        args.on_error = OnErrorPolicy::SkipRows;
        let err = execute_bulk_insert_csv_postgres_copy(
            "postgres://invalid", // won't even attempt to connect; error fires first
            &args,
            &["public".to_string()],
            b"a\n1\n",
        )
        .await
        .unwrap_err();
        assert!(err.contains("is not yet supported"));
    }

    #[tokio::test]
    async fn bulk_insert_rejects_missing_csv_column_from_mapping() {
        // CSV header has columns [a, b] but mapping only covers [a]. v1
        // requires explicit mapping for every CSV column.
        let args: BulkInsertArgs = serde_json::from_str(
            r#"{"attachment_id":"x","table":"public.t","column_mapping":{"a":"a"}}"#,
        )
        .unwrap();
        let err = execute_bulk_insert_csv_postgres_copy(
            "postgres://invalid",
            &args,
            &["public".to_string()],
            b"a,b\n1,2\n",
        )
        .await
        .unwrap_err();
        assert!(err.contains("missing source column"));
    }

    // ─────────────────────────────────────────────────────────────────────
    // T3 — DB integration tests (#[ignore]-gated by TEST_DATABASE_URL)
    // ─────────────────────────────────────────────────────────────────────

    fn test_db_url() -> Option<String> {
        std::env::var("TEST_DATABASE_URL").ok()
    }

    fn unique_table_suffix() -> String {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::SeqCst);
        format!("{}_{}", std::process::id(), n)
    }

    async fn setup_table(url: &str, suffix: &str) -> String {
        // Two separate execute() calls because sqlx uses the extended Postgres
        // protocol which accepts only one statement per call (same root cause
        // as SQL T1 / Política C — see CHANGELOG §18).
        let pool = build_short_lived_pool(url).await.unwrap();
        let table = format!("test_bulk_{suffix}");
        sqlx::query(&format!("DROP TABLE IF EXISTS public.{table}"))
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(&format!(
            "CREATE TABLE public.{table} (
                 id INTEGER PRIMARY KEY,
                 sku TEXT NOT NULL,
                 price NUMERIC(10,2) NOT NULL
             )"
        ))
        .execute(&pool)
        .await
        .unwrap();
        table
    }

    async fn drop_table(url: &str, table: &str) {
        let pool = build_short_lived_pool(url).await.unwrap();
        let _ = sqlx::query(&format!("DROP TABLE IF EXISTS public.{table};"))
            .execute(&pool)
            .await;
    }

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL — run with `cargo test -- --ignored`"]
    async fn bulk_t3_inserts_3_rows_via_copy_happy_path() {
        let Some(url) = test_db_url() else {
            return;
        };
        let suffix = unique_table_suffix();
        let table = setup_table(&url, &suffix).await;
        let csv = b"id,sku,price\n1,A001,9.99\n2,A002,14.50\n3,A003,7.25\n";
        let args: BulkInsertArgs = serde_json::from_str(&format!(
            r#"{{
                "attachment_id":"any",
                "table":"public.{table}",
                "column_mapping":{{"id":"id","sku":"sku","price":"price"}}
            }}"#
        ))
        .unwrap();
        let resp = execute_bulk_insert_csv_postgres_copy(&url, &args, &["public".to_string()], csv)
            .await
            .unwrap();
        assert_eq!(resp.rows_inserted, 3);
        assert_eq!(resp.rows_skipped, 0);
        assert_eq!(resp.method, "copy_from_stdin");
        assert!(resp.errors.is_empty());
        drop_table(&url, &table).await;
    }

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL — run with `cargo test -- --ignored`"]
    async fn bulk_t3_renames_columns_via_mapping() {
        let Some(url) = test_db_url() else {
            return;
        };
        let suffix = unique_table_suffix();
        let table = setup_table(&url, &suffix).await;
        // CSV uses different column names: product_id → id, code → sku, cost → price.
        let csv = b"product_id,code,cost\n10,X100,1.50\n20,X200,2.50\n";
        let args: BulkInsertArgs = serde_json::from_str(&format!(
            r#"{{
                "attachment_id":"any",
                "table":"public.{table}",
                "column_mapping":{{"product_id":"id","code":"sku","cost":"price"}}
            }}"#
        ))
        .unwrap();
        let resp = execute_bulk_insert_csv_postgres_copy(&url, &args, &["public".to_string()], csv)
            .await
            .unwrap();
        assert_eq!(resp.rows_inserted, 2);
        drop_table(&url, &table).await;
    }

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL — run with `cargo test -- --ignored`"]
    async fn bulk_t3_rollback_on_constraint_violation_in_fail_fast() {
        let Some(url) = test_db_url() else {
            return;
        };
        let suffix = unique_table_suffix();
        let table = setup_table(&url, &suffix).await;
        // Row 3 has duplicate PK with row 1 → must rollback the whole batch.
        let csv = b"id,sku,price\n1,A001,9.99\n2,A002,14.50\n1,A003,7.25\n";
        let args: BulkInsertArgs = serde_json::from_str(&format!(
            r#"{{
                "attachment_id":"any",
                "table":"public.{table}",
                "column_mapping":{{"id":"id","sku":"sku","price":"price"}}
            }}"#
        ))
        .unwrap();
        let err = execute_bulk_insert_csv_postgres_copy(&url, &args, &["public".to_string()], csv)
            .await
            .unwrap_err();
        assert!(err.contains("COPY"));
        // Verify nothing committed.
        let pool = build_short_lived_pool(&url).await.unwrap();
        let n: (i64,) = sqlx::query_as(&format!("SELECT COUNT(*) FROM public.{table}"))
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n.0, 0, "rollback failed — table not empty");
        drop_table(&url, &table).await;
    }

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL — run with `cargo test -- --ignored`"]
    async fn bulk_t3_target_table_schema_lookup_returns_columns() {
        let Some(url) = test_db_url() else {
            return;
        };
        let suffix = unique_table_suffix();
        let table = setup_table(&url, &suffix).await;
        let schema =
            fetch_target_table_schema(&url, &format!("public.{table}"), &["public".to_string()])
                .await
                .unwrap();
        assert_eq!(schema.table, format!("public.{table}"));
        assert_eq!(schema.columns.len(), 3);
        let names: Vec<&str> = schema.columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["id", "sku", "price"]);
        let id_col = &schema.columns[0];
        assert_eq!(id_col.data_type, "integer");
        assert!(!id_col.is_nullable);
        drop_table(&url, &table).await;
    }
}
