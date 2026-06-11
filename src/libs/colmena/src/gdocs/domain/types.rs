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

/// Bundle 2B (2026-06-11): one Drive permission entry on a file.
/// Mirrors the subset of `drive.permissions.get` fields the LLM uses to
/// decide whether to share/unshare a doc with someone. We omit fields
/// like `domain`, `expirationTime`, `pendingOwner` until a real use case
/// asks for them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionEntry {
    /// Drive's stable permission id — pass it to `delete_permission` to
    /// revoke a specific grant.
    pub permission_id: String,
    /// `"user" | "group" | "domain" | "anyone"`.
    #[serde(rename = "type")]
    pub permission_type: String,
    /// `"owner" | "organizer" | "fileOrganizer" | "writer" | "commenter" | "reader"`.
    pub role: String,
    /// Present when `permission_type == "user" | "group"` and the account
    /// surfaced an email (Drive masks this for some org policies).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// Present when `permission_type == "user"` (Drive resolves the
    /// display name from the linked Google account).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
}

/// Result of `list_permissions`. No pagination today — Drive returns all
/// entries in one page for files outside Shared Drives. If we ever need
/// to support large permission lists we can add `next_page_token` later.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionList {
    pub permissions: Vec<PermissionEntry>,
}

/// One entry in the `list_documents` response. Surfaces just the fields
/// the LLM cares about (id, name, url, modified time, owner emails);
/// downstream tools that need more metadata can call `get()` with the
/// returned `doc_id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentListItem {
    pub doc_id: DocumentId,
    pub name: String,
    pub url: String,
    /// RFC 3339 timestamp from Drive (`modifiedTime`).
    pub modified_time: String,
    /// Owner email addresses. Empty when Drive does not surface owners
    /// (e.g. files in a Shared Drive where ownership belongs to the org).
    #[serde(default)]
    pub owners: Vec<String>,
}

/// Result of `list_documents`. `next_page_token` is `Some(...)` when more
/// results are available — the LLM passes it back as `page_token` in the
/// next call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentListResult {
    pub documents: Vec<DocumentListItem>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_page_token: Option<String>,
}

/// Filter args for `list_documents`. All fields optional; an empty filter
/// returns the most-recently-modified documents the OAuth user can see.
#[derive(Debug, Clone, Default)]
pub struct DocumentListFilter<'a> {
    /// Substring match against `name`. Drive uses `contains` semantics.
    pub query: Option<&'a str>,
    /// Limit to a specific Drive folder. Folder must contain the doc as
    /// direct child (no recursion).
    pub parent_folder_id: Option<&'a str>,
    /// RFC 3339 lower bound on `modifiedTime`. Drive supports `>=` filter.
    pub modified_after: Option<&'a str>,
    /// Page size. Clamped to `[1, 100]` by the implementation.
    pub limit: Option<u32>,
    /// Pagination cursor from a prior call.
    pub page_token: Option<&'a str>,
}

/// Where to look for the `find` text. `All` is doc-wide (default).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Scope {
    #[default]
    All,
    Tab {
        tab_id: TabId,
    },
    Paragraph {
        n: u32,
    },
    UnderHeading {
        heading: String,
    },
    BetweenHeadings {
        after: String,
        before: Option<String>,
    },
}

/// 0..=1 normalised RGB.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RgbColor {
    pub r: f32,
    pub g: f32,
    pub b: f32,
}

/// Cascading heading level for paragraph styles, mapping to Google Docs'
/// `namedStyleType` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HeadingLevel {
    Normal,
    H1,
    H2,
    H3,
    H4,
    H5,
    H6,
    Title,
    Subtitle,
}

impl HeadingLevel {
    /// Wire-format string for `paragraphStyle.namedStyleType`.
    pub fn as_api_str(self) -> &'static str {
        match self {
            Self::Normal => "NORMAL_TEXT",
            Self::H1 => "HEADING_1",
            Self::H2 => "HEADING_2",
            Self::H3 => "HEADING_3",
            Self::H4 => "HEADING_4",
            Self::H5 => "HEADING_5",
            Self::H6 => "HEADING_6",
            Self::Title => "TITLE",
            Self::Subtitle => "SUBTITLE",
        }
    }
}

