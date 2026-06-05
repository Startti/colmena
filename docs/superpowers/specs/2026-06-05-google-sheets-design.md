# Google Sheets Integration — Design Spec (Subsystem E)

**Date:** 2026-06-05
**Status:** Draft → pending user review
**Subsystem:** E — Google Sheets integration
**Prior art:** Subsystems B (recent changes), C (pandas/run_python), F (cross-sheet
analysis), D (formulas) shipped on the local CRDT spreadsheet stack. E adds
parallel tooling for *Google* spreadsheets.

---

## 1. Problem statement

Today colmena agents can read/write/analyse spreadsheets via the local CRDT
documents subsystem (B/C/D/F). All the data lives in a colmena-owned
`yrs::Doc`. That's great for ADP-internal workflows where the user-facing
canvas is also colmena's, but it has zero reach into spreadsheets the user
already keeps in **Google Sheets** — and Google Sheets is where most
business data actually lives.

This subsystem gives agents first-class tools to operate on Google Sheets
directly: list tabs, read ranges, write cells, write formulas (Google
evaluates them server-side), create new spreadsheets, add tabs, and
upload a local `.xlsx` as a native Google Sheet.

Critically, the existing pandas workflow (`run_python` reading from CRDT,
computing, writing back) generalises 1-for-1 to Google Sheets — same
mental model, just a different "backend". Skills like
`crdt-doc-cross-sheet-analysis` can be mirrored as
`gsheets-cross-sheet-analysis` with mechanical find-and-replace.

---

## 2. Goals & non-goals

### Hard architectural constraint

**Colmena is an open-source library. Nothing ADP-specific lands here.** All
auth, scope, and behavioural config flows in through env vars / node config
/ traits — no hardcoded ADP OAuth client, no ADP-specific assumptions.
ADP can implement OAuth user-scoped flows on top of colmena, but colmena
itself ships only Service Account JSON + ADC auth in v1 (see §5).

### Goals (v1)

- 9 synthetic LLM tools (see §6) that mirror the shape of the existing
  `crdt_doc_*` tools, so agents reuse the same mental model.
- Two auth paths: **AUTH-A** Service Account JSON (via
  `GOOGLE_APPLICATION_CREDENTIALS` or explicit config path) and
  **AUTH-C** Application Default Credentials (ADC) — already the pattern
  used by `image_generation.rs`. No new auth crates.
- Write paths that include formulas (`"=SUM(A1:A10)"`) get Google's
  server-side evaluation for free — no need for our backend
  `formula_engine` here.
- Bulk writes use the Sheets API `batchUpdate` to amortise round-trips
  against the per-user 60-req/min quota.
- Auto-retry on 429 (rate-limited) and 5xx with exponential backoff.
- Excel upload (`.xlsx` attachment → native Google Sheet) via Drive API
  `files.create` with automatic conversion (the `mimeType` trick).
- Excel download (Google Sheet → `.xlsx` attachment) via the Sheets
  `export` endpoint.
- A skill `gsheets-cross-sheet-analysis` mirroring F's
  `crdt-doc-cross-sheet-analysis` so the same 6 analysis patterns (Row
  Diff, Schema Diff, Enrichment, Pivot, Cross-Sheet Aggregation, Join)
  apply.

### Non-goals (deferred to v1.1)

- `gsheets_list_spreadsheets()` (Drive scope discovery; would require
  scoping to a shared Drive folder for safety).
- OAuth user-scoped flow (acting on behalf of a specific human user —
  ADP can build this on top).
- Cell formatting (colors, borders, column widths), charts, conditional
  formatting, data validation.
- Permissions / sharing tools (`drive.permissions.*`).
- Revisions / undo via Drive Revisions API.
- Webhook / push-notification subscriptions for live change events.

---

## 3. Architecture

