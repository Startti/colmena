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
/// Bundle 2A (2026-06-11): Drive discovery scoped to spreadsheets.
pub const TOOL_LIST_SPREADSHEETS: &str = "gsheets_list_spreadsheets";
/// Bundle 2B (2026-06-11): Drive permissions tools.
pub const TOOL_SHARE: &str = "gsheets_share";
pub const TOOL_LIST_PERMISSIONS: &str = "gsheets_list_permissions";
pub const TOOL_UNSHARE: &str = "gsheets_unshare";
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

/// Bundle 2A (2026-06-11): Args for [`TOOL_LIST_SPREADSHEETS`]. Mirrors
/// the gdocs `ListDocumentsArgs` shape.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListSpreadsheetsArgs {
    /// Substring match against the spreadsheet name. Drive `name contains`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    /// Restrict results to a single Drive folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_folder_id: Option<String>,
    /// RFC 3339 lower bound (`modifiedTime >= ...`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modified_after: Option<String>,
    /// Page size. Default 20, max 100.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// Pagination cursor from a prior call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_token: Option<String>,
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
    /// "markdown" (default) renders a markdown table; "json" returns the
    /// structured `values` array (honoring `as_records`).
    pub format: Option<String>,
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
///
/// `PermissionDenied` is special-cased: instead of a single `message`
/// field, it carries the share email as a structured field AND a `hint`
/// string the LLM can paraphrase to the user. The hint is intentionally
/// directive ("pedile al usuario que verifique...") so models with a
/// terse default reply style still pass the actionable bit through to
/// the user.
pub(crate) fn error_to_json(e: SheetsError) -> serde_json::Value {
    match &e {
        SheetsError::PermissionDenied(share_email) => permission_denied_payload(share_email),
        _ => {
            let (kind, message) = match &e {
                SheetsError::NotConfigured(m) => ("gsheets_not_configured", m.clone()),
                SheetsError::AuthFailed(m) => ("auth_failed", m.clone()),
                SheetsError::SpreadsheetNotFound(s) => ("spreadsheet_not_found", s.clone()),
                SheetsError::SheetNotFound(s) => ("sheet_not_found", s.clone()),
                SheetsError::InvalidRange(m) => ("invalid_range", m.clone()),
                SheetsError::PermissionDenied(_) => unreachable!("handled above"),
                SheetsError::RateLimit(s) => ("rate_limit", format!("retry after {s}s")),
                SheetsError::Http(m) => ("http_error", m.clone()),
                SheetsError::Internal(m) => ("internal", m.clone()),
            };
            serde_json::json!({"error": kind, "message": message})
        }
    }
}