/// Style patch supplied by the agent. Only set fields are applied.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct StylePatch {
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub underline: Option<bool>,
    pub strikethrough: Option<bool>,
    pub font_size_pt: Option<f32>,
    pub foreground: Option<RgbColor>,
    pub background: Option<RgbColor>,
    pub link: Option<String>,
    pub heading_level: Option<HeadingLevel>,
}

/// What kind of edit a [`ChangeRecord`] describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Replace,
    Insert,
    Delete,
    Style,
}

/// One observable edit applied to a paragraph, returned to the dispatcher
/// so the agent can report what actually changed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChangeRecord {
    pub kind: ChangeKind,
    pub paragraph: u32,
    pub before: Option<String>,
    pub after: Option<String>,
    pub tab_id: Option<TabId>,
}

/// Lossless round-trip — one of 11 paragraph shapes Google Docs supports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParagraphKind {
    Heading1,
    Heading2,
    Heading3,
    Heading4,
    Heading5,
    Heading6,
    Title,
    Subtitle,
    Paragraph,
    ListItem,
    TableRow,
}

/// A single line in the document outline returned to the agent after every
/// edit so it can reason about structure without re-fetching the doc.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OutlineEntry {
    pub paragraph: u32,
    pub tab_id: Option<TabId>,
    pub kind: ParagraphKind,
    pub text_preview: String,
}

/// Kind of edit a human collaborator made to the doc since the agent last
/// synced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HumanChangeKind {
    Insert,
    Modify,
    Delete,
}

/// A human-authored change observed outside the agent's current edit scope,
/// surfaced so the agent can decide whether to proceed or re-plan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HumanChange {
    pub kind: HumanChangeKind,
    pub paragraph: u32,
    pub preview: String,
    pub modified_time: chrono::DateTime<chrono::Utc>,
    pub modifying_user: Option<String>,
    /// Tab where the change happened. `None` for single-tab docs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab_id: Option<TabId>,
    /// Paragraph text before the change. `None` for `Insert`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before_text: Option<String>,
    /// Paragraph text after the change. `None` for `Delete`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_text: Option<String>,
}

/// A markdown element that could not be losslessly converted to Google
/// Docs (e.g. a footnote or math block); reported so the agent can warn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LossyConversion {
    pub element_type: String,
    pub original_markdown: String,
}

/// A single hit when previewing a find/replace operation before applying.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MatchPreview {
    pub n: u32,
    pub paragraph: u32,
    pub preview: String,
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

/// Top-level metadata about a Google Doc returned by `open`/`create` calls.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocumentMeta {
    pub doc_id: DocumentId,
    pub url: String,
    pub title: String,
    pub revision_id: RevisionId,
    pub tabs: Vec<TabMeta>,
}

/// Metadata for a single tab inside a multi-tab doc.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TabMeta {
    pub tab_id: TabId,
    pub title: String,
    pub index: u32,
    pub parent_tab_id: Option<TabId>,
}

/// Metadata describing a named range — a stable, name-addressable paragraph
/// span the agent can reference across edits.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NamedRangeMeta {
    pub named_range_id: String,
    pub name: String,
    pub paragraph_start: u32,
    pub paragraph_end: u32,
}

/// Metadata about a past revision returned by Drive's `revisions.list`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RevisionMeta {
    pub revision_id: RevisionId,
    pub modified_time: chrono::DateTime<chrono::Utc>,
    pub modifying_user_email: Option<String>,
}

/// What `create_from_markdown` returns — doc metadata plus the initial
/// outline and any markdown elements that didn't round-trip cleanly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateFromMarkdownResult {
    pub meta: DocumentMeta,
    pub outline_snapshot: Vec<OutlineEntry>,
    pub lossy_conversions: Vec<LossyConversion>,
}

