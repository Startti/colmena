//! HTTP request node — makes outbound HTTP calls from a DAG.
//!
//! ## Standalone use
//! Configure via `config`: `base_url`, `endpoint`, `method`, `headers`, `query_params`,
//! `body`, `bearer_token`, `authorization`. All string values support `${ENV_VAR}` resolution.
//! Input edges override config values (inputs take priority over config).
//!
//! ## As an LLM tool (via `tool_configurations`)
//! When invoked by `DagToolExecutor`, extra non-reserved input keys with primitive values
//! (string, number, boolean) are automatically appended as URL query parameters.
//! This is the mechanism that allows `node_schema` container children and `$DYNAMIC`
//! top-level fields to reach the node as flat inputs.
//!
//! ## Outputs
//! Always returns `{ "status": u16, "body": Value }`.
//! `body` is parsed as JSON; if the response is not valid JSON, `body` is `null`.
//! The default output port is `body`.

use crate::dag_engine::domain::node::{ExecutableNode, NodeInputs};
use reqwest::{Client, Method, Url};
use serde_json::{json, Value};
use std::error::Error as StdError;
use std::str::FromStr;
use std::sync::Arc;

/// Executes HTTP requests. Implements [`ExecutableNode`]. Stateless — all configuration
/// comes from `inputs` (highest priority) and `config`.
pub struct HttpNode;

impl HttpNode {
    fn resolve_env_vars(input: &str) -> Result<String, String> {
        let mut result = String::new();
        let mut last_end = 0;

        while let Some(start) = input[last_end..].find("${") {
            let absolute_start = last_end + start;
            result.push_str(&input[last_end..absolute_start]);

            if let Some(end) = input[absolute_start..].find('}') {
                let absolute_end = absolute_start + end;
                let var_name = &input[absolute_start + 2..absolute_end];
                let val = std::env::var(var_name)
                    .map_err(|_| format!("Env var {} not found", var_name))?;
                result.push_str(&val);
                last_end = absolute_end + 1;
            } else {
                result.push_str(&input[absolute_start..]);
                last_end = input.len();
                break;
            }
        }
        result.push_str(&input[last_end..]);
        Ok(result)
    }

    /// Resolve `${ENV_VAR}` in all string values within a JSON Value (recursive).
    fn resolve_env_vars_in_value(val: &Value) -> Value {
        match val {
            Value::String(s) => {
                Value::String(Self::resolve_env_vars(s).unwrap_or_else(|_| s.clone()))
            }
            Value::Object(map) => {
                let mut out = serde_json::Map::new();
                for (k, v) in map {
                    out.insert(k.clone(), Self::resolve_env_vars_in_value(v));
                }
                Value::Object(out)
            }
            Value::Array(arr) => {
                Value::Array(arr.iter().map(Self::resolve_env_vars_in_value).collect())
            }
            other => other.clone(),
        }
    }
}

