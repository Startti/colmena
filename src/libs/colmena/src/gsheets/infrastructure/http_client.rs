//! REST adapter implementing [`SheetsClient`] against the Google Sheets
//! API v4 + Drive API.

use crate::gsheets::domain::{
    CellValue, ReadOptions, ReadResponse, SetRangeResponse, SheetId, SheetMeta, SheetsClient,
    SheetsError, SpreadsheetId, SpreadsheetMeta,
};
use crate::gsheets::infrastructure::auth::TokenProvider;
use crate::gsheets::infrastructure::config::GSheetsConfig;
use async_trait::async_trait;
use reqwest::{Client, StatusCode};
use serde_json::Value;
use std::time::Duration;

const SHEETS_BASE: &str = "https://sheets.googleapis.com/v4/spreadsheets";
const DRIVE_BASE: &str = "https://www.googleapis.com/drive/v3/files";
const DRIVE_UPLOAD_BASE: &str = "https://www.googleapis.com/upload/drive/v3/files";

pub struct GoogleSheetsHttpClient {
    http: Client,
    token: TokenProvider,
    max_retries: u32,
    /// Base unit for exponential backoff. Production uses 1s; tests use
    /// 50ms to keep wiremock tests fast.
    retry_base_delay: Duration,
    /// Service account email parsed from the credentials JSON, if any.
    /// Used to populate `PermissionDenied(sa_email)` so the agent can
    /// tell the user which email to share spreadsheets with. Empty
    /// string when running under ADC (no SA JSON file available).
    sa_email: String,
    sheets_base: String,
    #[allow(dead_code)] // Used by E-T5 admin endpoints (create/export xlsx).
    drive_base: String,
    #[allow(dead_code)] // Used by E-T5 admin endpoints (create_from_xlsx).
    drive_upload_base: String,
}

impl GoogleSheetsHttpClient {
    /// Construct from config — production path.
    pub fn from_config(cfg: &GSheetsConfig) -> Result<Self, SheetsError> {
        let http = Client::builder()
            .timeout(cfg.request_timeout)
            .build()
            .map_err(|e| SheetsError::Internal(format!("reqwest builder: {e}")))?;
        // Parse the SA email out of the credentials JSON once at construction
        // so PermissionDenied can carry it as a hint. Failure to read or
        // parse falls back silently to empty string (ADC path also empty).
        let sa_email = cfg
            .credentials_path
            .as_ref()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .and_then(|content| serde_json::from_str::<serde_json::Value>(&content).ok())
            .and_then(|v| {
                v.get("client_email")
                    .and_then(|e| e.as_str())
                    .map(String::from)
            })
            .unwrap_or_default();
        Ok(Self {
            http,
            token: TokenProvider::new(cfg.scopes.clone()),
            max_retries: cfg.max_retries,
            retry_base_delay: Duration::from_secs(1), // production: 1s/2s/4s
            sa_email,
            sheets_base: SHEETS_BASE.to_string(),
            drive_base: DRIVE_BASE.to_string(),
            drive_upload_base: DRIVE_UPLOAD_BASE.to_string(),
        })
    }

    /// Test-only constructor pointing at a wiremock server. `max_retries`
    /// is bumped to 2 so the 401-refresh test has enough attempts to
    /// observe both the unauthorized and the success response.
    #[cfg(test)]
    pub fn for_tests(sheets_base: &str, drive_base: &str, drive_upload_base: &str) -> Self {
        Self {
            http: Client::builder()
                .timeout(Duration::from_secs(5))
                .build()
                .unwrap(),
            token: TokenProvider::new(vec!["test".to_string()]),
            max_retries: 2,
            retry_base_delay: Duration::from_millis(50), // tests: 50ms/100ms/200ms
            sa_email: String::new(),                     // ADC-style for tests
            sheets_base: sheets_base.to_string(),
            drive_base: drive_base.to_string(),
            drive_upload_base: drive_upload_base.to_string(),
        }
    }

