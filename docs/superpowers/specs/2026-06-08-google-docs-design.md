# Google Docs Integration — Design Spec (Subsystem G)

**Date:** 2026-06-08
**Status:** Draft → pending user review
**Subsystem:** G — Google Docs integration
**Prior art:** Subsystem E (`gsheets`, shipped 2026-06-05) provides the
template for auth, hexagonal layout, dispatcher patterns, and the
"toolkit-alias" UX. This spec adapts that template for word-processing
documents, where the data model is text-structural (paragraphs, runs,
indices) instead of tabular.

---

## 1. Problem statement

Colmena agents today can read/write/analyse spreadsheets (`crdt_doc_*`
for local CRDT spreadsheets, `gsheets_*` for Google Sheets). They have
NO first-class tooling for Google Docs. Operators who want agents to
**write reports, fill templates, edit long-form text** in the user's
Google Drive have no choice but to drop into raw HTTP nodes against the
Google Docs API and hand-roll all the structural logic — which means
the LLM ends up handling UTF-16 indices, write-backwards ordering, and
batchUpdate request shaping. That is unsafe in practice: every demo
where an LLM directly emits index-based edits ends with the model
silently rewriting unrelated parts of the document.

This subsystem gives agents first-class tools to operate on Google Docs
**content-addressed**: the agent specifies WHAT text to change (not
WHERE), and the dispatcher resolves indices internally. The dispatcher
also tracks human edits via the Drive Revisions API and blocks any agent
write that would silently overwrite a recent human change.

The design principle is **surgical precision over regeneration**: every
edit touches the smallest possible range, every edit is auditable, no
agent action ever re-emits the full document.

---

## 2. Goals & non-goals

### Hard architectural constraint

**Colmena is an open-source library. Nothing ADP-specific lands here.**
All auth, scope, and behavioural config flows in through env vars / node
config / traits — no hardcoded ADP OAuth client, no ADP-specific
assumptions. ADP can implement OAuth user-scoped flows on top of
colmena, but colmena itself ships only Service Account JSON + ADC auth
in v1 (same as gsheets §5).

### Goals (v1)

- ~22 synthetic LLM tools (see §6) that follow the **content-addressed
  surgical edit** model. The LLM never computes a UTF-16 index.
- Two auth paths: **AUTH-A** Service Account JSON (via
  `GOOGLE_APPLICATION_CREDENTIALS`) and **AUTH-C** Application Default
  Credentials. Same `yup-oauth2` plumbing as `gsheets` / `image_generation`.
- **User-owned docs.** The agent never owns docs. Two creation paths:
  (i) the user creates the doc and shares it with the SA;
  (ii) the user creates a shared folder, the agent creates new docs
  inside it — Drive inherits permissions automatically.
- **Multi-tab support** from day 1. Google Docs supports multi-tab
  documents (each tab is a sub-document). Every edit tool accepts an
  optional `tab_id`; `gdocs_add_tab` / `gdocs_list_tabs` round it out.
- **Markdown-first creation** via Drive's native `text/markdown` import.
  Detects which markdown elements were lost during conversion and
  returns the loss list in the tool result.
- **Markdown-aware partial inserts.** When the agent wants to insert
  formatted content into an existing doc (a new section, a bullet list,
  a table), the dispatcher parses the markdown locally with
  `pulldown-cmark` and emits the corresponding batchUpdate request mix
  in one atomic call.
- **Human↔agent co-editing safety.** Every edit tool runs a revision
  check before applying (cached 5 seconds per doc). If the doc changed
  since the agent's last known revision and the human's changes overlap
  the agent's intended scope, the edit fails with `HumanChangesPending`.
  The agent must explicitly call `gdocs_acknowledge_human_changes` to
  proceed. Non-overlapping human changes surface as a soft warning.
- **`outline_snapshot` in every tool result.** Each `gdocs_*` response
  carries an up-to-date outline (paragraph numbers + previews) so the
  agent always has fresh structural addressing without having to re-read.
- **Ambiguity / over-broad action protection.** Find/replace operations
  with `>=5 matches` in scope return `ConfirmManyMatches` until the
  agent re-calls with `confirm_many: true`. Find/replace with multiple
  matches and no disambiguator returns `AmbiguousMatch` with previews.
- **`apply_edits`** — an atomic compound: N content-addressed edits
  composed into one `batchUpdate`. If any one fails, none apply.
- **Hyperlinks + text styles + headings** via `style_text` (content-addressed).
- **Export** to `docx`, `pdf`, `markdown`, `txt`, `rtf`, `epub`, `odt`,
  `html`.

### Non-goals (deferred to v1.1)

- **Suggesting mode** for edits (`writeControl.suggestionsEnabled`).
  v1 ships `mode: "direct"` only — but `mode` is wired as a no-op param
  on every edit tool so adding `"suggest"` in v1.1 is non-breaking.
- **Surgical table cell edits.** v1 supports inserting tables via
  markdown (with `|` syntax) and deleting them whole; editing individual
  cells in place is v1.1.
- **Drive Comments API integration** (read/write doc comments — useful
  for human↔agent bidirectional messaging inside the doc itself).
- **Drive Revisions restore / undo** (the API can pin a prior revision
  as the current one; we read revisions for human-change detection but
  never restore).
- **`acknowledge_human_changes` enriched mode** that summarises human
  edits via a cheap-tier LLM call and flags potential conflicts with
  the agent's pending plan. v1 ships "acuso recibo simple"; v1.1 can
  add the summary fields without breaking the v1 contract.
- **Image insertion of any kind.** v1 does NOT ship `insert_image_*`.
  Markdown image references in `create_from_markdown` are logged as
  `LossyConversion` records and skipped. v1.1 adds a dedicated
  `gdocs_insert_image_after_text` tool that takes an attachment_id or
  a public URL, plus richer flows (replace, resize, caption).
- **`gdocs_list_documents`** (Drive discovery scoped to a folder). Drive
  has `files.list`; v1 leaves it out for the same reason gsheets did —
  scoping cleanly is the v1.1 conversation.
- **OAuth user-scoped flow** — same posture as gsheets v1.
- **Real-time presence indicators** — Google has them natively in the
  UI; nothing to do.

---

## 3. Architecture