```
┌──────────────────────────────────────────────────────────────────┐
│                          Agent (LLM)                              │
└────────┬────────────────────────────────────────┬────────────────┘
         │ gsheets_read / set_cell / ...          │ run_python
         ▼                                        ▼
┌─────────────────────────────┐         ┌──────────────────────────┐
│  llm_synthetic_tools/       │         │  PythonNode sandbox      │
│  gsheets_tools.rs           │         │  (existing)              │
│  (9 dispatchers)            │         └──────────────────────────┘
└────────┬────────────────────┘
         │ delegates to
         ▼
┌─────────────────────────────────────────────────────────────────┐
│  src/libs/colmena/src/gsheets/    (NEW MODULE)                  │
│  ├─ domain/                                                      │
│  │   ├─ traits.rs:    `SheetsClient` trait (the port)            │
│  │   ├─ types.rs:     SpreadsheetId, Range, ValueRenderOption,   │
│  │   │                CellValue, SheetMeta, etc.                 │
│  │   └─ errors.rs:    `SheetsError` enum                         │
│  ├─ application/                                                 │
│  │   ├─ read.rs:      read_range use case                        │
│  │   ├─ write.rs:     set_cell / set_range use cases             │
│  │   └─ admin.rs:     create / add_sheet / delete_sheet /        │
│  │                     upload_xlsx / export_xlsx                 │
│  └─ infrastructure/                                              │
│      ├─ http_client.rs: `GoogleSheetsHttpClient` (impl trait)    │
│      ├─ auth.rs:        ADC + SA JSON via existing yup-oauth2    │
│      └─ config.rs:      `GSheetsConfig`                          │
└──────────────────────────────────────────────────────────────────┘
```

**Hexagonal**: `SheetsClient` trait in domain (port). `GoogleSheetsHttpClient`
in infrastructure (adapter). Tests use a `MockSheetsClient` impl. Tools in
`dag_engine/.../llm_synthetic_tools/gsheets_tools.rs` only ever touch the
trait — fully testable without hitting Google.

**Zero new dependencies.** `yup-oauth2` is already in
`src/libs/colmena/Cargo.toml` (used by `image_generation.rs`); we reuse
its ADC + SA JSON support directly. HTTP comes from `reqwest` (already
present). JSON via `serde_json` (already present).

---

## 4. Components

### 4.1 `gsheets/domain/types.rs`

```rust
pub struct SpreadsheetId(pub String);
pub struct SheetId(pub i64);

pub enum CellValue {
    Null,
    Bool(bool),
    Number(f64),
    String(String),       // includes formulas — strings starting with `=`
                          // are evaluated server-side by Google when written
                          // with `valueInputOption: "USER_ENTERED"`.
    Error(String),        // Excel error string read back from the sheet
}

pub enum ValueRenderOption {
    FormattedValue,       // "1,234.50" — what the user sees
    UnformattedValue,     // 1234.5 — number, default for pandas
    Formula,              // "=A1*2" — formula text, not the result
}

pub struct ReadOptions {
    pub value_render: ValueRenderOption,
    pub as_records: bool, // true → return [{"A":...,"B":...}], false → rectangular [[v]]
}

pub struct ReadResponse {
    pub sheet: String,
    pub range: String,
    pub values: serde_json::Value, // shape depends on as_records
}

pub struct SetRangeResponse {
    pub updated_cells: u64,
    pub updated_range: String,
}

pub struct SheetMeta {
    pub sheet_id: SheetId,
    pub title: String,
    pub index: u32,
    pub row_count: u32,
    pub col_count: u32,
}

pub struct SpreadsheetMeta {
    pub spreadsheet_id: SpreadsheetId,
    pub title: String,
    pub url: String,                // https://docs.google.com/...
    pub sheets: Vec<SheetMeta>,
}
```

### 4.2 `gsheets/domain/traits.rs` — `SheetsClient`

```rust
#[async_trait]
pub trait SheetsClient: Send + Sync {
    async fn create_spreadsheet(&self, title: &str)
        -> Result<SpreadsheetMeta, SheetsError>;

    async fn create_from_xlsx(&self, title: &str, bytes: Vec<u8>)
        -> Result<SpreadsheetMeta, SheetsError>;

    async fn export_xlsx(&self, id: &SpreadsheetId)
        -> Result<Vec<u8>, SheetsError>;

    async fn list_sheets(&self, id: &SpreadsheetId)
        -> Result<Vec<SheetMeta>, SheetsError>;

    async fn add_sheet(&self, id: &SpreadsheetId, name: &str)
        -> Result<SheetMeta, SheetsError>;

    async fn delete_sheet(&self, id: &SpreadsheetId, name_or_sheet_id: &str)
        -> Result<(), SheetsError>;

    async fn read_range(
        &self,
        id: &SpreadsheetId,
        sheet: &str,
        range: Option<&str>,    // e.g. "A1:D10"; None → entire sheet
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

### 4.3 `gsheets/domain/errors.rs`

```rust
#[derive(Debug, thiserror::Error)]
pub enum SheetsError {
    #[error("gsheets_not_configured: {0}")]
    NotConfigured(String),