/// Drive sharing role granted by a `share` call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShareRole {
    Reader,
    Commenter,
    Writer,
}

impl ShareRole {
    /// Wire-format string for Drive's `permissions.role` field.
    pub fn as_api_str(self) -> &'static str {
        match self {
            Self::Reader => "reader",
            Self::Commenter => "commenter",
            Self::Writer => "writer",
        }
    }
}

/// Export formats supported by Drive's `files.export`.
///
/// Note: `Html` exports as a zipped HTML + assets bundle, so its MIME is
/// `application/zip` — see [`ExportFormat::mime`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExportFormat {
    Docx,
    Pdf,
    Markdown,
    Txt,
    Rtf,
    Epub,
    Odt,
    Html,
}

impl ExportFormat {
    /// Wire-format MIME string for Drive's `files.export?mimeType=...`.
    pub fn mime(self) -> &'static str {
        match self {
            Self::Docx => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            Self::Pdf => "application/pdf",
            Self::Markdown => "text/markdown",
            Self::Txt => "text/plain",
            Self::Rtf => "application/rtf",
            Self::Epub => "application/epub+zip",
            Self::Odt => "application/vnd.oasis.opendocument.text",
            // Drive bundles HTML + assets into a zip — this is the documented MIME.
            Self::Html => "application/zip",
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

/// One tab's worth of paragraphs inside a [`DocumentSnapshot`]. `tab_id` is
/// `None` for single-tab docs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TabSnapshot {
    pub tab_id: Option<TabId>,
    pub paragraphs: Vec<ParagraphSnapshot>,
}

/// One paragraph inside a [`TabSnapshot`], with the character offsets the
/// Docs API uses to address it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParagraphSnapshot {
    pub n: u32,
    pub kind: ParagraphKind,
    pub text: String,
    pub start_index: u32,
    pub end_index: u32,
}

/// Raw result of a `documents.batchUpdate` round-trip — kept verbatim so
/// higher layers can extract reply-specific fields.
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
            (json!({"kind": "all"}), Scope::All),
            (
                json!({"kind": "tab", "tab_id": "t1"}),
                Scope::Tab {
                    tab_id: TabId("t1".into()),
                },
            ),
            (
                json!({"kind": "paragraph", "n": 5}),
                Scope::Paragraph { n: 5 },
            ),
            (
                json!({"kind": "under_heading", "heading": "## A"}),
                Scope::UnderHeading {
                    heading: "## A".into(),
                },
            ),
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
        assert_eq!(
            ExportFormat::Docx.mime(),
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
        );
        assert_eq!(ExportFormat::Markdown.mime(), "text/markdown");
    }

    #[test]
    fn human_change_serializes_new_fields() {
        let c = HumanChange {
            kind: HumanChangeKind::Modify,
            paragraph: 4,
            preview: "abc".into(),
            modified_time: "2026-06-09T23:25:13Z".parse().unwrap(),
            modifying_user: None,
            tab_id: Some(TabId("t1".into())),
            before_text: Some("a".into()),
            after_text: Some("b".into()),
        };
        let v = serde_json::to_value(&c).unwrap();
        assert_eq!(v["tab_id"], "t1");
        assert_eq!(v["before_text"], "a");
        assert_eq!(v["after_text"], "b");
        assert_eq!(v["kind"], "modify");
    }

    #[test]
    fn human_change_omits_new_fields_when_none() {
        let c = HumanChange {
            kind: HumanChangeKind::Modify,
            paragraph: 4,
            preview: "abc".into(),
            modified_time: "2026-06-09T23:25:13Z".parse().unwrap(),
            modifying_user: None,
            tab_id: None,
            before_text: None,
            after_text: None,
        };
        let v = serde_json::to_value(&c).unwrap();
        assert!(v.get("tab_id").is_none());
        assert!(v.get("before_text").is_none());
        assert!(v.get("after_text").is_none());
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
