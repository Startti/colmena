//! REST adapter implementing [`SearchPort`] against the Tavily API
//! (https://docs.tavily.com). Uses `reqwest` for transport.
//!
//! Endpoints used:
//! - `POST /search`  — web search (optionally includes extracted content).
//! - `POST /extract` — read a specific URL.
//!
//! Status-code mapping (see spec §"Infrastructure"):
//!   | Upstream           | Domain error                  |
//!   |--------------------|-------------------------------|
//!   | 200                | Ok(...)                        |
//!   | 401 / 403          | `WebDomainError::AdapterInit` |
//!   | 403, block page    | `WebDomainError::Upstream`     |
//!   | 429                | `WebDomainError::RateLimit`    |
//!   | 5xx                | `WebDomainError::Upstream`     |
//!   | transport timeout  | `WebDomainError::Timeout`      |
//!   | other transport    | `WebDomainError::Upstream`     |
//!
//! A 403 whose body is an HTML page (or names nginx) is the network being
//! refused at the edge, not the key being rejected: it is reported as a
//! recoverable `Upstream` error with a fixed message, without the page.

use crate::web::domain::errors::WebDomainError;
#[allow(unused_imports)]
use crate::web::domain::search_port::{
    ExtractFormat, FetchRequest, FetchResponse, SearchDepth, SearchPort, SearchRequest,
    SearchResponse, SearchResult,
};
use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};
use std::time::Duration;

const DEFAULT_BASE_URL: &str = "https://api.tavily.com";

/// Longest prefix of an upstream body that goes into an error message.
const MAX_ERROR_BODY_CHARS: usize = 200;

/// What the model reads when the provider answers 403 with a block page. The
/// page itself is never included: it is noise, and says nothing about the key.
const BLOCK_PAGE_MESSAGE: &str = "The search provider refused this request from the current \
    network with a block page (HTTP 403). This is not a problem with the API key. It usually \
    clears within a few minutes; try again later.";

/// True when a 403 body is an HTML page from the provider's edge (a block page)
/// rather than an API answer: it starts with `<` and holds an `<html` or
/// `<!doctype html` marker, or it names nginx.
fn looks_like_block_page(body: &str) -> bool {
    let lower = body.trim().to_lowercase();
    (lower.starts_with('<') && (lower.contains("<html") || lower.contains("<!doctype html")))
        || lower.contains("nginx")
}

/// The first [`MAX_ERROR_BODY_CHARS`] characters of `body`, with `...` when cut.
fn truncate_body(body: &str) -> String {
    match body.char_indices().nth(MAX_ERROR_BODY_CHARS) {
        Some((cut, _)) => format!("{}...", &body[..cut]),
        None => body.to_string(),
    }
}

#[derive(Debug)]
pub struct TavilyAdapter {
    client: Client,
    api_key: String,
    base_url: String,
}

impl TavilyAdapter {
    /// Build a new adapter. `timeout` is applied per-request by reqwest.
    pub fn new(api_key: impl Into<String>, timeout: Duration) -> Result<Self, WebDomainError> {
        let api_key = api_key.into();
        if api_key.trim().is_empty() {
            return Err(WebDomainError::AdapterInit(
                "Tavily api_key is empty".to_string(),
            ));
        }
        let client = crate::shared::http_client::builder()
            .timeout(timeout)
            .build()
            .map_err(|e| WebDomainError::AdapterInit(format!("reqwest build: {e}")))?;
        Ok(Self {
            client,
            api_key,
            base_url: DEFAULT_BASE_URL.to_string(),
        })
    }

    /// Test helper: override the base URL (for wiremock).
    #[cfg(test)]
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    /// Map an HTTP response (after reading body) to `WebDomainError` for non-2xx.
    fn map_error(status: u16, body: String) -> WebDomainError {
        match status {
            403 if looks_like_block_page(&body) => WebDomainError::Upstream {
                status,
                body: BLOCK_PAGE_MESSAGE.to_string(),
            },
            401 | 403 => WebDomainError::AdapterInit(format!(
                "Tavily auth failed (status {status}): {}",
                truncate_body(&body)
            )),
            429 => WebDomainError::RateLimit {
                calls_used: 0,
                cap: 0,
            },
            s if (500..600).contains(&s) => WebDomainError::Upstream { status: s, body },
            s => WebDomainError::Upstream { status: s, body },
        }
    }