```
┌──────────────────────────────────────────────────────────────────┐
│                          Agent (LLM)                              │
└────┬──────────────────────────────────────────────┬──────────────┘
     │ gdocs_replace_text / insert_after_text / ... │ apply_edits
     ▼                                              ▼
┌──────────────────────────────────────────────────────────────────┐
│  llm_synthetic_tools/                                             │
│  ├─ gdocs_tools.rs       (~22 dispatchers)                        │
│  └─ markdown_to_docs_ops.rs                                       │
│     (pure markdown→batchUpdate ops converter, no I/O)             │
└────┬──────────────────────────────────────────────────────────────┘
     │ delegates to
     ▼
┌──────────────────────────────────────────────────────────────────┐
│  src/libs/colmena/src/gdocs/    (NEW MODULE)                      │
│  ├─ domain/                                                       │
│  │   ├─ traits.rs:        `DocsClient` trait (the port)           │
│  │   ├─ types.rs:         DocumentId, RevisionId, TabId, Scope,   │
│  │   │                    StylePatch, EditResult, OutlineEntry,…  │
│  │   └─ errors.rs:        `DocsError` enum                        │
│  ├─ application/                                                  │
│  │   ├─ replace_text.rs:    scoped find/replace use case          │
│  │   ├─ insert.rs:          insert_after / insert_between / etc.  │
│  │   ├─ style.rs:           content-addressed style updates       │
│  │   ├─ co_edit_guard.rs:   revision check + human-diff resolver  │
│  │   └─ apply_edits.rs:     compound atomic batch                 │
│  └─ infrastructure/                                               │
│      ├─ http_client.rs:     `GoogleDocsHttpClient` (impl trait)   │
│      ├─ auth.rs:            ADC + SA JSON (reuses gsheets pattern)│
│      ├─ config.rs:          `GDocsConfig`                         │
│      ├─ revision_store.rs:  Postgres-backed session state         │
│      └─ outline_cache.rs:   in-memory 5s cache per (sess, doc_id) │
└──────────────────────────────────────────────────────────────────┘
```

**Hexagonal.** `DocsClient` trait in domain. `GoogleDocsHttpClient` in
infrastructure. Tools always reach for the trait — `MockDocsClient`
enables full dispatcher testing without hitting Google.

**Zero new external crates.** `yup-oauth2` already present (gsheets,
image_generation). `pulldown-cmark` already present (`documents/`
subsystem). `regex` already present. `reqwest`, `serde_json`, `tokio`
already present. **One** new internal table in Postgres for revision
state — `gdocs_session_state`, dead simple migration.

**Layering note.** The `application/` layer is thicker here than in
`gsheets`. That's deliberate: `replace_text`, `insert_after`, and
`apply_edits` all share the same co-edit-guard + outline-snapshot
+ batchUpdate-emit pipeline, and pulling that into application use
cases lets the dispatcher layer stay thin (parse args → call use case
→ shape JSON result).

---

## 4. Components

### 4.1 `gdocs/domain/types.rs`

```rust
/// Stable Google identifier for a Doc (the part after `/document/d/`).
pub struct DocumentId(pub String);

/// Stable Google identifier for a tab within a multi-tab Doc.
pub struct TabId(pub String);

/// Opaque Google revision identifier. We pass the latest one back to
/// Google as `writeControl.requiredRevisionId` for optimistic concurrency.
pub struct RevisionId(pub String);

/// Where to look for the `find` text. `All` is doc-wide (default).
pub enum Scope {
    All,
    Tab { tab_id: TabId },
    Paragraph { n: u32 },                  // 1-based, as the agent sees it
    UnderHeading { heading: String },       // "## Section 2"
    BetweenHeadings { after: String, before: String },
}

/// Style patch supplied by the agent. All fields optional — only set
/// fields are applied. Mirrors the shape of Google's TextStyle subset
/// we expose.
pub struct StylePatch {
    pub bold:           Option<bool>,
    pub italic:         Option<bool>,
    pub underline:      Option<bool>,
    pub strikethrough:  Option<bool>,
    pub font_size_pt:   Option<f32>,
    pub foreground:     Option<RgbColor>,  // {r,g,b} 0..1
    pub background:     Option<RgbColor>,
    pub link:           Option<String>,     // URL — sets hyperlink
    pub heading_level:  Option<HeadingLevel>, // NORMAL, H1..H6, TITLE, SUBTITLE
}

pub enum HeadingLevel { Normal, H1, H2, H3, H4, H5, H6, Title, Subtitle }

/// Outcome of a single content-addressed write. Multiple of these in
/// `EditResult.changes` when one tool call resolves to N matches.
pub struct ChangeRecord {
    pub kind: ChangeKind,                   // replace | insert | delete | style
    pub paragraph: u32,
    pub before: Option<String>,
    pub after: Option<String>,
    pub tab_id: Option<TabId>,
}

pub struct EditResult {
    pub changes: Vec<ChangeRecord>,
    pub revision_id_after: RevisionId,
    pub outline_snapshot: Vec<OutlineEntry>,
    pub lossy_conversions: Vec<LossyConversion>,         // create_from_markdown only
    pub pending_human_changes_outside_scope: Vec<HumanChange>, // soft warning
}

pub struct OutlineEntry {
    pub paragraph: u32,
    pub tab_id: Option<TabId>,
    pub kind: ParagraphKind,                // heading_1..6 | paragraph | list_item | table_row
    pub text_preview: String,               // first 80 chars
}

pub struct HumanChange {
    pub kind: HumanChangeKind,              // insert | modify | delete
    pub paragraph: u32,
    pub preview: String,                    // diff-style ~80 chars
    pub modified_time: chrono::DateTime<chrono::Utc>,
    pub modifying_user: Option<String>,     // email if Google reports it
}

pub struct LossyConversion {
    pub element_type: String,               // "footnote" | "table_merged_cells" | …
    pub original_markdown: String,          // first 200 chars
}
```

### 4.2 `gdocs/domain/traits.rs` — `DocsClient`

```rust
#[async_trait]
pub trait DocsClient: Send + Sync {
    // ── Lifecycle ────────────────────────────────────────────────
    async fn create(&self, title: &str, parent_folder: Option<&str>)
        -> Result<DocumentMeta, DocsError>;

    async fn create_from_markdown(
        &self, title: &str, md: &str, parent_folder: Option<&str>,
    ) -> Result<CreateFromMarkdownResult, DocsError>;

    async fn create_from_docx(
        &self, title: &str, bytes: Vec<u8>, parent_folder: Option<&str>,
    ) -> Result<DocumentMeta, DocsError>;

    async fn share(&self, id: &DocumentId, email: &str, role: ShareRole)
        -> Result<(), DocsError>;

    async fn export(&self, id: &DocumentId, format: ExportFormat)
        -> Result<Vec<u8>, DocsError>;

    // ── Reads ────────────────────────────────────────────────────
    async fn get(&self, id: &DocumentId) -> Result<DocumentSnapshot, DocsError>;

    async fn read_as_markdown(&self, id: &DocumentId, tab_id: Option<&TabId>)
        -> Result<String, DocsError>;

    async fn read_outline(&self, id: &DocumentId, tab_id: Option<&TabId>)
        -> Result<Vec<OutlineEntry>, DocsError>;

    async fn list_named_ranges(&self, id: &DocumentId)
        -> Result<Vec<NamedRangeMeta>, DocsError>;

    async fn list_tabs(&self, id: &DocumentId)
        -> Result<Vec<TabMeta>, DocsError>;

    async fn list_revisions_since(&self, id: &DocumentId, since: &RevisionId)
        -> Result<Vec<RevisionMeta>, DocsError>;

    /// Fetch a historical snapshot of a doc by revision, as plain text.
    /// Used by the co-edit guard to diff prior-vs-current and attribute
    /// changes to paragraphs. We use `revisions.get` with media export
    /// to `text/plain` (cheap; sufficient to compute paragraph diffs).
    async fn get_at_revision(&self, id: &DocumentId, revision: &RevisionId)
        -> Result<String, DocsError>;

    // ── Raw batchUpdate (used by application layer) ─────────────
    async fn batch_update(
        &self, id: &DocumentId, requests: Vec<serde_json::Value>,
        required_revision: Option<&RevisionId>,
    ) -> Result<BatchUpdateResult, DocsError>;

    // ── Tabs ─────────────────────────────────────────────────────
    async fn add_tab(&self, id: &DocumentId, title: &str, after_tab: Option<&TabId>)
        -> Result<TabMeta, DocsError>;

    // ── Named ranges ─────────────────────────────────────────────
    async fn create_named_range(
        &self, id: &DocumentId, name: &str, scope: Scope,
    ) -> Result<NamedRangeMeta, DocsError>;
}
```