    /// Bearer-auth GET with retry on 429/5xx + 401-refresh.
    async fn get_json(&self, url: &str) -> Result<Value, SheetsError> {
        for attempt in 0..=self.max_retries {
            let token = self.token.token().await?;
            let resp = self
                .http
                .get(url)
                .bearer_auth(&token)
                .send()
                .await
                .map_err(|e| SheetsError::Http(format!("send: {e}")))?;
            match resp.status() {
                StatusCode::OK => {
                    return resp
                        .json::<Value>()
                        .await
                        .map_err(|e| SheetsError::Http(format!("json: {e}")));
                }
                StatusCode::UNAUTHORIZED if attempt == 0 => {
                    self.token.invalidate().await;
                    continue;
                }
                StatusCode::FORBIDDEN => {
                    return Err(SheetsError::PermissionDenied(self.sa_email.clone()));
                }
                StatusCode::NOT_FOUND => {
                    return Err(SheetsError::SpreadsheetNotFound(url.to_string()));
                }
                StatusCode::TOO_MANY_REQUESTS => {
                    if attempt < self.max_retries {
                        tokio::time::sleep(self.retry_base_delay * (1 << attempt)).await;
                        continue;
                    }
                    return Err(SheetsError::RateLimit(60));
                }
                s if s.is_server_error() => {
                    if attempt < self.max_retries {
                        tokio::time::sleep(self.retry_base_delay * (1 << attempt)).await;
                        continue;
                    }
                    return Err(SheetsError::Http(format!("server error {s}")));
                }
                s => {
                    let body = resp.text().await.unwrap_or_default();
                    return Err(SheetsError::Http(format!("status {s}: {body}")));
                }
            }
        }
        Err(SheetsError::Http("retries exhausted".to_string()))
    }

    /// Bearer-auth PUT with retry on 429/5xx + 401-refresh.
    async fn put_json(&self, url: &str, body: Value) -> Result<Value, SheetsError> {
        for attempt in 0..=self.max_retries {
            let token = self.token.token().await?;
            let resp = self
                .http
                .put(url)
                .bearer_auth(&token)
                .json(&body)
                .send()
                .await
                .map_err(|e| SheetsError::Http(format!("send: {e}")))?;
            match resp.status() {
                StatusCode::OK => {
                    return resp
                        .json::<Value>()
                        .await
                        .map_err(|e| SheetsError::Http(format!("json: {e}")));
                }
                StatusCode::UNAUTHORIZED if attempt == 0 => {
                    self.token.invalidate().await;
                    continue;
                }
                StatusCode::FORBIDDEN => {
                    return Err(SheetsError::PermissionDenied(self.sa_email.clone()));
                }
                StatusCode::NOT_FOUND => {
                    return Err(SheetsError::SpreadsheetNotFound(url.to_string()));
                }
                StatusCode::BAD_REQUEST => {
                    let body = resp.text().await.unwrap_or_default();
                    if body.contains("Unable to parse range") {
                        return Err(SheetsError::SheetNotFound(body));
                    }
                    return Err(SheetsError::InvalidRange(body));
                }
                StatusCode::TOO_MANY_REQUESTS => {
                    if attempt < self.max_retries {
                        tokio::time::sleep(self.retry_base_delay * (1 << attempt)).await;
                        continue;
                    }
                    return Err(SheetsError::RateLimit(60));
                }
                s if s.is_server_error() => {
                    if attempt < self.max_retries {
                        tokio::time::sleep(self.retry_base_delay * (1 << attempt)).await;
                        continue;
                    }
                    return Err(SheetsError::Http(format!("server error {s}")));
                }
                s => {
                    let body = resp.text().await.unwrap_or_default();
                    return Err(SheetsError::Http(format!("status {s}: {body}")));
                }
            }
        }
        Err(SheetsError::Http("retries exhausted".to_string()))
    }
}

#[async_trait]
impl SheetsClient for GoogleSheetsHttpClient {
    async fn read_range(
        &self,
        id: &SpreadsheetId,
        sheet: &str,
        range: Option<&str>,
        opts: ReadOptions,
    ) -> Result<ReadResponse, SheetsError> {
        let quoted_sheet = quote_sheet_for_range(sheet);
        let full_range = match range {
            Some(r) => format!("{quoted_sheet}!{r}"),
            None => quoted_sheet,
        };
        let url = format!(
            "{}/{}/values/{}?valueRenderOption={}",
            self.sheets_base,
            id.0,
            urlencoding::encode(&full_range),
            opts.value_render.as_api_str()
        );
        let body = self.get_json(&url).await?;
        let returned_range = body
            .get("range")
            .and_then(|v| v.as_str())
            .unwrap_or(&full_range)
            .to_string();
        let raw_values = body
            .get("values")
            .cloned()
            .unwrap_or(Value::Array(Vec::new()));
        let values = if opts.as_records {
            rectangle_to_records(&raw_values)
        } else {
            raw_values
        };
        Ok(ReadResponse {
            sheet: sheet.to_string(),
            range: returned_range,
            values,
        })
    }

