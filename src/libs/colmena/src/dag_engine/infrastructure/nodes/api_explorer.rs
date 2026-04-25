//! `api_explorer` toolkit node. Exposes five LLM sub-tools — `load_spec`,
//! `list_endpoints`, `search_endpoint`, `get_endpoint_details`,
//! `build_http_request` — over a cached OpenAPI 3.x / Swagger 2.0 spec.
//!
//! Spec: docs/superpowers/specs/2026-04-23-web-nodes-c-api-explorer-design.md
//!
//! The node holds a single [`ApiSpecUseCase`] plus its shared
//! [`SessionRegistry`] so per-conversation spec caches survive across
//! sub-tool calls. It subscribes to [`ConversationLifecycleBus`] so the
//! registry is evicted eagerly when a conversation closes.

use crate::dag_engine::application::secure_value_service::SecureValueService;
use crate::dag_engine::domain::node::{ExecutableNode, NodeInputs};
use crate::dag_engine::domain::observer::ExecutionObserver;
use crate::dag_engine::domain::toolkit_node::{SubToolDefinition, ToolkitNode, SUB_TOOL_INPUT_KEY};
use crate::llm::domain::ParameterProperty;
use crate::web::application::api_spec_use_case::{
    ApiSpecUseCase, ApiSpecUseCaseConfig, SpecCache,
};
use crate::web::domain::api_spec_port::ApiSpecPort;
use crate::web::domain::lifecycle::ConversationLifecycleSubscriber;
use crate::web::domain::{SessionRegistry, TtlConfig};
use crate::web::infrastructure::openapi_adapter::{OpenApiAdapter, OpenApiAdapterConfig};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::error::Error as StdError;
use std::sync::Arc;

/// `api_explorer` node.
///
/// Construction happens once at registry build time. The adapter is
/// stateless (no API key) so a single `ApiSpecUseCase` is shared across
/// calls. The per-conversation cache lives inside the
/// `SessionRegistry<Arc<SpecCache>>` owned by the use case.
pub struct ApiExplorerNode {
    use_case: Arc<ApiSpecUseCase>,
    registry: Arc<SessionRegistry<Arc<SpecCache>>>,
    #[allow(dead_code)] // used in Tasks 14-15 (build_http_request needs secret resolution)
    secure_values: Option<Arc<SecureValueService>>,
}

impl ApiExplorerNode {
    pub fn new() -> Self {
        let port: Arc<dyn ApiSpecPort> = Arc::new(
            OpenApiAdapter::new(OpenApiAdapterConfig::default())
                .expect("OpenApiAdapter init failed"),
        );
        let registry = SessionRegistry::<Arc<SpecCache>>::new(TtlConfig::default());
        let cfg = ApiSpecUseCaseConfig::default();
        let use_case = Arc::new(ApiSpecUseCase::new(port, registry.clone(), cfg));
        Self {
            use_case,
            registry,
            secure_values: None,
        }
    }

    /// Build a node with a custom port — used by tests that inject a
    /// `CountingPort` or similar. Not part of the public API.
    #[cfg(test)]
    pub(crate) fn new_with_port(port: Arc<dyn ApiSpecPort>) -> Self {
        Self::new_with_port_and_config(port, ApiSpecUseCaseConfig::default())
    }

    #[cfg(test)]
    pub(crate) fn new_with_port_and_config(
        port: Arc<dyn ApiSpecPort>,
        cfg: ApiSpecUseCaseConfig,
    ) -> Self {
        let registry = SessionRegistry::<Arc<SpecCache>>::new(TtlConfig::default());
        let use_case = Arc::new(ApiSpecUseCase::new(port, registry.clone(), cfg));
        Self {
            use_case,
            registry,
            secure_values: None,
        }
    }

    pub fn with_secure_values(mut self, svc: Arc<SecureValueService>) -> Self {
        self.secure_values = Some(svc);
        self
    }

    /// Registry handle so the registrar can subscribe the node to a
    /// `ConversationLifecycleBus`.
    pub fn registry(&self) -> Arc<SessionRegistry<Arc<SpecCache>>> {
        self.registry.clone()
    }

