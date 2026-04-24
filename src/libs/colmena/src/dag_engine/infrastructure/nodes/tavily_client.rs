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
        name: "search".into(),
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
        name: "fetch".into(),
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