    /// Map a `reqwest::Error` (transport / timeout) to `WebDomainError`.
    fn map_transport_error(err: reqwest::Error) -> WebDomainError {
        if err.is_timeout() {
            WebDomainError::Timeout { ms: 0 }
        } else {
            WebDomainError::Upstream {
                status: 0,
                body: err.to_string(),
            }
        }
    }
}

#[async_trait]
impl SearchPort for TavilyAdapter {
    async fn search(&self, req: SearchRequest) -> Result<SearchResponse, WebDomainError> {
        let mut body = json!({
            "api_key": self.api_key,
            "query": req.query,
            "search_depth": req.search_depth.as_str(),
            "max_results": req.max_results,
            "include_answer": true,
            "include_raw_content": req.include_content,
        });

        if !req.include_domains.is_empty() {
            body["include_domains"] = json!(req.include_domains);
        }
        if !req.exclude_domains.is_empty() {
            body["exclude_domains"] = json!(req.exclude_domains);
        }
        if let Some(range) = req.time_range {
            body["time_range"] = json!(range.as_str());
        }

        let url = format!("{}/search", self.base_url);
        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(Self::map_transport_error)?;

        let status = resp.status().as_u16();
        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(Self::map_error(status, body));
        }
        let raw: Value = resp.json().await.map_err(|e| WebDomainError::Upstream {
            status: 200,
            body: format!("invalid JSON from Tavily /search: {e}"),
        })?;

