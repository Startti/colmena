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
    #[cfg(test)]
    pub(crate) test_use_case: Option<Arc<SearchUseCase>>,
}

impl TavilyClientNode {
    pub fn new() -> Self {
        Self {
            secure_values: None,
            #[cfg(test)]
            test_use_case: None,
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
        #[cfg(test)]
        if let Some(uc) = &self.test_use_case {
            return Ok(uc.clone());
        }

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

impl TavilyClientNode {
    async fn handle_search(
        &self,
        inputs: &NodeInputs,
        config: &Value,
        session_id: &str,
    ) -> Result<Value, Box<dyn StdError + Send + Sync>> {
        use crate::web::domain::search_port::{SearchDepth, SearchRequest, TimeRange};

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
            Err(e) => format_llm_error(e, config),
        }
    }
}

#[async_trait]
impl ExecutableNode for TavilyClientNode {
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
        // `dag_run_id` not yet threaded through ExecutableNode — we reuse a
        // stable default session id for rate-limiting. See Plan 0 Task 12 for
        // the full ConversationLifecycleBus story.
        let session_id = "default";
        match sub {
            "search" => self.handle_search(inputs, config, session_id).await,
            "fetch" => Err("tavily_client: fetch not yet implemented".into()),
            other => Err(format!("tavily_client: unknown sub_tool '{other}'").into()),
        }
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

    use crate::web::application::search_use_case::{
        SearchUseCase as UcSearchUseCase, SearchUseCaseConfig as UcCfg,
    };
    use crate::web::domain::errors::WebDomainError;
    use crate::web::domain::search_port::{
        FetchRequest as FReq, FetchResponse as FResp, SearchPort,
        SearchRequest as SReq, SearchResponse as SResp, SearchResult,
    };
    use std::sync::Mutex;

    struct StubPort {
        search_calls: Mutex<u32>,
        fetch_calls: Mutex<u32>,
    }

    #[async_trait]
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

    fn node_with_stub() -> (Arc<StubPort>, TavilyClientNode, Arc<UcSearchUseCase>) {
        let port = Arc::new(StubPort {
            search_calls: Mutex::new(0),
            fetch_calls: Mutex::new(0),
        });
        let uc = Arc::new(UcSearchUseCase::new(
            port.clone() as Arc<dyn SearchPort>,
            UcCfg::default(),
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
        let uc = Arc::new(UcSearchUseCase::new(
            port.clone() as Arc<dyn SearchPort>,
            UcCfg {
                max_calls_per_run: 1,
                fail_on_limit: false,
                enable_cache: false,
                ..Default::default()
            },
        ));
        let mut node = TavilyClientNode::new();
        node.test_use_case = Some(uc);

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
        let uc = Arc::new(UcSearchUseCase::new(
            port.clone() as Arc<dyn SearchPort>,
            UcCfg {
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
}
