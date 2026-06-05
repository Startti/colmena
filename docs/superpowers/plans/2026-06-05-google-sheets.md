# Google Sheets Integration Implementation Plan (Subsystem E)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship 9 synthetic LLM tools that let colmena agents read, write, create, and analyse Google Sheets via the Sheets API v4 + Drive API, using only ADC/Service-Account auth (no OAuth user flow). Tool shape mirrors `crdt_doc_*` so existing skills transfer with mechanical find-and-replace.

**Architecture:** Hexagonal — `SheetsClient` trait in `src/libs/colmena/src/gsheets/domain/`, `GoogleSheetsHttpClient` REST adapter in `infrastructure/`, application-layer use cases per operation, and one `gsheets_tools.rs` dispatcher file in `dag_engine/.../llm_synthetic_tools/`. Tests mock the trait; integration tests hit Google behind an `#[ignore]` gate.

**Tech Stack:** Rust 1.95, `reqwest` (existing), `yup-oauth2 = "11"` (existing — same crate used by `image_generation.rs`), `serde_json` (existing), `wiremock = "0.6"` (existing) for HTTP mock tests. **Zero new dependencies.**

**Reference spec:** [`docs/superpowers/specs/2026-06-05-google-sheets-design.md`](../specs/2026-06-05-google-sheets-design.md)

---

## File Structure

### Create

| Path | Responsibility |
|---|---|
| `src/libs/colmena/src/gsheets/mod.rs` | Module root; re-exports public items. |
| `src/libs/colmena/src/gsheets/domain/mod.rs` | Domain layer index. |
| `src/libs/colmena/src/gsheets/domain/types.rs` | `SpreadsheetId`, `SheetId`, `CellValue`, `ValueRenderOption`, `ReadOptions`, `ReadResponse`, `SetRangeResponse`, `SheetMeta`, `SpreadsheetMeta`. |
| `src/libs/colmena/src/gsheets/domain/errors.rs` | `SheetsError` enum (thiserror). |
| `src/libs/colmena/src/gsheets/domain/traits.rs` | `SheetsClient` async trait. |
| `src/libs/colmena/src/gsheets/infrastructure/mod.rs` | Infra layer index. |
| `src/libs/colmena/src/gsheets/infrastructure/config.rs` | `GSheetsConfig::from_env()`. |
| `src/libs/colmena/src/gsheets/infrastructure/auth.rs` | ADC + SA JSON token acquisition via `yup-oauth2`; in-memory cache with 50-min TTL. |
| `src/libs/colmena/src/gsheets/infrastructure/http_client.rs` | `GoogleSheetsHttpClient` implementing `SheetsClient` over REST. |
| `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gsheets_tools.rs` | 9 tool dispatchers + tool definitions. |
| `src/libs/colmena/skills/gsheets-cross-sheet-analysis/SKILL.md` | Skill index. |
| `src/libs/colmena/skills/gsheets-cross-sheet-analysis/references/*.md` | 6 pattern reference files (a-cell-diff, b-row-diff, c-schema-diff, d-statistical, e-join-enrich, f-conditional-transform). |
| `src/libs/colmena/tests/gsheets_integration_test.rs` | `#[ignore]`-gated integration test hitting the real API. |
| `tests/graphs/agents/gsheets_smoke.json` | DAG smoke graph that creates a spreadsheet, writes a formula, reads back. |

### Modify

| Path | Change |
|---|---|
| `src/libs/colmena/src/lib.rs` (or `src/libs/colmena/src/main.rs` — wherever top-level modules are declared) | Add `pub mod gsheets;`. |
| `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs` | Add `pub mod gsheets_tools;` + re-exports of tool name constants and dispatcher fns. |
| `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs` | Add a `gsheets_*` dispatch arm that routes to the new dispatcher fns. |
| `docs/developer_guide/38_crdt_documents.md` (or a new `docs/developer_guide/39_gsheets.md` if §5.9 is too crowded) | Add §5.9 documenting tool surface + auth setup. |
| `docs/node_as_tools_reference.json` | Add 9 tool entries with arg schema + returns. |
| `docs/BACKLOG.md` | New "Subsystem E v1.1" section with deferred items. |
| `docs/CHANGELOG_2026-06.md` | E entry. |

---

## Task 1: Domain types + errors

**Files:**
- Create: `src/libs/colmena/src/gsheets/mod.rs`
- Create: `src/libs/colmena/src/gsheets/domain/mod.rs`
- Create: `src/libs/colmena/src/gsheets/domain/types.rs`
- Create: `src/libs/colmena/src/gsheets/domain/errors.rs`
- Modify: `src/libs/colmena/src/lib.rs` (find the line listing top-level modules and add `pub mod gsheets;` alphabetically)

- [ ] **Step 1: Locate the top-level module declaration site**

Run: `grep -n "^pub mod " src/libs/colmena/src/lib.rs | head -20`
Expected: list of `pub mod foo;` lines. Note where `gsheets` would land alphabetically (probably between `dag_engine` and `llm`).

- [ ] **Step 2: Create `gsheets/mod.rs`**

Write `src/libs/colmena/src/gsheets/mod.rs`:

```rust
//! Google Sheets integration (Subsystem E).
//!
//! Hexagonal layout:
//! - [`domain`] — port (`SheetsClient` trait), value types, errors.
//! - [`infrastructure`] — REST adapter, auth, config.
//!
//! Tool dispatchers live in
//! `crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::gsheets_tools`
//! and delegate to the `SheetsClient` trait.

pub mod domain;
pub mod infrastructure;
```

- [ ] **Step 3: Create `gsheets/domain/mod.rs`**

Write:

```rust
//! Domain layer — port + value types + errors. No infra deps.

pub mod errors;
pub mod traits;
pub mod types;

pub use errors::SheetsError;
pub use traits::SheetsClient;
pub use types::*;
```

- [ ] **Step 4: Create `gsheets/domain/errors.rs`**

```rust
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
```

- [ ] **Step 5: Create `gsheets/domain/types.rs`**

```rust
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
    pub fn as_api_str(self) -> &'static str {
        match self {
            Self::FormattedValue => "FORMATTED_VALUE",
            Self::UnformattedValue => "UNFORMATTED_VALUE",
            Self::Formula => "FORMULA",
        }
    }
}

/// Options passed to a read call.
#[derive(Debug, Clone)]
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn cell_value_round_trips_through_json() {
        let cases = [
            (json!(null), CellValue::Null),
            (json!(true), CellValue::Bool(true)),
            (json!(42), CellValue::Number(42.0)),
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
        assert_eq!(ValueRenderOption::FormattedValue.as_api_str(), "FORMATTED_VALUE");
        assert_eq!(ValueRenderOption::UnformattedValue.as_api_str(), "UNFORMATTED_VALUE");
        assert_eq!(ValueRenderOption::Formula.as_api_str(), "FORMULA");
    }
}
```

- [ ] **Step 6: Add `pub mod gsheets;` to `lib.rs`**

Open `src/libs/colmena/src/lib.rs` and insert `pub mod gsheets;` alphabetically among the other `pub mod` declarations (using the location identified in Step 1).

- [ ] **Step 7: Verify build + tests pass**

```bash
cargo check -p colmena_dag_engine --lib 2>&1 | tail -10
cargo test -p colmena_dag_engine --lib gsheets::domain 2>&1 | tail -15
cargo clippy -p colmena_dag_engine --lib --tests -- -D warnings 2>&1 | tail
```

Expected: clean build, 3 tests pass, zero clippy warnings.

- [ ] **Step 8: Commit**

```bash
git add src/libs/colmena/src/gsheets/ src/libs/colmena/src/lib.rs
git commit -m "$(cat <<'EOF'
feat(E-T1): gsheets domain — types, errors, module scaffold

Public surface: SpreadsheetId, SheetId, CellValue, ValueRenderOption,
ReadOptions, ReadResponse, SetRangeResponse, SheetMeta, SpreadsheetMeta,
SheetsError. Three round-trip unit tests on CellValue + ValueRenderOption
API strings.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: SheetsClient trait

**Files:**
- Create: `src/libs/colmena/src/gsheets/domain/traits.rs`

- [ ] **Step 1: Write the trait**

```rust
//! Port (in hexagonal terms) for any backend that can read/write Google
//! Sheets-like spreadsheets. `infrastructure::GoogleSheetsHttpClient` is
//! the production impl; tests use a `MockSheetsClient`.

use crate::gsheets::domain::{
    CellValue, ReadOptions, ReadResponse, SetRangeResponse, SheetMeta, SheetsError,
    SpreadsheetId, SpreadsheetMeta,
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

    async fn list_sheets(&self, id: &SpreadsheetId) -> Result<Vec<SheetMeta>, SheetsError>;

    async fn add_sheet(
        &self,
        id: &SpreadsheetId,
        name: &str,
    ) -> Result<SheetMeta, SheetsError>;

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
}
```

- [ ] **Step 2: Verify build**

```bash
cargo check -p colmena_dag_engine --lib 2>&1 | tail
```

Expected: clean.

- [ ] **Step 3: Commit**

```bash
git add src/libs/colmena/src/gsheets/domain/traits.rs
git commit -m "$(cat <<'EOF'
feat(E-T2): gsheets SheetsClient trait — the port

9 methods spanning create / list / add / delete / read / set_cell /
set_range / create_from_xlsx / export_xlsx. async_trait + Send + Sync
bounds so dispatcher can hold one as `Arc<dyn SheetsClient>`.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: Auth (ADC + Service Account JSON) with token cache

**Files:**
- Create: `src/libs/colmena/src/gsheets/infrastructure/mod.rs`
- Create: `src/libs/colmena/src/gsheets/infrastructure/auth.rs`
- Create: `src/libs/colmena/src/gsheets/infrastructure/config.rs`

- [ ] **Step 1: Create the infrastructure module index**

`src/libs/colmena/src/gsheets/infrastructure/mod.rs`:

```rust
//! Infrastructure layer — REST adapter + auth + config.

pub mod auth;
pub mod config;
pub mod http_client;
```

