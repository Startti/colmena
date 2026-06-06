//! Google Sheets synthetic LLM tools (Subsystem E).
//!
//! 9 tools mirroring the shape of `crdt_doc_*` so skills transfer:
//! - gsheets_create_spreadsheet
//! - gsheets_create_from_xlsx
//! - gsheets_export_xlsx
//! - gsheets_list_sheets
//! - gsheets_add_sheet
//! - gsheets_delete_sheet
//! - gsheets_read
//! - gsheets_set_cell
//! - gsheets_set_range
//!
//! Each dispatcher builds a `GoogleSheetsHttpClient` from env config and
//! delegates to the [`SheetsClient`] trait. Errors are mapped to JSON
//! shapes the agent can pattern-match on.
//!
//! UX aliases (per D-T16 lessons): `address` ↔ `addr`, `start` ↔ `start_addr`,
//! `values` ↔ `values_2d`, `name` ↔ `sheet`. Single-A1 ranges auto-expanded.

use crate::gsheets::domain::{
    CellValue, ReadOptions, SheetsClient, SheetsError, SpreadsheetId, ValueRenderOption,
};
use crate::gsheets::infrastructure::config::GSheetsConfig;
use crate::gsheets::infrastructure::http_client::GoogleSheetsHttpClient;
use crate::llm::domain::tools::ToolDefinition;
use crate::text;
use schemars::JsonSchema;
use serde::Deserialize;

pub const TOOL_CREATE_SPREADSHEET: &str = "gsheets_create_spreadsheet";
pub const TOOL_CREATE_FROM_XLSX: &str = "gsheets_create_from_xlsx";
pub const TOOL_EXPORT_XLSX: &str = "gsheets_export_xlsx";
pub const TOOL_LIST_SHEETS: &str = "gsheets_list_sheets";
pub const TOOL_ADD_SHEET: &str = "gsheets_add_sheet";
pub const TOOL_DELETE_SHEET: &str = "gsheets_delete_sheet";
pub const TOOL_READ: &str = "gsheets_read";
pub const TOOL_SET_CELL: &str = "gsheets_set_cell";
pub const TOOL_SET_RANGE: &str = "gsheets_set_range";

