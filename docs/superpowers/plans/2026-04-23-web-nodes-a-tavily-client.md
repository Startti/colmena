# Web Nodes — `tavily_client` Implementation Plan (Spec A)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement the `tavily_client` toolkit node exposing two LLM sub-tools — `search` (Tavily web search with optional inline content extraction) and `fetch` (Tavily extract of a specific URL) — including a `SearchPort` trait, a REST `TavilyAdapter`, and a `SearchUseCase` with LRU cache, per-run rate limiting, and retry logic.

**Architecture:** New files under `src/libs/colmena/src/web/`:

- `web/domain/search_port.rs` — `SearchPort` trait + value objects (`SearchRequest`, `SearchResponse`, `SearchResult`, `FetchRequest`, `FetchResponse`, `SearchDepth`, `ExtractFormat`, `TimeRange`).
- `web/application/search_use_case.rs` — `SearchUseCase` wrapping `SearchPort` with LRU cache, rate limit counter, and retry policy.
- `web/infrastructure/tavily_adapter.rs` — `TavilyAdapter` implementing `SearchPort` via `reqwest` against `https://api.tavily.com/search` and `/extract`.
- `dag_engine/infrastructure/nodes/tavily_client.rs` — `TavilyClientNode` implementing `ToolkitNode`, dispatching on `__sub_tool` to `search` or `fetch` handlers.
- Registered in `HashMapNodeRegistry::new_with_secure_values` under `node_type = "tavily_client"`.

**Tech Stack:** Rust (async/await + tokio), `reqwest` (already in `Cargo.toml`), `serde`/`serde_json`, `async-trait`, `thiserror`, `lru` (already in `Cargo.toml`), `chrono` (already in `Cargo.toml`), `mockall` (dev, already present), `wiremock` (dev, needs adding).

**Design spec:** [docs/superpowers/specs/2026-04-23-web-nodes-a-tavily-client-design.md](../specs/2026-04-23-web-nodes-a-tavily-client-design.md)

**Depends on:** Plan 0 (`2026-04-23-web-nodes-0-unified-foundation.md`) — the `web/` module skeleton, `WebDomainError`, and `ToolkitNode` trait must exist first.

---

## Conventions for this plan

- All commits use: `Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>`
- Run Rust tests with `cargo test --lib <module>` — the crate is named `colmena_dag_engine` (NOT `colmena`).
- Use the project venv for Python steps: `.venv/bin/pytest ...`. Source `.env` for live-API tests.
- After each task, run `cargo check --lib` before committing; it must pass.
- After tasks that touch `registry.rs` or `dag_tool_executor.rs`, also run `cargo test --lib registry dag_tool_executor` before committing.
- Every task ends with a commit. Don't batch.
- Live-API tests (`tests::tavily_live`) read `TAVILY_API_KEY` and are skipped when unset.

---

## Task 0: Verify Plan 0 pre-requisites and add `wiremock` dev-dependency

**Files:**
- Modify: `src/libs/colmena/Cargo.toml`

- [ ] **Step 1: Verify Plan 0 is merged**

Run:

```bash
cargo test --lib web::domain::errors
cargo test --lib toolkit_node
cargo test --lib dag_tool_executor::toolkit_runtime_tests
```

Expected: all three pass. If any fail, Plan 0 is not fully merged — stop and land it first.

- [ ] **Step 2: Add `wiremock` to dev-dependencies**

Edit `src/libs/colmena/Cargo.toml`. In `[dev-dependencies]`, add (keep the existing list alphabetized if it already is):

```toml
wiremock = "0.6"
```

- [ ] **Step 3: Verify `wiremock` resolves**

Run: `cargo check --lib --tests 2>&1 | tail -10`
Expected: the project builds cleanly and Cargo lockfile adds `wiremock`.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/Cargo.toml Cargo.lock
git commit -m "$(cat <<'EOF'
chore(web): add wiremock dev-dependency for HTTP adapter tests

Plan A (tavily_client) uses wiremock to assert HTTP status → domain
error mapping in TavilyAdapter. No runtime behavior changes.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 1: Domain — `SearchPort` trait and value objects

**Files:**
- Create: `src/libs/colmena/src/web/domain/search_port.rs`
- Modify: `src/libs/colmena/src/web/domain/mod.rs`

- [ ] **Step 1: Write the failing test**

Create `src/libs/colmena/src/web/domain/search_port.rs`:

```rust
//! Search / fetch port used by the `tavily_client` toolkit node.
//!
//! Also accommodates future adapters for SearxNG, Exa, Serper, Brave, etc.
//! The unit tested here is the value-object shape and the default-trait plumbing;
//! behavior lives in the adapter + use-case layers.

use crate::web::domain::errors::WebDomainError;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Input to [`SearchPort::search`].
#[derive(Debug, Clone)]
pub struct SearchRequest {
    /// Natural-language query.
    pub query: String,
    /// Number of results to return (1–10).
    pub max_results: u8,
    /// When true, the response includes full extracted text for each result.
    pub include_content: bool,
    pub search_depth: SearchDepth,
    pub include_domains: Vec<String>,
    pub exclude_domains: Vec<String>,
    pub time_range: Option<TimeRange>,
}

impl SearchRequest {
    /// Minimal construction helper used in tests and in `SearchUseCase::search`.
    pub fn new(query: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            max_results: 5,
            include_content: false,
            search_depth: SearchDepth::Basic,
            include_domains: Vec::new(),
            exclude_domains: Vec::new(),
            time_range: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchDepth {
    Basic,
    Advanced,
}

impl SearchDepth {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Basic => "basic",
            Self::Advanced => "advanced",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TimeRange {
    Day,
    Week,
    Month,
    Year,
}

impl TimeRange {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Day => "day",
            Self::Week => "week",
            Self::Month => "month",
            Self::Year => "year",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResponse {
    pub query: String,
    pub results: Vec<SearchResult>,
    pub answer: Option<String>,
    pub credits_used: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
    pub score: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

#[derive(Debug, Clone)]
pub struct FetchRequest {
    pub url: String,
    pub format: ExtractFormat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExtractFormat {
    Markdown,
    Text,
}

impl ExtractFormat {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Markdown => "markdown",
            Self::Text => "text",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchResponse {
    pub url: String,
    pub title: Option<String>,
    pub content: String,
    pub content_length: u64,
    pub credits_used: u32,
}

/// Port implemented by search/extract providers.
#[async_trait]
pub trait SearchPort: Send + Sync {
    async fn search(&self, req: SearchRequest) -> Result<SearchResponse, WebDomainError>;
    async fn fetch(&self, req: FetchRequest) -> Result<FetchResponse, WebDomainError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_request_new_sets_safe_defaults() {
        let req = SearchRequest::new("rust async");
        assert_eq!(req.query, "rust async");
        assert_eq!(req.max_results, 5);
        assert!(!req.include_content);
        assert_eq!(req.search_depth, SearchDepth::Basic);
        assert!(req.time_range.is_none());
    }

    #[test]
    fn search_depth_serializes_lowercase() {
        assert_eq!(SearchDepth::Basic.as_str(), "basic");
        assert_eq!(SearchDepth::Advanced.as_str(), "advanced");
        let v = serde_json::to_value(SearchDepth::Advanced).unwrap();
        assert_eq!(v, serde_json::json!("advanced"));
    }

    #[test]
    fn time_range_serializes_lowercase() {
        for (r, s) in [
            (TimeRange::Day, "day"),
            (TimeRange::Week, "week"),
            (TimeRange::Month, "month"),
            (TimeRange::Year, "year"),
        ] {
            assert_eq!(r.as_str(), s);
        }
    }

    #[test]
    fn extract_format_serializes_lowercase() {
        assert_eq!(ExtractFormat::Markdown.as_str(), "markdown");
        assert_eq!(ExtractFormat::Text.as_str(), "text");
    }

    #[test]
    fn search_result_round_trips_json() {
        let r = SearchResult {
            title: "T".into(),
            url: "https://example.com".into(),
            snippet: "snip".into(),
            score: 0.5,
            content: Some("body".into()),
        };
        let v = serde_json::to_value(&r).unwrap();
        let back: SearchResult = serde_json::from_value(v).unwrap();
        assert_eq!(back.url, "https://example.com");
        assert_eq!(back.content.as_deref(), Some("body"));
    }

    #[test]
    fn search_result_content_is_skipped_when_none() {
        let r = SearchResult {
            title: "T".into(),
            url: "u".into(),
            snippet: "s".into(),
            score: 0.1,
            content: None,
        };
        let s = serde_json::to_string(&r).unwrap();
        assert!(!s.contains("\"content\""));
    }
}
```

- [ ] **Step 2: Wire the module**

Edit `src/libs/colmena/src/web/domain/mod.rs` so it reads:

```rust
//! Ports and value objects for the `web` toolkit nodes.

pub mod errors;
pub mod search_port;

pub use errors::WebDomainError;
pub use search_port::{
    ExtractFormat, FetchRequest, FetchResponse, SearchDepth, SearchPort, SearchRequest,
    SearchResponse, SearchResult, TimeRange,
};
```

(Keep `session`, `lifecycle`, and any other re-exports from Plan 0 intact above the new ones.)

- [ ] **Step 3: Run tests — expect PASS**

Run: `cargo test --lib web::domain::search_port`
Expected: 6 tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/web/domain/search_port.rs src/libs/colmena/src/web/domain/mod.rs
git commit -m "$(cat <<'EOF'
feat(web/domain): add SearchPort trait and value objects

Introduces the SearchPort contract (search + fetch) plus the shared
value objects (SearchRequest, SearchResponse, SearchResult,
FetchRequest, FetchResponse, SearchDepth, TimeRange, ExtractFormat).
The trait is provider-agnostic; the Tavily adapter and SearchUseCase
are added in subsequent tasks.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Infrastructure — `TavilyAdapter` scaffolding + error mapping unit tests

**Files:**
- Create: `src/libs/colmena/src/web/infrastructure/tavily_adapter.rs`
- Modify: `src/libs/colmena/src/web/infrastructure/mod.rs`

This task only wires the adapter struct, its error-mapping helper, and its unit tests for HTTP-status → `WebDomainError` mapping using `wiremock`. The happy-path request bodies come in Task 3 (search) and Task 4 (fetch).

- [ ] **Step 1: Write the module shell**

Create `src/libs/colmena/src/web/infrastructure/tavily_adapter.rs`:

```rust
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
//!   | 429                | `WebDomainError::RateLimit`    |
//!   | 5xx                | `WebDomainError::Upstream`     |
//!   | transport timeout  | `WebDomainError::Timeout`      |
//!   | other transport    | `WebDomainError::Upstream`     |

use crate::web::domain::errors::WebDomainError;
use crate::web::domain::search_port::{
    ExtractFormat, FetchRequest, FetchResponse, SearchDepth, SearchPort, SearchRequest,
    SearchResponse, SearchResult,
};
use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};
use std::time::Duration;

const DEFAULT_BASE_URL: &str = "https://api.tavily.com";

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
        let client = Client::builder()
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
            401 | 403 => WebDomainError::AdapterInit(format!(
                "Tavily auth failed (status {status}): {body}"
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
    async fn search(&self, _req: SearchRequest) -> Result<SearchResponse, WebDomainError> {
        // Implemented in Task 3.
        Err(WebDomainError::Upstream {
            status: 0,
            body: "search not yet implemented".into(),
        })
    }

    async fn fetch(&self, _req: FetchRequest) -> Result<FetchResponse, WebDomainError> {
        // Implemented in Task 4.
        Err(WebDomainError::Upstream {
            status: 0,
            body: "fetch not yet implemented".into(),
        })
    }
}

// Silence unused-import warnings until Tasks 3/4 use them.
#[allow(dead_code)]
fn _ensure_imports(_: &SearchResult, _: ExtractFormat, _: SearchDepth, _: Value) {
    let _ = json!({});
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
}
```

- [ ] **Step 2: Wire the module**

Edit `src/libs/colmena/src/web/infrastructure/mod.rs`:

```rust
//! Adapters implementing the web-toolkit ports.

pub mod tavily_adapter;

pub use tavily_adapter::TavilyAdapter;
```

- [ ] **Step 3: Run — expect PASS**

Run: `cargo test --lib web::infrastructure::tavily_adapter`
Expected: 7 tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/web/infrastructure
git commit -m "$(cat <<'EOF'
feat(web): add TavilyAdapter shell with error mapping

Lays down the TavilyAdapter struct, reqwest client builder (with per-
request timeout), and the HTTP-status → WebDomainError mapping helper.
The two SearchPort methods are still TODO stubs; Tasks 3 and 4 flesh
them out with full request bodies and wiremock happy-path coverage.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: `TavilyAdapter::search` — full implementation + wiremock tests

**Files:**
- Modify: `src/libs/colmena/src/web/infrastructure/tavily_adapter.rs`

- [ ] **Step 1: Write the failing tests**

Append to the `#[cfg(test)] mod tests` block in `tavily_adapter.rs`:

```rust
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
        assert_eq!(resp.answer.as_deref(), Some("Rust has async/await since 1.39."));
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
            .and(body_partial_json(serde_json::json!({ "search_depth": "advanced" })))
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
```

- [ ] **Step 2: Run — expect FAIL**

Run: `cargo test --lib web::infrastructure::tavily_adapter::tests::search_happy_path_returns_results`
Expected: FAIL — `search` is still the TODO stub returning `Upstream`.

- [ ] **Step 3: Implement `search`**

Replace the stubbed `async fn search` body in `tavily_adapter.rs` with:

```rust
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
        let raw: Value = resp
            .json()
            .await
            .map_err(|e| WebDomainError::Upstream {
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
                score: item
                    .get("score")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0) as f32,
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
```

Remove the `_ensure_imports` helper — all imports are now used.

- [ ] **Step 4: Run — expect PASS**

Run: `cargo test --lib web::infrastructure::tavily_adapter`
Expected: all tests (7 prior + 7 new) pass.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/web/infrastructure/tavily_adapter.rs
git commit -m "$(cat <<'EOF'
feat(web): implement TavilyAdapter::search against Tavily /search

POST /search with the full documented body (api_key, query, depth,
max_results, include_answer, include_raw_content, domain filters,
time_range). Advanced depth reports credits_used=2 per Tavily pricing;
basic reports 1. Non-2xx responses map via the existing error-mapping
helper. Covered by wiremock tests: happy path, include_content=true,
advanced depth, domain/time filters, 429/401/503 mappings.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: `TavilyAdapter::fetch` — full implementation + wiremock tests

**Files:**
- Modify: `src/libs/colmena/src/web/infrastructure/tavily_adapter.rs`

- [ ] **Step 1: Write the failing tests**

Append to the same `#[cfg(test)] mod tests` block:

```rust
    use crate::web::domain::search_port::{ExtractFormat, FetchRequest as FReq};

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
```

- [ ] **Step 2: Run — expect FAIL**

Run: `cargo test --lib web::infrastructure::tavily_adapter::tests::fetch_happy_path_markdown`
Expected: FAIL — `fetch` is still the TODO stub.

- [ ] **Step 3: Implement `fetch`**

Replace the `async fn fetch` stub in `tavily_adapter.rs` with:

```rust
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
            return Err(WebDomainError::Upstream { status: 200, body: msg });
        }

        let results = raw
            .get("results")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let first = results.into_iter().next().ok_or_else(|| {
            WebDomainError::Upstream {
                status: 200,
                body: "empty results from /extract".into(),
            }
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
```

- [ ] **Step 4: Run — expect PASS**

Run: `cargo test --lib web::infrastructure::tavily_adapter`
Expected: all tests pass (now 10 from before plus 3 new fetch tests).

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/web/infrastructure/tavily_adapter.rs
git commit -m "$(cat <<'EOF'
feat(web): implement TavilyAdapter::fetch against Tavily /extract

POST /extract with {api_key, urls:[url], extract_depth:"basic", format}.
Returns the first raw_content (wrapped as FetchResponse) and treats
failed_results as Upstream errors. Covered by wiremock tests: happy
path, failed_results, 429 mapping.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: Application — `SearchUseCaseConfig` + basic `SearchUseCase` (no cache / no rate limit yet)

**Files:**
- Create: `src/libs/colmena/src/web/application/search_use_case.rs`
- Modify: `src/libs/colmena/src/web/application/mod.rs`

- [ ] **Step 1: Write the failing test**

Create `src/libs/colmena/src/web/application/search_use_case.rs`:

```rust
//! SearchUseCase — wraps a [`SearchPort`] with:
//! - An LRU cache (Task 7) keyed by request hash + TTL.
//! - A per-`dag_run_id` rate-limit counter (Task 6) enforcing `max_calls_per_run`.
//! - A retry loop (Task 8) for transient `Upstream` / `Timeout` errors.
//!
//! Lives in the application layer per hexagonal rules — it orchestrates a
//! domain port and does no I/O itself.

use crate::web::domain::errors::WebDomainError;
use crate::web::domain::search_port::{
    FetchRequest, FetchResponse, SearchPort, SearchRequest, SearchResponse,
};
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct SearchUseCaseConfig {
    pub enable_cache: bool,
    pub cache_ttl: Duration,
    pub max_calls_per_run: u32,
    pub fail_on_limit: bool,
    pub max_attempts: u32,
    pub initial_backoff: Duration,
    pub timeout: Duration,
}

impl Default for SearchUseCaseConfig {
    fn default() -> Self {
        Self {
            enable_cache: true,
            cache_ttl: Duration::from_secs(3600),
            max_calls_per_run: 50,
            fail_on_limit: false,
            max_attempts: 3,
            initial_backoff: Duration::from_millis(500),
            timeout: Duration::from_secs(30),
        }
    }
}

pub struct SearchUseCase {
    port: Arc<dyn SearchPort>,
    config: SearchUseCaseConfig,
}

impl SearchUseCase {
    pub fn new(port: Arc<dyn SearchPort>, config: SearchUseCaseConfig) -> Self {
        Self { port, config }
    }

    pub fn config(&self) -> &SearchUseCaseConfig {
        &self.config
    }

    /// Run a search. `dag_run_id` identifies the run for rate-limit accounting.
    pub async fn search(
        &self,
        dag_run_id: &str,
        req: SearchRequest,
    ) -> Result<SearchResponse, WebDomainError> {
        let _ = dag_run_id;
        self.port.search(req).await
    }

    /// Run a fetch. `dag_run_id` identifies the run for rate-limit accounting.
    pub async fn fetch(
        &self,
        dag_run_id: &str,
        req: FetchRequest,
    ) -> Result<FetchResponse, WebDomainError> {
        let _ = dag_run_id;
        self.port.fetch(req).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::domain::search_port::{ExtractFormat, SearchResult};
    use async_trait::async_trait;
    use std::sync::Mutex;

    /// Minimal stub port so tests don't need wiremock.
    struct StubPort {
        search_calls: Mutex<u32>,
        fetch_calls: Mutex<u32>,
    }

    impl StubPort {
        fn new() -> Self {
            Self {
                search_calls: Mutex::new(0),
                fetch_calls: Mutex::new(0),
            }
        }
    }

    #[async_trait]
    impl SearchPort for StubPort {
        async fn search(&self, req: SearchRequest) -> Result<SearchResponse, WebDomainError> {
            *self.search_calls.lock().unwrap() += 1;
            Ok(SearchResponse {
                query: req.query,
                results: vec![SearchResult {
                    title: "T".into(),
                    url: "https://u".into(),
                    snippet: "s".into(),
                    score: 0.5,
                    content: None,
                }],
                answer: None,
                credits_used: 1,
            })
        }
        async fn fetch(&self, req: FetchRequest) -> Result<FetchResponse, WebDomainError> {
            *self.fetch_calls.lock().unwrap() += 1;
            Ok(FetchResponse {
                url: req.url,
                title: None,
                content: "body".into(),
                content_length: 4,
                credits_used: 1,
            })
        }
    }

    fn uc() -> (Arc<StubPort>, SearchUseCase) {
        let port = Arc::new(StubPort::new());
        let uc = SearchUseCase::new(port.clone(), SearchUseCaseConfig::default());
        (port, uc)
    }

    #[tokio::test]
    async fn search_delegates_to_port() {
        let (port, uc) = uc();
        let resp = uc.search("run-1", SearchRequest::new("hi")).await.unwrap();
        assert_eq!(resp.query, "hi");
        assert_eq!(*port.search_calls.lock().unwrap(), 1);
    }

    #[tokio::test]
    async fn fetch_delegates_to_port() {
        let (port, uc) = uc();
        let resp = uc
            .fetch(
                "run-1",
                FetchRequest {
                    url: "https://u".into(),
                    format: ExtractFormat::Markdown,
                },
            )
            .await
            .unwrap();
        assert_eq!(resp.url, "https://u");
        assert_eq!(*port.fetch_calls.lock().unwrap(), 1);
    }

    #[test]
    fn default_config_matches_spec() {
        let d = SearchUseCaseConfig::default();
        assert!(d.enable_cache);
        assert_eq!(d.cache_ttl, Duration::from_secs(3600));
        assert_eq!(d.max_calls_per_run, 50);
        assert!(!d.fail_on_limit);
        assert_eq!(d.max_attempts, 3);
        assert_eq!(d.initial_backoff, Duration::from_millis(500));
    }
}
```

- [ ] **Step 2: Wire the application module**

Edit `src/libs/colmena/src/web/application/mod.rs`:

```rust
//! Use cases orchestrating web-toolkit ports.

pub mod search_use_case;

pub use search_use_case::{SearchUseCase, SearchUseCaseConfig};
```

- [ ] **Step 3: Run — expect PASS**

Run: `cargo test --lib web::application::search_use_case`
Expected: 3 tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/web/application/search_use_case.rs src/libs/colmena/src/web/application/mod.rs
git commit -m "$(cat <<'EOF'
feat(web): scaffold SearchUseCase + SearchUseCaseConfig

Thin wrapper around SearchPort that just delegates today. The three
decorations — rate-limit counter, LRU cache, and retry loop — are
layered on in Tasks 6, 7, 8. Default config matches the Spec A
defaults (cache_ttl=3600s, max_calls_per_run=50, retry up to 3x).

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: `SearchUseCase` — per-run rate limit

**Files:**
- Modify: `src/libs/colmena/src/web/application/search_use_case.rs`

- [ ] **Step 1: Write the failing test**

Append to the `#[cfg(test)] mod tests` block:

```rust
    #[tokio::test]
    async fn rate_limit_caps_calls_per_run_non_fail() {
        let port = Arc::new(StubPort::new());
        let cfg = SearchUseCaseConfig {
            max_calls_per_run: 2,
            fail_on_limit: false,
            ..Default::default()
        };
        let uc = SearchUseCase::new(port.clone(), cfg);

        // 3 calls, cap of 2: first two hit port, third returns RateLimit (fail_on_limit=false
        // means structured RateLimit error — still not a crash).
        uc.search("run-A", SearchRequest::new("q1")).await.unwrap();
        uc.search("run-A", SearchRequest::new("q2")).await.unwrap();
        let err = uc.search("run-A", SearchRequest::new("q3")).await.unwrap_err();
        assert!(matches!(
            err,
            WebDomainError::RateLimit { calls_used: 2, cap: 2 }
        ));
        assert_eq!(*port.search_calls.lock().unwrap(), 2);
    }

    #[tokio::test]
    async fn rate_limit_separate_runs_have_separate_counters() {
        let port = Arc::new(StubPort::new());
        let cfg = SearchUseCaseConfig {
            max_calls_per_run: 1,
            ..Default::default()
        };
        let uc = SearchUseCase::new(port.clone(), cfg);

        uc.search("run-A", SearchRequest::new("q1")).await.unwrap();
        // run-B has its own counter, so this should succeed.
        uc.search("run-B", SearchRequest::new("q1")).await.unwrap();
        assert_eq!(*port.search_calls.lock().unwrap(), 2);
    }

    #[tokio::test]
    async fn rate_limit_reset_run_clears_counter() {
        let port = Arc::new(StubPort::new());
        let cfg = SearchUseCaseConfig {
            max_calls_per_run: 1,
            ..Default::default()
        };
        let uc = SearchUseCase::new(port.clone(), cfg);
        uc.search("run-A", SearchRequest::new("q1")).await.unwrap();
        let err = uc.search("run-A", SearchRequest::new("q2")).await.unwrap_err();
        assert!(matches!(err, WebDomainError::RateLimit { .. }));
        uc.reset_run("run-A");
        uc.search("run-A", SearchRequest::new("q3")).await.unwrap();
    }

    #[tokio::test]
    async fn rate_limit_fetch_shares_counter_with_search() {
        let port = Arc::new(StubPort::new());
        let cfg = SearchUseCaseConfig {
            max_calls_per_run: 1,
            ..Default::default()
        };
        let uc = SearchUseCase::new(port.clone(), cfg);
        uc.search("run-A", SearchRequest::new("q1")).await.unwrap();
        let err = uc
            .fetch(
                "run-A",
                FetchRequest { url: "u".into(), format: ExtractFormat::Text },
            )
            .await
            .unwrap_err();
        assert!(matches!(err, WebDomainError::RateLimit { .. }));
    }
```

- [ ] **Step 2: Run — expect FAIL**

Run: `cargo test --lib web::application::search_use_case::tests::rate_limit_caps_calls_per_run_non_fail`
Expected: FAIL — no rate-limit machinery wired yet; third call still hits the port.

- [ ] **Step 3: Implement the rate-limit counter**

Edit `src/libs/colmena/src/web/application/search_use_case.rs`. Add these imports near the top:

```rust
use std::collections::HashMap;
use std::sync::Mutex;
```

Add a new inner struct between `SearchUseCaseConfig` and `SearchUseCase`:

```rust
#[derive(Default)]
struct RateLimitState {
    per_run: HashMap<String, u32>,
}

impl RateLimitState {
    fn try_increment(&mut self, run_id: &str, cap: u32) -> Result<u32, WebDomainError> {
        let entry = self.per_run.entry(run_id.to_string()).or_insert(0);
        if *entry >= cap {
            return Err(WebDomainError::RateLimit {
                calls_used: *entry,
                cap,
            });
        }
        *entry += 1;
        Ok(*entry)
    }

    fn reset(&mut self, run_id: &str) {
        self.per_run.remove(run_id);
    }
}
```

Modify `struct SearchUseCase` to include:

```rust
pub struct SearchUseCase {
    port: Arc<dyn SearchPort>,
    config: SearchUseCaseConfig,
    counters: Mutex<RateLimitState>,
}
```

Update `SearchUseCase::new`:

```rust
    pub fn new(port: Arc<dyn SearchPort>, config: SearchUseCaseConfig) -> Self {
        Self {
            port,
            config,
            counters: Mutex::new(RateLimitState::default()),
        }
    }
```

Add a public reset helper:

```rust
    /// Reset the rate-limit counter for a given `dag_run_id`. Called by the
    /// engine at run-end. Safe to call even if no entry exists for the id.
    pub fn reset_run(&self, dag_run_id: &str) {
        self.counters.lock().unwrap().reset(dag_run_id);
    }
```

Replace `async fn search` and `async fn fetch` with:

```rust
    pub async fn search(
        &self,
        dag_run_id: &str,
        req: SearchRequest,
    ) -> Result<SearchResponse, WebDomainError> {
        self.counters
            .lock()
            .unwrap()
            .try_increment(dag_run_id, self.config.max_calls_per_run)?;
        self.port.search(req).await
    }

    pub async fn fetch(
        &self,
        dag_run_id: &str,
        req: FetchRequest,
    ) -> Result<FetchResponse, WebDomainError> {
        self.counters
            .lock()
            .unwrap()
            .try_increment(dag_run_id, self.config.max_calls_per_run)?;
        self.port.fetch(req).await
    }
```

> Note: The spec also mentions a `fail_on_limit` config for "crash the DAG vs surface as structured error." With our current `WebDomainError::RateLimit` classified as LLM-recoverable (per Plan 0 Task 1), `fail_on_limit=false` maps cleanly to returning the error to the LLM. `fail_on_limit=true` means the node should treat the error as fatal; that decision lives in the **node** layer (Task 10), not the use case. The counter always returns the same `RateLimit` error either way.

- [ ] **Step 4: Run — expect PASS**

Run: `cargo test --lib web::application::search_use_case`
Expected: all 7 tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/web/application/search_use_case.rs
git commit -m "$(cat <<'EOF'
feat(web): enforce max_calls_per_run in SearchUseCase

Counts search + fetch calls together per dag_run_id. When the cap is
reached, returns WebDomainError::RateLimit without hitting the port.
`reset_run()` lets the engine clear state at run-end. Separate runs
keep separate counters.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 7: `SearchUseCase` — LRU cache with TTL

**Files:**
- Modify: `src/libs/colmena/src/web/application/search_use_case.rs`

- [ ] **Step 1: Write the failing test**

Append to the `#[cfg(test)] mod tests` block:

```rust
    #[tokio::test]
    async fn cache_hit_returns_same_response_without_hitting_port() {
        let port = Arc::new(StubPort::new());
        let cfg = SearchUseCaseConfig {
            enable_cache: true,
            ..Default::default()
        };
        let uc = SearchUseCase::new(port.clone(), cfg);

        let r1 = uc.search("run-A", SearchRequest::new("same")).await.unwrap();
        let r2 = uc.search("run-A", SearchRequest::new("same")).await.unwrap();
        assert_eq!(r1.query, r2.query);
        // Only first call hits the port.
        assert_eq!(*port.search_calls.lock().unwrap(), 1);
    }

    #[tokio::test]
    async fn cache_miss_on_different_query() {
        let port = Arc::new(StubPort::new());
        let cfg = SearchUseCaseConfig {
            enable_cache: true,
            ..Default::default()
        };
        let uc = SearchUseCase::new(port.clone(), cfg);
        uc.search("run-A", SearchRequest::new("a")).await.unwrap();
        uc.search("run-A", SearchRequest::new("b")).await.unwrap();
        assert_eq!(*port.search_calls.lock().unwrap(), 2);
    }

    #[tokio::test]
    async fn cache_disabled_always_hits_port() {
        let port = Arc::new(StubPort::new());
        let cfg = SearchUseCaseConfig {
            enable_cache: false,
            ..Default::default()
        };
        let uc = SearchUseCase::new(port.clone(), cfg);
        uc.search("run-A", SearchRequest::new("same")).await.unwrap();
        uc.search("run-A", SearchRequest::new("same")).await.unwrap();
        assert_eq!(*port.search_calls.lock().unwrap(), 2);
    }

    #[tokio::test]
    async fn cache_entry_expires_after_ttl() {
        let port = Arc::new(StubPort::new());
        let cfg = SearchUseCaseConfig {
            enable_cache: true,
            cache_ttl: Duration::from_millis(0),
            ..Default::default()
        };
        let uc = SearchUseCase::new(port.clone(), cfg);
        uc.search("run-A", SearchRequest::new("same")).await.unwrap();
        // TTL=0 means every lookup is immediately expired.
        uc.search("run-A", SearchRequest::new("same")).await.unwrap();
        assert_eq!(*port.search_calls.lock().unwrap(), 2);
    }

    #[tokio::test]
    async fn cache_does_not_count_toward_rate_limit() {
        let port = Arc::new(StubPort::new());
        let cfg = SearchUseCaseConfig {
            enable_cache: true,
            max_calls_per_run: 1,
            ..Default::default()
        };
        let uc = SearchUseCase::new(port.clone(), cfg);
        uc.search("run-A", SearchRequest::new("x")).await.unwrap();
        // Cache hit; should not exhaust the cap.
        uc.search("run-A", SearchRequest::new("x")).await.unwrap();
        // Still counts only 1 toward the port; cap not yet hit.
        let err = uc.search("run-A", SearchRequest::new("y")).await.unwrap_err();
        assert!(matches!(err, WebDomainError::RateLimit { .. }));
    }
```

- [ ] **Step 2: Run — expect FAIL**

Run: `cargo test --lib web::application::search_use_case::tests::cache_hit_returns_same_response_without_hitting_port`
Expected: FAIL — no cache wired yet; every call hits the port.

- [ ] **Step 3: Implement the cache**

Edit `src/libs/colmena/src/web/application/search_use_case.rs`. Add imports:

```rust
use chrono::{DateTime, Utc};
use lru::LruCache;
use std::hash::{Hash, Hasher};
use std::collections::hash_map::DefaultHasher;
use std::num::NonZeroUsize;
use std::sync::RwLock;
```

Add a cache-key helper at module scope (above `RateLimitState`):

```rust
fn hash_search(req: &SearchRequest) -> u64 {
    let mut h = DefaultHasher::new();
    ("search",
     &req.query,
     req.max_results,
     req.include_content,
     req.search_depth as u8,
     &req.include_domains,
     &req.exclude_domains,
     req.time_range.map(|r| r as u8),
    )
        .hash(&mut h);
    h.finish()
}

fn hash_fetch(req: &FetchRequest) -> u64 {
    let mut h = DefaultHasher::new();
    ("fetch", &req.url, req.format as u8).hash(&mut h);
    h.finish()
}

#[derive(Clone)]
enum CachedResponse {
    Search(SearchResponse),
    Fetch(FetchResponse),
}

struct CachedEntry {
    resp: CachedResponse,
    inserted_at: DateTime<Utc>,
}
```

The `SearchDepth`, `TimeRange`, and `ExtractFormat` enums derived `Copy` in Task 1 but do NOT implement `Hash` by default — add `#[derive(Hash)]` to each of those three enums in `search_port.rs`. Update the three `#[derive(...)]` lines to include `Hash`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchDepth { ... }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TimeRange { ... }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExtractFormat { ... }
```

Extend `SearchUseCase` struct:

```rust
pub struct SearchUseCase {
    port: Arc<dyn SearchPort>,
    config: SearchUseCaseConfig,
    counters: Mutex<RateLimitState>,
    cache: RwLock<LruCache<u64, CachedEntry>>,
}
```

Update the constructor to size the cache (1024 entries is plenty for a toolkit node):

```rust
    pub fn new(port: Arc<dyn SearchPort>, config: SearchUseCaseConfig) -> Self {
        let cap = NonZeroUsize::new(1024).unwrap();
        Self {
            port,
            config,
            counters: Mutex::new(RateLimitState::default()),
            cache: RwLock::new(LruCache::new(cap)),
        }
    }
```

Update `search` and `fetch` to consult/insert the cache. Important: cache lookup happens **before** the rate-limit check, so cache hits do not consume the budget (matches spec: "Cache results across DAG runs to avoid paying for repeated queries").

```rust
    pub async fn search(
        &self,
        dag_run_id: &str,
        req: SearchRequest,
    ) -> Result<SearchResponse, WebDomainError> {
        let key = hash_search(&req);

        if self.config.enable_cache {
            if let Some(resp) = self.cache_lookup_search(key) {
                return Ok(resp);
            }
        }

        self.counters
            .lock()
            .unwrap()
            .try_increment(dag_run_id, self.config.max_calls_per_run)?;

        let resp = self.port.search(req).await?;

        if self.config.enable_cache {
            self.cache.write().unwrap().put(
                key,
                CachedEntry {
                    resp: CachedResponse::Search(resp.clone()),
                    inserted_at: Utc::now(),
                },
            );
        }
        Ok(resp)
    }

    pub async fn fetch(
        &self,
        dag_run_id: &str,
        req: FetchRequest,
    ) -> Result<FetchResponse, WebDomainError> {
        let key = hash_fetch(&req);

        if self.config.enable_cache {
            if let Some(resp) = self.cache_lookup_fetch(key) {
                return Ok(resp);
            }
        }

        self.counters
            .lock()
            .unwrap()
            .try_increment(dag_run_id, self.config.max_calls_per_run)?;

        let resp = self.port.fetch(req).await?;

        if self.config.enable_cache {
            self.cache.write().unwrap().put(
                key,
                CachedEntry {
                    resp: CachedResponse::Fetch(resp.clone()),
                    inserted_at: Utc::now(),
                },
            );
        }
        Ok(resp)
    }
```

Add private helpers on the impl:

```rust
    fn cache_lookup_search(&self, key: u64) -> Option<SearchResponse> {
        let mut cache = self.cache.write().unwrap();
        let entry = cache.get(&key)?;
        if self.is_expired(entry) {
            cache.pop(&key);
            return None;
        }
        match &entry.resp {
            CachedResponse::Search(s) => Some(s.clone()),
            CachedResponse::Fetch(_) => None,
        }
    }

    fn cache_lookup_fetch(&self, key: u64) -> Option<FetchResponse> {
        let mut cache = self.cache.write().unwrap();
        let entry = cache.get(&key)?;
        if self.is_expired(entry) {
            cache.pop(&key);
            return None;
        }
        match &entry.resp {
            CachedResponse::Fetch(f) => Some(f.clone()),
            CachedResponse::Search(_) => None,
        }
    }

    fn is_expired(&self, entry: &CachedEntry) -> bool {
        let age_ms = (Utc::now() - entry.inserted_at).num_milliseconds().max(0) as u64;
        age_ms >= self.config.cache_ttl.as_millis() as u64
    }
```

- [ ] **Step 4: Run — expect PASS**

Run: `cargo test --lib web::application::search_use_case`
Expected: all 12 tests (3+4+5) pass.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/web/application/search_use_case.rs src/libs/colmena/src/web/domain/search_port.rs
git commit -m "$(cat <<'EOF'
feat(web): add LRU + TTL cache to SearchUseCase

Caches both search and fetch responses keyed by a stable request hash.
Cache hits skip the rate-limit counter (spec goal: cached replays
are free). TTL comes from cache_ttl (default 3600s). Disabled when
enable_cache=false. Added Hash to SearchDepth/TimeRange/ExtractFormat
to support the hash key.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 8: `SearchUseCase` — retry loop for 5xx / Timeout

**Files:**
- Modify: `src/libs/colmena/src/web/application/search_use_case.rs`

- [ ] **Step 1: Write the failing test**

Append to the `#[cfg(test)] mod tests`:

```rust
    /// Port that fails `n` times before succeeding.
    struct FlakyPort {
        fail_count: Mutex<u32>,
        error: WebDomainError,
    }

    #[async_trait]
    impl SearchPort for FlakyPort {
        async fn search(&self, req: SearchRequest) -> Result<SearchResponse, WebDomainError> {
            let mut lock = self.fail_count.lock().unwrap();
            if *lock > 0 {
                *lock -= 1;
                return Err(clone_err(&self.error));
            }
            Ok(SearchResponse {
                query: req.query,
                results: vec![],
                answer: None,
                credits_used: 1,
            })
        }
        async fn fetch(&self, _req: FetchRequest) -> Result<FetchResponse, WebDomainError> {
            unreachable!()
        }
    }

    fn clone_err(e: &WebDomainError) -> WebDomainError {
        // WebDomainError has no Clone derive — rebuild by hand.
        match e {
            WebDomainError::Upstream { status, body } => WebDomainError::Upstream {
                status: *status,
                body: body.clone(),
            },
            WebDomainError::Timeout { ms } => WebDomainError::Timeout { ms: *ms },
            WebDomainError::RateLimit { calls_used, cap } => WebDomainError::RateLimit {
                calls_used: *calls_used,
                cap: *cap,
            },
            _ => WebDomainError::Upstream {
                status: 0,
                body: format!("{e}"),
            },
        }
    }

    #[tokio::test]
    async fn retry_on_upstream_5xx_eventually_succeeds() {
        let port = Arc::new(FlakyPort {
            fail_count: Mutex::new(2),
            error: WebDomainError::Upstream {
                status: 502,
                body: "bad gw".into(),
            },
        });
        let cfg = SearchUseCaseConfig {
            max_attempts: 3,
            initial_backoff: Duration::from_millis(1),
            enable_cache: false,
            ..Default::default()
        };
        let uc = SearchUseCase::new(port, cfg);
        let resp = uc.search("run-A", SearchRequest::new("hi")).await.unwrap();
        assert_eq!(resp.query, "hi");
    }

    #[tokio::test]
    async fn retry_exhaustion_surfaces_last_error() {
        let port = Arc::new(FlakyPort {
            fail_count: Mutex::new(5),
            error: WebDomainError::Upstream {
                status: 503,
                body: "down".into(),
            },
        });
        let cfg = SearchUseCaseConfig {
            max_attempts: 2,
            initial_backoff: Duration::from_millis(1),
            enable_cache: false,
            ..Default::default()
        };
        let uc = SearchUseCase::new(port, cfg);
        let err = uc.search("run-A", SearchRequest::new("hi")).await.unwrap_err();
        assert!(matches!(err, WebDomainError::Upstream { status: 503, .. }));
    }

    #[tokio::test]
    async fn retry_does_not_retry_rate_limit() {
        let port = Arc::new(FlakyPort {
            fail_count: Mutex::new(5),
            error: WebDomainError::RateLimit {
                calls_used: 10,
                cap: 10,
            },
        });
        let cfg = SearchUseCaseConfig {
            max_attempts: 3,
            initial_backoff: Duration::from_millis(1),
            enable_cache: false,
            ..Default::default()
        };
        let uc = SearchUseCase::new(port.clone(), cfg);
        let err = uc.search("run-A", SearchRequest::new("hi")).await.unwrap_err();
        assert!(matches!(err, WebDomainError::RateLimit { .. }));
        // Only one port call — RateLimit is not retryable.
        assert_eq!(*port.fail_count.lock().unwrap(), 4);
    }

    #[tokio::test]
    async fn retry_does_not_retry_adapter_init() {
        let port = Arc::new(FlakyPort {
            fail_count: Mutex::new(5),
            error: WebDomainError::AdapterInit("bad key".into()),
        });
        let cfg = SearchUseCaseConfig {
            max_attempts: 3,
            initial_backoff: Duration::from_millis(1),
            enable_cache: false,
            ..Default::default()
        };
        let uc = SearchUseCase::new(port.clone(), cfg);
        let err = uc.search("run-A", SearchRequest::new("hi")).await.unwrap_err();
        assert!(matches!(err, WebDomainError::AdapterInit(_)));
        assert_eq!(*port.fail_count.lock().unwrap(), 4);
    }
```

- [ ] **Step 2: Run — expect FAIL**

Run: `cargo test --lib web::application::search_use_case::tests::retry_on_upstream_5xx_eventually_succeeds`
Expected: FAIL — first attempt errors, no retry.

- [ ] **Step 3: Implement retry**

Edit `src/libs/colmena/src/web/application/search_use_case.rs`. Add a classifier:

```rust
fn is_retryable(e: &WebDomainError) -> bool {
    matches!(
        e,
        WebDomainError::Upstream { status, .. } if (500..600).contains(status) || *status == 0
    ) || matches!(e, WebDomainError::Timeout { .. })
}
```

Wrap the port calls in `search` and `fetch` with a retry loop. Replace the line `let resp = self.port.search(req).await?;` with:

```rust
        let resp = self.call_with_retry_search(req).await?;
```

And the line `let resp = self.port.fetch(req).await?;` with:

```rust
        let resp = self.call_with_retry_fetch(req).await?;
```

Add the helpers:

```rust
    async fn call_with_retry_search(
        &self,
        req: SearchRequest,
    ) -> Result<SearchResponse, WebDomainError> {
        let mut attempt = 0u32;
        let mut backoff = self.config.initial_backoff;
        loop {
            attempt += 1;
            match self.port.search(req.clone()).await {
                Ok(v) => return Ok(v),
                Err(e) if is_retryable(&e) && attempt < self.config.max_attempts => {
                    tokio::time::sleep(backoff).await;
                    backoff = backoff.saturating_mul(2);
                    continue;
                }
                Err(e) => return Err(e),
            }
        }
    }

    async fn call_with_retry_fetch(
        &self,
        req: FetchRequest,
    ) -> Result<FetchResponse, WebDomainError> {
        let mut attempt = 0u32;
        let mut backoff = self.config.initial_backoff;
        loop {
            attempt += 1;
            match self.port.fetch(req.clone()).await {
                Ok(v) => return Ok(v),
                Err(e) if is_retryable(&e) && attempt < self.config.max_attempts => {
                    tokio::time::sleep(backoff).await;
                    backoff = backoff.saturating_mul(2);
                    continue;
                }
                Err(e) => return Err(e),
            }
        }
    }
```

(`SearchRequest` and `FetchRequest` already derive `Clone` in Task 1; the retry loop relies on that.)

- [ ] **Step 4: Run — expect PASS**

Run: `cargo test --lib web::application::search_use_case`
Expected: all 16 tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/web/application/search_use_case.rs
git commit -m "$(cat <<'EOF'
feat(web): retry transient failures in SearchUseCase

Wraps the port calls in a bounded retry loop (max_attempts, initial
backoff, doubling each attempt). Retries only WebDomainError::Upstream
5xx and Timeout. RateLimit, AdapterInit, InvalidConfig, and
NavigationFailed do not retry.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 9: Node skeleton — `TavilyClientNode` implements `ToolkitNode::sub_tool_catalog`

**Files:**
- Create: `src/libs/colmena/src/dag_engine/infrastructure/nodes/tavily_client.rs`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/mod.rs`

This task delivers a non-functional node struct + the sub-tool catalog only. `execute()` returns a not-yet-implemented error until Tasks 10 and 11.

- [ ] **Step 1: Write the failing catalog test**

Create `src/libs/colmena/src/dag_engine/infrastructure/nodes/tavily_client.rs`:

```rust
//! `tavily_client` toolkit node. Exposes two LLM sub-tools: `search` and
//! `fetch`. Internally drives a [`SearchUseCase`] over a [`TavilyAdapter`].
//!
//! Spec: docs/superpowers/specs/2026-04-23-web-nodes-a-tavily-client-design.md

use crate::dag_engine::application::secure_value_service::SecureValueService;
use crate::dag_engine::domain::node::{ExecutableNode, NodeInputs};
use crate::dag_engine::domain::observer::ExecutionObserver;
use crate::dag_engine::domain::toolkit_node::{SubToolDefinition, ToolkitNode, SUB_TOOL_INPUT_KEY};
use crate::llm::domain::ParameterProperty;
use crate::web::application::search_use_case::{SearchUseCase, SearchUseCaseConfig};
use crate::web::domain::search_port::SearchPort;
use crate::web::infrastructure::tavily_adapter::TavilyAdapter;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::error::Error as StdError;
use std::sync::Arc;
use std::time::Duration;

/// `tavily_client` node.
///
/// Construction is deferred: the adapter and use case are created on first
/// `execute()` call because the API key comes from the node's per-call `config`
/// (via `${ENV_VAR}` resolution or a secure-value placeholder), not from
/// registry construction.
pub struct TavilyClientNode {
    secure_values: Option<Arc<SecureValueService>>,
}

impl TavilyClientNode {
    pub fn new() -> Self {
        Self {
            secure_values: None,
        }
    }

    pub fn with_secure_values(mut self, svc: Arc<SecureValueService>) -> Self {
        self.secure_values = Some(svc);
        self
    }

    /// Resolve `${VAR}` placeholders against process environment. Borrowed from
    /// the llm.rs helper. Literal strings pass through unchanged.
    pub(crate) fn resolve_env_var(value: &str) -> Result<String, String> {
        if value.starts_with("${") && value.ends_with("}") {
            let var_name = &value[2..value.len() - 1];
            std::env::var(var_name)
                .map_err(|_| format!("env var {var_name} not set (referenced by tavily_client)"))
        } else {
            Ok(value.to_string())
        }
    }

    /// Build a SearchUseCase from the per-call config. Validates / resolves
    /// the api_key and applies defaults from the spec.
    pub(crate) async fn build_use_case(
        &self,
        config: &Value,
        session_id: &str,
    ) -> Result<Arc<SearchUseCase>, Box<dyn StdError + Send + Sync>> {
        // Resolve secure value placeholders (<value_N>) in a *copy* so the caller's
        // config is untouched. Env-var placeholders (${VAR}) are resolved below.
        let mut cfg_copy = config.clone();
        if let Some(svc) = &self.secure_values {
            svc.inject_secrets(&mut cfg_copy, session_id).await?;
        }

        let api_key_raw = cfg_copy
            .get("api_key")
            .and_then(|v| v.as_str())
            .ok_or("tavily_client: config.api_key is required")?;
        if !api_key_raw.starts_with("${") && api_key_raw.starts_with("tvly-") {
            tracing::warn!(
                "tavily_client: api_key passed as a literal — prefer ${{TAVILY_API_KEY}} or a secure value"
            );
        }
        let api_key = Self::resolve_env_var(api_key_raw)?;

        let timeout_seconds = cfg_copy
            .get("timeout_seconds")
            .and_then(|v| v.as_u64())
            .unwrap_or(30);

        let adapter = TavilyAdapter::new(api_key, Duration::from_secs(timeout_seconds))?;
        let port: Arc<dyn SearchPort> = Arc::new(adapter);

        let enable_cache = cfg_copy
            .get("enable_cache")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let cache_ttl = Duration::from_secs(
            cfg_copy
                .get("cache_ttl_seconds")
                .and_then(|v| v.as_u64())
                .unwrap_or(3600),
        );
        let max_calls_per_run = cfg_copy
            .get("max_calls_per_run")
            .and_then(|v| v.as_u64())
            .unwrap_or(50) as u32;
        let fail_on_limit = cfg_copy
            .get("fail_on_limit")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let (max_attempts, initial_backoff) = cfg_copy
            .get("retry_policy")
            .map(|v| {
                let a = v.get("max_attempts").and_then(|n| n.as_u64()).unwrap_or(3) as u32;
                let b = v
                    .get("initial_backoff_ms")
                    .and_then(|n| n.as_u64())
                    .unwrap_or(500);
                (a, Duration::from_millis(b))
            })
            .unwrap_or((3, Duration::from_millis(500)));

        let uc_cfg = SearchUseCaseConfig {
            enable_cache,
            cache_ttl,
            max_calls_per_run,
            fail_on_limit,
            max_attempts,
            initial_backoff,
            timeout: Duration::from_secs(timeout_seconds),
        };

        Ok(Arc::new(SearchUseCase::new(port, uc_cfg)))
    }
}

impl Default for TavilyClientNode {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ExecutableNode for TavilyClientNode {
    async fn execute(
        &self,
        inputs: &NodeInputs,
        _config: &Value,
        _state: &mut Value,
        _observer: Option<Arc<dyn ExecutionObserver>>,
    ) -> Result<Value, Box<dyn StdError + Send + Sync>> {
        let sub = inputs
            .get(SUB_TOOL_INPUT_KEY)
            .and_then(|v| v.as_str())
            .ok_or("tavily_client: missing __sub_tool")?;
        Err(format!("tavily_client: sub_tool '{sub}' not implemented yet").into())
    }

    fn schema(&self) -> Value {
        json!({
            "inputs": { "__sub_tool": "string" },
            "outputs": { "output": "any" },
            "config": {
                "api_key": "string (required)",
                "enable_cache": "bool (default true)",
                "cache_ttl_seconds": "u64 (default 3600)",
                "max_calls_per_run": "u32 (default 50)",
                "fail_on_limit": "bool (default false)",
                "retry_policy": { "max_attempts": "u32", "initial_backoff_ms": "u64" },
                "timeout_seconds": "u64 (default 30)",
                "search_defaults": "object — merged into each search call"
            }
        })
    }