(We'll create `http_client.rs` in Task 4 — this declaration is for now a forward reference; the build will fail until Task 4 lands. Acceptable as a TDD-style intentional break.)

Actually to avoid the broken build, comment out `http_client` for now:

```rust
//! Infrastructure layer — REST adapter + auth + config.

pub mod auth;
pub mod config;
// pub mod http_client; — added in Task 4
```

- [ ] **Step 2: Create config**

`src/libs/colmena/src/gsheets/infrastructure/config.rs`:

```rust
//! `GSheetsConfig` — read from env vars at dispatcher construction.

use std::path::PathBuf;
use std::time::Duration;

/// All `https://www.googleapis.com/auth/spreadsheets` and
/// `https://www.googleapis.com/auth/drive.file` by default; override
/// via the `COLMENA_GSHEETS_SCOPES` env var (comma-separated short
/// names; we prepend the URL prefix).
pub const DEFAULT_SCOPES: &[&str] = &[
    "https://www.googleapis.com/auth/spreadsheets",
    "https://www.googleapis.com/auth/drive.file",
];

#[derive(Debug, Clone)]
pub struct GSheetsConfig {
    /// Path to a service-account JSON. When `None`, `yup-oauth2` falls
    /// through to ADC (`GOOGLE_APPLICATION_CREDENTIALS` or gcloud-saved
    /// user creds or GCE metadata server, in that order).
    pub credentials_path: Option<PathBuf>,
    /// Scopes the token must cover.
    pub scopes: Vec<String>,
    /// Per-request HTTP timeout.
    pub request_timeout: Duration,
    /// Max retries on 429 / 5xx before giving up.
    pub max_retries: u32,
}

impl GSheetsConfig {
    /// Read from environment. Always succeeds — the *consumer* of the
    /// config is the one that surfaces `SheetsError::NotConfigured` when
    /// the chosen auth method actually fails.
    pub fn from_env() -> Self {
        let credentials_path = std::env::var("GOOGLE_APPLICATION_CREDENTIALS")
            .ok()
            .map(PathBuf::from);
        let scopes = std::env::var("COLMENA_GSHEETS_SCOPES")
            .ok()
            .map(|s| {
                s.split(',')
                    .map(|p| {
                        let p = p.trim();
                        if p.starts_with("https://") {
                            p.to_string()
                        } else {
                            format!("https://www.googleapis.com/auth/{p}")
                        }
                    })
                    .collect()
            })
            .unwrap_or_else(|| DEFAULT_SCOPES.iter().map(|s| s.to_string()).collect());
        Self {
            credentials_path,
            scopes,
            request_timeout: Duration::from_secs(30),
            max_retries: 3,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_scopes_cover_sheets_and_drive_file() {
        let cfg = GSheetsConfig {
            credentials_path: None,
            scopes: DEFAULT_SCOPES.iter().map(|s| s.to_string()).collect(),
            request_timeout: Duration::from_secs(30),
            max_retries: 3,
        };
        assert!(cfg
            .scopes
            .iter()
            .any(|s| s.ends_with("/spreadsheets")));
        assert!(cfg
            .scopes
            .iter()
            .any(|s| s.ends_with("/drive.file")));
    }
}
```

- [ ] **Step 3: Create auth with token cache**

`src/libs/colmena/src/gsheets/infrastructure/auth.rs`:

```rust
//! Token acquisition + caching for the Google Sheets REST client.
//!
//! Pattern mirrors `dag_engine::infrastructure::nodes::image_generation`
//! (see `get_vertex_token` around line 572) — same `yup-oauth2` ADC
//! flow, conservative 50-min cache (Google tokens last ~1h).

use crate::gsheets::domain::SheetsError;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

#[derive(Debug)]
struct CachedToken {
    token: String,
    expires_at: Instant,
}

/// Holds a token cache + the configured scopes. Cheap to clone — the
/// inner state is an `Arc<Mutex<_>>`.
#[derive(Clone)]
pub struct TokenProvider {
    cache: Arc<Mutex<Option<CachedToken>>>,
    scopes: Vec<String>,
}

impl TokenProvider {
    pub fn new(scopes: Vec<String>) -> Self {
        Self {
            cache: Arc::new(Mutex::new(None)),
            scopes,
        }
    }

    /// Returns a fresh bearer token, hitting `yup-oauth2` only when the
    /// cache is empty or within 60s of expiry.
    pub async fn token(&self) -> Result<String, SheetsError> {
        use yup_oauth2::authenticator::ApplicationDefaultCredentialsTypes;
        use yup_oauth2::{
            ApplicationDefaultCredentialsAuthenticator, ApplicationDefaultCredentialsFlowOpts,
        };

        let mut cache = self.cache.lock().await;
        if let Some(c) = &*cache {
            if c.expires_at > Instant::now() + Duration::from_secs(60) {
                return Ok(c.token.clone());
            }
        }

        let opts = ApplicationDefaultCredentialsFlowOpts::default();
        let auth = match ApplicationDefaultCredentialsAuthenticator::builder(opts).await {
            ApplicationDefaultCredentialsTypes::InstanceMetadata(builder) => builder
                .build()
                .await
                .map_err(|e| SheetsError::AuthFailed(format!("metadata server: {e}")))?,
            ApplicationDefaultCredentialsTypes::ServiceAccount(builder) => builder
                .build()
                .await
                .map_err(|e| SheetsError::AuthFailed(format!("service account: {e}")))?,
        };

        let scope_refs: Vec<&str> = self.scopes.iter().map(String::as_str).collect();
        let token = auth.token(&scope_refs).await.map_err(|e| {
            let msg = e.to_string();
            if msg.to_lowercase().contains("no credentials")
                || msg.contains("GOOGLE_APPLICATION_CREDENTIALS")
            {
                SheetsError::NotConfigured(
                    "no Google credentials found — set \
                     GOOGLE_APPLICATION_CREDENTIALS to a service-account JSON \
                     path, or run `gcloud auth application-default login` for \
                     local dev"
                        .to_string(),
                )
            } else {
                SheetsError::AuthFailed(msg)
            }
        })?;
        let access = token
            .token()
            .ok_or_else(|| SheetsError::AuthFailed("empty access token".to_string()))?
            .to_string();

        let expires_at = Instant::now() + Duration::from_secs(50 * 60);
        *cache = Some(CachedToken {
            token: access.clone(),
            expires_at,
        });
        Ok(access)
    }

    /// Force-invalidate the cache. Called by the HTTP client after a 401
    /// to trigger refresh on the retry.
    pub async fn invalidate(&self) {
        let mut cache = self.cache.lock().await;
        *cache = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_is_cloneable_cheaply() {
        // Sanity: cloning shares the same cache (Arc).
        let p1 = TokenProvider::new(vec!["scope1".into()]);
        let p2 = p1.clone();
        assert!(Arc::ptr_eq(&p1.cache, &p2.cache));
    }
}
```

- [ ] **Step 4: Build + test**

```bash
cargo test -p colmena_dag_engine --lib gsheets 2>&1 | tail -10
cargo clippy -p colmena_dag_engine --lib --tests -- -D warnings 2>&1 | tail
```

Expected: 2 unit tests pass (from config + auth), clippy clean. (We do NOT test the real ADC fetch here — that's an integration concern.)

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/gsheets/infrastructure/
git commit -m "$(cat <<'EOF'
feat(E-T3): gsheets auth + config

TokenProvider wraps yup-oauth2's ADC flow with a 50-min in-memory cache,
mirroring image_generation.rs's pattern. Surfaces "no credentials" as
SheetsError::NotConfigured with a clear hint; other failures as
AuthFailed. GSheetsConfig::from_env() parses GOOGLE_APPLICATION_CREDENTIALS
and COLMENA_GSHEETS_SCOPES (latter accepts short names; we prefix the
googleapis URL).

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: GoogleSheetsHttpClient — read + write endpoints

**Files:**
- Create: `src/libs/colmena/src/gsheets/infrastructure/http_client.rs`
- Modify: `src/libs/colmena/src/gsheets/infrastructure/mod.rs` (uncomment `pub mod http_client;`)

This task covers the 4 most-used endpoints: `read_range`, `set_cell`, `set_range`, `list_sheets`. The remaining 5 endpoints come in Task 5 so each PR stays reviewable.

- [ ] **Step 1: Stub the client struct + first failing test**

Write the initial file:

```rust
//! REST adapter implementing [`SheetsClient`] against the Google Sheets
//! API v4 + Drive API.

use crate::gsheets::domain::{
    CellValue, ReadOptions, ReadResponse, SetRangeResponse, SheetMeta, SheetsClient,
    SheetsError, SheetId, SpreadsheetId, SpreadsheetMeta,
};
use crate::gsheets::infrastructure::auth::TokenProvider;
use crate::gsheets::infrastructure::config::GSheetsConfig;
use async_trait::async_trait;
use reqwest::{Client, StatusCode};
use serde_json::Value;
use std::time::Duration;

/// Base URL constants. Parameterised in tests via `with_base_urls`.
const SHEETS_BASE: &str = "https://sheets.googleapis.com/v4/spreadsheets";
const DRIVE_BASE: &str = "https://www.googleapis.com/drive/v3/files";
const DRIVE_UPLOAD_BASE: &str = "https://www.googleapis.com/upload/drive/v3/files";

pub struct GoogleSheetsHttpClient {
    http: Client,
    token: TokenProvider,
    max_retries: u32,
    sheets_base: String,
    drive_base: String,
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

    /// Test-only constructor that points at a wiremock server.
    #[cfg(test)]
    pub fn for_tests(sheets_base: &str, drive_base: &str, drive_upload_base: &str) -> Self {
        Self {
            http: Client::builder()
                .timeout(Duration::from_secs(5))
                .build()
                .unwrap(),
            token: TokenProvider::new(vec!["test".to_string()]),
            max_retries: 1,
            sheets_base: sheets_base.to_string(),
            drive_base: drive_base.to_string(),
            drive_upload_base: drive_upload_base.to_string(),
        }
    }
}
```

Add to the inline tests module at the bottom (initially empty — first test added in step 2):

```rust
#[cfg(test)]
mod tests {
    use super::*;
}
```

- [ ] **Step 2: Add the test for read_range happy path**

In the `tests` module, add a wiremock-based test:

```rust
    use wiremock::matchers::{header, method, path_regex};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Set up a wiremock server and a client pointed at it. The client's
    /// TokenProvider will hit yup-oauth2 — we shortcut that by mocking
    /// the token at the HTTP layer (the Authorization header is sent but
    /// our wiremock matcher just checks it exists; the token value is
    /// irrelevant for the API contract test).
    ///
    /// To avoid yup-oauth2 actually trying to authenticate, we override
    /// the token by setting a sentinel directly in the provider's cache
    /// (only possible via a test-only helper — see step 3).
    async fn setup_mock() -> (MockServer, GoogleSheetsHttpClient) {
        let server = MockServer::start().await;
        let client = GoogleSheetsHttpClient::for_tests(
            &server.uri(),
            &server.uri(),
            &server.uri(),
        );
        // Skip real auth — manually pre-fill the cache.
        client.token.set_token_for_test("fake-bearer-token").await;
        (server, client)
    }
```

This test won't compile yet — we need `set_token_for_test`. Add it to `TokenProvider` in `auth.rs`:

```rust
    /// Test-only: seed the cache with a known token so HTTP tests don't
    /// hit yup-oauth2. Available only under `#[cfg(test)]`.
    #[cfg(test)]
    pub async fn set_token_for_test(&self, token: impl Into<String>) {
        let mut cache = self.cache.lock().await;
        *cache = Some(CachedToken {
            token: token.into(),
            expires_at: Instant::now() + Duration::from_secs(60 * 60),
        });
    }
```

- [ ] **Step 3: Write the read_range test**

Append to the `tests` module in `http_client.rs`:

```rust
    #[tokio::test]
    async fn read_range_returns_2d_array_for_unformatted() {
        let (server, client) = setup_mock().await;

        Mock::given(method("GET"))
            .and(path_regex(r"/abc/values/Sheet1!A1:B2"))
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
        assert_eq!(
            resp.values,
            serde_json::json!([["x", 1], ["y", 2]])
        );
    }
```

- [ ] **Step 4: Run the test to confirm it fails (no `read_range` impl yet)**

```bash
cargo test -p colmena_dag_engine --lib gsheets::infrastructure::http_client 2>&1 | tail -20
```

Expected: compile error or test failure — the `SheetsClient for GoogleSheetsHttpClient` impl doesn't exist yet.

- [ ] **Step 5: Implement `read_range`**

Add to the `impl` block in `http_client.rs`:

```rust
impl GoogleSheetsHttpClient {
    /// Auth header + retry-aware GET. Returns the body as `serde_json::Value`.
    async fn get_json(&self, url: &str) -> Result<Value, SheetsError> {
        let token = self.token.token().await?;
        for attempt in 0..=self.max_retries {
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
                        let backoff = Duration::from_secs(1 << attempt);
                        tokio::time::sleep(backoff).await;
                        continue;
                    }
                    return Err(SheetsError::RateLimit(60));
                }
                s if s.is_server_error() => {
                    if attempt < self.max_retries {
                        let backoff = Duration::from_secs(1 << attempt);
                        tokio::time::sleep(backoff).await;
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
```

Then add the `SheetsClient` impl (just the `read_range` method for now — leave the others as `unimplemented!()`):

```rust
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

    // ── stubs for the rest of the trait (impl in Tasks 4 cont. + 5) ──
    async fn create_spreadsheet(&self, _title: &str) -> Result<SpreadsheetMeta, SheetsError> {
        unimplemented!("Task 5")
    }
    async fn create_from_xlsx(
        &self,
        _title: &str,
        _bytes: Vec<u8>,
    ) -> Result<SpreadsheetMeta, SheetsError> {
        unimplemented!("Task 5")
    }
    async fn export_xlsx(&self, _id: &SpreadsheetId) -> Result<Vec<u8>, SheetsError> {
        unimplemented!("Task 5")
    }
    async fn list_sheets(&self, _id: &SpreadsheetId) -> Result<Vec<SheetMeta>, SheetsError> {
        unimplemented!("Task 4 step 7")
    }
    async fn add_sheet(
        &self,
        _id: &SpreadsheetId,
        _name: &str,
    ) -> Result<SheetMeta, SheetsError> {
        unimplemented!("Task 5")
    }
    async fn delete_sheet(
        &self,
        _id: &SpreadsheetId,
        _name_or_sheet_id: &str,
    ) -> Result<(), SheetsError> {
        unimplemented!("Task 5")
    }
    async fn set_cell(
        &self,
        _id: &SpreadsheetId,
        _sheet: &str,
        _addr: &str,
        _value: CellValue,
    ) -> Result<(), SheetsError> {
        unimplemented!("Task 4 step 9")
    }
    async fn set_range(
        &self,
        _id: &SpreadsheetId,
        _sheet: &str,
        _start_addr: &str,
        _values_2d: Vec<Vec<CellValue>>,
    ) -> Result<SetRangeResponse, SheetsError> {
        unimplemented!("Task 4 step 11")
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
```

`urlencoding` is not currently a dep. Add to `src/libs/colmena/Cargo.toml`:

```toml
urlencoding = "2"
```

Actually wait — verify that's not already present:

```bash
grep "^urlencoding" src/libs/colmena/Cargo.toml
```

If empty, add it. If present, skip.

- [ ] **Step 6: Run the read_range test**

```bash
cargo test -p colmena_dag_engine --lib gsheets::infrastructure::http_client::tests::read_range_returns_2d_array 2>&1 | tail -10
```

Expected: PASS.

- [ ] **Step 7: Implement list_sheets + test**

Add a test:

```rust
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
```

Run, see it fail with `unimplemented!`, then replace the stub:

```rust
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
```

Run again, confirm PASS.

- [ ] **Step 8: Add a put_json helper (for set_cell + set_range)**

In the `impl GoogleSheetsHttpClient` block, add:

```rust
    async fn put_json(&self, url: &str, body: Value) -> Result<Value, SheetsError> {
        let token = self.token.token().await?;
        for attempt in 0..=self.max_retries {
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
                    // Heuristic: Google returns "Unable to parse range" for unknown sheets.
                    if body.contains("Unable to parse range") {
                        return Err(SheetsError::SheetNotFound(body));
                    }
                    return Err(SheetsError::InvalidRange(body));
                }
                StatusCode::TOO_MANY_REQUESTS => {
                    if attempt < self.max_retries {
                        tokio::time::sleep(Duration::from_secs(1 << attempt)).await;
                        continue;
                    }
                    return Err(SheetsError::RateLimit(60));
                }
                s if s.is_server_error() => {
                    if attempt < self.max_retries {
                        tokio::time::sleep(Duration::from_secs(1 << attempt)).await;
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
```

- [ ] **Step 9: Implement set_cell + test**

Test:

```rust
    #[tokio::test]
    async fn set_cell_sends_user_entered_value_input_option() {
        let (server, client) = setup_mock().await;
        Mock::given(method("PUT"))
            .and(path_regex(r"/abc/values/Sheet1!E1"))
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
```

Impl:

```rust
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
```

Run + verify PASS.

- [ ] **Step 10: Implement set_range + test**

Test:

```rust
    #[tokio::test]
    async fn set_range_writes_2d_block_and_returns_updated_cells() {
        let (server, client) = setup_mock().await;
        Mock::given(method("PUT"))
            .and(path_regex(r"/abc/values/Sheet1!A1"))
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
```

Impl:

```rust
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
            updated_cells: resp.get("updatedCells").and_then(Value::as_u64).unwrap_or(0),
            updated_range: resp
                .get("updatedRange")
                .and_then(Value::as_str)
                .unwrap_or(&range)
                .to_string(),
        })
    }
```

- [ ] **Step 11: Add error-mapping tests (404, 429, 401)**

Three tests in the same module:

```rust
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
        // Always 429 — client tries (1 + max_retries) = 2 times, then bails.
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
            .up_to_n_times(1) // first attempt
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "range": "Sheet1!A1",
                "values": [["ok"]],
            })))
            .mount(&server)
            .await;
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
```

- [ ] **Step 12: Uncomment http_client in `infrastructure/mod.rs`**

Change `// pub mod http_client;` back to `pub mod http_client;`.

- [ ] **Step 13: Run all gsheets tests + clippy**

```bash
cargo test -p colmena_dag_engine --lib gsheets 2>&1 | tail -25
cargo clippy -p colmena_dag_engine --lib --tests -- -D warnings 2>&1 | tail
```

Expected: 8-10 tests pass, zero clippy warnings.

- [ ] **Step 14: Commit**

```bash
git add src/libs/colmena/src/gsheets/ src/libs/colmena/Cargo.toml
git commit -m "$(cat <<'EOF'
feat(E-T4): GoogleSheetsHttpClient read/write endpoints

Implements read_range, list_sheets, set_cell, set_range against wiremock.
Auth header attached via TokenProvider; 401 triggers one refresh+retry;
429/5xx retry with exponential backoff; 404 → SpreadsheetNotFound;
400 with "Unable to parse range" → SheetNotFound, otherwise InvalidRange.
USER_ENTERED valueInputOption so formula strings get Google-side eval.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: HTTP client — admin endpoints (create / add_sheet / delete_sheet / xlsx upload+export)

**Files:**
- Modify: `src/libs/colmena/src/gsheets/infrastructure/http_client.rs`

- [ ] **Step 1: Implement create_spreadsheet + test**

Test:

```rust
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
        let meta = client.create_spreadsheet("My Sheet").await.expect("create ok");
        assert_eq!(meta.spreadsheet_id.0, "new_id");
        assert_eq!(meta.title, "My Sheet");
        assert!(meta.url.contains("new_id"));
        assert_eq!(meta.sheets.len(), 1);
    }
```

Add a `post_json` helper to `impl GoogleSheetsHttpClient` (parallel to `put_json`):

```rust
    async fn post_json(&self, url: &str, body: Value) -> Result<Value, SheetsError> {
        let token = self.token.token().await?;
        for attempt in 0..=self.max_retries {
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
                StatusCode::FORBIDDEN => return Err(SheetsError::PermissionDenied(String::new())),
                StatusCode::NOT_FOUND => return Err(SheetsError::SpreadsheetNotFound(url.into())),
                StatusCode::TOO_MANY_REQUESTS => {
                    if attempt < self.max_retries {
                        tokio::time::sleep(Duration::from_secs(1 << attempt)).await;
                        continue;
                    }
                    return Err(SheetsError::RateLimit(60));
                }
                s if s.is_server_error() => {
                    if attempt < self.max_retries {
                        tokio::time::sleep(Duration::from_secs(1 << attempt)).await;
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
        Err(SheetsError::Http("retries exhausted".into()))
    }

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
                            sheet_id: SheetId(props.get("sheetId").and_then(Value::as_i64).unwrap_or(0)),
                            title: props.get("title").and_then(Value::as_str).unwrap_or("").to_string(),
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
```

Replace the `create_spreadsheet` stub:

```rust
    async fn create_spreadsheet(&self, title: &str) -> Result<SpreadsheetMeta, SheetsError> {
        let url = self.sheets_base.clone();
        let body = serde_json::json!({
            "properties": {"title": title},
        });
        let resp = self.post_json(&url, body).await?;
        Self::parse_meta(&resp, None)
    }
```

Run + PASS.

- [ ] **Step 2: Implement add_sheet + test**

Test:

```rust
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
```

Impl:

```rust
    async fn add_sheet(
        &self,
        id: &SpreadsheetId,
        name: &str,
    ) -> Result<SheetMeta, SheetsError> {
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
            title: props.get("title").and_then(Value::as_str).unwrap_or("").to_string(),
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
```

- [ ] **Step 3: Implement delete_sheet + test**

`delete_sheet` accepts either a title or a numeric sheet id. When the input parses as i64, use it directly; otherwise resolve via `list_sheets` first.

Test:

```rust
    #[tokio::test]
    async fn delete_sheet_by_numeric_id() {
        let (server, client) = setup_mock().await;
        Mock::given(method("POST"))
            .and(path_regex(r"/abc:batchUpdate"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"replies":[]})))
            .mount(&server)
            .await;
        client
            .delete_sheet(&SpreadsheetId("abc".into()), "999")
            .await
            .expect("delete ok");
    }
```

Impl:

```rust
    async fn delete_sheet(
        &self,
        id: &SpreadsheetId,
        name_or_sheet_id: &str,
    ) -> Result<(), SheetsError> {
        let sheet_id: i64 = match name_or_sheet_id.parse::<i64>() {
            Ok(n) => n,
            Err(_) => {
                // Resolve by title.
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
```

- [ ] **Step 4: Implement create_from_xlsx (Drive API multipart upload) + test**

Test:

```rust
    #[tokio::test]
    async fn create_from_xlsx_uploads_via_drive_and_returns_meta() {
        let (server, client) = setup_mock().await;
        // Drive POST returns just the file id; we then GET the sheets meta separately.
        Mock::given(method("POST"))
            .and(path_regex(r"/upload/drive/v3/files|/v3/files|/files"))
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
```

Impl. Drive API multipart uploads use `multipart/related` with two parts: metadata (JSON) and body (binary). `reqwest` doesn't have a high-level `multipart/related` helper, so we craft it manually:

```rust
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
        body.extend_from_slice(
            b"Content-Type: application/json; charset=UTF-8\r\n\r\n",
        );
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
                    SheetsError::PermissionDenied(body)
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

        // Fetch the Sheets metadata so we can return SpreadsheetMeta with sheets[].
        let meta_url = format!("{}/{}", self.sheets_base, new_id);
        let meta_resp = self.get_json(&meta_url).await?;
        Self::parse_meta(&meta_resp, Some(&new_id))
    }
```

`uuid` is a dep — confirm:

```bash
grep "^uuid" src/libs/colmena/Cargo.toml
```

Should already be there per CLAUDE.md (it's used for tool call ids etc.). If not, add `uuid = { version = "1", features = ["v4"] }`.

Run the test, confirm PASS.

- [ ] **Step 5: Implement export_xlsx + test**

Test:

```rust
    #[tokio::test]
    async fn export_xlsx_returns_binary_bytes() {
        let (server, client) = setup_mock().await;
        Mock::given(method("GET"))
            .and(path_regex(r"/files/abc/export"))
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
```

Impl:

```rust
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
                StatusCode::FORBIDDEN => SheetsError::PermissionDenied(String::new()),
                _ => SheetsError::Http(format!("export {status}: {body}")),
            });
        }
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| SheetsError::Http(format!("export bytes: {e}")))?;
        Ok(bytes.to_vec())
    }
```

- [ ] **Step 6: Run all gsheets tests**

```bash
cargo test -p colmena_dag_engine --lib gsheets 2>&1 | tail -25
cargo clippy -p colmena_dag_engine --lib --tests -- -D warnings 2>&1 | tail
```

Expected: ~14 tests pass, zero warnings.

- [ ] **Step 7: Commit**

```bash
git add src/libs/colmena/src/gsheets/infrastructure/http_client.rs
git commit -m "$(cat <<'EOF'
feat(E-T5): gsheets admin endpoints + xlsx upload/export

Creates spreadsheets, adds/deletes tabs (delete accepts title or numeric
sheet id), uploads .xlsx as a native Google Sheet via Drive
multipart/related, downloads as .xlsx via Drive export. parse_meta
helper shared between create_spreadsheet and create_from_xlsx.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: Tool dispatchers (9 tools) + tool definitions

**Files:**
- Create: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gsheets_tools.rs`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs` (add `pub mod gsheets_tools;` + re-exports)

This file ends up ~600 LOC. We build it incrementally — one tool per step group — so each commit is small.

- [ ] **Step 1: Scaffold the module + Args structs for all 9 tools**

Create `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gsheets_tools.rs`:

```rust
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

use crate::gsheets::domain::{
    CellValue, ReadOptions, SheetsClient, SheetsError, SpreadsheetId, ValueRenderOption,
};
use crate::gsheets::infrastructure::config::GSheetsConfig;
use crate::gsheets::infrastructure::http_client::GoogleSheetsHttpClient;
use crate::llm::domain::tools::ToolDefinition;
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
    GoogleSheetsHttpClient::from_config(&GSheetsConfig::from_env())
        .map_err(error_to_json)
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
    super::build_synthetic_tool::<CreateSpreadsheetArgs>(
        TOOL_CREATE_SPREADSHEET,
        "Create a new empty Google Spreadsheet with the given title. \
         Returns {spreadsheet_id, url} you can use in subsequent calls.",
    )
}

pub fn tool_create_from_xlsx() -> ToolDefinition {
    super::build_synthetic_tool::<CreateFromXlsxArgs>(
        TOOL_CREATE_FROM_XLSX,
        "Upload an .xlsx attachment as a NATIVE Google Spreadsheet \
         (auto-conversion via Drive API). attachment_id refers to a \
         file already present in the run's attachment store. Returns \
         {spreadsheet_id, url, sheets[]} with the converted tabs.",
    )
}

pub fn tool_export_xlsx() -> ToolDefinition {
    super::build_synthetic_tool::<ExportXlsxArgs>(
        TOOL_EXPORT_XLSX,
        "Download a Google Spreadsheet as .xlsx and register it as an \
         attachment. Returns {attachment_id}.",
    )
}

pub fn tool_list_sheets() -> ToolDefinition {
    super::build_synthetic_tool::<ListSheetsArgs>(
        TOOL_LIST_SHEETS,
        "List the tabs (sheets) inside a Google Spreadsheet. Returns \
         {sheets: [{sheet_id, title, index, row_count, col_count}]}.",
    )
}

pub fn tool_add_sheet() -> ToolDefinition {
    super::build_synthetic_tool::<AddSheetArgs>(
        TOOL_ADD_SHEET,
        "Add a new tab (sheet) to an existing Google Spreadsheet. \
         Returns {sheet_id, title}.",
    )
}

pub fn tool_delete_sheet() -> ToolDefinition {
    super::build_synthetic_tool::<DeleteSheetArgs>(
        TOOL_DELETE_SHEET,
        "Delete a tab from a Google Spreadsheet. `sheet` accepts \
         either the human title or a stringified numeric sheet_id.",
    )
}

pub fn tool_read() -> ToolDefinition {
    super::build_synthetic_tool::<ReadArgs>(
        TOOL_READ,
        "Read cells from a tab. Defaults: value_render=UNFORMATTED_VALUE \
         (pandas-friendly scalars), as_records=false (2D array). Set \
         value_render='FORMULA' to read formula text instead of \
         evaluated values. Set as_records=true to use the first row as \
         headers and return [{col: val, ...}].",
    )
}

pub fn tool_set_cell() -> ToolDefinition {
    super::build_synthetic_tool::<SetCellArgs>(
        TOOL_SET_CELL,
        "Write a single cell. Strings starting with `=` are evaluated \
         by Google as formulas (USER_ENTERED valueInputOption). \
         Cascading recalc happens server-side automatically.",
    )
}

pub fn tool_set_range() -> ToolDefinition {
    super::build_synthetic_tool::<SetRangeArgs>(
        TOOL_SET_RANGE,
        "Bulk-write a rectangular block of cells starting at start_addr. \
         values_2d is a row-major 2D array. Strings starting with `=` \
         are evaluated by Google as formulas.",
    )
}
```

(Don't add dispatchers yet — those come in steps 2-10 to keep test cycles short.)

- [ ] **Step 2: Add `pub mod gsheets_tools;` to `mod.rs`**

In `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs`:

Find the list of `pub mod foo;` declarations and add:
```rust
pub mod gsheets_tools;
```

At the bottom where re-exports live (look for the section near `pub use crdt_doc_tools::{...}`), add:

```rust
pub use gsheets_tools::{
    tool_add_sheet as gsheets_tool_add_sheet,
    tool_create_from_xlsx as gsheets_tool_create_from_xlsx,
    tool_create_spreadsheet as gsheets_tool_create_spreadsheet,
    tool_delete_sheet as gsheets_tool_delete_sheet,
    tool_export_xlsx as gsheets_tool_export_xlsx,
    tool_list_sheets as gsheets_tool_list_sheets,
    tool_read as gsheets_tool_read,
    tool_set_cell as gsheets_tool_set_cell,
    tool_set_range as gsheets_tool_set_range,
    TOOL_ADD_SHEET as GSHEETS_ADD_SHEET_TOOL,
    TOOL_CREATE_FROM_XLSX as GSHEETS_CREATE_FROM_XLSX_TOOL,
    TOOL_CREATE_SPREADSHEET as GSHEETS_CREATE_SPREADSHEET_TOOL,
    TOOL_DELETE_SHEET as GSHEETS_DELETE_SHEET_TOOL,
    TOOL_EXPORT_XLSX as GSHEETS_EXPORT_XLSX_TOOL,
    TOOL_LIST_SHEETS as GSHEETS_LIST_SHEETS_TOOL,
    TOOL_READ as GSHEETS_READ_TOOL,
    TOOL_SET_CELL as GSHEETS_SET_CELL_TOOL,
    TOOL_SET_RANGE as GSHEETS_SET_RANGE_TOOL,
};
```

Run `cargo check -p colmena_dag_engine --lib` — should compile (we only added definitions; no dispatchers used yet).

- [ ] **Step 3: Add dispatcher functions (one block, after the tool defs in gsheets_tools.rs)**

```rust
// ── Dispatchers ───────────────────────────────────────────────────────

pub async fn dispatch_create_spreadsheet(args: serde_json::Value) -> serde_json::Value {
    let parsed: CreateSpreadsheetArgs = match serde_json::from_value(args) {
        Ok(a) => a,
        Err(e) => return serde_json::json!({"error": "invalid_args", "message": e.to_string()}),
    };
    let client = match build_client() {
        Ok(c) => c,
        Err(e) => return e,
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

pub async fn dispatch_list_sheets(args: serde_json::Value) -> serde_json::Value {
    let parsed: ListSheetsArgs = match serde_json::from_value(args) {
        Ok(a) => a,
        Err(e) => return serde_json::json!({"error": "invalid_args", "message": e.to_string()}),
    };
    let client = match build_client() {
        Ok(c) => c,
        Err(e) => return e,
    };
    match client.list_sheets(&SpreadsheetId(parsed.spreadsheet_id)).await {
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

pub async fn dispatch_add_sheet(args: serde_json::Value) -> serde_json::Value {
    let parsed: AddSheetArgs = match serde_json::from_value(args) {
        Ok(a) => a,
        Err(e) => return serde_json::json!({"error": "invalid_args", "message": e.to_string()}),
    };
    let client = match build_client() {
        Ok(c) => c,
        Err(e) => return e,
    };
    match client.add_sheet(&SpreadsheetId(parsed.spreadsheet_id), &parsed.name).await {
        Ok(meta) => serde_json::json!({
            "ok": true,
            "sheet_id": meta.sheet_id.0,
            "title": meta.title,
        }),
        Err(e) => error_to_json(e),
    }
}

pub async fn dispatch_delete_sheet(args: serde_json::Value) -> serde_json::Value {
    let parsed: DeleteSheetArgs = match serde_json::from_value(args) {
        Ok(a) => a,
        Err(e) => return serde_json::json!({"error": "invalid_args", "message": e.to_string()}),
    };
    let client = match build_client() {
        Ok(c) => c,
        Err(e) => return e,
    };
    match client.delete_sheet(&SpreadsheetId(parsed.spreadsheet_id), &parsed.sheet).await {
        Ok(_) => serde_json::json!({"ok": true}),
        Err(e) => error_to_json(e),
    }
}

pub async fn dispatch_read(args: serde_json::Value) -> serde_json::Value {
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
    let client = match build_client() {
        Ok(c) => c,
        Err(e) => return e,
    };
    let opts = ReadOptions {
        value_render: parse_value_render(parsed.value_render.as_deref()),
        as_records: parsed.as_records,
    };
    match client
        .read_range(&SpreadsheetId(parsed.spreadsheet_id), &parsed.sheet, range.as_deref(), opts)
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

pub async fn dispatch_set_cell(args: serde_json::Value) -> serde_json::Value {
    let parsed: SetCellArgs = match serde_json::from_value(args) {
        Ok(a) => a,
        Err(e) => return serde_json::json!({"error": "invalid_args", "message": e.to_string()}),
    };
    let client = match build_client() {
        Ok(c) => c,
        Err(e) => return e,
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

pub async fn dispatch_set_range(args: serde_json::Value) -> serde_json::Value {
    let parsed: SetRangeArgs = match serde_json::from_value(args) {
        Ok(a) => a,
        Err(e) => return serde_json::json!({"error": "invalid_args", "message": e.to_string()}),
    };
    let client = match build_client() {
        Ok(c) => c,
        Err(e) => return e,
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
```

`dispatch_create_from_xlsx` and `dispatch_export_xlsx` need attachment-store access — they go in the next step.

- [ ] **Step 4: Add xlsx dispatchers (attachment-bound)**

Read how `crdt_doc_import_sheet`'s dispatcher resolves an attachment id:

```bash
grep -n "attachment_id\|load_attachment\|fetch_attachment" src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/crdt_doc_import_sheet.rs 2>&1 | head
```

Mirror whatever helper it uses. The expected pattern is something like
`ctx.load_attachment(&attachment_id).await -> Result<Vec<u8>, ...>` from a passed-in `AttachmentResolver` trait or similar. If the helper requires a context object the dispatcher doesn't have, **for v1 take the resolver as a parameter** — the dispatch arm in `dag_tool_executor.rs` (Task 7) constructs and passes it.

Provisional sketch (adapt to the actual resolver type — if you can't find one, drop these two tools to a follow-up E-T6b and proceed):

```rust
pub async fn dispatch_create_from_xlsx(
    args: serde_json::Value,
    attachment_bytes: Vec<u8>,  // resolved by the caller
) -> serde_json::Value {
    let parsed: CreateFromXlsxArgs = match serde_json::from_value(args) {
        Ok(a) => a,
        Err(e) => return serde_json::json!({"error": "invalid_args", "message": e.to_string()}),
    };
    let client = match build_client() {
        Ok(c) => c,
        Err(e) => return e,
    };
    match client.create_from_xlsx(&parsed.title, attachment_bytes).await {
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

pub async fn dispatch_export_xlsx(
    args: serde_json::Value,
    register_attachment: impl FnOnce(Vec<u8>, &str) -> String,
) -> serde_json::Value {
    let parsed: ExportXlsxArgs = match serde_json::from_value(args) {
        Ok(a) => a,
        Err(e) => return serde_json::json!({"error": "invalid_args", "message": e.to_string()}),
    };
    let client = match build_client() {
        Ok(c) => c,
        Err(e) => return e,
    };
    match client.export_xlsx(&SpreadsheetId(parsed.spreadsheet_id.clone())).await {
        Ok(bytes) => {
            let suggested = format!("{}.xlsx", parsed.spreadsheet_id);
            let id = register_attachment(bytes, &suggested);
            serde_json::json!({"ok": true, "attachment_id": id})
        }
        Err(e) => error_to_json(e),
    }
}
```

This shape (resolver as a callback param) keeps the dispatcher pure-functional and the actual attachment plumbing localized to Task 7.

- [ ] **Step 5: Add 2-3 unit tests for the dispatchers (mock client)**

To test dispatchers without hitting Google we need to either:
- Inject a `Box<dyn SheetsClient>` (adds plumbing), or
- Override `build_client` (not test-friendly).

Simpler approach: write the dispatchers around `&dyn SheetsClient` from the start. Refactor:

```rust
pub async fn dispatch_list_sheets_with_client(
    args: serde_json::Value,
    client: &dyn SheetsClient,
) -> serde_json::Value {
    // ... same body as dispatch_list_sheets but uses `client` instead of `build_client()`
}

pub async fn dispatch_list_sheets(args: serde_json::Value) -> serde_json::Value {
    let client = match build_client() {
        Ok(c) => c,
        Err(e) => return e,
    };
    dispatch_list_sheets_with_client(args, &client).await
}
```

Do this for all 7 dispatchers that take no extra params (skip the xlsx pair). Then unit-test the `_with_client` variants:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::gsheets::domain::{
        ReadResponse, SetRangeResponse, SheetMeta, SheetId, SpreadsheetMeta,
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
        async fn create_from_xlsx(&self, _t: &str, _b: Vec<u8>) -> Result<SpreadsheetMeta, SheetsError> {
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
        let result = dispatch_list_sheets_with_client(
            serde_json::json!({}),
            &FakeClient,
        )
        .await;
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
```

- [ ] **Step 6: Build + tests**

```bash
cargo test -p colmena_dag_engine --lib gsheets_tools 2>&1 | tail -15
cargo clippy -p colmena_dag_engine --lib --tests -- -D warnings 2>&1 | tail
```

Expected: 4 tests pass, clippy clean.

- [ ] **Step 7: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gsheets_tools.rs \
        src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs
git commit -m "$(cat <<'EOF'
feat(E-T6): gsheets tool dispatchers + tool definitions

9 tool definitions + dispatchers. _with_client variants for unit-testing
without HTTP; the public variants build a real HTTP client via env config.
Args structs include UX aliases (address ↔ addr, start ↔ start_addr,
values ↔ values_2d, name ↔ sheet) — same lessons from D-T16. Error
mapping returns {error, message} JSON shapes for the agent.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 7: Wire dispatchers into the tool executor router

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs`

- [ ] **Step 1: Locate the existing CRDT dispatch block**

Run:

```bash
grep -n "CRDT_DOC_LIST_SHEETS_TOOL\|dispatch_crdt_doc_list_sheets" src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs | head
```

You should see a block around lines 714-760 with a `use` import and a `match` arm dispatching to `dispatch_crdt_doc_*`. The gsheets arm goes immediately after.

- [ ] **Step 2: Add the gsheets dispatch block**

Right after the closing brace of the CRDT block (look for `}` that closes the `match` block on `n if n == CRDT_DOC_*`), add:

```rust
        // --- Synthetic gsheets tools (gsheets_*) ---
        {
            use crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::gsheets_tools::{
                dispatch_add_sheet, dispatch_create_spreadsheet, dispatch_delete_sheet,
                dispatch_list_sheets, dispatch_read, dispatch_set_cell, dispatch_set_range,
            };
            use crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::{
                GSHEETS_ADD_SHEET_TOOL, GSHEETS_CREATE_SPREADSHEET_TOOL,
                GSHEETS_DELETE_SHEET_TOOL, GSHEETS_LIST_SHEETS_TOOL, GSHEETS_READ_TOOL,
                GSHEETS_SET_CELL_TOOL, GSHEETS_SET_RANGE_TOOL,
            };
            if let Some(name) = tool_name.as_deref() {
                match name {
                    n if n == GSHEETS_CREATE_SPREADSHEET_TOOL => {
                        let args = parsed_args.unwrap_or(serde_json::Value::Null);
                        let result = dispatch_create_spreadsheet(args).await;
                        return Ok(json_to_tool_result(&result));
                    }
                    n if n == GSHEETS_LIST_SHEETS_TOOL => {
                        let result = dispatch_list_sheets(
                            parsed_args.unwrap_or(serde_json::Value::Null),
                        )
                        .await;
                        return Ok(json_to_tool_result(&result));
                    }
                    n if n == GSHEETS_ADD_SHEET_TOOL => {
                        let result =
                            dispatch_add_sheet(parsed_args.unwrap_or(serde_json::Value::Null)).await;
                        return Ok(json_to_tool_result(&result));
                    }
                    n if n == GSHEETS_DELETE_SHEET_TOOL => {
                        let result = dispatch_delete_sheet(
                            parsed_args.unwrap_or(serde_json::Value::Null),
                        )
                        .await;
                        return Ok(json_to_tool_result(&result));
                    }
                    n if n == GSHEETS_READ_TOOL => {
                        let result =
                            dispatch_read(parsed_args.unwrap_or(serde_json::Value::Null)).await;
                        return Ok(json_to_tool_result(&result));
                    }
                    n if n == GSHEETS_SET_CELL_TOOL => {
                        let result =
                            dispatch_set_cell(parsed_args.unwrap_or(serde_json::Value::Null))
                                .await;
                        return Ok(json_to_tool_result(&result));
                    }
                    n if n == GSHEETS_SET_RANGE_TOOL => {
                        let result =
                            dispatch_set_range(parsed_args.unwrap_or(serde_json::Value::Null))
                                .await;
                        return Ok(json_to_tool_result(&result));
                    }
                    _ => {} // fall through
                }
            }
        }
```

(`parsed_args` / `tool_name` / `json_to_tool_result` are existing variables in surrounding scope; if their names differ, adapt — the goal is to call the dispatcher with the parsed JSON args and wrap the result in the executor's expected `ToolResult` type.)

For the two xlsx dispatchers (`dispatch_create_from_xlsx`, `dispatch_export_xlsx`), they need attachment plumbing. Find how `crdt_doc_import_sheet` resolves its `attachment_id` in the same file — typically via an `AttachmentResolver` available on the executor's context. Adapt:

```rust
                    n if n == GSHEETS_CREATE_FROM_XLSX_TOOL => {
                        let args = parsed_args.unwrap_or(serde_json::Value::Null);
                        let attachment_id = args.get("attachment_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let bytes = match resolve_attachment_bytes(attachment_id).await {
                            Ok(b) => b,
                            Err(msg) => return Ok(json_to_tool_result(
                                &serde_json::json!({"error": "attachment_not_found", "message": msg})
                            )),
                        };
                        let result = dispatch_create_from_xlsx(args, bytes).await;
                        return Ok(json_to_tool_result(&result));
                    }
                    n if n == GSHEETS_EXPORT_XLSX_TOOL => {
                        let args = parsed_args.unwrap_or(serde_json::Value::Null);
                        let result = dispatch_export_xlsx(args, |bytes, suggested| {
                            register_attachment_bytes(bytes, suggested) // returns new attachment_id
                        })
                        .await;
                        return Ok(json_to_tool_result(&result));
                    }
```

`resolve_attachment_bytes` and `register_attachment_bytes` are illustrative names. **Read the actual helpers used by `crdt_doc_import_sheet` and `crdt_doc_export_xlsx` (or equivalent) and reuse them.** If neither tool has a counterpart in this file, you may need to thread an `AttachmentRegistry` through the executor — flag as a deviation and defer the two xlsx dispatchers to a follow-up E-T7b task.

- [ ] **Step 3: Verify build**

```bash
cargo check -p colmena_dag_engine --lib 2>&1 | tail
cargo test -p colmena_dag_engine --lib gsheets 2>&1 | tail -5
```

Expected: clean build, all existing gsheets unit tests still pass.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs
git commit -m "$(cat <<'EOF'
feat(E-T7): wire gsheets_* dispatchers into the tool executor router

Routes the 9 gsheets_* tool names to their dispatchers, mirroring the
existing crdt_doc_* dispatch block. Attachment-bound dispatchers
(create_from_xlsx, export_xlsx) reuse the same attachment resolver
crdt_doc_import_sheet uses.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 8: Register the 9 tools in the registry

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/registry.rs`

- [ ] **Step 1: Find how `crdt_doc_*` tools register**

```bash
grep -n "crdt_doc\|register\|llm_call\|build_all_crdt_doc_tools" src/libs/colmena/src/dag_engine/infrastructure/registry.rs | head
```

Most likely the registry doesn't list synthetic tools directly — `llm_call.rs` builds the tool list from `enabled_tools` + the synthetic-tool registry of names. Verify by searching:

```bash
grep -rn "CRDT_DOC_LIST_SHEETS_TOOL\|enabled_tools" src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs | head
```

You should find a `match` / `if` that pushes a `ToolDefinition` for each named tool. Add gsheets cases to that list.

- [ ] **Step 2: Add gsheets arms to llm.rs's tool-registration loop**

Open `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs`. Find the place where `enabled_tools` strings are matched against the synthetic tool constants. Add:

```rust
        n if n == GSHEETS_CREATE_SPREADSHEET_TOOL => Some(gsheets_tool_create_spreadsheet()),
        n if n == GSHEETS_CREATE_FROM_XLSX_TOOL => Some(gsheets_tool_create_from_xlsx()),
        n if n == GSHEETS_EXPORT_XLSX_TOOL => Some(gsheets_tool_export_xlsx()),
        n if n == GSHEETS_LIST_SHEETS_TOOL => Some(gsheets_tool_list_sheets()),
        n if n == GSHEETS_ADD_SHEET_TOOL => Some(gsheets_tool_add_sheet()),
        n if n == GSHEETS_DELETE_SHEET_TOOL => Some(gsheets_tool_delete_sheet()),
        n if n == GSHEETS_READ_TOOL => Some(gsheets_tool_read()),
        n if n == GSHEETS_SET_CELL_TOOL => Some(gsheets_tool_set_cell()),
        n if n == GSHEETS_SET_RANGE_TOOL => Some(gsheets_tool_set_range()),
```

with the appropriate `use` imports at the top of llm.rs.

- [ ] **Step 3: Build + run all lib tests**

```bash
cargo test -p colmena_dag_engine --lib 2>&1 | tail
cargo clippy -p colmena_dag_engine --lib --tests -- -D warnings 2>&1 | tail
```

Expected: still ~1305 tests passing (1295 prior + ~10 new from this subsystem), zero warnings.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs
git commit -m "$(cat <<'EOF'
feat(E-T8): register gsheets_* tools in llm.rs's enabled_tools dispatch

Each of the 9 gsheets tools can now be enabled per-graph via
`enabled_tools: ["gsheets_read", "gsheets_set_cell", ...]`.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 9: Integration test against real API (`#[ignore]`-gated)

**Files:**
- Create: `src/libs/colmena/tests/gsheets_integration_test.rs`

- [ ] **Step 1: Write the integration test scaffolding**

```rust
//! Integration test for Google Sheets — hits the real API. Gated by
//! `#[ignore]` so it doesn't run in CI without explicit opt-in.
//!
//! Run locally:
//!   source .env  # must define GOOGLE_APPLICATION_CREDENTIALS and
//!                #                COLMENA_GSHEETS_TEST_SPREADSHEET_ID
//!   cargo test --test gsheets_integration_test -- --ignored --nocapture

use colmena::gsheets::domain::{CellValue, ReadOptions, SheetsClient, SpreadsheetId, ValueRenderOption};
use colmena::gsheets::infrastructure::config::GSheetsConfig;
use colmena::gsheets::infrastructure::http_client::GoogleSheetsHttpClient;

fn test_id() -> SpreadsheetId {
    SpreadsheetId(
        std::env::var("COLMENA_GSHEETS_TEST_SPREADSHEET_ID")
            .expect("COLMENA_GSHEETS_TEST_SPREADSHEET_ID required for integration test"),
    )
}

fn client() -> GoogleSheetsHttpClient {
    GoogleSheetsHttpClient::from_config(&GSheetsConfig::from_env()).expect("build client")
}

#[tokio::test]
#[ignore = "requires GOOGLE_APPLICATION_CREDENTIALS + COLMENA_GSHEETS_TEST_SPREADSHEET_ID"]
async fn add_write_read_delete_sheet_round_trip() {
    let c = client();
    let id = test_id();
    let tab = format!("e2e_{}", uuid::Uuid::new_v4().simple());
    let added = c.add_sheet(&id, &tab).await.expect("add ok");
    assert_eq!(added.title, tab);

    c.set_cell(&id, &tab, "A1", CellValue::Number(42.0))
        .await
        .expect("set ok");
    let r = c.read_range(&id, &tab, Some("A1"), ReadOptions::default())
        .await
        .expect("read ok");
    assert_eq!(r.values, serde_json::json!([[42.0]]));

    c.delete_sheet(&id, &tab).await.expect("delete ok");
}

#[tokio::test]
#[ignore = "requires GOOGLE_APPLICATION_CREDENTIALS + COLMENA_GSHEETS_TEST_SPREADSHEET_ID"]
async fn formula_evaluated_server_side() {
    let c = client();
    let id = test_id();
    let tab = format!("formula_{}", uuid::Uuid::new_v4().simple());
    c.add_sheet(&id, &tab).await.expect("add");
    c.set_range(
        &id,
        &tab,
        "A1",
        vec![
            vec![CellValue::Number(10.0)],
            vec![CellValue::Number(20.0)],
            vec![CellValue::Number(30.0)],
        ],
    )
    .await
    .expect("seed");
    c.set_cell(&id, &tab, "B1", CellValue::String("=SUM(A1:A3)".to_string()))
        .await
        .expect("write formula");

    // Read as evaluated number.
    let r = c.read_range(
        &id,
        &tab,
        Some("B1"),
        ReadOptions { value_render: ValueRenderOption::UnformattedValue, as_records: false },
    )
    .await
    .expect("read evaluated");
    assert_eq!(r.values, serde_json::json!([[60.0]]));

    // Read as formula text.
    let f = c.read_range(
        &id,
        &tab,
        Some("B1"),
        ReadOptions { value_render: ValueRenderOption::Formula, as_records: false },
    )
    .await
    .expect("read formula");
    assert_eq!(f.values, serde_json::json!([["=SUM(A1:A3)"]]));

    c.delete_sheet(&id, &tab).await.expect("cleanup");
}

#[tokio::test]
#[ignore = "requires GOOGLE_APPLICATION_CREDENTIALS + COLMENA_GSHEETS_TEST_SPREADSHEET_ID"]
async fn spreadsheet_not_found_for_bogus_id() {
    let c = client();
    let result = c.list_sheets(&SpreadsheetId("totally-bogus-id-xxxxxx".to_string())).await;
    assert!(matches!(
        result,
        Err(colmena::gsheets::domain::SheetsError::SpreadsheetNotFound(_))
            | Err(colmena::gsheets::domain::SheetsError::Http(_))
    ));
}
```

- [ ] **Step 2: Verify the test BUILDS (compiles) but is skipped by default**

```bash
cargo test --test gsheets_integration_test 2>&1 | tail -10
```

Expected: 3 tests, 3 ignored, 0 failed (the `#[ignore]` gate prevents execution).

- [ ] **Step 3: Document the env vars needed**

Add to the test file's top doc-comment:

```rust
//! Required env:
//!   GOOGLE_APPLICATION_CREDENTIALS=/path/to/sa.json
//!   COLMENA_GSHEETS_TEST_SPREADSHEET_ID=<id of an empty test sheet>
//!
//! The test sheet must be shared with the SA email (read+edit). The
//! tests create temporary tabs prefixed "e2e_" or "formula_" and delete
//! them at the end.
```

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/tests/gsheets_integration_test.rs
git commit -m "$(cat <<'EOF'
test(E-T9): integration test against real Google Sheets API

3 #[ignore]-gated tests:
- add_write_read_delete_sheet_round_trip
- formula_evaluated_server_side (sets =SUM, reads number, reads formula)
- spreadsheet_not_found_for_bogus_id

Requires GOOGLE_APPLICATION_CREDENTIALS + COLMENA_GSHEETS_TEST_SPREADSHEET_ID.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 10: Smoke graph

**Files:**
- Create: `tests/graphs/agents/gsheets_smoke.json`

- [ ] **Step 1: Pattern the smoke graph after an existing one**

```bash
ls tests/graphs/agents/ | grep crdt
# pick one like crdt_doc_formulas.json as the template
cat tests/graphs/crdt_documents/d_formulas_smoke.json | head -50
```

- [ ] **Step 2: Write the smoke graph**

`tests/graphs/agents/gsheets_smoke.json`:

```json
{
  "_comment": "Subsistema E smoke — Google Sheets. The agent creates a spreadsheet, adds a tab, seeds 5 numbers, writes a SUM formula, reads back both evaluated value and formula text. Requires GOOGLE_APPLICATION_CREDENTIALS env. Run: `set -a; source .env; set +a; cargo run --bin dag_engine -- run tests/graphs/agents/gsheets_smoke.json --agent-session-id e_smoke_001 --include-extra-info`",
  "nodes": {
    "trigger": {
      "type": "trigger_webhook",
      "config": {
        "path": "/e-smoke",
        "method": "POST",
        "test_payload": {
          "prompt": "Run a Google Sheets smoke:\n1. Create a new spreadsheet titled 'Colmena E Smoke <UTC timestamp>'. Capture its spreadsheet_id and url.\n2. Add a tab named 'Numbers'.\n3. Use set_range starting at A1 with values_2d=[[10],[20],[30],[40],[50]].\n4. Set B1 to formula =SUM(A1:A5).\n5. Read B1 with value_render='UNFORMATTED_VALUE' — confirm it's 150.\n6. Read B1 with value_render='FORMULA' — confirm it's '=SUM(A1:A5)'.\n7. Final report (under 200 words): the spreadsheet url + the value/formula you read."
        }
      }
    },
    "agent": {
      "type": "llm_call",
      "config": {
        "provider": "google",
        "api_key": "${GEMINI_API_KEY}",
        "model": "gemini-2.5-flash",
        "stream": false,
        "max_iterations": 15,
        "lazy_tool_loading": true,
        "connection_url": "${DATABASE_URL}",
        "system_message": "You are an assistant exercising the Google Sheets tool surface. Use gsheets_* tools. Report what you observed at each step (cell counts, formula text, evaluated values).",
        "enabled_tools": [
          "gsheets_create_spreadsheet",
          "gsheets_add_sheet",
          "gsheets_set_cell",
          "gsheets_set_range",
          "gsheets_read"
        ]
      }
    },
    "log": { "type": "log" }
  },
  "edges": [
    { "from": "trigger", "to": "agent" },
    { "from": "agent", "to": "log" }
  ]
}
```

- [ ] **Step 3: Run the smoke (requires real auth)**

```bash
set -a; source .env; set +a
mkdir -p /tmp/colmena_e2e
cargo run --release --bin dag_engine -- run tests/graphs/agents/gsheets_smoke.json \
  --agent-session-id e_smoke_$(date +%s) \
  --include-extra-info 2>&1 | tee /tmp/colmena_e2e/e_smoke.sse
```

Expected: agent runs all 7 steps, B1 reads back as 150 then as "=SUM(A1:A5)". Final SSE event has `finishReason: stop`.

If the run fails:
- `gsheets_not_configured` → user needs to set GOOGLE_APPLICATION_CREDENTIALS (or run `gcloud auth application-default login`).
- `permission_denied` → service account doesn't have permission to create spreadsheets. (Default ADC user creds should work.)

If you can't get real creds locally, **this step is optional** — leave the smoke graph committed; the integration test from E-T9 covers the API surface.

- [ ] **Step 4: Commit (graph file always; SSE only if you ran it locally)**

```bash
git add tests/graphs/agents/gsheets_smoke.json
git commit -m "$(cat <<'EOF'
test(E-T10): smoke graph for Google Sheets tool surface

7-step agent flow: create spreadsheet, add tab, seed numbers, write
formula, read evaluated value, read formula text. Validates the full
gsheets_* surface end-to-end against the real API.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 11: Skill `gsheets-cross-sheet-analysis`

**Files:**
- Create: `src/libs/colmena/skills/gsheets-cross-sheet-analysis/SKILL.md`
- Create: `src/libs/colmena/skills/gsheets-cross-sheet-analysis/references/*.md` (6 files)

The 6 pattern reference files mirror F's `crdt-doc-cross-sheet-analysis` with mechanical find-and-replace: `crdt_doc_*` → `gsheets_*`, add `spreadsheet_id` to args, drop `sheet_id` (use plain `sheet` title), and use `gsheets_read(..., as_records=true)` instead of `crdt_doc_read(..., as_records=true)`.

- [ ] **Step 1: Read F's skill as the template**

```bash
cat src/libs/colmena/skills/crdt-doc-cross-sheet-analysis/SKILL.md
ls src/libs/colmena/skills/crdt-doc-cross-sheet-analysis/references/
```

Read each reference file too — they're each ~30-50 lines of pandas/json examples.

- [ ] **Step 2: Write `gsheets-cross-sheet-analysis/SKILL.md`**

```markdown
---
name: gsheets-cross-sheet-analysis
description: Use when comparing two sheets, joining/enriching data from one sheet into another, or transforming rows based on conditions from another sheet — same patterns as crdt-doc-cross-sheet-analysis but for Google Sheets via gsheets_* tools. Load THIS skill first; then load the specific pattern reference you need.
references:
  - name: pattern-a-cell-diff
    description: Cell-by-cell diff between two sheets with identical shape (DataFrame.compare). Use when comparing two versions of the same report.
  - name: pattern-b-row-diff
    description: Row-level diff by a key column — tags each row only_in_A / only_in_B / changed / unchanged. The MOST COMMON case.
  - name: pattern-c-schema-diff
    description: Compare column structure of two sheets (which exist where, with what dtype).
  - name: pattern-d-statistical
    description: Statistical comparison of numeric columns (mean, std, t-test) to detect drift between two snapshots.
  - name: pattern-e-join-enrich
    description: Bring columns from one sheet into another via left join. Reports unmatched keys.
  - name: pattern-f-conditional-transform
    description: Apply per-row rules defined in another sheet (e.g. discounts by Region with min Qty).
---

# gsheets-cross-sheet-analysis — Index

Compare, join, enrich and transform data across Google Sheets tabs that
may live in different spreadsheets. Source data is **read into pandas**;
results are written back via `gsheets_set_range`.

## The canonical flow

1. `gsheets_list_sheets({spreadsheet_id})` — discover tabs in the
   spreadsheet you have an id for.
2. `gsheets_read({spreadsheet_id, sheet, range?, as_records: true})` —
   pull source data as `[{col: val, ...}, ...]` records, ready for
   `pd.DataFrame(records)`.
3. `run_python({script, inputs})` — do the analysis (joins, diffs,
   pivots, etc.). Output is more records.
4. `gsheets_add_sheet({spreadsheet_id, name})` if needed, then
   `gsheets_set_range({spreadsheet_id, sheet, start_addr, values_2d})`
   with `[headers, ...rows]` as the 2D array.

## When to load which reference

| User says… | Load reference |
|---|---|
| "compará", "qué cambió", "diferencias entre" with a key column | `pattern-b-row-diff` |
| "compará" sin key (mismo shape, mismas columnas y filas) | `pattern-a-cell-diff` |
| "qué columnas tiene cada uno" / structural check | `pattern-c-schema-diff` |
| "comparar promedios", "detectar drift" | `pattern-d-statistical` |
| "enriquecer A con info de B" | `pattern-e-join-enrich` |
| "aplicar reglas definidas en otra hoja" | `pattern-f-conditional-transform` |

Tool name parity with the local-CRDT skill is intentional — same
patterns, different backend.

## Working with multiple spreadsheets

`gsheets_*` tools all take an explicit `spreadsheet_id`. To compare
across two spreadsheets, the agent receives BOTH ids in its prompt (or
via prior tool results) and threads them per call. There is no
"current spreadsheet" or session state — every call is explicit.
```

- [ ] **Step 3: Write each of the 6 pattern references**

For each pattern file (`pattern-a-cell-diff.md` through `pattern-f-conditional-transform.md`):

1. Read the F equivalent: `cat src/libs/colmena/skills/crdt-doc-cross-sheet-analysis/references/pattern-a-cell-diff.md`
2. Copy to `src/libs/colmena/skills/gsheets-cross-sheet-analysis/references/pattern-a-cell-diff.md`
3. Mechanical find-and-replace:
   - `crdt_doc_read` → `gsheets_read` (and add `spreadsheet_id` arg)
   - `crdt_doc_set_range` → `gsheets_set_range` (idem)
   - `sheet_id` → `sheet` (title-based instead of internal id)
   - Any references to artifact_id / list_my_artifacts → drop (Google has no equivalent in v1)
4. Add a top-of-file note: `> Same pattern as the crdt-doc equivalent — see crdt-doc-cross-sheet-analysis if you need the local-CRDT variant.`

Repeat for all 6. Each file ends up ~30-50 lines.

- [ ] **Step 4: Verify skill auto-discovery picks it up**

```bash
cargo test -p colmena_dag_engine --lib builtin_skill_repository 2>&1 | tail
```

Expected: all existing skill tests still pass, the new skill should appear in `available_builtin_names_includes_authored_skills` if it asserts a list (add `gsheets-cross-sheet-analysis` to that assertion if needed).

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/skills/gsheets-cross-sheet-analysis/
git commit -m "$(cat <<'EOF'
docs(E-T11): skill gsheets-cross-sheet-analysis (mirror of F's CRDT skill)

SKILL.md + 6 reference files (pattern-a through pattern-f) covering
cell-diff, row-diff, schema-diff, statistical, join-enrich, and
conditional-transform. Same shape as crdt-doc-cross-sheet-analysis,
mechanical find-and-replace with spreadsheet_id added.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 12: Docs (dev guide §5.9 + node_as_tools_reference + BACKLOG + CHANGELOG)

**Files:**
- Modify: `docs/developer_guide/38_crdt_documents.md` (or create `docs/developer_guide/39_gsheets.md`)
- Modify: `docs/node_as_tools_reference.json`
- Modify: `docs/BACKLOG.md`
- Modify: `docs/CHANGELOG_2026-06.md`

- [ ] **Step 1: dev guide entry**

Decide: append §5.9 to `38_crdt_documents.md`, or create a new
`39_gsheets.md`. Recommended: new file for cleanliness (gsheets has its own
auth story and the CRDT doc is already long).

Create `docs/developer_guide/39_gsheets.md`:

```markdown
# 39. Google Sheets integration (Subsystem E)

> v1 ships 9 synthetic LLM tools mirroring `crdt_doc_*` shape — agents
> read, write, create, and analyse Google Sheets via the Sheets API v4 +
> Drive API. Auth via Service Account JSON or Application Default
> Credentials. No OAuth user-scoped flow in v1.

## Tool surface

| Tool | What it does |
|---|---|
| `gsheets_create_spreadsheet` | Create a new empty spreadsheet. Returns `{spreadsheet_id, url}`. |
| `gsheets_create_from_xlsx` | Upload an `.xlsx` attachment as a native Google Sheet (auto-conversion). |
| `gsheets_export_xlsx` | Download a Google Sheet as `.xlsx`, register as attachment. |
| `gsheets_list_sheets` | List tabs in a spreadsheet. |
| `gsheets_add_sheet` | Add a new tab. |
| `gsheets_delete_sheet` | Delete a tab by title or numeric id. |
| `gsheets_read` | Read cells; `value_render` controls formula vs evaluated; `as_records` controls 2D-array vs records shape. |
| `gsheets_set_cell` | Write one cell. Strings starting with `=` are evaluated by Google server-side. |
| `gsheets_set_range` | Bulk-write a rectangular block. Same formula semantics. |

UX aliases (per D-T16 lessons): `address` ↔ `addr`, `start` ↔ `start_addr`,
`values` ↔ `values_2d`, `name` ↔ `sheet`. Single-A1 ranges
auto-expanded (`"C1"` → `"C1:C1"`).

## Auth

Two paths, no app config required in colmena itself:

1. **Service Account JSON** — set `GOOGLE_APPLICATION_CREDENTIALS=/path/to/sa.json`.
   The SA email must be shared (Edit access) on each spreadsheet the
   agent touches. Best for unattended/automation use.
2. **Application Default Credentials** — when the env var is unset,
   `yup-oauth2` falls back to ADC: GCP metadata server in cloud
   environments, or `gcloud auth application-default login` for local
   dev.

Scopes: defaults to `spreadsheets` + `drive.file`. Override via
`COLMENA_GSHEETS_SCOPES=<comma-sep>` (short names or full URLs).

## Formulas — Google evaluates them

Unlike subsystem D (where colmena's `formula_engine` evaluates
spreadsheet formulas), Google Sheets evaluates `=...` formulas
server-side. Write a string starting with `=` to `gsheets_set_cell` /
`gsheets_set_range` and Google parses, evaluates, and cascades it. Read
back via `gsheets_read(..., value_render="UNFORMATTED_VALUE")` to get
the computed number, or `value_render="FORMULA"` to get the text.

## Pandas analysis flow

Same shape as `crdt_doc_*` analysis (subsystem F). Skill
`gsheets-cross-sheet-analysis` documents 6 patterns
(`pattern-a-cell-diff` through `pattern-f-conditional-transform`).

## Hexagonal layout

- `src/libs/colmena/src/gsheets/domain/` — `SheetsClient` trait,
  value types, errors.
- `src/libs/colmena/src/gsheets/infrastructure/` — REST adapter
  (`http_client.rs`), auth (`auth.rs`), config (`config.rs`).
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gsheets_tools.rs` —
  9 dispatchers.

## Out of scope for v1 (BACKLOG)

See "Subsystem E v1.1" in `docs/BACKLOG.md`: list_spreadsheets discovery,
OAuth user-scoped auth, cell formatting, charts, conditional formatting,
permissions / sharing, revisions, webhooks.

## Spec + plan

- Spec: `docs/superpowers/specs/2026-06-05-google-sheets-design.md`
- Plan: `docs/superpowers/plans/2026-06-05-google-sheets.md`
```

- [ ] **Step 2: BACKLOG entries**

Open `docs/BACKLOG.md`. Append a new "Subsystem E v1.1 (Google Sheets)" section:

```markdown
## Subsystem E v1.1 (Google Sheets)

- [ ] **`gsheets_list_spreadsheets()`** — Drive discovery scoped to a
  shared folder. Needs `drive.metadata.readonly` scope and a
  folder-filter mechanism so the agent doesn't see the whole Drive.
- [ ] **OAuth user-scoped auth** — act on behalf of a specific human
  user. Likely a new `SheetsAuthProvider` trait so ADP (or any
  downstream consumer) can plug in its OAuth flow.
- [ ] **Cell formatting** — colors, borders, column widths via
  `batchUpdate` + `repeatCell`/`updateBorders`.
- [ ] **Charts** via `batchUpdate.addChart`.
- [ ] **Conditional formatting** via `batchUpdate.addConditionalFormatRule`.
- [ ] **Data validation (dropdowns)** via `batchUpdate.setDataValidation`.
- [ ] **Permissions / sharing tools** via `drive.permissions.*`.
- [ ] **Revisions / undo** via Drive Revisions API.
- [ ] **Webhook subscriptions** for push notifications on sheet changes.
- [ ] **Per-call credential overrides** — a graph could provide a different
  SA for different spreadsheets, useful for multi-tenant scenarios.
- [ ] **Apps Script execution** from colmena (a single new tool calling
  `scripts.run`).
```

- [ ] **Step 3: CHANGELOG**

In `docs/CHANGELOG_2026-06.md`, append a section (after the D entry):

```markdown
### E — Google Sheets integration (subsystem E, v1)

- **9 synthetic LLM tools** mirroring `crdt_doc_*` shape: create
  spreadsheet, create_from_xlsx, export_xlsx, list_sheets, add_sheet,
  delete_sheet, read, set_cell, set_range. Tool descriptions are
  deliberately parallel so skills transfer with find-and-replace.
- **Hexagonal layout** at `src/libs/colmena/src/gsheets/`: `SheetsClient`
  trait in domain, `GoogleSheetsHttpClient` REST adapter in
  infrastructure. Tests mock the trait; integration tests `#[ignore]`-gated
  behind GOOGLE_APPLICATION_CREDENTIALS + COLMENA_GSHEETS_TEST_SPREADSHEET_ID.
- **Auth via existing `yup-oauth2`** — Service Account JSON via
  `GOOGLE_APPLICATION_CREDENTIALS` or Application Default Credentials.
  Same pattern as `image_generation.rs`. **Zero new dependencies.**
- **Formulas evaluated by Google** — write a `"=SUM(...)"` string and
  read back via `value_render: "UNFORMATTED_VALUE"` (computed) or
  `"FORMULA"` (text). No client-side formula engine here.
- **xlsx upload** via Drive API multipart/related conversion;
  **xlsx download** via Drive `export` endpoint. Both flow through the
  existing attachment plumbing.
- **Auto-retry on 429 / 5xx** with exponential backoff (1s/2s/4s).
- **UX aliases** carried forward from D-T16: `address`/`addr`,
  `start`/`start_addr`, `values`/`values_2d`, `name`/`sheet`.
- **Skill `gsheets-cross-sheet-analysis`** — mirror of F's CRDT skill,
  6 pattern references.
- **No OAuth user-scoped flow in v1.** Deferred to v1.1 so ADP (or any
  downstream) can build it on top.

Refs: spec `docs/superpowers/specs/2026-06-05-google-sheets-design.md`,
plan `docs/superpowers/plans/2026-06-05-google-sheets.md`.
```

- [ ] **Step 4: `docs/node_as_tools_reference.json`**

Add 9 new entries to this JSON file. The shape mirrors the existing
`crdt_doc_*` entries — find one like `crdt_doc_read` to use as template:

```bash
grep -n "crdt_doc_read\|crdt_doc_set_cell" docs/node_as_tools_reference.json | head
```

For each gsheets tool, add a block like:

```json
"gsheets_read": {
  "description": "Read cells from a Google Spreadsheet tab.",
  "args_schema": {
    "type": "object",
    "properties": {
      "spreadsheet_id": {"type": "string"},
      "sheet": {"type": "string"},
      "range": {"type": "string", "description": "Optional A1 range, e.g. \"A1:D10\""},
      "value_render": {"type": "string", "enum": ["FORMULA", "UNFORMATTED_VALUE", "FORMATTED_VALUE"]},
      "as_records": {"type": "boolean"}
    },
    "required": ["spreadsheet_id", "sheet"]
  },
  "returns_schema": {
    "type": "object",
    "properties": {
      "ok": {"type": "boolean"},
      "sheet": {"type": "string"},
      "range": {"type": "string"},
      "values": {"type": "array"}
    }
  }
}
```

Validate the file parses:

```bash
python3 -c "import json; json.load(open('docs/node_as_tools_reference.json'))" && echo OK
```

- [ ] **Step 5: Build verification (nothing should have broken)**

```bash
cargo test -p colmena_dag_engine --lib 2>&1 | tail -5
cargo clippy -p colmena_dag_engine --lib --tests -- -D warnings 2>&1 | tail
```

- [ ] **Step 6: Commit**

```bash
git add docs/developer_guide/39_gsheets.md docs/node_as_tools_reference.json docs/BACKLOG.md docs/CHANGELOG_2026-06.md
git commit -m "$(cat <<'EOF'
docs(E-T12): dev guide §39, node configs, BACKLOG (11 v1.1 items), CHANGELOG

§39 documents the tool surface, auth model, formula semantics, pandas
flow, and hexagonal layout. BACKLOG enumerates 11 deferred items.
CHANGELOG notes E shipped with zero new deps.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 13: Final sweep

**Files:** none — verification only.

- [ ] **Step 1: Full cargo test --verbose**

```bash
cargo test --verbose 2>&1 | tail -30
```

Expected: all green (unit + integration + doctests).

- [ ] **Step 2: Clippy**

```bash
cargo clippy --all-targets -- -D warnings 2>&1 | tail
```

Expected: zero warnings.

- [ ] **Step 3: fmt**

```bash
cargo fmt --all -- --check 2>&1 | tail
```

If diffs, run `cargo fmt --all` and commit them.

- [ ] **Step 4: Re-run the smoke graph (if real auth available)**

```bash
set -a; source .env; set +a
cargo run --release --bin dag_engine -- run tests/graphs/agents/gsheets_smoke.json \
  --agent-session-id e_final_sweep_$(date +%s) \
  --include-extra-info 2>&1 | tee /tmp/colmena_e2e/e_final_sweep.sse
```

Verify all 7 prompt steps complete.

- [ ] **Step 5: Final summary**

Report to the user:

- Tasks completed: 13 (E-T1 .. E-T13).
- Commits since plan: ~14-16 depending on quality-revision splits.
- LOC delta vs estimate (~2200).
- Tests: ~14 unit (gsheets) + 3 integration (ignored) + 4 dispatcher unit + smoke graph.
- Skill: `gsheets-cross-sheet-analysis` with 6 references.
- Zero new dependencies.
- BACKLOG: 11 v1.1 items.

- [ ] **Step 6: fmt-only commit if step 3 generated diffs**

```bash
git status --short
git add -u
git commit -m "chore(E-T13): cargo fmt sweep"
```

---

## Self-review (run against the spec)

### Spec coverage

| Spec section | Plan coverage |
|---|---|
| §1 Problem statement | Implicit — entire plan delivers tools. |
| §2 Hard open-source rule | E-T2/T3 use env vars + traits; no ADP-specific code anywhere. |
| §2 Goals v1 — 9 tools | E-T1 (types), E-T2 (trait), E-T3 (auth), E-T4/T5 (HTTP impl), E-T6 (dispatchers), E-T7 (router), E-T8 (registry). |
| §2 Goals — ADC + SA JSON | E-T3 |
| §2 Goals — `USER_ENTERED` for formulas | E-T4 step 8 (set_cell), step 10 (set_range) |
| §2 Goals — auto-retry 429/5xx | E-T4 step 5 (get_json), step 8 (put_json), E-T5 step 1 (post_json) |
| §2 Goals — xlsx upload via Drive | E-T5 step 4 |
| §2 Goals — xlsx download | E-T5 step 5 |
| §2 Goals — skill mirror | E-T11 |
| §3 Architecture | E-T1 + E-T6 (module layout) |
| §4 Components | E-T1..T6 cover types, errors, trait, config, auth, http_client, dispatchers |
| §5 Auth model | E-T3 |
| §6 Tool surface | E-T6 (definitions) + E-T7 (router) + E-T8 (registry) |
| §7 Data flow | All 5 flows in spec → E-T4 (read with formulas), E-T4 (write with formula), F-pattern parity (E-T11 skill), E-T5 (xlsx upload), E-T5 + E-T11 (Excel → process → upload composite) |
| §8 Error handling | E-T1 (enum) + E-T4 (get/put_json mapping) + E-T5 (post_json + create_from_xlsx + export_xlsx) + E-T6 (error_to_json) |
| §9 Testing | E-T1/T3/T4/T5 (unit), E-T9 (integration), E-T10 (smoke), E-T6 (dispatcher unit) |
| §10 Performance | Implicit in batch-write semantics + retry policy |
| §11 Back-compat | No existing tools renamed; new module isolated. Confirmed in E-T8 + E-T13. |
| §12 Out-of-scope | E-T12 BACKLOG entries |

**No gaps identified.** Every spec requirement maps to at least one task.

### Placeholder scan

- No "TBD" / "implement later" / "add appropriate error handling".
- Two `unimplemented!()` calls in E-T4 step 5 (for the other 6 trait
  methods) are intentional TDD scaffolding — replaced in E-T4/T5
  subsequent steps. They never survive to a commit (each is replaced in
  the same task as the test that exercises it).
- E-T7 has one conditional escape: "If neither tool has a counterpart
  in this file, you may need to thread an `AttachmentRegistry` …". This
  is necessary because the attachment-resolver mechanism is
  inspected-by-implementer rather than fully specified — flag is
  explicit and provides a fallback (defer the two xlsx dispatchers).

### Type consistency check

- `SheetsError` variants used identically across types.rs / http_client.rs / dispatchers.
- `CellValue::from_json` / `to_json` signatures match across all uses.
- `ValueRenderOption::as_api_str` strings match Google's docs verbatim
  (`FORMATTED_VALUE`, `UNFORMATTED_VALUE`, `FORMULA`).
- `SpreadsheetId(String)` consistent — every dispatcher wraps the
  incoming string with `SpreadsheetId(...)`.
- `parse_meta` helper in E-T5 step 1 shared between `create_spreadsheet`
  and `create_from_xlsx`; both consume it correctly (one with default_id
  None, one with Some(new_id)).
- Tool name constants exported from `gsheets_tools.rs` and re-exported
  via `mod.rs` are referenced in E-T7 router AND E-T8 registry — exact
  identifier match across all three sites.

All clean. Plan is ready for execution.