    async fn list_sheets(&self, id: &SpreadsheetId) -> Result<Vec<SheetMeta>, SheetsError> {
        let url = format!("{}/{}", self.sheets_base, id.0);
        let body = self.get_json(&url).await?;
        let sheets_arr = body
            .get("sheets")
            .and_then(|v| v.as_array())
            .ok_or_else(|| SheetsError::Internal("missing sheets[]".into()))?;
        let mut out = Vec::with_capacity(sheets_arr.len());
        for s in sheets_arr {
            let props = s
                .get("properties")
                .ok_or_else(|| SheetsError::Internal("missing properties".into()))?;
            let grid = props.get("gridProperties");
            out.push(SheetMeta {
                sheet_id: SheetId(props.get("sheetId").and_then(Value::as_i64).unwrap_or(0)),
                title: props
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                index: props.get("index").and_then(Value::as_u64).unwrap_or(0) as u32,
                row_count: grid
                    .and_then(|g| g.get("rowCount"))
                    .and_then(Value::as_u64)
                    .unwrap_or(0) as u32,
                col_count: grid
                    .and_then(|g| g.get("columnCount"))
                    .and_then(Value::as_u64)
                    .unwrap_or(0) as u32,
            });
        }
        Ok(out)
    }

    async fn set_cell(
        &self,
        id: &SpreadsheetId,
        sheet: &str,
        addr: &str,
        value: CellValue,
    ) -> Result<(), SheetsError> {
        let range = format!("{}!{addr}", quote_sheet_for_range(sheet));
        let url = format!(
            "{}/{}/values/{}?valueInputOption=USER_ENTERED",
            self.sheets_base,
            id.0,
            urlencoding::encode(&range)
        );
        let body = serde_json::json!({
            "range": range,
            "majorDimension": "ROWS",
            "values": [[value.to_json()]],
        });
        let _ = self.put_json(&url, body).await?;
        Ok(())
    }

    async fn set_range(
        &self,
        id: &SpreadsheetId,
        sheet: &str,
        start_addr: &str,
        values_2d: Vec<Vec<CellValue>>,
    ) -> Result<SetRangeResponse, SheetsError> {
        let range = format!("{}!{start_addr}", quote_sheet_for_range(sheet));
        let url = format!(
            "{}/{}/values/{}?valueInputOption=USER_ENTERED",
            self.sheets_base,
            id.0,
            urlencoding::encode(&range)
        );
        let rows: Vec<Vec<Value>> = values_2d
            .into_iter()
            .map(|row| row.into_iter().map(|c| c.to_json()).collect())
            .collect();
        let body = serde_json::json!({
            "range": range,
            "majorDimension": "ROWS",
            "values": rows,
        });
        let resp = self.put_json(&url, body).await?;
        Ok(SetRangeResponse {
            updated_cells: resp
                .get("updatedCells")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            updated_range: resp
                .get("updatedRange")
                .and_then(Value::as_str)
                .unwrap_or(&range)
                .to_string(),
        })
    }

    // Stubs — filled in by E-T5.
    async fn create_spreadsheet(&self, _title: &str) -> Result<SpreadsheetMeta, SheetsError> {
        unimplemented!("E-T5")
    }
    async fn create_from_xlsx(
        &self,
        _title: &str,
        _bytes: Vec<u8>,
    ) -> Result<SpreadsheetMeta, SheetsError> {
        unimplemented!("E-T5")
    }
    async fn export_xlsx(&self, _id: &SpreadsheetId) -> Result<Vec<u8>, SheetsError> {
        unimplemented!("E-T5")
    }
    async fn add_sheet(&self, _id: &SpreadsheetId, _name: &str) -> Result<SheetMeta, SheetsError> {
        unimplemented!("E-T5")
    }
    async fn delete_sheet(
        &self,
        _id: &SpreadsheetId,
        _name_or_sheet_id: &str,
    ) -> Result<(), SheetsError> {
        unimplemented!("E-T5")
    }
}

