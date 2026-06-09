//! Google Docs synthetic LLM tools (Subsystem G). 22 tools mapping to
//! content-addressed surgical edits. Each dispatcher builds a
//! `GoogleDocsHttpClient` + `OutlineCache` + `PostgresRevisionStore`
//! once per process (`OnceCell`), then routes to the matching
//! application use case.
//!
//! Layered like `gsheets_tools` for skill transfer:
//! - Const tool names
//! - Args structs (`#[derive(Deserialize, JsonSchema)]`)
//! - Tool factories (`tool_*`) wrap the args via `build_synthetic_tool_with_summary`
//! - Dispatcher async fns (`dispatch_*`) parse args, build a
//!   `GuardContext`, and call into `gdocs::application`.

use crate::gdocs::application::{
    apply_edits, co_edit_guard, delete_text, insert, named_range, replace_section, replace_text,
    style,
};
use crate::gdocs::domain::{
    DocsClient, DocsError, DocumentId, ExportFormat, NamedRangeMeta, Scope, ShareRole, StylePatch,
    TabId,
};
use crate::gdocs::infrastructure::config::GDocsConfig;
use crate::gdocs::infrastructure::http_client::GoogleDocsHttpClient;
use crate::gdocs::infrastructure::outline_cache::OutlineCache;
use crate::gdocs::infrastructure::revision_store::{PostgresRevisionStore, RevisionStore};
use crate::llm::domain::tools::ToolDefinition;
use crate::text;
use schemars::JsonSchema;
use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::OnceCell;

// ── 22 tool name constants ────────────────────────────────────────────

pub const TOOL_CREATE: &str = "gdocs_create";
pub const TOOL_CREATE_FROM_MARKDOWN: &str = "gdocs_create_from_markdown";
pub const TOOL_CREATE_FROM_DOCX: &str = "gdocs_create_from_docx";
pub const TOOL_SHARE: &str = "gdocs_share";
pub const TOOL_EXPORT: &str = "gdocs_export";
pub const TOOL_LIST_TABS: &str = "gdocs_list_tabs";
pub const TOOL_ADD_TAB: &str = "gdocs_add_tab";
pub const TOOL_READ_AS_MARKDOWN: &str = "gdocs_read_as_markdown";
pub const TOOL_READ_OUTLINE: &str = "gdocs_read_outline";
pub const TOOL_LIST_NAMED_RANGES: &str = "gdocs_list_named_ranges";
pub const TOOL_REPLACE_TEXT: &str = "gdocs_replace_text";
pub const TOOL_INSERT_AFTER_TEXT: &str = "gdocs_insert_after_text";
pub const TOOL_INSERT_BEFORE_TEXT: &str = "gdocs_insert_before_text";
pub const TOOL_INSERT_BETWEEN: &str = "gdocs_insert_between";
pub const TOOL_DELETE_TEXT: &str = "gdocs_delete_text";
pub const TOOL_REPLACE_SECTION: &str = "gdocs_replace_section";
pub const TOOL_APPEND_MARKDOWN: &str = "gdocs_append_markdown";
pub const TOOL_APPLY_EDITS: &str = "gdocs_apply_edits";
pub const TOOL_STYLE_TEXT: &str = "gdocs_style_text";
pub const TOOL_CREATE_NAMED_RANGE: &str = "gdocs_create_named_range";
pub const TOOL_REPLACE_NAMED_RANGE: &str = "gdocs_replace_named_range";
pub const TOOL_ACKNOWLEDGE_HUMAN_CHANGES: &str = "gdocs_acknowledge_human_changes";

// ── Process-wide singletons ───────────────────────────────────────────

static CLIENT: OnceCell<Arc<GoogleDocsHttpClient>> = OnceCell::const_new();
static CACHE: OnceCell<Arc<OutlineCache>> = OnceCell::const_new();
static REVS: OnceCell<Arc<dyn RevisionStore>> = OnceCell::const_new();

async fn shared_client() -> Result<Arc<GoogleDocsHttpClient>, serde_json::Value> {
    CLIENT
        .get_or_try_init(|| async {
            let cfg = GDocsConfig::from_env();
            GoogleDocsHttpClient::from_config(&cfg).map(Arc::new)
        })
        .await
        .cloned()
        .map_err(error_to_json)
}

async fn shared_cache() -> Arc<OutlineCache> {
    CACHE
        .get_or_init(|| async {
            let cfg = GDocsConfig::from_env();
            Arc::new(OutlineCache::new(cfg.revision_cache_ttl))
        })
        .await
        .clone()
}

async fn shared_revs() -> Result<Arc<dyn RevisionStore>, serde_json::Value> {
    REVS.get_or_try_init(|| async {
        let url = std::env::var("DATABASE_URL").map_err(|_| {
            DocsError::NotConfigured("DATABASE_URL required for revision tracking".into())
        })?;
        let pool = sqlx::PgPool::connect(&url)
            .await
            .map_err(|e| DocsError::Internal(format!("db connect: {e}")))?;
        let arc: Arc<dyn RevisionStore> = Arc::new(PostgresRevisionStore::new(pool).await);
        Ok::<_, DocsError>(arc)
    })
    .await
    .cloned()
    .map_err(error_to_json)
}

// ── Error mapping ─────────────────────────────────────────────────────

