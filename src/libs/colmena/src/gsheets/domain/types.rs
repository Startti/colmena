//! Value types used across the Sheets API surface.

use serde::{Deserialize, Serialize};

/// Stable Google identifier for a spreadsheet (the part after
/// `/spreadsheets/d/` in a Sheets URL). Treat as opaque.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SpreadsheetId(pub String);

impl std::fmt::Display for SpreadsheetId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Google's internal numeric sheet (tab) identifier. Used by some
/// `batchUpdate` requests; the human-friendly `title` is what most tools
/// take as input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SheetId(pub i64);

/// A cell payload as understood by the Sheets API. Strings that start
/// with `=` and are written with `valueInputOption: "USER_ENTERED"` are
/// evaluated server-side by Google as formulas.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CellValue {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
}

impl CellValue {
    /// Convert from a `serde_json::Value` (what dispatchers receive from
    /// the LLM). Arrays and objects map to `Null` (they aren't valid
    /// scalar cell values).
    pub fn from_json(v: &serde_json::Value) -> Self {
        use serde_json::Value;
        match v {
            Value::Null => Self::Null,
            Value::Bool(b) => Self::Bool(*b),
            Value::Number(n) => Self::Number(n.as_f64().unwrap_or(0.0)),
            Value::String(s) => Self::String(s.clone()),
            Value::Array(_) | Value::Object(_) => Self::Null,
        }
    }

    /// Convert back to a `serde_json::Value` for tool-result payloads
    /// or wire encoding. Inverse of [`Self::from_json`].
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            Self::Null => serde_json::Value::Null,
            Self::Bool(b) => serde_json::Value::Bool(*b),
            Self::Number(n) => serde_json::json!(n),
            Self::String(s) => serde_json::Value::String(s.clone()),
        }
    }
}

/// What kind of value to ask Google for on read. Matches the Sheets API
/// `valueRenderOption` enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ValueRenderOption {
    /// `"1,234.50"` — what the user sees in the cell, locale-formatted.
    FormattedValue,
    /// `1234.5` — raw underlying value. Default for pandas workflows.
    UnformattedValue,
    /// `"=SUM(A1:A10)"` — the formula text, not the result.
    Formula,
}

impl ValueRenderOption {
    /// Wire-format string for the Sheets API `valueRenderOption`
    /// query param. Matches the names Google's REST docs use.
    pub fn as_api_str(self) -> &'static str {
        match self {
            Self::FormattedValue => "FORMATTED_VALUE",
            Self::UnformattedValue => "UNFORMATTED_VALUE",
            Self::Formula => "FORMULA",
        }
    }
}

/// Options passed to a read call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadOptions {
    pub value_render: ValueRenderOption,
    /// When true, the response shape is a list of records keyed by the
    /// first row (treated as headers). When false, a rectangular 2D array.
    pub as_records: bool,
}

impl Default for ReadOptions {
    fn default() -> Self {
        Self {
            value_render: ValueRenderOption::UnformattedValue,
            as_records: false,
        }
    }
}

/// Result of a read call. `values` is always JSON — either a `[[...]]`
/// rectangle (when `as_records=false`) or `[{col: val, ...}]` list of
/// records (when `as_records=true`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadResponse {
    pub sheet: String,
    pub range: String,
    pub values: serde_json::Value,
}

/// Result of `set_range`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetRangeResponse {
    pub updated_cells: u64,
    pub updated_range: String,
}

/// Metadata for one tab inside a spreadsheet.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SheetMeta {
    pub sheet_id: SheetId,
    pub title: String,
    pub index: u32,
    pub row_count: u32,
    pub col_count: u32,
}

/// Metadata for a whole spreadsheet (top-level identifier + tabs).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpreadsheetMeta {
    pub spreadsheet_id: SpreadsheetId,
    pub title: String,
    /// Convenience: `https://docs.google.com/spreadsheets/d/<id>`.
    pub url: String,
    pub sheets: Vec<SheetMeta>,
}

/// One entry in the `list_spreadsheets` response. Like `DocumentListItem`
/// but scoped to spreadsheets. Only the fields the LLM cares about are
/// surfaced; downstream tools that need more metadata call
/// `list_sheets()` or `read_range()`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpreadsheetListItem {
    pub spreadsheet_id: SpreadsheetId,
    pub name: String,
    pub url: String,
    /// RFC 3339 timestamp from Drive (`modifiedTime`).
    pub modified_time: String,
    /// Owner email addresses. Empty for Shared Drive files.
    #[serde(default)]
    pub owners: Vec<String>,
}

/// Result of `list_spreadsheets`. `next_page_token` is `Some(...)` when
/// more results are available — the LLM passes it back as `page_token`
/// in the next call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpreadsheetListResult {
    pub spreadsheets: Vec<SpreadsheetListItem>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_page_token: Option<String>,
}

/// Filter args for `list_spreadsheets`. Symmetric to `DocumentListFilter`
/// in the gdocs subsystem.
#[derive(Debug, Clone, Default)]
pub struct SpreadsheetListFilter<'a> {
    pub query: Option<&'a str>,
    pub parent_folder_id: Option<&'a str>,
    pub modified_after: Option<&'a str>,
    pub limit: Option<u32>,
    pub page_token: Option<&'a str>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn cell_value_round_trips_through_json() {
        let cases = [
            (json!(null), CellValue::Null),
            (json!(true), CellValue::Bool(true)),
            (json!(42.0), CellValue::Number(42.0)),
            (json!("hi"), CellValue::String("hi".into())),
        ];
        for (j, expected) in cases {
            let cv = CellValue::from_json(&j);
            assert_eq!(cv, expected);
            assert_eq!(cv.to_json(), j);
        }
    }

    #[test]
    fn array_and_object_collapse_to_null() {
        assert_eq!(CellValue::from_json(&json!([1, 2])), CellValue::Null);
        assert_eq!(CellValue::from_json(&json!({"a": 1})), CellValue::Null);
    }

    #[test]
    fn value_render_option_api_strings_match_google_docs() {
        assert_eq!(
            ValueRenderOption::FormattedValue.as_api_str(),
            "FORMATTED_VALUE"
        );
        assert_eq!(
            ValueRenderOption::UnformattedValue.as_api_str(),
            "UNFORMATTED_VALUE"
        );
        assert_eq!(ValueRenderOption::Formula.as_api_str(), "FORMULA");
    }
}