#[async_trait::async_trait]
impl ExecutableNode for HttpNode {
    /// Execute an HTTP request.
    ///
    /// # Priority
    /// For every field (`base_url`, `endpoint`, `method`, `headers`, `query_params`, `body`,
    /// `bearer_token`, `authorization`), the value from `inputs` takes priority over `config`.
    ///
    /// # Env var resolution
    /// All string values in `config` (and input headers) support `${VAR_NAME}` syntax, resolved
    /// via `std::env::var` at call time. This is the primary mechanism for injecting API keys.
    ///
    /// # Extra query params
    /// Any input key not in `reserved_keys` that holds a primitive value (string, number, bool)
    /// is automatically appended as a URL query parameter. When called as an LLM tool, the
    /// executor passes `node_schema` child fields and `$DYNAMIC` replacements as flat inputs,
    /// which this mechanism then routes to query params or body as appropriate.
    ///
    /// # Outputs
    /// Returns `{"status": <u16>, "body": <json_value_or_null>}`. The `body` is the default
    /// output port — downstream nodes without a field selector receive it directly.
    async fn execute(
        &self,
        inputs: &NodeInputs,
        config: &Value,
        _state: &mut Value,
        _observer: Option<Arc<dyn crate::dag_engine::domain::observer::ExecutionObserver>>,
    ) -> Result<Value, Box<dyn StdError + Send + Sync>> {
        // 1. Parse Configuration (Inputs > Config)
        let base_url_raw = inputs
            .get("base_url")
            .and_then(|v| v.as_str())
            .or_else(|| config.get("base_url").and_then(|v| v.as_str()))
            .unwrap_or("");
        let base_url = Self::resolve_env_vars(base_url_raw).map_err(|e| {
            Box::new(std::io::Error::new(std::io::ErrorKind::InvalidInput, e))
                as Box<dyn StdError + Send + Sync>
        })?;

        let endpoint_raw = inputs
            .get("endpoint")
            .and_then(|v| v.as_str())
            .or_else(|| config.get("endpoint").and_then(|v| v.as_str()))
            .unwrap_or("");
        let endpoint = Self::resolve_env_vars(endpoint_raw).map_err(|e| {
            Box::new(std::io::Error::new(std::io::ErrorKind::InvalidInput, e))
                as Box<dyn StdError + Send + Sync>
        })?;

        let method_str = inputs
            .get("method")
            .and_then(|v| v.as_str())
            .or_else(|| config.get("method").and_then(|v| v.as_str()))
            .unwrap_or("GET");

        // 2. Construct URL
        // Handle trailing/leading slashes to avoid double slashes or missing slashes
        let base = base_url.trim_end_matches('/');
        let path = endpoint.trim_start_matches('/');
        let full_url_str = if path.is_empty() {
            base.to_string()
        } else {
            format!("{}/{}", base, path)
        };

        let url = Url::parse(&full_url_str)
            .map_err(|e| format!("Invalid URL '{}': {}", full_url_str, e))?;
        let method = Method::from_str(method_str)
            .map_err(|e| format!("Invalid HTTP method '{}': {}", method_str, e))?;

        // 3. Prepare Client and Request
        // Build client forcing HTTP/1.1 to avoid HTTP/2 issues with some APIs
        let client = Client::builder().http1_only().build()?;

        println!("[HttpNode] → {} {}", method, url);

        let mut request_builder = client.request(method, url);

        // Add a default User-Agent to improve compatibility with some APIs
        request_builder = request_builder.header("User-Agent", "colmena-http-node/0.1");

        // 4. Headers (Config + Inputs)
        // Config headers
        if let Some(headers) = config.get("headers").and_then(|v| v.as_object()) {
            for (k, v) in headers {
                if let Some(v_str) = v.as_str() {
                    let v_resolved = Self::resolve_env_vars(v_str).map_err(|e| {
                        Box::new(std::io::Error::new(std::io::ErrorKind::InvalidInput, e))
                            as Box<dyn StdError + Send + Sync>
                    })?;
                    request_builder = request_builder.header(k, v_resolved);
                }
            }
        }
        // Input headers (override config)
        if let Some(headers) = inputs.get("headers").and_then(|v| v.as_object()) {
            for (k, v) in headers {
                if let Some(v_str) = v.as_str() {
                    let v_resolved = Self::resolve_env_vars(v_str).map_err(|e| {
                        Box::new(std::io::Error::new(std::io::ErrorKind::InvalidInput, e))
                            as Box<dyn StdError + Send + Sync>
                    })?;
                    request_builder = request_builder.header(k, v_resolved);
                }
            }
        }

        // Handle specific auth inputs
        if let Some(token) = inputs.get("bearer_token").and_then(|v| v.as_str()) {
            request_builder = request_builder.header("Authorization", format!("Bearer {}", token));
        }
        if let Some(auth) = inputs.get("authorization").and_then(|v| v.as_str()) {
            request_builder = request_builder.header("Authorization", auth);
        }

        // 5. Query Params (Config + Inputs) — resolve ${ENV_VAR} in values
        if let Some(params) = config.get("query_params").and_then(|v| v.as_object()) {
            let mut resolved = serde_json::Map::new();
            for (k, v) in params {
                if let Some(s) = v.as_str() {
                    let s_resolved = Self::resolve_env_vars(s).map_err(|e| {
                        Box::new(std::io::Error::new(std::io::ErrorKind::InvalidInput, e))
                            as Box<dyn StdError + Send + Sync>
                    })?;
                    resolved.insert(k.clone(), Value::String(s_resolved));
                } else {
                    resolved.insert(k.clone(), v.clone());
                }
            }
            request_builder = request_builder.query(&resolved);
        } else if let Some(params) = config.get("query_params") {
            request_builder = request_builder.query(params);
        }
        if let Some(params) = inputs.get("query_params").and_then(|v| v.as_object()) {
            let mut resolved = serde_json::Map::new();
            for (k, v) in params {
                if let Some(s) = v.as_str() {
                    let s_resolved = Self::resolve_env_vars(s).map_err(|e| {
                        Box::new(std::io::Error::new(std::io::ErrorKind::InvalidInput, e))
                            as Box<dyn StdError + Send + Sync>
                    })?;
                    resolved.insert(k.clone(), Value::String(s_resolved));
                } else {
                    resolved.insert(k.clone(), v.clone());
                }
            }
            request_builder = request_builder.query(&resolved);
        } else if let Some(params) = inputs.get("query_params") {
            request_builder = request_builder.query(params);
        }

        // Collect extra inputs as query params (for tools that flatten params)
        let reserved_keys = [
            "base_url",
            "endpoint",
            "method",
            "headers",
            "body",
            "query_params",     // correct key used throughout the codebase
            "query_parameters", // kept for backward compat
            "bearer_token",
            "authorization",
            "secure", // internal Colmena flag — NEVER send to external APIs
            "__colmena_session_id",
            "__node_id",
            "__colmena_resume_answer",
        ];
        let mut extra_params = std::collections::HashMap::new();
        for (k, v) in inputs {
            if !reserved_keys.contains(&k.as_str()) {
                // Only include primitives (String, Number, Boolean)
                match v {
                    serde_json::Value::String(s) => {
                        let s_resolved = Self::resolve_env_vars(s).unwrap_or(s.to_string());
                        extra_params.insert(k, serde_json::Value::String(s_resolved));
                    }
                    serde_json::Value::Number(_) | serde_json::Value::Bool(_) => {
                        extra_params.insert(k, v.clone());
                    }
                    _ => {
                        // Ignore Objects, Arrays, Nulls
                    }
                }
            }
        }
        if !extra_params.is_empty() {
            request_builder = request_builder.query(&extra_params);
        }

        // 6. Body (Inputs or Config)
        let body_val = inputs.get("body").or_else(|| config.get("body"));

        if let Some(body) = body_val {
            if let Some(s) = body.as_str() {
                let s_resolved = Self::resolve_env_vars(s).map_err(|e| {
                    Box::new(std::io::Error::new(std::io::ErrorKind::InvalidInput, e))
                        as Box<dyn StdError + Send + Sync>
                })?;
                // Never log body contents — may contain credentials or PII
                request_builder = request_builder.body(s_resolved);
            } else {
                // Resolve ${ENV_VAR} in body object string values before sending
                let resolved_body = Self::resolve_env_vars_in_value(body);
                // Never log body contents — may contain credentials or PII
                request_builder = request_builder.json(&resolved_body);
            }
        }

        // 7. Execute Request
        // Note: Headers are not easily printable from request_builder, but we can print what we added
        // println!("DEBUG: Headers: {:?}", request_builder); // RequestBuilder doesn't implement Debug nicely for headers

        let response = request_builder.send().await?;
        let status = response.status().as_u16();
        println!("[HttpNode] ← {} ({})", status, full_url_str);

        // Try to parse response as JSON, fallback to text/string
        let response_body: Value = match response.json::<Value>().await {
            Ok(json) => {
                // Never log response body — it may contain tokens, keys, or PII
                json
            }
            Err(_) => {
                println!("[HttpNode] Response body is not JSON or is empty");
                Value::Null
            }
        };

        // 8. Return Output
        Ok(json!({
            "status": status,
            "body": response_body
        }))
    }

    /// Human-readable description of this node type, used in LLM tool definitions.
    fn description(&self) -> Option<&str> {
        Some("Make HTTP requests to external APIs. Supports GET, POST, PUT, DELETE methods with custom headers and query parameters.")
    }

    /// The default output port is `body` — the parsed JSON response body.
    fn default_output(&self) -> Option<&str> {
        Some("body")
    }

    /// JSON schema describing the node's config and input/output ports.
    fn schema(&self) -> Value {
        json!({
            "type": "http_request",
            "config": {
                "base_url": "string",
                "endpoint": "string",
                "method": "string (GET, POST, PUT, DELETE, etc.)",
                "headers": "map<string, string> (optional)",
                "query_params": "any (optional)"
            },
            "inputs": {
                "base_url": "string (optional)",
                "endpoint": "string (optional)",
                "method": "string (optional)",
                "body": "any (optional)",
                "headers": "map<string, string> (optional)",
                "query_params": "any (optional)"
            },
            "outputs": {
                "status": "integer",
                "body": "any"
            }
        })
    }
}