Note: high-level tools (`replace_text`, `insert_after_text`, etc.) live
in `gdocs/application/`, not on the trait. The trait is the lowest-level
HTTP-shaped port. The application use cases call `get` + `batch_update`
+ `list_revisions_since` to assemble the high-level operations.

### 4.3 `gdocs/domain/errors.rs`

```rust
#[derive(Debug, thiserror::Error)]
pub enum DocsError {
    #[error("gdocs_not_configured: {0}")]
    NotConfigured(String),

    #[error("auth_failed: {0}")]
    AuthFailed(String),

    #[error("document_not_found: {0}")]
    DocumentNotFound(String),

    #[error("tab_not_found: {0}")]
    TabNotFound(String),

    #[error("permission_denied: share the doc with {0}")]
    PermissionDenied(String),                  // SA email

    #[error("no_parent_folder_configured")]
    NoParentFolder,

    // ── Content-addressed edit errors (returned to LLM as structured JSON) ─
    #[error("ambiguous_match: {find}")]
    AmbiguousMatch { find: String, matches: Vec<MatchPreview> },

    #[error("text_not_found: {find}")]
    TextNotFound { find: String, fuzzy_suggestions: Vec<String> },

    #[error("confirm_many_matches: {count} matches in scope")]
    ConfirmManyMatches { find: String, count: u32, preview: Vec<MatchPreview> },

    #[error("human_changes_pending")]
    HumanChangesPending {
        since: chrono::DateTime<chrono::Utc>,
        changes_overlapping_scope: Vec<HumanChange>,
        changes_outside_scope: Vec<HumanChange>,
    },

    #[error("scope_crosses_structural_boundary: {0}")]
    ScopeCrossesBoundary(String),              // find spans paragraphs / tables

    #[error("tab_exists: {0}")]
    TabExists(String),

    // ── Transport ────────────────────────────────────────────────
    #[error("rate_limit: retry after {0}s")]
    RateLimit(u32),

    #[error("conflict: doc changed during write; revision check failed")]
    Conflict,

    #[error("http_error: {0}")]
    Http(String),

    #[error("invalid_args: {0}")]
    InvalidArgs(String),

    #[error("internal: {0}")]
    Internal(String),
}
```

### 4.4 `gdocs/infrastructure/auth.rs`

Identical pattern to `gsheets/infrastructure/auth.rs` and
`image_generation.rs:572` — `ApplicationDefaultCredentialsAuthenticator`
from `yup-oauth2`, scopes `documents` + `drive.file`, token cached
in-memory 50 min TTL. **No new code beyond a copy/paste-and-rename**.
Re-using gsheets's `auth.rs` verbatim (single shared module) was
considered but the scope sets diverge, and the existing module already
has gsheets-specific error messages — cleaner to fork.

### 4.5 `gdocs/infrastructure/http_client.rs`

`GoogleDocsHttpClient` implements `DocsClient`. Endpoints:

| Operation | Endpoint |
|---|---|
| Create blank | `POST https://docs.googleapis.com/v1/documents` |
| Get doc tree | `GET  https://docs.googleapis.com/v1/documents/{id}` (with `includeTabsContent=true` for multi-tab) |
| batchUpdate | `POST https://docs.googleapis.com/v1/documents/{id}:batchUpdate` |
| Create from markdown / docx | `POST https://www.googleapis.com/upload/drive/v3/files?uploadType=multipart` with `mimeType=application/vnd.google-apps.document` |
| Export | `GET  https://www.googleapis.com/drive/v3/files/{id}/export?mimeType=...` |
| Share | `POST https://www.googleapis.com/drive/v3/files/{id}/permissions` |
| List revisions | `GET  https://www.googleapis.com/drive/v3/files/{id}/revisions` (filtered client-side by `modifiedTime`) |

Retry policy identical to gsheets: 3 retries with 1s/2s/4s backoff on
429 / 5xx; 4xx surfaces immediately as the appropriate `DocsError`
variant. 401 → refresh once and retry. 404 → `DocumentNotFound` or
`TabNotFound` (per endpoint).

### 4.6 `gdocs/infrastructure/config.rs`

```rust
pub struct GDocsConfig {
    pub credentials_path:        Option<PathBuf>,
    pub scopes:                  Vec<String>,
    pub default_parent_folder:   Option<String>,
    pub request_timeout:         Duration,
    pub max_retries:             u32,
    pub revision_cache_ttl:      Duration,        // default 5s
}

impl GDocsConfig {
    pub fn from_env() -> Self { /* mirrors GSheetsConfig */ }
}
```

Env vars:
- `GOOGLE_APPLICATION_CREDENTIALS` (shared with gsheets / image_gen).
- `COLMENA_GDOCS_SCOPES` (override). Defaults:
  `https://www.googleapis.com/auth/documents` +
  `https://www.googleapis.com/auth/drive.file`.
- `COLMENA_GDOCS_DEFAULT_PARENT_FOLDER_ID`.
- `COLMENA_GDOCS_REVISION_CACHE_SECS` (default 5).

### 4.7 `gdocs/infrastructure/revision_store.rs`

```rust
#[async_trait]
pub trait RevisionStore: Send + Sync {
    async fn get(&self, session_id: &str, doc_id: &DocumentId)
        -> Result<Option<RevisionId>, DocsError>;
    async fn put(&self, session_id: &str, doc_id: &DocumentId, rev: &RevisionId)
        -> Result<(), DocsError>;
}

pub struct PostgresRevisionStore { pool: PgPool }
```

Migration `gdocs_session_state` (additive — see §11):

