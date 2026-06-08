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
    DocumentSnapshot, ExportFormat, NamedRangeMeta, OutlineEntry, ParagraphKind, ParagraphSnapshot,
    RevisionId, RevisionMeta, Scope, ShareRole, TabId, TabMeta, TabSnapshot,
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
    #[allow(dead_code)] // populated for Task 12 (resumable upload)
    base_drive_upld: String,
}

impl GoogleDocsHttpClient {
    /// Build a production client from operator config (env-derived).
    pub fn from_config(cfg: &GDocsConfig) -> Result<Self, DocsError> {
        let http = Client::builder()
            .timeout(cfg.request_timeout)
            .build()
            .map_err(|e| DocsError::Http(e.to_string()))?;
        let tokens = Arc::new(TokenCache::new(cfg.scopes.clone()));
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
        let tokens = Arc::new(TokenCache::new(cfg.scopes.clone()));
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

fn paragraph_text_and_range(
    elem: &serde_json::Value,
    p: &serde_json::Value,
) -> (String, u32, u32) {
    let start = elem
        .get("startIndex")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;
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

    async fn create(
        &self,
        _title: &str,
        _parent_folder: Option<&str>,
    ) -> Result<DocumentMeta, DocsError> {
        todo!("filled by Task 12")
    }

    async fn create_from_markdown(
        &self,
        _title: &str,
        _md: &str,
        _parent_folder: Option<&str>,
    ) -> Result<CreateFromMarkdownResult, DocsError> {
        todo!("filled by Task 12")
    }

    async fn create_from_docx(
        &self,
        _title: &str,
        _bytes: Vec<u8>,
        _parent_folder: Option<&str>,
    ) -> Result<DocumentMeta, DocsError> {
        todo!("filled by Task 12")
    }

    async fn share(
        &self,
        _id: &DocumentId,
        _email: &str,
        _role: ShareRole,
    ) -> Result<(), DocsError> {
        todo!("filled by Task 12")
    }

    async fn export(
        &self,
        _id: &DocumentId,
        _format: ExportFormat,
    ) -> Result<Vec<u8>, DocsError> {
        todo!("filled by Task 12")
    }

    async fn read_as_markdown(
        &self,
        _id: &DocumentId,
        _tab_id: Option<&TabId>,
    ) -> Result<String, DocsError> {
        todo!("filled by Task 11")
    }

    async fn read_outline(
        &self,
        _id: &DocumentId,
        _tab_id: Option<&TabId>,
    ) -> Result<Vec<OutlineEntry>, DocsError> {
        todo!("filled by Task 11")
    }

    async fn list_named_ranges(
        &self,
        _id: &DocumentId,
    ) -> Result<Vec<NamedRangeMeta>, DocsError> {
        todo!("filled by Task 11")
    }

    async fn list_tabs(&self, _id: &DocumentId) -> Result<Vec<TabMeta>, DocsError> {
        todo!("filled by Task 11")
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

    async fn batch_update(
        &self,
        id: &DocumentId,
        requests: Vec<serde_json::Value>,
        required_revision: Option<&RevisionId>,
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

    async fn add_tab(
        &self,
        _id: &DocumentId,
        _title: &str,
        _after_tab: Option<&TabId>,
    ) -> Result<TabMeta, DocsError> {
        todo!("filled by Task 12")
    }

    async fn create_named_range(
        &self,
        _id: &DocumentId,
        _name: &str,
        _scope: Scope,
    ) -> Result<NamedRangeMeta, DocsError> {
        todo!("filled by Task 12")
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
            credentials_path: None,
            scopes: vec!["https://www.googleapis.com/auth/documents".to_string()],
            default_parent_folder: None,
            request_timeout: Duration::from_secs(5),
            max_retries: 0,
            revision_cache_ttl: Duration::from_secs(5),
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
}
