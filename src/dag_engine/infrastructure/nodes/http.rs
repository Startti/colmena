use crate::dag_engine::domain::node::{ExecutableNode, NodeInputs};
use reqwest::{Client, Method, Url};
use serde_json::{json, Value};
use std::error::Error as StdError;
use std::str::FromStr;
use std::sync::Arc;

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
}

#[async_trait::async_trait]
impl ExecutableNode for HttpNode {
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

        println!("DEBUG: Sending HTTP Request");
        println!("DEBUG: URL: {}", url);
        println!("DEBUG: Method: {}", method);

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

        // 5. Query Params (Config + Inputs)
        if let Some(params) = config.get("query_params") {
            request_builder = request_builder.query(params);
        }
        if let Some(params) = inputs.get("query_params") {
            request_builder = request_builder.query(params);
        }

        // Collect extra inputs as query params (for tools that flatten params)
        let reserved_keys = [
            "base_url",
            "endpoint",
            "method",
            "headers",
            "body",
            "query_parameters",
            "bearer_token",
            "authorization",
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
                println!("DEBUG: Request Body: {}", s_resolved);
                request_builder = request_builder.body(s_resolved);
            } else {
                println!("DEBUG: Request Body: {}", body);
                request_builder = request_builder.json(body);
            }
        }

        // 7. Execute Request
        // Note: Headers are not easily printable from request_builder, but we can print what we added
        // println!("DEBUG: Headers: {:?}", request_builder); // RequestBuilder doesn't implement Debug nicely for headers

        let response = request_builder.send().await?;
        let status = response.status().as_u16();
        println!("DEBUG: Response Status: {}", status);

        // Try to parse response as JSON, fallback to text/string
        let response_body: Value = match response.json::<Value>().await {
            Ok(json) => {
                println!("DEBUG: Response Body: {}", json);
                json
            }
            Err(_) => {
                println!("DEBUG: Response Body is not JSON or empty");
                Value::Null
            } // Or handle text content if needed
        };

        // 8. Return Output
        Ok(json!({
            "output": {
                "status": status,
                "body": response_body
            }
        }))
    }

    fn description(&self) -> Option<&str> {
        Some("Make HTTP requests to external APIs. Supports GET, POST, PUT, DELETE methods with custom headers and query parameters.")
    }

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