```sql
CREATE TABLE IF NOT EXISTS gdocs_session_state (
    agent_session_id TEXT NOT NULL,
    document_id      TEXT NOT NULL,
    last_revision_id TEXT NOT NULL,
    last_edit_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (agent_session_id, document_id)
);
CREATE INDEX gdocs_session_state_last_edit_at
    ON gdocs_session_state (last_edit_at);
```

Key is `(agent_session_id, document_id)` — same convention as
`secure_value_mappings`, `dag_runs`, `llm_node_history` (CLAUDE.md
"Regla — Usar `--agent-session-id`"). The `last_edit_at` index supports
optional cleanup tasks later.

### 4.8 `gdocs/infrastructure/outline_cache.rs`

In-memory cache: `Mutex<HashMap<(agent_session_id, DocumentId), CachedSnapshot>>`.
Each entry: `{snapshot: DocumentSnapshot, fetched_at: Instant}`. TTL
configurable (default 5s). Used by the co-edit-guard pipeline to avoid
re-fetching the doc on tool-call bursts. **Not** persistent across
process restarts — that's fine, a cold cache just means the next edit
pays one extra `documents.get`.

### 4.9 `markdown_to_docs_ops.rs`

Pure converter, **no I/O**:

```rust
pub fn markdown_to_batch_update_requests(
    md: &str,
    insertion_point: InsertionPoint,   // index + tabId, OR endOfSegment
) -> Result<Vec<serde_json::Value>, MarkdownError>;
```

Tokenises markdown with `pulldown-cmark`, then walks the event stream
to emit a sequence of:
- `InsertTextRequest` (plain text body, write-backwards)
- `UpdateParagraphStyleRequest` (`HEADING_1`..`6`, `TITLE`, normal)
- `CreateParagraphBulletsRequest` (for `-` / `*` / `1.` lists)
- `UpdateTextStyleRequest` (`bold`, `italic`, `underline`, `link.url`)
- `InsertTableRequest` + per-cell `InsertTextRequest` for tables

All requests are emitted in the order Google's batchUpdate prefers
(text first, then style overlays — Google applies them in array order).

**Lossy elements** (footnotes, image references inside markdown, complex
nested tables) emit a `LossyConversion` record and skip the unsupported
fragment; the surrounding markdown still converts. The dispatcher
surfaces these to the agent.

Golden tests: 30+ markdown→ops fixtures committed to
`tests/gdocs_markdown_fixtures/`, asserting deterministic op output.

---

## 5. Auth model

### Supported in v1

- **AUTH-A — Service Account JSON.** `GOOGLE_APPLICATION_CREDENTIALS`.
- **AUTH-C — Application Default Credentials.** Fallback.

Both flow through `yup-oauth2::ApplicationDefaultCredentialsAuthenticator`,
same as gsheets. Token cache: 50-min TTL.

### Scopes (defaults)

- `https://www.googleapis.com/auth/documents` — full read/write to Docs.
  Sensitive (requires app verification for non-internal use); operators
  can downgrade to `documents.readonly` for read-only agent profiles.
- `https://www.googleapis.com/auth/drive.file` — per-file access. Used
  for create/share/export. **Non-sensitive** — strongly preferred over
  `drive` for compliance.

Overridable per-operator via `COLMENA_GDOCS_SCOPES` (comma-sep). v1.1
work that needs `drive.metadata.readonly` (e.g. `gdocs_list_documents`)
is BACKLOG.

### User ↔ agent ownership model

The agent NEVER owns docs in v1. Two operator-configured patterns:

**Pattern 1: User shares doc with SA.** User creates the doc in their
own Drive, clicks Share, adds the SA email as Editor. Agent operates
on the doc by `document_id`. The agent's `create*` tools fail unless
a parent folder is configured (see Pattern 2).

**Pattern 2: User shares a folder with SA.** User creates a folder in
their Drive, shares it with the SA as Editor. Operator sets
`COLMENA_GDOCS_DEFAULT_PARENT_FOLDER_ID=<folder_id>` (or per-tool
`fixed_config.parent_folder_id`). When the agent calls
`gdocs_create*`, Drive creates the new doc inside that folder; permissions
inherit from the folder → the user has access automatically.

If neither pattern is set up when `create*` is called:
`DocsError::NoParentFolder` with a clear hint.

---

## 6. Tool surface — agent-facing

22 tools, grouped:

### Lifecycle (5)

| Tool | Args | Returns |
|---|---|---|
| `gdocs_create` | `{title, parent_folder_id?}` | `{ok, doc_id, url, revision_id}` |
| `gdocs_create_from_markdown` | `{title, markdown, parent_folder_id?}` | `{ok, doc_id, url, revision_id, outline_snapshot, lossy_conversions}` |
| `gdocs_create_from_docx` | `{title, attachment_id, parent_folder_id?}` | `{ok, doc_id, url, revision_id, outline_snapshot}` |
| `gdocs_share` | `{doc_id, email, role}` — role ∈ `reader \| commenter \| writer` | `{ok}` |
| `gdocs_export` | `{doc_id, format}` — format ∈ `docx \| pdf \| markdown \| txt \| rtf \| epub \| odt \| html` | `{ok, attachment_id}` |

### Tabs (2)

| Tool | Args | Returns |
|---|---|---|
| `gdocs_list_tabs` | `{doc_id}` | `{ok, tabs: [{tab_id, title, index, parent_tab_id?}]}` |
| `gdocs_add_tab` | `{doc_id, title, after_tab_id?, markdown?}` — collides with existing title → `TabExists` (override via `fixed_config.on_existing_tab: "auto_suffix"`) | `{ok, tab_id, title, revision_id, outline_snapshot, lossy_conversions?}` |

### Reads (3)

| Tool | Args | Returns |
|---|---|---|
| `gdocs_read_as_markdown` | `{doc_id, tab_id?}` | `{ok, markdown, revision_id}` |
| `gdocs_read_outline` | `{doc_id, tab_id?}` | `{ok, outline: [{paragraph, kind, text_preview, tab_id?}], revision_id}` |
| `gdocs_list_named_ranges` | `{doc_id}` | `{ok, named_ranges: [{name, range}]}` |

### Surgical text edits (5)

All edits run the **co-edit guard** before applying (§8). All return
`EditResult` (changes + revision_id_after + outline_snapshot).

| Tool | Args |
|---|---|
| `gdocs_replace_text` | `{doc_id, find, replace, scope?, case_sensitive?, whole_word?, dry_run?, confirm_many?, occurrence?, anchor?, mode?}` |
| `gdocs_insert_after_text` | `{doc_id, anchor, new_markdown, occurrence?, mode?}` |
| `gdocs_insert_before_text` | `{doc_id, anchor, new_markdown, occurrence?, mode?}` |
| `gdocs_insert_between` | `{doc_id, after_heading, before_heading, new_markdown, mode?}` — `before_heading` may be `null` meaning "end of section" |
| `gdocs_delete_text` | `{doc_id, find, scope?, case_sensitive?, whole_word?, occurrence?, confirm_many?, mode?}` |

