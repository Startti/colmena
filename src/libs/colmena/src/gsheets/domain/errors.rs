//! Error type for the Google Sheets integration. Public so dispatchers
//! can map to JSON tool results.

#[derive(Debug, thiserror::Error)]
pub enum SheetsError {
    /// No credentials configured (no `GOOGLE_APPLICATION_CREDENTIALS`
    /// env var and ADC fallback also failed). The hint string tells the
    /// operator/agent what to do.
    #[error("gsheets_not_configured: {0}")]
    NotConfigured(String),

    /// Auth flow ran but token acquisition failed (network, malformed
    /// SA JSON, etc.).
    #[error("auth_failed: {0}")]
    AuthFailed(String),

    /// Spreadsheet id doesn't resolve. The id is included for the agent
    /// to surface back to the user.
    #[error("spreadsheet_not_found: {0}")]
    SpreadsheetNotFound(String),

    /// Sheet (tab) name unknown within an existing spreadsheet.
    #[error("sheet_not_found: {0}")]
    SheetNotFound(String),

    /// Range syntax invalid (e.g. "Foo" instead of "Sheet1!A1:B2").
    #[error("invalid_range: {0}")]
    InvalidRange(String),

    /// 403 from Google. The string is best-effort the service-account
    /// email so the agent can tell the user "share this spreadsheet with
    /// <email>". Empty string if the SA email isn't available.
    #[error("permission_denied: {0}")]
    PermissionDenied(String),

    /// 429 — rate limit hit. Retry after the given seconds.
    #[error("rate_limit: retry after {0}s")]
    RateLimit(u32),

    /// Network / 5xx / timeout. Free-form message.
    #[error("http_error: {0}")]
    Http(String),

    /// Unexpected internal failure (shouldn't happen in well-tested code).
    #[error("internal: {0}")]
    Internal(String),
}