    /// Extract `conversation_id` from node inputs. Toolkit executor passes
    /// this through from the llm_call parent. Falls back to "default" so
    /// the node remains usable when the engine does not supply one.
    fn extract_conversation_id(inputs: &NodeInputs) -> String {
        inputs
            .get("conversation_id")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| "default".into())
    }

    /// Helper — read a required string field from the LLM's argument map.
    /// Returns a structured LLM-recoverable error JSON on miss.
    pub(crate) fn require_str<'a>(inputs: &'a NodeInputs, key: &str) -> Result<&'a str, Value> {
        match inputs.get(key).and_then(|v| v.as_str()) {
            Some(s) if !s.is_empty() => Ok(s),
            _ => Err(json!({
                "error": "invalid_input",
                "missing": key,
                "message": format!("`{key}` is required (string)")
            })),
        }
    }

    /// Handler for the `load_spec` sub-tool. Fetches (or reuses the
    /// cached) spec for this conversation and returns the summary the
    /// LLM consumes.
    async fn handle_load_spec(
        &self,
        inputs: &NodeInputs,
        _config: &Value,
        conversation_id: &str,
    ) -> Result<Value, Box<dyn StdError + Send + Sync>> {
        let url = match Self::require_str(inputs, "url") {
            Ok(u) => u.to_string(),
            Err(v) => return Ok(v),
        };
        let force_reload = inputs
            .get("force_reload")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        match self
            .use_case
            .fetch_spec(conversation_id, &url, force_reload)
            .await
        {
            Ok((entry, was_cached)) => {
                let parsed = &entry.parsed;
                let security_schemes: Vec<String> =
                    parsed.security_schemes.keys().cloned().collect();
                Ok(json!({
                    "spec_url_input": url,
                    "resolved_url": parsed.resolved_url,
                    "original_format": parsed.original_format.as_str(),
                    "internal_format": parsed.internal_format,
                    "title": parsed.title,
                    "version": parsed.version,
                    "description": parsed.description,
                    "server_url": parsed.servers.first().cloned().unwrap_or_default(),
                    "endpoints_count": parsed.endpoints.len(),
                    "tags": parsed.tags,
                    "security_schemes": security_schemes,
                    "cached": was_cached,
                }))
            }
            Err(e) => Ok(format_spec_error(e)),
        }
    }

    /// Handler for the `list_endpoints` sub-tool. Returns a paginated
    /// summary; `spec_not_loaded` if `load_spec` was never called for
    /// this conversation.
    async fn handle_list_endpoints(
        &self,
        inputs: &NodeInputs,
        conversation_id: &str,
    ) -> Result<Value, Box<dyn StdError + Send + Sync>> {
        let spec_url = match Self::require_str(inputs, "spec_url") {
            Ok(u) => u.to_string(),
            Err(v) => return Ok(v),
        };
        let tag = inputs
            .get("tag")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let limit = inputs
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(50)
            .clamp(1, 200) as usize;
        let offset = inputs
            .get("offset")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize;

        match self
            .use_case
            .list_endpoints(conversation_id, &spec_url, tag.as_deref(), limit, offset)
            .await
        {
            Ok(page) => Ok(json!({
                "total": page.total,
                "returned": page.returned,
                "offset": page.offset,
                "endpoints": page
                    .endpoints
                    .iter()
                    .map(|e| json!({
                        "operation_id": e.operation_id,
                        "method": e.method,
                        "path": e.path,
                        "summary": e.summary,
                        "tags": e.tags,
                    }))
                    .collect::<Vec<_>>(),
            })),
            Err(e) => Ok(format_spec_error(e)),
        }
    }

    /// Handler for the `search_endpoint` sub-tool.
    async fn handle_search_endpoint(
        &self,
        inputs: &NodeInputs,
        conversation_id: &str,
    ) -> Result<Value, Box<dyn StdError + Send + Sync>> {
        let spec_url = match Self::require_str(inputs, "spec_url") {
            Ok(u) => u.to_string(),
            Err(v) => return Ok(v),
        };
        let query = match Self::require_str(inputs, "query") {
            Ok(q) => q.to_string(),
            Err(v) => return Ok(v),
        };
        let method_filter = inputs
            .get("method")
            .and_then(|v| v.as_str())
            .map(str::to_ascii_uppercase);
        let max_results = inputs
            .get("max_results")
            .and_then(|v| v.as_u64())
            .unwrap_or(10)
            .clamp(1, 50) as usize;

        match self
            .use_case
            .search_endpoint(
                conversation_id,
                &spec_url,
                &query,
                method_filter.as_deref(),
                max_results,
            )
            .await
        {
            Ok(results) => Ok(json!({
                "query": query,
                "results": results
                    .into_iter()
                    .map(|r| json!({
                        "operation_id": r.operation_id,
                        "method": r.method,
                        "path": r.path,
                        "summary": r.summary,
                        "score": r.score,
                        "match_reason": r.match_reason,
                    }))
                    .collect::<Vec<_>>(),
            })),
            Err(e) => Ok(format_spec_error(e)),
        }
    }

    /// Handler for the `get_endpoint_details` sub-tool. Wraps
    /// [`ApiSpecUseCase::get_endpoint_details`] and forwards its JSON
    /// shape directly. EndpointNotFound carries `did_you_mean`.
    async fn handle_get_endpoint_details(
        &self,
        inputs: &NodeInputs,
        conversation_id: &str,
    ) -> Result<Value, Box<dyn StdError + Send + Sync>> {
        let spec_url = match Self::require_str(inputs, "spec_url") {
            Ok(u) => u.to_string(),
            Err(v) => return Ok(v),
        };
        let operation_id = match Self::require_str(inputs, "operation_id") {
            Ok(o) => o.to_string(),
            Err(v) => return Ok(v),
        };

        match self
            .use_case
            .get_endpoint_details(conversation_id, &spec_url, &operation_id)
            .await
        {
            Ok(details) => Ok(details),
            Err(e) => Ok(format_spec_error(e)),
        }
    }

    /// Handler for the `build_http_request` sub-tool. Returns the JSON
    /// envelope the `http_request` node consumes; auth secrets travel
    /// as `${SECURE:<ref>}` placeholders that are resolved at execute
    /// time.
    async fn handle_build_http_request(
        &self,
        inputs: &NodeInputs,
        conversation_id: &str,
    ) -> Result<Value, Box<dyn StdError + Send + Sync>> {
        let spec_url = match Self::require_str(inputs, "spec_url") {
            Ok(u) => u.to_string(),
            Err(v) => return Ok(v),
        };
        let operation_id = match Self::require_str(inputs, "operation_id") {
            Ok(o) => o.to_string(),
            Err(v) => return Ok(v),
        };
        let params = match inputs.get("params") {
            Some(v) if v.is_object() => v.clone(),
            _ => {
                return Ok(json!({
                    "error": "invalid_input",
                    "missing": "params",
                    "message": "`params` must be a JSON object mapping parameter names to values",
                }));
            }
        };
        let auth_secret_ref = inputs
            .get("auth_secret_ref")
            .and_then(|v| v.as_str())
            .map(str::to_string);

        match self
            .use_case
            .build_http_request(
                conversation_id,
                &spec_url,
                &operation_id,
                &params,
                auth_secret_ref.as_deref(),
            )
            .await
        {
            Ok(request_value) => Ok(request_value),
            Err(e) => Ok(format_spec_error(e)),
        }
    }
}