Notes:
- `scope` is `"all"` by default. Other shapes: `{tab_id}`, `{paragraph: N}`,
  `{under_heading: "..."}`, `{between_headings: [a, b]}`. Mutually
  exclusive with `paragraph` / `occurrence` (one disambiguator at a time).
- `mode` is `"direct"` only in v1 — accepted but no-op-validated for forward compat
  with v1.1 `"suggest"`.
- `whole_word: true` triggers `\bfind\b` regex matching client-side
  (filters before emitting batchUpdate ops).
- `dry_run: true` runs the guard + match resolution + plan emission but
  skips the actual `batchUpdate`. Returns `{would_change: [...]}`.

### Structural (3)

| Tool | Args | Notes |
|---|---|---|
| `gdocs_replace_section` | `{doc_id, heading, new_markdown, tab_id?, mode?}` | Replaces everything between `heading` and the next same-or-higher level heading (or EOF). Co-edit-guarded. |
| `gdocs_append_markdown` | `{doc_id, markdown, tab_id?, mode?}` | Appends at end-of-segment. No ambiguity possible. |
| `gdocs_apply_edits` | `{doc_id, edits: [Edit, …], mode?}` | One atomic `batchUpdate` composed of N content-addressed edits. Co-edit-guard runs ONCE for the compound. If any sub-edit hits `AmbiguousMatch` / `TextNotFound` / `HumanChangesPending` → whole compound fails, nothing applied. |

### Style (1)

| Tool | Args |
|---|---|
| `gdocs_style_text` | `{doc_id, find, style: StylePatch, scope?, case_sensitive?, whole_word?, dry_run?, occurrence?, anchor?, mode?}` |

`StylePatch` is the struct in §4.1 — bold/italic/underline/strikethrough/
font_size_pt/foreground/background/link/heading_level. Same ambiguity
rules as `replace_text`.

### Named ranges (2)

| Tool | Args | Returns |
|---|---|---|
| `gdocs_create_named_range` | `{doc_id, name, scope}` — `scope` here is `{paragraph: N}` or `{find: "..."}` (creates a range around the first match of `find`) | `{ok, named_range_id, revision_id}` |
| `gdocs_replace_named_range` | `{doc_id, name, new_text, mode?}` | `{ok, changes, revision_id_after, outline_snapshot}` |

### Co-edition safety (1)

| Tool | Args | Returns |
|---|---|---|
| `gdocs_acknowledge_human_changes` | `{doc_id}` | `{ok, revision_id_now}` — updates `last_known_revision_id`. After this, the next edit proceeds without `HumanChangesPending`. |

**Toolkit alias.** Operators activate everything with
`enabled_tools: ["gdocs"]` (registered via `toolkit_packages.rs`).
Exclude write-side tools for read-only profiles:

```json
"enabled_tools": [
  "gdocs",
  "!gdocs_create", "!gdocs_create_from_markdown", "!gdocs_create_from_docx",
  "!gdocs_add_tab", "!gdocs_share",
  "!gdocs_replace_text", "!gdocs_delete_text",
  "!gdocs_insert_after_text", "!gdocs_insert_before_text", "!gdocs_insert_between",
  "!gdocs_replace_section", "!gdocs_append_markdown", "!gdocs_apply_edits",
  "!gdocs_style_text",
  "!gdocs_create_named_range", "!gdocs_replace_named_range"
]
```

(In practice we'll also ship a read-only sub-alias `gdocs_readonly` in
`toolkit_packages.rs` to make this less verbose.)

---

## 7. Data flow examples

### 7.1 Create + populate via markdown

```
agent: gdocs_create_from_markdown({
  title: "Q3 Sales Report",
  markdown: "# Q3 Sales\n\n## Highlights\n- 12% growth\n- New region\n\n## Details\n...",
})

dispatcher
  → Drive upload: POST /upload/drive/v3/files?uploadType=multipart
       part 1: {mimeType:"application/vnd.google-apps.document",
                name:"Q3 Sales Report",
                parents:[<configured folder>]}
       part 2: <markdown bytes, mime "text/markdown">
  → Google converts → returns {id: "doc_abc"}
  → re-export as markdown for loss-detection:
       GET /drive/v3/files/doc_abc/export?mimeType=text/markdown
  → diff(input_md, re_exported_md) → LossyConversions[]
  → docs.get(doc_abc) for outline_snapshot + revision_id

dispatcher → JSON:
  {ok: true, doc_id: "doc_abc",
   url: "https://docs.google.com/document/d/doc_abc",
   revision_id: "rev_1",
   outline_snapshot: [
     {paragraph: 1, kind: "heading_1", text_preview: "Q3 Sales"},
     {paragraph: 2, kind: "heading_2", text_preview: "Highlights"},
     ...
   ],
   lossy_conversions: []}

revision_store.put(session_id, doc_abc, "rev_1")
```

### 7.2 Surgical content-addressed replace, scoped under a heading

```
agent: gdocs_replace_text({
  doc_id: "doc_abc",
  find: "cliente",
  replace: "Acme",
  scope: {under_heading: "## Sección 2"},
  case_sensitive: false,
  whole_word: true,
})

dispatcher (application layer):
  1. co_edit_guard.check(session_id, doc_abc, scope) →
       fetches outline (cache hit if <5s old, else docs.get),
       calls list_revisions_since(doc_abc, "rev_1"),
       filters revisions not authored by SA,
       computes overlap of human changes vs scope.
     If overlapping → return HumanChangesPending error JSON.
  2. resolve_scope_to_range(outline, scope) → range_start..range_end
  3. extract_text_in_range(snapshot, range) → segment_text
  4. matches = regex(\bcliente\b, case_insensitive).find_iter(segment_text)
  5. if matches.len() >= 5 and not confirm_many → ConfirmManyMatches
  6. if matches.len() == 0 → TextNotFound (with fuzzy suggestions)
  7. build batchUpdate requests (write-backwards):
     for each match (reverse order):
       DeleteContentRangeRequest{range:{startIndex,endIndex,tabId}}
       InsertTextRequest{location:{index,tabId}, text:"Acme"}
  8. POST documents/doc_abc:batchUpdate
        {requests:[…], writeControl:{requiredRevisionId:"rev_1"}}
     → on 400 "Invalid requiredRevisionId" → re-run guard once,
       refetch, retry up to 3 times → if still failing, Conflict.
  9. on success: revision_store.put(session_id, doc_abc, new_rev)
 10. fetch fresh outline → outline_snapshot

dispatcher → JSON:
  {ok: true,
   changes: [
     {kind: "replace", paragraph: 8, before: "...el cliente confirmó...",
      after: "...el Acme confirmó..."},
     {kind: "replace", paragraph: 11, before: "...datos del cliente...",
      after: "...datos del Acme..."}
   ],
   revision_id_after: "rev_2",
   outline_snapshot: [...],
   pending_human_changes_outside_scope: []}
```

### 7.3 Apply N edits atomically

