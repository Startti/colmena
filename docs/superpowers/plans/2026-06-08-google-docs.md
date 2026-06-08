# Google Docs Integration Implementation Plan (Subsystem G)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship ~22 synthetic LLM tools that let colmena agents read, write, create, and edit Google Docs via the Docs API v1 + Drive API, using a content-addressed surgical-edit model (the LLM never touches UTF-16 indices). Multi-tab, markdown-aware partial inserts, and a co-edit safety pipeline that diffs human changes via Drive Revisions and blocks overlapping writes.

**Architecture:** Hexagonal — `DocsClient` trait in `src/libs/colmena/src/gdocs/domain/`, `GoogleDocsHttpClient` REST adapter in `infrastructure/`, application-layer use cases for high-level operations (replace_text, insert, style, apply_edits, named_range, co_edit_guard), and one `gdocs_tools.rs` dispatcher file in `dag_engine/.../llm_synthetic_tools/`. Markdown→Doc ops conversion lives in a pure `markdown_to_docs_ops.rs` module. Postgres-backed revision tracking keyed on `agent_session_id`. Tests mock the trait; integration tests hit Google behind an `#[ignore]` gate.

**Tech Stack:** Rust 1.95, `reqwest` (existing), `yup-oauth2 = "11"` (existing), `serde_json` (existing), `pulldown-cmark` (existing — already used by `documents/`), `sqlx` (existing — Postgres), `regex` (existing), `chrono` (existing), `wiremock = "0.6"` (existing) for HTTP-mock tests. **One new dependency:** `similar = "2"` for outline diffing in the co-edit guard.

**Reference spec:** [`docs/superpowers/specs/2026-06-08-google-docs-design.md`](../specs/2026-06-08-google-docs-design.md)

---

## File Structure

### Create

| Path | Responsibility |
|---|---|
| `src/libs/colmena/src/gdocs/mod.rs` | Module root; re-exports. |
| `src/libs/colmena/src/gdocs/domain/mod.rs` | Domain index. |
| `src/libs/colmena/src/gdocs/domain/types.rs` | All value types (DocumentId, TabId, RevisionId, Scope, StylePatch, OutlineEntry, etc.). |
| `src/libs/colmena/src/gdocs/domain/errors.rs` | `DocsError` enum. |
| `src/libs/colmena/src/gdocs/domain/traits.rs` | `DocsClient` async trait. |
| `src/libs/colmena/src/gdocs/infrastructure/mod.rs` | Infra index. |
| `src/libs/colmena/src/gdocs/infrastructure/config.rs` | `GDocsConfig::from_env()`. |
| `src/libs/colmena/src/gdocs/infrastructure/auth.rs` | ADC + SA JSON token; 50-min cache. |
| `src/libs/colmena/src/gdocs/infrastructure/http_client.rs` | `GoogleDocsHttpClient` implementing `DocsClient`. |
| `src/libs/colmena/src/gdocs/infrastructure/revision_store.rs` | `PostgresRevisionStore` + `RevisionStore` trait. |
| `src/libs/colmena/src/gdocs/infrastructure/outline_cache.rs` | In-memory snapshot cache (5s TTL). |
| `src/libs/colmena/src/gdocs/application/mod.rs` | Application index. |
| `src/libs/colmena/src/gdocs/application/co_edit_guard.rs` | Revision check + human-diff pipeline. |
| `src/libs/colmena/src/gdocs/application/scope_resolver.rs` | Scope → paragraph range translator (shared by all edit use cases). |
| `src/libs/colmena/src/gdocs/application/replace_text.rs` | `replace_text` use case. |
| `src/libs/colmena/src/gdocs/application/insert.rs` | `insert_after_text` / `insert_before_text` / `insert_between`. |
| `src/libs/colmena/src/gdocs/application/delete_text.rs` | `delete_text` use case. |
| `src/libs/colmena/src/gdocs/application/replace_section.rs` | `replace_section` + `append_markdown`. |
| `src/libs/colmena/src/gdocs/application/style.rs` | `style_text` use case. |
| `src/libs/colmena/src/gdocs/application/apply_edits.rs` | Atomic compound edit. |
| `src/libs/colmena/src/gdocs/application/named_range.rs` | `create_named_range` + `replace_named_range`. |
| `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gdocs_tools.rs` | 22 tool dispatchers + tool definitions. |
| `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/markdown_to_docs_ops.rs` | Markdown → batchUpdate request emitter. |
| `src/libs/colmena/text/tools/gdocs.yaml` | LLM-facing tool descriptions/summaries (22 entries). |
| `src/libs/colmena/migrations/<next>_gdocs_session_state.sql` | Postgres migration for revision tracking. |
| `src/libs/colmena/tests/gdocs_integration_test.rs` | `#[ignore]`-gated end-to-end test. |
| `tests/gdocs_markdown_fixtures/*.md` + `tests/gdocs_markdown_fixtures/*.ops.json` | Golden fixtures for markdown→ops. |
| `tests/graphs/agents/gdocs_smoke.json` | DAG smoke graph. |
| `docs/developer_guide/44_gdocs.md` | Developer guide §44. |

### Modify

| Path | Change |
|---|---|
| `src/libs/colmena/Cargo.toml` | Add `similar = "2"` dependency. |
| `src/libs/colmena/src/lib.rs` | Add `pub mod gdocs;`. |
| `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs` | Add `pub mod gdocs_tools;` + `pub mod markdown_to_docs_ops;` + re-exports. |
| `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/toolkit_packages.rs` | Add `gdocs` + `gdocs_readonly` aliases. |
| `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs` | Add `gdocs_*` dispatch arms. |
| `docs/node_as_tools_reference.json` | Add 22 tool entries. |
| `docs/BACKLOG.md` | New "Subsystem G v1.1" section. |
| `docs/CHANGELOG_2026-06.md` | G entry. |
| `CLAUDE.md` | Add `gdocs` to "current status" + tool config standard if needed. |

---

## Task 1: Module scaffolding + `similar` dep

**Files:**
- Create: `src/libs/colmena/src/gdocs/mod.rs`
- Create: `src/libs/colmena/src/gdocs/domain/mod.rs`
- Create: `src/libs/colmena/src/gdocs/infrastructure/mod.rs`
- Create: `src/libs/colmena/src/gdocs/application/mod.rs`
- Modify: `src/libs/colmena/src/lib.rs` (add `pub mod gdocs;`)
- Modify: `src/libs/colmena/Cargo.toml` (add `similar = "2"`)

- [ ] **Step 1: Locate top-level module declarations**

Run: `grep -n "^pub mod " src/libs/colmena/src/lib.rs | head -25`
Expected: list of `pub mod foo;`. Identify where `gdocs` lands alphabetically (between `dag_engine` and `gsheets`).

- [ ] **Step 2: Add `similar` to Cargo.toml**

In `src/libs/colmena/Cargo.toml` under `[dependencies]`, add (keep alphabetical):

```toml
similar = "2"
```

Run: `cargo check -p colmena_dag_engine --no-default-features`
Expected: compiles (dep downloaded).

- [ ] **Step 3: Create `gdocs/mod.rs`**

Write `src/libs/colmena/src/gdocs/mod.rs`:

```rust
//! Google Docs integration (Subsystem G).
//!
//! Hexagonal layout:
//! - [`domain`] — `DocsClient` port, value types, errors.
//! - [`application`] — high-level use cases (replace_text, insert, style,
//!   apply_edits, co_edit_guard, …).
//! - [`infrastructure`] — REST adapter, auth, config, postgres revision
//!   store, in-memory outline cache.
//!
//! Tool dispatchers live in
//! `crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::gdocs_tools`.

pub mod application;
pub mod domain;
pub mod infrastructure;
```

- [ ] **Step 4: Create `gdocs/domain/mod.rs`**

```rust
//! Domain layer — port + value types + errors. No infra deps.

pub mod errors;
pub mod traits;
pub mod types;

pub use errors::DocsError;
pub use traits::DocsClient;
pub use types::*;
```

- [ ] **Step 5: Create `gdocs/infrastructure/mod.rs`**

```rust
//! Infrastructure layer — REST adapter, auth, config, persistence.

pub mod auth;
pub mod config;
pub mod http_client;
pub mod outline_cache;
pub mod revision_store;
```

- [ ] **Step 6: Create `gdocs/application/mod.rs`**

```rust
//! Application layer — high-level use cases shared across dispatchers.

pub mod apply_edits;
pub mod co_edit_guard;
pub mod delete_text;
pub mod insert;
pub mod named_range;
pub mod replace_section;
pub mod replace_text;
pub mod scope_resolver;
pub mod style;
```

- [ ] **Step 7: Add module to lib.rs**

In `src/libs/colmena/src/lib.rs`, add (alphabetically, between `dag_engine` and `gsheets`):

```rust
pub mod gdocs;
```

- [ ] **Step 8: Verify the skeleton compiles (empty modules expected to fail — that's OK; we'll fill them next)**

Run: `cargo check -p colmena_dag_engine --no-default-features 2>&1 | head -20`
Expected: errors about missing `errors.rs`, `traits.rs`, `types.rs`, etc. That's fine — we add them in Tasks 2-7.

- [ ] **Step 9: Commit**

```bash
git add src/libs/colmena/Cargo.toml \
        src/libs/colmena/src/lib.rs \
        src/libs/colmena/src/gdocs
git commit -m "feat(gdocs): scaffold module + add similar dep"
```

---

## Task 2: Domain types

**Files:**
- Create: `src/libs/colmena/src/gdocs/domain/types.rs`

- [ ] **Step 1: Write `types.rs` with full type set**

```rust
//! Value types for the Google Docs subsystem.
//!
//! All types implement `Serialize`/`Deserialize` so they can flow through
//! the dispatcher layer into JSON tool results without conversion.

use serde::{Deserialize, Serialize};

/// Stable Google identifier for a Doc (`/document/d/<this>`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DocumentId(pub String);

impl std::fmt::Display for DocumentId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Stable identifier of a tab within a multi-tab Doc.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TabId(pub String);

/// Opaque Google revision id. We pass the latest one back to Google as
/// `writeControl.requiredRevisionId` for optimistic concurrency.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RevisionId(pub String);

/// Where to look for the `find` text. `All` is doc-wide (default).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Scope {
    All,
    Tab { tab_id: TabId },
    Paragraph { n: u32 },
    UnderHeading { heading: String },
    BetweenHeadings { after: String, before: Option<String> },
}

impl Default for Scope {
    fn default() -> Self { Self::All }
}

/// 0..=1 normalised RGB.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RgbColor { pub r: f32, pub g: f32, pub b: f32 }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HeadingLevel { Normal, H1, H2, H3, H4, H5, H6, Title, Subtitle }

impl HeadingLevel {
    /// Wire-format string for `paragraphStyle.namedStyleType`.
    pub fn as_api_str(self) -> &'static str {
        match self {
            Self::Normal   => "NORMAL_TEXT",
            Self::H1       => "HEADING_1",
            Self::H2       => "HEADING_2",
            Self::H3       => "HEADING_3",
            Self::H4       => "HEADING_4",
            Self::H5       => "HEADING_5",
            Self::H6       => "HEADING_6",
            Self::Title    => "TITLE",
            Self::Subtitle => "SUBTITLE",
        }
    }
}

/// Style patch supplied by the agent. Only set fields are applied.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct StylePatch {
    pub bold:           Option<bool>,
    pub italic:         Option<bool>,
    pub underline:      Option<bool>,
    pub strikethrough:  Option<bool>,
    pub font_size_pt:   Option<f32>,
    pub foreground:     Option<RgbColor>,
    pub background:     Option<RgbColor>,
    pub link:           Option<String>,
    pub heading_level:  Option<HeadingLevel>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind { Replace, Insert, Delete, Style }

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChangeRecord {
    pub kind: ChangeKind,
    pub paragraph: u32,
    pub before: Option<String>,
    pub after: Option<String>,
    pub tab_id: Option<TabId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParagraphKind {
    Heading1, Heading2, Heading3, Heading4, Heading5, Heading6,
    Title, Subtitle, Paragraph, ListItem, TableRow,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OutlineEntry {
    pub paragraph: u32,
    pub tab_id: Option<TabId>,
    pub kind: ParagraphKind,
    pub text_preview: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HumanChangeKind { Insert, Modify, Delete }

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HumanChange {
    pub kind: HumanChangeKind,
    pub paragraph: u32,
    pub preview: String,
    pub modified_time: chrono::DateTime<chrono::Utc>,
    pub modifying_user: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LossyConversion {
    pub element_type: String,
    pub original_markdown: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MatchPreview {
    pub n: u32,                 // 1-based ordinal
    pub paragraph: u32,
    pub preview: String,        // ~80 chars surrounding the match
}

/// What every successful edit returns to the dispatcher.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EditResult {
    pub changes: Vec<ChangeRecord>,
    pub revision_id_after: RevisionId,
    pub outline_snapshot: Vec<OutlineEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lossy_conversions: Vec<LossyConversion>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending_human_changes_outside_scope: Vec<HumanChange>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocumentMeta {
    pub doc_id: DocumentId,
    pub url: String,
    pub title: String,
    pub revision_id: RevisionId,
    pub tabs: Vec<TabMeta>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TabMeta {
    pub tab_id: TabId,
    pub title: String,
    pub index: u32,
    pub parent_tab_id: Option<TabId>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NamedRangeMeta {
    pub named_range_id: String,
    pub name: String,
    pub paragraph_start: u32,
    pub paragraph_end: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RevisionMeta {
    pub revision_id: RevisionId,
    pub modified_time: chrono::DateTime<chrono::Utc>,
    pub modifying_user_email: Option<String>,   // None when redacted
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateFromMarkdownResult {
    pub meta: DocumentMeta,
    pub outline_snapshot: Vec<OutlineEntry>,
    pub lossy_conversions: Vec<LossyConversion>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShareRole { Reader, Commenter, Writer }

impl ShareRole {
    pub fn as_api_str(self) -> &'static str {
        match self {
            Self::Reader => "reader",
            Self::Commenter => "commenter",
            Self::Writer => "writer",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExportFormat { Docx, Pdf, Markdown, Txt, Rtf, Epub, Odt, Html }

impl ExportFormat {
    pub fn mime(self) -> &'static str {
        match self {
            Self::Docx     => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            Self::Pdf      => "application/pdf",
            Self::Markdown => "text/markdown",
            Self::Txt      => "text/plain",
            Self::Rtf      => "application/rtf",
            Self::Epub     => "application/epub+zip",
            Self::Odt      => "application/vnd.oasis.opendocument.text",
            Self::Html     => "application/zip",  // Google zips HTML
        }
    }
}

/// A doc snapshot the application layer holds in memory. Sufficient for
/// scope resolution, outline emission, and paragraph diffing. The infra
/// layer constructs these from `documents.get` responses.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocumentSnapshot {
    pub doc_id: DocumentId,
    pub revision_id: RevisionId,
    pub title: String,
    pub tabs: Vec<TabSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TabSnapshot {
    pub tab_id: Option<TabId>,           // None for single-tab legacy docs
    pub paragraphs: Vec<ParagraphSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParagraphSnapshot {
    pub n: u32,                          // 1-based, document-global
    pub kind: ParagraphKind,
    pub text: String,                    // full plain text of this paragraph
    pub start_index: u32,                // segment-relative UTF-16
    pub end_index: u32,                  // exclusive
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BatchUpdateResult {
    pub revision_id_after: RevisionId,
    pub replies: Vec<serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn scope_roundtrips() {
        let cases = [
            (json!({"kind": "all"}),                       Scope::All),
            (json!({"kind": "tab", "tab_id": "t1"}),       Scope::Tab { tab_id: TabId("t1".into()) }),
            (json!({"kind": "paragraph", "n": 5}),         Scope::Paragraph { n: 5 }),
            (json!({"kind": "under_heading", "heading": "## A"}),
                Scope::UnderHeading { heading: "## A".into() }),
        ];
        for (j, expected) in cases {
            let s: Scope = serde_json::from_value(j.clone()).unwrap();
            assert_eq!(s, expected);
            assert_eq!(serde_json::to_value(&s).unwrap(), j);
        }
    }

    #[test]
    fn heading_level_api_strings() {
        assert_eq!(HeadingLevel::H1.as_api_str(), "HEADING_1");
        assert_eq!(HeadingLevel::Normal.as_api_str(), "NORMAL_TEXT");
        assert_eq!(HeadingLevel::Title.as_api_str(), "TITLE");
    }

    #[test]
    fn export_format_mime_types() {
        assert_eq!(ExportFormat::Docx.mime(),
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document");
        assert_eq!(ExportFormat::Markdown.mime(), "text/markdown");
    }

    #[test]
    fn edit_result_omits_empty_optional_fields() {
        let er = EditResult {
            changes: vec![],
            revision_id_after: RevisionId("rev_2".into()),
            outline_snapshot: vec![],
            lossy_conversions: vec![],
            pending_human_changes_outside_scope: vec![],
        };
        let j = serde_json::to_value(&er).unwrap();
        assert!(j.get("lossy_conversions").is_none());
        assert!(j.get("pending_human_changes_outside_scope").is_none());
    }
}
```

- [ ] **Step 2: Compile + run the unit tests**

Run: `cargo test -p colmena_dag_engine --lib gdocs::domain::types`
Expected: 4 tests pass.

- [ ] **Step 3: Commit**

```bash
git add src/libs/colmena/src/gdocs/domain/types.rs
git commit -m "feat(gdocs): add domain value types"
```

---

## Task 3: Domain errors

**Files:**
- Create: `src/libs/colmena/src/gdocs/domain/errors.rs`

- [ ] **Step 1: Write `errors.rs`**

