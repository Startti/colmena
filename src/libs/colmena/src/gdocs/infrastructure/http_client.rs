//! `GoogleDocsHttpClient` — REST adapter implementing [`DocsClient`].
//!
//! Endpoint base URLs:
//!   - `https://docs.googleapis.com/v1/documents/...`
//!   - `https://www.googleapis.com/drive/v3/files/...`
//!   - `https://www.googleapis.com/upload/drive/v3/files`
//!
//! Retry policy: up to `max_retries` (default 3) retries with
//! 1s/2s/4s backoff on 429 + 5xx. 401 surfaces immediately as
//! `AuthFailed`. Other 4xx → typed `DocsError`.
//!
//! THIS FILE IS BUILT INCREMENTALLY. Task 9 adds `get` + snapshot
//! parsing; Tasks 10/11/12 fill in the rest of the trait via
//! `todo!("filled by Task N")` stubs that compile until then.

use crate::gdocs::domain::{
    BatchUpdateResult, CreateFromMarkdownResult, DocsClient, DocsError, DocumentId, DocumentMeta,
    DocumentSnapshot, ExportFormat, LossyConversion, NamedRangeMeta, OutlineEntry, ParagraphKind,
    ParagraphSnapshot, RevisionId, RevisionMeta, Scope, ShareRole, TabId, TabMeta, TabSnapshot,
};
use crate::gdocs::infrastructure::auth::TokenCache;
use crate::gdocs::infrastructure::config::GDocsConfig;
use async_trait::async_trait;
use reqwest::{Client, Method, Response, StatusCode};
use std::sync::Arc;
use std::time::Duration;

/// Hardcoded production base URLs. The test constructor
/// `with_base_urls` lets wiremock intercept them.
const PROD_BASE_DOCS: &str = "https://docs.googleapis.com/v1";
const PROD_BASE_DRIVE: &str = "https://www.googleapis.com/drive/v3";
const PROD_BASE_DRIVE_UPLD: &str = "https://www.googleapis.com/upload/drive/v3";

/// Production REST adapter. Holds a `reqwest::Client`, a shared
/// `TokenCache`, and per-instance base URLs (rebindable in tests).
pub struct GoogleDocsHttpClient {
    cfg: GDocsConfig,
    http: Client,
    pub(crate) tokens: Arc<TokenCache>,
    base_docs: String,
    base_drive: String,
    base_drive_upld: String,
}

impl GoogleDocsHttpClient {
    /// Build a production client from operator config (env-derived).
    ///
    /// Reads OAuth credentials from env via
    /// `OAuthCredentials::from_env`. Any missing variable surfaces as
    /// `DocsError::NotConfigured` carrying the full list of missing
    /// vars — so deploys see one clear error per boot rather than
    /// playing whack-a-mole through them.
    pub fn from_config(cfg: &GDocsConfig) -> Result<Self, DocsError> {
        let http = Client::builder()
            .timeout(cfg.request_timeout)
            .build()
            .map_err(|e| DocsError::Http(e.to_string()))?;
        let creds = crate::google_oauth::infrastructure::OAuthCredentials::from_env()
            .map_err(|e| DocsError::NotConfigured(format!("{e}")))?;
        let tokens = Arc::new(TokenCache::from_oauth_credentials(creds));
        Ok(Self {
            cfg: cfg.clone(),
            http,
            tokens,
            base_docs: PROD_BASE_DOCS.to_string(),
            base_drive: PROD_BASE_DRIVE.to_string(),
            base_drive_upld: PROD_BASE_DRIVE_UPLD.to_string(),
        })
    }

    /// Construct a client whose endpoints point at user-supplied base
    /// URLs. Used by wiremock-based tests; do NOT use in production.
    /// Uses the static (pre-seedable) token cache so tests don't need
    /// to fake `oauth2.googleapis.com`.
    #[cfg(test)]
    pub fn with_base_urls(
        cfg: &GDocsConfig,
        base_docs: impl Into<String>,
        base_drive: impl Into<String>,
        base_drive_upld: impl Into<String>,
    ) -> Result<Self, DocsError> {
        let http = Client::builder()
            .timeout(cfg.request_timeout)
            .build()
            .map_err(|e| DocsError::Http(e.to_string()))?;
        let tokens = Arc::new(TokenCache::for_tests_static());
        Ok(Self {
            cfg: cfg.clone(),
            http,
            tokens,
            base_docs: base_docs.into(),
            base_drive: base_drive.into(),
            base_drive_upld: base_drive_upld.into(),
        })
    }

    async fn bearer(&self) -> Result<String, DocsError> {
        self.tokens.get().await
    }

    /// Send with retry on 429/5xx. The `build_req` closure rebuilds the
    /// request on each retry (so token refreshes apply).
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
                return Ok(resp);
            }
            let backoff = 1u64 << (attempt - 1);
            tokio::time::sleep(Duration::from_secs(backoff)).await;
        }
    }

    /// Map a non-success `Response` to the matching `DocsError`. Reads
    /// the body for context.
    pub(crate) async fn map_status(&self, r: Response, ctx: &str) -> Result<Response, DocsError> {
        if r.status().is_success() {
            return Ok(r);
        }
        let s = r.status();
        let body = r.text().await.unwrap_or_default();
        Err(match s {
            StatusCode::UNAUTHORIZED => DocsError::AuthFailed(format!("{ctx}: 401 {body}")),
            StatusCode::FORBIDDEN => DocsError::PermissionDenied(format!("{ctx}: {body}")),
            StatusCode::NOT_FOUND => DocsError::DocumentNotFound(ctx.into()),
            StatusCode::TOO_MANY_REQUESTS => DocsError::RateLimit(60),
            StatusCode::BAD_REQUEST if body.contains("requiredRevisionId") => DocsError::Conflict,
            _ => DocsError::Http(format!("{ctx}: {s} {body}")),
        })
    }
}

/// Bundle 4A: shape a `drive.comments.get`/list element into our domain
/// `CommentEntry`. Tolerant of missing optional fields (Drive omits
/// `anchor` for doc-wide comments, and `author.emailAddress` is gated by
/// the OAuth user's permission to see it).
fn parse_comment(
    j: &serde_json::Value,
) -> Result<crate::gdocs::domain::types::CommentEntry, DocsError> {
    use crate::gdocs::domain::types::CommentEntry;
    let comment_id = j
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| DocsError::Http("comment missing id".into()))?
        .to_string();
    let content = j
        .get("content")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let created_time = j
        .get("createdTime")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let resolved = j.get("resolved").and_then(|v| v.as_bool()).unwrap_or(false);
    let anchor = j
        .get("anchor")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(String::from);
    let (author_display_name, author_email) = match j.get("author") {
        Some(a) => (
            a.get("displayName")
                .and_then(|v| v.as_str())
                .map(String::from),
            a.get("emailAddress")
                .and_then(|v| v.as_str())
                .map(String::from),
        ),
        None => (None, None),
    };
    Ok(CommentEntry {
        comment_id,
        content,
        created_time,
        resolved,
        anchor,
        author_display_name,
        author_email,
    })
}