```
agent: gdocs_apply_edits({
  doc_id: "doc_abc",
  edits: [
    {tool: "replace_text", find: "draft", replace: "FINAL", scope: "all"},
    {tool: "style_text", find: "FINAL", style: {bold: true, foreground: {r:1,g:0,b:0}}},
    {tool: "insert_after_text", anchor: "## Conclusiones",
     new_markdown: "Aprobado por la Dirección.\n"}
  ]
})

dispatcher:
  1. co_edit_guard.check ONCE for the union of all scopes.
  2. resolve each edit to a list of batchUpdate requests.
  3. flatten + dedup + sort write-backwards across the whole compound.
  4. one POST batchUpdate (atomic per Google's docs).
  5. on any sub-resolution failure (AmbiguousMatch on edit 2 etc.) →
     whole compound fails, nothing applied, error pinpoints which sub-edit.

dispatcher → JSON:
  {ok: true, changes: [...all changes...], revision_id_after, outline_snapshot}
```

### 7.4 Human edits detected, agent acknowledges, retries

```
[Turno N]   agent → gdocs_replace_text(...) → succeeds. last_known = R_N
[fuera de banda] user opens doc, edits 2 paragraphs → doc is now at R_N+2
[Turno N+1] agent → gdocs_replace_text(another find/replace)
                  → co_edit_guard detects R_now > R_known
                  → 1 of 2 human changes overlaps the requested scope
                  → return HumanChangesPending JSON
            agent presents to user, user says "ok, proceed"
            agent → gdocs_acknowledge_human_changes(doc_abc)
                  → last_known = R_now, returns {ok}
            agent → gdocs_replace_text(...) → succeeds, last_known = R_now+1
```

---

## 8. Co-edition safety pipeline

This is the heart of the design. Every edit tool runs this before
touching `batchUpdate`:

```
fn co_edit_guard(session_id, doc_id, intended_scope) -> Result<GuardOk, DocsError>:
  let cache_key = (session_id, doc_id)
  let snapshot = outline_cache.get_or_fetch(cache_key, ttl=5s)
  let known_rev = revision_store.get(session_id, doc_id)?  // None on first contact
  let current_rev = snapshot.revision_id

  if known_rev.is_none() or known_rev == current_rev:
    return Ok(GuardOk { snapshot, current_rev })

  // human edits happened — fetch the diff
  let revisions = http.list_revisions_since(doc_id, &known_rev)?
  let human_revisions = revisions.filter(r => r.modifying_user != sa_email)
  if human_revisions.is_empty():
    // all changes are ours (unusual; maybe parallel agent process)
    return Ok(GuardOk { snapshot, current_rev })

  let prior_snapshot = http.get_at_revision(doc_id, known_rev)?
  let human_changes = diff_outlines(prior_snapshot, snapshot)
  let (overlap, outside) = partition_by_scope(human_changes, intended_scope)

  if !overlap.is_empty():
    Err(HumanChangesPending { changes_overlapping_scope: overlap,
                              changes_outside_scope:   outside, since: ... })
  else:
    // soft warning, return Ok but include outside-scope changes in result
    Ok(GuardOk { snapshot, current_rev, soft_warning: outside })
```

Implementation notes:

- **`diff_outlines`** uses the `similar` crate (added as a new dep —
  ~80 KB, MIT-licensed, widely used). Operates on paragraph-text-preview
  arrays — coarse enough to be cheap, fine enough to attribute changes
  to a paragraph number.
- **`partition_by_scope`** resolves intended scope to a paragraph range
  and checks whether any human-change paragraph falls inside.