```rust
//! Domain errors for the Google Docs subsystem.
//!
//! Every variant maps to a stable JSON shape in the dispatcher layer
//! (see `gdocs_tools::error_to_json`). The content-addressed edit
//! errors (`AmbiguousMatch`, `TextNotFound`, `ConfirmManyMatches`,
//! `HumanChangesPending`) carry rich payloads the LLM uses to recover.

use crate::gdocs::domain::types::{HumanChange, MatchPreview};
use thiserror::Error;

#[derive(Debug, Clone, Error)]
pub enum DocsError {
    #[error("gdocs_not_configured: {0}")]
    NotConfigured(String),

    #[error("auth_failed: {0}")]
    AuthFailed(String),

    #[error("document_not_found: {0}")]
    DocumentNotFound(String),

    #[error("tab_not_found: {0}")]
    TabNotFound(String),

    #[error("permission_denied: share with {0}")]
    PermissionDenied(String),

    #[error("no_parent_folder_configured")]
    NoParentFolder,

    #[error("ambiguous_match: '{find}' has {} candidates", matches.len())]
    AmbiguousMatch { find: String, matches: Vec<MatchPreview> },

    #[error("text_not_found: '{find}'")]
    TextNotFound { find: String, fuzzy_suggestions: Vec<String> },

    #[error("confirm_many_matches: {count} matches in scope")]
    ConfirmManyMatches { find: String, count: u32, preview: Vec<MatchPreview> },

    #[error("human_changes_pending: {} overlapping, {} outside",
        changes_overlapping_scope.len(), changes_outside_scope.len())]
    HumanChangesPending {
        since: chrono::DateTime<chrono::Utc>,
        changes_overlapping_scope: Vec<HumanChange>,
        changes_outside_scope: Vec<HumanChange>,
    },

    #[error("scope_crosses_boundary: {0}")]
    ScopeCrossesBoundary(String),

    #[error("tab_exists: {0}")]
    TabExists(String),

    #[error("rate_limit: retry after {0}s")]
    RateLimit(u32),

    #[error("conflict: doc changed during write")]
    Conflict,

    #[error("http_error: {0}")]
    Http(String),

    #[error("invalid_args: {0}")]
    InvalidArgs(String),

    #[error("internal: {0}")]
    Internal(String),
}
```

- [ ] **Step 2: Compile**

Run: `cargo check -p colmena_dag_engine --no-default-features`
Expected: still errors about `traits.rs` missing — domain types + errors compile clean now.

- [ ] **Step 3: Commit**

```bash
git add src/libs/colmena/src/gdocs/domain/errors.rs
git commit -m "feat(gdocs): add domain error enum"
```

---

## Task 4: `DocsClient` trait

**Files:**
- Create: `src/libs/colmena/src/gdocs/domain/traits.rs`

- [ ] **Step 1: Write `traits.rs`**

```rust
//! Port for any backend that can read/write Google Docs.
//! `infrastructure::GoogleDocsHttpClient` is the production impl;
//! tests use `MockDocsClient` (declared inside `tests` modules).

use crate::gdocs::domain::{
    DocsError, DocumentId, DocumentMeta, DocumentSnapshot, CreateFromMarkdownResult,
    BatchUpdateResult, ExportFormat, NamedRangeMeta, OutlineEntry, RevisionId,
    RevisionMeta, Scope, ShareRole, TabId, TabMeta,
};
use async_trait::async_trait;

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

    /// Fetch a historical snapshot as plain text. Used by the co-edit
    /// guard to compute paragraph-level diffs against the current state.
    async fn get_at_revision(&self, id: &DocumentId, revision: &RevisionId)
        -> Result<String, DocsError>;

    // ── batchUpdate ──────────────────────────────────────────────
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

- [ ] **Step 2: Compile**

Run: `cargo check -p colmena_dag_engine --no-default-features`
Expected: domain layer compiles clean; infra modules still missing.

- [ ] **Step 3: Commit**

```bash
git add src/libs/colmena/src/gdocs/domain/traits.rs
git commit -m "feat(gdocs): add DocsClient trait"
```

---

## Task 5: Config

**Files:**
- Create: `src/libs/colmena/src/gdocs/infrastructure/config.rs`

- [ ] **Step 1: Write `config.rs`**

```rust
//! Operator-facing config. Constructed from env vars; can be overridden
//! at runtime by callers (per-tool fixed_config).

use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct GDocsConfig {
    pub credentials_path:      Option<PathBuf>,
    pub scopes:                Vec<String>,
    pub default_parent_folder: Option<String>,
    pub request_timeout:       Duration,
    pub max_retries:           u32,
    pub revision_cache_ttl:    Duration,
}