fn is_retryable(s: StatusCode) -> bool {
    s == StatusCode::TOO_MANY_REQUESTS || s.is_server_error()
}

// ── Snapshot parser ────────────────────────────────────────────────
// Walks the JSON from `documents.get` and emits a flat sequence of
// `ParagraphSnapshot`s, with paragraph numbering 1-based and
// document-global across all tabs.

pub(crate) fn parse_snapshot(
    j: &serde_json::Value,
    id: &DocumentId,
) -> Result<DocumentSnapshot, DocsError> {
    let title = j
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let revision_id = RevisionId(
        j.get("revisionId")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
    );

    let mut tabs = Vec::new();
    let mut para_counter: u32 = 0;

    if let Some(tabs_json) = j.get("tabs").and_then(|v| v.as_array()) {
        for tab in tabs_json {
            let tab_id = tab
                .pointer("/tabProperties/tabId")
                .and_then(|v| v.as_str())
                .map(|s| TabId(s.into()));
            let body_arr = tab
                .pointer("/documentTab/body/content")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            let paragraphs = parse_paragraphs(&body_arr, &mut para_counter);
            tabs.push(TabSnapshot { tab_id, paragraphs });
        }
    } else if let Some(body) = j.pointer("/body/content").and_then(|v| v.as_array()) {
        // Legacy single-tab doc.
        let paragraphs = parse_paragraphs(body, &mut para_counter);
        tabs.push(TabSnapshot {
            tab_id: None,
            paragraphs,
        });
    }

    Ok(DocumentSnapshot {
        doc_id: id.clone(),
        revision_id,
        title,
        tabs,
    })
}

fn parse_paragraphs(content: &[serde_json::Value], counter: &mut u32) -> Vec<ParagraphSnapshot> {
    let mut out = Vec::new();
    for elem in content {
        if let Some(p) = elem.get("paragraph") {
            *counter += 1;
            let kind = paragraph_kind(p);
            let (text, start, end) = paragraph_text_and_range(elem, p);
            out.push(ParagraphSnapshot {
                n: *counter,
                kind,
                text,
                start_index: start,
                end_index: end,
            });
        }
        // Tables / section breaks / TOC are NOT counted as paragraphs
        // in v1's numbering (consistent with what `read_outline` shows
        // to the agent).
    }
    out
}

fn paragraph_kind(p: &serde_json::Value) -> ParagraphKind {
    let style = p
        .pointer("/paragraphStyle/namedStyleType")
        .and_then(|v| v.as_str())
        .unwrap_or("NORMAL_TEXT");
    if p.get("bullet").is_some() {
        return ParagraphKind::ListItem;
    }
    match style {
        "HEADING_1" => ParagraphKind::Heading1,
        "HEADING_2" => ParagraphKind::Heading2,
        "HEADING_3" => ParagraphKind::Heading3,
        "HEADING_4" => ParagraphKind::Heading4,
        "HEADING_5" => ParagraphKind::Heading5,
        "HEADING_6" => ParagraphKind::Heading6,
        "TITLE" => ParagraphKind::Title,
        "SUBTITLE" => ParagraphKind::Subtitle,
        _ => ParagraphKind::Paragraph,
    }
}

fn paragraph_text_and_range(elem: &serde_json::Value, p: &serde_json::Value) -> (String, u32, u32) {
    let start = elem.get("startIndex").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    let end = elem.get("endIndex").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    let mut text = String::new();
    if let Some(elems) = p.get("elements").and_then(|v| v.as_array()) {
        for e in elems {
            if let Some(t) = e.pointer("/textRun/content").and_then(|v| v.as_str()) {
                text.push_str(t);
            }
        }
    }
    let trimmed = text.trim_end_matches('\n').to_string();
    (trimmed, start, end)
}

/// Recursively flatten Google's nested `tabs[].childTabs[]` structure
/// into a single `Vec<TabMeta>` in pre-order, with `index` reflecting
/// the position in that flat sequence.
fn walk_tabs(
    arr: &[serde_json::Value],
    parent: Option<TabId>,
    out: &mut Vec<TabMeta>,
    idx: &mut u32,
) {
    for t in arr {
        let props = t
            .get("tabProperties")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        let tab_id = TabId(
            props
                .get("tabId")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
        );
        let title = props
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        out.push(TabMeta {
            tab_id: tab_id.clone(),
            title,
            index: *idx,
            parent_tab_id: parent.clone(),
        });
        *idx += 1;
        if let Some(children) = t.get("childTabs").and_then(|v| v.as_array()) {
            walk_tabs(children, Some(tab_id), out, idx);
        }
    }
}

/// Map a (segment-relative) UTF-16 index range to a 1-based paragraph
/// range against the FIRST tab of the snapshot. Multi-tab named ranges
/// are out of scope for v1 — operators can call `read_outline` to
/// inspect named ranges across tabs.
fn map_index_range_to_paragraphs(snap: &DocumentSnapshot, s: u32, e: u32) -> (u32, u32) {
    let mut ps = 0u32;
    let mut pe = 0u32;
    if let Some(tab) = snap.tabs.first() {
        for p in &tab.paragraphs {
            if ps == 0 && p.start_index <= s && s < p.end_index {
                ps = p.n;
            }
            if p.start_index < e && e <= p.end_index {
                pe = p.n;
                break;
            }
        }
    }
    if ps == 0 {
        ps = snap
            .tabs
            .first()
            .and_then(|t| t.paragraphs.first())
            .map(|p| p.n)
            .unwrap_or(1);
    }
    if pe == 0 {
        pe = ps;
    }
    (ps, pe)
}