/// Translate a [`WebDomainError`] into the structured JSON the LLM sees.
///
/// Non-recoverable variants (`InvalidConfig`, `AdapterInit`,
/// `SpecTooLarge`) bubble out as errors so the DAG crashes; everything
/// else maps to a stable `error` discriminator the model can branch on.
fn format_spec_error(e: crate::web::domain::WebDomainError) -> Value {
    use crate::web::domain::WebDomainError as E;
    match e {
        E::SpecParseFailed { details } => json!({
            "error": "spec_parse_failed",
            "details": details,
            "message": "Spec could not be parsed as OpenAPI 3.x or Swagger 2.0."
        }),
        E::UnexpectedHtmlResponse { url, resolved_url } => json!({
            "error": "unexpected_html_response",
            "url_given": url,
            "resolved_url": resolved_url,
            "message": "URL returned HTML. If this is a Git-forge blob URL for a lesser-known host, use the raw content URL instead."
        }),
        E::Swagger2ConversionFailed { reason, unsupported_feature } => json!({
            "error": "swagger2_conversion_failed",
            "reason": reason,
            "unsupported_feature": unsupported_feature,
            "message": "This Swagger 2.0 spec uses a feature the converter does not handle. Fall back to reading docs with web__fetch."
        }),
        E::UnsupportedSpecFormat { detected } => json!({
            "error": "unsupported_spec_format",
            "detected": detected,
            "message": "api_explorer supports OpenAPI 3.x and Swagger 2.0 only."
        }),
        E::EndpointNotFound { searched_for, did_you_mean } => json!({
            "error": "endpoint_not_found",
            "searched_for": searched_for,
            "did_you_mean": did_you_mean,
        }),
        E::MissingRequiredParams { missing, hints } => json!({
            "error": "missing_required_params",
            "missing": missing,
            "hints": hints,
        }),
        E::InvalidParamType { param, expected_type, got } => json!({
            "error": "invalid_param_type",
            "param": param,
            "expected_type": expected_type,
            "got": got,
        }),
        E::MissingAuth { scheme, message } => json!({
            "error": "missing_auth",
            "scheme": scheme,
            "message": message,
        }),
        E::SpecNotLoaded { spec_url } => json!({
            "error": "spec_not_loaded",
            "spec_url": spec_url,
            "message": "Call load_spec(url) first.",
        }),
        E::Timeout { ms } => json!({
            "error": "fetch_failed",
            "reason": "timeout",
            "ms": ms,
            "retryable": true,
        }),
        E::Upstream { status, body } => json!({
            "error": "fetch_failed",
            "status": status,
            "retryable": status >= 500,
            "message": body,
        }),
        E::RateLimit { calls_used, cap } => json!({
            "error": "rate_limit",
            "calls_used": calls_used,
            "cap": cap,
        }),
        E::SessionLost { last_known_url } => json!({
            "error": "session_lost",
            "last_known_url": last_known_url,
        }),
        E::SelectorNotFound { selector, page_url, hints } => json!({
            "error": "selector_not_found",
            "selector": selector,
            "page_url": page_url,
            "hints": hints,
        }),
        E::NavigationFailed(msg) => json!({
            "error": "navigation_failed",
            "message": msg,
        }),
        E::SessionCapReached { active, cap } => json!({
            "error": "session_cap_reached",
            "active": active,
            "cap": cap,
        }),
        other => json!({
            "error": "web_error",
            "message": other.to_string(),
        }),
    }
}

impl Default for ApiExplorerNode {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ConversationLifecycleSubscriber for ApiExplorerNode {
    async fn on_conversation_closed(&self, conversation_id: &str) {
        self.registry.cleanup_conversation(conversation_id, |_v| {}).await;
    }
}

#[async_trait]
impl ExecutableNode for ApiExplorerNode {
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
            .ok_or("api_explorer: missing __sub_tool")?;
        let conversation_id = Self::extract_conversation_id(inputs);

        match sub {
            "load_spec" => self.handle_load_spec(inputs, config, &conversation_id).await,
            "list_endpoints" => self.handle_list_endpoints(inputs, &conversation_id).await,
            "search_endpoint" => self.handle_search_endpoint(inputs, &conversation_id).await,
            "get_endpoint_details" => {
                self.handle_get_endpoint_details(inputs, &conversation_id)
                    .await
            }
            "build_http_request" => {
                self.handle_build_http_request(inputs, &conversation_id)
                    .await
            }
            other => Ok(json!({
                "error": "unknown_sub_tool",
                "sub_tool": other,
                "message": "Use one of: load_spec, list_endpoints, search_endpoint, get_endpoint_details, build_http_request"
            })),
        }
    }

    fn schema(&self) -> Value {
        json!({
            "inputs": { "__sub_tool": "string" },
            "outputs": { "output": "any" },
            "config": {
                "enable_cache": "bool (default true)",
                "cache_ttl_seconds": "u64 (default 86400)",
                "max_cached_specs": "u64 (default 100)",
                "session_idle_ttl_seconds": "u64 (default 900)",
                "session_max_lifetime_seconds": "u64 (default 3600)",
                "max_spec_size_bytes": "u64 (default 10 MiB)",
                "spec_download_timeout_seconds": "u64 (default 60)",
                "default_base_url_override": "string | null",
                "fuzzy_match_threshold": "f32 (default 0.6)",
                "retry_policy": { "max_attempts": "u32", "initial_backoff_ms": "u64" }
            }
        })
    }

    fn description(&self) -> Option<&str> {
        Some(
            "OpenAPI / Swagger 2.0 discovery + request builder. Exposes five sub-tools: \
             load_spec, list_endpoints, search_endpoint, get_endpoint_details, \
             build_http_request. Output of build_http_request is ready-to-execute input \
             for the http_request node.",
        )
    }
}

