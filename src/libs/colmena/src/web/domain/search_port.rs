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