    #[error("auth_failed: {0}")]
    AuthFailed(String),

    #[error("spreadsheet_not_found: {0}")]
    SpreadsheetNotFound(String),

    #[error("sheet_not_found: {0}")]
    SheetNotFound(String),

    #[error("invalid_range: {0}")]
    InvalidRange(String),

    #[error("permission_denied: share the spreadsheet with {0}")]
    PermissionDenied(String),     // populated with the SA email when possible

    #[error("rate_limit: retry after {0}s")]
    RateLimit(u32),

    #[error("http_error: {0}")]
    Http(String),

    #[error("internal: {0}")]
    Internal(String),
}
```

### 4.4 `gsheets/infrastructure/auth.rs`

Token acquisition reuses the exact pattern at
`src/libs/colmena/src/dag_engine/infrastructure/nodes/image_generation.rs:572`:

```rust
pub async fn get_token(scopes: &[&str]) -> Result<String, SheetsError> {
    use yup_oauth2::authenticator::ApplicationDefaultCredentialsTypes;
    use yup_oauth2::{
        ApplicationDefaultCredentialsAuthenticator,
        ApplicationDefaultCredentialsFlowOpts,
    };
    let opts = ApplicationDefaultCredentialsFlowOpts::default();
    let auth = match ApplicationDefaultCredentialsAuthenticator::builder(opts).await {
        ApplicationDefaultCredentialsTypes::InstanceMetadata(builder) =>
            builder.build().await.map_err(|e| SheetsError::AuthFailed(e.to_string()))?,
        ApplicationDefaultCredentialsTypes::ServiceAccount(builder) =>
            builder.build().await.map_err(|e| SheetsError::AuthFailed(e.to_string()))?,
    };
    let token = auth.token(scopes).await.map_err(|e| {
        // Surface "no credentials" as NotConfigured with a hint
        let msg = e.to_string();
        if msg.contains("no credentials") || msg.contains("GOOGLE_APPLICATION_CREDENTIALS") {
            SheetsError::NotConfigured(
                "set GOOGLE_APPLICATION_CREDENTIALS or run `gcloud auth application-default login`"
                    .to_string()
            )
        } else {
            SheetsError::AuthFailed(msg)
        }
    })?;
    Ok(token.token().ok_or_else(|| SheetsError::AuthFailed("empty token".into()))?.to_string())
}
```

Token cached in-memory by the HTTP client with a 50-minute TTL (Google
tokens last 60 minutes; conservative refresh).

### 4.5 `gsheets/infrastructure/http_client.rs`

`GoogleSheetsHttpClient` implements `SheetsClient`. Each method:

1. Acquires/refreshes the bearer token (`get_token`).
2. Builds the HTTP request via `reqwest::Client`.
3. Parses the JSON response or maps the HTTP status to a `SheetsError`:
   - 401 → refresh token once, retry; if still fails → `AuthFailed`.
   - 403 → `PermissionDenied(sa_email)` (parsed from token).
   - 404 → `SpreadsheetNotFound` or `SheetNotFound` (per endpoint).
   - 429 → `RateLimit(retry_after_seconds)`; auto-retry up to 3 times with
     exponential backoff (1s, 2s, 4s).
   - 5xx → auto-retry as for 429.
   - 400 (bad range etc) → `InvalidRange(message)`.

Endpoints used (all REST, no client libraries needed):

| Operation | Endpoint |
|---|---|
| Create spreadsheet | `POST https://sheets.googleapis.com/v4/spreadsheets` |
| List tabs (sheets) | `GET  https://sheets.googleapis.com/v4/spreadsheets/{id}` (returns metadata incl. sheets[]) |
| Add tab | `POST https://sheets.googleapis.com/v4/spreadsheets/{id}:batchUpdate` with `addSheet` |
| Delete tab | `POST https://sheets.googleapis.com/v4/spreadsheets/{id}:batchUpdate` with `deleteSheet` |
| Read range | `GET  https://sheets.googleapis.com/v4/spreadsheets/{id}/values/{range}?valueRenderOption=...` |
| Write cell / range | `PUT  https://sheets.googleapis.com/v4/spreadsheets/{id}/values/{range}?valueInputOption=USER_ENTERED` |
| Upload xlsx | `POST https://www.googleapis.com/upload/drive/v3/files?uploadType=multipart` |
| Download xlsx | `GET  https://www.googleapis.com/drive/v3/files/{id}/export?mimeType=application/vnd.openxmlformats-officedocument.spreadsheetml.sheet` |

