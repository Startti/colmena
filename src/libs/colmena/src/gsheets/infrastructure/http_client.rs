//! REST adapter implementing [`SheetsClient`] against the Google Sheets
//! API v4 + Drive API.

use crate::gsheets::domain::{
    CellValue, ReadOptions, ReadResponse, SetRangeResponse, SheetId, SheetMeta, SheetsClient,
    SheetsError, SpreadsheetId, SpreadsheetMeta, ValueRenderOption,
};
use crate::gsheets::infrastructure::auth::TokenProvider;
use crate::gsheets::infrastructure::config::GSheetsConfig;
#[allow(unused_imports)]
use crate::gsheets::infrastructure::merge_fill::{forward_fill_merges, MergeRect};
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
    /// Workspace user email the agent acts as — read from
    /// `COLMENA_GOOGLE_SHARE_EMAIL` via [`GSheetsConfig`]. Surfaced in
    /// `SheetsError::PermissionDenied` so the LLM can tell the user
    /// which address to share the spreadsheet with. Empty string in
    /// degraded deployments where the var is not set.
    share_email: String,
    sheets_base: String,
    drive_base: String,
    drive_upload_base: String,
}

impl GoogleSheetsHttpClient {
    /// Construct from config — production path.
    ///
    /// Reads OAuth credentials from env (via
    /// `OAuthCredentials::from_env`). Any missing variable surfaces as
    /// `SheetsError::NotConfigured` with the full list of missing
    /// vars in the message — so deploys see one clear error per boot
    /// rather than playing whack-a-mole.
    pub fn from_config(cfg: &GSheetsConfig) -> Result<Self, SheetsError> {
        let http = Client::builder()
            .timeout(cfg.request_timeout)
            .build()
            .map_err(|e| SheetsError::Internal(format!("reqwest builder: {e}")))?;
        let creds = crate::google_oauth::infrastructure::OAuthCredentials::from_env()
            .map_err(|e| SheetsError::NotConfigured(format!("{e}")))?;
        Ok(Self {
            http,
            token: TokenProvider::from_oauth_credentials(creds),
            max_retries: cfg.max_retries,
            retry_base_delay: Duration::from_secs(1), // production: 1s/2s/4s
            share_email: cfg.share_email.clone(),
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
            token: TokenProvider::for_tests_static(),
            max_retries: 2,
            retry_base_delay: Duration::from_millis(50), // tests: 50ms/100ms/200ms
            share_email: String::new(),
            sheets_base: sheets_base.to_string(),
            drive_base: drive_base.to_string(),
            drive_upload_base: drive_upload_base.to_string(),
        }
    }

    /// Test-only helper that seeds a sticky bearer token on the
    /// internal `TokenProvider`. Used by tests in sibling modules
    /// (e.g. `gsheets_run_python::tests`) that can't reach the private
    /// `token` field directly.
    #[cfg(test)]
    pub async fn token_test_seed(&self, token: impl Into<String>) {
        self.token.set_token_for_test(token).await;
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
                    return Err(SheetsError::PermissionDenied(self.share_email.clone()));
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
                    return Err(SheetsError::PermissionDenied(self.share_email.clone()));
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

    /// Bearer-auth POST with retry on 429/5xx + 401-refresh.
    async fn post_json(&self, url: &str, body: Value) -> Result<Value, SheetsError> {
        for attempt in 0..=self.max_retries {
            let token = self.token.token().await?;
            let resp = self
                .http
                .post(url)
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
                    return Err(SheetsError::PermissionDenied(self.share_email.clone()));
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

    /// Parse a Sheets API response into [`SpreadsheetMeta`]. Used by
    /// `create_spreadsheet` (response has `spreadsheetId` inline) and
    /// `create_from_xlsx` (Drive returns the id; we fetch metadata
    /// separately and pass `default_id` so spreadsheets without that
    /// field still resolve).
    fn parse_meta(value: &Value, default_id: Option<&str>) -> Result<SpreadsheetMeta, SheetsError> {
        let id = value
            .get("spreadsheetId")
            .and_then(Value::as_str)
            .or(default_id)
            .ok_or_else(|| SheetsError::Internal("missing spreadsheetId".into()))?
            .to_string();
        let title = value
            .get("properties")
            .and_then(|p| p.get("title"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let url = value
            .get("spreadsheetUrl")
            .and_then(Value::as_str)
            .map(String::from)
            .unwrap_or_else(|| format!("https://docs.google.com/spreadsheets/d/{id}"));
        let sheets = value
            .get("sheets")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|s| {
                        let props = s.get("properties")?;
                        let grid = props.get("gridProperties");
                        Some(SheetMeta {
                            sheet_id: SheetId(
                                props.get("sheetId").and_then(Value::as_i64).unwrap_or(0),
                            ),
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
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(SpreadsheetMeta {
            spreadsheet_id: SpreadsheetId(id),
            title,
            url,
            sheets,
        })
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

    async fn create_spreadsheet(&self, title: &str) -> Result<SpreadsheetMeta, SheetsError> {
        let url = self.sheets_base.clone();
        let body = serde_json::json!({
            "properties": {"title": title},
        });
        let resp = self.post_json(&url, body).await?;
        Self::parse_meta(&resp, None)
    }

    async fn create_from_xlsx(
        &self,
        title: &str,
        bytes: Vec<u8>,
    ) -> Result<SpreadsheetMeta, SheetsError> {
        let token = self.token.token().await?;
        let upload_url = format!("{}?uploadType=multipart", self.drive_upload_base);

        let boundary = format!("colmena-bnd-{}", uuid::Uuid::new_v4().simple());
        let metadata = serde_json::json!({
            "name": title,
            "mimeType": "application/vnd.google-apps.spreadsheet",
        })
        .to_string();
        let mut body: Vec<u8> = Vec::with_capacity(bytes.len() + 512);
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        body.extend_from_slice(b"Content-Type: application/json; charset=UTF-8\r\n\r\n");
        body.extend_from_slice(metadata.as_bytes());
        body.extend_from_slice(format!("\r\n--{boundary}\r\n").as_bytes());
        body.extend_from_slice(
            b"Content-Type: application/vnd.openxmlformats-officedocument.spreadsheetml.sheet\r\n\r\n",
        );
        body.extend_from_slice(&bytes);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());

        let resp = self
            .http
            .post(&upload_url)
            .bearer_auth(&token)
            .header(
                "Content-Type",
                format!("multipart/related; boundary={boundary}"),
            )
            .body(body)
            .send()
            .await
            .map_err(|e| SheetsError::Http(format!("upload send: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(match status {
                StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
                    SheetsError::PermissionDenied(self.share_email.clone())
                }
                _ => SheetsError::Http(format!("upload {status}: {body}")),
            });
        }
        let drive_resp: Value = resp
            .json()
            .await
            .map_err(|e| SheetsError::Http(format!("upload json: {e}")))?;
        let new_id = drive_resp
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| SheetsError::Internal("drive upload missing id".into()))?
            .to_string();

        let meta_url = format!("{}/{}", self.sheets_base, new_id);
        let meta_resp = self.get_json(&meta_url).await?;
        Self::parse_meta(&meta_resp, Some(&new_id))
    }

    async fn export_xlsx(&self, id: &SpreadsheetId) -> Result<Vec<u8>, SheetsError> {
        let token = self.token.token().await?;
        let url = format!(
            "{}/{}/export?mimeType=application%2Fvnd.openxmlformats-officedocument.spreadsheetml.sheet",
            self.drive_base, id.0
        );
        let resp = self
            .http
            .get(&url)
            .bearer_auth(&token)
            .send()
            .await
            .map_err(|e| SheetsError::Http(format!("export send: {e}")))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(match status {
                StatusCode::NOT_FOUND => SheetsError::SpreadsheetNotFound(id.0.clone()),
                StatusCode::FORBIDDEN => SheetsError::PermissionDenied(self.share_email.clone()),
                _ => SheetsError::Http(format!("export {status}: {body}")),
            });
        }
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| SheetsError::Http(format!("export bytes: {e}")))?;
        Ok(bytes.to_vec())
    }

    async fn share(
        &self,
        id: &SpreadsheetId,
        email: &str,
        role: crate::gsheets::domain::types::ShareRole,
    ) -> Result<(), SheetsError> {
        let url = format!(
            "{}/{}/permissions?supportsAllDrives=true",
            self.drive_base, id.0
        );
        let body = serde_json::json!({
            "role": role.as_api_str(),
            "type": "user",
            "emailAddress": email,
        });
        // post_json returns the created Permission resource; we only care
        // that the call succeeds (404 / 403 surfaced as typed errors by
        // post_json), so discard the body.
        self.post_json(&url, body).await?;
        Ok(())
    }

    async fn list_permissions(
        &self,
        id: &SpreadsheetId,
    ) -> Result<crate::gsheets::domain::types::PermissionList, SheetsError> {
        use crate::gsheets::domain::types::{PermissionEntry, PermissionList};
        let url = format!(
            "{}/{}/permissions?fields={}&supportsAllDrives=true",
            self.drive_base,
            id.0,
            urlencoding::encode("permissions(id,type,role,emailAddress,displayName),nextPageToken"),
        );
        let j = self.get_json(&url).await?;
        let perms = j
            .get("permissions")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let mut out = Vec::with_capacity(perms.len());
        for p in perms {
            let permission_id = p
                .get("id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| SheetsError::Internal("permission missing id".into()))?
                .to_string();
            let permission_type = p
                .get("type")
                .and_then(|v| v.as_str())
                .unwrap_or("user")
                .to_string();
            let role = p
                .get("role")
                .and_then(|v| v.as_str())
                .unwrap_or("reader")
                .to_string();
            let email = p
                .get("emailAddress")
                .and_then(|v| v.as_str())
                .map(String::from);
            let display_name = p
                .get("displayName")
                .and_then(|v| v.as_str())
                .map(String::from);
            out.push(PermissionEntry {
                permission_id,
                permission_type,
                role,
                email,
                display_name,
            });
        }
        Ok(PermissionList { permissions: out })
    }

    async fn delete_permission(
        &self,
        id: &SpreadsheetId,
        permission_id: &str,
    ) -> Result<(), SheetsError> {
        let url = format!(
            "{}/{}/permissions/{}?supportsAllDrives=true",
            self.drive_base, id.0, permission_id
        );
        // No existing delete_json helper — issue the request directly with
        // a single retry on auth failure, matching the contract of get/put.
        for attempt in 0..=self.max_retries {
            let token = self.token.token().await?;
            let resp = self
                .http
                .delete(&url)
                .bearer_auth(&token)
                .send()
                .await
                .map_err(|e| SheetsError::Http(format!("send: {e}")))?;
            match resp.status() {
                s if s.is_success() => return Ok(()),
                StatusCode::UNAUTHORIZED if attempt == 0 => {
                    self.token.invalidate().await;
                    continue;
                }
                StatusCode::FORBIDDEN => {
                    return Err(SheetsError::PermissionDenied(self.share_email.clone()));
                }
                StatusCode::NOT_FOUND => {
                    return Err(SheetsError::SpreadsheetNotFound(format!(
                        "permission {permission_id} on {}",
                        id.0
                    )));
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

    async fn list_spreadsheets<'a>(
        &self,
        filter: &crate::gsheets::domain::types::SpreadsheetListFilter<'a>,
    ) -> Result<crate::gsheets::domain::types::SpreadsheetListResult, SheetsError> {
        use crate::gsheets::domain::types::{SpreadsheetListItem, SpreadsheetListResult};
        // Build the Drive `q` parameter as `and`-joined predicates.
        let mut q_parts: Vec<String> = vec![
            "mimeType='application/vnd.google-apps.spreadsheet'".into(),
            "trashed=false".into(),
        ];
        if let Some(query) = filter.query.filter(|s| !s.trim().is_empty()) {
            let safe = query.replace('\'', "\\'");
            q_parts.push(format!("name contains '{safe}'"));
        }
        if let Some(folder) = filter.parent_folder_id.filter(|s| !s.is_empty()) {
            let safe = folder.replace('\'', "\\'");
            q_parts.push(format!("'{safe}' in parents"));
        }
        if let Some(after) = filter.modified_after.filter(|s| !s.is_empty()) {
            let safe = after.replace('\'', "\\'");
            q_parts.push(format!("modifiedTime >= '{safe}'"));
        }
        let q = q_parts.join(" and ");
        let limit = filter.limit.unwrap_or(20).clamp(1, 100);
        let fields = "nextPageToken,files(id,name,modifiedTime,owners(emailAddress))";

        let mut url = format!(
            "{}?q={}&pageSize={}&fields={}&orderBy=modifiedTime desc",
            self.drive_base,
            urlencoding::encode(&q),
            limit,
            urlencoding::encode(fields),
        );
        if let Some(pt) = filter.page_token.filter(|s| !s.is_empty()) {
            url.push_str("&pageToken=");
            url.push_str(&urlencoding::encode(pt));
        }
        let j = self.get_json(&url).await?;

        let files = j
            .get("files")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let mut spreadsheets = Vec::with_capacity(files.len());
        for f in files {
            let spreadsheet_id = f
                .get("id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| SheetsError::Internal("file missing id".into()))?
                .to_string();
            let name = f
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("(untitled)")
                .to_string();
            let modified_time = f
                .get("modifiedTime")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let owners: Vec<String> = f
                .get("owners")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|o| {
                            o.get("emailAddress")
                                .and_then(|v| v.as_str())
                                .map(String::from)
                        })
                        .collect()
                })
                .unwrap_or_default();
            spreadsheets.push(SpreadsheetListItem {
                spreadsheet_id: SpreadsheetId(spreadsheet_id.clone()),
                name,
                url: format!("https://docs.google.com/spreadsheets/d/{spreadsheet_id}"),
                modified_time,
                owners,
            });
        }
        let next_page_token = j
            .get("nextPageToken")
            .and_then(|v| v.as_str())
            .map(String::from)
            .filter(|s| !s.is_empty());
        Ok(SpreadsheetListResult {
            spreadsheets,
            next_page_token,
        })
    }

    async fn get_modified_time(&self, id: &SpreadsheetId) -> Result<Option<String>, SheetsError> {
        let url = format!(
            "{}/{}?fields=modifiedTime&supportsAllDrives=true",
            self.drive_base, id.0
        );
        let j = self.get_json(&url).await?;
        Ok(j.get("modifiedTime")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(String::from))
    }

    async fn add_sheet(&self, id: &SpreadsheetId, name: &str) -> Result<SheetMeta, SheetsError> {
        let url = format!("{}/{}:batchUpdate", self.sheets_base, id.0);
        let body = serde_json::json!({
            "requests": [{"addSheet": {"properties": {"title": name}}}],
        });
        let resp = self.post_json(&url, body).await?;
        let props = resp
            .get("replies")
            .and_then(|r| r.as_array())
            .and_then(|arr| arr.first())
            .and_then(|r| r.get("addSheet"))
            .and_then(|r| r.get("properties"))
            .ok_or_else(|| SheetsError::Internal("missing addSheet.properties".into()))?;
        let grid = props.get("gridProperties");
        Ok(SheetMeta {
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
        })
    }

    async fn delete_sheet(
        &self,
        id: &SpreadsheetId,
        name_or_sheet_id: &str,
    ) -> Result<(), SheetsError> {
        let sheet_id: i64 = match name_or_sheet_id.parse::<i64>() {
            Ok(n) => n,
            Err(_) => {
                let sheets = self.list_sheets(id).await?;
                let m = sheets
                    .iter()
                    .find(|s| s.title == name_or_sheet_id)
                    .ok_or_else(|| SheetsError::SheetNotFound(name_or_sheet_id.to_string()))?;
                m.sheet_id.0
            }
        };
        let url = format!("{}/{}:batchUpdate", self.sheets_base, id.0);
        let body = serde_json::json!({
            "requests": [{"deleteSheet": {"sheetId": sheet_id}}],
        });
        let _ = self.post_json(&url, body).await?;
        Ok(())
    }

    async fn batch_update_cells(
        &self,
        id: &SpreadsheetId,
        sheet: &str,
        updates: Vec<(String, CellValue)>,
    ) -> Result<SetRangeResponse, SheetsError> {
        if updates.is_empty() {
            return Ok(SetRangeResponse {
                updated_cells: 0,
                updated_range: String::new(),
            });
        }
        let n_updates = updates.len();
        let value_ranges: Vec<Value> = updates
            .into_iter()
            .map(|(addr, val)| {
                serde_json::json!({
                    "range": format!("{}!{addr}", quote_sheet_for_range(sheet)),
                    "majorDimension": "ROWS",
                    "values": [[val.to_json()]],
                })
            })
            .collect();
        let body = serde_json::json!({
            "valueInputOption": "USER_ENTERED",
            "data": value_ranges,
        });
        let url = format!("{}/{}/values:batchUpdate", self.sheets_base, id.0);
        let resp = self.post_json(&url, body).await?;
        let updated = resp
            .get("totalUpdatedCells")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        Ok(SetRangeResponse {
            updated_cells: updated,
            updated_range: format!("{n_updates} cells in {sheet}"),
        })
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

/// Google `ExtendedValue` → scalar JSON. `ExtendedValue` is a one-of:
/// `{numberValue | stringValue | boolValue | formulaValue | errorValue}`.
/// Empty / unknown ⇒ `Null`. Error cells surface their `type` string
/// (e.g. `"#REF!"`).
#[allow(dead_code)]
fn extended_value_to_scalar(ev: &Value) -> Value {
    if let Some(n) = ev.get("numberValue") {
        n.clone()
    } else if let Some(s) = ev.get("stringValue") {
        s.clone()
    } else if let Some(b) = ev.get("boolValue") {
        b.clone()
    } else if let Some(f) = ev.get("formulaValue") {
        f.clone()
    } else if let Some(e) = ev.get("errorValue") {
        Value::String(
            e.get("type")
                .and_then(Value::as_str)
                .unwrap_or("ERROR")
                .to_string(),
        )
    } else {
        Value::Null
    }
}

/// Map one grid `CellData` to the scalar JSON the read pipeline expects,
/// honoring the requested render option. Mirrors what `values.get` returned:
/// numbers as JSON numbers, strings as strings, booleans as bools, empty as
/// `Null`.
#[allow(dead_code)]
fn cell_scalar(cell: &Value, render: ValueRenderOption) -> Value {
    match render {
        ValueRenderOption::FormattedValue => {
            cell.get("formattedValue").cloned().unwrap_or(Value::Null)
        }
        ValueRenderOption::UnformattedValue => cell
            .get("effectiveValue")
            .map(extended_value_to_scalar)
            .unwrap_or(Value::Null),
        ValueRenderOption::Formula => match cell.get("userEnteredValue") {
            // Formula cells: the user-entered formula text. Non-formula
            // cells: their literal user-entered value.
            Some(uev) => match uev.get("formulaValue") {
                Some(f) => f.clone(),
                None => extended_value_to_scalar(uev),
            },
            None => Value::Null,
        },
    }
}

/// Parse the first sheet's first `GridData` block from a `spreadsheets.get`
/// response into a RECTANGULAR grid of scalar values plus the block's absolute
/// top-left offset `(row_offset, col_offset)`. Rows are padded to a uniform
/// width so forward-fill can reach every cell of a merge.
#[allow(dead_code)]
fn parse_grid_block(body: &Value, render: ValueRenderOption) -> (Vec<Vec<Value>>, usize, usize) {
    let block = body
        .get("sheets")
        .and_then(|s| s.as_array())
        .and_then(|s| s.first())
        .and_then(|s| s.get("data"))
        .and_then(|d| d.as_array())
        .and_then(|d| d.first());
    let Some(block) = block else {
        return (Vec::new(), 0, 0);
    };
    let row_offset = block.get("startRow").and_then(Value::as_u64).unwrap_or(0) as usize;
    let col_offset = block
        .get("startColumn")
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize;
    let mut grid: Vec<Vec<Value>> = block
        .get("rowData")
        .and_then(|r| r.as_array())
        .map(|rows| {
            rows.iter()
                .map(|row| {
                    row.get("values")
                        .and_then(|v| v.as_array())
                        .map(|cells| cells.iter().map(|c| cell_scalar(c, render)).collect())
                        .unwrap_or_default()
                })
                .collect()
        })
        .unwrap_or_default();
    let width = grid.iter().map(Vec::len).max().unwrap_or(0);
    for row in &mut grid {
        row.resize(width, Value::Null);
    }
    (grid, row_offset, col_offset)
}

/// Parse the first sheet's `merges` array into `MergeRect`s. Absent ⇒ empty.
#[allow(dead_code)]
fn parse_merges(body: &Value) -> Vec<MergeRect> {
    body.get("sheets")
        .and_then(|s| s.as_array())
        .and_then(|s| s.first())
        .and_then(|s| s.get("merges"))
        .and_then(|m| m.as_array())
        .map(|arr| {
            arr.iter()
                .map(|m| MergeRect {
                    start_row: m.get("startRowIndex").and_then(Value::as_u64).unwrap_or(0) as usize,
                    end_row: m.get("endRowIndex").and_then(Value::as_u64).unwrap_or(0) as usize,
                    start_col: m
                        .get("startColumnIndex")
                        .and_then(Value::as_u64)
                        .unwrap_or(0) as usize,
                    end_col: m.get("endColumnIndex").and_then(Value::as_u64).unwrap_or(0) as usize,
                })
                .collect()
        })
        .unwrap_or_default()
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
pub(crate) fn rectangle_to_records(rect: &Value) -> Value {
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

    fn sample_grid_body() -> serde_json::Value {
        // Two rows: header "Cat"/"N", then a row where Cat is empty (a merge
        // gap) and N=5. A merges entry covers A1:A2 (the Cat column).
        serde_json::json!({
            "sheets": [{
                "data": [{
                    "startRow": 0,
                    "startColumn": 0,
                    "rowData": [
                        {"values": [
                            {"effectiveValue": {"stringValue": "Cat"}, "formattedValue": "Cat",
                             "userEnteredValue": {"stringValue": "Cat"}},
                            {"effectiveValue": {"stringValue": "N"}, "formattedValue": "N",
                             "userEnteredValue": {"stringValue": "N"}}
                        ]},
                        {"values": [
                            {},
                            {"effectiveValue": {"numberValue": 5.0}, "formattedValue": "5",
                             "userEnteredValue": {"numberValue": 5.0}}
                        ]}
                    ]
                }],
                "merges": [
                    {"startRowIndex": 0, "endRowIndex": 2, "startColumnIndex": 0, "endColumnIndex": 1}
                ]
            }]
        })
    }

    #[test]
    fn parse_grid_block_unformatted_maps_effective_value() {
        let body = sample_grid_body();
        let (grid, ro, co) = parse_grid_block(&body, ValueRenderOption::UnformattedValue);
        assert_eq!(ro, 0);
        assert_eq!(co, 0);
        assert_eq!(
            grid,
            vec![
                vec![serde_json::json!("Cat"), serde_json::json!("N")],
                vec![serde_json::json!(null), serde_json::json!(5.0)],
            ]
        );
    }

    #[test]
    fn parse_grid_block_formatted_uses_formatted_value() {
        let body = sample_grid_body();
        let (grid, _, _) = parse_grid_block(&body, ValueRenderOption::FormattedValue);
        // Number 5 surfaces as its formatted string "5".
        assert_eq!(grid[1][1], serde_json::json!("5"));
    }

    #[test]
    fn parse_grid_block_formula_prefers_formula_value() {
        let body = serde_json::json!({
            "sheets": [{
                "data": [{"startRow": 0, "startColumn": 0, "rowData": [
                    {"values": [
                        {"userEnteredValue": {"formulaValue": "=SUM(B:B)"},
                         "effectiveValue": {"numberValue": 7.0}, "formattedValue": "7"}
                    ]}
                ]}],
                "merges": []
            }]
        });
        let (grid, _, _) = parse_grid_block(&body, ValueRenderOption::Formula);
        assert_eq!(grid[0][0], serde_json::json!("=SUM(B:B)"));
    }

    #[test]
    fn parse_merges_reads_rectangles() {
        let body = sample_grid_body();
        let merges = parse_merges(&body);
        assert_eq!(merges.len(), 1);
        assert_eq!(
            merges[0],
            MergeRect {
                start_row: 0,
                end_row: 2,
                start_col: 0,
                end_col: 1
            }
        );
    }

    #[test]
    fn parse_grid_block_no_merges_is_plain_rectangle() {
        // Regression guard: a body with no merges parses to the same logical
        // 2-D values the old values.get path produced.
        let body = serde_json::json!({
            "sheets": [{
                "data": [{"startRow": 0, "startColumn": 0, "rowData": [
                    {"values": [
                        {"effectiveValue": {"stringValue": "x"}},
                        {"effectiveValue": {"numberValue": 1.0}}
                    ]},
                    {"values": [
                        {"effectiveValue": {"stringValue": "y"}},
                        {"effectiveValue": {"numberValue": 2.0}}
                    ]}
                ]}],
                "merges": []
            }]
        });
        let (grid, _, _) = parse_grid_block(&body, ValueRenderOption::UnformattedValue);
        assert_eq!(
            grid,
            vec![
                vec![serde_json::json!("x"), serde_json::json!(1.0)],
                vec![serde_json::json!("y"), serde_json::json!(2.0)],
            ]
        );
        assert!(parse_merges(&body).is_empty());
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
    async fn batch_update_cells_posts_value_ranges() {
        let server = wiremock::MockServer::start().await;
        let client = GoogleSheetsHttpClient::for_tests(&server.uri(), &server.uri(), &server.uri());
        client.token_test_seed("fake-token").await;

        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path_regex(r"/ss_b/values:batchUpdate"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "totalUpdatedCells": 3,
                    "responses": [
                        {"updatedRange": "Sheet1!B5"},
                        {"updatedRange": "Sheet1!B6"},
                        {"updatedRange": "Sheet1!B7"},
                    ]
                })),
            )
            .expect(1)
            .mount(&server)
            .await;

        let updates = vec![
            (
                "B5".to_string(),
                crate::gsheets::domain::CellValue::Number(99.0),
            ),
            (
                "B6".to_string(),
                crate::gsheets::domain::CellValue::Number(99.0),
            ),
            (
                "B7".to_string(),
                crate::gsheets::domain::CellValue::Number(99.0),
            ),
        ];
        let resp = client
            .batch_update_cells(
                &crate::gsheets::domain::SpreadsheetId("ss_b".to_string()),
                "Sheet1",
                updates,
            )
            .await
            .expect("batch_update_cells must succeed");
        assert_eq!(resp.updated_cells, 3);
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

    #[tokio::test]
    async fn create_spreadsheet_returns_meta_with_url() {
        let (server, client) = setup_mock().await;
        Mock::given(method("POST"))
            .and(path_regex(r"/v4/spreadsheets$|/$"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "spreadsheetId": "new_id",
                "properties": {"title": "My Sheet"},
                "spreadsheetUrl": "https://docs.google.com/spreadsheets/d/new_id",
                "sheets": [{"properties": {
                    "sheetId": 0, "title": "Sheet1", "index": 0,
                    "gridProperties": {"rowCount": 1000, "columnCount": 26}
                }}],
            })))
            .mount(&server)
            .await;
        let meta = client
            .create_spreadsheet("My Sheet")
            .await
            .expect("create ok");
        assert_eq!(meta.spreadsheet_id.0, "new_id");
        assert_eq!(meta.title, "My Sheet");
        assert!(meta.url.contains("new_id"));
        assert_eq!(meta.sheets.len(), 1);
    }

    #[tokio::test]
    async fn add_sheet_returns_new_tab_meta() {
        let (server, client) = setup_mock().await;
        Mock::given(method("POST"))
            .and(path_regex(r"/abc:batchUpdate"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "replies": [{
                    "addSheet": {"properties": {
                        "sheetId": 999, "title": "New", "index": 1,
                        "gridProperties": {"rowCount": 1000, "columnCount": 26}
                    }}
                }],
            })))
            .mount(&server)
            .await;
        let m = client
            .add_sheet(&SpreadsheetId("abc".into()), "New")
            .await
            .expect("add ok");
        assert_eq!(m.title, "New");
        assert_eq!(m.sheet_id, SheetId(999));
    }

    #[tokio::test]
    async fn delete_sheet_by_numeric_id() {
        let (server, client) = setup_mock().await;
        Mock::given(method("POST"))
            .and(path_regex(r"/abc:batchUpdate"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"replies":[]})),
            )
            .mount(&server)
            .await;
        client
            .delete_sheet(&SpreadsheetId("abc".into()), "999")
            .await
            .expect("delete ok");
    }

    #[tokio::test]
    async fn create_from_xlsx_uploads_via_drive_and_returns_meta() {
        let (server, client) = setup_mock().await;
        // In tests, drive_upload_base = server.uri() (no path component),
        // so the upload request hits "/" with ?uploadType=multipart.
        Mock::given(method("POST"))
            .and(path_regex(r"^/$"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "new_sheet_id",
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path_regex(r"/new_sheet_id$"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "spreadsheetId": "new_sheet_id",
                "properties": {"title": "Q3 Sales"},
                "spreadsheetUrl": "https://docs.google.com/spreadsheets/d/new_sheet_id",
                "sheets": [],
            })))
            .mount(&server)
            .await;

        let meta = client
            .create_from_xlsx("Q3 Sales", b"fake-xlsx-bytes".to_vec())
            .await
            .expect("upload ok");
        assert_eq!(meta.spreadsheet_id.0, "new_sheet_id");
        assert_eq!(meta.title, "Q3 Sales");
    }

    #[tokio::test]
    async fn export_xlsx_returns_binary_bytes() {
        let (server, client) = setup_mock().await;
        // In tests, drive_base = server.uri() (no path component), so the
        // export request hits "/abc/export" not "/files/abc/export".
        Mock::given(method("GET"))
            .and(path_regex(r"^/abc/export$"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_bytes(b"FAKE_XLSX_BYTES".as_ref())
                    .insert_header(
                        "content-type",
                        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
                    ),
            )
            .mount(&server)
            .await;
        let bytes = client
            .export_xlsx(&SpreadsheetId("abc".into()))
            .await
            .expect("export ok");
        assert_eq!(bytes, b"FAKE_XLSX_BYTES");
    }
}
