use crate::domain::node::{ExecutableNode, NodeInputs};
use serde_json::{json, Value};
use std::error::Error as StdError;
use std::str::FromStr;
use reqwest::{Client, Method, Url};

pub struct HttpNode;

#[async_trait::async_trait]
impl ExecutableNode for HttpNode {
    async fn execute(
        &self,
        inputs: &NodeInputs,
        config: &Value,
        _state: &mut Value,
    ) -> Result<Value, Box<dyn StdError>> {
        // 1. Parse Configuration
        let base_url = config.get("base_url").and_then(|v| v.as_str()).unwrap_or("");
        let endpoint = config.get("endpoint").and_then(|v| v.as_str()).unwrap_or("");
        let method_str = config.get("method").and_then(|v| v.as_str()).unwrap_or("GET");
        
        // 2. Construct URL
        // Handle trailing/leading slashes to avoid double slashes or missing slashes
        let base = base_url.trim_end_matches('/');
        let path = endpoint.trim_start_matches('/');
        let full_url_str = if path.is_empty() {
            base.to_string()
        } else {
            format!("{}/{}", base, path)
        };

        let url = Url::parse(&full_url_str).map_err(|e| format!("Invalid URL '{}': {}", full_url_str, e))?;
        let method = Method::from_str(method_str).map_err(|e| format!("Invalid HTTP method '{}': {}", method_str, e))?;

        // 3. Prepare Client and Request
        let client = Client::new();
        let mut request_builder = client.request(method, url);

        // 4. Headers (Config + Inputs)
        // Config headers
        if let Some(headers) = config.get("headers").and_then(|v| v.as_object()) {
            for (k, v) in headers {
                if let Some(v_str) = v.as_str() {
                    request_builder = request_builder.header(k, v_str);
                }
            }
        }
        // Input headers (override config)
        if let Some(headers) = inputs.get("headers").and_then(|v| v.as_object()) {
            for (k, v) in headers {
                if let Some(v_str) = v.as_str() {
                    request_builder = request_builder.header(k, v_str);
                }
            }
        }

        // 5. Query Params (Config + Inputs)
        if let Some(params) = config.get("query_params") {
            request_builder = request_builder.query(params);
        }
        if let Some(params) = inputs.get("query_params") {
            request_builder = request_builder.query(params);
        }

        // 6. Body (Inputs)
        // Usually body comes from inputs. We look for a "body" key in inputs.
        // If the method is POST/PUT/PATCH, we attach it.
        if let Some(body) = inputs.get("body") {
            request_builder = request_builder.json(body);
        }

        // 7. Execute Request
        let response = request_builder.send().await?;
        let status = response.status().as_u16();
        
        // Try to parse response as JSON, fallback to text/string
        let response_body: Value = match response.json::<Value>().await {
            Ok(json) => json,
            Err(_) => Value::Null, // Or handle text content if needed
        };

        // 8. Return Output
        Ok(json!({
            "output": {
                "status": status,
                "body": response_body
            }
        }))
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