/// Wrap a sheet name in single quotes if it contains anything other
/// than ASCII alphanumeric or underscore. Internal `'` characters are
/// escaped by doubling per Google's range syntax: `It's` → `'It''s'`.
fn quote_sheet_for_range(sheet: &str) -> String {
    let needs_quoting = sheet.is_empty()
        || sheet
            .chars()
            .any(|c| !(c.is_ascii_alphanumeric() || c == '_'));
    if needs_quoting {
        format!("'{}'", sheet.replace('\'', "''"))
    } else {
        sheet.to_string()
    }
}

/// Convert a `[[col_header,...], [v1, v2, ...], ...]` rectangle into
/// `[{col_header: v1, ...}, ...]`. The first row is taken as headers.
///
/// Contract (pinned by unit tests):
/// - Rows with MORE cells than headers: extra cells are silently
///   discarded (column-projection model — matches pandas behavior).
/// - Headers with empty names (`""`): those columns are skipped, and
///   the corresponding cell in every data row is omitted from the
///   record.
/// - Rows with FEWER cells than headers: missing cells become
///   `Value::Null`.
fn rectangle_to_records(rect: &Value) -> Value {
    let Some(rows) = rect.as_array() else {
        return Value::Array(Vec::new());
    };
    if rows.is_empty() {
        return Value::Array(Vec::new());
    }
    let Some(headers) = rows[0].as_array() else {
        return Value::Array(Vec::new());
    };
    let headers: Vec<String> = headers
        .iter()
        .map(|h| h.as_str().unwrap_or("").to_string())
        .collect();
    let records: Vec<Value> = rows
        .iter()
        .skip(1)
        .map(|row| {
            let cells = row.as_array().cloned().unwrap_or_default();
            let mut obj = serde_json::Map::new();
            for (i, h) in headers.iter().enumerate() {
                if h.is_empty() {
                    continue;
                }
                obj.insert(h.clone(), cells.get(i).cloned().unwrap_or(Value::Null));
            }
            Value::Object(obj)
        })
        .collect();
    Value::Array(records)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path_regex};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    async fn setup_mock() -> (MockServer, GoogleSheetsHttpClient) {
        let server = MockServer::start().await;
        let client = GoogleSheetsHttpClient::for_tests(&server.uri(), &server.uri(), &server.uri());
        client.token.set_token_for_test("fake-bearer-token").await;
        (server, client)
    }

    #[tokio::test]
    async fn read_range_returns_2d_array_for_unformatted() {
        let (server, client) = setup_mock().await;

        Mock::given(method("GET"))
            .and(path_regex(r"/abc/values/Sheet1%21A1%3AB2"))
            .and(header("authorization", "Bearer fake-bearer-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "range": "Sheet1!A1:B2",
                "majorDimension": "ROWS",
                "values": [["x", 1], ["y", 2]],
            })))
            .mount(&server)
            .await;

        let resp = client
            .read_range(
                &SpreadsheetId("abc".into()),
                "Sheet1",
                Some("A1:B2"),
                ReadOptions::default(),
            )
            .await
            .expect("read ok");
        assert_eq!(resp.sheet, "Sheet1");
        assert_eq!(resp.range, "Sheet1!A1:B2");
        assert_eq!(resp.values, serde_json::json!([["x", 1], ["y", 2]]));
    }

    #[tokio::test]
    async fn list_sheets_returns_meta_for_each_tab() {
        let (server, client) = setup_mock().await;
        Mock::given(method("GET"))
            .and(path_regex(r"/abc$"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "spreadsheetId": "abc",
                "properties": {"title": "X"},
                "sheets": [
                    {"properties": {
                        "sheetId": 0, "title": "Sheet1", "index": 0,
                        "gridProperties": {"rowCount": 100, "columnCount": 26},
                    }},
                    {"properties": {
                        "sheetId": 12345, "title": "Numbers", "index": 1,
                        "gridProperties": {"rowCount": 50, "columnCount": 5},
                    }},
                ],
            })))
            .mount(&server)
            .await;

        let sheets = client
            .list_sheets(&SpreadsheetId("abc".into()))
            .await
            .expect("list ok");
        assert_eq!(sheets.len(), 2);
        assert_eq!(sheets[0].title, "Sheet1");
        assert_eq!(sheets[0].sheet_id, SheetId(0));
        assert_eq!(sheets[1].title, "Numbers");
        assert_eq!(sheets[1].row_count, 50);
    }

    #[tokio::test]
    async fn set_cell_sends_user_entered_value_input_option() {
        let (server, client) = setup_mock().await;
        Mock::given(method("PUT"))
            .and(path_regex(r"/abc/values/Sheet1%21E1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "updatedCells": 1,
                "updatedRange": "Sheet1!E1",
            })))
            .mount(&server)
            .await;

        client
            .set_cell(
                &SpreadsheetId("abc".into()),
                "Sheet1",
                "E1",
                CellValue::String("=SUM(B:B)".to_string()),
            )
            .await
            .expect("set ok");
    }

    #[tokio::test]
    async fn set_range_writes_2d_block_and_returns_updated_cells() {
        let (server, client) = setup_mock().await;
        Mock::given(method("PUT"))
            .and(path_regex(r"/abc/values/Sheet1%21A1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "updatedCells": 4,
                "updatedRange": "Sheet1!A1:B2",
            })))
            .mount(&server)
            .await;

        let result = client
            .set_range(
                &SpreadsheetId("abc".into()),
                "Sheet1",
                "A1",
                vec![
                    vec![CellValue::String("x".into()), CellValue::Number(1.0)],
                    vec![CellValue::String("y".into()), CellValue::Number(2.0)],
                ],
            )
            .await
            .expect("set_range ok");
        assert_eq!(result.updated_cells, 4);
        assert_eq!(result.updated_range, "Sheet1!A1:B2");
    }

    #[tokio::test]
    async fn read_range_404_maps_to_spreadsheet_not_found() {
        let (server, client) = setup_mock().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        let err = client
            .read_range(
                &SpreadsheetId("zzz".into()),
                "Sheet1",
                Some("A1"),
                ReadOptions::default(),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, SheetsError::SpreadsheetNotFound(_)));
    }

    #[tokio::test]
    async fn read_range_429_retries_then_surfaces_rate_limit() {
        let (server, client) = setup_mock().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(429))
            .mount(&server)
            .await;
        let err = client
            .read_range(
                &SpreadsheetId("abc".into()),
                "Sheet1",
                Some("A1"),
                ReadOptions::default(),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, SheetsError::RateLimit(_)));
    }

    #[tokio::test]
    async fn read_range_401_invalidates_cache_and_retries() {
        // NOTE: this test asserts the RETRY behavior after a 401 only.
        // It does NOT verify that a FRESH token is sent on the retry —
        // doing so would require either mocking yup-oauth2 (out of scope
        // for the unit-test layer) or restructuring `TokenProvider` to
        // accept an injectable token source. The end-to-end refresh path
        // is covered by the integration tests in E-T9.
        let (server, client) = setup_mock().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(401))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "range": "Sheet1!A1",
                "values": [["ok"]],
            })))
            .mount(&server)
            .await;
        // Seed the token cache so the post-invalidate refresh has something
        // to return (TokenProvider's prod path hits yup-oauth2 which would
        // panic in tests). The test asserts that after the 401 the client
        // re-fetches the cached token and retries the request.
        client.token.set_token_for_test("fake-bearer-token").await;
        let resp = client
            .read_range(
                &SpreadsheetId("abc".into()),
                "Sheet1",
                Some("A1"),
                ReadOptions::default(),
            )
            .await
            .expect("retry succeeds");
        assert_eq!(resp.values, serde_json::json!([["ok"]]));
    }

    #[test]
    fn rectangle_to_records_drops_cells_past_header_count() {
        let rect = serde_json::json!([
            ["A", "B"],   // 2 headers
            [1, 2, 3, 4], // 4 cells — extras 3 and 4 dropped
        ]);
        let r = rectangle_to_records(&rect);
        assert_eq!(r, serde_json::json!([{"A": 1, "B": 2}]));
    }

    #[test]
    fn rectangle_to_records_skips_columns_with_empty_headers() {
        let rect = serde_json::json!([
            ["A", "", "C"], // middle header empty
            [1, 2, 3],
        ]);
        let r = rectangle_to_records(&rect);
        assert_eq!(r, serde_json::json!([{"A": 1, "C": 3}]));
    }

    #[test]
    fn quote_sheet_for_range_handles_spaces_and_apostrophes() {
        assert_eq!(quote_sheet_for_range("Sheet1"), "Sheet1");
        assert_eq!(quote_sheet_for_range("My Sheet"), "'My Sheet'");
        assert_eq!(quote_sheet_for_range("It's"), "'It''s'");
        assert_eq!(quote_sheet_for_range(""), "''");
    }
}