/// Build the `permission_denied` tool result. Pulled into its own
/// helper so the gdocs side can use the same shape and the wording
/// stays in lockstep across subsystems. The hint string is the
/// load-bearing UX: the LLM reads it and translates to the user's
/// chat language.
fn permission_denied_payload(share_email: &str) -> serde_json::Value {
    if share_email.is_empty() {
        // Degraded mode — COLMENA_GOOGLE_SHARE_EMAIL was not set on
        // the worker, so we don't know what email to instruct the
        // user to share with. Surface a generic but still actionable
        // hint that points the LLM to ask the operator.
        return serde_json::json!({
            "error": "permission_denied",
            "share_email": null,
            "hint": "Google devolvió 403. Pedile al usuario que verifique \
                     con el operador del agente cuál es el correo con el \
                     que debe compartir el documento como Editor."
        });
    }
    serde_json::json!({
        "error": "permission_denied",
        "share_email": share_email,
        "hint": format!(
            "Google devolvió 403 — el agent no tiene acceso al sheet. \
             Pedile al usuario que verifique que compartió el documento \
             como Editor (no Viewer) con `{share_email}`. Si dice que sí, \
             pedile que abra el share dialog del sheet y confirme que ese \
             correo aparece listado con permiso de Editor.",
            share_email = share_email
        )
    })
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

/// Render a single cell value for a markdown table cell.
/// null → empty; bool/number → plain repr; string → escape `|` and newlines.
fn md_cell(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Null => String::new(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::String(s) => s.replace('|', "\\|").replace('\n', "<br>"),
        // Arrays/objects shouldn't appear in a flat sheet cell; stringify defensively.
        other => other.to_string().replace('|', "\\|").replace('\n', "<br>"),
    }
}

/// Convert a 2-D values array (`[[..],[..]]`) into a GitHub markdown table.
/// First row is the header. Ragged rows are padded to the max column count.
/// Returns an empty string for an empty range.
fn values_to_markdown(values: &serde_json::Value) -> String {
    let rows = match values.as_array() {
        Some(r) if !r.is_empty() => r,
        _ => return String::new(),
    };
    let width = rows
        .iter()
        .map(|row| row.as_array().map(|a| a.len()).unwrap_or(0))
        .max()
        .unwrap_or(0);
    if width == 0 {
        return String::new();
    }
    let render_row = |row: &serde_json::Value| -> String {
        let cells = row.as_array().cloned().unwrap_or_default();
        let mut out = String::from("|");
        for i in 0..width {
            let cell = cells.get(i).map(md_cell).unwrap_or_default();
            out.push_str(&format!(" {cell} |"));
        }
        out
    };
    let mut lines = Vec::with_capacity(rows.len() + 1);
    lines.push(render_row(&rows[0])); // header
    lines.push(format!("|{}", " --- |".repeat(width))); // separator
    for row in &rows[1..] {
        lines.push(render_row(row));
    }
    lines.join("\n")
}

/// Compute the data extent of a read result: rows = number of returned rows
/// (includes the header row in 2-D mode), columns = max row width. Works for
/// both 2-D arrays (cells) and record arrays (object keys).
fn compute_dimensions(values: &serde_json::Value) -> serde_json::Value {
    let arr = values.as_array();
    let rows = arr.map(|a| a.len()).unwrap_or(0);
    let columns = arr
        .map(|a| {
            a.iter()
                .map(|row| match row {
                    serde_json::Value::Array(r) => r.len(),
                    serde_json::Value::Object(o) => o.len(),
                    _ => 0,
                })
                .max()
                .unwrap_or(0)
        })
        .unwrap_or(0);
    serde_json::json!({ "rows": rows, "columns": columns })
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

pub fn tool_list_spreadsheets() -> ToolDefinition {
    super::build_synthetic_tool_with_summary::<ListSpreadsheetsArgs>(
        TOOL_LIST_SPREADSHEETS,
        text::tool_description(TOOL_LIST_SPREADSHEETS),
        text::tool_summary(TOOL_LIST_SPREADSHEETS),
    )
}

/// Bundle 2B (2026-06-11): Args for [`TOOL_SHARE`].
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ShareArgs {
    pub spreadsheet_id: String,
    pub email: String,
    /// `"reader" | "commenter" | "writer"`.
    pub role: String,
}

pub fn tool_share() -> ToolDefinition {
    super::build_synthetic_tool_with_summary::<ShareArgs>(
        TOOL_SHARE,
        text::tool_description(TOOL_SHARE),
        text::tool_summary(TOOL_SHARE),
    )
}

/// Bundle 2B (2026-06-11): Args for [`TOOL_LIST_PERMISSIONS`].
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListPermissionsArgs {
    pub spreadsheet_id: String,
}

pub fn tool_list_permissions() -> ToolDefinition {
    super::build_synthetic_tool_with_summary::<ListPermissionsArgs>(
        TOOL_LIST_PERMISSIONS,
        text::tool_description(TOOL_LIST_PERMISSIONS),
        text::tool_summary(TOOL_LIST_PERMISSIONS),
    )
}

/// Bundle 2B (2026-06-11): Args for [`TOOL_UNSHARE`].
#[derive(Debug, Deserialize, JsonSchema)]
pub struct UnshareArgs {
    pub spreadsheet_id: String,
    /// Drive's stable permission id (NOT the email). Get it from
    /// `gsheets_list_permissions`.
    pub permission_id: String,
}

pub fn tool_unshare() -> ToolDefinition {
    super::build_synthetic_tool_with_summary::<UnshareArgs>(
        TOOL_UNSHARE,
        text::tool_description(TOOL_UNSHARE),
        text::tool_summary(TOOL_UNSHARE),
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

/// Bundle 2A (2026-06-11) — Drive discovery dispatcher.
pub async fn dispatch_list_spreadsheets_with_client(
    args: serde_json::Value,
    client: &dyn SheetsClient,
) -> serde_json::Value {
    let parsed: ListSpreadsheetsArgs = match serde_json::from_value(args) {
        Ok(a) => a,
        Err(e) => return serde_json::json!({"error": "invalid_args", "message": e.to_string()}),
    };
    let filter = crate::gsheets::domain::types::SpreadsheetListFilter {
        query: parsed.query.as_deref(),
        parent_folder_id: parsed.parent_folder_id.as_deref(),
        modified_after: parsed.modified_after.as_deref(),
        limit: parsed.limit,
        page_token: parsed.page_token.as_deref(),
    };
    match client.list_spreadsheets(&filter).await {
        Ok(res) => serde_json::json!({
            "ok": true,
            "spreadsheets": res.spreadsheets,
            "next_page_token": res.next_page_token,
        }),
        Err(e) => error_to_json(e),
    }
}

pub async fn dispatch_list_spreadsheets(args: serde_json::Value) -> serde_json::Value {
    let client = match build_client() {
        Ok(c) => c,
        Err(e) => return e,
    };
    dispatch_list_spreadsheets_with_client(args, &client).await
}

/// Bundle 2B (2026-06-11) — grant access to a spreadsheet.
pub async fn dispatch_share_with_client(
    args: serde_json::Value,
    client: &dyn SheetsClient,
) -> serde_json::Value {
    let parsed: ShareArgs = match serde_json::from_value(args) {
        Ok(a) => a,
        Err(e) => return serde_json::json!({"error": "invalid_args", "message": e.to_string()}),
    };
    let role = match parsed.role.to_ascii_lowercase().as_str() {
        "reader" => crate::gsheets::domain::types::ShareRole::Reader,
        "commenter" => crate::gsheets::domain::types::ShareRole::Commenter,
        "writer" => crate::gsheets::domain::types::ShareRole::Writer,
        other => {
            return serde_json::json!({
                "error": "invalid_role",
                "message": format!(
                    "role must be one of reader/commenter/writer, got '{other}'"
                ),
            });
        }
    };
    match client
        .share(&SpreadsheetId(parsed.spreadsheet_id), &parsed.email, role)
        .await
    {
        Ok(()) => serde_json::json!({ "ok": true }),
        Err(e) => error_to_json(e),
    }
}

pub async fn dispatch_share(args: serde_json::Value) -> serde_json::Value {
    let client = match build_client() {
        Ok(c) => c,
        Err(e) => return e,
    };
    dispatch_share_with_client(args, &client).await
}

/// Bundle 2B (2026-06-11) — list every Drive permission on a spreadsheet.
pub async fn dispatch_list_permissions_with_client(
    args: serde_json::Value,
    client: &dyn SheetsClient,
) -> serde_json::Value {
    let parsed: ListPermissionsArgs = match serde_json::from_value(args) {
        Ok(a) => a,
        Err(e) => return serde_json::json!({"error": "invalid_args", "message": e.to_string()}),
    };
    match client
        .list_permissions(&SpreadsheetId(parsed.spreadsheet_id))
        .await
    {
        Ok(list) => serde_json::json!({
            "ok": true,
            "permissions": list.permissions,
        }),
        Err(e) => error_to_json(e),
    }
}

pub async fn dispatch_list_permissions(args: serde_json::Value) -> serde_json::Value {
    let client = match build_client() {
        Ok(c) => c,
        Err(e) => return e,
    };
    dispatch_list_permissions_with_client(args, &client).await
}

/// Bundle 2B (2026-06-11) — revoke a permission from a spreadsheet.
pub async fn dispatch_unshare_with_client(
    args: serde_json::Value,
    client: &dyn SheetsClient,
) -> serde_json::Value {
    let parsed: UnshareArgs = match serde_json::from_value(args) {
        Ok(a) => a,
        Err(e) => return serde_json::json!({"error": "invalid_args", "message": e.to_string()}),
    };
    match client
        .delete_permission(&SpreadsheetId(parsed.spreadsheet_id), &parsed.permission_id)
        .await
    {
        Ok(()) => serde_json::json!({ "ok": true }),
        Err(e) => error_to_json(e),
    }
}

pub async fn dispatch_unshare(args: serde_json::Value) -> serde_json::Value {
    let client = match build_client() {
        Ok(c) => c,
        Err(e) => return e,
    };
    dispatch_unshare_with_client(args, &client).await
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
    let want_markdown = parsed.format.as_deref().unwrap_or("markdown") != "json";
    let opts = ReadOptions {
        value_render: parse_value_render(parsed.value_render.as_deref()),
        // Markdown needs the 2-D grid (header row); json honors as_records.
        as_records: if want_markdown {
            false
        } else {
            parsed.as_records
        },
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
        Ok(r) => {
            let dimensions = compute_dimensions(&r.values);
            if want_markdown {
                serde_json::json!({
                    "ok": true,
                    "sheet": r.sheet,
                    "range": r.range,
                    "dimensions": dimensions,
                    "markdown": values_to_markdown(&r.values),
                })
            } else {
                serde_json::json!({
                    "ok": true,
                    "sheet": r.sheet,
                    "range": r.range,
                    "dimensions": dimensions,
                    "values": r.values,
                })
            }
        }
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

// E-T7b (Bundle 1, 2026-06-10) — production via_executor variants that
// use the shared attachment plumbing (Bulk T0).

/// Fetch a registered `.xlsx` attachment from the conversation catalog,
/// upload to Drive with mime conversion to a native Google Spreadsheet.
pub async fn dispatch_create_from_xlsx_via_executor(
    executor: &crate::dag_engine::infrastructure::dag_tool_executor::DagToolExecutor,
    args: serde_json::Value,
) -> serde_json::Value {
    let parsed: CreateFromXlsxArgs = match serde_json::from_value(args) {
        Ok(a) => a,
        Err(e) => return serde_json::json!({"error": "invalid_args", "message": e.to_string()}),
    };
    let stored = match executor.fetch_attachment_bytes(&parsed.attachment_id).await {
        Ok(b) => b,
        Err(e) => {
            return serde_json::json!({
                "error": "attachment_fetch_failed",
                "message": e,
                "attachment_id": parsed.attachment_id,
            });
        }
    };
    let client = match build_client() {
        Ok(c) => c,
        Err(e) => return e,
    };
    use crate::gsheets::domain::SheetsClient;
    match client.create_from_xlsx(&parsed.title, stored.bytes).await {
        Ok(meta) => serde_json::json!({
            "ok": true,
            "spreadsheet_id": meta.spreadsheet_id.0,
            "title": meta.title,
            "url": format!("https://docs.google.com/spreadsheets/d/{}", meta.spreadsheet_id.0),
            "sheets": meta.sheets,
        }),
        Err(e) => serde_json::json!({
            "error": "sheets_create_from_xlsx_failed",
            "message": e.to_string(),
        }),
    }
}

/// Export a spreadsheet as `.xlsx` and register the bytes as a new
/// attachment so the LLM receives an `attachment_id` it can pass to
/// downstream tools or share via `$attachment:<id>`.
pub async fn dispatch_export_xlsx_via_executor(
    executor: &crate::dag_engine::infrastructure::dag_tool_executor::DagToolExecutor,
    args: serde_json::Value,
) -> serde_json::Value {
    let parsed: ExportXlsxArgs = match serde_json::from_value(args) {
        Ok(a) => a,
        Err(e) => return serde_json::json!({"error": "invalid_args", "message": e.to_string()}),
    };
    let client = match build_client() {
        Ok(c) => c,
        Err(e) => return e,
    };
    use crate::gsheets::domain::{SheetsClient, SpreadsheetId};
    let bytes = match client
        .export_xlsx(&SpreadsheetId(parsed.spreadsheet_id.clone()))
        .await
    {
        Ok(b) => b,
        Err(e) => {
            return serde_json::json!({
                "error": "sheets_export_xlsx_failed",
                "message": e.to_string(),
            });
        }
    };
    let byte_len = bytes.len();
    let filename = format!("{}.xlsx", parsed.spreadsheet_id);
    let mime = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet".to_string();
    match executor
        .register_attachment_bytes(bytes, mime.clone(), filename.clone())
        .await
    {
        Ok(attachment_id) => serde_json::json!({
            "ok": true,
            "attachment_id": attachment_id,
            "byte_len": byte_len,
            "mime_type": mime,
            "filename": filename,
            "spreadsheet_id": parsed.spreadsheet_id,
        }),
        Err(e) => serde_json::json!({
            "error": "attachment_register_failed",
            "message": e,
            "byte_len": byte_len,
        }),
    }
}

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
        async fn get_modified_time(
            &self,
            _id: &SpreadsheetId,
        ) -> Result<Option<String>, SheetsError> {
            Ok(Some("2026-06-04T10:23:00Z".into()))
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
        async fn list_spreadsheets<'a>(
            &self,
            _filter: &crate::gsheets::domain::types::SpreadsheetListFilter<'a>,
        ) -> Result<crate::gsheets::domain::types::SpreadsheetListResult, SheetsError> {
            Err(SheetsError::Internal("not used".into()))
        }
        async fn share(
            &self,
            _id: &SpreadsheetId,
            _email: &str,
            _role: crate::gsheets::domain::types::ShareRole,
        ) -> Result<(), SheetsError> {
            Err(SheetsError::Internal("not used".into()))
        }
        async fn list_permissions(
            &self,
            _id: &SpreadsheetId,
        ) -> Result<crate::gsheets::domain::types::PermissionList, SheetsError> {
            Err(SheetsError::Internal("not used".into()))
        }
        async fn delete_permission(
            &self,
            _id: &SpreadsheetId,
            _permission_id: &str,
        ) -> Result<(), SheetsError> {
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
        async fn batch_update_cells(
            &self,
            _id: &SpreadsheetId,
            _s: &str,
            _u: Vec<(String, CellValue)>,
        ) -> Result<SetRangeResponse, SheetsError> {
            Err(SheetsError::Internal("not used".into()))
        }

        async fn batch_update(
            &self,
            _id: &SpreadsheetId,
            _requests: Vec<serde_json::Value>,
        ) -> Result<(), crate::gsheets::domain::SheetsError> {
            Ok(())
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

    /// Pins the `permission_denied` tool result shape. The LLM relies
    /// on `share_email` being a structured field (so it can quote the
    /// email back verbatim) AND a directive `hint` (so it doesn't
    /// paraphrase away the share instruction).
    ///
    /// Keep in lockstep with the gdocs equivalent — both subsystems
    /// emit the same shape so an agent that mixes gsheets + gdocs
    /// tools sees consistent error UX.
    #[test]
    fn permission_denied_payload_emits_share_email_and_directive_hint() {
        let v = error_to_json(SheetsError::PermissionDenied("agents@startti.co".into()));
        assert_eq!(v["error"], "permission_denied");
        assert_eq!(v["share_email"], "agents@startti.co");
        let hint = v["hint"].as_str().expect("hint must be a string");
        // Must reference the email so the LLM can pass it through.
        assert!(
            hint.contains("agents@startti.co"),
            "hint must inline the share email: {hint}"
        );
        // Directive language so terse models still surface the
        // actionable instruction.
        assert!(
            hint.contains("Editor"),
            "hint must mention Editor permission: {hint}"
        );
        assert!(
            hint.contains("verifique") || hint.contains("compartió"),
            "hint must direct the LLM to verify the share: {hint}"
        );
    }

    /// Degraded mode — when COLMENA_GOOGLE_SHARE_EMAIL is unset and
    /// the share_email arrives empty, the payload must still be
    /// actionable: tell the LLM to ask the operator for the email
    /// rather than fail silently.
    #[test]
    fn permission_denied_payload_in_degraded_mode_directs_to_operator() {
        let v = error_to_json(SheetsError::PermissionDenied(String::new()));
        assert_eq!(v["error"], "permission_denied");
        assert!(v["share_email"].is_null());
        let hint = v["hint"].as_str().unwrap();
        assert!(
            hint.contains("operador"),
            "degraded hint must point to the operator: {hint}"
        );
    }

    /// A minimal `SheetsClient` that returns a fixed `values` JSON array from
    /// `read_range` and errors on all other methods. Used by dispatch_read tests.
    struct ReadClient {
        values: serde_json::Value,
    }

    fn mock_client_returning(values: serde_json::Value) -> ReadClient {
        ReadClient { values }
    }

    #[async_trait]
    impl SheetsClient for ReadClient {
        async fn list_sheets(&self, _id: &SpreadsheetId) -> Result<Vec<SheetMeta>, SheetsError> {
            Err(SheetsError::Internal("not used".into()))
        }
        async fn get_modified_time(
            &self,
            _id: &SpreadsheetId,
        ) -> Result<Option<String>, SheetsError> {
            Err(SheetsError::Internal("not used".into()))
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
        async fn list_spreadsheets<'a>(
            &self,
            _filter: &crate::gsheets::domain::types::SpreadsheetListFilter<'a>,
        ) -> Result<crate::gsheets::domain::types::SpreadsheetListResult, SheetsError> {
            Err(SheetsError::Internal("not used".into()))
        }
        async fn share(
            &self,
            _id: &SpreadsheetId,
            _email: &str,
            _role: crate::gsheets::domain::types::ShareRole,
        ) -> Result<(), SheetsError> {
            Err(SheetsError::Internal("not used".into()))
        }
        async fn list_permissions(
            &self,
            _id: &SpreadsheetId,
        ) -> Result<crate::gsheets::domain::types::PermissionList, SheetsError> {
            Err(SheetsError::Internal("not used".into()))
        }
        async fn delete_permission(
            &self,
            _id: &SpreadsheetId,
            _permission_id: &str,
        ) -> Result<(), SheetsError> {
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
            Ok(ReadResponse {
                sheet: "Sheet1".into(),
                range: "A1:B3".into(),
                values: self.values.clone(),
            })
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
        async fn batch_update_cells(
            &self,
            _id: &SpreadsheetId,
            _s: &str,
            _u: Vec<(String, CellValue)>,
        ) -> Result<SetRangeResponse, SheetsError> {
            Err(SheetsError::Internal("not used".into()))
        }

        async fn batch_update(
            &self,
            _id: &SpreadsheetId,
            _requests: Vec<serde_json::Value>,
        ) -> Result<(), crate::gsheets::domain::SheetsError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn dispatch_read_defaults_to_markdown_with_dimensions() {
        // Mock returns the raw 2-D grid for as_records=false.
        let client = mock_client_returning(serde_json::json!([["name", "qty"], ["apple", 3]]));
        let args = serde_json::json!({ "spreadsheet_id": "id", "sheet": "Sheet1" });
        let out = dispatch_read_with_client(args, &client).await;
        assert_eq!(out["ok"], true);
        assert_eq!(
            out["markdown"],
            "| name | qty |\n| --- | --- |\n| apple | 3 |"
        );
        assert_eq!(
            out["dimensions"],
            serde_json::json!({ "rows": 2, "columns": 2 })
        );
        assert!(
            out.get("values").is_none(),
            "markdown mode must not include raw values"
        );
    }

    #[tokio::test]
    async fn dispatch_read_json_format_preserves_values() {
        let client = mock_client_returning(serde_json::json!([["name", "qty"], ["apple", 3]]));
        let args =
            serde_json::json!({ "spreadsheet_id": "id", "sheet": "Sheet1", "format": "json" });
        let out = dispatch_read_with_client(args, &client).await;
        assert_eq!(
            out["values"],
            serde_json::json!([["name", "qty"], ["apple", 3]])
        );
        assert_eq!(
            out["dimensions"],
            serde_json::json!({ "rows": 2, "columns": 2 })
        );
        assert!(out.get("markdown").is_none());
    }

    #[test]
    fn values_to_markdown_renders_table_with_header() {
        let v = serde_json::json!([["name", "qty"], ["apple", 3], ["pear", 10]]);
        let md = values_to_markdown(&v);
        assert_eq!(
            md,
            "| name | qty |\n| --- | --- |\n| apple | 3 |\n| pear | 10 |"
        );
    }

    #[test]
    fn values_to_markdown_pads_ragged_rows_and_renders_types() {
        // ragged (2nd row shorter), null cell, bool, pipe + newline escaping
        let v = serde_json::json!([
            ["a", "b", "c"],
            [true, serde_json::Value::Null],
            ["x|y", "line1\nline2", 1.5]
        ]);
        let md = values_to_markdown(&v);
        assert_eq!(
            md,
            "| a | b | c |\n| --- | --- | --- |\n| true |  |  |\n| x\\|y | line1<br>line2 | 1.5 |"
        );
    }

    #[test]
    fn values_to_markdown_empty_is_empty_string() {
        let v = serde_json::json!([]);
        assert_eq!(values_to_markdown(&v), "");
    }

    #[test]
    fn compute_dimensions_uses_max_width_including_header() {
        let v = serde_json::json!([["a", "b", "c"], [1, 2]]);
        assert_eq!(
            compute_dimensions(&v),
            serde_json::json!({ "rows": 2, "columns": 3 })
        );
    }
}
