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
