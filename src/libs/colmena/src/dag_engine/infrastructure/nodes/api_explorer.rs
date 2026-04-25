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
    #[allow(dead_code)] // used by handlers in Tasks 13-15
    use_case: Arc<ApiSpecUseCase>,
    registry: Arc<SessionRegistry<Arc<SpecCache>>>,
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
    #[allow(dead_code)] // used by port-injection tests in Tasks 13-15
    pub(crate) fn new_with_port(port: Arc<dyn ApiSpecPort>) -> Self {
        let registry = SessionRegistry::<Arc<SpecCache>>::new(TtlConfig::default());
        let cfg = ApiSpecUseCaseConfig::default();
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
    #[allow(dead_code)] // used by handlers in Tasks 13-15
    fn extract_conversation_id(inputs: &NodeInputs) -> String {
        inputs
            .get("conversation_id")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| "default".into())
    }

    /// Helper — read a required string field from the LLM's argument map.
    /// Returns a structured LLM-recoverable error JSON on miss.
    #[allow(dead_code)] // used by handlers in Tasks 13-15
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
        _config: &Value,
        _state: &mut Value,
        _observer: Option<Arc<dyn ExecutionObserver>>,
    ) -> Result<Value, Box<dyn StdError + Send + Sync>> {
        let sub = inputs
            .get(SUB_TOOL_INPUT_KEY)
            .and_then(|v| v.as_str())
            .ok_or("api_explorer: missing __sub_tool")?;
        Err(format!("api_explorer: sub_tool '{sub}' not implemented yet").into())
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
    async fn dispatch_stub_errors_until_handlers_land() {
        let node = ApiExplorerNode::new();
        let mut inputs: NodeInputs = HashMap::new();
        inputs.insert(SUB_TOOL_INPUT_KEY.into(), json!("load_spec"));
        let mut state = json!({});
        let err = node
            .execute(&inputs, &json!({}), &mut state, None)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("not implemented"));
    }
}