        let results = raw
            .get("results")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|item| SearchResult {
                title: item
                    .get("title")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                url: item
                    .get("url")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                snippet: item
                    .get("content")
                    .and_then(|v| v.as_str())
                    .map(|s| s.chars().take(400).collect())
                    .unwrap_or_default(),
                score: item.get("score").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32,
                content: if req.include_content {
                    item.get("content")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                } else {
                    None
                },
            })
            .collect::<Vec<_>>();

        Ok(SearchResponse {
            query: raw
                .get("query")
                .and_then(|v| v.as_str())
                .unwrap_or(&req.query)
                .to_string(),
            results,
            answer: raw
                .get("answer")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            credits_used: if matches!(req.search_depth, SearchDepth::Advanced) {
                2
            } else {
                1
            },
        })
    }

    async fn fetch(&self, req: FetchRequest) -> Result<FetchResponse, WebDomainError> {
        let body = json!({
            "api_key": self.api_key,
            "urls": [req.url],
            "extract_depth": "basic",
            "format": req.format.as_str(),
        });

        let url = format!("{}/extract", self.base_url);
        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(Self::map_transport_error)?;

        let status = resp.status().as_u16();
        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(Self::map_error(status, body));
        }
        let raw: Value = resp.json().await.map_err(|e| WebDomainError::Upstream {
            status: 200,
            body: format!("invalid JSON from Tavily /extract: {e}"),
        })?;

        // Tavily returns results + failed_results arrays. We requested one URL.
        if let Some(failed) = raw
            .get("failed_results")
            .and_then(|v| v.as_array())
            .filter(|a| !a.is_empty())
        {
            let msg = failed
                .first()
                .and_then(|v| v.get("error"))
                .and_then(|v| v.as_str())
                .unwrap_or("extract failed")
                .to_string();
            return Err(WebDomainError::Upstream {
                status: 200,
                body: msg,
            });
        }

        let results = raw
            .get("results")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let first = results
            .into_iter()
            .next()
            .ok_or_else(|| WebDomainError::Upstream {
                status: 200,
                body: "empty results from /extract".into(),
            })?;
        let content = first
            .get("raw_content")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let title = first
            .get("title")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let resolved_url = first
            .get("url")
            .and_then(|v| v.as_str())
            .unwrap_or(&req.url)
            .to_string();
        let content_length = content.len() as u64;
        Ok(FetchResponse {
            url: resolved_url,
            title,
            content,
            content_length,
            credits_used: 1,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty_api_key() {
        let err = TavilyAdapter::new("", Duration::from_secs(5)).unwrap_err();
        assert!(matches!(err, WebDomainError::AdapterInit(_)));
    }

    #[test]
    fn accepts_nonempty_api_key() {
        let a = TavilyAdapter::new("tvly-xxx", Duration::from_secs(5)).unwrap();
        assert_eq!(a.api_key, "tvly-xxx");
        assert_eq!(a.base_url, DEFAULT_BASE_URL);
    }

    #[test]
    fn map_error_429_is_rate_limit() {
        let e = TavilyAdapter::map_error(429, "too many".into());
        assert!(matches!(e, WebDomainError::RateLimit { .. }));
    }

    #[test]
    fn map_error_401_is_adapter_init() {
        let e = TavilyAdapter::map_error(401, "nope".into());
        assert!(matches!(e, WebDomainError::AdapterInit(_)));
    }

    #[test]
    fn map_error_403_is_adapter_init() {
        let e = TavilyAdapter::map_error(403, "nope".into());
        assert!(matches!(e, WebDomainError::AdapterInit(_)));
    }

    #[test]
    fn map_error_502_is_upstream() {
        let e = TavilyAdapter::map_error(502, "bad gw".into());
        assert!(matches!(e, WebDomainError::Upstream { status: 502, .. }));
    }

    #[test]
    fn map_error_418_is_upstream() {
        // Non-standard statuses should still round-trip as Upstream so callers can log them.
        let e = TavilyAdapter::map_error(418, "teapot".into());
        assert!(matches!(e, WebDomainError::Upstream { status: 418, .. }));
    }

    const NGINX_BLOCK_PAGE: &str = "<html>\r\n<head><title>403 Forbidden</title></head>\r\n<body>\r\n\
        <center><h1>403 Forbidden</h1></center>\r\n<hr><center>nginx</center>\r\n</body>\r\n</html>";

    /// The text of a `WebDomainError::Upstream`, or a panic naming what came back.
    fn upstream_text(e: WebDomainError) -> (u16, String) {
        match e {
            WebDomainError::Upstream { status, body } => (status, body),
            other => panic!("expected Upstream, got {other:?}"),
        }
    }

    fn auth_text(e: WebDomainError) -> String {
        match e {
            WebDomainError::AdapterInit(m) => m,
            other => panic!("expected AdapterInit, got {other:?}"),
        }
    }

    #[test]
    fn map_error_403_html_block_page_is_a_readable_upstream_error() {
        let e = TavilyAdapter::map_error(403, NGINX_BLOCK_PAGE.into());
        assert!(e.is_llm_recoverable(), "the model must get to read it");
        let (status, text) = upstream_text(e);
        assert_eq!(status, 403);
        assert!(text.contains("block page"), "{text}");
        assert!(text.contains("not a problem with the API key"), "{text}");
        assert!(text.contains("few minutes"), "{text}");
        assert!(text.contains("try again later"), "{text}");
    }

    #[test]
    fn map_error_403_block_page_text_carries_none_of_the_page() {
        let (_, text) = upstream_text(TavilyAdapter::map_error(403, NGINX_BLOCK_PAGE.into()));
        for leaked in ["<", ">", "nginx", "center", "Forbidden"] {
            assert!(!text.contains(leaked), "{leaked:?} leaked into: {text}");
        }
    }

    #[test]
    fn map_error_403_block_page_is_recognised_ignoring_case_and_padding() {
        let page = "\n  <!DOCTYPE HTML>\n<HTML><BODY>Access denied</BODY></HTML>\n";
        let (status, text) = upstream_text(TavilyAdapter::map_error(403, page.into()));
        assert_eq!(status, 403);
        assert!(text.contains("block page"), "{text}");
    }

    #[test]
    fn map_error_403_html_tag_page_without_a_doctype_or_nginx_is_a_block_page() {
        let page = "<html><body>Access denied</body></html>";
        assert!(matches!(
            TavilyAdapter::map_error(403, page.into()),
            WebDomainError::Upstream { status: 403, .. }
        ));
    }

    #[test]
    fn map_error_403_doctype_page_without_an_html_tag_is_a_block_page() {
        let page = "<!doctype html>\n<title>Blocked</title>";
        assert!(matches!(
            TavilyAdapter::map_error(403, page.into()),
            WebDomainError::Upstream { status: 403, .. }
        ));
    }

    #[test]
    fn map_error_403_plain_text_naming_nginx_is_a_block_page() {
        let (_, text) = upstream_text(TavilyAdapter::map_error(
            403,
            "403 Forbidden - NGINX/1.24".into(),
        ));
        assert!(text.contains("block page"), "{text}");
        assert!(!text.to_lowercase().contains("nginx"), "{text}");
    }

    #[test]
    fn map_error_403_json_body_is_still_an_auth_error() {
        let body = r#"{"detail":{"error":"Unauthorized: missing or invalid API key."}}"#;
        let msg = auth_text(TavilyAdapter::map_error(403, body.into()));
        assert!(msg.contains("Tavily auth failed (status 403)"), "{msg}");
        assert!(msg.contains("invalid API key"), "{msg}");
    }

    #[test]
    fn map_error_403_json_that_merely_contains_html_is_still_an_auth_error() {
        // Not a page: the body does not start with `<`, and it never names nginx.
        let body = r#"{"detail":"bad key <html> in header"}"#;
        assert!(matches!(
            TavilyAdapter::map_error(403, body.into()),
            WebDomainError::AdapterInit(_)
        ));
    }

    #[test]
    fn map_error_401_is_an_auth_error_with_its_status_and_body() {
        let msg = auth_text(TavilyAdapter::map_error(401, "bad key".into()));
        assert!(msg.contains("Tavily auth failed (status 401)"), "{msg}");
        assert!(msg.contains("bad key"), "{msg}");
    }

    #[test]
    fn map_error_401_with_an_html_body_stays_an_auth_error() {
        // The block page is a 403 phenomenon; a 401 keeps meaning "the key".
        assert!(matches!(
            TavilyAdapter::map_error(401, NGINX_BLOCK_PAGE.into()),
            WebDomainError::AdapterInit(_)
        ));
    }

    #[test]
    fn map_error_auth_body_is_cut_to_a_short_prefix() {
        let body = "x".repeat(5_000);
        for status in [401, 403] {
            let msg = auth_text(TavilyAdapter::map_error(status, body.clone()));
            assert!(msg.contains(&"x".repeat(MAX_ERROR_BODY_CHARS)), "{status}");
            assert!(
                !msg.contains(&"x".repeat(MAX_ERROR_BODY_CHARS + 1)),
                "{status}"
            );
            assert!(msg.ends_with("..."), "{msg}");
            assert!(msg.len() < 300, "{status}: {} bytes", msg.len());
        }
    }

    #[test]
    fn map_error_auth_body_at_the_limit_is_kept_whole() {
        let body = "y".repeat(MAX_ERROR_BODY_CHARS);
        let msg = auth_text(TavilyAdapter::map_error(401, body.clone()));
        assert!(msg.ends_with(&body), "{msg}");
        assert!(!msg.ends_with("..."), "{msg}");
    }

    #[test]
    fn map_error_auth_body_is_cut_on_a_character_boundary() {
        // 2-byte characters: a byte cut at 200 would land inside one and panic.
        let body = "é".repeat(MAX_ERROR_BODY_CHARS + 50);
        let msg = auth_text(TavilyAdapter::map_error(403, body));
        assert!(msg.contains(&"é".repeat(MAX_ERROR_BODY_CHARS)), "{msg}");
        assert!(
            !msg.contains(&"é".repeat(MAX_ERROR_BODY_CHARS + 1)),
            "{msg}"
        );
    }

    use crate::web::domain::search_port::{SearchRequest as SReq, TimeRange};
    use wiremock::matchers::{body_partial_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn fast_adapter(url: &str) -> TavilyAdapter {
        TavilyAdapter::new("tvly-test", Duration::from_secs(5))
            .unwrap()
            .with_base_url(url)
    }

    #[tokio::test]
    async fn search_happy_path_returns_results() {
        let server = MockServer::start().await;
        let body = serde_json::json!({
            "query": "rust async",
            "answer": "Rust has async/await since 1.39.",
            "results": [
                {
                    "title": "Rust Async Book",
                    "url": "https://rust-lang.github.io/async-book/",
                    "content": "Full content...",
                    "score": 0.92
                },
                {
                    "title": "Async Rust",
                    "url": "https://example.com/a",
                    "content": "Snippet only",
                    "score": 0.80
                }
            ]
        });
        Mock::given(method("POST"))
            .and(path("/search"))
            .and(header("content-type", "application/json"))
            .and(body_partial_json(serde_json::json!({
                "api_key": "tvly-test",
                "query": "rust async",
                "search_depth": "basic",
                "max_results": 5,
                "include_answer": true,
                "include_raw_content": false
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&server)
            .await;

        let a = fast_adapter(&server.uri());
        let resp = a.search(SReq::new("rust async")).await.unwrap();
        assert_eq!(resp.query, "rust async");
        assert_eq!(resp.results.len(), 2);
        assert_eq!(resp.results[0].title, "Rust Async Book");
        assert_eq!(resp.results[0].score, 0.92);
        assert_eq!(
            resp.answer.as_deref(),
            Some("Rust has async/await since 1.39.")
        );
        assert_eq!(resp.credits_used, 1);
    }

    #[tokio::test]
    async fn search_with_content_sets_include_raw_content_true() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/search"))
            .and(body_partial_json(serde_json::json!({
                "include_raw_content": true
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "query": "q",
                "results": [
                    { "title": "T", "url": "https://u", "content": "body", "score": 0.5 }
                ]
            })))
            .mount(&server)
            .await;

        let a = fast_adapter(&server.uri());
        let mut req = SReq::new("q");
        req.include_content = true;
        let resp = a.search(req).await.unwrap();
        assert_eq!(resp.results[0].content.as_deref(), Some("body"));
    }

    #[tokio::test]
    async fn search_advanced_depth_charges_two_credits() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/search"))
            .and(body_partial_json(
                serde_json::json!({ "search_depth": "advanced" }),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "query": "q",
                "results": []
            })))
            .mount(&server)
            .await;

        let a = fast_adapter(&server.uri());
        let mut req = SReq::new("q");
        req.search_depth = SearchDepth::Advanced;
        let resp = a.search(req).await.unwrap();
        assert_eq!(resp.credits_used, 2);
    }

    #[tokio::test]
    async fn search_forwards_domain_filters_and_time_range() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/search"))
            .and(body_partial_json(serde_json::json!({
                "include_domains": ["docs.aws.amazon.com"],
                "exclude_domains": ["reddit.com"],
                "time_range": "week"
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "query": "q",
                "results": []
            })))
            .mount(&server)
            .await;

        let a = fast_adapter(&server.uri());
        let mut req = SReq::new("q");
        req.include_domains = vec!["docs.aws.amazon.com".into()];
        req.exclude_domains = vec!["reddit.com".into()];
        req.time_range = Some(TimeRange::Week);
        a.search(req).await.unwrap();
    }

    #[tokio::test]
    async fn search_429_maps_to_rate_limit() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/search"))
            .respond_with(ResponseTemplate::new(429).set_body_string("slow down"))
            .mount(&server)
            .await;

        let a = fast_adapter(&server.uri());
        let err = a.search(SReq::new("q")).await.unwrap_err();
        assert!(matches!(err, WebDomainError::RateLimit { .. }));
    }

    #[tokio::test]
    async fn search_401_maps_to_adapter_init() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/search"))
            .respond_with(ResponseTemplate::new(401).set_body_string("bad key"))
            .mount(&server)
            .await;

        let a = fast_adapter(&server.uri());
        let err = a.search(SReq::new("q")).await.unwrap_err();
        assert!(matches!(err, WebDomainError::AdapterInit(_)));
    }

    #[tokio::test]
    async fn search_403_block_page_reaches_the_caller_as_a_readable_upstream_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/search"))
            .respond_with(ResponseTemplate::new(403).set_body_string(NGINX_BLOCK_PAGE))
            .mount(&server)
            .await;

        let a = fast_adapter(&server.uri());
        let err = a.search(SReq::new("q")).await.unwrap_err();
        assert!(err.is_llm_recoverable());
        let (status, text) = upstream_text(err);
        assert_eq!(status, 403);
        assert!(text.contains("not a problem with the API key"), "{text}");
        assert!(!text.contains("nginx"), "{text}");
    }

    #[tokio::test]
    async fn search_503_maps_to_upstream() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/search"))
            .respond_with(ResponseTemplate::new(503).set_body_string("down"))
            .mount(&server)
            .await;

        let a = fast_adapter(&server.uri());
        let err = a.search(SReq::new("q")).await.unwrap_err();
        assert!(matches!(err, WebDomainError::Upstream { status: 503, .. }));
    }

    use crate::web::domain::search_port::FetchRequest as FReq;

    #[tokio::test]
    async fn fetch_happy_path_markdown() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/extract"))
            .and(body_partial_json(serde_json::json!({
                "api_key": "tvly-test",
                "urls": ["https://example.com"],
                "extract_depth": "basic",
                "format": "markdown"
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "results": [
                    {
                        "url": "https://example.com",
                        "raw_content": "# Hello\n\nbody text."
                    }
                ],
                "failed_results": []
            })))
            .mount(&server)
            .await;

        let a = fast_adapter(&server.uri());
        let resp = a
            .fetch(FReq {
                url: "https://example.com".into(),
                format: ExtractFormat::Markdown,
            })
            .await
            .unwrap();
        assert_eq!(resp.url, "https://example.com");
        assert!(resp.content.contains("# Hello"));
        assert_eq!(resp.content_length as usize, resp.content.len());
        assert_eq!(resp.credits_used, 1);
    }

    #[tokio::test]
    async fn fetch_reports_failed_results_as_upstream() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/extract"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "results": [],
                "failed_results": [
                    { "url": "https://bad.example", "error": "connection refused" }
                ]
            })))
            .mount(&server)
            .await;

        let a = fast_adapter(&server.uri());
        let err = a
            .fetch(FReq {
                url: "https://bad.example".into(),
                format: ExtractFormat::Text,
            })
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            WebDomainError::Upstream { .. } | WebDomainError::NavigationFailed(_)
        ));
    }

    #[tokio::test]
    async fn fetch_403_block_page_reaches_the_caller_as_a_readable_upstream_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/extract"))
            .respond_with(ResponseTemplate::new(403).set_body_string(NGINX_BLOCK_PAGE))
            .mount(&server)
            .await;

        let a = fast_adapter(&server.uri());
        let err = a
            .fetch(FReq {
                url: "https://example.com".into(),
                format: ExtractFormat::Text,
            })
            .await
            .unwrap_err();
        let (status, text) = upstream_text(err);
        assert_eq!(status, 403);
        assert!(text.contains("block page"), "{text}");
    }

    #[tokio::test]
    async fn fetch_429_maps_to_rate_limit() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/extract"))
            .respond_with(ResponseTemplate::new(429))
            .mount(&server)
            .await;

        let a = fast_adapter(&server.uri());
        let err = a
            .fetch(FReq {
                url: "https://example.com".into(),
                format: ExtractFormat::Markdown,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, WebDomainError::RateLimit { .. }));
    }
}