// ── Args structs ──────────────────────────────────────────────────────

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CreateSpreadsheetArgs {
    pub title: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CreateFromXlsxArgs {
    pub attachment_id: String,
    pub title: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ExportXlsxArgs {
    pub spreadsheet_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListSheetsArgs {
    pub spreadsheet_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AddSheetArgs {
    pub spreadsheet_id: String,
    pub name: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DeleteSheetArgs {
    pub spreadsheet_id: String,
    #[serde(alias = "name")]
    pub sheet: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReadArgs {
    pub spreadsheet_id: String,
    pub sheet: String,
    pub range: Option<String>,
    /// "FORMULA" | "UNFORMATTED_VALUE" | "FORMATTED_VALUE". Default
    /// UNFORMATTED_VALUE (pandas-friendly scalars).
    pub value_render: Option<String>,
    #[serde(default)]
    pub as_records: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SetCellArgs {
    pub spreadsheet_id: String,
    pub sheet: String,
    #[serde(alias = "address")]
    pub addr: String,
    pub value: serde_json::Value,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SetRangeArgs {
    pub spreadsheet_id: String,
    pub sheet: String,
    #[serde(alias = "start")]
    pub start_addr: String,
    #[serde(alias = "values")]
    pub values_2d: Vec<Vec<serde_json::Value>>,
}

// ── Shared helpers ────────────────────────────────────────────────────

/// Map a `SheetsError` to a tool-result JSON value with a stable shape.
pub(crate) fn error_to_json(e: SheetsError) -> serde_json::Value {
    let (kind, message) = match &e {
        SheetsError::NotConfigured(m) => ("gsheets_not_configured", m.clone()),
        SheetsError::AuthFailed(m) => ("auth_failed", m.clone()),
        SheetsError::SpreadsheetNotFound(s) => ("spreadsheet_not_found", s.clone()),
        SheetsError::SheetNotFound(s) => ("sheet_not_found", s.clone()),
        SheetsError::InvalidRange(m) => ("invalid_range", m.clone()),
        SheetsError::PermissionDenied(sa) => ("permission_denied", sa.clone()),
        SheetsError::RateLimit(s) => ("rate_limit", format!("retry after {s}s")),
        SheetsError::Http(m) => ("http_error", m.clone()),
        SheetsError::Internal(m) => ("internal", m.clone()),
    };
    serde_json::json!({"error": kind, "message": message})
}

fn build_client() -> Result<GoogleSheetsHttpClient, serde_json::Value> {
    GoogleSheetsHttpClient::from_config(&GSheetsConfig::from_env()).map_err(error_to_json)
}

fn parse_value_render(s: Option<&str>) -> ValueRenderOption {
    match s.unwrap_or("UNFORMATTED_VALUE") {
        "FORMULA" => ValueRenderOption::Formula,
        "FORMATTED_VALUE" => ValueRenderOption::FormattedValue,
        _ => ValueRenderOption::UnformattedValue,
    }
}

// ── Tool definitions ──────────────────────────────────────────────────

pub fn tool_create_spreadsheet() -> ToolDefinition {
    super::build_synthetic_tool_with_summary::<CreateSpreadsheetArgs>(
        TOOL_CREATE_SPREADSHEET,
        text::tool_description(TOOL_CREATE_SPREADSHEET),
        text::tool_summary(TOOL_CREATE_SPREADSHEET),
    )
}

pub fn tool_create_from_xlsx() -> ToolDefinition {
    super::build_synthetic_tool_with_summary::<CreateFromXlsxArgs>(
        TOOL_CREATE_FROM_XLSX,
        text::tool_description(TOOL_CREATE_FROM_XLSX),
        text::tool_summary(TOOL_CREATE_FROM_XLSX),
    )
}

pub fn tool_export_xlsx() -> ToolDefinition {
    super::build_synthetic_tool_with_summary::<ExportXlsxArgs>(
        TOOL_EXPORT_XLSX,
        text::tool_description(TOOL_EXPORT_XLSX),
        text::tool_summary(TOOL_EXPORT_XLSX),
    )
}

pub fn tool_list_sheets() -> ToolDefinition {
    super::build_synthetic_tool_with_summary::<ListSheetsArgs>(
        TOOL_LIST_SHEETS,
        text::tool_description(TOOL_LIST_SHEETS),
        text::tool_summary(TOOL_LIST_SHEETS),
    )
}

pub fn tool_add_sheet() -> ToolDefinition {
    super::build_synthetic_tool_with_summary::<AddSheetArgs>(
        TOOL_ADD_SHEET,
        text::tool_description(TOOL_ADD_SHEET),
        text::tool_summary(TOOL_ADD_SHEET),
    )
}

pub fn tool_delete_sheet() -> ToolDefinition {
    super::build_synthetic_tool_with_summary::<DeleteSheetArgs>(
        TOOL_DELETE_SHEET,
        text::tool_description(TOOL_DELETE_SHEET),
        text::tool_summary(TOOL_DELETE_SHEET),
    )
}

pub fn tool_read() -> ToolDefinition {
    super::build_synthetic_tool_with_summary::<ReadArgs>(
        TOOL_READ,
        text::tool_description(TOOL_READ),
        text::tool_summary(TOOL_READ),
    )
}

pub fn tool_set_cell() -> ToolDefinition {
    super::build_synthetic_tool_with_summary::<SetCellArgs>(
        TOOL_SET_CELL,
        text::tool_description(TOOL_SET_CELL),
        text::tool_summary(TOOL_SET_CELL),
    )
}

pub fn tool_set_range() -> ToolDefinition {
    super::build_synthetic_tool_with_summary::<SetRangeArgs>(
        TOOL_SET_RANGE,
        text::tool_description(TOOL_SET_RANGE),
        text::tool_summary(TOOL_SET_RANGE),
    )
}

// ── Dispatchers (with-client variants for testability) ────────────────

pub async fn dispatch_list_sheets_with_client(
    args: serde_json::Value,
    client: &dyn SheetsClient,
) -> serde_json::Value {
    let parsed: ListSheetsArgs = match serde_json::from_value(args) {
        Ok(a) => a,
        Err(e) => return serde_json::json!({"error": "invalid_args", "message": e.to_string()}),
    };
    match client
        .list_sheets(&SpreadsheetId(parsed.spreadsheet_id))
        .await
    {
        Ok(sheets) => serde_json::json!({
            "ok": true,
            "sheets": sheets.iter().map(|s| serde_json::json!({
                "sheet_id": s.sheet_id.0,
                "title": s.title,
                "index": s.index,
                "row_count": s.row_count,
                "col_count": s.col_count,
            })).collect::<Vec<_>>(),
        }),
        Err(e) => error_to_json(e),
    }
}

pub async fn dispatch_list_sheets(args: serde_json::Value) -> serde_json::Value {
    let client = match build_client() {
        Ok(c) => c,
        Err(e) => return e,
    };
    dispatch_list_sheets_with_client(args, &client).await
}

pub async fn dispatch_create_spreadsheet_with_client(
    args: serde_json::Value,
    client: &dyn SheetsClient,
) -> serde_json::Value {
    let parsed: CreateSpreadsheetArgs = match serde_json::from_value(args) {
        Ok(a) => a,
        Err(e) => return serde_json::json!({"error": "invalid_args", "message": e.to_string()}),
    };
    match client.create_spreadsheet(&parsed.title).await {
        Ok(meta) => serde_json::json!({
            "ok": true,
            "spreadsheet_id": meta.spreadsheet_id.0,
            "url": meta.url,
            "title": meta.title,
            "sheets": meta.sheets.iter().map(|s| serde_json::json!({
                "sheet_id": s.sheet_id.0,
                "title": s.title,
                "index": s.index,
                "row_count": s.row_count,
                "col_count": s.col_count,
            })).collect::<Vec<_>>(),
        }),
        Err(e) => error_to_json(e),
    }
}

pub async fn dispatch_create_spreadsheet(args: serde_json::Value) -> serde_json::Value {
    let client = match build_client() {
        Ok(c) => c,
        Err(e) => return e,
    };
    dispatch_create_spreadsheet_with_client(args, &client).await
}

pub async fn dispatch_add_sheet_with_client(
    args: serde_json::Value,
    client: &dyn SheetsClient,
) -> serde_json::Value {
    let parsed: AddSheetArgs = match serde_json::from_value(args) {
        Ok(a) => a,
        Err(e) => return serde_json::json!({"error": "invalid_args", "message": e.to_string()}),
    };
    match client
        .add_sheet(&SpreadsheetId(parsed.spreadsheet_id), &parsed.name)
        .await
    {
        Ok(meta) => serde_json::json!({
            "ok": true,
            "sheet_id": meta.sheet_id.0,
            "title": meta.title,
        }),
        Err(e) => error_to_json(e),
    }
}

pub async fn dispatch_add_sheet(args: serde_json::Value) -> serde_json::Value {
    let client = match build_client() {
        Ok(c) => c,
        Err(e) => return e,
    };
    dispatch_add_sheet_with_client(args, &client).await
}

pub async fn dispatch_delete_sheet_with_client(
    args: serde_json::Value,
    client: &dyn SheetsClient,
) -> serde_json::Value {
    let parsed: DeleteSheetArgs = match serde_json::from_value(args) {
        Ok(a) => a,
        Err(e) => return serde_json::json!({"error": "invalid_args", "message": e.to_string()}),
    };
    match client
        .delete_sheet(&SpreadsheetId(parsed.spreadsheet_id), &parsed.sheet)
        .await
    {
        Ok(_) => serde_json::json!({"ok": true}),
        Err(e) => error_to_json(e),
    }
}

pub async fn dispatch_delete_sheet(args: serde_json::Value) -> serde_json::Value {
    let client = match build_client() {
        Ok(c) => c,
        Err(e) => return e,
    };
    dispatch_delete_sheet_with_client(args, &client).await
}

pub async fn dispatch_read_with_client(
    args: serde_json::Value,
    client: &dyn SheetsClient,
) -> serde_json::Value {
    let parsed: ReadArgs = match serde_json::from_value(args) {
        Ok(a) => a,
        Err(e) => return serde_json::json!({"error": "invalid_args", "message": e.to_string()}),
    };
    // Auto-expand single A1 cell to A1:A1 (UX alias per D-T16).
    let range = parsed.range.as_deref().map(|r| {
        if !r.contains(':') {
            format!("{r}:{r}")
        } else {
            r.to_string()
        }
    });
    let opts = ReadOptions {
        value_render: parse_value_render(parsed.value_render.as_deref()),
        as_records: parsed.as_records,
    };
    match client
        .read_range(
            &SpreadsheetId(parsed.spreadsheet_id),
            &parsed.sheet,
            range.as_deref(),
            opts,
        )
        .await
    {
        Ok(r) => serde_json::json!({
            "ok": true,
            "sheet": r.sheet,
            "range": r.range,
            "values": r.values,
        }),
        Err(e) => error_to_json(e),
    }
}

pub async fn dispatch_read(args: serde_json::Value) -> serde_json::Value {
    let client = match build_client() {
        Ok(c) => c,
        Err(e) => return e,
    };
    dispatch_read_with_client(args, &client).await
}

pub async fn dispatch_set_cell_with_client(
    args: serde_json::Value,
    client: &dyn SheetsClient,
) -> serde_json::Value {
    let parsed: SetCellArgs = match serde_json::from_value(args) {
        Ok(a) => a,
        Err(e) => return serde_json::json!({"error": "invalid_args", "message": e.to_string()}),
    };
    match client
        .set_cell(
            &SpreadsheetId(parsed.spreadsheet_id),
            &parsed.sheet,
            &parsed.addr,
            CellValue::from_json(&parsed.value),
        )
        .await
    {
        Ok(_) => serde_json::json!({"ok": true}),
        Err(e) => error_to_json(e),
    }
}

pub async fn dispatch_set_cell(args: serde_json::Value) -> serde_json::Value {
    let client = match build_client() {
        Ok(c) => c,
        Err(e) => return e,
    };
    dispatch_set_cell_with_client(args, &client).await
}

pub async fn dispatch_set_range_with_client(
    args: serde_json::Value,
    client: &dyn SheetsClient,
) -> serde_json::Value {
    let parsed: SetRangeArgs = match serde_json::from_value(args) {
        Ok(a) => a,
        Err(e) => return serde_json::json!({"error": "invalid_args", "message": e.to_string()}),
    };
    let cells: Vec<Vec<CellValue>> = parsed
        .values_2d
        .iter()
        .map(|row| row.iter().map(CellValue::from_json).collect())
        .collect();
    match client
        .set_range(
            &SpreadsheetId(parsed.spreadsheet_id),
            &parsed.sheet,
            &parsed.start_addr,
            cells,
        )
        .await
    {
        Ok(resp) => serde_json::json!({
            "ok": true,
            "updated_cells": resp.updated_cells,
            "updated_range": resp.updated_range,
        }),
        Err(e) => error_to_json(e),
    }
}

pub async fn dispatch_set_range(args: serde_json::Value) -> serde_json::Value {
    let client = match build_client() {
        Ok(c) => c,
        Err(e) => return e,
    };
    dispatch_set_range_with_client(args, &client).await
}

// Note: create_from_xlsx + export_xlsx need attachment plumbing — wired
// in E-T7 (router) where the attachment resolver lives.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gsheets::domain::{
        ReadResponse, SetRangeResponse, SheetId, SheetMeta, SpreadsheetMeta,
    };
    use async_trait::async_trait;

    struct FakeClient;

    #[async_trait]
    impl SheetsClient for FakeClient {
        async fn list_sheets(&self, _id: &SpreadsheetId) -> Result<Vec<SheetMeta>, SheetsError> {
            Ok(vec![SheetMeta {
                sheet_id: SheetId(42),
                title: "Numbers".into(),
                index: 0,
                row_count: 100,
                col_count: 26,
            }])
        }
        async fn create_spreadsheet(&self, _title: &str) -> Result<SpreadsheetMeta, SheetsError> {
            Err(SheetsError::Internal("not used".into()))
        }
        async fn create_from_xlsx(
            &self,
            _t: &str,
            _b: Vec<u8>,
        ) -> Result<SpreadsheetMeta, SheetsError> {
            Err(SheetsError::Internal("not used".into()))
        }
        async fn export_xlsx(&self, _id: &SpreadsheetId) -> Result<Vec<u8>, SheetsError> {
            Err(SheetsError::Internal("not used".into()))
        }
        async fn add_sheet(&self, _id: &SpreadsheetId, _n: &str) -> Result<SheetMeta, SheetsError> {
            Err(SheetsError::Internal("not used".into()))
        }
        async fn delete_sheet(&self, _id: &SpreadsheetId, _n: &str) -> Result<(), SheetsError> {
            Err(SheetsError::Internal("not used".into()))
        }
        async fn read_range(
            &self,
            _id: &SpreadsheetId,
            _s: &str,
            _r: Option<&str>,
            _o: ReadOptions,
        ) -> Result<ReadResponse, SheetsError> {
            Err(SheetsError::Internal("not used".into()))
        }
        async fn set_cell(
            &self,
            _id: &SpreadsheetId,
            _s: &str,
            _a: &str,
            _v: CellValue,
        ) -> Result<(), SheetsError> {
            Err(SheetsError::Internal("not used".into()))
        }
        async fn set_range(
            &self,
            _id: &SpreadsheetId,
            _s: &str,
            _a: &str,
            _v: Vec<Vec<CellValue>>,
        ) -> Result<SetRangeResponse, SheetsError> {
            Err(SheetsError::Internal("not used".into()))
        }
    }

    #[tokio::test]
    async fn dispatch_list_sheets_returns_ok_envelope() {
        let result = dispatch_list_sheets_with_client(
            serde_json::json!({"spreadsheet_id": "abc"}),
            &FakeClient,
        )
        .await;
        assert_eq!(result["ok"], true);
        assert_eq!(result["sheets"][0]["title"], "Numbers");
        assert_eq!(result["sheets"][0]["sheet_id"], 42);
    }

    #[tokio::test]
    async fn dispatch_invalid_args_returns_invalid_args_error() {
        let result = dispatch_list_sheets_with_client(serde_json::json!({}), &FakeClient).await;
        assert_eq!(result["error"], "invalid_args");
    }

    #[test]
    fn parse_value_render_defaults_to_unformatted() {
        assert!(matches!(
            parse_value_render(None),
            ValueRenderOption::UnformattedValue
        ));
        assert!(matches!(
            parse_value_render(Some("FORMULA")),
            ValueRenderOption::Formula
        ));
    }

    #[test]
    fn error_to_json_includes_kind_and_message() {
        let v = error_to_json(SheetsError::SpreadsheetNotFound("abc".into()));
        assert_eq!(v["error"], "spreadsheet_not_found");
        assert_eq!(v["message"], "abc");
    }
}