- **Drive Revisions API** notes:
  - The endpoint returns the LAST 200 revisions by default. For docs with
    very high churn, we may need to paginate; v1 takes the first page
    and warns if there are more (`revisions_truncated: true`).
  - `lastModifyingUser.emailAddress` is sometimes redacted in Drive
    (when a user isn't part of the same Workspace). When redacted, we
    can't attribute the change to a specific human, so we conservatively
    treat ALL unattributable revisions as human edits (safe default).
- **`get_at_revision`** uses `revisions.get?fields=...` to recover an
  older snapshot for diffing. Google's response for Docs revisions is
  not as rich as for binary files — it gives metadata + an exportable
  document. For v1 we re-fetch as `text/plain` per revision and diff
  text-only (no formatting attribution). Cheap and sufficient for the
  "what paragraphs changed" question.

- **Cache invalidation.** After any successful write, the cache entry
  for that `(session_id, doc_id)` is invalidated immediately so the
  next call sees fresh data.

---

## 9. Error handling — JSON shapes the LLM sees

All errors share the envelope `{error: <kind>, ...details}`. The kinds:

| Kind | Triggers when | Detail fields |
|---|---|---|
| `gdocs_not_configured` | No SA / ADC creds | `hint` |
| `auth_failed` | 401 after refresh | `message` |
| `document_not_found` | 404 on docs.get | `doc_id` |
| `tab_not_found` | tab_id resolves to no tab | `tab_id` |
| `permission_denied` | 403 | `hint: share with <sa_email>` |
| `no_parent_folder_configured` | `create*` without folder | `hint: set COLMENA_GDOCS_DEFAULT_PARENT_FOLDER_ID …` |
| `ambiguous_match` | >1 match, no disambiguator | `find`, `matches: [{paragraph, preview}]`, `valid_next_moves` |
| `text_not_found` | 0 matches | `find`, `fuzzy_suggestions` |
| `confirm_many_matches` | ≥5 matches, no `confirm_many: true` | `count`, `preview`, `valid_next_moves` |
| `human_changes_pending` | revision mismatch + overlap | `since`, `changes_overlapping_scope`, `changes_outside_scope`, `valid_next_moves` |
| `scope_crosses_boundary` | `find` would span paragraphs/tables | `message` |
| `tab_exists` | `add_tab` with collision and policy=fail | `title`, `existing_tab_id`, `valid_next_moves` |
| `rate_limit` | 429 after 3 retries | `retry_after_seconds` |
| `conflict` | `requiredRevisionId` rejected 3× | `message` |
| `http_error` | other 5xx / network | `message` |
| `invalid_args` | bad args | `message` |
| `lossy_conversion_warning` | (NOT an error — included in `ok` responses) | `lossy_conversions: [{element_type, original_markdown}]` |

The `valid_next_moves` pattern mirrors `SheetExists` in
sheets-write-safety: every recoverable error MUST surface 2-3 concrete
next-call examples the LLM can copy.

---

## 10. Markdown → Doc ops design notes

**Two paths** by use case:

| Use case | Strategy |
|---|---|
| `create_from_markdown` (full doc) | Drive native `text/markdown` import + post-hoc loss detection via re-export diff |
| `create_from_docx` (full doc) | Drive native conversion (same multipart upload pattern as gsheets `create_from_xlsx`) |
| `insert_*` / `replace_section` / `append_markdown` / `add_tab` (partial) | Our `markdown_to_docs_ops.rs` parses MD with `pulldown-cmark` and emits batchUpdate requests |

Why split: Drive's native conversion is high-fidelity for whole-doc
imports but offers no way to inject a partial fragment into an existing
doc cleanly. For partials we need our own emitter — but it's also
small (~600 LOC) because we only need a subset of markdown features:

- Paragraphs (default `NORMAL_TEXT`)
- Headings `#` … `######` → `HEADING_1` … `HEADING_6`
- Bold `**` → `textStyle.bold`
- Italic `*` → `textStyle.italic`
- Underline (HTML `<u>` only, MD doesn't have it) → ignored, lossy log
- Strikethrough `~~` → `textStyle.strikethrough`
- Links `[text](url)` → `textStyle.link.url`
- Inline code → `textStyle.font.name = "Roboto Mono"` (Google
  convention) + light background
- Code blocks ``` ``` → mono block (paragraph + monospace style)
- Bullet lists `-` / `*` → `CreateParagraphBulletsRequest BULLET_DISC_*`
- Numbered lists `1.` → `BULLET_NUMBERED_*`
- Tables `|` … `|` → `InsertTableRequest` + per-cell `InsertTextRequest`
- Horizontal rule `---` → `InsertSectionBreakRequest`
- Block quotes `>` → `paragraphStyle.indentStart` + custom border

Unsupported (logged as `LossyConversion`):
- Footnotes
- Definition lists
- HTML pass-through
- Math (`$…$`)
- Images via markdown reference — v1 logs them as lossy and skips
  (v1.1 will add `gdocs_insert_image_after_text`)

Golden tests cover all 14 supported elements + 5 unsupported (to
verify the lossy log is emitted correctly).

---

## 11. Testing strategy

### 11.1 Unit tests (target ~60)

Inline `#[cfg(test)]` in each module:

- `gdocs/domain` — type round-trips, scope parsing.
- `gdocs/application/replace_text.rs` — happy path, AmbiguousMatch,
  TextNotFound, ConfirmManyMatches, scope resolution per variant, whole_word filtering.
- `gdocs/application/co_edit_guard.rs` — no human change / overlap / outside-scope, redacted user email handling, paginated revisions.
- `markdown_to_docs_ops.rs` — golden tests for the 14 supported elements + lossy log for the 5 unsupported.
- `gdocs_tools.rs` — each dispatcher's arg parsing + error mapping.
- `MockDocsClient` covers every trait method.

### 11.2 Integration tests (`#[ignore]`-gated)

`tests/gdocs_integration.rs`:

Requires:
- `GOOGLE_APPLICATION_CREDENTIALS` → SA JSON.
- `COLMENA_GDOCS_TEST_PARENT_FOLDER_ID` → a folder shared with the SA.
- `DATABASE_URL` → postgres for revision store.

~10 scenarios:
1. Create + read_outline + replace_text + verify diff.
2. create_from_markdown with one unsupported element, verify lossy_conversions.
3. apply_edits atomic — 3 sub-edits, all succeed, single batchUpdate.
4. apply_edits atomic — 1 sub-edit fails AmbiguousMatch, none applied.
5. style_text — bold + link, verify TextStyle on the matched range.
6. Multi-tab: add_tab + insert in second tab + verify tab_id routing.
7. Human edit simulation: write, mutate via raw HTTP as another identity,
   verify HumanChangesPending then acknowledge.
8. ConfirmManyMatches: replace_text with 7 matches → error → re-call with confirm_many.
9. Named range create + replace.
10. Export as docx → re-attach → verify roundtrip.

### 11.3 Smoke graph

`tests/graphs/agents/gdocs_smoke.json` — DAG with one agent that:
1. Creates a doc from markdown.
2. Reads outline.
3. Replaces a known string in a known scope.
4. Inserts a section between two headings.
5. Adds a tab with markdown content.
6. Exports as pdf.
7. Reports.

Runs against real APIs (env-gated, skipped if creds absent).

### 11.4 Postgres migration

Additive — `gdocs_session_state` is a new table. No backfill. Migration
file: `migrations/<seq>_gdocs_session_state.sql`.

---

## 12. Performance & quota considerations

- **Docs API quotas:** ~300 reads/min + 60 writes/min per user. With
  the 5-second outline cache, a burst of N edits in 5s consumes
  1 docs.get + 1 batchUpdate + (N-1) batchUpdates = N+1 requests. The
  cache absorbs the read amplification.
- **Drive Revisions API:** ~1000 reqs/100s per user. We hit it on every
  edit when revision check is active — well within quotas for normal
  use. Could become a hotspot in heavy multi-agent scenarios; v1.1
  could add session-scoped batching.
- **Latency budget per edit:**
  - cache hit: ~0ms outline + ~1 list_revisions_since (~150ms) + 1 batchUpdate (~400ms) = ~550ms
  - cache miss: + 1 docs.get (~250ms) ≈ ~800ms
  - confirmed acceptable for chat-paced agent flows.
- **`apply_edits` win:** N sub-edits collapse to 1 docs.get + 1
  list_revisions_since + 1 batchUpdate ≈ same ~550-800ms total.

---

## 13. Migration / back-compat

- **No back-compat concerns for colmena itself** — brand-new module.
- **No impact on existing subsystems.** `gsheets` / `crdt_documents` /
  `documents` continue unchanged.
- **Postgres migration is additive.** New table, no schema changes
  to existing tables.
- **Toolkit registry** — 22 new tools registered; toolkit alias `gdocs`
  added; nothing renamed or removed.
- **ADP impact** — zero breaking changes. ADP opts in by enabling
  `gdocs` and providing creds. The `mode: "suggest"` placeholder accepted
  in v1 → adding the actual behavior in v1.1 is non-breaking.
- **CLAUDE.md `agent_session_id` rule** — fully respected: revision
  store keys on `agent_session_id`.

---

## 14. Out-of-scope (v1.1 BACKLOG)

- `mode: "suggest"` behavior (currently accepted, no-op-validated).
- Surgical table-cell edits (`gdocs_set_table_cell`, `gdocs_insert_table_row`).
- Drive Comments API integration for human↔agent inline messaging.
- Drive Revisions restore (rollback to a prior revision).
- `acknowledge_human_changes` enriched mode (cheap-tier LLM summary).
- `gdocs_list_documents` (folder-scoped discovery).
- OAuth user-scoped auth.
- Image editing flows beyond simple insertion (resize, replace, caption).
- Real-time presence integration.
- Apps Script execution.

---

## 15. Implementation task preview

| ID | Task | Est. LOC |
|---|---|---:|
| G-T1 | `gdocs/domain` — types, errors, traits | 280 |
| G-T2 | `gdocs/infrastructure/auth.rs` (mirror of gsheets) | 90 |
| G-T3 | `gdocs/infrastructure/config.rs` | 70 |
| G-T4 | `gdocs/infrastructure/http_client.rs` — get / batch_update / list_revisions | 500 |
| G-T5 | `gdocs/infrastructure/http_client.rs` — create / create_from_markdown / create_from_docx / share / export / add_tab | 350 |
| G-T6 | `gdocs/infrastructure/revision_store.rs` + Postgres migration | 180 |
| G-T7 | `gdocs/infrastructure/outline_cache.rs` | 120 |
| G-T8 | `markdown_to_docs_ops.rs` (parser + emitter + 14 elements) | 650 |
| G-T9 | `gdocs/application/co_edit_guard.rs` | 250 |
| G-T10 | `gdocs/application/replace_text.rs` | 200 |
| G-T11 | `gdocs/application/insert.rs` (after / before / between) | 180 |
| G-T12 | `gdocs/application/replace_section.rs` + `append_markdown.rs` | 160 |
| G-T13 | `gdocs/application/style.rs` (style_text) | 180 |
| G-T14 | `gdocs/application/apply_edits.rs` (atomic compound) | 250 |
| G-T15 | `gdocs/application/named_range.rs` | 150 |
| G-T16 | 22 tool dispatchers + tool defs in `gdocs_tools.rs` | 900 |
| G-T17 | Toolkit-package alias `gdocs` + `gdocs_readonly` | 60 |
| G-T18 | Text registry: `text/tools/gdocs.yaml` — 22 description/summary entries | 400 |
| G-T19 | `MockDocsClient` + ~60 unit tests | 600 |
| G-T20 | Integration test `#[ignore]`-gated (~10 scenarios) | 350 |
| G-T21 | Smoke graph `tests/graphs/agents/gdocs_smoke.json` | 200 |
| G-T22 | Skill `gdocs-editing-best-practices.md` — content addressing, scopes, co-edit guard primer | docs |
| G-T23 | Docs: `developer_guide/44_gdocs.md`, `node_configurations.json`, BACKLOG, CHANGELOG | docs |
| G-T24 | Final sweep: `cargo test --verbose`, clippy, fmt, smoke E2E | — |

**Total estimated: ~5500 LOC + tests + docs. ~7-10 days subagent-driven.**

(Markedly larger than gsheets's ~2200 LOC because the markdown↔ops
parser + co-edit guard + multi-tab plumbing have no analogue in gsheets.)

---

## 16. Open questions

None at design time. Decisions explicit in §2 (scope), §3 (architecture),
§5 (auth), §6 (tools), §8 (co-edit guard), §10 (markdown conversion),
§14 (deferred to v1.1).

If a reviewer wants to bring forward a v1.1 item (e.g. suggesting mode,
table-cell edits, comments API), move the row from §14 to §15 and the
plan grows by ~200-500 LOC per item.

---

## 17. Live verification findings 2026-06-09

End-to-end verification ran against a real user-shared Google Doc
(`1QkeEG4PU0PFBwDs8dP6WaYUIEafVwjL3D27eA1w8f0k`) with SA
`colmena-sheets-tester@startti-dev.iam.gserviceaccount.com`. The full
co-edit flow ("agent builds plan in new tab → user manually edits doc
mid-session → agent's next replace_text gets `human_changes_pending` →
agent calls `acknowledge_human_changes` → retry succeeds") was verified
green at 04:25 UTC.

Nine post-spec bug fixes landed during verification:

| # | Commit | What was broken | Fix |
|---|---|---|---|
| 1 | `b82bd35` | `llm.rs` registry didn't expose the gdocs tools — LLM only saw `recall_history` | Added the 22 gdocs dispatchers to the synthetic-tool enumeration |
| 2 | `79eae72` | Default scope `drive.file` rejected user-shared docs with `appNotAuthorizedToFile` | Changed default to `drive` (full scope); operators can still downgrade |
| 3 | `8c9ea0e` | `gdocs_append_markdown` used placeholder `index: 0` → HTTP 400 from Google | Computed the real end-of-segment index from the snapshot |
| 4 | `940d508` | `gdocs_list_tabs` returned empty because the HTTP client called `documents.get` with `includeTabsContent=false` and Google omits the tabs field entirely in that mode | Switched to `includeTabsContent=true` |
| 5 | `baaf48d` | `markdown_to_docs_ops` didn't emit `location.tabId` AND `add_tab` didn't invalidate the outline cache → all content seeded after `add_tab` landed on tab 1 instead of the new tab | Emit `tabId` in every `location`/`range`; invalidate cache after `add_tab` |
| 6 | `8de4922` | Application use cases emitted `range` / `location` JSON without `tabId` for content-addressed edits → `replace_text` reported success but the batchUpdate landed in tab 1 because Google defaulted | Pass-through `tab_id` from the resolved scope into every emitted request |
| 7 | `081b0d2` | Dispatcher keyed `gdocs_session_state` on the CLI-ephemeral `session_id` (UUID per invocation), so co-edit cursors never survived a single run | Switched to `agent_session_id` (stable across runs — same rule as gsheets, llm_history, dag_runs) |
| 8 | `c868794` | **Design pivot.** Spec §8 assumed Drive Revisions API exposed per-edit revisions usable for paragraph-level diff. Empirically Drive only returns _named versions_ for native Google Docs (not per-edit log). | Pivoted v1 to **revisionId equality check only** — `human_changes_pending` signals "something changed" but does not include the diff. Paragraph-level diff moves to v1.1 (requires colmena-side snapshot caching, see BACKLOG) |
| 9 | `de05cbf` | Use cases stored `batch_update`'s `writeControl.requiredRevisionId` (~25 chars) as the cursor; the next guard chunked `documents.get`'s `revisionId` (~111 chars) and got a mismatch → false `human_changes_pending` on every turn | After every successful write, fetch a fresh `documents.get` snapshot and store its `revisionId` as the new cursor |

**Operational findings.** Documented in dev guide §"Limitaciones en v1":
(i) `create_*` fails on personal Gmail folders by
`storageQuotaExceeded` — realistic v1 pattern is "user-creates-first";
(ii) `dispatch_create_from_docx` returns `not_yet_wired` (attachment
plumbing v1.1); (iii) `dispatch_export` returns `byte_len` without
wrapping bytes as an attachment (v1.1); (iv) `add_tab` with `markdown`
arg responds `pending_markdown_seed: true` and does not seed content
(v1.1); (v) `add_tab` works on legacy single-tab docs — the previous
belief that it didn't was a masking effect from bug #4.

**Spec sections superseded by this appendix:**
- §8 "Co-edition safety pipeline" — the `diff_outlines` + per-paragraph
  attribution flow described there is **not v1**. v1 ships revisionId
  equality only. The full per-paragraph diff vision is preserved as a
  v1.1 target.
- §6 "Tool surface — agent-facing", `gdocs_create_from_docx` /
  `gdocs_export` / `gdocs_add_tab(markdown)` — dispatchers exist but
  return structured "deferred" markers.
