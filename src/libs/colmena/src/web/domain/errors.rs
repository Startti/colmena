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

    #[error("spec parse failed: {details}")]
    SpecParseFailed { details: String },

    #[error("unsupported spec format: {detected}")]
    UnsupportedSpecFormat { detected: String },

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

    #[error("spec too large: {size_bytes} bytes > {limit_bytes}")]
    SpecTooLarge { size_bytes: u64, limit_bytes: u64 },

    /// A Swagger 2.0 document could not be converted to OpenAPI 3.0.3.
    /// The `unsupported_feature` field pinpoints the bit that tripped us up.
    #[error("swagger 2.0 conversion failed: {reason}")]
    Swagger2ConversionFailed {
        reason: String,
        unsupported_feature: Option<String>,
    },

    #[error("missing required parameters: {missing:?}")]
    MissingRequiredParams {
        missing: Vec<String>,
        hints: Option<String>,
    },

    #[error("invalid param type: {param} — expected {expected_type}, got {got}")]
    InvalidParamType {
        param: String,
        expected_type: String,
        got: String,
    },

    #[error("missing auth for scheme {scheme}")]
    MissingAuth { scheme: String, message: String },
}

impl WebDomainError {
    /// Returns `true` when this error should be surfaced to the LLM as a structured
    /// tool result. Returns `false` for configuration / adapter init failures that
    /// should bubble up and crash the DAG.
    pub fn is_llm_recoverable(&self) -> bool {
        !matches!(
            self,
            Self::InvalidConfig(_) | Self::AdapterInit(_) | Self::SpecTooLarge { .. }
        )
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
    fn swagger2_conversion_failed_is_recoverable() {
        assert!(WebDomainError::Swagger2ConversionFailed {
            reason: "bad flow".into(),
            unsupported_feature: Some("oauth2.flow".into())
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

    #[test]
    fn spec_too_large_is_not_recoverable() {
        assert!(!WebDomainError::SpecTooLarge {
            size_bytes: 11_000_000,
            limit_bytes: 10_485_760,
        }
        .is_llm_recoverable());
    }

    #[test]
    fn spec_parse_failed_is_recoverable() {
        assert!(WebDomainError::SpecParseFailed {
            details: "json parse: unexpected token".into()
        }
        .is_llm_recoverable());
    }

    #[test]
    fn unsupported_spec_format_is_recoverable() {
        assert!(WebDomainError::UnsupportedSpecFormat {
            detected: "asyncapi 2.4.0".into()
        }
        .is_llm_recoverable());
    }

    #[test]
    fn missing_required_params_is_recoverable() {
        assert!(WebDomainError::MissingRequiredParams {
            missing: vec!["customer".into()],
            hints: None,
        }
        .is_llm_recoverable());
    }

    #[test]
    fn invalid_param_type_is_recoverable() {
        assert!(WebDomainError::InvalidParamType {
            param: "petId".into(),
            expected_type: "integer".into(),
            got: "\"not-a-number\"".into(),
        }
        .is_llm_recoverable());
    }

    #[test]
    fn missing_auth_is_recoverable() {
        assert!(WebDomainError::MissingAuth {
            scheme: "BearerAuth".into(),
            message: "no secret ref".into(),
        }
        .is_llm_recoverable());
    }
}