impl ToolkitNode for ApiExplorerNode {
    fn sub_tool_catalog(&self, _config: &Value) -> Vec<SubToolDefinition> {
        vec![
            load_spec_sub_tool(),
            list_endpoints_sub_tool(),
            search_endpoint_sub_tool(),
            get_endpoint_details_sub_tool(),
            build_http_request_sub_tool(),
        ]
    }
}

fn load_spec_sub_tool() -> SubToolDefinition {
    let mut props = HashMap::new();
    props.insert(
        "url".into(),
        ParameterProperty {
            property_type: "string".into(),
            description: "Absolute URL of an OpenAPI 3.x or Swagger 2.0 JSON/YAML file. \
                Git-forge blob URLs (github.com/.../blob/..., gitlab.com/.../-/blob/..., \
                bitbucket.org/.../src/...) are accepted and auto-rewritten to raw."
                .into(),
            enum_values: None,
            pattern: None,
        },
    );
    props.insert(
        "force_reload".into(),
        ParameterProperty {
            property_type: "boolean".into(),
            description: "If true, bypass cache and re-download. Default false.".into(),
            enum_values: None,
            pattern: None,
        },
    );
    SubToolDefinition {
        name: "load_spec".into(),
        description: "Download and parse an OpenAPI 3.x or Swagger 2.0 specification from a URL. \
            Must be called before any other api_explorer tool. The parsed spec is cached for \
            the conversation so subsequent tools are fast. Returns a summary of what the spec \
            contains. You can paste Git-forge URLs — the node rewrites them to the raw-content \
            URL automatically; use `resolved_url` in the result to see what was actually fetched. \
            Swagger 2.0 documents are converted internally to OpenAPI 3.0 so all subsequent tools \
            behave identically. If the download returns HTML (usually because a Git-forge blob \
            URL could not be normalized), you get a clear error suggesting the raw URL format."
            .into(),
        properties: props,
        required: vec!["url".into()],
    }
}

fn list_endpoints_sub_tool() -> SubToolDefinition {
    let mut props = HashMap::new();
    props.insert(
        "spec_url".into(),
        ParameterProperty {
            property_type: "string".into(),
            description: "The URL of the previously-loaded spec (the `spec_url_input` you \
                passed to load_spec)."
                .into(),
            enum_values: None,
            pattern: None,
        },
    );
    props.insert(
        "tag".into(),
        ParameterProperty {
            property_type: "string".into(),
            description: "Filter by tag (e.g., \"Subscriptions\").".into(),
            enum_values: None,
            pattern: None,
        },
    );
    props.insert(
        "limit".into(),
        ParameterProperty {
            property_type: "integer".into(),
            description: "Page size. Default 50, max 200.".into(),
            enum_values: None,
            pattern: None,
        },
    );
    props.insert(
        "offset".into(),
        ParameterProperty {
            property_type: "integer".into(),
            description: "Pagination offset. Default 0.".into(),
            enum_values: None,
            pattern: None,
        },
    );
    SubToolDefinition {
        name: "list_endpoints".into(),
        description: "List all endpoints in a previously loaded spec. Prefer `search_endpoint` \
            unless you want to browse by category. Results are paginated. If you do not know \
            which tags exist, call `load_spec` first — its result lists them."
            .into(),
        properties: props,
        required: vec!["spec_url".into()],
    }
}

fn search_endpoint_sub_tool() -> SubToolDefinition {
    let mut props = HashMap::new();
    props.insert(
        "spec_url".into(),
        ParameterProperty {
            property_type: "string".into(),
            description: "The URL of the previously-loaded spec.".into(),
            enum_values: None,
            pattern: None,
        },
    );
    props.insert(
        "query".into(),
        ParameterProperty {
            property_type: "string".into(),
            description: "Free-text query, e.g. \"create subscription\", \"list customers\".".into(),
            enum_values: None,
            pattern: None,
        },
    );
    props.insert(
        "method".into(),
        ParameterProperty {
            property_type: "string".into(),
            description: "Filter by HTTP method.".into(),
            enum_values: Some(vec![
                "GET".into(),
                "POST".into(),
                "PUT".into(),
                "PATCH".into(),
                "DELETE".into(),
            ]),
            pattern: None,
        },
    );
    props.insert(
        "max_results".into(),
        ParameterProperty {
            property_type: "integer".into(),
            description: "Default 10, max 50.".into(),
            enum_values: None,
            pattern: None,
        },
    );
    SubToolDefinition {
        name: "search_endpoint".into(),
        description: "Find endpoints by keyword. Matches against path, summary, description, \
            operation_id, and tags. Uses fuzzy matching so typos and reordered words still work. \
            Returns the best ranked matches with relevance scores. Prefer this over \
            `list_endpoints` when you have any idea what you are looking for."
            .into(),
        properties: props,
        required: vec!["spec_url".into(), "query".into()],
    }
}

fn get_endpoint_details_sub_tool() -> SubToolDefinition {
    let mut props = HashMap::new();
    props.insert(
        "spec_url".into(),
        ParameterProperty {
            property_type: "string".into(),
            description: "The URL of the previously-loaded spec.".into(),
            enum_values: None,
            pattern: None,
        },
    );
    props.insert(
        "operation_id".into(),
        ParameterProperty {
            property_type: "string".into(),
            description: "The operation id from `search_endpoint` or `list_endpoints`.".into(),
            enum_values: None,
            pattern: None,
        },
    );
    SubToolDefinition {
        name: "get_endpoint_details".into(),
        description: "Retrieve the full specification of a single endpoint: parameters (path, \
            query, headers), request body schema, response schemas, and required auth. Call this \
            before `build_http_request` if you need to know what arguments are required. If the \
            operation_id is wrong, the result includes a `did_you_mean` list of the nearest \
            matches so you can retry."
            .into(),
        properties: props,
        required: vec!["spec_url".into(), "operation_id".into()],
    }
}

