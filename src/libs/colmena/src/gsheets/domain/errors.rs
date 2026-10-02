//! Error type for the Google Sheets integration. Public so dispatchers
//! can map to JSON tool results.

/// User-facing text for [`SheetsError::GoogleAccountReconnectRequired`]
/// (shared with the gdocs equivalent).
pub const RECONNECT_GOOGLE_MESSAGE: &str = "The connected Google account's authorization expired \
     or was revoked. Reconnect Google in ADP and run the agent again.";

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

    /// 403 from Google while acting as the user's connected Google account
    /// (per-node `google_workspace_auth`): that account has no access to the
    /// file. There is no platform address to share with.
    #[error("permission_denied: the connected Google account has no access to this file")]
    ConnectedAccountPermissionDenied,

    /// The connected Google account's refresh was rejected (`invalid_grant`):
    /// its authorization expired or was revoked. The user reconnects Google;
    /// there is nothing for the operator to set up.
    #[error("google_account_reconnect_required: {RECONNECT_GOOGLE_MESSAGE}")]
    GoogleAccountReconnectRequired,

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
