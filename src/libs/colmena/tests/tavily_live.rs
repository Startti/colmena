//! Integration tests that hit the real Tavily API. Skipped when
//! TAVILY_API_KEY is not set in the environment.
//!
//! Run manually: `source .env && cargo test --test tavily_live -- --ignored --nocapture`

use colmena::web::application::search_use_case::{SearchUseCase, SearchUseCaseConfig};
use colmena::web::domain::errors::WebDomainError;
use colmena::web::domain::search_port::{ExtractFormat, FetchRequest, SearchPort, SearchRequest};
use colmena::web::infrastructure::tavily_adapter::TavilyAdapter;
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
            enable_cache: false,
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
