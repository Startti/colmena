//! Domain errors shared across the three web-toolkit ports (search, api_spec, browser).
//!
//! The convention (per spec): variants whose `Display` message is stable and
//! LLM-addressable are returned to the LLM as structured tool results. Variants
//! categorized as "configuration/init" failures crash the DAG. `WebDomainError::is_llm_recoverable()`
//! classifies which is which for use-case layers.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum WebDomainError {
    // Crash the DAG (config/init). Not recoverable by the LLM.
    #[error("invalid config: {0}")]
    InvalidConfig(String),

    #[error("adapter init failed: {0}")]
    AdapterInit(String),

    // Returned to the LLM as structured results (recoverable).
    #[error("rate limit exceeded ({calls_used}/{cap})")]
    RateLimit { calls_used: u32, cap: u32 },

    #[error("session lost")]
    SessionLost { last_known_url: Option<String> },

    #[error("selector not found: {selector} on {page_url}")]
    SelectorNotFound {
        selector: String,
        page_url: String,
        hints: Vec<String>,
    },

    #[error("navigation failed: {0}")]
    NavigationFailed(String),

    #[error("timeout after {ms}ms")]
    Timeout { ms: u64 },

    #[error("spec parse failed: {0}")]
    SpecParseError(String),

    #[error("endpoint not found: {searched_for}")]
    EndpointNotFound {
        searched_for: String,
        did_you_mean: Vec<String>,
    },

    #[error("upstream {status}: {body}")]
    Upstream { status: u16, body: String },

    #[error("session cap reached ({active}/{cap})")]
    SessionCapReached { active: u32, cap: u32 },

    #[error("unexpected HTML response from {url}")]
    UnexpectedHtmlResponse { url: String, resolved_url: String },
}

impl WebDomainError {
    /// Returns `true` when this error should be surfaced to the LLM as a structured
    /// tool result. Returns `false` for configuration / adapter init failures that
    /// should bubble up and crash the DAG.
    pub fn is_llm_recoverable(&self) -> bool {
        !matches!(self, Self::InvalidConfig(_) | Self::AdapterInit(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_config_crashes_dag() {
        assert!(!WebDomainError::InvalidConfig("bad".into()).is_llm_recoverable());
    }

    #[test]
    fn adapter_init_crashes_dag() {
        assert!(!WebDomainError::AdapterInit("no token".into()).is_llm_recoverable());
    }

    #[test]
    fn rate_limit_is_recoverable() {
        assert!(WebDomainError::RateLimit {
            calls_used: 51,
            cap: 50
        }
        .is_llm_recoverable());
    }

    #[test]
    fn session_lost_is_recoverable() {
        assert!(WebDomainError::SessionLost {
            last_known_url: None
        }
        .is_llm_recoverable());
    }

    #[test]
    fn timeout_is_recoverable() {
        assert!(WebDomainError::Timeout { ms: 3000 }.is_llm_recoverable());
    }

    #[test]
    fn upstream_is_recoverable() {
        assert!(WebDomainError::Upstream {
            status: 502,
            body: "bad gateway".into()
        }
        .is_llm_recoverable());
    }

    #[test]
    fn display_uses_thiserror_message() {
        let e = WebDomainError::RateLimit {
            calls_used: 51,
            cap: 50,
        };
        assert_eq!(e.to_string(), "rate limit exceeded (51/50)");
    }
}
