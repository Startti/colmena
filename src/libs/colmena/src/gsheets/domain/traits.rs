//! Port (in hexagonal terms) for any backend that can read/write Google
//! Sheets-like spreadsheets. `infrastructure::GoogleSheetsHttpClient`
//! (added in E-T4/T5) is the production impl; tests use a mock impl.

use crate::gsheets::domain::{
    CellValue, ReadOptions, ReadResponse, SetRangeResponse, SheetMeta, SheetsError, SpreadsheetId,
    SpreadsheetMeta,
};
use async_trait::async_trait;

#[async_trait]
pub trait SheetsClient: Send + Sync {
    async fn create_spreadsheet(&self, title: &str) -> Result<SpreadsheetMeta, SheetsError>;

    async fn create_from_xlsx(
        &self,
        title: &str,
        bytes: Vec<u8>,
    ) -> Result<SpreadsheetMeta, SheetsError>;

    async fn export_xlsx(&self, id: &SpreadsheetId) -> Result<Vec<u8>, SheetsError>;

    /// Drive discovery — return the spreadsheets the OAuth user can see,
    /// filtered by `filter`. Used by `gsheets_list_spreadsheets`.
    ///
    /// Symmetric to `DocsClient::list_documents`. Hits the same Drive
    /// endpoint with `mimeType='application/vnd.google-apps.spreadsheet'`.
    async fn list_spreadsheets<'a>(
        &self,
        filter: &crate::gsheets::domain::types::SpreadsheetListFilter<'a>,
    ) -> Result<crate::gsheets::domain::types::SpreadsheetListResult, SheetsError>;

    async fn list_sheets(&self, id: &SpreadsheetId) -> Result<Vec<SheetMeta>, SheetsError>;

    async fn add_sheet(&self, id: &SpreadsheetId, name: &str) -> Result<SheetMeta, SheetsError>;

    /// `name_or_sheet_id` accepts either the human-friendly sheet title
    /// or a stringified numeric `SheetId`. Implementations resolve.
    async fn delete_sheet(
        &self,
        id: &SpreadsheetId,
        name_or_sheet_id: &str,
    ) -> Result<(), SheetsError>;

    async fn read_range(
        &self,
        id: &SpreadsheetId,
        sheet: &str,
        range: Option<&str>,
        opts: ReadOptions,
    ) -> Result<ReadResponse, SheetsError>;

    async fn set_cell(
        &self,
        id: &SpreadsheetId,
        sheet: &str,
        addr: &str,
        value: CellValue,
    ) -> Result<(), SheetsError>;

    async fn set_range(
        &self,
        id: &SpreadsheetId,
        sheet: &str,
        start_addr: &str,
        values_2d: Vec<Vec<CellValue>>,
    ) -> Result<SetRangeResponse, SheetsError>;

    /// Apply N cell-level writes in one HTTPS round-trip via
    /// `spreadsheets.values.batchUpdate`. Each `(addr, value)` tuple is
    /// an A1-addressed cell on `sheet`. Returns total cells updated.
    async fn batch_update_cells(
        &self,
        id: &SpreadsheetId,
        sheet: &str,
        updates: Vec<(String, CellValue)>,
    ) -> Result<SetRangeResponse, SheetsError>;
}
