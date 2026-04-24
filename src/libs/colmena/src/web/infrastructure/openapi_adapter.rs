//! Reqwest-backed implementation of `ApiSpecPort`.
//!
//! Pipeline (see Spec C §Architecture):
//!   1. Normalize URL (Git-forge blob → raw).
//!   2. GET with streamed body; abort if size > cap.
//!   3. Detect HTML-instead-of-spec responses.
//!   4. Detect JSON vs YAML.
//!   5. Detect OpenAPI 3.x vs Swagger 2.0 by root keys.
//!   6. Convert Swagger 2.0 → OpenAPI 3.0 if needed.
//!   7. Parse via `oas3` → map to `ParsedSpec`.
//!   8. Honor ETag / Last-Modified (304).
//!
//! This module contains Tasks 5–7's incremental slices. Tests in this
//! file are structured by pipeline stage so later tasks can add more
//! without breaking earlier ones.

use crate::web::application::url_normalizer::{normalize_forge_url, NormalizedUrl};
use crate::web::domain::{ApiSpecPort, SpecFetchResult, WebDomainError};
use async_trait::async_trait;
use std::time::Duration;

/// Caps and limits shared across the pipeline stages.
#[derive(Debug, Clone)]
pub struct OpenApiAdapterConfig {
    pub max_bytes: u64,
    pub timeout: Duration,
}

impl Default for OpenApiAdapterConfig {
    fn default() -> Self {
        Self {
            max_bytes: 10 * 1024 * 1024,
            timeout: Duration::from_secs(60),
        }
    }
}

pub struct OpenApiAdapter {
    client: reqwest::Client,
    config: OpenApiAdapterConfig,
}

impl OpenApiAdapter {
    pub fn new(config: OpenApiAdapterConfig) -> Result<Self, WebDomainError> {
        let client = reqwest::Client::builder()
            .timeout(config.timeout)
            .user_agent("colmena-api-explorer/0.1")
            .build()
            .map_err(|e| WebDomainError::AdapterInit(format!("reqwest client init: {e}")))?;
        Ok(Self { client, config })
    }

    /// Lower-level fetch stage. Returns the raw bytes plus response
    /// metadata (content-type, ETag, Last-Modified, final URL). Used by
    /// Task 6/7's parsing layer.
    pub(crate) async fn fetch_raw(
        &self,
        input_url: &str,
        if_none_match: Option<&str>,
        if_modified_since: Option<&str>,
    ) -> Result<FetchRawResult, WebDomainError> {
        let NormalizedUrl { resolved, rewritten: _ } = normalize_forge_url(input_url);

        let mut req = self.client.get(&resolved);
        if let Some(etag) = if_none_match {
            req = req.header("If-None-Match", etag);
        }
        if let Some(lm) = if_modified_since {
            req = req.header("If-Modified-Since", lm);
        }

        let resp = req.send().await.map_err(|e| {
            if e.is_timeout() {
                WebDomainError::Timeout { ms: self.config.timeout.as_millis() as u64 }
            } else {
                WebDomainError::Upstream {
                    status: 0,
                    body: format!("fetch error: {e}"),
                }
            }
        })?;

        let status = resp.status();
        if status.as_u16() == 304 {
            return Ok(FetchRawResult::NotModified);
        }
        if !status.is_success() {
            return Err(WebDomainError::Upstream {
                status: status.as_u16(),
                body: format!("HTTP {} from {resolved}", status.as_u16()),
            });
        }

        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(String::from);
        let etag = resp
            .headers()
            .get(reqwest::header::ETAG)
            .and_then(|v| v.to_str().ok())
            .map(String::from);
        let last_modified = resp
            .headers()
            .get(reqwest::header::LAST_MODIFIED)
            .and_then(|v| v.to_str().ok())
            .map(String::from);

        // Reject obvious HTML before spending bytes on download.
        if let Some(ct) = &content_type {
            if ct.starts_with("text/html") {
                return Err(WebDomainError::UnexpectedHtmlResponse {
                    url: input_url.to_string(),
                    resolved_url: resolved.clone(),
                });
            }
        }

        // Stream body with size cap. reqwest's content-length hint is best-effort;
        // we count bytes as they arrive and abort past the cap.
        let mut stream = resp.bytes_stream();
        let mut buf: Vec<u8> = Vec::new();
        use futures::StreamExt;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| WebDomainError::Upstream {
                status: 0,
                body: format!("stream error: {e}"),
            })?;
            if (buf.len() as u64) + (chunk.len() as u64) > self.config.max_bytes {
                return Err(WebDomainError::SpecTooLarge {
                    size_bytes: buf.len() as u64 + chunk.len() as u64,
                    limit_bytes: self.config.max_bytes,
                });
            }
            buf.extend_from_slice(&chunk);
        }

        // If content-type was absent, sniff body: HTML bodies usually start with '<'.
        if content_type.is_none() {
            let first = buf
                .iter()
                .copied()
                .find(|b| !b.is_ascii_whitespace());
            if matches!(first, Some(b'<')) {
                return Err(WebDomainError::UnexpectedHtmlResponse {
                    url: input_url.to_string(),
                    resolved_url: resolved.clone(),
                });
            }
        }

        Ok(FetchRawResult::Fresh {
            body: buf,
            content_type,
            etag,
            last_modified,
            resolved_url: resolved,
        })
    }
}

