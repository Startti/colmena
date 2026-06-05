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
        Ok(Self {
            http,
            token: TokenProvider::new(cfg.scopes.clone()),
            max_retries: cfg.max_retries,
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
                    return Err(SheetsError::PermissionDenied(String::new()));
                }
                StatusCode::NOT_FOUND => {
                    return Err(SheetsError::SpreadsheetNotFound(url.to_string()));
                }
                StatusCode::TOO_MANY_REQUESTS => {
                    if attempt < self.max_retries {
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        continue;
                    }
                    return Err(SheetsError::RateLimit(60));
                }
                s if s.is_server_error() => {
                    if attempt < self.max_retries {
                        tokio::time::sleep(Duration::from_millis(50)).await;
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
                    return Err(SheetsError::PermissionDenied(String::new()));
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
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        continue;
                    }
                    return Err(SheetsError::RateLimit(60));
                }
                s if s.is_server_error() => {
                    if attempt < self.max_retries {
                        tokio::time::sleep(Duration::from_millis(50)).await;
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
        let full_range = match range {
            Some(r) => format!("{sheet}!{r}"),
            None => sheet.to_string(),
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
        let range = format!("{sheet}!{addr}");
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
        let range = format!("{sheet}!{start_addr}");
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

/// Convert a `[[col_header,...], [v1, v2, ...], ...]` rectangle into
/// `[{col_header: v1, ...}, ...]`. The first row is taken as headers.
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
    async fn read_range_401_refreshes_token_and_retries_once() {
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
}
