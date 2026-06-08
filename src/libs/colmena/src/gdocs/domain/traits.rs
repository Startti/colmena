//! Port for any backend that can read/write Google Docs.
//! `infrastructure::GoogleDocsHttpClient` is the production impl;
//! tests use `MockDocsClient` (mockall::automock — enabled in Task 19).

use crate::gdocs::domain::{
    BatchUpdateResult, CreateFromMarkdownResult, DocsError, DocumentId, DocumentMeta,
    DocumentSnapshot, ExportFormat, NamedRangeMeta, OutlineEntry, RevisionId, RevisionMeta,
    Scope, ShareRole, TabId, TabMeta,
};
use async_trait::async_trait;

/// Hexagonal port — every method the application layer needs from the
/// Google Docs + Drive REST surface. Implementations live in the
/// infrastructure layer.
#[async_trait]
pub trait DocsClient: Send + Sync {
    // ── Lifecycle ────────────────────────────────────────────────

    /// Create a blank doc with the given `title`. If `parent_folder` is
    /// `Some`, Drive places the new file inside it (the operator's
    /// configured "shared folder" pattern).
    async fn create(&self, title: &str, parent_folder: Option<&str>)
        -> Result<DocumentMeta, DocsError>;

    /// Create a doc from a markdown string. Drive performs the
    /// conversion natively. The implementation MAY re-export the doc
    /// back to markdown to detect lossy elements (see
    /// `CreateFromMarkdownResult::lossy_conversions`).
    async fn create_from_markdown(
        &self, title: &str, md: &str, parent_folder: Option<&str>,
    ) -> Result<CreateFromMarkdownResult, DocsError>;

    /// Upload an `.docx` byte buffer and have Drive convert it to a
    /// native Google Doc.
    async fn create_from_docx(
        &self, title: &str, bytes: Vec<u8>, parent_folder: Option<&str>,
    ) -> Result<DocumentMeta, DocsError>;

    /// Grant access to `email` with the requested `role`.
    async fn share(&self, id: &DocumentId, email: &str, role: ShareRole)
        -> Result<(), DocsError>;

    /// Export the doc as bytes in the requested format. The dispatcher
    /// layer wraps the result as a colmena attachment.
    async fn export(&self, id: &DocumentId, format: ExportFormat)
        -> Result<Vec<u8>, DocsError>;

    // ── Reads ────────────────────────────────────────────────────

    /// Fetch a full document tree as a `DocumentSnapshot`. Always
    /// includes tab content (`includeTabsContent=true`).
    async fn get(&self, id: &DocumentId) -> Result<DocumentSnapshot, DocsError>;

    /// Drive `files.export` to `text/markdown`. When `tab_id` is set,
    /// future versions may slice; v1 returns the full export.
    async fn read_as_markdown(&self, id: &DocumentId, tab_id: Option<&TabId>)
        -> Result<String, DocsError>;

    /// Headings + first ~80 chars per paragraph. Built from `get` so a
    /// fresh snapshot is fetched; pair with the outline cache for
    /// burst-friendly UX.
    async fn read_outline(&self, id: &DocumentId, tab_id: Option<&TabId>)
        -> Result<Vec<OutlineEntry>, DocsError>;

    /// All `namedRanges` declared in the doc.
    async fn list_named_ranges(&self, id: &DocumentId)
        -> Result<Vec<NamedRangeMeta>, DocsError>;

    /// All tabs (including nested `childTabs`).
    async fn list_tabs(&self, id: &DocumentId)
        -> Result<Vec<TabMeta>, DocsError>;

    /// Drive `revisions.list` filtered to entries AFTER `since`.
    /// Used by the co-edit guard.
    async fn list_revisions_since(&self, id: &DocumentId, since: &RevisionId)
        -> Result<Vec<RevisionMeta>, DocsError>;

    /// Fetch a historical snapshot as plain text — used by the co-edit
    /// guard to compute paragraph-level diffs.
    async fn get_at_revision(&self, id: &DocumentId, revision: &RevisionId)
        -> Result<String, DocsError>;

    // ── batchUpdate ──────────────────────────────────────────────

    /// Apply a sequence of `Request` JSON objects in one atomic call.
    /// `required_revision` triggers optimistic concurrency.
    async fn batch_update(
        &self, id: &DocumentId, requests: Vec<serde_json::Value>,
        required_revision: Option<&RevisionId>,
    ) -> Result<BatchUpdateResult, DocsError>;

    // ── Tabs ─────────────────────────────────────────────────────

    /// Create a new tab. `after_tab` chooses placement; `None` appends.
    /// Returns `DocsError::TabExists` when a tab with that title already
    /// exists and the operator's policy is `fail` (the default).
    async fn add_tab(&self, id: &DocumentId, title: &str, after_tab: Option<&TabId>)
        -> Result<TabMeta, DocsError>;

    // ── Named ranges ─────────────────────────────────────────────

    /// Create a named range scoped to a paragraph or a found anchor.
    /// v1 supports `Scope::Paragraph` directly; other scopes must be
    /// resolved to a paragraph by the application layer first.
    async fn create_named_range(
        &self, id: &DocumentId, name: &str, scope: Scope,
    ) -> Result<NamedRangeMeta, DocsError>;
}