/// Compare input markdown vs Google's re-export and flag elements
/// present in the input but absent in the export. Conservative — only
/// flags well-known lossy elements (footnotes, image references, math).
fn diff_markdown_for_lossy(orig: &str, after: &str) -> Vec<LossyConversion> {
    let mut out = Vec::new();

    // Footnotes: pattern [^id] in orig, gone in after.
    if let Ok(re) = regex::Regex::new(r"\[\^([^\]]+)\]:?[^\n]*") {
        for m in re.find_iter(orig) {
            if !after.contains(m.as_str()) {
                out.push(LossyConversion {
                    element_type: "footnote".into(),
                    original_markdown: m.as_str().chars().take(200).collect(),
                });
            }
        }
    }

    // Image references: `![alt](url)`.
    if let Ok(re) = regex::Regex::new(r"!\[[^\]]*\]\([^)]+\)") {
        for m in re.find_iter(orig) {
            if !after.contains(m.as_str()) {
                out.push(LossyConversion {
                    element_type: "image_reference".into(),
                    original_markdown: m.as_str().chars().take(200).collect(),
                });
            }
        }
    }

    // Math: `$...$` inline or `$$...$$` block (single-line).
    if let Ok(re) = regex::Regex::new(r"\$\$?[^$\n]+\$\$?") {
        for m in re.find_iter(orig) {
            if !after.contains(m.as_str()) {
                out.push(LossyConversion {
                    element_type: "math_expression".into(),
                    original_markdown: m.as_str().chars().take(200).collect(),
                });
            }
        }
    }

    out
}

// ── DocsClient impl ────────────────────────────────────────────────
//
// Only `get` is real in Task 9. Every other trait method has a
// `todo!("filled by Task N")` body so the impl compiles. Task 10
// implements batch_update/revisions; Task 11 implements the reads;
// Task 12 implements create*/share/export/add_tab/named_range.