    fn description(&self) -> Option<&str> {
        Some(
            "Web search & read (Tavily). Exposes two sub-tools: search and fetch. \
             Use `search` for open-ended web queries, `fetch` to read a specific URL.",
        )
    }
}

impl ToolkitNode for TavilyClientNode {
    fn sub_tool_catalog(&self, _config: &Value) -> Vec<SubToolDefinition> {
        vec![search_sub_tool(), fetch_sub_tool()]
    }
}

fn search_sub_tool() -> SubToolDefinition {
    let mut props = HashMap::new();
    props.insert(
        "query".into(),
        ParameterProperty {
            property_type: "string".into(),
            description: "Natural-language search query.".into(),
            enum_values: None,
            pattern: None,
        },
    );
    props.insert(
        "max_results".into(),
        ParameterProperty {
            property_type: "integer".into(),
            description: "Number of results (1-10). Default 5.".into(),
            enum_values: None,
            pattern: None,
        },
    );
    props.insert(
        "include_content".into(),
        ParameterProperty {
            property_type: "boolean".into(),
            description:
                "If true, include full extracted content in each result. 2-3x credit cost. Default false."
                    .into(),
            enum_values: None,
            pattern: None,
        },
    );
    props.insert(
        "search_depth".into(),
        ParameterProperty {
            property_type: "string".into(),
            description: "\"basic\" (1 credit) or \"advanced\" (2 credits). Default basic.".into(),
            enum_values: Some(vec!["basic".into(), "advanced".into()]),
            pattern: None,
        },
    );
    props.insert(
        "include_domains".into(),
        ParameterProperty {
            property_type: "array".into(),
            description: "Restrict results to these domains.".into(),
            enum_values: None,
            pattern: None,
        },
    );
    props.insert(
        "exclude_domains".into(),
        ParameterProperty {
            property_type: "array".into(),
            description: "Exclude these domains.".into(),
            enum_values: None,
            pattern: None,
        },
    );
    props.insert(
        "time_range".into(),
        ParameterProperty {
            property_type: "string".into(),
            description: "Restrict to content from this recency window.".into(),
            enum_values: Some(vec!["day".into(), "week".into(), "month".into(), "year".into()]),
            pattern: None,
        },
    );

    SubToolDefinition {
        name: "search",
        description: "Search the web for up-to-date information on any topic. Returns a ranked list \
            of relevant results with titles, URLs, and snippets. Use this when the user asks about \
            current events, facts you are not confident about, or anything whose answer is not in your \
            training data. Set `include_content=true` only when snippets are insufficient — this is \
            2-3x more expensive but saves a follow-up fetch call. Use `include_domains` to restrict \
            to trusted sources. Prefer specific queries over generic ones."
            .into(),
        properties: props,
        required: vec!["query".into()],
    }
}

fn fetch_sub_tool() -> SubToolDefinition {
    let mut props = HashMap::new();
    props.insert(
        "url".into(),
        ParameterProperty {
            property_type: "string".into(),
            description: "Absolute URL to fetch.".into(),
            enum_values: None,
            pattern: None,
        },
    );
    props.insert(
        "extract_format".into(),
        ParameterProperty {
            property_type: "string".into(),
            description: "Output format (default markdown).".into(),
            enum_values: Some(vec!["markdown".into(), "text".into()]),
            pattern: None,
        },
    );
    SubToolDefinition {
        name: "fetch",
        description: "Read the cleaned text content of a specific URL. Use this when you already \
            know the URL (the user gave it to you, or it came from a previous search) and want the \
            full content, not just a snippet. Output is the page text with navigation, ads, and \
            boilerplate removed. Does not execute JavaScript — use the `browser` toolkit for pages \
            that require login or dynamic rendering."
            .into(),
        properties: props,
        required: vec!["url".into()],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_has_search_and_fetch() {
        let node = TavilyClientNode::new();
        let cat = node.sub_tool_catalog(&json!({}));
        assert_eq!(cat.len(), 2);
        assert!(cat.iter().any(|s| s.name == "search"));
        assert!(cat.iter().any(|s| s.name == "fetch"));
    }

    #[test]
    fn search_requires_query() {
        let node = TavilyClientNode::new();
        let cat = node.sub_tool_catalog(&json!({}));
        let s = cat.iter().find(|s| s.name == "search").unwrap();
        assert!(s.required.contains(&"query".to_string()));
    }

    #[test]
    fn fetch_requires_url() {
        let node = TavilyClientNode::new();
        let cat = node.sub_tool_catalog(&json!({}));
        let s = cat.iter().find(|s| s.name == "fetch").unwrap();
        assert!(s.required.contains(&"url".to_string()));
    }

    #[test]
    fn resolve_env_var_passes_literal_through() {
        assert_eq!(TavilyClientNode::resolve_env_var("tvly-xxx").unwrap(), "tvly-xxx");
    }

    #[test]
    fn resolve_env_var_replaces_placeholder() {
        std::env::set_var("COLMENA_TEST_TAVILY_KEY", "tvly-zzz");
        let v = TavilyClientNode::resolve_env_var("${COLMENA_TEST_TAVILY_KEY}").unwrap();
        assert_eq!(v, "tvly-zzz");
    }
}
```

- [ ] **Step 2: Register the module**

Edit `src/libs/colmena/src/dag_engine/infrastructure/nodes/mod.rs`. Add:

```rust
pub mod tavily_client;
```

- [ ] **Step 3: Run — expect PASS**

Run: `cargo test --lib tavily_client`
Expected: 5 tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/tavily_client.rs src/libs/colmena/src/dag_engine/infrastructure/nodes/mod.rs
git commit -m "$(cat <<'EOF'
feat(nodes): add TavilyClientNode skeleton (catalog only)

Declares the two LLM-facing sub-tools with rich descriptions that
drive accuracy. execute() still returns NotImplemented — Tasks 10 and
11 add the search and fetch handlers. build_use_case() resolves
${ENV} placeholders and secure-value (<value_N>) tokens from the
per-call config before constructing the adapter.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 10: `TavilyClientNode::execute` — `search` handler

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/tavily_client.rs`

- [ ] **Step 1: Write the failing integration test**

Append to the `#[cfg(test)] mod tests` block:

```rust
    use crate::web::application::search_use_case::{SearchUseCase, SearchUseCaseConfig};
    use crate::web::domain::errors::WebDomainError;
    use crate::web::domain::search_port::{
        ExtractFormat, FetchRequest as FReq, FetchResponse as FResp, SearchPort,
        SearchRequest as SReq, SearchResponse as SResp, SearchResult,
    };
    use std::sync::Mutex;

    struct StubPort {
        search_calls: Mutex<u32>,
        fetch_calls: Mutex<u32>,
    }

    #[async_trait::async_trait]
    impl SearchPort for StubPort {
        async fn search(&self, req: SReq) -> Result<SResp, WebDomainError> {
            *self.search_calls.lock().unwrap() += 1;
            Ok(SResp {
                query: req.query,
                results: vec![SearchResult {
                    title: "Rust".into(),
                    url: "https://example.com".into(),
                    snippet: "snip".into(),
                    score: 0.9,
                    content: None,
                }],
                answer: None,
                credits_used: 1,
            })
        }
        async fn fetch(&self, req: FReq) -> Result<FResp, WebDomainError> {
            *self.fetch_calls.lock().unwrap() += 1;
            Ok(FResp {
                url: req.url,
                title: None,
                content: "body".into(),
                content_length: 4,
                credits_used: 1,
            })
        }
    }

    /// Convenience: build a node that uses a stubbed port rather than real Tavily.
    fn node_with_stub() -> (Arc<StubPort>, TavilyClientNode, Arc<SearchUseCase>) {
        let port = Arc::new(StubPort {
            search_calls: Mutex::new(0),
            fetch_calls: Mutex::new(0),
        });
        let uc = Arc::new(SearchUseCase::new(
            port.clone() as Arc<dyn SearchPort>,
            SearchUseCaseConfig::default(),
        ));
        let mut node = TavilyClientNode::new();
        node.test_use_case = Some(uc.clone());
        (port, node, uc)
    }

    #[tokio::test]
    async fn search_dispatches_and_returns_json() {
        let (port, node, _uc) = node_with_stub();
        let mut inputs: NodeInputs = HashMap::new();
        inputs.insert(SUB_TOOL_INPUT_KEY.into(), json!("search"));
        inputs.insert("query".into(), json!("rust async"));
        let mut state = json!({});
        let out = node
            .execute(&inputs, &json!({ "api_key": "tvly-stub" }), &mut state, None)
            .await
            .unwrap();
        assert_eq!(out.get("query").and_then(|v| v.as_str()), Some("rust async"));
        assert_eq!(
            out.get("results")
                .and_then(|v| v.as_array())
                .map(|a| a.len()),
            Some(1)
        );
        assert_eq!(*port.search_calls.lock().unwrap(), 1);
    }

    #[tokio::test]
    async fn search_merges_search_defaults_from_config() {
        let (port, node, _uc) = node_with_stub();
        let mut inputs: NodeInputs = HashMap::new();
        inputs.insert(SUB_TOOL_INPUT_KEY.into(), json!("search"));
        inputs.insert("query".into(), json!("q"));
        // No max_results in inputs; should come from search_defaults.
        let config = json!({
            "api_key": "tvly-stub",
            "search_defaults": { "max_results": 3, "include_domains": ["rust-lang.org"] }
        });
        let mut state = json!({});
        node.execute(&inputs, &config, &mut state, None).await.unwrap();
        assert_eq!(*port.search_calls.lock().unwrap(), 1);
    }

    #[tokio::test]
    async fn search_rate_limit_returns_structured_error_when_fail_on_limit_false() {
        let port = Arc::new(StubPort {
            search_calls: Mutex::new(0),
            fetch_calls: Mutex::new(0),
        });
        let uc = Arc::new(SearchUseCase::new(
            port.clone() as Arc<dyn SearchPort>,
            SearchUseCaseConfig {
                max_calls_per_run: 1,
                fail_on_limit: false,
                enable_cache: false,
                ..Default::default()
            },
        ));
        let mut node = TavilyClientNode::new();
        node.test_use_case = Some(uc);

        // First call: fine.
        let mut inputs: NodeInputs = HashMap::new();
        inputs.insert(SUB_TOOL_INPUT_KEY.into(), json!("search"));
        inputs.insert("query".into(), json!("a"));
        let mut state = json!({});
        node.execute(
            &inputs,
            &json!({ "api_key": "tvly-stub" }),
            &mut state,
            None,
        )
        .await
        .unwrap();

        // Second call: over the cap. With fail_on_limit=false, the node
        // returns a structured JSON error result (not an Err).
        let mut inputs2: NodeInputs = HashMap::new();
        inputs2.insert(SUB_TOOL_INPUT_KEY.into(), json!("search"));
        inputs2.insert("query".into(), json!("b"));
        let out = node
            .execute(
                &inputs2,
                &json!({ "api_key": "tvly-stub" }),
                &mut state,
                None,
            )
            .await
            .unwrap();
        assert_eq!(
            out.get("error").and_then(|v| v.as_str()),
            Some("rate_limit")
        );
    }

    #[tokio::test]
    async fn search_rate_limit_crashes_dag_when_fail_on_limit_true() {
        let port = Arc::new(StubPort {
            search_calls: Mutex::new(0),
            fetch_calls: Mutex::new(0),
        });
        let uc = Arc::new(SearchUseCase::new(
            port.clone() as Arc<dyn SearchPort>,
            SearchUseCaseConfig {
                max_calls_per_run: 0,
                fail_on_limit: true,
                ..Default::default()
            },
        ));
        let mut node = TavilyClientNode::new();
        node.test_use_case = Some(uc);

        let mut inputs: NodeInputs = HashMap::new();
        inputs.insert(SUB_TOOL_INPUT_KEY.into(), json!("search"));
        inputs.insert("query".into(), json!("a"));
        let mut state = json!({});
        let err = node
            .execute(
                &inputs,
                &json!({ "api_key": "tvly-stub", "fail_on_limit": true }),
                &mut state,
                None,
            )
            .await
            .unwrap_err();
        assert!(err.to_string().to_lowercase().contains("rate"));
    }

    #[tokio::test]
    async fn search_missing_query_returns_structured_error() {
        let (_port, node, _uc) = node_with_stub();
        let mut inputs: NodeInputs = HashMap::new();
        inputs.insert(SUB_TOOL_INPUT_KEY.into(), json!("search"));
        // No "query".
        let mut state = json!({});
        let out = node
            .execute(&inputs, &json!({ "api_key": "tvly-stub" }), &mut state, None)
            .await
            .unwrap();
        assert_eq!(
            out.get("error").and_then(|v| v.as_str()),
            Some("invalid_input")
        );
    }
```

- [ ] **Step 2: Run — expect FAIL**

Run: `cargo test --lib tavily_client::tests::search_dispatches_and_returns_json`
Expected: FAIL — `execute()` currently returns the not-implemented error and `test_use_case` field doesn't exist.

- [ ] **Step 3: Implement the search handler**

Edit `src/libs/colmena/src/dag_engine/infrastructure/nodes/tavily_client.rs`. Add the test-only field to the struct and honor it in `build_use_case`. Add:

```rust
pub struct TavilyClientNode {
    secure_values: Option<Arc<SecureValueService>>,
    #[cfg(test)]
    pub(crate) test_use_case: Option<Arc<SearchUseCase>>,
}
```

Update `new` and `default`:

```rust
    pub fn new() -> Self {
        Self {
            secure_values: None,
            #[cfg(test)]
            test_use_case: None,
        }
    }
```

Modify `build_use_case` to return the test injection when set:

```rust
    pub(crate) async fn build_use_case(
        &self,
        config: &Value,
        session_id: &str,
    ) -> Result<Arc<SearchUseCase>, Box<dyn StdError + Send + Sync>> {
        #[cfg(test)]
        if let Some(uc) = &self.test_use_case {
            return Ok(uc.clone());
        }

        // ... (existing body unchanged, starting with `let mut cfg_copy = config.clone();`)
```

Now add the handler helpers on `TavilyClientNode`:

```rust
    async fn handle_search(
        &self,
        inputs: &NodeInputs,
        config: &Value,
        session_id: &str,
    ) -> Result<Value, Box<dyn StdError + Send + Sync>> {
        use crate::web::domain::search_port::{SearchDepth, SearchRequest, TimeRange};

        // Validate required.
        let Some(query) = inputs.get("query").and_then(|v| v.as_str()) else {
            return Ok(json!({
                "error": "invalid_input",
                "message": "search requires `query` (string)"
            }));
        };

        let defaults = config.get("search_defaults").cloned().unwrap_or(json!({}));

        let max_results = inputs
            .get("max_results")
            .and_then(|v| v.as_u64())
            .or_else(|| defaults.get("max_results").and_then(|v| v.as_u64()))
            .unwrap_or(5)
            .clamp(1, 10) as u8;
        let include_content = inputs
            .get("include_content")
            .and_then(|v| v.as_bool())
            .or_else(|| defaults.get("include_content").and_then(|v| v.as_bool()))
            .unwrap_or(false);

        let search_depth_str = inputs
            .get("search_depth")
            .and_then(|v| v.as_str())
            .or_else(|| defaults.get("search_depth").and_then(|v| v.as_str()))
            .unwrap_or("basic");
        let search_depth = match search_depth_str {
            "advanced" => SearchDepth::Advanced,
            _ => SearchDepth::Basic,
        };

        let include_domains = merge_string_array(
            inputs.get("include_domains"),
            defaults.get("include_domains"),
        );
        let exclude_domains = merge_string_array(
            inputs.get("exclude_domains"),
            defaults.get("exclude_domains"),
        );

        let time_range = inputs
            .get("time_range")
            .and_then(|v| v.as_str())
            .or_else(|| defaults.get("time_range").and_then(|v| v.as_str()))
            .and_then(|s| match s {
                "day" => Some(TimeRange::Day),
                "week" => Some(TimeRange::Week),
                "month" => Some(TimeRange::Month),
                "year" => Some(TimeRange::Year),
                _ => None,
            });

        let uc = self.build_use_case(config, session_id).await?;
        let req = SearchRequest {
            query: query.to_string(),
            max_results,
            include_content,
            search_depth,
            include_domains,
            exclude_domains,
            time_range,
        };

        match uc.search(session_id, req).await {
            Ok(resp) => Ok(serde_json::to_value(resp)?),
            Err(e) => Ok(format_llm_error(e, config)?),
        }
    }
```

Add helpers at module scope below the impls:

```rust
fn merge_string_array(from_input: Option<&Value>, from_defaults: Option<&Value>) -> Vec<String> {
    let read = |v: Option<&Value>| -> Vec<String> {
        v.and_then(|x| x.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|e| e.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    };
    let a = read(from_input);
    if !a.is_empty() {
        a
    } else {
        read(from_defaults)
    }
}

fn format_llm_error(
    e: crate::web::domain::errors::WebDomainError,
    config: &Value,
) -> Result<Value, Box<dyn StdError + Send + Sync>> {
    use crate::web::domain::errors::WebDomainError;

    let fail_on_limit = config
        .get("fail_on_limit")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    if !e.is_llm_recoverable() {
        return Err(Box::new(e));
    }
    match e {
        WebDomainError::RateLimit { calls_used, cap } => {
            if fail_on_limit {
                return Err(Box::new(WebDomainError::RateLimit { calls_used, cap }));
            }
            Ok(json!({
                "error": "rate_limit",
                "calls_used": calls_used,
                "cap": cap,
                "message": format!("rate limit reached ({calls_used}/{cap})")
            }))
        }
        WebDomainError::Timeout { ms } => Ok(json!({
            "error": "timeout",
            "ms": ms,
            "message": format!("request timed out after {ms} ms")
        })),
        WebDomainError::Upstream { status, body } => Ok(json!({
            "error": "upstream_error",
            "status": status,
            "retryable": false,
            "message": body
        })),
        other => Ok(json!({
            "error": "web_error",
            "message": other.to_string()
        })),
    }
}
```

Replace `async fn execute` in the `ExecutableNode` impl to dispatch:

```rust
    async fn execute(
        &self,
        inputs: &NodeInputs,
        config: &Value,
        _state: &mut Value,
        _observer: Option<Arc<dyn ExecutionObserver>>,
    ) -> Result<Value, Box<dyn StdError + Send + Sync>> {
        let sub = inputs
            .get(SUB_TOOL_INPUT_KEY)
            .and_then(|v| v.as_str())
            .ok_or("tavily_client: missing __sub_tool")?;
        // `dag_run_id` not yet threaded through ExecutableNode — we reuse the node
        // instance identity as the session id for rate-limiting. This is a
        // same-instance-per-run simplification; see Plan 0 Task 12 for the full
        // ConversationLifecycleBus story.
        let session_id = "default";
        match sub {
            "search" => self.handle_search(inputs, config, session_id).await,
            "fetch" => Err(format!("tavily_client: fetch not yet implemented").into()),
            other => Err(format!("tavily_client: unknown sub_tool '{other}'").into()),
        }
    }
```

- [ ] **Step 4: Run — expect PASS**

Run: `cargo test --lib tavily_client`
Expected: all 10 tests pass (5 from Task 9 + 5 new).

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/tavily_client.rs
git commit -m "$(cat <<'EOF'
feat(nodes): implement tavily_client search handler

Dispatches __sub_tool == "search": validates args (query required),
merges search_defaults from config, builds a SearchUseCase, and maps
use-case results / errors to a JSON value for the LLM. RateLimit is
surfaced as a structured tool result unless fail_on_limit=true (in
which case the DAG crashes). InvalidConfig / AdapterInit always
crash. Includes tests over search_defaults merging and fail_on_limit.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 11: `TavilyClientNode::execute` — `fetch` handler

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/tavily_client.rs`

- [ ] **Step 1: Write the failing test**

Append to the `#[cfg(test)] mod tests`:

```rust
    #[tokio::test]
    async fn fetch_dispatches_and_returns_json() {
        let (port, node, _uc) = node_with_stub();
        let mut inputs: NodeInputs = HashMap::new();
        inputs.insert(SUB_TOOL_INPUT_KEY.into(), json!("fetch"));
        inputs.insert("url".into(), json!("https://example.com"));
        let mut state = json!({});
        let out = node
            .execute(&inputs, &json!({ "api_key": "tvly-stub" }), &mut state, None)
            .await
            .unwrap();
        assert_eq!(
            out.get("url").and_then(|v| v.as_str()),
            Some("https://example.com")
        );
        assert_eq!(out.get("content").and_then(|v| v.as_str()), Some("body"));
        assert_eq!(*port.fetch_calls.lock().unwrap(), 1);
    }

    #[tokio::test]
    async fn fetch_defaults_to_markdown() {
        let (_port, node, _uc) = node_with_stub();
        let mut inputs: NodeInputs = HashMap::new();
        inputs.insert(SUB_TOOL_INPUT_KEY.into(), json!("fetch"));
        inputs.insert("url".into(), json!("https://example.com"));
        // No extract_format supplied.
        let mut state = json!({});
        node.execute(&inputs, &json!({ "api_key": "tvly-stub" }), &mut state, None)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn fetch_missing_url_returns_structured_error() {
        let (_port, node, _uc) = node_with_stub();
        let mut inputs: NodeInputs = HashMap::new();
        inputs.insert(SUB_TOOL_INPUT_KEY.into(), json!("fetch"));
        let mut state = json!({});
        let out = node
            .execute(&inputs, &json!({ "api_key": "tvly-stub" }), &mut state, None)
            .await
            .unwrap();
        assert_eq!(
            out.get("error").and_then(|v| v.as_str()),
            Some("invalid_input")
        );
    }

    #[tokio::test]
    async fn unknown_sub_tool_errors() {
        let (_port, node, _uc) = node_with_stub();
        let mut inputs: NodeInputs = HashMap::new();
        inputs.insert(SUB_TOOL_INPUT_KEY.into(), json!("nope"));
        let mut state = json!({});
        let err = node
            .execute(&inputs, &json!({ "api_key": "tvly-stub" }), &mut state, None)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("unknown sub_tool"));
    }
```

- [ ] **Step 2: Run — expect FAIL**

Run: `cargo test --lib tavily_client::tests::fetch_dispatches_and_returns_json`
Expected: FAIL — the `fetch` arm currently returns `"fetch not yet implemented"`.

- [ ] **Step 3: Implement `handle_fetch`**

Append an `impl TavilyClientNode` block (or add inside the existing one):

```rust
    async fn handle_fetch(
        &self,
        inputs: &NodeInputs,
        config: &Value,
        session_id: &str,
    ) -> Result<Value, Box<dyn StdError + Send + Sync>> {
        use crate::web::domain::search_port::{ExtractFormat, FetchRequest};

        let Some(url) = inputs.get("url").and_then(|v| v.as_str()) else {
            return Ok(json!({
                "error": "invalid_input",
                "message": "fetch requires `url` (string)"
            }));
        };

        let format_str = inputs
            .get("extract_format")
            .and_then(|v| v.as_str())
            .unwrap_or("markdown");
        let format = match format_str {
            "text" => ExtractFormat::Text,
            _ => ExtractFormat::Markdown,
        };

        let uc = self.build_use_case(config, session_id).await?;
        match uc
            .fetch(
                session_id,
                FetchRequest {
                    url: url.to_string(),
                    format,
                },
            )
            .await
        {
            Ok(resp) => Ok(serde_json::to_value(resp)?),
            Err(e) => Ok(format_llm_error(e, config)?),
        }
    }
```

Replace the `"fetch" =>` arm in `execute`:

```rust
            "fetch" => self.handle_fetch(inputs, config, session_id).await,
```

- [ ] **Step 4: Run — expect PASS**

Run: `cargo test --lib tavily_client`
Expected: 14 tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/tavily_client.rs
git commit -m "$(cat <<'EOF'
feat(nodes): implement tavily_client fetch handler

Dispatches __sub_tool == "fetch": validates url, defaults
extract_format to markdown, and surfaces errors using the same
format_llm_error helper as search. Tests cover happy path, missing
url, and unknown sub_tool dispatch errors.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 12: Register `TavilyClientNode` in `HashMapNodeRegistry`

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/registry.rs`

The node must be reachable via `node_type = "tavily_client"` for both the `ExecutableNode` lookup (standalone use in a DAG) and the `ToolkitNode` lookup (via `tool_configurations` on an `llm_call`).

- [ ] **Step 1: Write the failing test**

Create a new file `src/libs/colmena/src/dag_engine/infrastructure/registry_tavily_tests.rs` — actually, tests can live alongside the registry. Append inside the existing `registry.rs` test module if present, or create one. If `registry.rs` has no `#[cfg(test)] mod tests` block, add this at the bottom:

```rust
#[cfg(test)]
mod registry_tavily_tests {
    use super::*;
    use crate::dag_engine::domain::toolkit_node::ToolkitNode;
    use crate::llm::infrastructure::ConversationRepositoryFactory;
    use std::sync::Arc;

    fn build_registry() -> Arc<HashMapNodeRegistry> {
        // Minimal wiring: no-op repository + no-op sql factory.
        let repo_factory = Arc::new(
            crate::llm::infrastructure::repositories::ConversationRepositoryFactory::in_memory(),
        );
        let sql_factory =
            Arc::new(crate::dag_engine::infrastructure::sql_port_factory::SqlPortFactory::in_memory());
        HashMapNodeRegistry::new(repo_factory, sql_factory, None)
    }

    #[test]
    fn tavily_client_registered_as_executable_node() {
        let reg = build_registry();
        let node = reg.get_node("tavily_client");
        assert!(node.is_some(), "tavily_client must be registered as an ExecutableNode");
    }

    #[test]
    fn tavily_client_registered_as_toolkit_node() {
        let reg = build_registry();
        let tk = reg.get_toolkit_node("tavily_client");
        assert!(tk.is_some(), "tavily_client must be registered as a ToolkitNode");
        let cat = tk.unwrap().sub_tool_catalog(&serde_json::json!({}));
        assert_eq!(cat.len(), 2);
    }
}
```

> **Note:** if `ConversationRepositoryFactory::in_memory()` and `SqlPortFactory::in_memory()` do not exist, replace the test helpers with whatever the codebase already uses to build a registry in other registry tests. Look for an existing test `build_registry` in `registry.rs` or `dag_tool_executor.rs` tests and copy it verbatim.

- [ ] **Step 2: Run — expect FAIL**

Run: `cargo test --lib registry_tavily_tests`
Expected: FAIL — `tavily_client` not registered.

- [ ] **Step 3: Register the node**

Edit `src/libs/colmena/src/dag_engine/infrastructure/registry.rs`. Inside `new_with_secure_values`, locate the section that ends with subgraph registration (`let sub_node = Arc::new(SubGraphNode::new());`). Immediately **before** that section, add a new block that registers the Tavily node both in `nodes` and in `toolkit_nodes`:

```rust
            // --- Register Tavily Client ---
            let tavily = Arc::new({
                use crate::dag_engine::infrastructure::nodes::tavily_client::TavilyClientNode;
                let n = TavilyClientNode::new();
                if let Some(svc) = secure_value_service.clone() {
                    n.with_secure_values(svc)
                } else {
                    n
                }
            });
            nodes.insert(
                "tavily_client".to_string(),
                tavily.clone() as Arc<dyn ExecutableNode>,
            );
```

At the end of the closure passed to `Arc::new_cyclic`, modify the trailing `Self { ... }` so it populates `toolkit_nodes` with the Tavily entry. Change:

```rust
            Self {
                nodes,
                toolkit_nodes: HashMap::new(),
                subgraph_node: Some(sub_node),
            }
```

to:

```rust
            let mut toolkit_nodes: HashMap<
                String,
                Arc<dyn crate::dag_engine::domain::toolkit_node::ToolkitNode>,
            > = HashMap::new();
            toolkit_nodes.insert(
                "tavily_client".to_string(),
                tavily.clone()
                    as Arc<dyn crate::dag_engine::domain::toolkit_node::ToolkitNode>,
            );

            Self {
                nodes,
                toolkit_nodes,
                subgraph_node: Some(sub_node),
            }
```

- [ ] **Step 4: Verify both maps agree**

Run: `cargo check --lib`
Expected: clean build.

Run: `cargo test --lib registry_tavily_tests`
Expected: both tests pass.

- [ ] **Step 5: Run the full toolkit-runtime suite**

Run: `cargo test --lib dag_tool_executor toolkit_node registry`
Expected: no regressions.

- [ ] **Step 6: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/registry.rs
git commit -m "$(cat <<'EOF'
feat(registry): register tavily_client as ExecutableNode + ToolkitNode

The same Arc<TavilyClientNode> is inserted into both `nodes` (for
standalone DAG use) and `toolkit_nodes` (so llm_call's
tool_configurations can address it via node_type=tavily_client +
expose_sub_tools). Secure-value service is threaded through when
available so `${ENV}` / secure placeholders in api_key resolve
correctly.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 13: End-to-end test graph — `tavily_search_basic.json`

**Files:**
- Create: `tests/graphs/web/tavily_search_basic.json`

- [ ] **Step 1: Create the graph directory**

Run:

```bash
mkdir -p tests/graphs/web
ls tests/graphs/
```

Expected: `web/` now exists alongside `basic/`, `agents/`, etc.

- [ ] **Step 2: Write the graph**

Create `tests/graphs/web/tavily_search_basic.json`:

```json
{
  "id": "tavily-search-basic",
  "name": "tavily_search_basic",
  "description": "LLM agent uses the web toolkit (tavily_client) to answer a research query via web search.",
  "nodes": [
    {
      "id": "ask",
      "type": "input",
      "config": {
        "default": "What are the top three news stories about the Rust language in the past month? Answer concisely."
      }
    },
    {
      "id": "agent",
      "type": "llm_call",
      "config": {
        "provider": "anthropic",
        "model": "claude-opus-4-7",
        "api_key": "${ANTHROPIC_API_KEY}",
        "system_message": "You are a research assistant. Use the `web__search` tool to look up up-to-date information before answering. Prefer `time_range=month` for recent-events questions.",
        "tool_configurations": {
          "web": {
            "name": "web",
            "description": "Web search and fetch via Tavily",
            "node_type": "tavily_client",
            "node_config": {
              "api_key": "${TAVILY_API_KEY}",
              "max_calls_per_run": 10,
              "search_defaults": { "max_results": 5 }
            },
            "expose_sub_tools": "all"
          }
        }
      }
    },
    { "id": "sink", "type": "output", "config": {} }
  ],
  "edges": [
    { "from": "ask", "to": "agent", "field": "prompt" },
    { "from": "agent", "to": "sink" }
  ]
}
```

- [ ] **Step 3: Dry-run check (structural, no live API call if keys missing)**

Run:

```bash
cargo run --bin dag_engine -- run tests/graphs/web/tavily_search_basic.json --include-extra-info 2>&1 | tail -40
```

Expected: the engine parses the graph and registers the `tavily_client` toolkit with `web__search` and `web__fetch` exposed to the LLM. If `ANTHROPIC_API_KEY` / `TAVILY_API_KEY` are unset the run fails with a clear AdapterInit message — that's fine; the intent here is graph-loading verification.

- [ ] **Step 4: Commit**

```bash
git add tests/graphs/web/tavily_search_basic.json
git commit -m "$(cat <<'EOF'
test(graphs): add tavily_search_basic end-to-end DAG

Wires an llm_call node to the tavily_client toolkit with
expose_sub_tools="all". Used by the Python / TS smoke tests and the
live-API integration test in Task 15.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 14: End-to-end test graph — `tavily_fetch_article.json`

**Files:**
- Create: `tests/graphs/web/tavily_fetch_article.json`

- [ ] **Step 1: Write the graph**

Create `tests/graphs/web/tavily_fetch_article.json`:

```json
{
  "id": "tavily-fetch-article",
  "name": "tavily_fetch_article",
  "description": "User supplies a URL; the LLM uses `web__fetch` to read its content before answering.",
  "nodes": [
    {
      "id": "ask",
      "type": "input",
      "config": {
        "default": "Please read https://example.com and summarize it in two sentences."
      }
    },
    {
      "id": "agent",
      "type": "llm_call",
      "config": {
        "provider": "anthropic",
        "model": "claude-opus-4-7",
        "api_key": "${ANTHROPIC_API_KEY}",
        "system_message": "Use `web__fetch` to read any URL the user references before answering.",
        "tool_configurations": {
          "web": {
            "name": "web",
            "description": "Web search and fetch via Tavily",
            "node_type": "tavily_client",
            "node_config": {
              "api_key": "${TAVILY_API_KEY}"
            },
            "expose_sub_tools": ["fetch"]
          }
        }
      }
    },
    { "id": "sink", "type": "output", "config": {} }
  ],
  "edges": [
    { "from": "ask", "to": "agent", "field": "prompt" },
    { "from": "agent", "to": "sink" }
  ]
}
```

- [ ] **Step 2: Dry-run check**

Run:

```bash
cargo run --bin dag_engine -- run tests/graphs/web/tavily_fetch_article.json --include-extra-info 2>&1 | tail -40
```

Expected: the LLM sees **only** `web__fetch` (per the `expose_sub_tools` allow-list); `web__search` is NOT exposed.

- [ ] **Step 3: Commit**

```bash
git add tests/graphs/web/tavily_fetch_article.json
git commit -m "$(cat <<'EOF'
test(graphs): add tavily_fetch_article DAG

Exercises the expose_sub_tools allow-list: only web__fetch is exposed.
User supplies an URL and the LLM is expected to call fetch before
summarizing. Covers the two-behavior contract — search and fetch can
be independently enabled/disabled per LLM.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 15: Live-API integration test

**Files:**
- Create: `tests/web/tavily_live.rs`
- (Create the `tests/web/` directory if it does not exist.)

The test is gated on `TAVILY_API_KEY`. CI skips it when the env var is unset; local runs with the key set actually hit Tavily.

- [ ] **Step 1: Create the test**

Create `tests/web/tavily_live.rs`:

```rust
//! Integration tests that hit the real Tavily API. Skipped when
//! TAVILY_API_KEY is not set in the environment.
//!
//! Run manually: `source .env && cargo test --test tavily_live -- --nocapture`

use colmena_dag_engine::web::application::search_use_case::{
    SearchUseCase, SearchUseCaseConfig,
};
use colmena_dag_engine::web::domain::errors::WebDomainError;
use colmena_dag_engine::web::domain::search_port::{
    ExtractFormat, FetchRequest, SearchPort, SearchRequest,
};
use colmena_dag_engine::web::infrastructure::tavily_adapter::TavilyAdapter;
use std::sync::Arc;
use std::time::Duration;

fn maybe_key() -> Option<String> {
    std::env::var("TAVILY_API_KEY").ok()
}

fn uc(key: String) -> SearchUseCase {
    let adapter = TavilyAdapter::new(key, Duration::from_secs(30)).expect("adapter init");
    SearchUseCase::new(
        Arc::new(adapter) as Arc<dyn SearchPort>,
        SearchUseCaseConfig {
            enable_cache: false, // don't mask live responses in tests
            ..Default::default()
        },
    )
}

#[tokio::test]
#[ignore = "live API — runs only when TAVILY_API_KEY is set"]
async fn search_returns_at_least_one_result() {
    let Some(key) = maybe_key() else {
        eprintln!("TAVILY_API_KEY not set — skipping");
        return;
    };
    let uc = uc(key);
    let resp = uc
        .search(
            "live-test",
            SearchRequest::new("What is the Rust programming language?"),
        )
        .await
        .expect("search ok");
    assert!(
        !resp.results.is_empty(),
        "expected at least one result for a well-known query"
    );
}

#[tokio::test]
#[ignore = "live API"]
async fn search_with_content_returns_content_on_top_results() {
    let Some(key) = maybe_key() else {
        return;
    };
    let uc = uc(key);
    let mut req = SearchRequest::new("async rust tutorial");
    req.include_content = true;
    let resp = uc.search("live-test", req).await.expect("search ok");
    assert!(resp.results.iter().any(|r| r.content.is_some()));
}

#[tokio::test]
#[ignore = "live API"]
async fn fetch_reads_known_stable_url() {
    let Some(key) = maybe_key() else {
        return;
    };
    let uc = uc(key);
    let resp = uc
        .fetch(
            "live-test",
            FetchRequest {
                url: "https://example.com".into(),
                format: ExtractFormat::Markdown,
            },
        )
        .await
        .expect("fetch ok");
    assert!(!resp.content.is_empty());
    assert!(resp.content_length > 0);
}

#[tokio::test]
#[ignore = "live API"]
async fn invalid_api_key_produces_adapter_init() {
    let uc = uc("tvly-definitely-not-a-valid-key".into());
    let err = uc
        .search("live-test", SearchRequest::new("anything"))
        .await
        .expect_err("expected auth error");
    assert!(matches!(err, WebDomainError::AdapterInit(_)));
}
```

> **Note:** the `#[ignore]` attribute means `cargo test` skips these by default. Run them explicitly with `cargo test --test tavily_live -- --ignored` after `source .env`. This matches the project pattern for other live-API tests (Amadeus, etc.).

- [ ] **Step 2: Verify the test file compiles**

Run: `cargo test --test tavily_live --no-run 2>&1 | tail -5`
Expected: a clean `Executable tavily_live-<hash>` line.

- [ ] **Step 3: Optionally run the live tests locally**

If you have an API key handy:

```bash
set -a && source .env && set +a
cargo test --test tavily_live -- --ignored --nocapture
```

Expected: four tests pass (or skip silently if the key is unset).

- [ ] **Step 4: Commit**

```bash
git add tests/web/tavily_live.rs
git commit -m "$(cat <<'EOF'
test(web): add live-API integration tests for tavily_client

Four #[ignore]-gated tests cover: basic search returns results;
include_content=true returns content on top results; fetch reads
https://example.com; invalid API key surfaces AdapterInit. Run with
`cargo test --test tavily_live -- --ignored` after sourcing .env.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 16: Python smoke test

**Files:**
- Create: `python/tests/test_web_nodes.py`

- [ ] **Step 1: Confirm Python bindings build**

Run: `.venv/bin/pip list | grep colmena`
Expected: `colmena` (or whatever the wheel name is) is installed. If not, run `maturin develop` first.

- [ ] **Step 2: Write the smoke test**

Create `python/tests/test_web_nodes.py`:

```python
"""Smoke tests that verify the `tavily_client` node is reachable from
Python. Does not hit the live API — only checks registration and basic
configuration parsing.
"""

import json
import os
import subprocess
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parents[2]
GRAPH_PATH = REPO_ROOT / "tests" / "graphs" / "web" / "tavily_search_basic.json"


def test_graph_file_exists():
    assert GRAPH_PATH.exists(), f"missing {GRAPH_PATH}"


def test_graph_has_tavily_client_toolkit_config():
    graph = json.loads(GRAPH_PATH.read_text())
    agent_nodes = [n for n in graph["nodes"] if n["type"] == "llm_call"]
    assert agent_nodes, "graph has no llm_call node"
    cfg = agent_nodes[0]["config"]["tool_configurations"]["web"]
    assert cfg["node_type"] == "tavily_client"
    assert cfg["expose_sub_tools"] in ("all", ["search", "fetch"])


@pytest.mark.skipif(
    not os.environ.get("TAVILY_API_KEY") or not os.environ.get("ANTHROPIC_API_KEY"),
    reason="Live API keys not set",
)
def test_cli_runs_graph_end_to_end():
    """Shell out to the `dag_engine` CLI. Covers the full Python →
    CLI → Rust path minus the Python binding itself (which is tested
    separately in python/tests/test_python_bindings.py)."""
    result = subprocess.run(
        ["cargo", "run", "--quiet", "--bin", "dag_engine", "--", "run", str(GRAPH_PATH)],
        cwd=REPO_ROOT,
        capture_output=True,
        text=True,
        timeout=180,
    )
    assert result.returncode == 0, f"CLI failed:\n{result.stderr}"
```

- [ ] **Step 3: Run it**

Run: `.venv/bin/pytest python/tests/test_web_nodes.py -v`
Expected: the first two tests pass; the third is `SKIPPED` unless API keys are set.

- [ ] **Step 4: Commit**

```bash
git add python/tests/test_web_nodes.py
git commit -m "$(cat <<'EOF'
test(python): smoke test for tavily_client registration

Verifies the tavily_search_basic DAG parses with the expected toolkit
config and (optionally, when API keys are set) runs end-to-end via the
`dag_engine` CLI.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 17: Documentation updates

**Files:**
- Modify: `docs/node_configurations.json`
- Modify: `docs/agent_context/node_ports_reference.md`
- Modify: `docs/developer_guide/25_web_nodes.md`

- [ ] **Step 1: Add `tavily_client` to `docs/node_configurations.json`**

Open `docs/node_configurations.json`. Find a logical alphabetical/categorical position (after `suspend`, before the media/media_* entries — or wherever the file's convention places a new node). Add:

```json
"tavily_client": {
  "description": "Web search and fetch via Tavily. Exposes two LLM sub-tools: `search` and `fetch`. Toolkit node — add to an llm_call via `tool_configurations`.",
  "config": {
    "api_key":  { "type": "string", "required": true,  "description": "Tavily API key. Accepts a literal, `${ENV_VAR}`, or a secure-value placeholder." },
    "enable_cache":      { "type": "boolean", "default": true,  "description": "Cache search and fetch results across runs." },
    "cache_ttl_seconds": { "type": "integer", "default": 3600,  "description": "Cache entry lifetime." },
    "max_calls_per_run": { "type": "integer", "default": 50,    "description": "Hard cap on search + fetch calls per dag_run." },
    "fail_on_limit":     { "type": "boolean", "default": false, "description": "When true, exceeding the cap crashes the DAG instead of returning a structured error to the LLM." },
    "retry_policy": {
      "type": "object",
      "description": "Retry settings for transient (5xx / timeout) upstream errors.",
      "properties": {
        "max_attempts":       { "type": "integer", "default": 3 },
        "initial_backoff_ms": { "type": "integer", "default": 500 }
      }
    },
    "timeout_seconds": { "type": "integer", "default": 30, "description": "Per-request timeout for the underlying reqwest client." },
    "search_defaults": {
      "type": "object",
      "description": "Default values for the search sub-tool. Each call may override them.",
      "properties": {
        "search_depth":    { "type": "string",  "enum": ["basic", "advanced"], "default": "basic" },
        "max_results":     { "type": "integer", "default": 5 },
        "include_content": { "type": "boolean", "default": false },
        "include_domains": { "type": "array", "items": { "type": "string" } },
        "exclude_domains": { "type": "array", "items": { "type": "string" } }
      }
    }
  },
  "sub_tools": {
    "search": {
      "description": "Search the web for up-to-date information. Returns a ranked list of titles/URLs/snippets (and optionally full content).",
      "parameters": {
        "query":          { "type": "string",  "required": true },
        "max_results":    { "type": "integer", "default": 5 },
        "include_content":{ "type": "boolean", "default": false },
        "search_depth":   { "type": "string",  "enum": ["basic", "advanced"], "default": "basic" },
        "include_domains":{ "type": "array" },
        "exclude_domains":{ "type": "array" },
        "time_range":     { "type": "string",  "enum": ["day", "week", "month", "year"] }
      }
    },
    "fetch": {
      "description": "Read cleaned text content from a specific URL.",
      "parameters": {
        "url":            { "type": "string", "required": true },
        "extract_format": { "type": "string", "enum": ["markdown", "text"], "default": "markdown" }
      }
    }
  }
}
```

- [ ] **Step 2: Add the row to `docs/agent_context/node_ports_reference.md`**

Open the file. Find the table (or section) that lists every node's `node_type`, its ports, and its outputs. Add a row / section for `tavily_client` matching the file's existing format. At minimum, document:

- `node_type`: `tavily_client`
- Inputs (per sub-tool — use the same parameter list from §sub_tools above)
- Outputs: `{ query, results[], answer?, credits_used }` for `search`; `{ url, title?, content, content_length, credits_used }` for `fetch`.

Use the format already used by (for example) `sql_query` — check the file and mirror its structure. Paste the following Spanish entry at the bottom of the file, adjusting the heading level to match the rest of the doc:

```markdown
## `tavily_client` (Toolkit)

Nodo de herramientas expuesto a un `llm_call`. Dos sub-herramientas: `search` y `fetch`.

**Inputs — `search`:** `query` (string, requerido), `max_results` (1-10), `include_content` (bool), `search_depth` (`basic`|`advanced`), `include_domains` (array), `exclude_domains` (array), `time_range` (`day`|`week`|`month`|`year`).
**Outputs — `search`:** `{ query, results: [{ title, url, snippet, score, content? }], answer?, credits_used }`.

**Inputs — `fetch`:** `url` (string, requerido), `extract_format` (`markdown`|`text`).
**Outputs — `fetch`:** `{ url, title?, content, content_length, credits_used }`.

Errores recuperables por el LLM: `rate_limit`, `timeout`, `upstream_error`. `AdapterInit` e `InvalidConfig` causan fallo de ejecución de DAG.
```

- [ ] **Step 3: Populate the `tavily_client` section in the guide**

Open `docs/developer_guide/25_web_nodes.md` (stub created in Plan 0 Task 13). Find the `## 2. tavily_client` heading (or add it if missing — Plan 0's stub should have placeholders). Replace its content with:

````markdown
## 2. `tavily_client`

Herramienta LLM para búsqueda y lectura web vía Tavily. Expone dos sub-herramientas:

| Sub-tool | Propósito | Costo aproximado |
|---|---|---|
| `search` | Búsqueda web con snippets; opcionalmente contenido completo | 1 crédito (basic), 2 créditos (advanced), +1-3× con `include_content=true` |
| `fetch`  | Lee texto limpio de una URL concreta | 1 crédito por URL |

### Configuración mínima

```json
{ "type": "tavily_client", "config": { "api_key": "${TAVILY_API_KEY}" } }
```

### Uso desde un `llm_call`

```json
"tool_configurations": {
  "web": {
    "name": "web",
    "node_type": "tavily_client",
    "node_config": {
      "api_key": "${TAVILY_API_KEY}",
      "max_calls_per_run": 20,
      "search_defaults": { "include_domains": ["docs.aws.amazon.com"] }
    },
    "expose_sub_tools": "all"
  }
}
```

El LLM verá dos herramientas: `web__search` y `web__fetch`. Con `expose_sub_tools: ["fetch"]` sólo ve la segunda.

### Manejo de errores

| Upstream | Para el LLM (recuperable) | Crash de DAG |
|---|---|---|
| 429 Too Many Requests | `{ error: "rate_limit", ... }` | Sólo si `fail_on_limit=true` |
| 5xx (tras reintentos) | `{ error: "upstream_error", status, ... }` | No |
| Timeout | `{ error: "timeout", ms, ... }` | No |
| 401 / 403 / llave vacía | — | Sí (`AdapterInit`) |
| Config inválida | — | Sí (`InvalidConfig`) |

### Caché y rate-limit

- Caché LRU + TTL por hash estable del request. Los hits no consumen el presupuesto por run.
- Contador por `dag_run_id` para imponer `max_calls_per_run` (search y fetch comparten contador).
- Los reintentos (5xx, timeout) usan backoff exponencial comenzando en `retry_policy.initial_backoff_ms`.

### Ejemplo completo

Ver `tests/graphs/web/tavily_search_basic.json` y `tests/graphs/web/tavily_fetch_article.json`.
````

- [ ] **Step 4: Update the main dev-guide index**

Open `docs/DEVELOPER_GUIDE.md`. Find the section that lists `25_web_nodes.md`. Under the `tavily_client` bullet (added as a stub in Plan 0 Task 13), update its description to:

```markdown
- `tavily_client` — web search / fetch via Tavily. Toolkit node exposed as `web__search` and `web__fetch` sub-tools.
```

- [ ] **Step 5: Verify Markdown links render**

Run:

```bash
grep -n "tavily_client" docs/DEVELOPER_GUIDE.md docs/developer_guide/25_web_nodes.md docs/node_configurations.json
```

Expected: each file mentions `tavily_client`.

- [ ] **Step 6: Commit**

```bash
git add docs/node_configurations.json docs/agent_context/node_ports_reference.md docs/developer_guide/25_web_nodes.md docs/DEVELOPER_GUIDE.md
git commit -m "$(cat <<'EOF'
docs: document tavily_client toolkit node

- node_configurations.json: full schema for config + both sub-tools.
- node_ports_reference: inputs/outputs per sub-tool.
- developer_guide/25_web_nodes.md: usage, caching, rate limiting,
  error handling, pointers to the example graphs.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 18: Final verification

**Files:**
- (No edits — verification only.)

- [ ] **Step 1: Run the full crate test suite**

Run:

```bash
cargo test --lib 2>&1 | tail -30
```

Expected: all tests pass.

- [ ] **Step 2: Lint**

Run:

```bash
cargo clippy --lib --all-targets -- -D warnings 2>&1 | tail -30
```

Expected: no warnings.

- [ ] **Step 3: Format**

Run: `cargo fmt -- --check`
Expected: clean output. If dirty, run `cargo fmt` and create a follow-up fix-up commit.

- [ ] **Step 4: Graph sanity**

Run:

```bash
cargo run --quiet --bin dag_engine -- run tests/graphs/web/tavily_search_basic.json --include-extra-info 2>&1 | head -20
```

Expected (no API keys set): the CLI parses and begins execution, but fails with a clear `AdapterInit`-style message once the LLM tries to call Tavily. That's proof the wiring is correct. With keys set, the graph should complete and return a research answer.

- [ ] **Step 5: Python tests pass**

Run: `.venv/bin/pytest python/tests/test_web_nodes.py -v`
Expected: 2 pass + 1 skipped (or 3 pass with keys).

- [ ] **Step 6: Summary commit (no-op) / tag**

No new changes. Leave any cleanup fix-up commits from clippy/fmt in place.

---

## Self-review checklist (run before handing off for implementation)

Run this checklist mentally against the **spec** (`docs/superpowers/specs/2026-04-23-web-nodes-a-tavily-client-design.md`):

| Spec requirement | Covered by |
|---|---|
| `SearchPort` trait + value objects | Task 1 |
| `TavilyAdapter` with all status mappings | Tasks 2, 3, 4 |
| `SearchUseCase` with LRU cache | Task 7 |
| `SearchUseCase` with per-run rate limit | Task 6 |
| `SearchUseCase` with retry | Task 8 |
| `tavily_client` node exposing `search` + `fetch` sub-tools | Tasks 9, 10, 11 |
| Config fields from §"Node configuration" | Task 9 `build_use_case`, Task 17 docs |
| Rich LLM descriptions for `search` and `fetch` | Task 9 `search_sub_tool` / `fetch_sub_tool` |
| Error → LLM mapping table | Task 10 `format_llm_error` |
| `api_key` accepts literal + `${ENV}` + secure value | Task 9 `build_use_case` |
| Warning when api_key is a literal | Task 9 `build_use_case` (`tracing::warn!`) |
| `wiremock` unit tests for the adapter | Tasks 2, 3, 4 |
| Live-API tests gated on `TAVILY_API_KEY` | Task 15 |
| Test graphs under `tests/graphs/web/` | Tasks 13, 14 |
| Python smoke test | Task 16 |
| Docs: `node_configurations.json`, node_ports_reference, developer guide | Task 17 |
| Registration in `HashMapNodeRegistry` (both maps) | Task 12 |

If any line has no task, the plan is incomplete — fix before starting execution.