/// Map a `DocsError` to a stable JSON envelope the LLM can pattern-match.
pub(crate) fn error_to_json(e: DocsError) -> serde_json::Value {
    use DocsError::*;
    match &e {
        AmbiguousMatch { find, matches } => serde_json::json!({
            "error": "ambiguous_match",
            "find": find,
            "matches": matches,
            "valid_next_moves": [
                {"action": "specify_occurrence",
                 "example": "gdocs_replace_text(..., occurrence: 1)"},
                {"action": "use_anchor",
                 "example": "gdocs_replace_text(..., anchor: 'preceded text…')"},
            ]
        }),
        TextNotFound {
            find,
            fuzzy_suggestions,
        } => serde_json::json!({
            "error": "text_not_found",
            "find": find,
            "fuzzy_suggestions": fuzzy_suggestions,
        }),
        ConfirmManyMatches {
            find,
            count,
            preview,
        } => serde_json::json!({
            "error": "confirm_many_matches",
            "find": find,
            "count": count,
            "preview": preview,
            "valid_next_moves": [
                {"action": "confirm",
                 "example": "<re-call with confirm_many: true>"},
                {"action": "narrow_scope",
                 "example": "<re-call with a more specific scope>"},
                {"action": "dry_run",
                 "example": "<re-call with dry_run: true to preview>"}
            ]
        }),
        HumanChangesPending {
            since,
            changes_overlapping_scope,
            changes_outside_scope,
        } => serde_json::json!({
            "error": "human_changes_pending",
            "since": since,
            "changes_overlapping_scope": changes_overlapping_scope,
            "changes_outside_scope": changes_outside_scope,
            "valid_next_moves": [
                {"action": "acknowledge",
                 "example": "gdocs_acknowledge_human_changes(doc_id) then retry"},
            ]
        }),
        NotConfigured(m) => serde_json::json!({"error": "gdocs_not_configured", "hint": m}),
        AuthFailed(m) => serde_json::json!({"error": "auth_failed", "message": m}),
        DocumentNotFound(d) => serde_json::json!({"error": "document_not_found", "doc_id": d}),
        TabNotFound(t) => serde_json::json!({"error": "tab_not_found", "tab_id": t}),
        PermissionDenied(sa) => serde_json::json!({
            "error": "permission_denied",
            "hint": format!("share the doc with {sa}")
        }),
        NoParentFolder => serde_json::json!({
            "error": "no_parent_folder_configured",
            "hint": "set COLMENA_GDOCS_DEFAULT_PARENT_FOLDER_ID or pass parent_folder_id"
        }),
        ScopeCrossesBoundary(m) => {
            serde_json::json!({"error": "scope_crosses_boundary", "message": m})
        }
        TabExists(t) => serde_json::json!({"error": "tab_exists", "title": t}),
        RateLimit(s) => serde_json::json!({"error": "rate_limit", "retry_after_seconds": s}),
        Conflict => serde_json::json!({"error": "conflict"}),
        Http(m) => serde_json::json!({"error": "http_error", "message": m}),
        InvalidArgs(m) => serde_json::json!({"error": "invalid_args", "message": m}),
        Internal(m) => serde_json::json!({"error": "internal", "message": m}),
    }
}

// ── Helpers ───────────────────────────────────────────────────────────

fn parse_scope(v: Option<serde_json::Value>) -> Scope {
    v.and_then(|j| serde_json::from_value(j).ok())
        .unwrap_or_default()
}

fn parse_style_patch(v: serde_json::Value) -> StylePatch {
    serde_json::from_value(v).unwrap_or_default()
}

fn parse_share_role(s: &str) -> Result<ShareRole, serde_json::Value> {
    match s {
        "reader" => Ok(ShareRole::Reader),
        "commenter" => Ok(ShareRole::Commenter),
        "writer" => Ok(ShareRole::Writer),
        other => Err(serde_json::json!({
            "error": "invalid_args",
            "message": format!("role must be reader|commenter|writer, got '{other}'")
        })),
    }
}

fn parse_export_format(s: &str) -> Result<ExportFormat, serde_json::Value> {
    match s {
        "docx" => Ok(ExportFormat::Docx),
        "pdf" => Ok(ExportFormat::Pdf),
        "markdown" => Ok(ExportFormat::Markdown),
        "txt" => Ok(ExportFormat::Txt),
        "rtf" => Ok(ExportFormat::Rtf),
        "epub" => Ok(ExportFormat::Epub),
        "odt" => Ok(ExportFormat::Odt),
        "html" => Ok(ExportFormat::Html),
        other => Err(serde_json::json!({
            "error": "invalid_args",
            "message": format!("format must be docx|pdf|markdown|txt|rtf|epub|odt|html, got '{other}'")
        })),
    }
}

fn named_range_meta_to_json(nr: &NamedRangeMeta) -> serde_json::Value {
    serde_json::json!({
        "named_range_id": nr.named_range_id,
        "name": nr.name,
        "paragraph_start": nr.paragraph_start,
        "paragraph_end": nr.paragraph_end,
    })
}