#[async_trait]
impl DocsClient for GoogleDocsHttpClient {
    async fn get(&self, id: &DocumentId) -> Result<DocumentSnapshot, DocsError> {
        let url = format!(
            "{}/documents/{}?includeTabsContent=true",
            self.base_docs, id.0
        );
        let resp = self
            .send_with_retry(|c, t| c.request(Method::GET, &url).bearer_auth(t))
            .await?;
        let resp = self.map_status(resp, &format!("docs.get {}", id.0)).await?;
        let j: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| DocsError::Http(format!("docs.get json: {e}")))?;
        parse_snapshot(&j, id)
    }

    async fn create<'a>(
        &self,
        title: &str,
        parent_folder: Option<&'a str>,
    ) -> Result<DocumentMeta, DocsError> {
        // Step 1: create blank via Docs API.
        let resp = self
            .send_with_retry(|c, t| {
                c.request(Method::POST, format!("{}/documents", self.base_docs))
                    .bearer_auth(t)
                    .json(&serde_json::json!({ "title": title }))
            })
            .await?;
        let resp = self.map_status(resp, "documents.create").await?;
        let j: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| DocsError::Http(format!("create json: {e}")))?;
        let doc_id = DocumentId(
            j.get("documentId")
                .and_then(|v| v.as_str())
                .ok_or_else(|| DocsError::Http("missing documentId".into()))?
                .to_string(),
        );

        // Step 2: move to parent_folder (call arg overrides config).
        let folder = parent_folder
            .map(String::from)
            .or_else(|| self.cfg.default_parent_folder.clone())
            .ok_or(DocsError::NoParentFolder)?;
        let url = format!(
            "{}/files/{}?addParents={}&removeParents=root&fields=id,parents",
            self.base_drive, doc_id.0, folder
        );
        let resp = self
            .send_with_retry(|c, t| {
                c.request(Method::PATCH, &url)
                    .bearer_auth(t)
                    .json(&serde_json::json!({}))
            })
            .await?;
        self.map_status(resp, "files.update parent").await?;

        // Step 3: fetch fresh snapshot for revision_id + url + tabs.
        let snap = self.get(&doc_id).await?;
        let tabs = self.list_tabs(&doc_id).await?;
        Ok(DocumentMeta {
            doc_id: snap.doc_id.clone(),
            url: format!("https://docs.google.com/document/d/{}", snap.doc_id.0),
            title: snap.title,
            revision_id: snap.revision_id,
            tabs,
        })
    }

    async fn create_from_markdown<'a>(
        &self,
        title: &str,
        md: &str,
        parent_folder: Option<&'a str>,
    ) -> Result<CreateFromMarkdownResult, DocsError> {
        let folder = parent_folder
            .map(String::from)
            .or_else(|| self.cfg.default_parent_folder.clone())
            .ok_or(DocsError::NoParentFolder)?;

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
            serde_json::to_string(&metadata).expect("valid metadata json")
        );

        let resp = self
            .send_with_retry(|c, t| {
                c.request(
                    Method::POST,
                    format!("{}/files?uploadType=multipart", self.base_drive_upld),
                )
                .bearer_auth(t)
                .header(
                    "Content-Type",
                    format!("multipart/related; boundary={boundary}"),
                )
                .body(body.clone())
            })
            .await?;
        let resp = self.map_status(resp, "create_from_markdown upload").await?;
        let j: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| DocsError::Http(format!("upload json: {e}")))?;
        let doc_id = DocumentId(
            j.get("id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| DocsError::Http("missing id".into()))?
                .to_string(),
        );

        // Re-export as markdown to detect lossy conversions.
        let re_md = self
            .read_as_markdown(&doc_id, None)
            .await
            .unwrap_or_default();
        let lossy = diff_markdown_for_lossy(md, &re_md);

        let snap = self.get(&doc_id).await?;
        let outline = self.read_outline(&doc_id, None).await?;
        let tabs = self.list_tabs(&doc_id).await?;
        Ok(CreateFromMarkdownResult {
            meta: DocumentMeta {
                doc_id: snap.doc_id.clone(),
                url: format!("https://docs.google.com/document/d/{}", snap.doc_id.0),
                title: snap.title,
                revision_id: snap.revision_id,
                tabs,
            },
            outline_snapshot: outline,
            lossy_conversions: lossy,
        })
    }

    async fn create_from_docx<'a>(
        &self,
        title: &str,
        bytes: Vec<u8>,
        parent_folder: Option<&'a str>,
    ) -> Result<DocumentMeta, DocsError> {
        let folder = parent_folder
            .map(String::from)
            .or_else(|| self.cfg.default_parent_folder.clone())
            .ok_or(DocsError::NoParentFolder)?;

        let metadata = serde_json::json!({
            "name": title,
            "mimeType": "application/vnd.google-apps.document",
            "parents": [folder],
        });
        let boundary = "colmena_gdocs_boundary";
        let metadata_part = format!(
            "--{boundary}\r\nContent-Type: application/json; charset=UTF-8\r\n\r\n{}\r\n",
            serde_json::to_string(&metadata).expect("valid metadata json")
        );
        let media_part_header = format!(
            "--{boundary}\r\n\
             Content-Type: application/vnd.openxmlformats-officedocument.wordprocessingml.document\r\n\r\n"
        );
        let trailer = format!("\r\n--{boundary}--");

        let mut body = Vec::new();
        body.extend_from_slice(metadata_part.as_bytes());
        body.extend_from_slice(media_part_header.as_bytes());
        body.extend_from_slice(&bytes);
        body.extend_from_slice(trailer.as_bytes());

        let resp = self
            .send_with_retry(|c, t| {
                c.request(
                    Method::POST,
                    format!("{}/files?uploadType=multipart", self.base_drive_upld),
                )
                .bearer_auth(t)
                .header(
                    "Content-Type",
                    format!("multipart/related; boundary={boundary}"),
                )
                .body(body.clone())
            })
            .await?;
        let resp = self.map_status(resp, "create_from_docx upload").await?;
        let j: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| DocsError::Http(format!("upload json: {e}")))?;
        let doc_id = DocumentId(
            j.get("id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| DocsError::Http("missing id".into()))?
                .to_string(),
        );
        let snap = self.get(&doc_id).await?;
        let tabs = self.list_tabs(&doc_id).await?;
        Ok(DocumentMeta {
            doc_id: snap.doc_id.clone(),
            url: format!("https://docs.google.com/document/d/{}", snap.doc_id.0),
            title: snap.title,
            revision_id: snap.revision_id,
            tabs,
        })
    }

    async fn share(&self, id: &DocumentId, email: &str, role: ShareRole) -> Result<(), DocsError> {
        let url = format!("{}/files/{}/permissions", self.base_drive, id.0);
        let body = serde_json::json!({
            "role": role.as_api_str(),
            "type": "user",
            "emailAddress": email,
        });
        let resp = self
            .send_with_retry(|c, t| c.request(Method::POST, &url).bearer_auth(t).json(&body))
            .await?;
        self.map_status(resp, "permissions.create").await?;
        Ok(())
    }

    async fn list_permissions(
        &self,
        id: &DocumentId,
    ) -> Result<crate::gdocs::domain::types::PermissionList, DocsError> {
        use crate::gdocs::domain::types::{PermissionEntry, PermissionList};
        let url = format!("{}/files/{}/permissions", self.base_drive, id.0);
        let url_for_req = url.clone();
        let fields_for_req =
            "permissions(id,type,role,emailAddress,displayName),nextPageToken".to_string();
        let resp = self
            .send_with_retry(move |c, t| {
                c.request(Method::GET, &url_for_req).bearer_auth(t).query(&[
                    ("fields", fields_for_req.as_str()),
                    ("supportsAllDrives", "true"),
                ])
            })
            .await?;
        let resp = self.map_status(resp, "permissions.list").await?;
        let j: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| DocsError::Http(format!("permissions.list json: {e}")))?;
        let perms = j
            .get("permissions")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let mut out = Vec::with_capacity(perms.len());
        for p in perms {
            let permission_id = p
                .get("id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| DocsError::Http("permission missing id".into()))?
                .to_string();
            let permission_type = p
                .get("type")
                .and_then(|v| v.as_str())
                .unwrap_or("user")
                .to_string();
            let role = p
                .get("role")
                .and_then(|v| v.as_str())
                .unwrap_or("reader")
                .to_string();
            let email = p
                .get("emailAddress")
                .and_then(|v| v.as_str())
                .map(String::from);
            let display_name = p
                .get("displayName")
                .and_then(|v| v.as_str())
                .map(String::from);
            out.push(PermissionEntry {
                permission_id,
                permission_type,
                role,
                email,
                display_name,
            });
        }
        Ok(PermissionList { permissions: out })
    }

    async fn delete_permission(
        &self,
        id: &DocumentId,
        permission_id: &str,
    ) -> Result<(), DocsError> {
        let url = format!(
            "{}/files/{}/permissions/{}",
            self.base_drive, id.0, permission_id
        );
        let resp = self
            .send_with_retry(|c, t| {
                c.request(Method::DELETE, &url)
                    .bearer_auth(t)
                    .query(&[("supportsAllDrives", "true")])
            })
            .await?;
        self.map_status(resp, "permissions.delete").await?;
        Ok(())
    }

    async fn list_documents<'a>(
        &self,
        filter: &crate::gdocs::domain::types::DocumentListFilter<'a>,
    ) -> Result<crate::gdocs::domain::types::DocumentListResult, DocsError> {
        use crate::gdocs::domain::types::{DocumentListItem, DocumentListResult};
        // Build the Drive `q` parameter as `and`-joined predicates.
        let mut q_parts: Vec<String> = vec![
            "mimeType='application/vnd.google-apps.document'".into(),
            "trashed=false".into(),
        ];
        if let Some(query) = filter.query.filter(|s| !s.trim().is_empty()) {
            // Escape single quotes inside the user query so Drive's `q`
            // parser does not break.
            let safe = query.replace('\'', "\\'");
            q_parts.push(format!("name contains '{safe}'"));
        }
        if let Some(folder) = filter.parent_folder_id.filter(|s| !s.is_empty()) {
            let safe = folder.replace('\'', "\\'");
            q_parts.push(format!("'{safe}' in parents"));
        }
        if let Some(after) = filter.modified_after.filter(|s| !s.is_empty()) {
            let safe = after.replace('\'', "\\'");
            q_parts.push(format!("modifiedTime >= '{safe}'"));
        }
        let q = q_parts.join(" and ");
        let limit = filter.limit.unwrap_or(20).clamp(1, 100);
        let url = format!("{}/files", self.base_drive);
        // Fields list (smallest subset that lets us populate
        // DocumentListItem) — Drive bills nothing extra and the response
        // is smaller.
        let fields = "nextPageToken,files(id,name,modifiedTime,owners(emailAddress))";
        let url_for_req = url.clone();
        let q_for_req = q.clone();
        let fields_for_req = fields.to_string();
        let page_token_for_req = filter.page_token.map(String::from);
        let resp = self
            .send_with_retry(move |c, t| {
                let mut req = c.request(Method::GET, &url_for_req).bearer_auth(t).query(&[
                    ("q", q_for_req.as_str()),
                    ("pageSize", &limit.to_string()),
                    ("fields", fields_for_req.as_str()),
                    ("orderBy", "modifiedTime desc"),
                ]);
                if let Some(ref pt) = page_token_for_req {
                    req = req.query(&[("pageToken", pt.as_str())]);
                }
                req
            })
            .await?;
        let resp = self.map_status(resp, "files.list (documents)").await?;
        let j: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| DocsError::Http(format!("files.list json: {e}")))?;

        let files = j
            .get("files")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let mut documents = Vec::with_capacity(files.len());
        for f in files {
            let doc_id = f
                .get("id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| DocsError::Http("file missing id".into()))?
                .to_string();
            let name = f
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("(untitled)")
                .to_string();
            let modified_time = f
                .get("modifiedTime")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let owners: Vec<String> = f
                .get("owners")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|o| {
                            o.get("emailAddress")
                                .and_then(|v| v.as_str())
                                .map(String::from)
                        })
                        .collect()
                })
                .unwrap_or_default();
            documents.push(DocumentListItem {
                doc_id: DocumentId(doc_id.clone()),
                name,
                url: format!("https://docs.google.com/document/d/{doc_id}"),
                modified_time,
                owners,
            });
        }
        let next_page_token = j
            .get("nextPageToken")
            .and_then(|v| v.as_str())
            .map(String::from)
            .filter(|s| !s.is_empty());
        Ok(DocumentListResult {
            documents,
            next_page_token,
        })
    }

    async fn export(&self, id: &DocumentId, format: ExportFormat) -> Result<Vec<u8>, DocsError> {
        // Manually encode the MIME — / and + need percent-encoding.
        // Safe for every MIME in `ExportFormat::mime()` (they only
        // contain `/`, `+`, alphanumeric, `.`, `-`).
        let encoded_mime = format.mime().replace('+', "%2B").replace('/', "%2F");
        let url = format!(
            "{}/files/{}/export?mimeType={}",
            self.base_drive, id.0, encoded_mime
        );
        let resp = self
            .send_with_retry(|c, t| c.request(Method::GET, &url).bearer_auth(t))
            .await?;
        let resp = self
            .map_status(resp, &format!("export {} {}", id.0, format.mime()))
            .await?;
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| DocsError::Http(format!("export bytes: {e}")))?;
        Ok(bytes.to_vec())
    }

    async fn add_comment<'a>(
        &self,
        id: &DocumentId,
        content: &str,
        anchor: Option<&'a str>,
    ) -> Result<crate::gdocs::domain::types::CommentEntry, DocsError> {
        let url = format!("{}/files/{}/comments", self.base_drive, id.0);
        let mut body = serde_json::json!({ "content": content });
        if let Some(a) = anchor {
            body["anchor"] = serde_json::Value::String(a.to_string());
        }
        let fields = "id,content,createdTime,resolved,anchor,author(displayName,emailAddress)";
        let fields_for_req = fields.to_string();
        let resp = self
            .send_with_retry(move |c, t| {
                c.request(Method::POST, &url)
                    .bearer_auth(t)
                    .query(&[("fields", fields_for_req.as_str())])
                    .json(&body)
            })
            .await?;
        let resp = self.map_status(resp, "comments.create").await?;
        let j: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| DocsError::Http(format!("comments.create json: {e}")))?;
        Ok(parse_comment(&j)?)
    }

    async fn list_comments<'a>(
        &self,
        id: &DocumentId,
        filter: &crate::gdocs::domain::types::CommentListFilter<'a>,
    ) -> Result<crate::gdocs::domain::types::CommentList, DocsError> {
        use crate::gdocs::domain::types::CommentList;
        let url = format!("{}/files/{}/comments", self.base_drive, id.0);
        let limit = filter.limit.unwrap_or(20).clamp(1, 100);
        let include_deleted = "false";
        let include_resolved = if filter.include_resolved {
            "true"
        } else {
            "false"
        };
        let fields = "nextPageToken,comments(id,content,createdTime,resolved,anchor,author(displayName,emailAddress))";
        let url_for_req = url.clone();
        let fields_for_req = fields.to_string();
        let page_token_for_req = filter.page_token.map(String::from);
        let resp = self
            .send_with_retry(move |c, t| {
                let mut req = c.request(Method::GET, &url_for_req).bearer_auth(t).query(&[
                    ("pageSize", limit.to_string().as_str()),
                    ("fields", fields_for_req.as_str()),
                    ("includeDeleted", include_deleted),
                    ("filter", include_resolved),
                ]);
                if let Some(ref pt) = page_token_for_req {
                    req = req.query(&[("pageToken", pt.as_str())]);
                }
                req
            })
            .await?;
        let resp = self.map_status(resp, "comments.list").await?;
        let j: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| DocsError::Http(format!("comments.list json: {e}")))?;
        let arr = j
            .get("comments")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let mut comments = Vec::with_capacity(arr.len());
        for c in arr {
            // Drive returns resolved comments only when include_resolved is
            // true; filter again client-side so the LLM never sees them by
            // accident if the operator flips the flag.
            if !filter.include_resolved
                && c.get("resolved").and_then(|v| v.as_bool()).unwrap_or(false)
            {
                continue;
            }
            comments.push(parse_comment(&c)?);
        }
        let next_page_token = j
            .get("nextPageToken")
            .and_then(|v| v.as_str())
            .map(String::from)
            .filter(|s| !s.is_empty());
        Ok(CommentList {
            comments,
            next_page_token,
        })
    }

    async fn resolve_comment<'a>(
        &self,
        id: &DocumentId,
        comment_id: &str,
        content: Option<&'a str>,
    ) -> Result<(), DocsError> {
        // Drive's resolve dance: POST a reply with `action: "resolve"`.
        // Drive flips the parent comment's `resolved` flag to true.
        let url = format!(
            "{}/files/{}/comments/{}/replies",
            self.base_drive, id.0, comment_id
        );
        let body = serde_json::json!({
            "action": "resolve",
            "content": content.unwrap_or(""),
        });
        let resp = self
            .send_with_retry(|c, t| {
                c.request(Method::POST, &url)
                    .bearer_auth(t)
                    .query(&[("fields", "id")])
                    .json(&body)
            })
            .await?;
        self.map_status(resp, "comments.replies.create (resolve)")
            .await?;
        Ok(())
    }

    async fn read_as_markdown<'a>(
        &self,
        id: &DocumentId,
        tab_id: Option<&'a TabId>,
    ) -> Result<String, DocsError> {
        // Drive export → text/markdown. v1 returns the full export; tab
        // slicing is deferred (Drive's export joins all tabs). When the
        // caller wants a single tab's content, the application layer can
        // intersect against the snapshot from `get`.
        let _ = tab_id;
        let url = format!(
            "{}/files/{}/export?mimeType=text/markdown",
            self.base_drive, id.0
        );
        let resp = self
            .send_with_retry(|c, t| c.request(Method::GET, &url).bearer_auth(t))
            .await?;
        let resp = self
            .map_status(resp, &format!("export markdown {}", id.0))
            .await?;
        resp.text()
            .await
            .map_err(|e| DocsError::Http(format!("export text: {e}")))
    }

    async fn read_outline<'a>(
        &self,
        id: &DocumentId,
        tab_id: Option<&'a TabId>,
    ) -> Result<Vec<OutlineEntry>, DocsError> {
        let snap = self.get(id).await?;
        let mut out = Vec::new();
        for tab in &snap.tabs {
            if let Some(want) = tab_id {
                if tab.tab_id.as_ref() != Some(want) {
                    continue;
                }
            }
            for p in &tab.paragraphs {
                let preview: String = p.text.chars().take(80).collect();
                out.push(OutlineEntry {
                    paragraph: p.n,
                    tab_id: tab.tab_id.clone(),
                    kind: p.kind,
                    text_preview: preview,
                });
            }
        }
        Ok(out)
    }

    async fn list_named_ranges(&self, id: &DocumentId) -> Result<Vec<NamedRangeMeta>, DocsError> {
        // Need the snapshot to map index-ranges to paragraph numbers.
        let snap = self.get(id).await?;
        // Drive's `documents.get` with `fields=namedRanges` gives just the
        // named-range payload without the body content — cheaper than
        // re-fetching the full tree.
        let url = format!("{}/documents/{}?fields=namedRanges", self.base_docs, id.0);
        let resp = self
            .send_with_retry(|c, t| c.request(Method::GET, &url).bearer_auth(t))
            .await?;
        let resp = self
            .map_status(resp, &format!("named_ranges {}", id.0))
            .await?;
        let j: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| DocsError::Http(format!("named_ranges json: {e}")))?;
        let mut out = Vec::new();
        if let Some(map) = j.get("namedRanges").and_then(|v| v.as_object()) {
            for (name, group) in map {
                if let Some(ranges) = group.get("namedRanges").and_then(|v| v.as_array()) {
                    for r in ranges {
                        let id_s = r
                            .get("namedRangeId")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let nr = r
                            .get("ranges")
                            .and_then(|v| v.as_array())
                            .and_then(|a| a.first())
                            .cloned()
                            .unwrap_or_default();
                        let start =
                            nr.get("startIndex").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                        let end = nr.get("endIndex").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                        let (ps, pe) = map_index_range_to_paragraphs(&snap, start, end);
                        out.push(NamedRangeMeta {
                            named_range_id: id_s,
                            name: name.clone(),
                            paragraph_start: ps,
                            paragraph_end: pe,
                        });
                    }
                }
            }
        }
        Ok(out)
    }

    async fn list_tabs(&self, id: &DocumentId) -> Result<Vec<TabMeta>, DocsError> {
        // Google's `documents.get?includeTabsContent=false` OMITS the
        // `tabs` field entirely (verified live 2026-06-08), and explicit
        // field masks like `tabs(tabProperties,childTabs)` hit
        // `include_comments` validation rejection. Cleanest path:
        // request the full doc with `includeTabsContent=true` (the
        // default for our `get` impl) and walk the resulting raw JSON
        // for tab metadata. Body content is ignored. Snapshot caching
        // upstream amortises the payload cost.
        let url = format!(
            "{}/documents/{}?includeTabsContent=true",
            self.base_docs, id.0
        );
        let resp = self
            .send_with_retry(|c, t| c.request(Method::GET, &url).bearer_auth(t))
            .await?;
        let resp = self
            .map_status(resp, &format!("list_tabs {}", id.0))
            .await?;
        let j: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| DocsError::Http(format!("list_tabs json: {e}")))?;
        let mut out = Vec::new();
        let mut idx: u32 = 0;
        if let Some(arr) = j.get("tabs").and_then(|v| v.as_array()) {
            walk_tabs(arr, None, &mut out, &mut idx);
        }
        Ok(out)
    }

    async fn list_revisions_since(
        &self,
        id: &DocumentId,
        since: &RevisionId,
    ) -> Result<Vec<RevisionMeta>, DocsError> {
        let url = format!(
            "{}/files/{}/revisions?fields=revisions(id,modifiedTime,lastModifyingUser/emailAddress)",
            self.base_drive, id.0
        );
        let resp = self
            .send_with_retry(|c, t| c.request(Method::GET, &url).bearer_auth(t))
            .await?;
        let resp = self
            .map_status(resp, &format!("revisions.list {}", id.0))
            .await?;
        let j: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| DocsError::Http(format!("revisions.list json: {e}")))?;
        let arr = j
            .get("revisions")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        // Drive returns revisions oldest→newest. Skip everything up to
        // and including `since`; return everything strictly after.
        let mut started = false;
        let mut out = Vec::new();
        for rev in arr {
            let rid = rev
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if !started {
                if rid == since.0 {
                    started = true;
                }
                continue;
            }
            let modified_time = rev
                .get("modifiedTime")
                .and_then(|v| v.as_str())
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|d| d.with_timezone(&chrono::Utc))
                .unwrap_or_else(chrono::Utc::now);
            let modifying_user_email = rev
                .pointer("/lastModifyingUser/emailAddress")
                .and_then(|v| v.as_str())
                .map(String::from);
            out.push(RevisionMeta {
                revision_id: RevisionId(rid),
                modified_time,
                modifying_user_email,
            });
        }
        Ok(out)
    }

    async fn get_at_revision(
        &self,
        id: &DocumentId,
        revision: &RevisionId,
    ) -> Result<String, DocsError> {
        // Drive `revisions.get` with media export to text/plain.
        let url = format!(
            "{}/files/{}/revisions/{}?alt=media",
            self.base_drive, id.0, revision.0
        );
        let resp = self
            .send_with_retry(|c, t| {
                c.request(Method::GET, &url)
                    .bearer_auth(t)
                    .header("Accept", "text/plain")
            })
            .await?;
        let resp = self
            .map_status(resp, &format!("revisions.get {} @ {}", id.0, revision.0))
            .await?;
        resp.text()
            .await
            .map_err(|e| DocsError::Http(format!("revisions.get text: {e}")))
    }

    async fn batch_update<'a>(
        &self,
        id: &DocumentId,
        requests: Vec<serde_json::Value>,
        required_revision: Option<&'a RevisionId>,
    ) -> Result<BatchUpdateResult, DocsError> {
        let url = format!("{}/documents/{}:batchUpdate", self.base_docs, id.0);
        let mut body = serde_json::json!({ "requests": requests });
        if let Some(rev) = required_revision {
            body["writeControl"] = serde_json::json!({ "requiredRevisionId": rev.0 });
        }
        let resp = self
            .send_with_retry(|c, t| c.request(Method::POST, &url).bearer_auth(t).json(&body))
            .await?;
        let resp = self
            .map_status(resp, &format!("batchUpdate {}", id.0))
            .await?;
        let j: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| DocsError::Http(format!("batchUpdate json: {e}")))?;

        // Try to recover the new revisionId from the response. Google
        // sometimes echoes it under writeControl.requiredRevisionId; if
        // not, fall back to a fresh GET.
        let echoed = j
            .get("writeControl")
            .and_then(|v| v.get("requiredRevisionId"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let revision_id_after = if echoed.is_empty() {
            self.get(id).await?.revision_id
        } else {
            RevisionId(echoed)
        };
        let replies = j
            .get("replies")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        Ok(BatchUpdateResult {
            revision_id_after,
            replies,
        })
    }

    async fn add_tab<'a>(
        &self,
        id: &DocumentId,
        title: &str,
        after_tab: Option<&'a TabId>,
    ) -> Result<TabMeta, DocsError> {
        // Pre-check: tab with this title already exists?
        let existing = self.list_tabs(id).await?;
        if existing.iter().any(|t| t.title == title) {
            return Err(DocsError::TabExists(title.into()));
        }

        let mut props = serde_json::json!({ "title": title });
        if let Some(after) = after_tab {
            props["indexAfter"] = serde_json::json!(after.0);
        }
        let requests = vec![serde_json::json!({
            "addDocumentTab": { "tabProperties": props }
        })];
        self.batch_update(id, requests, None).await?;

        // The batchUpdate reply doesn't reliably echo the new tab id, so
        // re-list and find the one with our title.
        let after_list = self.list_tabs(id).await?;
        after_list
            .into_iter()
            .find(|t| t.title == title)
            .ok_or_else(|| DocsError::Internal("add_tab: new tab not found after list".into()))
    }

    async fn create_named_range(
        &self,
        id: &DocumentId,
        name: &str,
        scope: Scope,
    ) -> Result<NamedRangeMeta, DocsError> {
        // v1 supports Scope::Paragraph { n } at this trait level — other
        // scopes must be resolved by the application layer first.
        let n = match scope {
            Scope::Paragraph { n } => n,
            _ => {
                return Err(DocsError::InvalidArgs(
                    "create_named_range scope must be {kind:\"paragraph\", n: <N>}".into(),
                ))
            }
        };
        let snap = self.get(id).await?;
        let p = snap
            .tabs
            .iter()
            .flat_map(|t| t.paragraphs.iter())
            .find(|p| p.n == n)
            .ok_or_else(|| DocsError::InvalidArgs(format!("paragraph {n} not found")))?;
        let requests = vec![serde_json::json!({
            "createNamedRange": {
                "name": name,
                "range": {"startIndex": p.start_index, "endIndex": p.end_index}
            }
        })];
        self.batch_update(id, requests, Some(&snap.revision_id))
            .await?;
        let listed = self.list_named_ranges(id).await?;
        listed
            .into_iter()
            .find(|nr| nr.name == name)
            .ok_or_else(|| DocsError::Internal("named_range not found after create".into()))
    }
}

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
    fn parse_snapshot_multi_tab_global_paragraph_numbering() {
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

    #[test]
    fn parse_snapshot_bullet_becomes_list_item() {
        let j = json!({
            "title": "Bullet",
            "revisionId": "r3",
            "body": {"content": [
                {"startIndex": 1, "endIndex": 8,
                 "paragraph": {
                    "elements": [{"textRun": {"content": "Item\n"}}],
                    "bullet": {"listId": "L1"}
                 }}
            ]}
        });
        let snap = parse_snapshot(&j, &DocumentId("d3".into())).unwrap();
        assert_eq!(snap.tabs[0].paragraphs[0].kind, ParagraphKind::ListItem);
    }

    use wiremock::matchers::{header_exists, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn test_cfg() -> GDocsConfig {
        GDocsConfig {
            scopes: vec!["https://www.googleapis.com/auth/documents".to_string()],
            default_parent_folder: None,
            request_timeout: Duration::from_secs(5),
            max_retries: 0,
            revision_cache_ttl: Duration::from_secs(5),
            share_email: String::new(),
        }
    }

    fn client_for(server: &MockServer) -> GoogleDocsHttpClient {
        let cfg = test_cfg();
        let base = server.uri();
        GoogleDocsHttpClient::with_base_urls(
            &cfg,
            format!("{}/docs", base),
            format!("{}/drive", base),
            format!("{}/upload", base),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn batch_update_uses_required_revision() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/docs/documents/doc1:batchUpdate"))
            .and(header_exists("authorization"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "writeControl": {"requiredRevisionId": "rev_after"},
                "replies": []
            })))
            .expect(1)
            .mount(&server)
            .await;

        let client = client_for(&server);
        client.tokens.set_token_for_test("fake".to_string()).await;

        let result = client
            .batch_update(
                &DocumentId("doc1".into()),
                vec![json!({"insertText": {"location": {"index": 1}, "text": "hi"}})],
                Some(&RevisionId("rev_before".into())),
            )
            .await
            .unwrap();
        assert_eq!(result.revision_id_after.0, "rev_after");
    }

    #[tokio::test]
    async fn list_revisions_returns_only_after_since() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/drive/files/doc1/revisions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "revisions": [
                    {"id": "r1", "modifiedTime": "2026-06-08T10:00:00Z",
                     "lastModifyingUser": {"emailAddress": "alice@example.com"}},
                    {"id": "r2", "modifiedTime": "2026-06-08T11:00:00Z",
                     "lastModifyingUser": {"emailAddress": "bob@example.com"}},
                    {"id": "r3", "modifiedTime": "2026-06-08T12:00:00Z",
                     "lastModifyingUser": {"emailAddress": "carol@example.com"}},
                ]
            })))
            .mount(&server)
            .await;

        let client = client_for(&server);
        client.tokens.set_token_for_test("fake".to_string()).await;

        let revs = client
            .list_revisions_since(&DocumentId("doc1".into()), &RevisionId("r1".into()))
            .await
            .unwrap();
        assert_eq!(revs.len(), 2);
        assert_eq!(revs[0].revision_id.0, "r2");
        assert_eq!(revs[1].revision_id.0, "r3");
        assert_eq!(
            revs[1].modifying_user_email.as_deref(),
            Some("carol@example.com")
        );
    }

    #[tokio::test]
    async fn get_at_revision_returns_text() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/drive/files/doc1/revisions/rev_5"))
            .respond_with(ResponseTemplate::new(200).set_body_string("Hello\nworld\n"))
            .mount(&server)
            .await;

        let client = client_for(&server);
        client.tokens.set_token_for_test("fake".to_string()).await;

        let txt = client
            .get_at_revision(&DocumentId("doc1".into()), &RevisionId("rev_5".into()))
            .await
            .unwrap();
        assert_eq!(txt, "Hello\nworld\n");
    }

    #[tokio::test]
    async fn read_as_markdown_returns_export() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/drive/files/doc1/export"))
            .respond_with(ResponseTemplate::new(200).set_body_string("# Hello\n\nWorld\n"))
            .mount(&server)
            .await;
        let client = client_for(&server);
        client.tokens.set_token_for_test("fake".to_string()).await;
        let md = client
            .read_as_markdown(&DocumentId("doc1".into()), None)
            .await
            .unwrap();
        assert!(md.contains("Hello"));
    }

    #[tokio::test]
    async fn read_outline_filters_by_tab() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/docs/documents/doc1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "title": "t", "revisionId": "r",
                "tabs": [
                    {"tabProperties": {"tabId": "t1", "title": "A"},
                     "documentTab": {"body": {"content": [
                        {"startIndex": 1, "endIndex": 5,
                         "paragraph": {"elements": [{"textRun": {"content": "in A\n"}}],
                                       "paragraphStyle": {"namedStyleType": "HEADING_1"}}}
                     ]}}},
                    {"tabProperties": {"tabId": "t2", "title": "B"},
                     "documentTab": {"body": {"content": [
                        {"startIndex": 1, "endIndex": 5,
                         "paragraph": {"elements": [{"textRun": {"content": "in B\n"}}]}}
                     ]}}}
                ]
            })))
            .mount(&server)
            .await;
        let client = client_for(&server);
        client.tokens.set_token_for_test("fake".to_string()).await;
        let only_a = client
            .read_outline(&DocumentId("doc1".into()), Some(&TabId("t1".into())))
            .await
            .unwrap();
        assert_eq!(only_a.len(), 1);
        assert_eq!(only_a[0].kind, ParagraphKind::Heading1);
        assert_eq!(only_a[0].text_preview, "in A");

        let all = client
            .read_outline(&DocumentId("doc1".into()), None)
            .await
            .unwrap();
        assert_eq!(all.len(), 2);
    }

    #[tokio::test]
    async fn list_tabs_flattens_child_tabs() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/docs/documents/doc1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "title": "t", "revisionId": "r",
                "tabs": [
                    {"tabProperties": {"tabId": "t1", "title": "Top1"},
                     "childTabs": [
                        {"tabProperties": {"tabId": "t1a", "title": "Top1.A"}}
                     ]},
                    {"tabProperties": {"tabId": "t2", "title": "Top2"}}
                ]
            })))
            .mount(&server)
            .await;
        let client = client_for(&server);
        client.tokens.set_token_for_test("fake".to_string()).await;
        let tabs = client.list_tabs(&DocumentId("doc1".into())).await.unwrap();
        assert_eq!(tabs.len(), 3);
        assert_eq!(tabs[0].tab_id.0, "t1");
        assert!(tabs[0].parent_tab_id.is_none());
        assert_eq!(tabs[1].tab_id.0, "t1a");
        assert_eq!(tabs[1].parent_tab_id.as_ref().unwrap().0, "t1");
        assert_eq!(tabs[2].tab_id.0, "t2");
    }

    #[tokio::test]
    async fn create_uses_configured_parent_folder() {
        let server = MockServer::start().await;
        // 1) POST /docs/documents (create blank)
        Mock::given(method("POST"))
            .and(path("/docs/documents"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "documentId": "new_doc"
            })))
            .mount(&server)
            .await;
        // 2) PATCH /drive/files/new_doc?addParents=...
        Mock::given(method("PATCH"))
            .and(path("/drive/files/new_doc"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .mount(&server)
            .await;
        // 3) GET /docs/documents/new_doc (snapshot + list_tabs)
        Mock::given(method("GET"))
            .and(path("/docs/documents/new_doc"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "title": "Hello", "revisionId": "r1",
                "body": {"content": []},
                "tabs": []
            })))
            .mount(&server)
            .await;

        let mut cfg = test_cfg();
        cfg.default_parent_folder = Some("folder123".into());
        let base = server.uri();
        let client = GoogleDocsHttpClient::with_base_urls(
            &cfg,
            format!("{}/docs", base),
            format!("{}/drive", base),
            format!("{}/upload", base),
        )
        .unwrap();
        client.tokens.set_token_for_test("fake".to_string()).await;

        let meta = client.create("Hello", None).await.unwrap();
        assert_eq!(meta.doc_id.0, "new_doc");
        assert!(meta.url.contains("new_doc"));
    }

    #[tokio::test]
    async fn create_without_folder_errors() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/docs/documents"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "documentId": "new_doc"
            })))
            .mount(&server)
            .await;
        let client = client_for(&server);
        client.tokens.set_token_for_test("fake".to_string()).await;
        let err = client.create("Hello", None).await.unwrap_err();
        assert!(matches!(err, DocsError::NoParentFolder));
    }

    #[tokio::test]
    async fn share_posts_permission() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/drive/files/doc1/permissions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": "perm1"})))
            .expect(1)
            .mount(&server)
            .await;
        let client = client_for(&server);
        client.tokens.set_token_for_test("fake".to_string()).await;
        client
            .share(
                &DocumentId("doc1".into()),
                "user@example.com",
                ShareRole::Writer,
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn export_returns_bytes() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/drive/files/doc1/export"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"%PDF-1.4 fake".to_vec()))
            .mount(&server)
            .await;
        let client = client_for(&server);
        client.tokens.set_token_for_test("fake".to_string()).await;
        let bytes = client
            .export(&DocumentId("doc1".into()), ExportFormat::Pdf)
            .await
            .unwrap();
        assert!(bytes.starts_with(b"%PDF"));
    }

    #[tokio::test]
    async fn add_tab_fails_when_title_exists() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/docs/documents/doc1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "title": "T", "revisionId": "r",
                "tabs": [{"tabProperties": {"tabId": "t1", "title": "Sales"}}]
            })))
            .mount(&server)
            .await;
        let client = client_for(&server);
        client.tokens.set_token_for_test("fake".to_string()).await;
        let err = client
            .add_tab(&DocumentId("doc1".into()), "Sales", None)
            .await
            .unwrap_err();
        assert!(matches!(err, DocsError::TabExists(_)));
    }
}