impl GDocsConfig {
    pub fn from_env() -> Self {
        let credentials_path = std::env::var("GOOGLE_APPLICATION_CREDENTIALS")
            .ok()
            .map(PathBuf::from);
        let scopes = std::env::var("COLMENA_GDOCS_SCOPES")
            .ok()
            .map(|s| s.split(',').map(|x| x.trim().to_string()).collect())
            .unwrap_or_else(|| {
                vec![
                    "https://www.googleapis.com/auth/documents".to_string(),
                    "https://www.googleapis.com/auth/drive.file".to_string(),
                ]
            });
        let default_parent_folder = std::env::var("COLMENA_GDOCS_DEFAULT_PARENT_FOLDER_ID").ok();
        let cache_secs = std::env::var("COLMENA_GDOCS_REVISION_CACHE_SECS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(5);
        Self {
            credentials_path,
            scopes,
            default_parent_folder,
            request_timeout:    Duration::from_secs(30),
            max_retries:        3,
            revision_cache_ttl: Duration::from_secs(cache_secs),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_when_env_empty() {
        // We intentionally do NOT manipulate env vars here — the test
        // only documents the default values. A separate env-isolated
        // test (kept as `#[ignore]`) would exercise overrides.
        let cfg = GDocsConfig {
            credentials_path: None,
            scopes: vec![
                "https://www.googleapis.com/auth/documents".to_string(),
                "https://www.googleapis.com/auth/drive.file".to_string(),
            ],
            default_parent_folder: None,
            request_timeout: Duration::from_secs(30),
            max_retries: 3,
            revision_cache_ttl: Duration::from_secs(5),
        };
        assert_eq!(cfg.max_retries, 3);
        assert_eq!(cfg.revision_cache_ttl, Duration::from_secs(5));
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p colmena_dag_engine --lib gdocs::infrastructure::config`
Expected: 1 test passes.

- [ ] **Step 3: Commit**

```bash
git add src/libs/colmena/src/gdocs/infrastructure/config.rs
git commit -m "feat(gdocs): add GDocsConfig with env-var resolution"
```

---

## Task 6: Auth (ADC + Service Account JSON) with token cache

**Files:**
- Create: `src/libs/colmena/src/gdocs/infrastructure/auth.rs`

> **Pattern reference:** Mirror `src/libs/colmena/src/gsheets/infrastructure/auth.rs`. Read it first — the structure is identical, only scopes and error variant names differ.

- [ ] **Step 1: Read the gsheets auth module to internalise the pattern**

Run: `cat src/libs/colmena/src/gsheets/infrastructure/auth.rs`
Expected: 148-line file. Note: `ApplicationDefaultCredentialsAuthenticator`, `ApplicationDefaultCredentialsTypes`, `get_token(scopes)`, in-memory token cache with `Mutex<Option<CachedToken>>`, 50-min TTL.

- [ ] **Step 2: Write `auth.rs`**

```rust
//! ADC + SA JSON token acquisition via `yup-oauth2`.
//! Token cached in-memory with 50-min TTL (Google tokens last 60 min).
//!
//! Mirrors `crate::gsheets::infrastructure::auth` — same `yup-oauth2`
//! pattern as `image_generation.rs:572`.

use crate::gdocs::domain::DocsError;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const TOKEN_TTL: Duration = Duration::from_secs(50 * 60);

#[derive(Debug, Clone)]
struct CachedToken {
    value: String,
    fetched_at: Instant,
}

#[derive(Default)]
pub struct TokenCache {
    inner: Mutex<Option<CachedToken>>,
}

impl TokenCache {
    pub fn new() -> Self { Self::default() }

    pub async fn get(&self, scopes: &[String]) -> Result<String, DocsError> {
        // Fast path — check the cache.
        if let Some(t) = self.inner.lock().expect("token mutex poisoned").as_ref() {
            if t.fetched_at.elapsed() < TOKEN_TTL {
                return Ok(t.value.clone());
            }
        }
        // Slow path — fetch fresh.
        let scope_refs: Vec<&str> = scopes.iter().map(|s| s.as_str()).collect();
        let value = acquire_token(&scope_refs).await?;
        *self.inner.lock().expect("token mutex poisoned") = Some(CachedToken {
            value: value.clone(),
            fetched_at: Instant::now(),
        });
        Ok(value)
    }
}

async fn acquire_token(scopes: &[&str]) -> Result<String, DocsError> {
    use yup_oauth2::authenticator::ApplicationDefaultCredentialsTypes;
    use yup_oauth2::{
        ApplicationDefaultCredentialsAuthenticator,
        ApplicationDefaultCredentialsFlowOpts,
    };

    let opts = ApplicationDefaultCredentialsFlowOpts::default();
    let auth = match ApplicationDefaultCredentialsAuthenticator::builder(opts).await {
        ApplicationDefaultCredentialsTypes::InstanceMetadata(builder) =>
            builder.build().await
                .map_err(|e| DocsError::AuthFailed(e.to_string()))?,
        ApplicationDefaultCredentialsTypes::ServiceAccount(builder) =>
            builder.build().await
                .map_err(|e| DocsError::AuthFailed(e.to_string()))?,
    };
    let token = auth.token(scopes).await.map_err(|e| {
        let msg = e.to_string();
        if msg.contains("no credentials") || msg.contains("GOOGLE_APPLICATION_CREDENTIALS") {
            DocsError::NotConfigured(
                "set GOOGLE_APPLICATION_CREDENTIALS or run `gcloud auth application-default login`"
                    .to_string(),
            )
        } else {
            DocsError::AuthFailed(msg)
        }
    })?;
    Ok(token
        .token()
        .ok_or_else(|| DocsError::AuthFailed("empty token".into()))?
        .to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn token_cache_returns_cached_value_within_ttl() {
        let cache = TokenCache::new();
        // Seed the cache directly (without hitting Google).
        {
            let mut guard = cache.inner.lock().unwrap();
            *guard = Some(CachedToken {
                value: "test_token".into(),
                fetched_at: Instant::now(),
            });
        }
        let scopes = vec!["https://www.googleapis.com/auth/documents".to_string()];
        let v = cache.get(&scopes).await.unwrap();
        assert_eq!(v, "test_token");
    }
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p colmena_dag_engine --lib gdocs::infrastructure::auth`
Expected: 1 test passes (the one that seeds the cache directly — does not hit Google).

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/gdocs/infrastructure/auth.rs
git commit -m "feat(gdocs): add ADC/SA-JSON auth with token cache"
```

---

*Plan continues — see Tasks 7-24 below. Cada task sigue el mismo patrón TDD: write test → run (fail) → implement → run (pass) → commit.*

---

## Task 7: Postgres `revision_store` + migration

**Files:**
- Create: `src/libs/colmena/migrations/<next_seq>_gdocs_session_state.sql`
- Create: `src/libs/colmena/src/gdocs/infrastructure/revision_store.rs`

- [ ] **Step 1: Find next migration sequence number**

Run: `ls src/libs/colmena/migrations/ | sort -V | tail -3`
Expected: list ending with the highest existing migration. Use `<that + 1>` for the new file (e.g. if last is `0042_xxx.sql`, new is `0043_gdocs_session_state.sql`).

- [ ] **Step 2: Write migration**

Create the new migration file with:

```sql
-- gdocs_session_state — tracks last-known revision per (agent_session, doc).
-- Used by the co-edit guard to detect human edits between agent writes.

CREATE TABLE IF NOT EXISTS gdocs_session_state (
    agent_session_id TEXT        NOT NULL,
    document_id      TEXT        NOT NULL,
    last_revision_id TEXT        NOT NULL,
    last_edit_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (agent_session_id, document_id)
);

CREATE INDEX IF NOT EXISTS gdocs_session_state_last_edit_at_idx
    ON gdocs_session_state (last_edit_at);
```

- [ ] **Step 3: Write `revision_store.rs` — trait + postgres impl**

```rust
//! Persistent storage of the last revision the agent saw per
//! (agent_session_id, document_id). Postgres-backed; mockable.

use crate::gdocs::domain::{DocsError, DocumentId, RevisionId};
use async_trait::async_trait;
use sqlx::PgPool;

#[async_trait]
pub trait RevisionStore: Send + Sync {
    async fn get(&self, session_id: &str, doc_id: &DocumentId)
        -> Result<Option<RevisionId>, DocsError>;
    async fn put(&self, session_id: &str, doc_id: &DocumentId, rev: &RevisionId)
        -> Result<(), DocsError>;
}

pub struct PostgresRevisionStore { pool: PgPool }

impl PostgresRevisionStore {
    pub fn new(pool: PgPool) -> Self { Self { pool } }
}

#[async_trait]
impl RevisionStore for PostgresRevisionStore {
    async fn get(&self, session_id: &str, doc_id: &DocumentId)
        -> Result<Option<RevisionId>, DocsError>
    {
        let row: Option<(String,)> = sqlx::query_as(
            "SELECT last_revision_id FROM gdocs_session_state \
             WHERE agent_session_id = $1 AND document_id = $2"
        )
        .bind(session_id)
        .bind(&doc_id.0)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| DocsError::Internal(format!("revision_store.get: {e}")))?;
        Ok(row.map(|(s,)| RevisionId(s)))
    }

    async fn put(&self, session_id: &str, doc_id: &DocumentId, rev: &RevisionId)
        -> Result<(), DocsError>
    {
        sqlx::query(
            "INSERT INTO gdocs_session_state \
                (agent_session_id, document_id, last_revision_id, last_edit_at) \
             VALUES ($1, $2, $3, now()) \
             ON CONFLICT (agent_session_id, document_id) \
             DO UPDATE SET last_revision_id = EXCLUDED.last_revision_id, \
                           last_edit_at     = now()"
        )
        .bind(session_id)
        .bind(&doc_id.0)
        .bind(&rev.0)
        .execute(&self.pool)
        .await
        .map_err(|e| DocsError::Internal(format!("revision_store.put: {e}")))?;
        Ok(())
    }
}

/// In-memory `RevisionStore` for unit tests.
#[cfg(any(test, feature = "test-utils"))]
pub struct InMemoryRevisionStore {
    map: tokio::sync::RwLock<std::collections::HashMap<(String, String), RevisionId>>,
}

#[cfg(any(test, feature = "test-utils"))]
impl InMemoryRevisionStore {
    pub fn new() -> Self {
        Self { map: tokio::sync::RwLock::new(Default::default()) }
    }
}

#[cfg(any(test, feature = "test-utils"))]
#[async_trait]
impl RevisionStore for InMemoryRevisionStore {
    async fn get(&self, sid: &str, doc: &DocumentId) -> Result<Option<RevisionId>, DocsError> {
        Ok(self.map.read().await.get(&(sid.into(), doc.0.clone())).cloned())
    }
    async fn put(&self, sid: &str, doc: &DocumentId, rev: &RevisionId) -> Result<(), DocsError> {
        self.map.write().await.insert((sid.into(), doc.0.clone()), rev.clone());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn in_memory_round_trip() {
        let store = InMemoryRevisionStore::new();
        let doc = DocumentId("doc1".into());
        let rev = RevisionId("rev_5".into());
        assert!(store.get("s1", &doc).await.unwrap().is_none());
        store.put("s1", &doc, &rev).await.unwrap();
        assert_eq!(store.get("s1", &doc).await.unwrap(), Some(rev));
    }

    #[tokio::test]
    async fn in_memory_scoped_by_session() {
        let store = InMemoryRevisionStore::new();
        let doc = DocumentId("doc1".into());
        store.put("s1", &doc, &RevisionId("ra".into())).await.unwrap();
        store.put("s2", &doc, &RevisionId("rb".into())).await.unwrap();
        assert_eq!(store.get("s1", &doc).await.unwrap().unwrap().0, "ra");
        assert_eq!(store.get("s2", &doc).await.unwrap().unwrap().0, "rb");
    }
}
```

- [ ] **Step 4: Run unit tests (in-memory only — Postgres path is integration tested)**

Run: `cargo test -p colmena_dag_engine --lib gdocs::infrastructure::revision_store`
Expected: 2 tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/migrations/<seq>_gdocs_session_state.sql \
        src/libs/colmena/src/gdocs/infrastructure/revision_store.rs
git commit -m "feat(gdocs): postgres revision store + migration"
```

---

## Task 8: In-memory outline cache (5-second TTL)

**Files:**
- Create: `src/libs/colmena/src/gdocs/infrastructure/outline_cache.rs`

- [ ] **Step 1: Write `outline_cache.rs`**

```rust
//! Per-(session, doc) in-memory cache of `DocumentSnapshot`. 5-second
//! default TTL — short-lived but long enough to absorb tool-call bursts.
//! Not persistent across process restarts (cold cache → 1 extra GET).

use crate::gdocs::domain::{DocumentId, DocumentSnapshot};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
struct Entry {
    snapshot: DocumentSnapshot,
    fetched_at: Instant,
}

pub struct OutlineCache {
    ttl: Duration,
    map: Mutex<HashMap<(String, DocumentId), Entry>>,
}

impl OutlineCache {
    pub fn new(ttl: Duration) -> Self {
        Self { ttl, map: Mutex::new(HashMap::new()) }
    }

    pub fn get_fresh(&self, session_id: &str, doc_id: &DocumentId) -> Option<DocumentSnapshot> {
        let guard = self.map.lock().expect("outline cache mutex poisoned");
        guard.get(&(session_id.to_string(), doc_id.clone())).and_then(|e| {
            if e.fetched_at.elapsed() < self.ttl {
                Some(e.snapshot.clone())
            } else {
                None
            }
        })
    }

    pub fn put(&self, session_id: &str, doc_id: &DocumentId, snapshot: DocumentSnapshot) {
        let mut guard = self.map.lock().expect("outline cache mutex poisoned");
        guard.insert(
            (session_id.to_string(), doc_id.clone()),
            Entry { snapshot, fetched_at: Instant::now() },
        );
    }

    pub fn invalidate(&self, session_id: &str, doc_id: &DocumentId) {
        let mut guard = self.map.lock().expect("outline cache mutex poisoned");
        guard.remove(&(session_id.to_string(), doc_id.clone()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gdocs::domain::{RevisionId, TabSnapshot};

    fn fake_snapshot() -> DocumentSnapshot {
        DocumentSnapshot {
            doc_id: DocumentId("d1".into()),
            revision_id: RevisionId("r1".into()),
            title: "Test".into(),
            tabs: vec![TabSnapshot { tab_id: None, paragraphs: vec![] }],
        }
    }

    #[test]
    fn put_and_get_within_ttl() {
        let c = OutlineCache::new(Duration::from_secs(5));
        c.put("s1", &DocumentId("d1".into()), fake_snapshot());
        let got = c.get_fresh("s1", &DocumentId("d1".into()));
        assert!(got.is_some());
        assert_eq!(got.unwrap().title, "Test");
    }

    #[test]
    fn miss_when_session_or_doc_differs() {
        let c = OutlineCache::new(Duration::from_secs(5));
        c.put("s1", &DocumentId("d1".into()), fake_snapshot());
        assert!(c.get_fresh("s2", &DocumentId("d1".into())).is_none());
        assert!(c.get_fresh("s1", &DocumentId("d2".into())).is_none());
    }

    #[test]
    fn expires_after_ttl() {
        let c = OutlineCache::new(Duration::from_millis(10));
        c.put("s1", &DocumentId("d1".into()), fake_snapshot());
        std::thread::sleep(Duration::from_millis(30));
        assert!(c.get_fresh("s1", &DocumentId("d1".into())).is_none());
    }

    #[test]
    fn invalidate_removes_entry() {
        let c = OutlineCache::new(Duration::from_secs(5));
        c.put("s1", &DocumentId("d1".into()), fake_snapshot());
        c.invalidate("s1", &DocumentId("d1".into()));
        assert!(c.get_fresh("s1", &DocumentId("d1".into())).is_none());
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p colmena_dag_engine --lib gdocs::infrastructure::outline_cache`
Expected: 4 tests pass.

- [ ] **Step 3: Commit**

```bash
git add src/libs/colmena/src/gdocs/infrastructure/outline_cache.rs
git commit -m "feat(gdocs): in-memory outline cache with TTL"
```

---

## Task 9: HTTP client — skeleton + `get` + snapshot parsing

**Files:**
- Create: `src/libs/colmena/src/gdocs/infrastructure/http_client.rs`

> This task implements **only** the foundation (struct, retry helper, `get` endpoint, snapshot parsing). Subsequent tasks (10, 11, 12) layer `batch_update`, `list_revisions_since`, `get_at_revision`, then `create*`/`export`/`share`/`add_tab`.

- [ ] **Step 1: Read the gsheets HTTP client to internalise the retry/error-mapping pattern**

Run: `grep -nE "send_with_retry|map_status|reqwest" src/libs/colmena/src/gsheets/infrastructure/http_client.rs | head -30`
Expected: lines showing `send_with_retry`, 429/5xx backoff, status→error mapping. Adopt the same shape.

- [ ] **Step 2: Write `http_client.rs` skeleton + `get` + snapshot parser**

```rust
//! `GoogleDocsHttpClient` — REST adapter implementing `DocsClient`.
//!
//! Endpoint base URLs:
//!   - `https://docs.googleapis.com/v1/documents/...`
//!   - `https://www.googleapis.com/drive/v3/files/...`
//!   - `https://www.googleapis.com/upload/drive/v3/files`
//!
//! Retry policy: 3 retries (1s/2s/4s backoff) on 429 + 5xx. 401 refresh
//! once and retry. 4xx → typed `DocsError` variant immediately.

use crate::gdocs::domain::{
    DocsClient, DocsError, DocumentId, DocumentSnapshot, ParagraphKind, ParagraphSnapshot,
    RevisionId, TabId, TabSnapshot,
};
use crate::gdocs::infrastructure::auth::TokenCache;
use crate::gdocs::infrastructure::config::GDocsConfig;
use async_trait::async_trait;
use reqwest::{Client, Method, Response, StatusCode};
use std::sync::Arc;
use std::time::Duration;

const BASE_DOCS:        &str = "https://docs.googleapis.com/v1";
const BASE_DRIVE:       &str = "https://www.googleapis.com/drive/v3";
const BASE_DRIVE_UPLD:  &str = "https://www.googleapis.com/upload/drive/v3";

pub struct GoogleDocsHttpClient {
    cfg:    GDocsConfig,
    http:   Client,
    tokens: Arc<TokenCache>,
}

impl GoogleDocsHttpClient {
    pub fn from_config(cfg: &GDocsConfig) -> Result<Self, DocsError> {
        let http = Client::builder()
            .timeout(cfg.request_timeout)
            .build()
            .map_err(|e| DocsError::Http(e.to_string()))?;
        Ok(Self {
            cfg:    cfg.clone(),
            http,
            tokens: Arc::new(TokenCache::new()),
        })
    }

    async fn bearer(&self) -> Result<String, DocsError> {
        self.tokens.get(&self.cfg.scopes).await
    }

    /// Send with retry on 429/5xx. Returns the response on success or
    /// the final transport-level error.
    async fn send_with_retry(
        &self,
        build_req: impl Fn(&Client, &str) -> reqwest::RequestBuilder,
    ) -> Result<Response, DocsError> {
        let mut attempt = 0u32;
        loop {
            let token = self.bearer().await?;
            let resp = build_req(&self.http, &token)
                .send()
                .await
                .map_err(|e| DocsError::Http(e.to_string()))?;
            let status = resp.status();
            if status.is_success() || !is_retryable(status) {
                return Ok(resp);
            }
            attempt += 1;
            if attempt > self.cfg.max_retries {
                return Ok(resp);  // caller maps to DocsError
            }
            let backoff = 1u64 << (attempt - 1);  // 1s, 2s, 4s
            tokio::time::sleep(Duration::from_secs(backoff)).await;
        }
    }

    pub(crate) async fn map_status(&self, r: Response, ctx: &str) -> Result<Response, DocsError> {
        if r.status().is_success() { return Ok(r); }
        let s = r.status();
        let body = r.text().await.unwrap_or_default();
        Err(match s {
            StatusCode::UNAUTHORIZED => DocsError::AuthFailed(format!("{ctx}: 401 {body}")),
            StatusCode::FORBIDDEN    => DocsError::PermissionDenied(format!("{ctx}: {body}")),
            StatusCode::NOT_FOUND    => DocsError::DocumentNotFound(ctx.into()),
            StatusCode::TOO_MANY_REQUESTS => DocsError::RateLimit(60),
            StatusCode::BAD_REQUEST if body.contains("requiredRevisionId")
                => DocsError::Conflict,
            _ => DocsError::Http(format!("{ctx}: {s} {body}")),
        })
    }
}

fn is_retryable(s: StatusCode) -> bool {
    s == StatusCode::TOO_MANY_REQUESTS || s.is_server_error()
}

// ── Snapshot parser ─────────────────────────────────────────────────
// Walks the JSON tree from `documents.get` and emits a flat sequence
// of `ParagraphSnapshot`s, tracking 1-based paragraph numbering across
// all tabs (single-tab docs use a single TabSnapshot with tab_id=None).

pub(crate) fn parse_snapshot(j: &serde_json::Value, id: &DocumentId)
    -> Result<DocumentSnapshot, DocsError>
{
    let title = j.get("title").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let revision_id = RevisionId(
        j.get("revisionId").and_then(|v| v.as_str()).unwrap_or("").to_string()
    );

    let mut tabs = Vec::new();
    let mut para_counter: u32 = 0;

    if let Some(tabs_json) = j.get("tabs").and_then(|v| v.as_array()) {
        for tab in tabs_json {
            let tab_id = tab.pointer("/tabProperties/tabId")
                .and_then(|v| v.as_str()).map(|s| TabId(s.into()));
            let body_arr = tab.pointer("/documentTab/body/content")
                .and_then(|v| v.as_array()).cloned().unwrap_or_default();
            let paragraphs = parse_paragraphs(&body_arr, &mut para_counter);
            tabs.push(TabSnapshot { tab_id, paragraphs });
        }
    } else if let Some(body) = j.pointer("/body/content").and_then(|v| v.as_array()) {
        // Legacy single-tab doc.
        let paragraphs = parse_paragraphs(body, &mut para_counter);
        tabs.push(TabSnapshot { tab_id: None, paragraphs });
    }

    Ok(DocumentSnapshot { doc_id: id.clone(), revision_id, title, tabs })
}

fn parse_paragraphs(content: &[serde_json::Value], counter: &mut u32) -> Vec<ParagraphSnapshot> {
    let mut out = Vec::new();
    for elem in content {
        if let Some(p) = elem.get("paragraph") {
            *counter += 1;
            let kind = paragraph_kind(p);
            let (text, start, end) = paragraph_text_and_range(elem, p);
            out.push(ParagraphSnapshot { n: *counter, kind, text, start_index: start, end_index: end });
        }
        // Tables/section breaks/TOC are NOT counted as paragraphs in
        // v1's numbering — agent uses `read_outline` paragraph numbers
        // and they're consistent with what they'll see.
    }
    out
}

fn paragraph_kind(p: &serde_json::Value) -> ParagraphKind {
    let style = p.pointer("/paragraphStyle/namedStyleType")
        .and_then(|v| v.as_str()).unwrap_or("NORMAL_TEXT");
    if p.get("bullet").is_some() { return ParagraphKind::ListItem; }
    match style {
        "HEADING_1" => ParagraphKind::Heading1,
        "HEADING_2" => ParagraphKind::Heading2,
        "HEADING_3" => ParagraphKind::Heading3,
        "HEADING_4" => ParagraphKind::Heading4,
        "HEADING_5" => ParagraphKind::Heading5,
        "HEADING_6" => ParagraphKind::Heading6,
        "TITLE"     => ParagraphKind::Title,
        "SUBTITLE"  => ParagraphKind::Subtitle,
        _           => ParagraphKind::Paragraph,
    }
}

fn paragraph_text_and_range(elem: &serde_json::Value, p: &serde_json::Value) -> (String, u32, u32) {
    let start = elem.get("startIndex").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    let end   = elem.get("endIndex"  ).and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    let mut text = String::new();
    if let Some(elems) = p.get("elements").and_then(|v| v.as_array()) {
        for e in elems {
            if let Some(t) = e.pointer("/textRun/content").and_then(|v| v.as_str()) {
                text.push_str(t);
            }
        }
    }
    // Paragraphs end with a `\n` we want to strip for previews.
    let trimmed = text.trim_end_matches('\n').to_string();
    (trimmed, start, end)
}

// ── DocsClient impl (foundation methods only — others added in later tasks) ─

#[async_trait]
impl DocsClient for GoogleDocsHttpClient {
    async fn get(&self, id: &DocumentId) -> Result<DocumentSnapshot, DocsError> {
        let url = format!("{BASE_DOCS}/documents/{}?includeTabsContent=true", id.0);
        let resp = self.send_with_retry(|c, t| {
            c.request(Method::GET, &url).bearer_auth(t)
        }).await?;
        let resp = self.map_status(resp, &format!("docs.get {}", id.0)).await?;
        let j: serde_json::Value = resp.json().await
            .map_err(|e| DocsError::Http(format!("docs.get json: {e}")))?;
        parse_snapshot(&j, id)
    }

    // ── Stubs filled in by later tasks ────────────────────────────
    async fn create(&self, _t: &str, _f: Option<&str>) -> Result<_, _> { unimplemented!("Task 12") }
    async fn create_from_markdown(&self, _t: &str, _m: &str, _f: Option<&str>) -> Result<_, _> { unimplemented!("Task 12") }
    async fn create_from_docx(&self, _t: &str, _b: Vec<u8>, _f: Option<&str>) -> Result<_, _> { unimplemented!("Task 12") }
    async fn share(&self, _i: &DocumentId, _e: &str, _r: _) -> Result<(), _> { unimplemented!("Task 12") }
    async fn export(&self, _i: &DocumentId, _f: _) -> Result<Vec<u8>, _> { unimplemented!("Task 12") }
    async fn read_as_markdown(&self, _i: &DocumentId, _t: Option<&TabId>) -> Result<String, _> { unimplemented!("Task 12") }
    async fn read_outline(&self, _i: &DocumentId, _t: Option<&TabId>) -> Result<_, _> { unimplemented!("Task 12") }
    async fn list_named_ranges(&self, _i: &DocumentId) -> Result<_, _> { unimplemented!("Task 12") }
    async fn list_tabs(&self, _i: &DocumentId) -> Result<_, _> { unimplemented!("Task 12") }
    async fn list_revisions_since(&self, _i: &DocumentId, _s: &RevisionId) -> Result<_, _> { unimplemented!("Task 10") }
    async fn get_at_revision(&self, _i: &DocumentId, _r: &RevisionId) -> Result<String, _> { unimplemented!("Task 10") }
    async fn batch_update(&self, _i: &DocumentId, _r: Vec<serde_json::Value>, _rv: Option<&RevisionId>) -> Result<_, _> { unimplemented!("Task 10") }
    async fn add_tab(&self, _i: &DocumentId, _t: &str, _a: Option<&TabId>) -> Result<_, _> { unimplemented!("Task 12") }
    async fn create_named_range(&self, _i: &DocumentId, _n: &str, _s: _) -> Result<_, _> { unimplemented!("Task 12") }
}
```

> ⚠️ The stub block above will NOT compile (placeholder underscores in
> return types). In Step 3 we replace it with real types but with `todo!()`
> bodies so the file compiles. The intent is just to claim the trait
> impl block so subsequent tasks can drop in real bodies one-by-one.

- [ ] **Step 3: Replace stubs with `todo!()` bodies that compile**

Replace the stub block with stubs that have real return types and `todo!()` bodies. Example:

```rust
async fn create(&self, _title: &str, _parent: Option<&str>)
    -> Result<crate::gdocs::domain::DocumentMeta, DocsError>
{
    todo!("filled in by Task 12")
}
// (repeat for every method: create_from_markdown, create_from_docx,
//  share, export, read_as_markdown, read_outline, list_named_ranges,
//  list_tabs, list_revisions_since, get_at_revision, batch_update,
//  add_tab, create_named_range — copy the exact signature from
//  traits.rs and put `todo!("filled in by Task <N>")` inside)
```

- [ ] **Step 4: Write a unit test that exercises `parse_snapshot` against a fake JSON**

Add to the bottom of `http_client.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_snapshot_single_tab_legacy() {
        let j = json!({
            "title": "Test Doc",
            "revisionId": "rev_1",
            "body": {
                "content": [
                    {"sectionBreak": {}},
                    {
                        "startIndex": 1, "endIndex": 11,
                        "paragraph": {
                            "elements": [{"textRun": {"content": "Hola mundo\n"}}],
                            "paragraphStyle": {"namedStyleType": "HEADING_1"}
                        }
                    },
                    {
                        "startIndex": 11, "endIndex": 25,
                        "paragraph": {
                            "elements": [{"textRun": {"content": "Texto normal\n"}}]
                        }
                    }
                ]
            }
        });
        let snap = parse_snapshot(&j, &DocumentId("d1".into())).unwrap();
        assert_eq!(snap.title, "Test Doc");
        assert_eq!(snap.revision_id.0, "rev_1");
        assert_eq!(snap.tabs.len(), 1);
        assert!(snap.tabs[0].tab_id.is_none());
        assert_eq!(snap.tabs[0].paragraphs.len(), 2);
        assert_eq!(snap.tabs[0].paragraphs[0].n, 1);
        assert_eq!(snap.tabs[0].paragraphs[0].kind, ParagraphKind::Heading1);
        assert_eq!(snap.tabs[0].paragraphs[0].text, "Hola mundo");
        assert_eq!(snap.tabs[0].paragraphs[1].n, 2);
        assert_eq!(snap.tabs[0].paragraphs[1].kind, ParagraphKind::Paragraph);
    }

    #[test]
    fn parse_snapshot_multi_tab() {
        let j = json!({
            "title": "Multi",
            "revisionId": "r2",
            "tabs": [
                {
                    "tabProperties": {"tabId": "tab_a", "title": "A"},
                    "documentTab": {"body": {"content": [
                        {"startIndex": 1, "endIndex": 5,
                         "paragraph": {"elements": [{"textRun": {"content": "A\n"}}]}}
                    ]}}
                },
                {
                    "tabProperties": {"tabId": "tab_b", "title": "B"},
                    "documentTab": {"body": {"content": [
                        {"startIndex": 1, "endIndex": 5,
                         "paragraph": {"elements": [{"textRun": {"content": "B\n"}}]}}
                    ]}}
                }
            ]
        });
        let snap = parse_snapshot(&j, &DocumentId("d2".into())).unwrap();
        assert_eq!(snap.tabs.len(), 2);
        assert_eq!(snap.tabs[0].tab_id, Some(TabId("tab_a".into())));
        assert_eq!(snap.tabs[1].tab_id, Some(TabId("tab_b".into())));
        // Paragraph numbering is GLOBAL across tabs.
        assert_eq!(snap.tabs[0].paragraphs[0].n, 1);
        assert_eq!(snap.tabs[1].paragraphs[0].n, 2);
    }
}
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p colmena_dag_engine --lib gdocs::infrastructure::http_client`
Expected: 2 tests pass. `cargo build` should compile clean (todo!() methods are not called).

- [ ] **Step 6: Commit**

```bash
git add src/libs/colmena/src/gdocs/infrastructure/http_client.rs
git commit -m "feat(gdocs): http_client skeleton + documents.get + snapshot parser"
```

---

## Task 10: HTTP client — `batch_update`, `list_revisions_since`, `get_at_revision`

**Files:**
- Modify: `src/libs/colmena/src/gdocs/infrastructure/http_client.rs` — replace 3 `todo!()` methods.

- [ ] **Step 1: Implement `batch_update`**

Replace the `batch_update` `todo!()` body with:

```rust
async fn batch_update(
    &self, id: &DocumentId, requests: Vec<serde_json::Value>,
    required_revision: Option<&RevisionId>,
) -> Result<crate::gdocs::domain::BatchUpdateResult, DocsError> {
    let url = format!("{BASE_DOCS}/documents/{}:batchUpdate", id.0);
    let mut body = serde_json::json!({"requests": requests});
    if let Some(rev) = required_revision {
        body["writeControl"] = serde_json::json!({"requiredRevisionId": rev.0});
    }
    let resp = self.send_with_retry(|c, t| {
        c.request(Method::POST, &url).bearer_auth(t).json(&body)
    }).await?;
    let resp = self.map_status(resp, &format!("batchUpdate {}", id.0)).await?;
    let j: serde_json::Value = resp.json().await
        .map_err(|e| DocsError::Http(format!("batchUpdate json: {e}")))?;
    let revision_id_after = RevisionId(
        j.get("writeControl")
            .and_then(|v| v.get("requiredRevisionId"))
            .or_else(|| j.get("documentId").map(|_| &serde_json::Value::Null))  // fallback below
            .and_then(|v| v.as_str()).unwrap_or("").to_string()
    );
    // Some Google responses don't echo back the new revisionId. If we
    // got empty, do a tiny GET to recover it (cheap; only on first edit
    // per session).
    let revision_id_after = if revision_id_after.0.is_empty() {
        self.get(id).await?.revision_id
    } else {
        revision_id_after
    };
    let replies = j.get("replies").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    Ok(crate::gdocs::domain::BatchUpdateResult { revision_id_after, replies })
}
```

- [ ] **Step 2: Implement `list_revisions_since`**

```rust
async fn list_revisions_since(&self, id: &DocumentId, since: &RevisionId)
    -> Result<Vec<crate::gdocs::domain::RevisionMeta>, DocsError>
{
    let url = format!(
        "{BASE_DRIVE}/files/{}/revisions?fields=revisions(id,modifiedTime,lastModifyingUser/emailAddress)",
        id.0
    );
    let resp = self.send_with_retry(|c, t| {
        c.request(Method::GET, &url).bearer_auth(t)
    }).await?;
    let resp = self.map_status(resp, &format!("revisions.list {}", id.0)).await?;
    let j: serde_json::Value = resp.json().await
        .map_err(|e| DocsError::Http(format!("revisions.list json: {e}")))?;
    let arr = j.get("revisions").and_then(|v| v.as_array()).cloned().unwrap_or_default();

    // Find the index of `since` and return everything AFTER it.
    let mut started = false;
    let mut out = Vec::new();
    for rev in arr {
        let rid = rev.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
        if !started {
            if rid == since.0 { started = true; }
            continue;
        }
        let modified_time = rev.get("modifiedTime")
            .and_then(|v| v.as_str())
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|d| d.with_timezone(&chrono::Utc))
            .unwrap_or_else(chrono::Utc::now);
        let modifying_user_email = rev.pointer("/lastModifyingUser/emailAddress")
            .and_then(|v| v.as_str()).map(String::from);
        out.push(crate::gdocs::domain::RevisionMeta {
            revision_id: RevisionId(rid),
            modified_time,
            modifying_user_email,
        });
    }
    Ok(out)
}
```

- [ ] **Step 3: Implement `get_at_revision`**

```rust
async fn get_at_revision(&self, id: &DocumentId, revision: &RevisionId)
    -> Result<String, DocsError>
{
    // Use Drive `revisions.get` with media export to text/plain.
    let url = format!(
        "{BASE_DRIVE}/files/{}/revisions/{}?alt=media",
        id.0, revision.0
    );
    let resp = self.send_with_retry(|c, t| {
        c.request(Method::GET, &url).bearer_auth(t)
            .header("Accept", "text/plain")
    }).await?;
    let resp = self.map_status(resp, &format!("revisions.get {} @ {}", id.0, revision.0)).await?;
    resp.text().await.map_err(|e| DocsError::Http(format!("revisions.get text: {e}")))
}
```

- [ ] **Step 4: Add unit tests using `wiremock`**

Add to the existing `tests` module in `http_client.rs`:

```rust
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

// Note: these tests construct GoogleDocsHttpClient pointing at a mock
// server. Because the BASE_* constants are hardcoded, we exercise the
// parser/route paths via a thin shim. If `BASE_DOCS`/`BASE_DRIVE` need
// runtime override for tests, plumb that via a hidden `with_base_urls`
// constructor (see `gsheets/infrastructure/http_client.rs` for the
// same pattern).
```

> Implementation detail: copy whatever `with_base_urls` pattern gsheets uses (look at the `tests` module in `gsheets/infrastructure/http_client.rs`). If the constants aren't overridable yet, add a `pub(crate) fn with_base_urls(...)` constructor for tests.

- [ ] **Step 5: Run tests**

Run: `cargo test -p colmena_dag_engine --lib gdocs::infrastructure::http_client`
Expected: all parser tests still pass; new wiremock-based tests for batch_update/revisions pass.

- [ ] **Step 6: Commit**

```bash
git add src/libs/colmena/src/gdocs/infrastructure/http_client.rs
git commit -m "feat(gdocs): batch_update + revisions endpoints"
```

---

## Task 11: HTTP client — `read_as_markdown`, `read_outline`, `list_tabs`, `list_named_ranges`

**Files:**
- Modify: `src/libs/colmena/src/gdocs/infrastructure/http_client.rs`

- [ ] **Step 1: Implement `read_as_markdown`**

```rust
async fn read_as_markdown(&self, id: &DocumentId, tab_id: Option<&TabId>)
    -> Result<String, DocsError>
{
    // Drive export → text/markdown. Note: when a doc has multiple tabs
    // Google exports them all (joined). For single-tab requests we then
    // need to slice — but in practice agents pass tab_id only when they
    // care about a specific section, so we currently return the full
    // export and let the caller pick. v1.1 can paginate by tab via
    // docs.get + per-tab markdown serialization.
    let _ = tab_id;
    let url = format!("{BASE_DRIVE}/files/{}/export?mimeType=text/markdown", id.0);
    let resp = self.send_with_retry(|c, t| {
        c.request(Method::GET, &url).bearer_auth(t)
    }).await?;
    let resp = self.map_status(resp, &format!("export markdown {}", id.0)).await?;
    resp.text().await.map_err(|e| DocsError::Http(format!("export text: {e}")))
}
```

- [ ] **Step 2: Implement `read_outline` (via `get` + serialise)**

```rust
async fn read_outline(&self, id: &DocumentId, tab_id: Option<&TabId>)
    -> Result<Vec<crate::gdocs::domain::OutlineEntry>, DocsError>
{
    let snap = self.get(id).await?;
    let mut out = Vec::new();
    for tab in &snap.tabs {
        if let Some(want) = tab_id {
            if tab.tab_id.as_ref() != Some(want) { continue; }
        }
        for p in &tab.paragraphs {
            let preview = p.text.chars().take(80).collect::<String>();
            out.push(crate::gdocs::domain::OutlineEntry {
                paragraph: p.n,
                tab_id: tab.tab_id.clone(),
                kind: p.kind,
                text_preview: preview,
            });
        }
    }
    Ok(out)
}
```

- [ ] **Step 3: Implement `list_tabs`**

```rust
async fn list_tabs(&self, id: &DocumentId)
    -> Result<Vec<crate::gdocs::domain::TabMeta>, DocsError>
{
    let url = format!("{BASE_DOCS}/documents/{}?includeTabsContent=false", id.0);
    let resp = self.send_with_retry(|c, t| {
        c.request(Method::GET, &url).bearer_auth(t)
    }).await?;
    let resp = self.map_status(resp, &format!("list_tabs {}", id.0)).await?;
    let j: serde_json::Value = resp.json().await
        .map_err(|e| DocsError::Http(format!("list_tabs json: {e}")))?;
    let mut out = Vec::new();
    fn walk(arr: &[serde_json::Value], parent: Option<TabId>,
            out: &mut Vec<crate::gdocs::domain::TabMeta>, idx: &mut u32)
    {
        for t in arr {
            let props = t.get("tabProperties").cloned().unwrap_or_default();
            let tab_id = TabId(props.get("tabId").and_then(|v| v.as_str()).unwrap_or("").to_string());
            let title = props.get("title").and_then(|v| v.as_str()).unwrap_or("").to_string();
            out.push(crate::gdocs::domain::TabMeta {
                tab_id: tab_id.clone(), title, index: *idx, parent_tab_id: parent.clone(),
            });
            *idx += 1;
            if let Some(children) = t.get("childTabs").and_then(|v| v.as_array()) {
                walk(children, Some(tab_id), out, idx);
            }
        }
    }
    if let Some(arr) = j.get("tabs").and_then(|v| v.as_array()) {
        let mut idx = 0u32;
        walk(arr, None, &mut out, &mut idx);
    }
    Ok(out)
}
```

- [ ] **Step 4: Implement `list_named_ranges`**

```rust
async fn list_named_ranges(&self, id: &DocumentId)
    -> Result<Vec<crate::gdocs::domain::NamedRangeMeta>, DocsError>
{
    let snap = self.get(id).await?;
    // The full snapshot fetch already includes namedRanges. We need
    // a thin pass over the raw JSON to extract them — re-fetch via
    // `documents.get` with no tab content to keep it cheap.
    let url = format!("{BASE_DOCS}/documents/{}?fields=namedRanges", id.0);
    let resp = self.send_with_retry(|c, t| {
        c.request(Method::GET, &url).bearer_auth(t)
    }).await?;
    let resp = self.map_status(resp, "list_named_ranges").await?;
    let j: serde_json::Value = resp.json().await
        .map_err(|e| DocsError::Http(format!("named_ranges json: {e}")))?;
    let mut out = Vec::new();
    if let Some(map) = j.get("namedRanges").and_then(|v| v.as_object()) {
        for (name, group) in map {
            if let Some(ranges) = group.get("namedRanges").and_then(|v| v.as_array()) {
                for r in ranges {
                    let id_s = r.get("namedRangeId").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    // Find which paragraphs the range spans.
                    let nr = r.get("ranges").and_then(|v| v.as_array())
                        .and_then(|a| a.first()).cloned().unwrap_or_default();
                    let start = nr.get("startIndex").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                    let end   = nr.get("endIndex"  ).and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                    // Map index range to paragraph range using the snapshot.
                    let (ps, pe) = map_index_range_to_paragraphs(&snap, start, end);
                    out.push(crate::gdocs::domain::NamedRangeMeta {
                        named_range_id: id_s,
                        name: name.clone(),
                        paragraph_start: ps,
                        paragraph_end:   pe,
                    });
                }
            }
        }
    }
    Ok(out)
}
```

> **Helper:** add `fn map_index_range_to_paragraphs(snap: &DocumentSnapshot, s: u32, e: u32) -> (u32, u32)` near the bottom of the file. Walk the paragraphs of the FIRST tab (named ranges in multi-tab docs are tab-scoped; v1 assumes single-tab for named ranges to keep complexity down).

- [ ] **Step 5: Compile and run tests**

Run: `cargo test -p colmena_dag_engine --lib gdocs::infrastructure::http_client`
Expected: existing tests pass; add wiremock tests for read_outline + list_tabs as you go.

- [ ] **Step 6: Commit**

```bash
git add src/libs/colmena/src/gdocs/infrastructure/http_client.rs
git commit -m "feat(gdocs): read_as_markdown + read_outline + list_tabs + list_named_ranges"
```

---

## Task 12: HTTP client — `create`, `create_from_markdown`, `create_from_docx`, `share`, `export`, `add_tab`, `create_named_range`

**Files:**
- Modify: `src/libs/colmena/src/gdocs/infrastructure/http_client.rs`

> All seven endpoints are Drive or Docs `POST`s with predictable JSON bodies. The most novel is `create_from_markdown` because it ALSO re-exports as markdown to compute `LossyConversion` records. Pattern reference: `gsheets/infrastructure/http_client.rs` `create_from_xlsx` for the multipart upload.

- [ ] **Step 1: Implement `create` (blank doc)**

```rust
async fn create(&self, title: &str, parent_folder: Option<&str>)
    -> Result<crate::gdocs::domain::DocumentMeta, DocsError>
{
    // Step 1: create blank via Docs API.
    let resp = self.send_with_retry(|c, t| {
        c.request(Method::POST, &format!("{BASE_DOCS}/documents"))
            .bearer_auth(t)
            .json(&serde_json::json!({"title": title}))
    }).await?;
    let resp = self.map_status(resp, "documents.create").await?;
    let j: serde_json::Value = resp.json().await
        .map_err(|e| DocsError::Http(format!("create json: {e}")))?;
    let doc_id = DocumentId(j.get("documentId").and_then(|v| v.as_str())
        .ok_or_else(|| DocsError::Http("missing documentId".into()))?.to_string());

    // Step 2: move to parent_folder if requested (or default).
    let folder = parent_folder.or(self.cfg.default_parent_folder.as_deref());
    if folder.is_none() {
        return Err(DocsError::NoParentFolder);
    }
    let folder = folder.unwrap().to_string();
    let url = format!("{BASE_DRIVE}/files/{}?addParents={}&removeParents=root", doc_id.0, folder);
    let _ = self.send_with_retry(|c, t| {
        c.request(Method::PATCH, &url).bearer_auth(t).json(&serde_json::json!({}))
    }).await?;

    // Step 3: fetch fresh snapshot for revision_id + tabs.
    let snap = self.get(&doc_id).await?;
    Ok(crate::gdocs::domain::DocumentMeta {
        doc_id: snap.doc_id.clone(),
        url: format!("https://docs.google.com/document/d/{}", snap.doc_id.0),
        title: snap.title,
        revision_id: snap.revision_id,
        tabs: self.list_tabs(&doc_id).await?,
    })
}
```

- [ ] **Step 2: Implement `create_from_markdown` with loss detection**

```rust
async fn create_from_markdown(&self, title: &str, md: &str, parent_folder: Option<&str>)
    -> Result<crate::gdocs::domain::CreateFromMarkdownResult, DocsError>
{
    let folder = parent_folder.or(self.cfg.default_parent_folder.as_deref())
        .ok_or(DocsError::NoParentFolder)?;
    // Drive multipart upload — markdown body, target mimeType = google-apps.document.
    let metadata = serde_json::json!({
        "name": title,
        "mimeType": "application/vnd.google-apps.document",
        "parents": [folder],
    });
    let boundary = "colmena_gdocs_boundary";
    let body = format!(
        "--{boundary}\r\nContent-Type: application/json; charset=UTF-8\r\n\r\n{}\r\n\
         --{boundary}\r\nContent-Type: text/markdown\r\n\r\n{md}\r\n\
         --{boundary}--",
        serde_json::to_string(&metadata).unwrap()
    );
    let resp = self.send_with_retry(|c, t| {
        c.request(Method::POST,
            &format!("{BASE_DRIVE_UPLD}/files?uploadType=multipart"))
            .bearer_auth(t)
            .header("Content-Type", format!("multipart/related; boundary={boundary}"))
            .body(body.clone())
    }).await?;
    let resp = self.map_status(resp, "create_from_markdown upload").await?;
    let j: serde_json::Value = resp.json().await
        .map_err(|e| DocsError::Http(format!("upload json: {e}")))?;
    let doc_id = DocumentId(j.get("id").and_then(|v| v.as_str())
        .ok_or_else(|| DocsError::Http("missing id".into()))?.to_string());

    // Re-export as markdown to compute lossy_conversions.
    let re_md = self.read_as_markdown(&doc_id, None).await.unwrap_or_default();
    let lossy = diff_markdown_for_lossy(md, &re_md);

    let snap = self.get(&doc_id).await?;
    let outline = self.read_outline(&doc_id, None).await?;
    let meta = crate::gdocs::domain::DocumentMeta {
        doc_id: snap.doc_id.clone(),
        url: format!("https://docs.google.com/document/d/{}", snap.doc_id.0),
        title: snap.title,
        revision_id: snap.revision_id,
        tabs: self.list_tabs(&doc_id).await?,
    };
    Ok(crate::gdocs::domain::CreateFromMarkdownResult {
        meta, outline_snapshot: outline, lossy_conversions: lossy,
    })
}
```

Add helper:

```rust
/// Compare input markdown vs Google's re-export and flag elements
/// present in the input but absent in the export. Conservative: only
/// flags well-known lossy elements (footnotes, images, math).
fn diff_markdown_for_lossy(orig: &str, after: &str) -> Vec<crate::gdocs::domain::LossyConversion> {
    let mut out = Vec::new();
    // Footnotes: pattern [^id] in orig, gone in after.
    let fn_re = regex::Regex::new(r"\[\^([^\]]+)\]:?[^\n]*").unwrap();
    for m in fn_re.find_iter(orig) {
        if !after.contains(m.as_str()) {
            out.push(crate::gdocs::domain::LossyConversion {
                element_type: "footnote".into(),
                original_markdown: m.as_str().chars().take(200).collect(),
            });
        }
    }
    // Images: ![alt](url) in orig, no image marker in after.
    let img_re = regex::Regex::new(r"!\[[^\]]*\]\([^)]+\)").unwrap();
    for m in img_re.find_iter(orig) {
        if !after.contains(m.as_str()) {
            out.push(crate::gdocs::domain::LossyConversion {
                element_type: "image_reference".into(),
                original_markdown: m.as_str().chars().take(200).collect(),
            });
        }
    }
    // Math: $...$ inline or $$...$$ block.
    let math_re = regex::Regex::new(r"\$\$?[^$\n]+\$\$?").unwrap();
    for m in math_re.find_iter(orig) {
        if !after.contains(m.as_str()) {
            out.push(crate::gdocs::domain::LossyConversion {
                element_type: "math_expression".into(),
                original_markdown: m.as_str().chars().take(200).collect(),
            });
        }
    }
    out
}
```

- [ ] **Step 3: Implement `create_from_docx`**

Same multipart pattern as `create_from_markdown` but with body mime
`application/vnd.openxmlformats-officedocument.wordprocessingml.document`
and binary bytes. No re-export needed (docx→doc conversion is
high-fidelity and we don't try to compute losses).

```rust
async fn create_from_docx(&self, title: &str, bytes: Vec<u8>, parent_folder: Option<&str>)
    -> Result<crate::gdocs::domain::DocumentMeta, DocsError>
{
    let folder = parent_folder.or(self.cfg.default_parent_folder.as_deref())
        .ok_or(DocsError::NoParentFolder)?;
    let metadata = serde_json::json!({
        "name": title,
        "mimeType": "application/vnd.google-apps.document",
        "parents": [folder],
    });
    let boundary = "colmena_gdocs_boundary";
    let metadata_part = format!(
        "--{boundary}\r\nContent-Type: application/json; charset=UTF-8\r\n\r\n{}\r\n",
        serde_json::to_string(&metadata).unwrap()
    );
    let media_part_header = format!(
        "--{boundary}\r\nContent-Type: application/vnd.openxmlformats-officedocument.wordprocessingml.document\r\n\r\n"
    );
    let trailer = format!("\r\n--{boundary}--");
    let mut body = Vec::new();
    body.extend_from_slice(metadata_part.as_bytes());
    body.extend_from_slice(media_part_header.as_bytes());
    body.extend_from_slice(&bytes);
    body.extend_from_slice(trailer.as_bytes());
    let resp = self.send_with_retry(|c, t| {
        c.request(Method::POST,
            &format!("{BASE_DRIVE_UPLD}/files?uploadType=multipart"))
            .bearer_auth(t)
            .header("Content-Type", format!("multipart/related; boundary={boundary}"))
            .body(body.clone())
    }).await?;
    let resp = self.map_status(resp, "create_from_docx upload").await?;
    let j: serde_json::Value = resp.json().await
        .map_err(|e| DocsError::Http(format!("upload json: {e}")))?;
    let doc_id = DocumentId(j.get("id").and_then(|v| v.as_str())
        .ok_or_else(|| DocsError::Http("missing id".into()))?.to_string());
    let snap = self.get(&doc_id).await?;
    Ok(crate::gdocs::domain::DocumentMeta {
        doc_id: snap.doc_id.clone(),
        url: format!("https://docs.google.com/document/d/{}", snap.doc_id.0),
        title: snap.title,
        revision_id: snap.revision_id,
        tabs: self.list_tabs(&doc_id).await?,
    })
}
```

- [ ] **Step 4: Implement `share`**

```rust
async fn share(&self, id: &DocumentId, email: &str, role: crate::gdocs::domain::ShareRole)
    -> Result<(), DocsError>
{
    let url = format!("{BASE_DRIVE}/files/{}/permissions", id.0);
    let body = serde_json::json!({
        "role": role.as_api_str(),
        "type": "user",
        "emailAddress": email,
    });
    let resp = self.send_with_retry(|c, t| {
        c.request(Method::POST, &url).bearer_auth(t).json(&body)
    }).await?;
    self.map_status(resp, "permissions.create").await?;
    Ok(())
}
```

- [ ] **Step 5: Implement `export`**

```rust
async fn export(&self, id: &DocumentId, format: crate::gdocs::domain::ExportFormat)
    -> Result<Vec<u8>, DocsError>
{
    let url = format!(
        "{BASE_DRIVE}/files/{}/export?mimeType={}",
        id.0, urlencoding::encode(format.mime())
    );
    let resp = self.send_with_retry(|c, t| {
        c.request(Method::GET, &url).bearer_auth(t)
    }).await?;
    let resp = self.map_status(resp, &format!("export {} {}", id.0, format.mime())).await?;
    let bytes = resp.bytes().await
        .map_err(|e| DocsError::Http(format!("export bytes: {e}")))?;
    Ok(bytes.to_vec())
}
```

> If `urlencoding` is not already a dep, add it (~10 KB, single function). Otherwise inline manual encoding for the mime string.

- [ ] **Step 6: Implement `add_tab`**

```rust
async fn add_tab(&self, id: &DocumentId, title: &str, after_tab: Option<&TabId>)
    -> Result<crate::gdocs::domain::TabMeta, DocsError>
{
    // Pre-check: does a tab with this title already exist?
    let existing = self.list_tabs(id).await?;
    if existing.iter().any(|t| t.title == title) {
        return Err(DocsError::TabExists(title.into()));
    }
    let mut props = serde_json::json!({"title": title});
    if let Some(after) = after_tab {
        props["indexAfter"] = serde_json::json!(after.0);
    }
    let requests = vec![serde_json::json!({
        "addDocumentTab": {"tabProperties": props}
    })];
    let _ = self.batch_update(id, requests, None).await?;
    // The reply doesn't reliably echo the tab id; re-list tabs.
    let after_list = self.list_tabs(id).await?;
    after_list.into_iter().find(|t| t.title == title)
        .ok_or_else(|| DocsError::Internal("add_tab: new tab not found after list".into()))
}
```

- [ ] **Step 7: Implement `create_named_range`**

```rust
async fn create_named_range(
    &self, id: &DocumentId, name: &str, scope: crate::gdocs::domain::Scope,
) -> Result<crate::gdocs::domain::NamedRangeMeta, DocsError>
{
    // For v1 we only support `Scope::Paragraph { n }` and a synthetic
    // "find first match" scope handled by application layer (which
    // calls us with Paragraph already resolved).
    use crate::gdocs::domain::Scope::*;
    let n = match scope {
        Paragraph { n } => n,
        _ => return Err(DocsError::InvalidArgs(
            "create_named_range scope must be {kind:'paragraph', n: <N>}".into()
        )),
    };
    let snap = self.get(id).await?;
    let p = snap.tabs.iter().flat_map(|t| t.paragraphs.iter())
        .find(|p| p.n == n)
        .ok_or_else(|| DocsError::InvalidArgs(format!("paragraph {n} not found")))?;
    let requests = vec![serde_json::json!({
        "createNamedRange": {
            "name": name,
            "range": {"startIndex": p.start_index, "endIndex": p.end_index}
        }
    })];
    let _ = self.batch_update(id, requests, Some(&snap.revision_id)).await?;
    let listed = self.list_named_ranges(id).await?;
    listed.into_iter().find(|nr| nr.name == name)
        .ok_or_else(|| DocsError::Internal("named_range not found after create".into()))
}
```

- [ ] **Step 8: Add wiremock-backed unit tests for each new endpoint (~7 tests)**

Pattern: spin up a `MockServer`, mount expectations for each Google endpoint, instantiate `GoogleDocsHttpClient::with_base_urls(...)`, call the method, assert the result.

- [ ] **Step 9: Run all gdocs tests**

Run: `cargo test -p colmena_dag_engine --lib gdocs::`
Expected: all tests pass.

- [ ] **Step 10: Commit**

```bash
git add src/libs/colmena/src/gdocs/infrastructure/http_client.rs
git commit -m "feat(gdocs): create/share/export/add_tab/named_range endpoints"
```

---

## Task 13: `markdown_to_docs_ops` — pure converter

**Files:**
- Create: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/markdown_to_docs_ops.rs`
- Create: `tests/gdocs_markdown_fixtures/*.md` + `*.ops.json` (golden fixtures)

> **Pure module.** No I/O. Takes markdown + insertion point; returns a `Vec<serde_json::Value>` of batchUpdate `Request`s plus a `Vec<LossyConversion>` for unsupported elements.

- [ ] **Step 1: Define the public API**

```rust
//! Pure converter from markdown to a batchUpdate request sequence.
//! Used by every tool that inserts formatted content (insert_*,
//! replace_section, append_markdown, add_tab with body, create_named_range).
//!
//! Strategy:
//! 1. Tokenise via pulldown-cmark.
//! 2. Build a flat list of "doc-events" (Paragraph(text, runs[]),
//!    Heading(level, text), BulletItem(text), Table(rows), …).
//! 3. Emit a write-backwards-safe sequence of batchUpdate requests:
//!    - InsertTextRequest with each paragraph's text (in order, all
//!      relative to a single `location.index`).
//!    - UpdateParagraphStyleRequest applying HEADING_N / NORMAL_TEXT to
//!      the just-inserted paragraph ranges.
//!    - CreateParagraphBulletsRequest for list items.
//!    - UpdateTextStyleRequest for bold/italic/underline/link/etc.
//!    - InsertTableRequest + per-cell text inserts for tables.

use crate::gdocs::domain::{LossyConversion, TabId};
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd, HeadingLevel as CmarkHeading};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InsertionPoint {
    /// Concrete (segment-relative) index. Pass `tab_id=None` for the
    /// first/only tab.
    Index { index: u32, tab_id: Option<TabId> },
    /// End-of-segment (used by `append_markdown` and `add_tab`).
    EndOfSegment { tab_id: Option<TabId> },
}

pub struct ConversionResult {
    pub requests: Vec<serde_json::Value>,
    pub lossy:    Vec<LossyConversion>,
}

pub fn markdown_to_requests(md: &str, ip: InsertionPoint) -> ConversionResult {
    let mut emitter = Emitter::new(ip);
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TASKLISTS);
    let parser = Parser::new_ext(md, opts);
    for ev in parser { emitter.feed(ev); }
    emitter.finish()
}

struct Emitter {
    ip: InsertionPoint,
    out_requests: Vec<serde_json::Value>,
    out_lossy:    Vec<LossyConversion>,
    // Running text buffer for the current paragraph.
    cur_text: String,
    // Style spans relative to cur_text's index 0.
    cur_styles: Vec<(usize, usize, StylePatchLite)>,
    // Stack of active inline styles (bold/italic/etc).
    style_stack: Vec<StylePatchLite>,
    // Current paragraph metadata.
    cur_kind: ParaKind,
    // Aggregate "lines" we've emitted so far (used for write-backwards offsetting).
    emitted_offset: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ParaKind { Normal, H1, H2, H3, H4, H5, H6, Bullet, Numbered, CodeBlock, Quote }

#[derive(Debug, Clone, Default)]
struct StylePatchLite {
    bold:          bool,
    italic:        bool,
    strikethrough: bool,
    link:          Option<String>,
}

impl Emitter {
    fn new(ip: InsertionPoint) -> Self {
        Self {
            ip, out_requests: vec![], out_lossy: vec![],
            cur_text: String::new(), cur_styles: vec![], style_stack: vec![],
            cur_kind: ParaKind::Normal, emitted_offset: 0,
        }
    }
    fn feed(&mut self, ev: Event) {
        // (Implementation detail: handle each Event variant — Start(Tag::Heading),
        // End(TagEnd::Heading), Text(cow), Code(cow), Start(Tag::List), …, plus
        // Start(Tag::Image) → record as LossyConversion(image_reference, alt).
        // See pulldown-cmark docs for the full Event enum. Pseudocode:
        match ev {
            Event::Start(Tag::Heading { level, .. }) => {
                self.flush_paragraph();
                self.cur_kind = heading_to_para_kind(level);
            }
            Event::End(TagEnd::Heading(_)) => { self.flush_paragraph(); }
            Event::Start(Tag::Paragraph)   => { /* default Normal */ }
            Event::End(TagEnd::Paragraph)  => { self.flush_paragraph(); }
            Event::Start(Tag::Emphasis)    => self.push_style(|s| s.italic = true),
            Event::End(TagEnd::Emphasis)   => self.pop_style(),
            Event::Start(Tag::Strong)      => self.push_style(|s| s.bold = true),
            Event::End(TagEnd::Strong)     => self.pop_style(),
            Event::Start(Tag::Strikethrough)  => self.push_style(|s| s.strikethrough = true),
            Event::End(TagEnd::Strikethrough) => self.pop_style(),
            Event::Start(Tag::Link { dest_url, .. }) => self.push_style(
                |s| s.link = Some(dest_url.to_string())
            ),
            Event::End(TagEnd::Link)       => self.pop_style(),
            Event::Text(cow) | Event::Code(cow) => self.append_text(&cow),
            Event::Start(Tag::Image { .. }) => self.out_lossy.push(LossyConversion {
                element_type: "image_reference".into(), original_markdown: "<image>".into(),
            }),
            Event::Start(Tag::List(None))       => { self.cur_kind = ParaKind::Bullet; }
            Event::Start(Tag::List(Some(_)))    => { self.cur_kind = ParaKind::Numbered; }
            Event::End(TagEnd::List(_))         => { self.cur_kind = ParaKind::Normal; }
            Event::SoftBreak | Event::HardBreak => self.append_text(" "),
            Event::Rule                         => self.flush_rule(),
            // Tables and code blocks: handle similarly — see fixture tests.
            _ => {}
        }
        */
    }
    fn push_style(&mut self, f: impl FnOnce(&mut StylePatchLite)) {
        let top = self.style_stack.last().cloned().unwrap_or_default();
        let mut new = top; f(&mut new);
        self.style_stack.push(new);
    }
    fn pop_style(&mut self) { self.style_stack.pop(); }
    fn append_text(&mut self, s: &str) { /* …append + record style span */ }
    fn flush_paragraph(&mut self) { /* …emit InsertTextRequest + style requests */ }
    fn flush_rule(&mut self) { /* …emit InsertSectionBreakRequest */ }
    fn finish(self) -> ConversionResult {
        ConversionResult { requests: self.out_requests, lossy: self.out_lossy }
    }
}

fn heading_to_para_kind(level: CmarkHeading) -> ParaKind {
    match level {
        CmarkHeading::H1 => ParaKind::H1,
        CmarkHeading::H2 => ParaKind::H2,
        CmarkHeading::H3 => ParaKind::H3,
        CmarkHeading::H4 => ParaKind::H4,
        CmarkHeading::H5 => ParaKind::H5,
        CmarkHeading::H6 => ParaKind::H6,
    }
}
```

> The pseudocode above marks where the per-Event behaviour goes. Fill in the methods. The TDD approach: write the golden fixture first, then iterate the implementation until the golden matches.

- [ ] **Step 2: Create the first golden fixture — single heading + paragraph**

Create `tests/gdocs_markdown_fixtures/01_heading_and_paragraph.md`:
```markdown
# Título

Texto del párrafo.
```

Create `tests/gdocs_markdown_fixtures/01_heading_and_paragraph.ops.json` with the EXPECTED request sequence (write it by HAND first — that's the spec for correct behaviour). Example shape (insertion point index=1, tab_id=None):

```json
{
  "requests": [
    {"insertText": {"location": {"index": 1}, "text": "Título\n"}},
    {"updateParagraphStyle": {
      "range": {"startIndex": 1, "endIndex": 8},
      "paragraphStyle": {"namedStyleType": "HEADING_1"},
      "fields": "namedStyleType"
    }},
    {"insertText": {"location": {"index": 8}, "text": "Texto del párrafo.\n"}},
    {"updateParagraphStyle": {
      "range": {"startIndex": 8, "endIndex": 27},
      "paragraphStyle": {"namedStyleType": "NORMAL_TEXT"},
      "fields": "namedStyleType"
    }}
  ],
  "lossy": []
}
```

- [ ] **Step 3: Write the golden-fixture test loader**

Add to `markdown_to_docs_ops.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn run_fixture(stem: &str) {
        let md_path  = format!("tests/gdocs_markdown_fixtures/{stem}.md");
        let exp_path = format!("tests/gdocs_markdown_fixtures/{stem}.ops.json");
        let md  = std::fs::read_to_string(&md_path).expect("md fixture");
        let exp_raw = std::fs::read_to_string(&exp_path).expect("expected json");
        let exp: serde_json::Value = serde_json::from_str(&exp_raw).expect("json parse");
        let got = markdown_to_requests(&md, InsertionPoint::Index { index: 1, tab_id: None });
        let got_json = serde_json::json!({
            "requests": got.requests,
            "lossy": got.lossy,
        });
        assert_eq!(got_json, exp, "fixture {stem} mismatch");
    }

    #[test] fn fixture_01_heading_and_paragraph() { run_fixture("01_heading_and_paragraph"); }
}
```

- [ ] **Step 4: Run the test and iterate the implementation until green**

Run: `cargo test -p colmena_dag_engine --lib markdown_to_docs_ops::tests::fixture_01`
Expected: initially RED. Fill in `Emitter::flush_paragraph` / `append_text` until GREEN.

- [ ] **Step 5: Add 13 more golden fixtures (one per supported element)**

Create fixtures:
- `02_bold_italic_link.md`
- `03_bullet_list.md`
- `04_numbered_list.md`
- `05_inline_code.md`
- `06_code_block.md`
- `07_blockquote.md`
- `08_horizontal_rule.md`
- `09_strikethrough.md`
- `10_table_simple.md`
- `11_nested_emphasis.md`
- `12_lossy_footnote.md` (expected: empty requests + 1 lossy)
- `13_lossy_image.md` (expected: empty requests + 1 lossy)
- `14_lossy_math.md` (expected: empty requests + 1 lossy)

For each, write the `.md` then the `.ops.json` by hand. Add one `#[test]` per fixture.

- [ ] **Step 6: Add `pub mod markdown_to_docs_ops;` to `llm_synthetic_tools/mod.rs`**

- [ ] **Step 7: Run all fixtures**

Run: `cargo test -p colmena_dag_engine --lib markdown_to_docs_ops`
Expected: 14 fixture tests pass.

- [ ] **Step 8: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/markdown_to_docs_ops.rs \
        src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs \
        tests/gdocs_markdown_fixtures/
git commit -m "feat(gdocs): markdown→batchUpdate ops converter with 14 golden fixtures"
```

---

## Task 14: Application — `scope_resolver`

**Files:**
- Create: `src/libs/colmena/src/gdocs/application/scope_resolver.rs`

> Resolves a `Scope` against a `DocumentSnapshot` into a paragraph-range
> `(start_para, end_para, tab_id)` so downstream use cases can clamp
> their text searches.

- [ ] **Step 1: Write `scope_resolver.rs`**

```rust
//! Translate a `Scope` to a concrete paragraph-range against a snapshot.

use crate::gdocs::domain::{DocsError, DocumentSnapshot, ParagraphKind, Scope, TabId};

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedScope {
    pub tab_id: Option<TabId>,
    pub paragraph_start: u32,   // inclusive
    pub paragraph_end:   u32,   // inclusive
}

pub fn resolve(scope: &Scope, snap: &DocumentSnapshot) -> Result<ResolvedScope, DocsError> {
    use Scope::*;
    match scope {
        All => {
            let first = snap.tabs.iter().flat_map(|t| t.paragraphs.iter())
                .map(|p| p.n).min().unwrap_or(1);
            let last  = snap.tabs.iter().flat_map(|t| t.paragraphs.iter())
                .map(|p| p.n).max().unwrap_or(first);
            Ok(ResolvedScope { tab_id: None, paragraph_start: first, paragraph_end: last })
        }
        Tab { tab_id } => {
            let tab = snap.tabs.iter().find(|t| t.tab_id.as_ref() == Some(tab_id))
                .ok_or_else(|| DocsError::TabNotFound(tab_id.0.clone()))?;
            let first = tab.paragraphs.iter().map(|p| p.n).min().unwrap_or(1);
            let last  = tab.paragraphs.iter().map(|p| p.n).max().unwrap_or(first);
            Ok(ResolvedScope { tab_id: Some(tab_id.clone()), paragraph_start: first, paragraph_end: last })
        }
        Paragraph { n } => {
            let n = *n;
            // Find which tab owns this paragraph.
            let tab = snap.tabs.iter().find(|t| t.paragraphs.iter().any(|p| p.n == n));
            let tab_id = tab.and_then(|t| t.tab_id.clone());
            if tab.is_none() {
                return Err(DocsError::InvalidArgs(format!("paragraph {n} not found")));
            }
            Ok(ResolvedScope { tab_id, paragraph_start: n, paragraph_end: n })
        }
        UnderHeading { heading } => {
            let level = heading_level(heading);
            // Find the heading paragraph; then end at the next same-or-higher heading.
            let mut found: Option<(u32, Option<TabId>)> = None;
            for tab in &snap.tabs {
                for p in &tab.paragraphs {
                    if matches_heading(p, heading) {
                        found = Some((p.n, tab.tab_id.clone()));
                        break;
                    }
                }
                if found.is_some() { break; }
            }
            let (start, tab_id) = found.ok_or_else(|| DocsError::InvalidArgs(
                format!("heading '{heading}' not found")
            ))?;
            // Find next heading at level <= our level after start.
            let mut end_excl = u32::MAX;
            for tab in &snap.tabs {
                for p in &tab.paragraphs {
                    if p.n <= start { continue; }
                    if let Some(l2) = paragraph_heading_level(p) {
                        if l2 <= level { end_excl = end_excl.min(p.n); break; }
                    }
                }
            }
            // Use last available paragraph if no next heading.
            let last_p = snap.tabs.iter().flat_map(|t| t.paragraphs.iter())
                .map(|p| p.n).max().unwrap_or(start);
            let paragraph_end = if end_excl == u32::MAX { last_p } else { end_excl - 1 };
            Ok(ResolvedScope { tab_id, paragraph_start: start + 1, paragraph_end })
        }
        BetweenHeadings { after, before } => {
            // start = paragraph AFTER the `after` heading
            // end   = paragraph BEFORE the `before` heading (or EOF if None)
            let start = find_heading_paragraph(snap, after)
                .ok_or_else(|| DocsError::InvalidArgs(format!("heading '{after}' not found")))?;
            let end = match before {
                Some(b) => find_heading_paragraph(snap, b)
                    .ok_or_else(|| DocsError::InvalidArgs(format!("heading '{b}' not found")))?,
                None => snap.tabs.iter().flat_map(|t| t.paragraphs.iter())
                    .map(|p| p.n).max().unwrap_or(start) + 1,
            };
            if end <= start + 1 {
                return Err(DocsError::InvalidArgs(
                    "between_headings: 'before' is not after 'after'".into()
                ));
            }
            Ok(ResolvedScope { tab_id: None, paragraph_start: start + 1, paragraph_end: end - 1 })
        }
    }
}

fn matches_heading(p: &crate::gdocs::domain::ParagraphSnapshot, target: &str) -> bool {
    // Accept "## Resumen" — strip the `#`s, compare text.
    let target = target.trim_start_matches('#').trim();
    p.text.trim() == target
        && matches!(p.kind, ParagraphKind::Heading1 | ParagraphKind::Heading2
                          | ParagraphKind::Heading3 | ParagraphKind::Heading4
                          | ParagraphKind::Heading5 | ParagraphKind::Heading6
                          | ParagraphKind::Title    | ParagraphKind::Subtitle)
}

fn paragraph_heading_level(p: &crate::gdocs::domain::ParagraphSnapshot) -> Option<u8> {
    match p.kind {
        ParagraphKind::Heading1 => Some(1),
        ParagraphKind::Heading2 => Some(2),
        ParagraphKind::Heading3 => Some(3),
        ParagraphKind::Heading4 => Some(4),
        ParagraphKind::Heading5 => Some(5),
        ParagraphKind::Heading6 => Some(6),
        _ => None,
    }
}

fn heading_level(s: &str) -> u8 {
    let hashes = s.chars().take_while(|c| *c == '#').count() as u8;
    hashes.max(1).min(6)
}

fn find_heading_paragraph(snap: &DocumentSnapshot, h: &str) -> Option<u32> {
    snap.tabs.iter().flat_map(|t| t.paragraphs.iter())
        .find(|p| matches_heading(p, h))
        .map(|p| p.n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gdocs::domain::{DocumentId, ParagraphSnapshot, RevisionId, TabSnapshot};

    fn snap(paras: Vec<(u32, ParagraphKind, &str)>) -> DocumentSnapshot {
        DocumentSnapshot {
            doc_id: DocumentId("d".into()), revision_id: RevisionId("r".into()),
            title: "T".into(),
            tabs: vec![TabSnapshot {
                tab_id: None,
                paragraphs: paras.into_iter().enumerate().map(|(_i, (n, kind, t))| {
                    ParagraphSnapshot { n, kind, text: t.into(), start_index: 0, end_index: 0 }
                }).collect(),
            }],
        }
    }

    #[test]
    fn under_heading_extends_to_next_same_level() {
        let s = snap(vec![
            (1, ParagraphKind::Heading2, "Sección 2"),
            (2, ParagraphKind::Paragraph, "primer texto"),
            (3, ParagraphKind::Paragraph, "segundo texto"),
            (4, ParagraphKind::Heading2, "Sección 3"),
            (5, ParagraphKind::Paragraph, "fuera del scope"),
        ]);
        let r = resolve(&Scope::UnderHeading { heading: "## Sección 2".into() }, &s).unwrap();
        assert_eq!(r.paragraph_start, 2);
        assert_eq!(r.paragraph_end, 3);
    }

    #[test]
    fn between_headings_inclusive_exclusive() {
        let s = snap(vec![
            (1, ParagraphKind::Heading1, "A"),
            (2, ParagraphKind::Paragraph, "x"),
            (3, ParagraphKind::Heading1, "B"),
            (4, ParagraphKind::Paragraph, "y"),
        ]);
        let r = resolve(&Scope::BetweenHeadings {
            after: "# A".into(), before: Some("# B".into())
        }, &s).unwrap();
        assert_eq!(r.paragraph_start, 2);
        assert_eq!(r.paragraph_end, 2);
    }

    #[test]
    fn paragraph_scope_returns_single() {
        let s = snap(vec![
            (1, ParagraphKind::Paragraph, "a"),
            (2, ParagraphKind::Paragraph, "b"),
        ]);
        let r = resolve(&Scope::Paragraph { n: 2 }, &s).unwrap();
        assert_eq!(r.paragraph_start, 2);
        assert_eq!(r.paragraph_end, 2);
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p colmena_dag_engine --lib gdocs::application::scope_resolver`
Expected: 3 tests pass.

- [ ] **Step 3: Commit**

```bash
git add src/libs/colmena/src/gdocs/application/scope_resolver.rs
git commit -m "feat(gdocs): scope resolver (paragraph range translator)"
```

---

## Task 15: Application — `co_edit_guard`

**Files:**
- Create: `src/libs/colmena/src/gdocs/application/co_edit_guard.rs`

> Runs before EVERY edit. Returns `Ok(GuardOk)` to proceed (possibly
> carrying outside-scope soft warnings) or `Err(HumanChangesPending)`
> if human edits overlap the requested scope.

- [ ] **Step 1: Write `co_edit_guard.rs`**

```rust
//! Co-edit guard pipeline. Detects human edits to the doc since the
//! agent's last known revision, partitions them by overlap with the
//! requested edit scope, and returns either `Ok(GuardOk)` (with
//! out-of-scope changes as soft warnings) or `Err(HumanChangesPending)`.

use crate::gdocs::application::scope_resolver::{self, ResolvedScope};
use crate::gdocs::domain::{
    DocsClient, DocsError, DocumentId, DocumentSnapshot, HumanChange, HumanChangeKind,
    RevisionId, Scope,
};
use crate::gdocs::infrastructure::outline_cache::OutlineCache;
use crate::gdocs::infrastructure::revision_store::RevisionStore;
use similar::TextDiff;

pub struct GuardOk {
    pub snapshot: DocumentSnapshot,
    pub resolved_scope: ResolvedScope,
    pub soft_warnings: Vec<HumanChange>,
}

pub struct GuardContext<'a> {
    pub client:         &'a dyn DocsClient,
    pub cache:          &'a OutlineCache,
    pub revisions:      &'a dyn RevisionStore,
    pub session_id:     &'a str,
    pub sa_email:       Option<&'a str>,    // identity to filter own revisions
}

pub async fn run_guard(
    ctx: &GuardContext<'_>,
    doc_id: &DocumentId,
    scope: &Scope,
) -> Result<GuardOk, DocsError> {
    // 1. Get a snapshot (cache or fresh).
    let snapshot = match ctx.cache.get_fresh(ctx.session_id, doc_id) {
        Some(s) => s,
        None => {
            let s = ctx.client.get(doc_id).await?;
            ctx.cache.put(ctx.session_id, doc_id, s.clone());
            s
        }
    };

    // 2. Resolve scope on this snapshot.
    let resolved_scope = scope_resolver::resolve(scope, &snapshot)?;

    // 3. Compare known vs current revision.
    let known = ctx.revisions.get(ctx.session_id, doc_id).await?;
    let current = snapshot.revision_id.clone();
    let known = match known {
        Some(k) if k != current => k,
        // First contact or unchanged → no need to diff.
        _ => return Ok(GuardOk { snapshot, resolved_scope, soft_warnings: vec![] }),
    };

    // 4. Fetch all revisions in (known, current] and filter to non-SA.
    let revs = ctx.client.list_revisions_since(doc_id, &known).await?;
    let human_revs: Vec<_> = revs.into_iter().filter(|r| {
        match (ctx.sa_email, r.modifying_user_email.as_deref()) {
            (Some(sa), Some(user)) => user != sa,
            (_, None) => true,    // unattributable — treat as human (conservative)
            (None, _) => true,    // no SA known — treat all as human
        }
    }).collect();
    if human_revs.is_empty() {
        ctx.revisions.put(ctx.session_id, doc_id, &current).await?;
        return Ok(GuardOk { snapshot, resolved_scope, soft_warnings: vec![] });
    }

    // 5. Fetch prior + current as plain text, compute paragraph-level diff.
    let prior_text = ctx.client.get_at_revision(doc_id, &known).await?;
    let current_text = ctx.client.get_at_revision(doc_id, &current).await?;
    let human_changes = diff_to_paragraph_changes(
        &prior_text, &current_text, &human_revs
    );

    // 6. Partition by overlap with resolved_scope.
    let (overlap, outside): (Vec<_>, Vec<_>) = human_changes.into_iter()
        .partition(|h| h.paragraph >= resolved_scope.paragraph_start
                    && h.paragraph <= resolved_scope.paragraph_end);

    if !overlap.is_empty() {
        let since = human_revs.iter().map(|r| r.modified_time).min().unwrap_or_else(chrono::Utc::now);
        return Err(DocsError::HumanChangesPending {
            since,
            changes_overlapping_scope: overlap,
            changes_outside_scope: outside,
        });
    }

    Ok(GuardOk { snapshot, resolved_scope, soft_warnings: outside })
}

fn diff_to_paragraph_changes(
    prior: &str, current: &str, revs: &[crate::gdocs::domain::RevisionMeta],
) -> Vec<HumanChange>
{
    let last_rev = revs.last();
    let modified_time = last_rev.map(|r| r.modified_time).unwrap_or_else(chrono::Utc::now);
    let modifying_user = last_rev.and_then(|r| r.modifying_user_email.clone());
    let prior_paras: Vec<&str> = prior.split('\n').collect();
    let current_paras: Vec<&str> = current.split('\n').collect();
    let diff = TextDiff::from_slices(&prior_paras, &current_paras);
    let mut out = Vec::new();
    let mut current_n: u32 = 0;
    for change in diff.iter_all_changes() {
        match change.tag() {
            similar::ChangeTag::Equal  => { current_n += 1; }
            similar::ChangeTag::Insert => {
                current_n += 1;
                out.push(HumanChange {
                    kind: HumanChangeKind::Insert,
                    paragraph: current_n,
                    preview: change.value().chars().take(80).collect(),
                    modified_time, modifying_user: modifying_user.clone(),
                });
            }
            similar::ChangeTag::Delete => {
                // Deletions: attribute to the paragraph index in the
                // CURRENT doc where the deletion happened (one past
                // current_n is where the missing text used to be).
                out.push(HumanChange {
                    kind: HumanChangeKind::Delete,
                    paragraph: current_n + 1,
                    preview: change.value().chars().take(80).collect(),
                    modified_time, modifying_user: modifying_user.clone(),
                });
            }
        }
    }
    // Adjacent Insert+Delete pairs can be merged as Modify (clean noise).
    merge_adjacent_to_modify(out)
}

fn merge_adjacent_to_modify(changes: Vec<HumanChange>) -> Vec<HumanChange> {
    let mut out = Vec::with_capacity(changes.len());
    let mut iter = changes.into_iter().peekable();
    while let Some(c) = iter.next() {
        if let Some(next) = iter.peek() {
            if c.kind == HumanChangeKind::Delete
                && next.kind == HumanChangeKind::Insert
                && next.paragraph == c.paragraph
            {
                let after = iter.next().unwrap();
                out.push(HumanChange {
                    kind: HumanChangeKind::Modify,
                    paragraph: c.paragraph,
                    preview: format!("- {} → + {}", c.preview, after.preview),
                    modified_time: c.modified_time,
                    modifying_user: c.modifying_user,
                });
                continue;
            }
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    // (Add tests using MockDocsClient + InMemoryRevisionStore +
    // OutlineCache. See Task 19 for the MockDocsClient setup.)
}
```

- [ ] **Step 2: Run a placeholder cargo check**

Run: `cargo check -p colmena_dag_engine --no-default-features`
Expected: compiles. Tests deferred until MockDocsClient exists (Task 19).

- [ ] **Step 3: Commit**

```bash
git add src/libs/colmena/src/gdocs/application/co_edit_guard.rs
git commit -m "feat(gdocs): co-edit guard with paragraph-level diff"
```

---

## Tasks 16-22: Application use cases (replace_text, insert, delete_text, replace_section, style, apply_edits, named_range)

> **Pattern shared by all seven tasks** — write the use case, write its
> tests against the trait via `MockDocsClient` (introduced in Task 19),
> commit. Each task is ~150-250 LOC + ~6-10 tests.

### Task 16: `replace_text` use case

**Files:**
- Create: `src/libs/colmena/src/gdocs/application/replace_text.rs`

- [ ] **Step 1: Write the use case**

```rust
//! `replace_text` use case — content-addressed scoped find/replace.
//! Implements the AmbiguousMatch / TextNotFound / ConfirmManyMatches
//! decision tree described in spec §6.

use crate::gdocs::application::co_edit_guard::{run_guard, GuardContext};
use crate::gdocs::domain::{
    ChangeKind, ChangeRecord, DocsError, DocumentId, EditResult, MatchPreview,
    RevisionId, Scope,
};
use regex::RegexBuilder;

#[derive(Debug, Clone)]
pub struct ReplaceTextInput {
    pub find: String,
    pub replace: String,
    pub scope: Scope,
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub dry_run: bool,
    pub confirm_many: bool,
    pub occurrence: Option<u32>,         // 1-based; if set, only the Nth match
    pub anchor: Option<String>,          // additional text that must appear in same paragraph
}

pub async fn run(
    ctx: &GuardContext<'_>,
    doc_id: &DocumentId,
    input: ReplaceTextInput,
) -> Result<EditResult, DocsError> {
    let guard = run_guard(ctx, doc_id, &input.scope).await?;
    let snap = &guard.snapshot;
    let scope = &guard.resolved_scope;

    // 1. Collect matches paragraph-by-paragraph within scope.
    let pattern = if input.whole_word {
        format!(r"\b{}\b", regex::escape(&input.find))
    } else {
        regex::escape(&input.find)
    };
    let re = RegexBuilder::new(&pattern)
        .case_insensitive(!input.case_sensitive)
        .build()
        .map_err(|e| DocsError::InvalidArgs(format!("regex: {e}")))?;

    let mut hits: Vec<(u32 /* para */, u32 /* offset_in_para */, u32 /* len */)> = Vec::new();
    for tab in &snap.tabs {
        if let Some(scope_tab) = &scope.tab_id {
            if tab.tab_id.as_ref() != Some(scope_tab) { continue; }
        }
        for p in &tab.paragraphs {
            if p.n < scope.paragraph_start || p.n > scope.paragraph_end { continue; }
            // Optional anchor — paragraph must also contain it.
            if let Some(a) = &input.anchor {
                if !p.text.contains(a) { continue; }
            }
            for m in re.find_iter(&p.text) {
                hits.push((p.n, m.start() as u32, (m.end() - m.start()) as u32));
            }
        }
    }

    // 2. Apply occurrence filter.
    if let Some(n) = input.occurrence {
        if (n as usize) == 0 || (n as usize) > hits.len() {
            return Err(DocsError::TextNotFound { find: input.find, fuzzy_suggestions: vec![] });
        }
        hits = vec![hits[(n - 1) as usize]];
    }

    // 3. Match-count gates.
    if hits.is_empty() {
        return Err(DocsError::TextNotFound { find: input.find, fuzzy_suggestions: vec![] });
    }
    if hits.len() >= 5 && !input.confirm_many && input.occurrence.is_none() {
        return Err(DocsError::ConfirmManyMatches {
            find: input.find,
            count: hits.len() as u32,
            preview: build_previews(snap, &hits),
        });
    }
    if hits.len() > 1 && input.occurrence.is_none() && matches!(input.scope, Scope::Paragraph { .. }) {
        // Still allowed — paragraph scope implies "all in this paragraph".
    }

    if input.dry_run {
        return Ok(EditResult {
            changes: hits.iter().map(|(n, off, len)| {
                let text = &lookup_para(snap, *n).text;
                let before = text.chars().skip(*off as usize).take(*len as usize).collect();
                ChangeRecord {
                    kind: ChangeKind::Replace, paragraph: *n,
                    before: Some(before), after: Some(input.replace.clone()),
                    tab_id: None,
                }
            }).collect(),
            revision_id_after: snap.revision_id.clone(),
            outline_snapshot: vec![],   // dry runs don't refresh
            lossy_conversions: vec![],
            pending_human_changes_outside_scope: guard.soft_warnings,
        });
    }

    // 4. Emit batchUpdate requests (write-backwards).
    let mut requests: Vec<serde_json::Value> = Vec::new();
    let mut changes: Vec<ChangeRecord> = Vec::new();
    let mut hits_sorted = hits.clone();
    hits_sorted.sort_by(|a, b| (b.0, b.1).cmp(&(a.0, a.1)));   // desc by (para, offset)

    for (n, off, len) in &hits_sorted {
        let p = lookup_para(snap, *n);
        let start = p.start_index + *off;
        let end = start + *len;
        let before: String = p.text.chars().skip(*off as usize).take(*len as usize).collect();
        requests.push(serde_json::json!({
            "deleteContentRange": {"range": {"startIndex": start, "endIndex": end}}
        }));
        requests.push(serde_json::json!({
            "insertText": {"location": {"index": start}, "text": input.replace.clone()}
        }));
        changes.push(ChangeRecord {
            kind: ChangeKind::Replace, paragraph: *n,
            before: Some(before), after: Some(input.replace.clone()),
            tab_id: lookup_tab_id(snap, *n),
        });
    }

    let result = ctx.client.batch_update(doc_id, requests, Some(&snap.revision_id)).await?;

    // 5. Refresh cache + revision state.
    ctx.cache.invalidate(ctx.session_id, doc_id);
    ctx.revisions.put(ctx.session_id, doc_id, &result.revision_id_after).await?;
    let fresh = ctx.client.get(doc_id).await?;
    ctx.cache.put(ctx.session_id, doc_id, fresh.clone());
    let outline = build_outline(&fresh);

    Ok(EditResult {
        changes,
        revision_id_after: result.revision_id_after,
        outline_snapshot: outline,
        lossy_conversions: vec![],
        pending_human_changes_outside_scope: guard.soft_warnings,
    })
}

fn lookup_para<'a>(s: &'a crate::gdocs::domain::DocumentSnapshot, n: u32)
    -> &'a crate::gdocs::domain::ParagraphSnapshot
{
    s.tabs.iter().flat_map(|t| t.paragraphs.iter()).find(|p| p.n == n).expect("para exists")
}

fn lookup_tab_id(s: &crate::gdocs::domain::DocumentSnapshot, n: u32)
    -> Option<crate::gdocs::domain::TabId>
{
    s.tabs.iter().find(|t| t.paragraphs.iter().any(|p| p.n == n))
        .and_then(|t| t.tab_id.clone())
}

fn build_previews(snap: &crate::gdocs::domain::DocumentSnapshot,
                   hits: &[(u32, u32, u32)]) -> Vec<MatchPreview> {
    hits.iter().enumerate().map(|(i, (n, off, len))| {
        let p = lookup_para(snap, *n);
        let pre_start = (*off as i32 - 30).max(0) as usize;
        let preview: String = p.text.chars().skip(pre_start).take(80).collect();
        MatchPreview {
            n: (i + 1) as u32,
            paragraph: *n,
            preview,
        }
    }).collect()
}

fn build_outline(s: &crate::gdocs::domain::DocumentSnapshot)
    -> Vec<crate::gdocs::domain::OutlineEntry>
{
    s.tabs.iter().flat_map(|t| t.paragraphs.iter().map(|p| {
        crate::gdocs::domain::OutlineEntry {
            paragraph: p.n, tab_id: t.tab_id.clone(), kind: p.kind,
            text_preview: p.text.chars().take(80).collect(),
        }
    })).collect()
}
```

- [ ] **Step 2: Defer tests to Task 19 (MockDocsClient lives there)**

- [ ] **Step 3: Commit**

```bash
git add src/libs/colmena/src/gdocs/application/replace_text.rs
git commit -m "feat(gdocs): replace_text use case"
```

### Task 17: `insert` use case (after_text / before_text / between)

**Files:**
- Create: `src/libs/colmena/src/gdocs/application/insert.rs`

Same pattern as `replace_text`:
1. Run guard → snapshot + resolved scope.
2. Find the anchor text (re-using regex logic).
3. Compute insertion index (after_text → end_index of match; before_text → start_index; between → after the `after` heading's paragraph endIndex).
4. Convert `new_markdown` to batchUpdate requests via `markdown_to_docs_ops::markdown_to_requests` with `InsertionPoint::Index { index, tab_id }`.
5. POST batchUpdate with `requiredRevisionId`.
6. Refresh + return `EditResult`.

Three public functions: `run_insert_after_text`, `run_insert_before_text`, `run_insert_between`. Each takes its own `Input` struct.

Commit: `feat(gdocs): insert after/before/between use cases`

### Task 18: `delete_text`, `replace_section`, `append_markdown`, `style_text`, `apply_edits`, `named_range`

> Same pattern. Each ~120-200 LOC. Each gets its own file under `gdocs/application/`. Each commits independently. **For `apply_edits` specifically** — accept `Vec<ApplyEditOp>` where each variant is a `ReplaceTextInput`/`InsertAfterTextInput`/etc., run the guard ONCE for the union scope (compute as union of all sub-scopes), resolve each sub-edit to its request fragments, concatenate (preserving write-backwards order across the full compound), POST as one `batchUpdate`. Per spec §7.3: if any sub-resolution fails (AmbiguousMatch/TextNotFound/Conflict), the whole compound fails.

Commits in order:
- `feat(gdocs): delete_text use case`
- `feat(gdocs): replace_section + append_markdown use cases`
- `feat(gdocs): style_text use case`
- `feat(gdocs): apply_edits atomic compound`
- `feat(gdocs): named_range use cases (create/replace)`

---

## Task 19: `MockDocsClient` + application-layer tests

**Files:**
- Modify: `src/libs/colmena/src/gdocs/domain/traits.rs` — add a `MockDocsClient` mock (or a `tests` submodule that does)
- Create: `src/libs/colmena/src/gdocs/application/_mock.rs` (re-used by all `application/*.rs` tests)

Use the `mockall` crate (already in the workspace; see CLAUDE.md "Mocking: `mockall` crate").

- [ ] **Step 1: Add `#[cfg_attr(test, mockall::automock)]` to `DocsClient`**

In `traits.rs`:

```rust
#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait DocsClient: Send + Sync { /* unchanged */ }
```

- [ ] **Step 2: Write ~6 tests per use case**

For `replace_text` (~10 tests):
- `replace_text_unambiguous_single_match` — happy path.
- `replace_text_ambiguous_returns_error` — 2+ matches no occurrence → not actually an error in this design (it's a successful multi-replace) — verify it replaces all 2.
- `replace_text_zero_matches_returns_text_not_found`.
- `replace_text_confirm_many_when_5_plus`.
- `replace_text_confirm_many_succeeds_with_flag`.
- `replace_text_dry_run_does_not_call_batch_update`.
- `replace_text_whole_word_filters_partial_matches`.
- `replace_text_case_sensitive_filters_uppercase`.
- `replace_text_scope_under_heading_limits_range`.
- `replace_text_human_change_overlap_blocks`.

Similar test counts for other use cases.

- [ ] **Step 3: Run the application test suite**

Run: `cargo test -p colmena_dag_engine --lib gdocs::application::`
Expected: ~60 tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/gdocs/
git commit -m "test(gdocs): mockall-based application use case suite"
```

---

## Task 20: Tool dispatchers — `gdocs_tools.rs`

**Files:**
- Create: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gdocs_tools.rs`
- Modify: `…/llm_synthetic_tools/mod.rs` — add `pub mod gdocs_tools;` + re-exports
- Create: `src/libs/colmena/text/tools/gdocs.yaml`

> The dispatcher file is large (~900 LOC) but mechanical: one `Args` struct per tool, one `tool_*()` factory function, one `dispatch_*` async function, and one `dispatch_*_with_client` variant for testability. **Pattern reference:** `gsheets_tools.rs` — same shape.

- [ ] **Step 1: Read the gsheets tools dispatcher as a structural template**

Run: `head -250 src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gsheets_tools.rs`
Expected: see the `TOOL_*` consts, Args structs (`#[derive(Deserialize, JsonSchema)]`), `tool_*()` builders, dispatchers.

- [ ] **Step 2: Write the file structure (constants + 22 Args structs)**

```rust
//! Google Docs synthetic LLM tools (Subsystem G). 22 tools mapping to
//! content-addressed surgical edits. Each dispatcher builds a
//! `GoogleDocsHttpClient` + `OutlineCache` + `PostgresRevisionStore`
//! once per process, then routes to the appropriate application use case.

use crate::gdocs::application::{
    apply_edits, co_edit_guard, delete_text, insert, named_range,
    replace_section, replace_text, scope_resolver, style,
};
use crate::gdocs::domain::{
    DocsClient, DocsError, DocumentId, ExportFormat, ShareRole, Scope, StylePatch, TabId,
};
use crate::gdocs::infrastructure::config::GDocsConfig;
use crate::gdocs::infrastructure::http_client::GoogleDocsHttpClient;
use crate::gdocs::infrastructure::outline_cache::OutlineCache;
use crate::gdocs::infrastructure::revision_store::{PostgresRevisionStore, RevisionStore};
use crate::llm::domain::tools::ToolDefinition;
use crate::text;
use once_cell::sync::OnceCell;
use schemars::JsonSchema;
use serde::Deserialize;
use std::sync::Arc;

// 22 tool name constants.
pub const TOOL_CREATE:                    &str = "gdocs_create";
pub const TOOL_CREATE_FROM_MARKDOWN:      &str = "gdocs_create_from_markdown";
pub const TOOL_CREATE_FROM_DOCX:          &str = "gdocs_create_from_docx";
pub const TOOL_SHARE:                     &str = "gdocs_share";
pub const TOOL_EXPORT:                    &str = "gdocs_export";
pub const TOOL_LIST_TABS:                 &str = "gdocs_list_tabs";
pub const TOOL_ADD_TAB:                   &str = "gdocs_add_tab";
pub const TOOL_READ_AS_MARKDOWN:          &str = "gdocs_read_as_markdown";
pub const TOOL_READ_OUTLINE:              &str = "gdocs_read_outline";
pub const TOOL_LIST_NAMED_RANGES:         &str = "gdocs_list_named_ranges";
pub const TOOL_REPLACE_TEXT:              &str = "gdocs_replace_text";
pub const TOOL_INSERT_AFTER_TEXT:         &str = "gdocs_insert_after_text";
pub const TOOL_INSERT_BEFORE_TEXT:        &str = "gdocs_insert_before_text";
pub const TOOL_INSERT_BETWEEN:            &str = "gdocs_insert_between";
pub const TOOL_DELETE_TEXT:               &str = "gdocs_delete_text";
pub const TOOL_REPLACE_SECTION:           &str = "gdocs_replace_section";
pub const TOOL_APPEND_MARKDOWN:           &str = "gdocs_append_markdown";
pub const TOOL_APPLY_EDITS:               &str = "gdocs_apply_edits";
pub const TOOL_STYLE_TEXT:                &str = "gdocs_style_text";
pub const TOOL_CREATE_NAMED_RANGE:        &str = "gdocs_create_named_range";
pub const TOOL_REPLACE_NAMED_RANGE:       &str = "gdocs_replace_named_range";
pub const TOOL_ACKNOWLEDGE_HUMAN_CHANGES: &str = "gdocs_acknowledge_human_changes";

// One Args struct per tool. Example:

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CreateArgs {
    pub title: String,
    pub parent_folder_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReplaceTextArgs {
    pub doc_id: String,
    pub find: String,
    pub replace: String,
    #[serde(default)] pub scope: Option<serde_json::Value>,     // parsed to Scope
    #[serde(default)] pub case_sensitive: bool,
    #[serde(default)] pub whole_word: bool,
    #[serde(default)] pub dry_run: bool,
    #[serde(default)] pub confirm_many: bool,
    pub occurrence: Option<u32>,
    pub anchor: Option<String>,
    pub mode: Option<String>,                                     // "direct" only in v1
}

// … 20 more Args structs (one per tool — see spec §6 for fields).
```

- [ ] **Step 3: Write a shared `build_session_ctx` helper + `error_to_json` mapper**

```rust
// Lazy singletons.
static CLIENT: OnceCell<Arc<GoogleDocsHttpClient>> = OnceCell::new();
static CACHE:  OnceCell<Arc<OutlineCache>> = OnceCell::new();
static REVS:   OnceCell<Arc<dyn RevisionStore>> = OnceCell::new();

fn shared_client() -> Result<Arc<GoogleDocsHttpClient>, serde_json::Value> {
    CLIENT.get_or_try_init(|| {
        let cfg = GDocsConfig::from_env();
        GoogleDocsHttpClient::from_config(&cfg)
            .map(Arc::new)
            .map_err(error_to_json)
    }).cloned().map_err(|v| v.clone())
}

fn shared_cache() -> Arc<OutlineCache> {
    CACHE.get_or_init(|| {
        let cfg = GDocsConfig::from_env();
        Arc::new(OutlineCache::new(cfg.revision_cache_ttl))
    }).clone()
}

fn shared_revs() -> Result<Arc<dyn RevisionStore>, serde_json::Value> {
    REVS.get_or_try_init(|| {
        let url = std::env::var("DATABASE_URL").map_err(|_| serde_json::json!({
            "error": "gdocs_not_configured",
            "message": "DATABASE_URL required for revision tracking"
        }))?;
        // Pool init — see how `crdt_doc_run_python` builds its pool for the exact code.
        let pool = futures::executor::block_on(sqlx::PgPool::connect(&url))
            .map_err(|e| serde_json::json!({"error": "internal", "message": e.to_string()}))?;
        Ok::<Arc<dyn RevisionStore>, serde_json::Value>(
            Arc::new(PostgresRevisionStore::new(pool))
        )
    }).cloned().map_err(|v: serde_json::Value| v.clone())
}

pub(crate) fn error_to_json(e: DocsError) -> serde_json::Value {
    use DocsError::*;
    match &e {
        AmbiguousMatch { find, matches } => serde_json::json!({
            "error": "ambiguous_match", "find": find, "matches": matches,
            "valid_next_moves": [
                {"action": "specify_occurrence",
                 "example": format!("gdocs_replace_text(..., occurrence: 1)")},
                {"action": "use_anchor",
                 "example": format!("gdocs_replace_text(..., anchor: 'preceded text…')")},
            ]
        }),
        TextNotFound { find, fuzzy_suggestions } => serde_json::json!({
            "error": "text_not_found", "find": find,
            "fuzzy_suggestions": fuzzy_suggestions,
        }),
        ConfirmManyMatches { find, count, preview } => serde_json::json!({
            "error": "confirm_many_matches", "find": find, "count": count, "preview": preview,
            "valid_next_moves": [
                {"action": "confirm",
                 "example": "<re-call with `confirm_many: true`>"},
                {"action": "narrow_scope",
                 "example": "<re-call with a more specific scope>"},
                {"action": "dry_run",
                 "example": "<re-call with `dry_run: true` to preview>"}
            ]
        }),
        HumanChangesPending { since, changes_overlapping_scope, changes_outside_scope } =>
            serde_json::json!({
                "error": "human_changes_pending",
                "since": since,
                "changes_overlapping_scope": changes_overlapping_scope,
                "changes_outside_scope": changes_outside_scope,
                "valid_next_moves": [
                    {"action": "acknowledge",
                     "example": "gdocs_acknowledge_human_changes(doc_id) — then retry the edit"},
                ]
            }),
        NotConfigured(m) => serde_json::json!({"error": "gdocs_not_configured", "hint": m}),
        AuthFailed(m)    => serde_json::json!({"error": "auth_failed", "message": m}),
        DocumentNotFound(d) => serde_json::json!({"error": "document_not_found", "doc_id": d}),
        TabNotFound(t)   => serde_json::json!({"error": "tab_not_found", "tab_id": t}),
        PermissionDenied(sa) => serde_json::json!({
            "error": "permission_denied",
            "hint": format!("share the doc with {sa}")
        }),
        NoParentFolder => serde_json::json!({
            "error": "no_parent_folder_configured",
            "hint": "set COLMENA_GDOCS_DEFAULT_PARENT_FOLDER_ID or pass parent_folder_id"
        }),
        ScopeCrossesBoundary(m) => serde_json::json!({"error": "scope_crosses_boundary", "message": m}),
        TabExists(t)     => serde_json::json!({"error": "tab_exists", "title": t}),
        RateLimit(s)     => serde_json::json!({"error": "rate_limit", "retry_after_seconds": s}),
        Conflict         => serde_json::json!({"error": "conflict"}),
        Http(m)          => serde_json::json!({"error": "http_error", "message": m}),
        InvalidArgs(m)   => serde_json::json!({"error": "invalid_args", "message": m}),
        Internal(m)      => serde_json::json!({"error": "internal", "message": m}),
    }
}
```

- [ ] **Step 4: Write `tool_*()` factories and `dispatch_*` async functions for all 22 tools**

For each tool, follow this exact pattern (example for `replace_text`):

```rust
pub fn tool_replace_text() -> ToolDefinition {
    super::build_synthetic_tool_with_summary::<ReplaceTextArgs>(
        TOOL_REPLACE_TEXT,
        text::tool_description(TOOL_REPLACE_TEXT),
        text::tool_summary(TOOL_REPLACE_TEXT),
    )
}

pub async fn dispatch_replace_text(
    args: serde_json::Value,
    session_id: &str,
) -> serde_json::Value {
    let parsed: ReplaceTextArgs = match serde_json::from_value(args) {
        Ok(a) => a,
        Err(e) => return serde_json::json!({"error": "invalid_args", "message": e.to_string()}),
    };
    let client = match shared_client() { Ok(c) => c, Err(e) => return e };
    let cache = shared_cache();
    let revs = match shared_revs() { Ok(r) => r, Err(e) => return e };
    let ctx = co_edit_guard::GuardContext {
        client: client.as_ref(), cache: cache.as_ref(),
        revisions: revs.as_ref(), session_id,
        sa_email: std::env::var("COLMENA_GDOCS_SA_EMAIL").ok().as_deref(),
    };
    let scope = parse_scope(parsed.scope).unwrap_or_default();
    let input = replace_text::ReplaceTextInput {
        find: parsed.find, replace: parsed.replace, scope,
        case_sensitive: parsed.case_sensitive, whole_word: parsed.whole_word,
        dry_run: parsed.dry_run, confirm_many: parsed.confirm_many,
        occurrence: parsed.occurrence, anchor: parsed.anchor,
    };
    let doc_id = DocumentId(parsed.doc_id);
    match replace_text::run(&ctx, &doc_id, input).await {
        Ok(r) => serde_json::json!({
            "ok": true,
            "changes": r.changes,
            "revision_id_after": r.revision_id_after.0,
            "outline_snapshot": r.outline_snapshot,
            "lossy_conversions": r.lossy_conversions,
            "pending_human_changes_outside_scope": r.pending_human_changes_outside_scope,
        }),
        Err(e) => error_to_json(e),
    }
}

fn parse_scope(v: Option<serde_json::Value>) -> Option<Scope> {
    v.and_then(|j| serde_json::from_value(j).ok())
}
```

> Repeat for every tool. The `dispatch_acknowledge_human_changes` is trivial (just calls `client.get()` + `revisions.put()` + returns the new revision). The `dispatch_apply_edits` is the most complex (parses the nested `edits` array into typed `ApplyEditOp` variants).

- [ ] **Step 5: Write `text/tools/gdocs.yaml` with 22 description/summary entries**

Mirror `text/tools/gsheets.yaml` style. Each tool gets:

```yaml
gdocs_replace_text:
  summary: Replace text content-addressed in a Google Doc, scoped optionally to a heading or paragraph
  description: |
    Find every occurrence of `find` in the doc (or within a scope: `all`,
    `{kind:"paragraph", n:N}`, `{kind:"under_heading", heading:"## X"}`,
    `{kind:"between_headings", after:"## A", before:"## B"}`,
    `{kind:"tab", tab_id:"…"}`) and replace it with `replace`. The LLM
    never computes UTF-16 indices — they're resolved by colmena.

    Options:
    - `case_sensitive: bool` (default false)
    - `whole_word: bool` (default false; matches `\bfind\b`)
    - `dry_run: bool` — preview without writing
    - `confirm_many: bool` — required when ≥5 matches
    - `occurrence: int` — only the Nth match (1-based)
    - `anchor: str` — additional text that must appear in the same paragraph
    - `mode: "direct"` — v1 only

    Errors the agent should handle:
    - `confirm_many_matches` (≥5 matches) — re-call with `confirm_many: true`
    - `text_not_found`
    - `human_changes_pending` — call `gdocs_acknowledge_human_changes` first
```

(Continue for all 22 tools.)

- [ ] **Step 6: Modify `llm_synthetic_tools/mod.rs`**

Add (alphabetical):

```rust
pub mod gdocs_tools;
pub mod markdown_to_docs_ops;

pub use gdocs_tools::{
    // tool factories
    tool_create as gdocs_tool_create,
    tool_create_from_markdown as gdocs_tool_create_from_markdown,
    // … (22 total) …
    // dispatchers
    dispatch_create as dispatch_gdocs_create,
    dispatch_create_from_markdown as dispatch_gdocs_create_from_markdown,
    // … (22 total) …
};
```

Also add `gdocs_tools` factories to the `all_synthetic_tools()` function so the registry coherence tests pick them up.

- [ ] **Step 7: Run a sanity build**

Run: `cargo build -p colmena_dag_engine --no-default-features`
Expected: compiles. (No live tests for dispatchers yet — those run via the integration test in Task 22.)

- [ ] **Step 8: Run all gdocs tests**

Run: `cargo test -p colmena_dag_engine --lib gdocs:: && cargo test -p colmena_dag_engine --lib markdown_to_docs_ops::`
Expected: all green.

- [ ] **Step 9: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gdocs_tools.rs \
        src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs \
        src/libs/colmena/text/tools/gdocs.yaml
git commit -m "feat(gdocs): 22 tool dispatchers + text registry entries"
```

---

## Task 21: Toolkit alias + dispatch routing

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/toolkit_packages.rs`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs`

- [ ] **Step 1: Add `gdocs` + `gdocs_readonly` aliases**

In `toolkit_packages.rs`, add (after the `gsheets` entry):

```rust
ToolkitPackage {
    alias: "gdocs",
    tools: &[
        "gdocs_create", "gdocs_create_from_markdown", "gdocs_create_from_docx",
        "gdocs_share", "gdocs_export",
        "gdocs_list_tabs", "gdocs_add_tab",
        "gdocs_read_as_markdown", "gdocs_read_outline", "gdocs_list_named_ranges",
        "gdocs_replace_text", "gdocs_insert_after_text", "gdocs_insert_before_text",
        "gdocs_insert_between", "gdocs_delete_text",
        "gdocs_replace_section", "gdocs_append_markdown", "gdocs_apply_edits",
        "gdocs_style_text",
        "gdocs_create_named_range", "gdocs_replace_named_range",
        "gdocs_acknowledge_human_changes",
    ],
},
ToolkitPackage {
    alias: "gdocs_readonly",
    tools: &[
        "gdocs_export",
        "gdocs_list_tabs",
        "gdocs_read_as_markdown", "gdocs_read_outline", "gdocs_list_named_ranges",
    ],
},
```

- [ ] **Step 2: Add the registry assertion test**

In the same file, add an assert mirroring `gsheets_package_has_all_ten_tools`:

```rust
#[test]
fn gdocs_package_has_all_twenty_two_tools() {
    let pkg = find_package("gdocs").expect("gdocs package");
    assert_eq!(pkg.tools.len(), 22);
}
#[test]
fn gdocs_readonly_package_has_five_tools() {
    let pkg = find_package("gdocs_readonly").expect("gdocs_readonly package");
    assert_eq!(pkg.tools.len(), 5);
}
```

- [ ] **Step 3: Add dispatch arms in `dag_tool_executor.rs`**

Find the existing gsheets dispatch block and add an analogous gdocs block. The pattern (rough sketch — read the existing block to match style exactly):

```rust
TOOL_GDOCS_CREATE => dispatch_gdocs_create(args, session_id).await,
TOOL_GDOCS_CREATE_FROM_MARKDOWN => dispatch_gdocs_create_from_markdown(args, session_id).await,
// … (22 arms total) …
```

> The exact pattern depends on whether the executor uses a `match` on the tool name string or a registry lookup. Read 200 lines of context first. If unsure, find the gsheets routing and replicate.

- [ ] **Step 4: Run all tests**

Run: `cargo test -p colmena_dag_engine --lib`
Expected: all green, including new package assertion tests.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/toolkit_packages.rs \
        src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs
git commit -m "feat(gdocs): toolkit aliases + executor dispatch routing"
```

---

## Task 22: Integration tests + smoke graph

**Files:**
- Create: `src/libs/colmena/tests/gdocs_integration_test.rs`
- Create: `tests/graphs/agents/gdocs_smoke.json`

- [ ] **Step 1: Write the integration test scaffolding**

```rust
//! End-to-end integration tests against the real Google APIs.
//! Skipped in CI; run locally with:
//!   source .env && cargo test --test gdocs_integration_test -- --ignored

#[tokio::test]
#[ignore = "requires GOOGLE_APPLICATION_CREDENTIALS + COLMENA_GDOCS_TEST_PARENT_FOLDER_ID + DATABASE_URL"]
async fn create_replace_export_roundtrip() {
    // 1. Create from markdown.
    // 2. read_outline → assert paragraph count.
    // 3. replace_text(find: 'X', replace: 'Y') → assert change.
    // 4. export(markdown) → assert Y in result.
    // 5. (Cleanup: delete via Drive API — optional.)
}

// (Add 9 more scenarios per spec §11.2.)
```

> 10 scenarios total — see spec §11.2 for the list.

- [ ] **Step 2: Write the smoke graph**

`tests/graphs/agents/gdocs_smoke.json` — DAG with one agent + the `gdocs` toolkit enabled. The agent's instructions reference the 7-step scenario in spec §11.3.

> Pattern reference: `tests/graphs/agents/gsheets_smoke.json` if it exists, or any other agent graph using a toolkit alias.

- [ ] **Step 3: Run integration tests locally (one-shot)**

Run:
```bash
source .env
cargo test --test gdocs_integration_test -- --ignored
```
Expected: 10 scenarios pass. If any fail, fix the underlying use case (NOT the test) and re-run.

- [ ] **Step 4: Run the smoke graph**

Run:
```bash
source .env
cargo run --bin dag_engine -- run tests/graphs/agents/gdocs_smoke.json \
    --agent-session-id smoke_gdocs_001 2>&1 | tee /tmp/colmena_e2e/gdocs_smoke.sse
```
Expected: the agent completes all 7 steps, final report mentions the created doc URL.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/tests/gdocs_integration_test.rs \
        tests/graphs/agents/gdocs_smoke.json
git commit -m "test(gdocs): integration tests + smoke graph"
```

---

## Task 23: Docs

**Files:**
- Create: `docs/developer_guide/44_gdocs.md`
- Modify: `docs/node_as_tools_reference.json` — 22 entries
- Modify: `docs/BACKLOG.md` — "Subsystem G v1.1" section
- Modify: `docs/CHANGELOG_2026-06.md` — G entry
- Modify: `CLAUDE.md` — "Current Status" bullet + mention of `gdocs` toolkit

- [ ] **Step 1: Write `docs/developer_guide/44_gdocs.md`**

Mirror `docs/developer_guide/39_gsheets.md` structure: opening blurb, recommended activation, tool surface table, auth, formulas-equivalent (here: content-addressed edits), co-edit safety section, markdown conversion section, hexagonal layout pointer, out of scope, spec+plan links.

- [ ] **Step 2: Add 22 entries to `docs/node_as_tools_reference.json`**

For each of the 22 tools, add an entry under `synthetic_tools` (or wherever gsheets's entries live — find them first with `grep -n gsheets_read docs/node_as_tools_reference.json`). Each entry needs at minimum: name, summary, args schema with field types/defaults/descriptions, returns shape, common-mistakes notes.

- [ ] **Step 3: Add v1.1 section to `docs/BACKLOG.md`**

```markdown
## Subsystem G v1.1 — Google Docs

- `mode: "suggest"` writeControl.suggestionsEnabled (param accepted in v1).
- Surgical table-cell edits (`gdocs_set_table_cell`, `gdocs_insert_table_row`).
- Drive Comments API for human↔agent in-doc messaging.
- `gdocs_acknowledge_human_changes` enriched mode (cheap-tier LLM summary +
  conflict warnings).
- `gdocs_insert_image_after_text` (attachment_id + URL flavors).
- `gdocs_list_documents` (folder-scoped Drive discovery).
- OAuth user-scoped auth flow (currently SA + ADC only).
- Apps Script execution.
- Drive Revisions restore (rollback).
```

- [ ] **Step 4: Add G entry to `docs/CHANGELOG_2026-06.md`**

```markdown
## §N — Google Docs integration shipped 2026-MM-DD (Subsystem G)

22 synthetic LLM tools mirroring the content-addressed edit model — the
agent specifies WHAT to change, never UTF-16 indices. Multi-tab, markdown
import/export, co-edit safety via Drive Revisions diff stored in
`gdocs_session_state(agent_session_id, document_id, last_revision_id)`.

See [`docs/developer_guide/44_gdocs.md`](developer_guide/44_gdocs.md) and
[`docs/superpowers/specs/2026-06-08-google-docs-design.md`](superpowers/specs/2026-06-08-google-docs-design.md).

Breaking changes: none (purely additive — new tools, new postgres table,
no rename/removal).

ADP impact: zero breaking changes. ADP can opt in by enabling the
`gdocs` toolkit and providing `GOOGLE_APPLICATION_CREDENTIALS` +
`COLMENA_GDOCS_DEFAULT_PARENT_FOLDER_ID` in its worker env.
```

- [ ] **Step 5: Update `CLAUDE.md` "Current Status" section**

Add a bullet near the other shipped subsystems:

```markdown
- **Google Docs integration shipped 2026-MM-DD** — 22 synthetic tools
  (`gdocs_*`) with content-addressed surgical edits, multi-tab,
  markdown import/export, co-edit safety. See
  [`docs/developer_guide/44_gdocs.md`](docs/developer_guide/44_gdocs.md).
```

- [ ] **Step 6: Run the registry coherence tests**

Run: `cargo test -p colmena_dag_engine --lib every_registered_tool_has_text_entry index_doc_covers_all_registered_tools`
Expected: pass — every gdocs tool has both a text entry AND a docs entry.

- [ ] **Step 7: Commit**

```bash
git add docs/developer_guide/44_gdocs.md \
        docs/node_as_tools_reference.json \
        docs/BACKLOG.md \
        docs/CHANGELOG_2026-06.md \
        CLAUDE.md
git commit -m "docs(gdocs): developer guide + node reference + backlog + changelog"
```

---

## Task 24: Final sweep

- [ ] **Step 1: Format**

Run: `cargo fmt --all`

- [ ] **Step 2: Clippy**

Run: `cargo clippy -p colmena_dag_engine --all-targets -- -D warnings`
Expected: zero warnings.

- [ ] **Step 3: Full test pass with `--verbose` (CI parity)**

Run: `cargo test -p colmena_dag_engine --verbose`
Expected: all unit + integration + doctests green.

- [ ] **Step 4: Final smoke against real APIs**

Run:
```bash
source .env
cargo run --bin dag_engine -- run tests/graphs/agents/gdocs_smoke.json \
    --agent-session-id gdocs_final_smoke 2>&1 \
    | tee /tmp/colmena_e2e/gdocs_final_smoke.sse
```
Save the SSE + a friendly summary report (per CLAUDE.md "Graph runs → save to /tmp + friendly report").

- [ ] **Step 5: Verify ADP worker still compiles against this colmena**

```bash
cd /Users/danielgarcia/startti/adp/apps/service/ia/platform/worker
cargo check 2>&1 | tail -20
```
Expected: clean (this subsystem is additive — should not affect ADP).

- [ ] **Step 6: Commit any final touch-ups**

```bash
git status
# If anything stray: git add . && git commit -m "chore(gdocs): final sweep"
```

- [ ] **Step 7: Verify spec ↔ plan ↔ implementation alignment**

Open the spec (`docs/superpowers/specs/2026-06-08-google-docs-design.md`) and the plan side by side. Confirm every section in spec §6 (the 22 tools) is implemented in a dispatcher AND has a text entry AND has a docs entry. Confirm spec §8 (co-edit pipeline) is reflected in `co_edit_guard.rs`. Confirm spec §10 (markdown conversion) is reflected in `markdown_to_docs_ops.rs` with golden fixtures.

- [ ] **Step 8: Done**

Subsystem G shipped. ~5500 LOC + tests + docs across 24 tasks.

---

## Self-review notes (post-write)

- **Spec coverage:** every section §1-§15 in the spec maps to at least one task in this plan. §1 (problem)/§2 (goals) → motivation, no task; §3 (architecture) → Tasks 1-9 (module skeleton + types/errors/traits + auth/config/cache/store/http); §4 (components) → matches Tasks 2-12; §5 (auth) → Task 6; §6 (tool surface) → Tasks 20-21; §7 (data flows) → walked through by integration tests (Task 22) + smoke graph; §8 (co-edit pipeline) → Task 15; §9 (errors) → Tasks 3 + 20; §10 (markdown conversion) → Task 13; §11 (testing) → Tasks 19, 22, 24; §12 (performance) → empirically validated by Task 22 smoke; §13 (migration) → Task 7 covers the postgres migration; §14 (v1.1 backlog) → Task 23; §15 (impl tasks G-T1..G-T24) → 1-to-1 with this plan's Tasks 1-24.
- **Placeholder scan:** no `TODO`/`TBD`/`FIXME` strings in this plan. `unimplemented!()` and `todo!()` are deliberate — they're filled by named subsequent tasks.
- **Type consistency:** `ReplaceTextInput` defined in Task 16, used in Task 20 dispatcher. `GuardContext` defined in Task 15, used in Tasks 16-18, 20. `Scope` defined in Task 2 (types.rs), parsed in dispatcher (Task 20), resolved in scope_resolver (Task 14), consumed by use cases (Tasks 16-18).
