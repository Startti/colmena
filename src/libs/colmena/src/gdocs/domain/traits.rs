//! Port for any backend that can read/write Google Docs.
//! `infrastructure::GoogleDocsHttpClient` is the production impl;
//! tests use `MockDocsClient` (mockall::automock — enabled in Task 19).

use crate::gdocs::domain::{
    BatchUpdateResult, CreateFromMarkdownResult, DocsError, DocumentId, DocumentMeta,
    DocumentSnapshot, ExportFormat, NamedRangeMeta, OutlineEntry, RevisionId, RevisionMeta, Scope,
    ShareRole, TabId, TabMeta,
};
use async_trait::async_trait;

/// Hexagonal port — every method the application layer needs from the
/// Google Docs + Drive REST surface. Implementations live in the
/// infrastructure layer.
///
/// Note on lifetimes: `Option<&Foo>` parameters carry explicit `'a`
/// lifetimes so that `mockall::automock` (gated by `#[cfg(test)]`) can
/// generate a working `MockDocsClient`. Without them mockall fails to
/// elide the borrow inside the `async_trait` desugaring.
#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait DocsClient: Send + Sync {
    // ── Lifecycle ────────────────────────────────────────────────

    /// Create a blank doc with the given `title`. If `parent_folder` is
    /// `Some`, Drive places the new file inside it (the operator's
    /// configured "shared folder" pattern).
    async fn create<'a>(
        &self,
        title: &str,
        parent_folder: Option<&'a str>,
    ) -> Result<DocumentMeta, DocsError>;

    /// Create a doc from a markdown string. Drive performs the
    /// conversion natively. The implementation MAY re-export the doc
    /// back to markdown to detect lossy elements (see
    /// `CreateFromMarkdownResult::lossy_conversions`).
    async fn create_from_markdown<'a>(
        &self,
        title: &str,
        md: &str,
        parent_folder: Option<&'a str>,
    ) -> Result<CreateFromMarkdownResult, DocsError>;

    /// Upload an `.docx` byte buffer and have Drive convert it to a
    /// native Google Doc.
    async fn create_from_docx<'a>(
        &self,
        title: &str,
        bytes: Vec<u8>,
        parent_folder: Option<&'a str>,
    ) -> Result<DocumentMeta, DocsError>;

    /// Grant access to `email` with the requested `role`.
    async fn share(&self, id: &DocumentId, email: &str, role: ShareRole) -> Result<(), DocsError>;

    /// Bundle 2B (2026-06-11): list every Drive permission on a doc.
    /// Used by `gdocs_list_permissions` so the LLM can show "who has
    /// access" before sharing or revoking.
    async fn list_permissions(
        &self,
        id: &DocumentId,
    ) -> Result<crate::gdocs::domain::types::PermissionList, DocsError>;

    /// Revoke a permission. The `permission_id` comes from
    /// `list_permissions` (Drive's stable id; NOT the email). Bundle 2B.
    async fn delete_permission(
        &self,
        id: &DocumentId,
        permission_id: &str,
    ) -> Result<(), DocsError>;

    /// Drive discovery — return the documents the OAuth user can see,
    /// filtered by `filter`. Used by `gdocs_list_documents`.
    ///
    /// Implementation hits `files.list?q=mimeType='application/vnd.google-apps.document'`
    /// with the filter fragments appended via `and`. The OAuth user-scoped
    /// account (`agents@startti.co` in the canonical deploy) only sees
    /// documents shared with it OR owned by it — there is no Drive-wide
    /// visibility.
    async fn list_documents<'a>(
        &self,
        filter: &crate::gdocs::domain::types::DocumentListFilter<'a>,
    ) -> Result<crate::gdocs::domain::types::DocumentListResult, DocsError>;

    /// Export the doc as bytes in the requested format. The dispatcher
    /// layer wraps the result as a colmena attachment.
    async fn export(&self, id: &DocumentId, format: ExportFormat) -> Result<Vec<u8>, DocsError>;

    /// Bundle 4A (2026-06-11): post a new Drive comment on the doc.
    /// `anchor` is the opaque JSON Google uses to pin to a text range;
    /// pass `None` for a doc-wide comment.
    async fn add_comment<'a>(
        &self,
        id: &DocumentId,
        content: &str,
        anchor: Option<&'a str>,
    ) -> Result<crate::gdocs::domain::types::CommentEntry, DocsError>;

    /// Bundle 4A (2026-06-11): list Drive comments on the doc.
    async fn list_comments<'a>(
        &self,
        id: &DocumentId,
        filter: &crate::gdocs::domain::types::CommentListFilter<'a>,
    ) -> Result<crate::gdocs::domain::types::CommentList, DocsError>;

    /// Bundle 4A (2026-06-11): mark a comment resolved by posting a reply
    /// with `action: "resolve"`. Drive flips the parent comment's
    /// `resolved` flag to `true`. `content` is the optional message
    /// attached to the resolution reply.
    async fn resolve_comment<'a>(
        &self,
        id: &DocumentId,
        comment_id: &str,
        content: Option<&'a str>,
    ) -> Result<(), DocsError>;

    // ── Reads ────────────────────────────────────────────────────

    /// Fetch a full document tree as a `DocumentSnapshot`. Always
    /// includes tab content (`includeTabsContent=true`).
    async fn get(&self, id: &DocumentId) -> Result<DocumentSnapshot, DocsError>;

    /// Drive `files.export` to `text/markdown`. When `tab_id` is set,
    /// future versions may slice; v1 returns the full export.
    async fn read_as_markdown<'a>(
        &self,
        id: &DocumentId,
        tab_id: Option<&'a TabId>,
    ) -> Result<String, DocsError>;

    /// Headings + first ~80 chars per paragraph. Built from `get` so a
    /// fresh snapshot is fetched; pair with the outline cache for
    /// burst-friendly UX.
    async fn read_outline<'a>(
        &self,
        id: &DocumentId,
        tab_id: Option<&'a TabId>,
    ) -> Result<Vec<OutlineEntry>, DocsError>;

    /// All `namedRanges` declared in the doc.
    async fn list_named_ranges(&self, id: &DocumentId) -> Result<Vec<NamedRangeMeta>, DocsError>;

    /// All tabs (including nested `childTabs`).
    async fn list_tabs(&self, id: &DocumentId) -> Result<Vec<TabMeta>, DocsError>;

    /// Drive `revisions.list` filtered to entries AFTER `since`.
    /// Used by the co-edit guard.
    async fn list_revisions_since(
        &self,
        id: &DocumentId,
        since: &RevisionId,
    ) -> Result<Vec<RevisionMeta>, DocsError>;

    /// Fetch a historical snapshot as plain text — used by the co-edit
    /// guard to compute paragraph-level diffs.
    async fn get_at_revision(
        &self,
        id: &DocumentId,
        revision: &RevisionId,
    ) -> Result<String, DocsError>;

    // ── batchUpdate ──────────────────────────────────────────────

    /// Apply a sequence of `Request` JSON objects in one atomic call.
    /// `required_revision` triggers optimistic concurrency.
    async fn batch_update<'a>(
        &self,
        id: &DocumentId,
        requests: Vec<serde_json::Value>,
        required_revision: Option<&'a RevisionId>,
    ) -> Result<BatchUpdateResult, DocsError>;

    // ── Tabs ─────────────────────────────────────────────────────

    /// Create a new tab. `after_tab` chooses placement; `None` appends.
    /// Returns `DocsError::TabExists` when a tab with that title already
    /// exists and the operator's policy is `fail` (the default).
    async fn add_tab<'a>(
        &self,
        id: &DocumentId,
        title: &str,
        after_tab: Option<&'a TabId>,
    ) -> Result<TabMeta, DocsError>;

    // ── Named ranges ─────────────────────────────────────────────

    /// Create a named range scoped to a paragraph or a found anchor.
    /// v1 supports `Scope::Paragraph` directly; other scopes must be
    /// resolved to a paragraph by the application layer first.
    async fn create_named_range(
        &self,
        id: &DocumentId,
        name: &str,
        scope: Scope,
    ) -> Result<NamedRangeMeta, DocsError>;

    // ── Drive image hosting ──────────────────────────────────────

    /// Upload raw image bytes to Drive as a standalone image file (NO Doc
    /// conversion). Returns the Drive `file_id`. Used to host an attachment
    /// publicly so `insertInlineImage` can fetch it. `mime` must be an image
    /// type (e.g. `image/png`). `filename` is the Drive file name.
    async fn upload_image_to_drive(
        &self,
        bytes: Vec<u8>,
        mime: &str,
        filename: &str,
    ) -> Result<String, DocsError>;

    /// Grant `anyone with the link` reader access to a Drive file (so Google
    /// can fetch it server-side for `insertInlineImage`).
    async fn set_anyone_reader(&self, file_id: &str) -> Result<(), DocsError>;

    /// Delete a Drive file by id (`files.delete`). Used to clean up the temp
    /// image after Docs has copied it into the document.
    async fn delete_drive_file(&self, file_id: &str) -> Result<(), DocsError>;
}