#[derive(Debug)]
pub(crate) enum FetchRawResult {
    Fresh {
        body: Vec<u8>,
        content_type: Option<String>,
        etag: Option<String>,
        last_modified: Option<String>,
        resolved_url: String,
    },
    NotModified,
}

/// Temporary `ApiSpecPort` impl — Task 6 replaces the body with real parsing.
#[async_trait]
impl ApiSpecPort for OpenApiAdapter {
    async fn fetch_and_parse(
        &self,
        _url: &str,
        _etag: Option<&str>,
        _last_modified: Option<&str>,
    ) -> Result<SpecFetchResult, WebDomainError> {
        Err(WebDomainError::AdapterInit(
            "OpenApiAdapter::fetch_and_parse not yet implemented — Task 6 wires it up".into(),
        ))
    }
}

#[cfg(test)]
mod tests_fetch {
    use super::*;
    use wiremock::matchers::{header_exists, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn adapter_at(config: OpenApiAdapterConfig) -> OpenApiAdapter {
        OpenApiAdapter::new(config).unwrap()
    }

    fn small_yaml() -> &'static str {
        "openapi: 3.0.3\ninfo:\n  title: T\n  version: '1'\npaths: {}\n"
    }

    #[tokio::test]
    async fn fetch_raw_returns_yaml_body() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/openapi.yaml"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("ETag", "\"abc\"")
                    .set_body_raw(small_yaml(), "application/yaml"),
            )
            .mount(&server)
            .await;

        let url = format!("{}/openapi.yaml", server.uri());
        let adapter = adapter_at(OpenApiAdapterConfig::default());
        let res = adapter.fetch_raw(&url, None, None).await.unwrap();
        match res {
            FetchRawResult::Fresh { body, content_type, etag, .. } => {
                assert!(std::str::from_utf8(&body).unwrap().contains("openapi: 3.0.3"));
                assert_eq!(content_type.as_deref(), Some("application/yaml"));
                assert_eq!(etag.as_deref(), Some("\"abc\""));
            }
            FetchRawResult::NotModified => panic!("expected Fresh"),
        }
    }

    #[tokio::test]
    async fn fetch_raw_rejects_html_by_content_type() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_raw("<!DOCTYPE html><html>...", "text/html; charset=utf-8"),
            )
            .mount(&server)
            .await;

        let url = format!("{}/", server.uri());
        let adapter = adapter_at(OpenApiAdapterConfig::default());
        let err = adapter.fetch_raw(&url, None, None).await.unwrap_err();
        match err {
            WebDomainError::UnexpectedHtmlResponse { resolved_url, .. } => {
                assert_eq!(resolved_url, url);
            }
            other => panic!("expected UnexpectedHtmlResponse, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn fetch_raw_rejects_html_body_when_content_type_missing() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"   <!DOCTYPE html>...".as_ref()))
            .mount(&server)
            .await;

        let url = format!("{}/", server.uri());
        let adapter = adapter_at(OpenApiAdapterConfig::default());
        let err = adapter.fetch_raw(&url, None, None).await.unwrap_err();
        assert!(matches!(err, WebDomainError::UnexpectedHtmlResponse { .. }));
    }

    #[tokio::test]
    async fn fetch_raw_enforces_size_cap() {
        let server = MockServer::start().await;
        let big = "x".repeat(10_000);
        Mock::given(method("GET"))
            .and(path("/big.yaml"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_raw(big, "application/yaml"),
            )
            .mount(&server)
            .await;

        let url = format!("{}/big.yaml", server.uri());
        let adapter = adapter_at(OpenApiAdapterConfig {
            max_bytes: 1024,
            ..OpenApiAdapterConfig::default()
        });
        let err = adapter.fetch_raw(&url, None, None).await.unwrap_err();
        match err {
            WebDomainError::SpecTooLarge { limit_bytes, .. } => {
                assert_eq!(limit_bytes, 1024);
            }
            other => panic!("expected SpecTooLarge, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn fetch_raw_propagates_if_none_match() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/openapi.yaml"))
            .and(header_exists("If-None-Match"))
            .respond_with(ResponseTemplate::new(304))
            .mount(&server)
            .await;

        let url = format!("{}/openapi.yaml", server.uri());
        let adapter = adapter_at(OpenApiAdapterConfig::default());
        let res = adapter
            .fetch_raw(&url, Some("\"abc\""), None)
            .await
            .unwrap();
        assert!(matches!(res, FetchRawResult::NotModified));
    }

    #[tokio::test]
    async fn fetch_raw_maps_500_to_upstream_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/openapi.yaml"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let url = format!("{}/openapi.yaml", server.uri());
        let adapter = adapter_at(OpenApiAdapterConfig::default());
        let err = adapter.fetch_raw(&url, None, None).await.unwrap_err();
        match err {
            WebDomainError::Upstream { status: 500, .. } => {}
            other => panic!("expected Upstream(500), got {other:?}"),
        }
    }
}