// ── Args structs ──────────────────────────────────────────────────────

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CreateArgs {
    pub title: String,
    pub parent_folder_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CreateFromMarkdownArgs {
    pub title: String,
    pub markdown: String,
    pub parent_folder_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CreateFromDocxArgs {
    pub title: String,
    pub attachment_id: String,
    pub parent_folder_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ShareArgs {
    pub doc_id: String,
    pub email: String,
    /// `"reader" | "commenter" | "writer"`.
    pub role: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ExportArgs {
    pub doc_id: String,
    /// `"docx" | "pdf" | "markdown" | "txt" | "rtf" | "epub" | "odt" | "html"`.
    pub format: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListTabsArgs {
    pub doc_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AddTabArgs {
    pub doc_id: String,
    pub title: String,
    pub after_tab_id: Option<String>,
    pub markdown: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReadAsMarkdownArgs {
    pub doc_id: String,
    pub tab_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReadOutlineArgs {
    pub doc_id: String,
    pub tab_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListNamedRangesArgs {
    pub doc_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReplaceTextArgs {
    pub doc_id: String,
    pub find: String,
    pub replace: String,
    pub scope: Option<serde_json::Value>,
    #[serde(default)]
    pub case_sensitive: bool,
    #[serde(default)]
    pub whole_word: bool,
    #[serde(default)]
    pub dry_run: bool,
    #[serde(default)]
    pub confirm_many: bool,
    pub occurrence: Option<u32>,
    pub anchor: Option<String>,
    pub mode: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct InsertAfterTextArgs {
    pub doc_id: String,
    pub anchor: String,
    pub new_markdown: String,
    pub occurrence: Option<u32>,
    pub mode: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct InsertBeforeTextArgs {
    pub doc_id: String,
    pub anchor: String,
    pub new_markdown: String,
    pub occurrence: Option<u32>,
    pub mode: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct InsertBetweenArgs {
    pub doc_id: String,
    pub after_heading: String,
    pub before_heading: Option<String>,
    pub new_markdown: String,
    pub mode: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DeleteTextArgs {
    pub doc_id: String,
    pub find: String,
    pub scope: Option<serde_json::Value>,
    #[serde(default)]
    pub case_sensitive: bool,
    #[serde(default)]
    pub whole_word: bool,
    pub occurrence: Option<u32>,
    #[serde(default)]
    pub confirm_many: bool,
    pub mode: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReplaceSectionArgs {
    pub doc_id: String,
    pub heading: String,
    pub new_markdown: String,
    pub tab_id: Option<String>,
    pub mode: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AppendMarkdownArgs {
    pub doc_id: String,
    pub markdown: String,
    pub tab_id: Option<String>,
    pub mode: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct StyleTextArgs {
    pub doc_id: String,
    pub find: String,
    pub style: serde_json::Value,
    pub scope: Option<serde_json::Value>,
    #[serde(default)]
    pub case_sensitive: bool,
    #[serde(default)]
    pub whole_word: bool,
    #[serde(default)]
    pub dry_run: bool,
    pub occurrence: Option<u32>,
    pub anchor: Option<String>,
    pub mode: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ApplyEditsArgs {
    pub doc_id: String,
    pub edits: Vec<serde_json::Value>,
    pub mode: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CreateNamedRangeArgs {
    pub doc_id: String,
    pub name: String,
    pub scope: serde_json::Value,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReplaceNamedRangeArgs {
    pub doc_id: String,
    pub name: String,
    pub new_text: String,
    pub mode: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AcknowledgeHumanChangesArgs {
    pub doc_id: String,
}

// ── Tool factories (22) ───────────────────────────────────────────────

pub fn tool_create() -> ToolDefinition {
    super::build_synthetic_tool_with_summary::<CreateArgs>(
        TOOL_CREATE,
        text::tool_description(TOOL_CREATE),
        text::tool_summary(TOOL_CREATE),
    )
}

pub fn tool_create_from_markdown() -> ToolDefinition {
    super::build_synthetic_tool_with_summary::<CreateFromMarkdownArgs>(
        TOOL_CREATE_FROM_MARKDOWN,
        text::tool_description(TOOL_CREATE_FROM_MARKDOWN),
        text::tool_summary(TOOL_CREATE_FROM_MARKDOWN),
    )
}

pub fn tool_create_from_docx() -> ToolDefinition {
    super::build_synthetic_tool_with_summary::<CreateFromDocxArgs>(
        TOOL_CREATE_FROM_DOCX,
        text::tool_description(TOOL_CREATE_FROM_DOCX),
        text::tool_summary(TOOL_CREATE_FROM_DOCX),
    )
}

pub fn tool_share() -> ToolDefinition {
    super::build_synthetic_tool_with_summary::<ShareArgs>(
        TOOL_SHARE,
        text::tool_description(TOOL_SHARE),
        text::tool_summary(TOOL_SHARE),
    )
}

pub fn tool_export() -> ToolDefinition {
    super::build_synthetic_tool_with_summary::<ExportArgs>(
        TOOL_EXPORT,
        text::tool_description(TOOL_EXPORT),
        text::tool_summary(TOOL_EXPORT),
    )
}

pub fn tool_list_tabs() -> ToolDefinition {
    super::build_synthetic_tool_with_summary::<ListTabsArgs>(
        TOOL_LIST_TABS,
        text::tool_description(TOOL_LIST_TABS),
        text::tool_summary(TOOL_LIST_TABS),
    )
}

pub fn tool_add_tab() -> ToolDefinition {
    super::build_synthetic_tool_with_summary::<AddTabArgs>(
        TOOL_ADD_TAB,
        text::tool_description(TOOL_ADD_TAB),
        text::tool_summary(TOOL_ADD_TAB),
    )
}

pub fn tool_read_as_markdown() -> ToolDefinition {
    super::build_synthetic_tool_with_summary::<ReadAsMarkdownArgs>(
        TOOL_READ_AS_MARKDOWN,
        text::tool_description(TOOL_READ_AS_MARKDOWN),
        text::tool_summary(TOOL_READ_AS_MARKDOWN),
    )
}

pub fn tool_read_outline() -> ToolDefinition {
    super::build_synthetic_tool_with_summary::<ReadOutlineArgs>(
        TOOL_READ_OUTLINE,
        text::tool_description(TOOL_READ_OUTLINE),
        text::tool_summary(TOOL_READ_OUTLINE),
    )
}

pub fn tool_list_named_ranges() -> ToolDefinition {
    super::build_synthetic_tool_with_summary::<ListNamedRangesArgs>(
        TOOL_LIST_NAMED_RANGES,
        text::tool_description(TOOL_LIST_NAMED_RANGES),
        text::tool_summary(TOOL_LIST_NAMED_RANGES),
    )
}

pub fn tool_replace_text() -> ToolDefinition {
    super::build_synthetic_tool_with_summary::<ReplaceTextArgs>(
        TOOL_REPLACE_TEXT,
        text::tool_description(TOOL_REPLACE_TEXT),
        text::tool_summary(TOOL_REPLACE_TEXT),
    )
}

pub fn tool_insert_after_text() -> ToolDefinition {
    super::build_synthetic_tool_with_summary::<InsertAfterTextArgs>(
        TOOL_INSERT_AFTER_TEXT,
        text::tool_description(TOOL_INSERT_AFTER_TEXT),
        text::tool_summary(TOOL_INSERT_AFTER_TEXT),
    )
}

pub fn tool_insert_before_text() -> ToolDefinition {
    super::build_synthetic_tool_with_summary::<InsertBeforeTextArgs>(
        TOOL_INSERT_BEFORE_TEXT,
        text::tool_description(TOOL_INSERT_BEFORE_TEXT),
        text::tool_summary(TOOL_INSERT_BEFORE_TEXT),
    )
}

pub fn tool_insert_between() -> ToolDefinition {
    super::build_synthetic_tool_with_summary::<InsertBetweenArgs>(
        TOOL_INSERT_BETWEEN,
        text::tool_description(TOOL_INSERT_BETWEEN),
        text::tool_summary(TOOL_INSERT_BETWEEN),
    )
}

pub fn tool_delete_text() -> ToolDefinition {
    super::build_synthetic_tool_with_summary::<DeleteTextArgs>(
        TOOL_DELETE_TEXT,
        text::tool_description(TOOL_DELETE_TEXT),
        text::tool_summary(TOOL_DELETE_TEXT),
    )
}

pub fn tool_replace_section() -> ToolDefinition {
    super::build_synthetic_tool_with_summary::<ReplaceSectionArgs>(
        TOOL_REPLACE_SECTION,
        text::tool_description(TOOL_REPLACE_SECTION),
        text::tool_summary(TOOL_REPLACE_SECTION),
    )
}

pub fn tool_append_markdown() -> ToolDefinition {
    super::build_synthetic_tool_with_summary::<AppendMarkdownArgs>(
        TOOL_APPEND_MARKDOWN,
        text::tool_description(TOOL_APPEND_MARKDOWN),
        text::tool_summary(TOOL_APPEND_MARKDOWN),
    )
}

pub fn tool_apply_edits() -> ToolDefinition {
    super::build_synthetic_tool_with_summary::<ApplyEditsArgs>(
        TOOL_APPLY_EDITS,
        text::tool_description(TOOL_APPLY_EDITS),
        text::tool_summary(TOOL_APPLY_EDITS),
    )
}

pub fn tool_style_text() -> ToolDefinition {
    super::build_synthetic_tool_with_summary::<StyleTextArgs>(
        TOOL_STYLE_TEXT,
        text::tool_description(TOOL_STYLE_TEXT),
        text::tool_summary(TOOL_STYLE_TEXT),
    )
}

pub fn tool_create_named_range() -> ToolDefinition {
    super::build_synthetic_tool_with_summary::<CreateNamedRangeArgs>(
        TOOL_CREATE_NAMED_RANGE,
        text::tool_description(TOOL_CREATE_NAMED_RANGE),
        text::tool_summary(TOOL_CREATE_NAMED_RANGE),
    )
}

pub fn tool_replace_named_range() -> ToolDefinition {
    super::build_synthetic_tool_with_summary::<ReplaceNamedRangeArgs>(
        TOOL_REPLACE_NAMED_RANGE,
        text::tool_description(TOOL_REPLACE_NAMED_RANGE),
        text::tool_summary(TOOL_REPLACE_NAMED_RANGE),
    )
}

pub fn tool_acknowledge_human_changes() -> ToolDefinition {
    super::build_synthetic_tool_with_summary::<AcknowledgeHumanChangesArgs>(
        TOOL_ACKNOWLEDGE_HUMAN_CHANGES,
        text::tool_description(TOOL_ACKNOWLEDGE_HUMAN_CHANGES),
        text::tool_summary(TOOL_ACKNOWLEDGE_HUMAN_CHANGES),
    )
}

/// Collect every gdocs tool factory output in one call. Mirrors the
/// `build_all_*` helpers in neighboring modules and is used by the
/// `every_registered_tool_has_text_entry` test in mod.rs.
pub fn build_all_gdocs_tools() -> Vec<ToolDefinition> {
    vec![
        tool_create(),
        tool_create_from_markdown(),
        tool_create_from_docx(),
        tool_share(),
        tool_export(),
        tool_list_tabs(),
        tool_add_tab(),
        tool_read_as_markdown(),
        tool_read_outline(),
        tool_list_named_ranges(),
        tool_replace_text(),
        tool_insert_after_text(),
        tool_insert_before_text(),
        tool_insert_between(),
        tool_delete_text(),
        tool_replace_section(),
        tool_append_markdown(),
        tool_apply_edits(),
        tool_style_text(),
        tool_create_named_range(),
        tool_replace_named_range(),
        tool_acknowledge_human_changes(),
    ]
}

// ── Dispatchers ───────────────────────────────────────────────────────

fn invalid_args(e: impl std::fmt::Display) -> serde_json::Value {
    serde_json::json!({"error": "invalid_args", "message": e.to_string()})
}

/// Encode an `EditResult` into the standard tool envelope.
fn edit_result_to_json(r: crate::gdocs::domain::EditResult) -> serde_json::Value {
    serde_json::json!({
        "ok": true,
        "changes": r.changes,
        "revision_id_after": r.revision_id_after.0,
        "outline_snapshot": r.outline_snapshot,
        "lossy_conversions": r.lossy_conversions,
        "pending_human_changes_outside_scope": r.pending_human_changes_outside_scope,
    })
}

// Lifecycle ────────────────────────────────────────────────────────────

pub async fn dispatch_create(args: serde_json::Value, _session_id: &str) -> serde_json::Value {
    let parsed: CreateArgs = match serde_json::from_value(args) {
        Ok(a) => a,
        Err(e) => return invalid_args(e),
    };
    let client = match shared_client().await {
        Ok(c) => c,
        Err(e) => return e,
    };
    let parent_env = std::env::var("COLMENA_GDOCS_DEFAULT_PARENT_FOLDER_ID").ok();
    let parent = parsed.parent_folder_id.as_deref().or(parent_env.as_deref());
    match client.create(&parsed.title, parent).await {
        Ok(m) => serde_json::json!({
            "ok": true,
            "doc_id": m.doc_id.0,
            "url": m.url,
            "title": m.title,
            "revision_id": m.revision_id.0,
            "tabs": m.tabs,
        }),
        Err(e) => error_to_json(e),
    }
}

pub async fn dispatch_create_from_markdown(
    args: serde_json::Value,
    _session_id: &str,
) -> serde_json::Value {
    let parsed: CreateFromMarkdownArgs = match serde_json::from_value(args) {
        Ok(a) => a,
        Err(e) => return invalid_args(e),
    };
    let client = match shared_client().await {
        Ok(c) => c,
        Err(e) => return e,
    };
    let parent_env = std::env::var("COLMENA_GDOCS_DEFAULT_PARENT_FOLDER_ID").ok();
    let parent = parsed.parent_folder_id.as_deref().or(parent_env.as_deref());
    match client
        .create_from_markdown(&parsed.title, &parsed.markdown, parent)
        .await
    {
        Ok(r) => serde_json::json!({
            "ok": true,
            "doc_id": r.meta.doc_id.0,
            "url": r.meta.url,
            "title": r.meta.title,
            "revision_id": r.meta.revision_id.0,
            "tabs": r.meta.tabs,
            "outline_snapshot": r.outline_snapshot,
            "lossy_conversions": r.lossy_conversions,
        }),
        Err(e) => error_to_json(e),
    }
}

pub async fn dispatch_create_from_docx(
    args: serde_json::Value,
    _session_id: &str,
) -> serde_json::Value {
    let parsed: CreateFromDocxArgs = match serde_json::from_value(args) {
        Ok(a) => a,
        Err(e) => return invalid_args(e),
    };
    // Attachment resolver plumbing lives in T21 (toolkit router). For now
    // surface a clear `not_yet_wired` envelope so the LLM gets a typed
    // signal instead of a 500.
    serde_json::json!({
        "error": "not_yet_wired",
        "message": "gdocs_create_from_docx requires attachment plumbing (Task 21).",
        "title": parsed.title,
        "attachment_id": parsed.attachment_id,
    })
}

pub async fn dispatch_share(args: serde_json::Value, _session_id: &str) -> serde_json::Value {
    let parsed: ShareArgs = match serde_json::from_value(args) {
        Ok(a) => a,
        Err(e) => return invalid_args(e),
    };
    let role = match parse_share_role(&parsed.role) {
        Ok(r) => r,
        Err(j) => return j,
    };
    let client = match shared_client().await {
        Ok(c) => c,
        Err(e) => return e,
    };
    match client
        .share(&DocumentId(parsed.doc_id), &parsed.email, role)
        .await
    {
        Ok(()) => serde_json::json!({"ok": true}),
        Err(e) => error_to_json(e),
    }
}

pub async fn dispatch_export(args: serde_json::Value, _session_id: &str) -> serde_json::Value {
    let parsed: ExportArgs = match serde_json::from_value(args) {
        Ok(a) => a,
        Err(e) => return invalid_args(e),
    };
    let fmt = match parse_export_format(&parsed.format) {
        Ok(f) => f,
        Err(j) => return j,
    };
    let client = match shared_client().await {
        Ok(c) => c,
        Err(e) => return e,
    };
    match client.export(&DocumentId(parsed.doc_id), fmt).await {
        Ok(bytes) => serde_json::json!({
            "ok": true,
            "format": parsed.format,
            "byte_len": bytes.len(),
            // Attachment registration is wired in T21 (router).
            "note": "binary bytes returned to the router; the agent receives an attachment_id from the router layer in v1.1",
        }),
        Err(e) => error_to_json(e),
    }
}

// Reads ────────────────────────────────────────────────────────────────

pub async fn dispatch_list_tabs(args: serde_json::Value, _session_id: &str) -> serde_json::Value {
    let parsed: ListTabsArgs = match serde_json::from_value(args) {
        Ok(a) => a,
        Err(e) => return invalid_args(e),
    };
    let client = match shared_client().await {
        Ok(c) => c,
        Err(e) => return e,
    };
    match client.list_tabs(&DocumentId(parsed.doc_id)).await {
        Ok(tabs) => serde_json::json!({"ok": true, "tabs": tabs}),
        Err(e) => error_to_json(e),
    }
}

pub async fn dispatch_add_tab(args: serde_json::Value, session_id: &str) -> serde_json::Value {
    let parsed: AddTabArgs = match serde_json::from_value(args) {
        Ok(a) => a,
        Err(e) => return invalid_args(e),
    };
    let client = match shared_client().await {
        Ok(c) => c,
        Err(e) => return e,
    };
    let after = parsed.after_tab_id.map(TabId);
    let doc_id = DocumentId(parsed.doc_id);
    match client.add_tab(&doc_id, &parsed.title, after.as_ref()).await {
        Ok(tab) => {
            // Invalidate the outline cache so the next read/edit sees
            // the new tab. Without this, an immediate
            // `append_markdown({tab_id: <new>, ...})` would resolve
            // its insert_index against a stale snapshot that lacks
            // the new tab and the markdown would land in the wrong
            // tab (Google defaults to tab 1 when tabId is unknown).
            shared_cache().await.invalidate(session_id, &doc_id);
            serde_json::json!({
                "ok": true,
                "tab_id": tab.tab_id.0,
                "title": tab.title,
                "index": tab.index,
                "parent_tab_id": tab.parent_tab_id.map(|t| t.0),
                // markdown seeding for new tabs is deferred to v1.1 — surface
                // the unused arg back so the LLM can see it was acknowledged.
                "pending_markdown_seed": parsed.markdown.is_some(),
            })
        }
        Err(e) => error_to_json(e),
    }
}

pub async fn dispatch_read_as_markdown(
    args: serde_json::Value,
    _session_id: &str,
) -> serde_json::Value {
    let parsed: ReadAsMarkdownArgs = match serde_json::from_value(args) {
        Ok(a) => a,
        Err(e) => return invalid_args(e),
    };
    let client = match shared_client().await {
        Ok(c) => c,
        Err(e) => return e,
    };
    let tab = parsed.tab_id.map(TabId);
    match client
        .read_as_markdown(&DocumentId(parsed.doc_id), tab.as_ref())
        .await
    {
        Ok(md) => serde_json::json!({"ok": true, "markdown": md}),
        Err(e) => error_to_json(e),
    }
}

pub async fn dispatch_read_outline(
    args: serde_json::Value,
    _session_id: &str,
) -> serde_json::Value {
    let parsed: ReadOutlineArgs = match serde_json::from_value(args) {
        Ok(a) => a,
        Err(e) => return invalid_args(e),
    };
    let client = match shared_client().await {
        Ok(c) => c,
        Err(e) => return e,
    };
    let tab = parsed.tab_id.map(TabId);
    match client
        .read_outline(&DocumentId(parsed.doc_id), tab.as_ref())
        .await
    {
        Ok(outline) => serde_json::json!({"ok": true, "outline": outline}),
        Err(e) => error_to_json(e),
    }
}

pub async fn dispatch_list_named_ranges(
    args: serde_json::Value,
    _session_id: &str,
) -> serde_json::Value {
    let parsed: ListNamedRangesArgs = match serde_json::from_value(args) {
        Ok(a) => a,
        Err(e) => return invalid_args(e),
    };
    let client = match shared_client().await {
        Ok(c) => c,
        Err(e) => return e,
    };
    match client.list_named_ranges(&DocumentId(parsed.doc_id)).await {
        Ok(ranges) => serde_json::json!({
            "ok": true,
            "named_ranges": ranges.iter().map(named_range_meta_to_json).collect::<Vec<_>>(),
        }),
        Err(e) => error_to_json(e),
    }
}

// Edits ────────────────────────────────────────────────────────────────

pub async fn dispatch_replace_text(args: serde_json::Value, session_id: &str) -> serde_json::Value {
    let parsed: ReplaceTextArgs = match serde_json::from_value(args) {
        Ok(a) => a,
        Err(e) => return invalid_args(e),
    };
    let client = match shared_client().await {
        Ok(c) => c,
        Err(e) => return e,
    };
    let cache = shared_cache().await;
    let revisions = match shared_revs().await {
        Ok(r) => r,
        Err(e) => return e,
    };
    let sa = std::env::var("COLMENA_GDOCS_SA_EMAIL").ok();
    let ctx = co_edit_guard::GuardContext {
        client: client.as_ref(),
        cache: cache.as_ref(),
        revisions: revisions.as_ref(),
        session_id,
        sa_email: sa.as_deref(),
    };
    let input = replace_text::ReplaceTextInput {
        find: parsed.find,
        replace: parsed.replace,
        scope: parse_scope(parsed.scope),
        case_sensitive: parsed.case_sensitive,
        whole_word: parsed.whole_word,
        dry_run: parsed.dry_run,
        confirm_many: parsed.confirm_many,
        occurrence: parsed.occurrence,
        anchor: parsed.anchor,
    };
    let doc_id = DocumentId(parsed.doc_id);
    match replace_text::run(&ctx, &doc_id, input).await {
        Ok(r) => edit_result_to_json(r),
        Err(e) => error_to_json(e),
    }
}

pub async fn dispatch_insert_after_text(
    args: serde_json::Value,
    session_id: &str,
) -> serde_json::Value {
    let parsed: InsertAfterTextArgs = match serde_json::from_value(args) {
        Ok(a) => a,
        Err(e) => return invalid_args(e),
    };
    let client = match shared_client().await {
        Ok(c) => c,
        Err(e) => return e,
    };
    let cache = shared_cache().await;
    let revisions = match shared_revs().await {
        Ok(r) => r,
        Err(e) => return e,
    };
    let sa = std::env::var("COLMENA_GDOCS_SA_EMAIL").ok();
    let ctx = co_edit_guard::GuardContext {
        client: client.as_ref(),
        cache: cache.as_ref(),
        revisions: revisions.as_ref(),
        session_id,
        sa_email: sa.as_deref(),
    };
    let input = insert::InsertAfterTextInput {
        anchor: parsed.anchor,
        new_markdown: parsed.new_markdown,
        occurrence: parsed.occurrence,
    };
    let doc_id = DocumentId(parsed.doc_id);
    match insert::run_after_text(&ctx, &doc_id, input).await {
        Ok(r) => edit_result_to_json(r),
        Err(e) => error_to_json(e),
    }
}

pub async fn dispatch_insert_before_text(
    args: serde_json::Value,
    session_id: &str,
) -> serde_json::Value {
    let parsed: InsertBeforeTextArgs = match serde_json::from_value(args) {
        Ok(a) => a,
        Err(e) => return invalid_args(e),
    };
    let client = match shared_client().await {
        Ok(c) => c,
        Err(e) => return e,
    };
    let cache = shared_cache().await;
    let revisions = match shared_revs().await {
        Ok(r) => r,
        Err(e) => return e,
    };
    let sa = std::env::var("COLMENA_GDOCS_SA_EMAIL").ok();
    let ctx = co_edit_guard::GuardContext {
        client: client.as_ref(),
        cache: cache.as_ref(),
        revisions: revisions.as_ref(),
        session_id,
        sa_email: sa.as_deref(),
    };
    let input = insert::InsertBeforeTextInput {
        anchor: parsed.anchor,
        new_markdown: parsed.new_markdown,
        occurrence: parsed.occurrence,
    };
    let doc_id = DocumentId(parsed.doc_id);
    match insert::run_before_text(&ctx, &doc_id, input).await {
        Ok(r) => edit_result_to_json(r),
        Err(e) => error_to_json(e),
    }
}

pub async fn dispatch_insert_between(
    args: serde_json::Value,
    session_id: &str,
) -> serde_json::Value {
    let parsed: InsertBetweenArgs = match serde_json::from_value(args) {
        Ok(a) => a,
        Err(e) => return invalid_args(e),
    };
    let client = match shared_client().await {
        Ok(c) => c,
        Err(e) => return e,
    };
    let cache = shared_cache().await;
    let revisions = match shared_revs().await {
        Ok(r) => r,
        Err(e) => return e,
    };
    let sa = std::env::var("COLMENA_GDOCS_SA_EMAIL").ok();
    let ctx = co_edit_guard::GuardContext {
        client: client.as_ref(),
        cache: cache.as_ref(),
        revisions: revisions.as_ref(),
        session_id,
        sa_email: sa.as_deref(),
    };
    let input = insert::InsertBetweenInput {
        after_heading: parsed.after_heading,
        before_heading: parsed.before_heading,
        new_markdown: parsed.new_markdown,
    };
    let doc_id = DocumentId(parsed.doc_id);
    match insert::run_between(&ctx, &doc_id, input).await {
        Ok(r) => edit_result_to_json(r),
        Err(e) => error_to_json(e),
    }
}

pub async fn dispatch_delete_text(args: serde_json::Value, session_id: &str) -> serde_json::Value {
    let parsed: DeleteTextArgs = match serde_json::from_value(args) {
        Ok(a) => a,
        Err(e) => return invalid_args(e),
    };
    let client = match shared_client().await {
        Ok(c) => c,
        Err(e) => return e,
    };
    let cache = shared_cache().await;
    let revisions = match shared_revs().await {
        Ok(r) => r,
        Err(e) => return e,
    };
    let sa = std::env::var("COLMENA_GDOCS_SA_EMAIL").ok();
    let ctx = co_edit_guard::GuardContext {
        client: client.as_ref(),
        cache: cache.as_ref(),
        revisions: revisions.as_ref(),
        session_id,
        sa_email: sa.as_deref(),
    };
    let input = delete_text::DeleteTextInput {
        find: parsed.find,
        scope: parse_scope(parsed.scope),
        case_sensitive: parsed.case_sensitive,
        whole_word: parsed.whole_word,
        occurrence: parsed.occurrence,
        confirm_many: parsed.confirm_many,
    };
    let doc_id = DocumentId(parsed.doc_id);
    match delete_text::run(&ctx, &doc_id, input).await {
        Ok(r) => edit_result_to_json(r),
        Err(e) => error_to_json(e),
    }
}

pub async fn dispatch_replace_section(
    args: serde_json::Value,
    session_id: &str,
) -> serde_json::Value {
    let parsed: ReplaceSectionArgs = match serde_json::from_value(args) {
        Ok(a) => a,
        Err(e) => return invalid_args(e),
    };
    let client = match shared_client().await {
        Ok(c) => c,
        Err(e) => return e,
    };
    let cache = shared_cache().await;
    let revisions = match shared_revs().await {
        Ok(r) => r,
        Err(e) => return e,
    };
    let sa = std::env::var("COLMENA_GDOCS_SA_EMAIL").ok();
    let ctx = co_edit_guard::GuardContext {
        client: client.as_ref(),
        cache: cache.as_ref(),
        revisions: revisions.as_ref(),
        session_id,
        sa_email: sa.as_deref(),
    };
    let input = replace_section::ReplaceSectionInput {
        heading: parsed.heading,
        new_markdown: parsed.new_markdown,
    };
    let doc_id = DocumentId(parsed.doc_id);
    match replace_section::run_replace_section(&ctx, &doc_id, input).await {
        Ok(r) => edit_result_to_json(r),
        Err(e) => error_to_json(e),
    }
}

pub async fn dispatch_append_markdown(
    args: serde_json::Value,
    session_id: &str,
) -> serde_json::Value {
    let parsed: AppendMarkdownArgs = match serde_json::from_value(args) {
        Ok(a) => a,
        Err(e) => return invalid_args(e),
    };
    let client = match shared_client().await {
        Ok(c) => c,
        Err(e) => return e,
    };
    let cache = shared_cache().await;
    let revisions = match shared_revs().await {
        Ok(r) => r,
        Err(e) => return e,
    };
    let sa = std::env::var("COLMENA_GDOCS_SA_EMAIL").ok();
    let ctx = co_edit_guard::GuardContext {
        client: client.as_ref(),
        cache: cache.as_ref(),
        revisions: revisions.as_ref(),
        session_id,
        sa_email: sa.as_deref(),
    };
    let input = replace_section::AppendMarkdownInput {
        new_markdown: parsed.markdown,
        tab_id: parsed.tab_id.map(TabId),
    };
    let doc_id = DocumentId(parsed.doc_id);
    match replace_section::run_append_markdown(&ctx, &doc_id, input).await {
        Ok(r) => edit_result_to_json(r),
        Err(e) => error_to_json(e),
    }
}

pub async fn dispatch_style_text(args: serde_json::Value, session_id: &str) -> serde_json::Value {
    let parsed: StyleTextArgs = match serde_json::from_value(args) {
        Ok(a) => a,
        Err(e) => return invalid_args(e),
    };
    let client = match shared_client().await {
        Ok(c) => c,
        Err(e) => return e,
    };
    let cache = shared_cache().await;
    let revisions = match shared_revs().await {
        Ok(r) => r,
        Err(e) => return e,
    };
    let sa = std::env::var("COLMENA_GDOCS_SA_EMAIL").ok();
    let ctx = co_edit_guard::GuardContext {
        client: client.as_ref(),
        cache: cache.as_ref(),
        revisions: revisions.as_ref(),
        session_id,
        sa_email: sa.as_deref(),
    };
    let input = style::StyleTextInput {
        find: parsed.find,
        style: parse_style_patch(parsed.style),
        scope: parse_scope(parsed.scope),
        case_sensitive: parsed.case_sensitive,
        whole_word: parsed.whole_word,
        occurrence: parsed.occurrence,
        anchor: parsed.anchor,
        dry_run: parsed.dry_run,
    };
    let doc_id = DocumentId(parsed.doc_id);
    match style::run(&ctx, &doc_id, input).await {
        Ok(r) => edit_result_to_json(r),
        Err(e) => error_to_json(e),
    }
}

pub async fn dispatch_apply_edits(args: serde_json::Value, session_id: &str) -> serde_json::Value {
    let parsed: ApplyEditsArgs = match serde_json::from_value(args) {
        Ok(a) => a,
        Err(e) => return invalid_args(e),
    };
    let mut ops: Vec<apply_edits::ApplyEditOp> = Vec::with_capacity(parsed.edits.len());
    for e in parsed.edits {
        let tool = e.get("tool").and_then(|v| v.as_str()).unwrap_or("");
        match tool {
            "replace_text" => {
                let find = e
                    .get("find")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let replace = e
                    .get("replace")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let scope = parse_scope(e.get("scope").cloned());
                ops.push(apply_edits::ApplyEditOp::ReplaceText {
                    find,
                    replace,
                    scope,
                });
            }
            "insert_after_text" => {
                let anchor = e
                    .get("anchor")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let new_markdown = e
                    .get("new_markdown")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                ops.push(apply_edits::ApplyEditOp::InsertAfterText {
                    anchor,
                    new_markdown,
                });
            }
            "delete_text" => {
                let find = e
                    .get("find")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let scope = parse_scope(e.get("scope").cloned());
                ops.push(apply_edits::ApplyEditOp::DeleteText { find, scope });
            }
            other => {
                return serde_json::json!({
                    "error": "invalid_args",
                    "message": format!(
                        "apply_edits: unknown sub-edit tool '{other}'. \
                         Allowed: replace_text|insert_after_text|delete_text"
                    )
                });
            }
        }
    }
    let client = match shared_client().await {
        Ok(c) => c,
        Err(e) => return e,
    };
    let cache = shared_cache().await;
    let revisions = match shared_revs().await {
        Ok(r) => r,
        Err(e) => return e,
    };
    let sa = std::env::var("COLMENA_GDOCS_SA_EMAIL").ok();
    let ctx = co_edit_guard::GuardContext {
        client: client.as_ref(),
        cache: cache.as_ref(),
        revisions: revisions.as_ref(),
        session_id,
        sa_email: sa.as_deref(),
    };
    let doc_id = DocumentId(parsed.doc_id);
    match apply_edits::run(&ctx, &doc_id, apply_edits::ApplyEditsInput { edits: ops }).await {
        Ok(r) => edit_result_to_json(r),
        Err(e) => error_to_json(e),
    }
}

pub async fn dispatch_create_named_range(
    args: serde_json::Value,
    session_id: &str,
) -> serde_json::Value {
    let parsed: CreateNamedRangeArgs = match serde_json::from_value(args) {
        Ok(a) => a,
        Err(e) => return invalid_args(e),
    };
    let scope: Scope = match serde_json::from_value(parsed.scope) {
        Ok(s) => s,
        Err(e) => return invalid_args(format!("scope: {e}")),
    };
    let client = match shared_client().await {
        Ok(c) => c,
        Err(e) => return e,
    };
    let cache = shared_cache().await;
    let revisions = match shared_revs().await {
        Ok(r) => r,
        Err(e) => return e,
    };
    let sa = std::env::var("COLMENA_GDOCS_SA_EMAIL").ok();
    let ctx = co_edit_guard::GuardContext {
        client: client.as_ref(),
        cache: cache.as_ref(),
        revisions: revisions.as_ref(),
        session_id,
        sa_email: sa.as_deref(),
    };
    let doc_id = DocumentId(parsed.doc_id);
    match named_range::run_create(
        &ctx,
        &doc_id,
        named_range::CreateNamedRangeInput {
            name: parsed.name,
            scope,
        },
    )
    .await
    {
        Ok(nr) => serde_json::json!({
            "ok": true,
            "named_range": named_range_meta_to_json(&nr),
        }),
        Err(e) => error_to_json(e),
    }
}

pub async fn dispatch_replace_named_range(
    args: serde_json::Value,
    session_id: &str,
) -> serde_json::Value {
    let parsed: ReplaceNamedRangeArgs = match serde_json::from_value(args) {
        Ok(a) => a,
        Err(e) => return invalid_args(e),
    };
    let client = match shared_client().await {
        Ok(c) => c,
        Err(e) => return e,
    };
    let cache = shared_cache().await;
    let revisions = match shared_revs().await {
        Ok(r) => r,
        Err(e) => return e,
    };
    let sa = std::env::var("COLMENA_GDOCS_SA_EMAIL").ok();
    let ctx = co_edit_guard::GuardContext {
        client: client.as_ref(),
        cache: cache.as_ref(),
        revisions: revisions.as_ref(),
        session_id,
        sa_email: sa.as_deref(),
    };
    let doc_id = DocumentId(parsed.doc_id);
    match named_range::run_replace(
        &ctx,
        &doc_id,
        named_range::ReplaceNamedRangeInput {
            name: parsed.name,
            new_text: parsed.new_text,
        },
    )
    .await
    {
        Ok(r) => edit_result_to_json(r),
        Err(e) => error_to_json(e),
    }
}

// Co-edit guard helpers ────────────────────────────────────────────────

pub async fn dispatch_acknowledge_human_changes(
    args: serde_json::Value,
    session_id: &str,
) -> serde_json::Value {
    let parsed: AcknowledgeHumanChangesArgs = match serde_json::from_value(args) {
        Ok(a) => a,
        Err(e) => return invalid_args(e),
    };
    let client = match shared_client().await {
        Ok(c) => c,
        Err(e) => return e,
    };
    let revisions = match shared_revs().await {
        Ok(r) => r,
        Err(e) => return e,
    };
    let doc_id = DocumentId(parsed.doc_id);
    let snap = match client.get(&doc_id).await {
        Ok(s) => s,
        Err(e) => return error_to_json(e),
    };
    if let Err(e) = revisions.put(session_id, &doc_id, &snap.revision_id).await {
        return error_to_json(e);
    }
    serde_json::json!({"ok": true, "revision_id_now": snap.revision_id.0})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_constants_unique() {
        let names = [
            TOOL_CREATE,
            TOOL_CREATE_FROM_MARKDOWN,
            TOOL_CREATE_FROM_DOCX,
            TOOL_SHARE,
            TOOL_EXPORT,
            TOOL_LIST_TABS,
            TOOL_ADD_TAB,
            TOOL_READ_AS_MARKDOWN,
            TOOL_READ_OUTLINE,
            TOOL_LIST_NAMED_RANGES,
            TOOL_REPLACE_TEXT,
            TOOL_INSERT_AFTER_TEXT,
            TOOL_INSERT_BEFORE_TEXT,
            TOOL_INSERT_BETWEEN,
            TOOL_DELETE_TEXT,
            TOOL_REPLACE_SECTION,
            TOOL_APPEND_MARKDOWN,
            TOOL_APPLY_EDITS,
            TOOL_STYLE_TEXT,
            TOOL_CREATE_NAMED_RANGE,
            TOOL_REPLACE_NAMED_RANGE,
            TOOL_ACKNOWLEDGE_HUMAN_CHANGES,
        ];
        let unique: std::collections::HashSet<_> = names.iter().collect();
        assert_eq!(unique.len(), 22);
    }

    #[test]
    fn build_all_returns_22_tools() {
        let tools = build_all_gdocs_tools();
        assert_eq!(tools.len(), 22);
    }

    #[test]
    fn error_to_json_includes_kind_for_each_variant() {
        let v = error_to_json(DocsError::DocumentNotFound("abc".into()));
        assert_eq!(v["error"], "document_not_found");
        assert_eq!(v["doc_id"], "abc");
        let v = error_to_json(DocsError::TabExists("Notes".into()));
        assert_eq!(v["error"], "tab_exists");
        let v = error_to_json(DocsError::NoParentFolder);
        assert_eq!(v["error"], "no_parent_folder_configured");
    }

    #[test]
    fn parse_share_role_maps_strings() {
        assert!(matches!(
            parse_share_role("reader").unwrap(),
            ShareRole::Reader
        ));
        assert!(matches!(
            parse_share_role("writer").unwrap(),
            ShareRole::Writer
        ));
        assert!(parse_share_role("bogus").is_err());
    }

    #[test]
    fn parse_export_format_maps_strings() {
        assert!(matches!(
            parse_export_format("docx").unwrap(),
            ExportFormat::Docx
        ));
        assert!(matches!(
            parse_export_format("pdf").unwrap(),
            ExportFormat::Pdf
        ));
        assert!(parse_export_format("xyz").is_err());
    }
}