### 4.6 `gsheets/infrastructure/config.rs`

```rust
pub struct GSheetsConfig {
    pub credentials_path: Option<PathBuf>,
    pub scopes: Vec<String>,         // defaults to spreadsheets + drive.file
    pub request_timeout: Duration,   // default 30s
    pub max_retries: u32,            // default 3
}

impl GSheetsConfig {
    pub fn from_env() -> Self {
        let creds = std::env::var("GOOGLE_APPLICATION_CREDENTIALS")
            .ok()
            .map(PathBuf::from);
        let scopes = std::env::var("COLMENA_GSHEETS_SCOPES")
            .ok()
            .map(|s| s.split(',').map(String::from).collect())
            .unwrap_or_else(|| vec![
                "https://www.googleapis.com/auth/spreadsheets".to_string(),
                "https://www.googleapis.com/auth/drive.file".to_string(),
            ]);
        Self {
            credentials_path: creds,
            scopes,
            request_timeout: Duration::from_secs(30),
            max_retries: 3,
        }
    }
}
```

### 4.7 `dag_engine/.../llm_synthetic_tools/gsheets_tools.rs`

Single file with the 9 dispatchers. Each dispatcher:

1. Reads `GSheetsConfig::from_env()`.
2. Constructs (or reuses cached) `GoogleSheetsHttpClient`.
3. Calls the trait method.
4. Maps `Result<X, SheetsError>` to a JSON tool result with a stable
   shape: `{ok, ...}` on success, `{error, kind, hint?}` on failure.

The 9 tool names + arg schemas:

| Tool | Args | Returns |
|---|---|---|
| `gsheets_create_spreadsheet` | `{title}` | `{ok, spreadsheet_id, url}` |
| `gsheets_create_from_xlsx` | `{attachment_id, title}` | `{ok, spreadsheet_id, url, sheets[]}` |
| `gsheets_export_xlsx` | `{spreadsheet_id}` | `{ok, attachment_id}` |
| `gsheets_list_sheets` | `{spreadsheet_id}` | `{ok, sheets: [{sheet_id, title, index, row_count, col_count}]}` |
| `gsheets_add_sheet` | `{spreadsheet_id, name}` | `{ok, sheet_id, title}` |
| `gsheets_delete_sheet` | `{spreadsheet_id, sheet}` | `{ok}` |
| `gsheets_read` | `{spreadsheet_id, sheet, range?, value_render?, as_records?}` — defaults: `value_render="UNFORMATTED_VALUE"` (pandas-friendly scalar numbers/strings/bools, matching `crdt_doc_read`'s default scalar shape), `as_records=false` (rectangular 2D array). Set `value_render="FORMULA"` to read formula text, mirroring `crdt_doc_read(include_formulas=true)`. | `{ok, sheet, range, values}` |
| `gsheets_set_cell` | `{spreadsheet_id, sheet, addr, value}` | `{ok}` |
| `gsheets_set_range` | `{spreadsheet_id, sheet, start_addr, values_2d}` | `{ok, updated_cells, updated_range}` |

All defaults match the equivalent `crdt_doc_*` tool where one exists. UX
aliases (per D-T16 lesson): `address` ↔ `addr`, `start` ↔ `start_addr`,
`values` ↔ `values_2d`, single-A1 range auto-expansion.

---

## 5. Auth model

### Supported in v1

- **AUTH-A — Service Account JSON.** Operator sets
  `GOOGLE_APPLICATION_CREDENTIALS=/path/to/sa.json`. The SA's email is the
  identity Google sees. User must explicitly share each spreadsheet with
  that email (`File → Share` in Google Sheets UI).
- **AUTH-C — Application Default Credentials.** When
  `GOOGLE_APPLICATION_CREDENTIALS` is unset, `yup-oauth2` falls back to
  ADC: in GCP it uses the workload's compute identity; locally it uses
  whatever `gcloud auth application-default login` wrote.

Both flow through the same `ApplicationDefaultCredentialsAuthenticator`
in `yup-oauth2` — zero conditional code in colmena.

### Scopes

Defaults (sufficient for v1):

- `https://www.googleapis.com/auth/spreadsheets` — read/write all sheets
  the identity has access to. Required for all sheet operations.
- `https://www.googleapis.com/auth/drive.file` — create files; access
  files the app created/opened. Required for `create_from_xlsx` /
  `export_xlsx`. **Critically, this scope does NOT grant blanket Drive
  access** — only files this app touches. Privacy-safe for v1.

Overridable per-operator via `COLMENA_GSHEETS_SCOPES=...` env var (comma
sep). v1.1's `gsheets_list_spreadsheets()` will need
`https://www.googleapis.com/auth/drive.metadata.readonly` plus a
folder-scoping mechanism; that work is BACKLOG.

### Not in v1: OAuth user-scoped

Explicitly out. ADP (or any other consumer that needs to act on behalf of
specific human users) can build that on top of colmena: typically by
taking the user's refresh_token, exchanging for an access_token, and
passing the access_token to a yet-to-exist colmena hook. The shape of
that hook (probably a new `SheetsAuthProvider` trait) is BACKLOG.

---

## 6. Tool surface — agent-facing

The 9 tools listed in §4.7. Tool descriptions deliberately MIRROR
`crdt_doc_*` so skills (and agent instincts) transfer:

| `gsheets_*` | `crdt_doc_*` analogue | Differences |
|---|---|---|
| `gsheets_list_sheets` | `crdt_doc_list_sheets` | Adds `spreadsheet_id` param. No `formula_count` (Google doesn't expose cheaply). |
| `gsheets_add_sheet` | `crdt_doc_add_sheet` | Adds `spreadsheet_id`. Returns numeric `sheet_id` (Google's internal id). |
| `gsheets_read` | `crdt_doc_read` | Adds `spreadsheet_id` + `value_render`. `include_formulas` from D-T6 replaced by `value_render: "FORMULA"` (Google's native). |
| `gsheets_set_cell` | `crdt_doc_set_cell` | Adds `spreadsheet_id`. **No `cells_recalculated`** in response — Google handles cascade transparently. **No `warnings`** for `needs_browser` (Google evaluates every formula it supports; warnings only for parse errors). |
| `gsheets_set_range` | `crdt_doc_set_range` | Adds `spreadsheet_id`. Returns `updated_cells` + `updated_range`. |
| `gsheets_create_spreadsheet` | `crdt_doc_create_artifact` | New. Returns `{spreadsheet_id, url}` — `url` is convenience for the agent to surface to the user. |
| `gsheets_create_from_xlsx` | (combo of `create_artifact` + `import_sheet`) | New. Native Drive API conversion. |
| `gsheets_export_xlsx` | (the `/export.xlsx` REST endpoint, indirectly) | New, attachment-based. |
| `gsheets_delete_sheet` | (no equiv in v1) | New. |

A new skill `gsheets-cross-sheet-analysis` mirrors F's
`crdt-doc-cross-sheet-analysis` — same 6 patterns, find-and-replace from
`crdt_doc_*` to `gsheets_*` plus added `spreadsheet_id` parameter
threading. About 150 lines.

---

## 7. Data flow

### 7.1 Read with formulas

```
agent: gsheets_read({
  spreadsheet_id: "abc",
  sheet: "Sales",
  range: "A1:D10",
  value_render: "FORMULA"
})

dispatcher
  → GoogleSheetsHttpClient::read_range(...)
  → GET https://sheets.googleapis.com/v4/spreadsheets/abc/values/Sales!A1:D10
       ?valueRenderOption=FORMULA
  → response: {range:"Sales!A1:D10", values:[["=SUM(B1:B5)", 5.0, ...], ...]}

dispatcher → JSON tool result: {ok, sheet:"Sales", range:"...", values:[[...]]}
```

Default `value_render: "UNFORMATTED_VALUE"` (pandas-friendly) returns the
evaluated numbers/strings/bools, not the formula text.

### 7.2 Write with formula (Google evaluates)

```
agent: gsheets_set_cell({
  spreadsheet_id: "abc",
  sheet: "Sales",
  addr: "E1",
  value: "=SUM(B:B)"
})

dispatcher
  → GoogleSheetsHttpClient::set_cell(...)
  → PUT https://sheets.googleapis.com/v4/spreadsheets/abc/values/Sales!E1
       ?valueInputOption=USER_ENTERED
       body: {values: [["=SUM(B:B)"]]}
  → response: {updatedCells:1, updatedRange:"Sales!E1"}

dispatcher → JSON: {ok:true}
```

Google evaluates `=SUM(B:B)` server-side and stores both the formula and
the cached value. A subsequent `gsheets_read` with
`value_render: "FORMULA"` returns the text; with `UNFORMATTED_VALUE` (or
`FORMATTED_VALUE`) returns the computed number. Cascade recalc is
Google's responsibility — when an input cell changes, dependent formulas
refresh.

### 7.3 Pandas cross-sheet analysis (identical to F's flow)

```
agent: gsheets_read("abc", "Q3 Sales", as_records=true)  → records_q3
agent: gsheets_read("def", "Q4 Sales", as_records=true)  → records_q4
agent: run_python({
  script: "merged = pd.merge(pd.DataFrame(records_q3), pd.DataFrame(records_q4),
                              on='SKU', how='outer', suffixes=('_q3','_q4'));
           diff = merged[merged['Cantidad_q4'] != merged['Cantidad_q3']];
           return {diff: diff.to_dict('records')}",
  inputs: {records_q3, records_q4}
})  → result.diff

agent: gsheets_add_sheet("abc", "Diff Q3 vs Q4")
agent: gsheets_set_range("abc", "Diff Q3 vs Q4", "A1",
                         [["SKU","Cantidad_q3","Cantidad_q4","Delta"], ...result.diff])
agent: gsheets_set_cell("abc", "Diff Q3 vs Q4", "E1", "=SUM(D:D)")
```

Identical pattern to `crdt_doc_*` cross-sheet analysis. Skill
`gsheets-cross-sheet-analysis` documents 6 such patterns.

### 7.4 Upload xlsx to Google

```
agent: gsheets_create_from_xlsx({
  attachment_id: "att_xyz",  // resolved via existing load_attachment infra
  title: "Q3 Sales"
})

dispatcher
  → load_attachment(att_xyz) → bytes
  → GoogleSheetsHttpClient::create_from_xlsx("Q3 Sales", bytes)
  → POST https://www.googleapis.com/upload/drive/v3/files?uploadType=multipart
       part 1 (metadata): {mimeType:"application/vnd.google-apps.spreadsheet",
                            name:"Q3 Sales"}
       part 2 (body): bytes (mime application/vnd.openxmlformats-...)
  → Google converts xlsx → native Google Sheet
  → response: {id:"new_id"}

dispatcher → GoogleSheetsHttpClient::list_sheets(new_id) for metadata
dispatcher → JSON: {ok, spreadsheet_id:"new_id", title:"Q3 Sales",
                     url:"https://docs.google.com/spreadsheets/d/new_id",
                     sheets: [...]}
```

### 7.5 Excel → process → upload (composite, no new tools)

The Excel-process-upload pattern the operator asked about is a composition
of existing pieces; no new tool needed:

```
agent: load_attachment(xlsx_id) → bytes
agent: run_python({script: "df = pd.read_excel(io.BytesIO(b64decode(payload)));
                            df_clean = df.dropna();
                            return {data: df_clean.to_dict('records')}",
                   inputs: {payload}})
agent: gsheets_create_spreadsheet("Q3 Clean") → {spreadsheet_id}
agent: gsheets_set_range(spreadsheet_id, "Sheet1", "A1", [headers, ...data])
```

Or, simpler when no preprocessing is needed:

```
agent: load_attachment(xlsx_id) → bytes
agent: gsheets_create_from_xlsx(xlsx_id, "Q3")  → uploaded as native sheet
```

---

## 8. Error handling

| Class | HTTP / yup-oauth2 signal | `SheetsError` | Tool JSON response |
|---|---|---|---|
| No creds configured | `yup-oauth2` "no credentials" | `NotConfigured` | `{error:"gsheets_not_configured", hint:"set GOOGLE_APPLICATION_CREDENTIALS or run gcloud auth application-default login"}` |
| Token refresh failed | 401 after one retry | `AuthFailed` | `{error:"auth_failed", message}` |
| User hasn't shared spreadsheet with SA | 403 | `PermissionDenied(sa_email)` | `{error:"permission_denied", hint:"share with <sa@project.iam.gserviceaccount.com>"}` |
| spreadsheet_id wrong | 404 on `/spreadsheets/{id}` | `SpreadsheetNotFound(id)` | `{error:"spreadsheet_not_found", spreadsheet_id}` |
| sheet name unknown | 400 with "Unable to parse range" | `SheetNotFound(name)` | `{error:"sheet_not_found", sheet}` |
| Bad range syntax | 400 | `InvalidRange(message)` | `{error:"invalid_range", message}` |
| Quota exhausted | 429 after 3 retries | `RateLimit(retry_after)` | `{error:"rate_limit", retry_after_seconds}` |
| 5xx (Google outage) | 5xx after 3 retries | `Http(...)` | `{error:"http_error", message}` |
| Network timeout | reqwest timeout | `Http("timeout")` | `{error:"http_error", message:"timeout"}` |

The retry policy (3 retries with 1s/2s/4s backoff) only applies to 429
and 5xx — not 4xx (auth/perms/not-found/bad-input), which are caller
errors and surface immediately.

---

## 9. Testing strategy

### 9.1 Unit tests

`gsheets/**/*.rs` modules ship inline `#[cfg(test)]` tests against a
`MockSheetsClient` that implements the trait. Coverage:

- All 9 use cases (happy path + error paths)
- Auth config parsing (env var presence, scope override)
- Error mapping (each HTTP status → expected `SheetsError`)
- Retry behaviour (mock returns 429 N times, verify final response)
- Bulk-write batching (assert N writes collapse into 1 HTTP call)

Target: ~40 unit tests.

### 9.2 Integration tests

`tests/gsheets_integration.rs` (`#[ignore]`-gated):

```rust
#[test]
#[ignore = "requires GOOGLE_APPLICATION_CREDENTIALS + COLMENA_GSHEETS_TEST_SPREADSHEET_ID"]
fn end_to_end_crud_against_real_api() { ... }
```

Requires:
- `GOOGLE_APPLICATION_CREDENTIALS` pointing to a real SA JSON.
- `COLMENA_GSHEETS_TEST_SPREADSHEET_ID` — an empty spreadsheet shared
  with the SA email.

Scenarios (~7):
1. Create + add sheet + write cell + read back + delete sheet.
2. Bulk set_range round-trip.
3. Write formula, read evaluated value (assert Google evaluated).
4. Write formula, read raw formula text (`value_render: "FORMULA"`).
5. Upload xlsx (small fixture) → assert spreadsheet has expected sheets.
6. Export xlsx → assert downloaded bytes parse as valid xlsx.
7. Trigger 404 (bogus spreadsheet_id) → assert `SpreadsheetNotFound`.

Run locally with `source .env && cargo test --test gsheets_integration -- --ignored`.

### 9.3 Smoke graph

`tests/graphs/agents/gsheets_smoke.json` — DAG with one agent that:

1. Creates a spreadsheet ("E Smoke <timestamp>").
2. Adds a sheet "Numbers".
3. Sets A1..A5 to 10/20/30/40/50.
4. Sets B1 to `=SUM(A1:A5)`.
5. Reads B1 with `value_render: "UNFORMATTED_VALUE"` — confirm 150.
6. Reads B1 with `value_render: "FORMULA"` — confirm "=SUM(A1:A5)".
7. Final agent report.

Run via the standard `dag_engine run` command; gated by env (skip if no
GOOGLE_APPLICATION_CREDENTIALS).

---

## 10. Performance & rate limit considerations

- Default Sheets quotas (per-project, per-user): 60 read req/min/user
  and 60 write req/min/user (300/min/project).
- `set_range` always uses `values.update` (one HTTP call per range
  write) rather than per-cell `set_cell` loops — operators writing
  bulk data should call `set_range` once with a 2D array.
- Cross-sheet analyses typically: 2 reads (one per sheet) + 1 batch
  write — well within quotas for normal use.
- For very large reads (>10K cells), the API paginates implicitly via
  range; tools accept arbitrary ranges with no client-side splitting in
  v1. If a user hits the per-response size cap, the response surfaces
  Google's error verbatim (operator can split the range).
- Retry with exponential backoff (3 retries, 1s/2s/4s) absorbs typical
  429 bursts. Beyond that, surfaces `rate_limit` and the agent decides
  whether to back off further.

---

## 11. Migration / back-compat

- **No back-compat concerns for colmena itself** — this is a brand-new
  module and tool set.
- **No impact on existing CRDT subsystems (B/C/D/F)** — they continue to
  work unchanged. The two stacks coexist.
- **Skill update**: the existing `crdt-doc-cross-sheet-analysis` skill is
  not modified. A new sibling skill `gsheets-cross-sheet-analysis` is
  added with the same 6 patterns adapted.
- **Tool registry**: 9 new tools added; no existing tools renamed or
  removed.
- **ADP (downstream consumer)**: zero breaking changes. ADP can opt in
  by enabling the new tools per-graph and providing credentials via env
  vars. If ADP later needs OAuth user-scoped auth, that's a v1.1
  conversation between this repo and the ADP team.

---

## 12. Out-of-scope (v1.1 BACKLOG items)

- `gsheets_list_spreadsheets()` with shared-Drive-folder filter.
- OAuth user-scoped auth (via a new `SheetsAuthProvider` trait).
- Cell formatting (colors, borders, column widths).
- Charts, conditional formatting, data validation.
- Sharing / permissions (`drive.permissions.*`).
- Revisions / undo via Drive Revisions API.
- Webhook / push notifications for sheet changes.
- Real-time collab presence indicators (Google has its own; we don't
  duplicate).
- Apps Script execution from colmena.
- Per-spreadsheet auth (currently one auth identity for all calls in a
  process; v1.1 might allow per-call credential overrides).

---

## 13. Implementation task preview

| ID | Task | Est. LOC |
|---|---|---:|
| E-T1 | `gsheets/domain` — types, errors, trait | 200 |
| E-T2 | `gsheets/infrastructure/auth.rs` (reuses yup-oauth2 pattern) | 80 |
| E-T3 | `gsheets/infrastructure/http_client.rs` — read/write endpoints | 400 |
| E-T4 | `gsheets/infrastructure/http_client.rs` — create/admin + xlsx upload | 200 |
| E-T5 | `gsheets/application` — use cases delegating to trait | 100 |
| E-T6 | 9 tool dispatchers + tool defs | 500 |
| E-T7 | Tool registry — register the 9 new tools | 30 |
| E-T8 | Mock client + unit tests (~40) | 400 |
| E-T9 | Integration test `#[ignore]`-gated (~7 scenarios) | 200 |
| E-T10 | Smoke graph `gsheets_smoke.json` + run E2E | 150 |
| E-T11 | Skill `gsheets-cross-sheet-analysis` (~150 lines from F's skill) | docs |
| E-T12 | Docs: dev guide §5.9 + node_configurations.json + BACKLOG + CHANGELOG | docs |
| E-T13 | Final sweep: cargo test + clippy + fmt + smoke | — |

**Total estimated: ~2200 LOC + tests + docs. ~3-5 days subagent-driven.**

---

## 14. Open questions

None at design time. All major decisions are explicit in §2 (scope), §3
(architecture), §5 (auth), §6 (tools), §7 (flows), §8 (errors), §12
(out-of-scope).

If a reviewer wants to bring forward a v1.1 item (e.g.
`gsheets_list_spreadsheets()`), move that row from §12 to §13 and the
plan grows by ~150-300 LOC.