fn build_http_request_sub_tool() -> SubToolDefinition {
    let mut props = HashMap::new();
    props.insert(
        "spec_url".into(),
        ParameterProperty {
            property_type: "string".into(),
            description: "The URL of the previously-loaded spec.".into(),
            enum_values: None,
            pattern: None,
        },
    );
    props.insert(
        "operation_id".into(),
        ParameterProperty {
            property_type: "string".into(),
            description: "The operation id from `search_endpoint` or `list_endpoints`.".into(),
            enum_values: None,
            pattern: None,
        },
    );
    props.insert(
        "params".into(),
        ParameterProperty {
            property_type: "object".into(),
            description: "A flat map of parameter values. Path params, query params, header \
                params, and body fields are all resolved from the same map. The node routes each \
                to the right location based on the spec."
                .into(),
            enum_values: None,
            pattern: None,
        },
    );
    props.insert(
        "auth_secret_ref".into(),
        ParameterProperty {
            property_type: "string".into(),
            description: "Name of a Secure Value containing the token / API key. Required if \
                the endpoint declares auth. The name ends up as a `${SECURE:<name>}` placeholder \
                in the returned headers, which the http_request node resolves at execute time."
                .into(),
            enum_values: None,
            pattern: None,
        },
    );
    SubToolDefinition {
        name: "build_http_request".into(),
        description: "Build a validated HTTP-request configuration for a specific endpoint. \
            The output is a JSON object in the exact shape the `http_request` node accepts — \
            pass it as the input to an `http_request` call to execute. Missing required \
            parameters or wrong types return an error with hints; do not invent values to make \
            the error go away — the hint tells you exactly what to ask the user for."
            .into(),
        properties: props,
        required: vec!["spec_url".into(), "operation_id".into(), "params".into()],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_has_all_five_sub_tools() {
        let node = ApiExplorerNode::new();
        let cat = node.sub_tool_catalog(&json!({}));
        assert_eq!(cat.len(), 5);
        let names: Vec<&str> = cat.iter().map(|s| s.name.as_ref()).collect();
        for expected in [
            "load_spec",
            "list_endpoints",
            "search_endpoint",
            "get_endpoint_details",
            "build_http_request",
        ] {
            assert!(names.contains(&expected), "missing sub-tool {expected}");
        }
    }

    #[test]
    fn load_spec_requires_url() {
        let node = ApiExplorerNode::new();
        let cat = node.sub_tool_catalog(&json!({}));
        let s = cat.iter().find(|s| s.name == "load_spec").unwrap();
        assert!(s.required.contains(&"url".to_string()));
    }

    #[test]
    fn build_http_request_requires_three_fields() {
        let node = ApiExplorerNode::new();
        let cat = node.sub_tool_catalog(&json!({}));
        let s = cat.iter().find(|s| s.name == "build_http_request").unwrap();
        for k in ["spec_url", "operation_id", "params"] {
            assert!(s.required.contains(&k.to_string()), "missing required {k}");
        }
    }

    #[test]
    fn search_endpoint_exposes_method_enum() {
        let node = ApiExplorerNode::new();
        let cat = node.sub_tool_catalog(&json!({}));
        let s = cat.iter().find(|s| s.name == "search_endpoint").unwrap();
        let method = s.properties.get("method").unwrap();
        let evs = method.enum_values.as_ref().unwrap();
        for m in ["GET", "POST", "PUT", "PATCH", "DELETE"] {
            assert!(evs.iter().any(|e| e == m));
        }
    }

    #[test]
    fn extract_conversation_id_falls_back_to_default() {
        let inputs: NodeInputs = HashMap::new();
        assert_eq!(ApiExplorerNode::extract_conversation_id(&inputs), "default");
        let mut inputs2: NodeInputs = HashMap::new();
        inputs2.insert("conversation_id".into(), json!("c-42"));
        assert_eq!(ApiExplorerNode::extract_conversation_id(&inputs2), "c-42");
    }

    #[tokio::test]
    async fn dispatch_unknown_sub_tool_returns_structured_error() {
        let node = ApiExplorerNode::new();
        let mut inputs: NodeInputs = HashMap::new();
        inputs.insert(SUB_TOOL_INPUT_KEY.into(), json!("does_not_exist"));
        let mut state = json!({});
        let out = node
            .execute(&inputs, &json!({}), &mut state, None)
            .await
            .unwrap();
        assert_eq!(
            out.get("error").and_then(|v| v.as_str()),
            Some("unknown_sub_tool")
        );
        assert_eq!(
            out.get("sub_tool").and_then(|v| v.as_str()),
            Some("does_not_exist")
        );
    }

    use crate::web::domain::{
        ApiKeyLocation, ApiSpecPort, Endpoint, HttpMethod, ParsedSpec, SecurityRequirement,
        SecurityScheme, SpecFetchResult, SpecFormat, WebDomainError,
    };
    use std::sync::Mutex as StdMutex;

    /// Minimal stub port returning a hand-built `ParsedSpec`. The
    /// `resolved_url` echoes the input URL so cache lookups by URL work.
    struct FakePort {
        calls: StdMutex<u32>,
        spec: ParsedSpec,
    }

    #[async_trait]
    impl ApiSpecPort for FakePort {
        async fn fetch_and_parse(
            &self,
            url: &str,
            _etag: Option<&str>,
            _last_modified: Option<&str>,
        ) -> Result<SpecFetchResult, WebDomainError> {
            *self.calls.lock().unwrap() += 1;
            let mut s = self.spec.clone();
            s.input_url = url.to_string();
            s.resolved_url = url.to_string();
            Ok(SpecFetchResult::Fresh {
                spec: s,
                etag: Some("W/\"v1\"".into()),
                last_modified: None,
            })
        }
    }

    fn fake_parsed_spec() -> ParsedSpec {
        let mut security_schemes = HashMap::new();
        security_schemes.insert(
            "ApiKeyAuth".into(),
            SecurityScheme::ApiKey {
                name: "X-API-Key".into(),
                location: ApiKeyLocation::Header,
            },
        );
        ParsedSpec {
            resolved_url: "".into(),
            input_url: "".into(),
            original_format: SpecFormat::OpenApi3x,
            internal_format: "openapi-3.0.3".into(),
            title: "Petstore".into(),
            version: "1.0.0".into(),
            description: Some("Sample API.".into()),
            servers: vec!["https://petstore.example.com".into()],
            endpoints: vec![Endpoint {
                operation_id: "listPets".into(),
                method: HttpMethod::Get,
                path: "/pets".into(),
                summary: Some("List pets".into()),
                description: None,
                tags: vec!["pets".into()],
                path_params: Vec::new(),
                query_params: Vec::new(),
                header_params: Vec::new(),
                request_body: None,
                responses: HashMap::new(),
                security: vec![SecurityRequirement {
                    scheme: "ApiKeyAuth".into(),
                    scopes: Vec::new(),
                }],
            }],
            security_schemes,
            tags: vec!["pets".into()],
        }
    }

    fn node_with_fake_port() -> (Arc<FakePort>, ApiExplorerNode) {
        let port = Arc::new(FakePort {
            calls: StdMutex::new(0),
            spec: fake_parsed_spec(),
        });
        let node = ApiExplorerNode::new_with_port(port.clone() as Arc<dyn ApiSpecPort>);
        (port, node)
    }

    /// Same as [`node_with_fake_port`] but with a very permissive
    /// fuzzy-match threshold so the tiny single-endpoint fake spec
    /// produces hits.
    fn node_with_fake_port_loose() -> (Arc<FakePort>, ApiExplorerNode) {
        let port = Arc::new(FakePort {
            calls: StdMutex::new(0),
            spec: fake_parsed_spec(),
        });
        let cfg = ApiSpecUseCaseConfig {
            fuzzy_match_threshold: 0.05,
            ..ApiSpecUseCaseConfig::default()
        };
        let node = ApiExplorerNode::new_with_port_and_config(
            port.clone() as Arc<dyn ApiSpecPort>,
            cfg,
        );
        (port, node)
    }

    #[tokio::test]
    async fn load_spec_returns_summary_with_resolved_url() {
        let (port, node) = node_with_fake_port();
        let mut inputs: NodeInputs = HashMap::new();
        inputs.insert(SUB_TOOL_INPUT_KEY.into(), json!("load_spec"));
        inputs.insert("url".into(), json!("https://example.com/petstore.yaml"));
        inputs.insert("conversation_id".into(), json!("c-1"));
        let mut state = json!({});
        let out = node
            .execute(&inputs, &json!({}), &mut state, None)
            .await
            .unwrap();

        assert_eq!(
            out.get("spec_url_input").and_then(|v| v.as_str()),
            Some("https://example.com/petstore.yaml")
        );
        assert_eq!(
            out.get("resolved_url").and_then(|v| v.as_str()),
            Some("https://example.com/petstore.yaml")
        );
        assert_eq!(out.get("title").and_then(|v| v.as_str()), Some("Petstore"));
        assert_eq!(
            out.get("original_format").and_then(|v| v.as_str()),
            Some("openapi-3.x")
        );
        assert_eq!(
            out.get("internal_format").and_then(|v| v.as_str()),
            Some("openapi-3.0.3")
        );
        assert_eq!(out.get("endpoints_count").and_then(|v| v.as_u64()), Some(1));
        assert_eq!(out.get("cached").and_then(|v| v.as_bool()), Some(false));
        let schemes = out
            .get("security_schemes")
            .and_then(|v| v.as_array())
            .unwrap();
        assert_eq!(schemes[0].as_str(), Some("ApiKeyAuth"));
        assert_eq!(*port.calls.lock().unwrap(), 1);
    }

    #[tokio::test]
    async fn load_spec_caches_within_conversation() {
        let (port, node) = node_with_fake_port();
        let mut inputs: NodeInputs = HashMap::new();
        inputs.insert(SUB_TOOL_INPUT_KEY.into(), json!("load_spec"));
        inputs.insert("url".into(), json!("https://example.com/petstore.yaml"));
        inputs.insert("conversation_id".into(), json!("c-cache"));
        let mut state = json!({});

        let first = node
            .execute(&inputs, &json!({}), &mut state, None)
            .await
            .unwrap();
        assert_eq!(first.get("cached").and_then(|v| v.as_bool()), Some(false));

        let second = node
            .execute(&inputs, &json!({}), &mut state, None)
            .await
            .unwrap();
        assert_eq!(second.get("cached").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(
            *port.calls.lock().unwrap(),
            1,
            "second call must hit cache, not the port"
        );
    }

    #[tokio::test]
    async fn load_spec_force_reload_bypasses_cache() {
        let (port, node) = node_with_fake_port();
        let mut inputs: NodeInputs = HashMap::new();
        inputs.insert(SUB_TOOL_INPUT_KEY.into(), json!("load_spec"));
        inputs.insert("url".into(), json!("https://example.com/petstore.yaml"));
        inputs.insert("conversation_id".into(), json!("c-force"));
        let mut state = json!({});

        node.execute(&inputs, &json!({}), &mut state, None)
            .await
            .unwrap();
        inputs.insert("force_reload".into(), json!(true));
        let out = node
            .execute(&inputs, &json!({}), &mut state, None)
            .await
            .unwrap();
        assert_eq!(*port.calls.lock().unwrap(), 2);
        assert_eq!(out.get("cached").and_then(|v| v.as_bool()), Some(false));
    }

    #[tokio::test]
    async fn load_spec_missing_url_returns_invalid_input() {
        let (_port, node) = node_with_fake_port();
        let mut inputs: NodeInputs = HashMap::new();
        inputs.insert(SUB_TOOL_INPUT_KEY.into(), json!("load_spec"));
        inputs.insert("conversation_id".into(), json!("c-x"));
        let mut state = json!({});
        let out = node
            .execute(&inputs, &json!({}), &mut state, None)
            .await
            .unwrap();
        assert_eq!(
            out.get("error").and_then(|v| v.as_str()),
            Some("invalid_input")
        );
        assert_eq!(out.get("missing").and_then(|v| v.as_str()), Some("url"));
    }

    #[tokio::test]
    async fn list_endpoints_returns_paginated_summary() {
        let (_port, node) = node_with_fake_port();
        let mut load: NodeInputs = HashMap::new();
        load.insert(SUB_TOOL_INPUT_KEY.into(), json!("load_spec"));
        load.insert("url".into(), json!("https://x/spec.yaml"));
        load.insert("conversation_id".into(), json!("c-list"));
        let mut state = json!({});
        node.execute(&load, &json!({}), &mut state, None).await.unwrap();

        let mut list: NodeInputs = HashMap::new();
        list.insert(SUB_TOOL_INPUT_KEY.into(), json!("list_endpoints"));
        list.insert("spec_url".into(), json!("https://x/spec.yaml"));
        list.insert("conversation_id".into(), json!("c-list"));
        let out = node
            .execute(&list, &json!({}), &mut state, None)
            .await
            .unwrap();
        assert_eq!(out.get("total").and_then(|v| v.as_u64()), Some(1));
        assert_eq!(out.get("returned").and_then(|v| v.as_u64()), Some(1));
        let eps = out.get("endpoints").and_then(|v| v.as_array()).unwrap();
        assert_eq!(
            eps[0].get("operation_id").and_then(|v| v.as_str()),
            Some("listPets")
        );
        assert_eq!(
            eps[0].get("method").and_then(|v| v.as_str()),
            Some("GET")
        );
    }

    #[tokio::test]
    async fn list_endpoints_on_unloaded_spec_returns_spec_not_loaded() {
        let (_port, node) = node_with_fake_port();
        let mut list: NodeInputs = HashMap::new();
        list.insert(SUB_TOOL_INPUT_KEY.into(), json!("list_endpoints"));
        list.insert("spec_url".into(), json!("https://never/loaded.yaml"));
        list.insert("conversation_id".into(), json!("c-unloaded"));
        let mut state = json!({});
        let out = node
            .execute(&list, &json!({}), &mut state, None)
            .await
            .unwrap();
        assert_eq!(
            out.get("error").and_then(|v| v.as_str()),
            Some("spec_not_loaded")
        );
    }

    #[tokio::test]
    async fn search_endpoint_ranks_by_fuzzy_score() {
        let (_port, node) = node_with_fake_port_loose();
        let mut load: NodeInputs = HashMap::new();
        load.insert(SUB_TOOL_INPUT_KEY.into(), json!("load_spec"));
        load.insert("url".into(), json!("https://x/spec.yaml"));
        load.insert("conversation_id".into(), json!("c-search"));
        let mut state = json!({});
        node.execute(&load, &json!({}), &mut state, None).await.unwrap();

        let mut search: NodeInputs = HashMap::new();
        search.insert(SUB_TOOL_INPUT_KEY.into(), json!("search_endpoint"));
        search.insert("spec_url".into(), json!("https://x/spec.yaml"));
        search.insert("conversation_id".into(), json!("c-search"));
        search.insert("query".into(), json!("listPets"));
        let out = node
            .execute(&search, &json!({}), &mut state, None)
            .await
            .unwrap();
        let results = out.get("results").and_then(|v| v.as_array()).unwrap();
        assert!(!results.is_empty(), "expected at least one fuzzy hit");
        assert_eq!(
            results[0].get("operation_id").and_then(|v| v.as_str()),
            Some("listPets")
        );
        assert!(results[0].get("score").and_then(|v| v.as_f64()).is_some());
    }

    #[tokio::test]
    async fn get_endpoint_details_returns_structured_json() {
        let (_port, node) = node_with_fake_port();
        let mut load: NodeInputs = HashMap::new();
        load.insert(SUB_TOOL_INPUT_KEY.into(), json!("load_spec"));
        load.insert("url".into(), json!("https://x/spec.yaml"));
        load.insert("conversation_id".into(), json!("c-det"));
        let mut state = json!({});
        node.execute(&load, &json!({}), &mut state, None).await.unwrap();

        let mut det: NodeInputs = HashMap::new();
        det.insert(SUB_TOOL_INPUT_KEY.into(), json!("get_endpoint_details"));
        det.insert("spec_url".into(), json!("https://x/spec.yaml"));
        det.insert("conversation_id".into(), json!("c-det"));
        det.insert("operation_id".into(), json!("listPets"));
        let out = node
            .execute(&det, &json!({}), &mut state, None)
            .await
            .unwrap();
        assert_eq!(
            out.get("operation_id").and_then(|v| v.as_str()),
            Some("listPets")
        );
        assert_eq!(out.get("method").and_then(|v| v.as_str()), Some("GET"));
        assert_eq!(out.get("path").and_then(|v| v.as_str()), Some("/pets"));
    }

    #[tokio::test]
    async fn get_endpoint_details_miss_returns_did_you_mean() {
        let (_port, node) = node_with_fake_port();
        let mut load: NodeInputs = HashMap::new();
        load.insert(SUB_TOOL_INPUT_KEY.into(), json!("load_spec"));
        load.insert("url".into(), json!("https://x/spec.yaml"));
        load.insert("conversation_id".into(), json!("c-miss"));
        let mut state = json!({});
        node.execute(&load, &json!({}), &mut state, None).await.unwrap();

        let mut det: NodeInputs = HashMap::new();
        det.insert(SUB_TOOL_INPUT_KEY.into(), json!("get_endpoint_details"));
        det.insert("spec_url".into(), json!("https://x/spec.yaml"));
        det.insert("conversation_id".into(), json!("c-miss"));
        det.insert("operation_id".into(), json!("listPet"));
        let out = node
            .execute(&det, &json!({}), &mut state, None)
            .await
            .unwrap();
        assert_eq!(
            out.get("error").and_then(|v| v.as_str()),
            Some("endpoint_not_found")
        );
        let dym = out.get("did_you_mean").and_then(|v| v.as_array()).unwrap();
        assert!(dym.iter().any(|v| v.as_str() == Some("listPets")));
    }

    #[tokio::test]
    async fn build_http_request_emits_ready_to_execute_config() {
        let (_port, node) = node_with_fake_port();
        let mut load: NodeInputs = HashMap::new();
        load.insert(SUB_TOOL_INPUT_KEY.into(), json!("load_spec"));
        load.insert("url".into(), json!("https://x/spec.yaml"));
        load.insert("conversation_id".into(), json!("c-build"));
        let mut state = json!({});
        node.execute(&load, &json!({}), &mut state, None).await.unwrap();

        let mut build: NodeInputs = HashMap::new();
        build.insert(SUB_TOOL_INPUT_KEY.into(), json!("build_http_request"));
        build.insert("spec_url".into(), json!("https://x/spec.yaml"));
        build.insert("conversation_id".into(), json!("c-build"));
        build.insert("operation_id".into(), json!("listPets"));
        build.insert("params".into(), json!({}));
        build.insert("auth_secret_ref".into(), json!("my_key"));
        let out = node
            .execute(&build, &json!({}), &mut state, None)
            .await
            .unwrap();
        assert_eq!(out.get("method").and_then(|v| v.as_str()), Some("GET"));
        assert_eq!(
            out.get("url").and_then(|v| v.as_str()),
            Some("https://petstore.example.com/pets")
        );
        let headers = out.get("headers").and_then(|v| v.as_object()).unwrap();
        assert_eq!(
            headers.get("X-API-Key").and_then(|v| v.as_str()),
            Some("${SECURE:my_key}")
        );
    }

    #[tokio::test]
    async fn build_http_request_missing_auth_returns_structured_error() {
        let (_port, node) = node_with_fake_port();
        let mut load: NodeInputs = HashMap::new();
        load.insert(SUB_TOOL_INPUT_KEY.into(), json!("load_spec"));
        load.insert("url".into(), json!("https://x/spec.yaml"));
        load.insert("conversation_id".into(), json!("c-auth"));
        let mut state = json!({});
        node.execute(&load, &json!({}), &mut state, None).await.unwrap();

        let mut build: NodeInputs = HashMap::new();
        build.insert(SUB_TOOL_INPUT_KEY.into(), json!("build_http_request"));
        build.insert("spec_url".into(), json!("https://x/spec.yaml"));
        build.insert("conversation_id".into(), json!("c-auth"));
        build.insert("operation_id".into(), json!("listPets"));
        build.insert("params".into(), json!({}));
        let out = node
            .execute(&build, &json!({}), &mut state, None)
            .await
            .unwrap();
        assert_eq!(
            out.get("error").and_then(|v| v.as_str()),
            Some("missing_auth")
        );
    }

    #[tokio::test]
    async fn build_http_request_params_not_object_returns_invalid_input() {
        let (_port, node) = node_with_fake_port();
        let mut load: NodeInputs = HashMap::new();
        load.insert(SUB_TOOL_INPUT_KEY.into(), json!("load_spec"));
        load.insert("url".into(), json!("https://x/spec.yaml"));
        load.insert("conversation_id".into(), json!("c-bad-params"));
        let mut state = json!({});
        node.execute(&load, &json!({}), &mut state, None).await.unwrap();

        let mut build: NodeInputs = HashMap::new();
        build.insert(SUB_TOOL_INPUT_KEY.into(), json!("build_http_request"));
        build.insert("spec_url".into(), json!("https://x/spec.yaml"));
        build.insert("conversation_id".into(), json!("c-bad-params"));
        build.insert("operation_id".into(), json!("listPets"));
        build.insert("params".into(), json!("not-an-object"));
        let out = node
            .execute(&build, &json!({}), &mut state, None)
            .await
            .unwrap();
        assert_eq!(
            out.get("error").and_then(|v| v.as_str()),
            Some("invalid_input")
        );
    }

    #[tokio::test]
    async fn search_endpoint_filters_by_method() {
        let (_port, node) = node_with_fake_port_loose();
        let mut load: NodeInputs = HashMap::new();
        load.insert(SUB_TOOL_INPUT_KEY.into(), json!("load_spec"));
        load.insert("url".into(), json!("https://x/spec.yaml"));
        load.insert("conversation_id".into(), json!("c-method"));
        let mut state = json!({});
        node.execute(&load, &json!({}), &mut state, None).await.unwrap();

        let mut search: NodeInputs = HashMap::new();
        search.insert(SUB_TOOL_INPUT_KEY.into(), json!("search_endpoint"));
        search.insert("spec_url".into(), json!("https://x/spec.yaml"));
        search.insert("conversation_id".into(), json!("c-method"));
        search.insert("query".into(), json!("pets"));
        search.insert("method".into(), json!("POST"));
        let out = node
            .execute(&search, &json!({}), &mut state, None)
            .await
            .unwrap();
        let results = out.get("results").and_then(|v| v.as_array()).unwrap();
        assert!(results.is_empty(), "no POST /pets in fake spec");
    }
}
