//! `rmcp`-backed [`McpClientPort`]. **The only file in the crate allowed to
//! name an `rmcp` type** (design §1, ADR-1).
//!
//! No `session.rs`: `rmcp`'s `StreamableHttpClientWorker` already stores
//! whatever session id a server returns on `initialize` and echoes it on
//! every later request, sending none when the server never issues one
//! (R2.3). Reimplementing that would be the "parallel mechanism" this
//! design forbids for containment (§6); the tests below prove both server
//! shapes work end-to-end instead.

use std::time::Duration;

use async_trait::async_trait;
use rmcp::model::{
    CallToolRequestParams, CallToolResult, ClientCapabilities, ClientInfo, ContentBlock,
    Implementation, Tool,
};
use rmcp::service::{RoleClient, RunningService};
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::{ServiceError, ServiceExt};
use serde_json::Value;

use crate::llm::domain::mcp::{
    McpClientPort, McpError, McpServerConfig, McpToolDescriptor, McpToolResult,
};

/// A live connection to one remote MCP server over `rmcp`'s streamable-HTTP
/// client transport.
#[derive(Debug)]
pub struct RmcpHttpClient {
    server_label: String,
    timeout: Duration,
    running: RunningService<RoleClient, ClientInfo>,
}

impl RmcpHttpClient {
    /// Connect and complete the handshake (`initialize` ->
    /// `notifications/initialized`, driven by `rmcp`'s `ServiceExt::serve`).
    /// HTTPS is enforced HERE, before any socket work (R2.1) — the
    /// defensive re-check; `Graph::validate` (later slice) is primary.
    pub async fn connect(server_label: &str, config: &McpServerConfig) -> Result<Self, McpError> {
        if !config.url.starts_with("https://") {
            return Err(McpError::InvalidConfig {
                detail: format!(
                    "MCP server URL must be HTTPS (server '{server_label}' has '{}')",
                    config.url
                ),
            });
        }
        Self::connect_transport(server_label, config).await
    }

    /// Everything `connect` does after the HTTPS guard — split out so tests
    /// can drive it against `wiremock`, which serves plain HTTP only.
    async fn connect_transport(
        server_label: &str,
        config: &McpServerConfig,
    ) -> Result<Self, McpError> {
        // `StreamableHttpClientTransportConfig::default()` sets
        // `allow_stateless: true`: both stateless and session-issuing
        // servers work without a config flag (R2.3).
        let transport_config = StreamableHttpClientTransportConfig::with_uri(config.url.clone());
        let transport = StreamableHttpClientTransport::from_config(transport_config);

        // `ClientCapabilities::default()` leaves `sampling: None`, so the
        // serialized `initialize` params never carry that key regardless of
        // what the server advertises back (R4.8).
        let client_info = ClientInfo::new(
            ClientCapabilities::default(),
            Implementation::new("colmena", env!("CARGO_PKG_VERSION")),
        );

        // Bound the handshake with the SAME configured timeout that guards
        // list_tools/call_tool. `ServiceExt::serve` has no internal deadline and
        // rmcp installs no reqwest connect/read timeout, so without this a server
        // that accepts the TCP connection and then goes silent would block the
        // caller forever — and `timeout_seconds` would not mean what it says.
        let running = tokio::time::timeout(config.timeout, client_info.serve(transport))
            .await
            .map_err(|_| McpError::Timeout {
                server: server_label.to_string(),
                seconds: config.timeout.as_secs(),
            })?
            .map_err(|e| McpError::Handshake {
                server: server_label.to_string(),
                detail: e.to_string(),
            })?;

        Ok(Self {
            server_label: server_label.to_string(),
            timeout: config.timeout,
            running,
        })
    }

    /// Test-only: bypasses the HTTPS guard so `wiremock` (plain HTTP, no
    /// TLS support) can exercise handshake/session/timeout behavior.
    /// `#[cfg(test)]` keeps it out of production builds; R2.1 is enforced by
    /// `connect` and asserted independently by `rmcp_connect_rejects_non_https_url`.
    #[cfg(test)]
    async fn connect_for_test(
        server_label: &str,
        config: &McpServerConfig,
    ) -> Result<Self, McpError> {
        Self::connect_transport(server_label, config).await
    }
}

#[async_trait]
impl McpClientPort for RmcpHttpClient {
    async fn list_tools(&self) -> Result<Vec<McpToolDescriptor>, McpError> {
        let tools = tokio::time::timeout(self.timeout, self.running.list_all_tools())
            .await
            .map_err(|_| self.timeout_error())?
            .map_err(|e| self.service_error(e))?;
        Ok(tools.iter().map(descriptor_from_tool).collect())
    }

    async fn call_tool(&self, name: &str, arguments: Value) -> Result<McpToolResult, McpError> {
        let params = self.build_call_params(name, arguments)?;
        let result = tokio::time::timeout(self.timeout, self.running.call_tool(params))
            .await
            .map_err(|_| self.timeout_error())?
            .map_err(|e| self.service_error(e))?;
        Ok(mcp_result_from(result))
    }

    fn server_label(&self) -> &str {
        &self.server_label
    }
}

impl RmcpHttpClient {
    fn build_call_params(
        &self,
        name: &str,
        arguments: Value,
    ) -> Result<CallToolRequestParams, McpError> {
        let params = CallToolRequestParams::new(name.to_string());
        match arguments {
            Value::Object(map) => Ok(params.with_arguments(map)),
            Value::Null => Ok(params),
            other => Err(McpError::Protocol {
                server: self.server_label.clone(),
                detail: format!("tool '{name}' arguments must be a JSON object, got: {other}"),
            }),
        }
    }

    fn timeout_error(&self) -> McpError {
        McpError::Timeout {
            server: self.server_label.clone(),
            seconds: self.timeout.as_secs(),
        }
    }

    /// Maps `rmcp`'s error onto our taxonomy — no catch-all inside `McpError`
    /// itself, but this boundary function may fall back to `Transport` for
    /// `rmcp` variants that don't fit a more specific case.
    fn service_error(&self, err: ServiceError) -> McpError {
        match err {
            ServiceError::McpError(e) => McpError::Protocol {
                server: self.server_label.clone(),
                detail: e.to_string(),
            },
            ServiceError::Timeout { .. } => self.timeout_error(),
            other => McpError::Transport {
                server: self.server_label.clone(),
                reason: other.to_string(),
            },
        }
    }
}

/// `input_schema` is forwarded verbatim (R4.4) — never flattened here.
fn descriptor_from_tool(tool: &Tool) -> McpToolDescriptor {
    McpToolDescriptor {
        name: tool.name.to_string(),
        title: tool.title.clone(),
        description: tool
            .description
            .clone()
            .map(|d| d.to_string())
            .unwrap_or_default(),
        input_schema: Value::Object((*tool.input_schema).clone()),
    }
}

/// Text blocks folded losslessly; non-text blocks are slice 2b's scope (R2.6).
fn mcp_result_from(result: CallToolResult) -> McpToolResult {
    let content = result
        .content
        .into_iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.text),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    McpToolResult {
        content,
        is_error: result.is_error.unwrap_or(false),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use rmcp::model::{
        CallToolResult, ClientJsonRpcMessage, ContentBlock, InitializeResult, ListToolsResult,
        ServerCapabilities, ServerJsonRpcMessage, ServerResult, Tool, ToolsCapability,
    };
    use serde_json::json;
    use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

    use super::{McpClientPort, RmcpHttpClient};
    use crate::llm::domain::mcp::{McpError, McpServerConfig, McpTransport};

    fn config(url: String, timeout_secs: u64) -> McpServerConfig {
        McpServerConfig {
            url,
            transport: McpTransport::StreamableHttp,
            header_refs: BTreeMap::new(),
            timeout: Duration::from_secs(timeout_secs),
            cache_ttl: Duration::from_secs(60),
        }
    }

    /// A tool whose schema carries constraints the flat `ParameterProperty`
    /// model cannot express — `minItems`, `minimum`, an array-of-enum and a
    /// `$schema` marker. Mirrors the shape observed on the real HuggingFace
    /// MCP server, which is why the domain type holds a raw `Value`.
    fn tool_with_rich_schema(name: &'static str) -> (Tool, serde_json::Value) {
        let schema = json!({
            "type": "object",
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "properties": {
                "repo_ids": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": 10,
                    "items": {"type": "string", "minLength": 1}
                },
                "operations": {
                    "type": "array",
                    "items": {"type": "string", "enum": ["overview", "preview"]}
                },
                "offset": {"type": "integer", "minimum": 0}
            },
            "required": ["repo_ids"]
        });
        let tool = Tool::new(name, "test tool", schema.as_object().cloned().unwrap());
        (tool, schema)
    }

    fn tool(name: &'static str) -> Tool {
        Tool::new(
            name,
            "test tool",
            json!({"type": "object"}).as_object().cloned().unwrap(),
        )
    }

    /// R2.1 — HTTPS is checked before any network attempt.
    #[tokio::test]
    async fn rmcp_connect_rejects_non_https_url() {
        let cfg = config("http://insecure.example.com/mcp".to_string(), 5);
        let err = RmcpHttpClient::connect("insecure", &cfg)
            .await
            .expect_err("http:// must be rejected before any network call");
        assert!(matches!(err, McpError::InvalidConfig { .. }));
        assert!(err.to_string().contains("HTTPS"));
    }

    /// Replies to `initialize`, `notifications/initialized`, `tools/list`
    /// and `tools/call` over the exact wire shape `rmcp`'s client sends
    /// (single POST endpoint, JSON-RPC bodies, optional `Mcp-Session-Id`).
    /// Every request body is recorded for tests that inspect it.
    struct McpMock {
        tools: Vec<Tool>,
        session_id: Option<&'static str>,
        call_delay: Option<Duration>,
        seen: Arc<Mutex<Vec<String>>>,
    }

    impl Respond for McpMock {
        fn respond(&self, request: &Request) -> ResponseTemplate {
            if request.method.as_str() == "GET" {
                // No persistent SSE stream in this mock; 405 tells `rmcp`
                // the server doesn't support it (non-fatal: "skip common stream").
                return ResponseTemplate::new(405);
            }
            self.seen
                .lock()
                .unwrap()
                .push(String::from_utf8_lossy(&request.body).to_string());

            let msg: ClientJsonRpcMessage = request
                .body_json()
                .expect("rmcp only sends well-formed JSON-RPC");
            let ClientJsonRpcMessage::Request(req) = msg else {
                return ResponseTemplate::new(202); // e.g. notifications/initialized
            };
            let id = req.id.clone();
            match req.request.method() {
                "initialize" => {
                    let mut capabilities = ServerCapabilities::default();
                    capabilities.tools = Some(ToolsCapability::default());
                    let body = ServerJsonRpcMessage::response(
                        ServerResult::InitializeResult(InitializeResult::new(capabilities)),
                        id,
                    );
                    let mut tmpl = ResponseTemplate::new(200).set_body_json(&body);
                    if let Some(sid) = self.session_id {
                        tmpl = tmpl.insert_header("mcp-session-id", sid);
                    }
                    tmpl
                }
                "tools/list" => {
                    let body = ServerJsonRpcMessage::response(
                        ServerResult::ListToolsResult(ListToolsResult::with_all_items(
                            self.tools.clone(),
                        )),
                        id,
                    );
                    ResponseTemplate::new(200).set_body_json(&body)
                }
                "tools/call" => {
                    let result = CallToolResult::success(vec![ContentBlock::text("ok")]);
                    let body =
                        ServerJsonRpcMessage::response(ServerResult::CallToolResult(result), id);
                    let mut tmpl = ResponseTemplate::new(200).set_body_json(&body);
                    if let Some(delay) = self.call_delay {
                        tmpl = tmpl.set_delay(delay);
                    }
                    tmpl
                }
                other => panic!("mock received an unexpected JSON-RPC method: {other}"),
            }
        }
    }

    fn mock(
        tools: Vec<Tool>,
        session_id: Option<&'static str>,
        call_delay: Option<Duration>,
    ) -> McpMock {
        McpMock {
            tools,
            session_id,
            call_delay,
            seen: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// R2.2 — full handshake against a stateless server succeeds.
    #[tokio::test]
    async fn rmcp_handshake_initialize_then_notified() {
        let server = MockServer::start().await;
        Mock::given(wiremock::matchers::any())
            .respond_with(mock(Vec::new(), None, None))
            .mount(&server)
            .await;

        let cfg = config(server.uri(), 5);
        let client = RmcpHttpClient::connect_for_test("test-server", &cfg).await;
        assert!(client.is_ok(), "handshake must succeed: {:?}", client.err());

        // `is_ok()` alone would pass even if only `initialize` were sent, so
        // assert on the wire: both messages, in this order.
        let methods: Vec<String> = server
            .received_requests()
            .await
            .expect("logging enabled")
            .iter()
            .filter(|r| r.method.as_str() == "POST")
            .filter_map(|r| {
                serde_json::from_slice::<serde_json::Value>(&r.body).ok()?["method"]
                    .as_str()
                    .map(str::to_string)
            })
            .collect();
        assert_eq!(
            methods,
            vec!["initialize", "notifications/initialized"],
            "the handshake must send initialize then notifications/initialized, in that order"
        );
    }

    /// R2.4 — the configured timeout must bound the HANDSHAKE too, not only
    /// `list_tools`/`call_tool`. `ServiceExt::serve` has no internal deadline
    /// and rmcp installs no reqwest timeout, so a server that accepts the
    /// connection and then goes silent would otherwise hang forever.
    #[tokio::test]
    async fn rmcp_connect_times_out_at_configured_seconds() {
        let server = MockServer::start().await;
        Mock::given(wiremock::matchers::any())
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(10)))
            .mount(&server)
            .await;

        let cfg = config(server.uri(), 1);
        let started = Instant::now();
        let err = RmcpHttpClient::connect_for_test("slow", &cfg)
            .await
            .expect_err("a silent server must not hang the handshake");
        assert!(
            matches!(err, McpError::Timeout { seconds: 1, .. }),
            "expected a Timeout carrying the configured seconds, got: {err:?}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "connect must give up at the configured timeout, took {:?}",
            started.elapsed()
        );
    }

    /// R2.2/Finding 1 — a stateless server's `tools/list` returns tools
    /// verbatim (mirrors DeepWiki: 3 tools, stateless) and no session id
    /// header is ever sent.
    #[tokio::test]
    async fn rmcp_list_tools_deepwiki_stateless_shape() {
        let server = MockServer::start().await;
        let tools = vec![
            tool("read_wiki_structure"),
            tool("read_wiki_contents"),
            tool("ask_question"),
        ];
        Mock::given(wiremock::matchers::any())
            .respond_with(mock(tools, None, None))
            .mount(&server)
            .await;

        let cfg = config(server.uri(), 5);
        let client = RmcpHttpClient::connect_for_test("deepwiki", &cfg)
            .await
            .expect("connect must succeed");
        let tools = client.list_tools().await.expect("list_tools must succeed");
        assert_eq!(tools.len(), 3);
        assert_eq!(tools[0].name, "read_wiki_structure");

        let requests = server.received_requests().await.expect("logging enabled");
        assert!(
            requests
                .iter()
                .all(|r| !r.headers.contains_key("mcp-session-id")),
            "a stateless server must never receive a session id header"
        );
    }

    /// R4.4 — the server's JSON Schema reaches the domain type BYTE-IDENTICAL.
    /// This is the load-bearing property of the whole feature: a later slice
    /// forwards this straight into `ToolDefinition::input_schema_override`,
    /// and the flat `ParameterProperty` model cannot represent `minItems`,
    /// `minimum` or `$schema`, so any flattening here is silent data loss.
    #[tokio::test]
    async fn rmcp_list_tools_forwards_input_schema_verbatim() {
        let server = MockServer::start().await;
        let (rich_tool, expected_schema) = tool_with_rich_schema("hub_repo_details");
        Mock::given(wiremock::matchers::any())
            .respond_with(mock(vec![rich_tool], None, None))
            .mount(&server)
            .await;

        let cfg = config(server.uri(), 5);
        let client = RmcpHttpClient::connect_for_test("hf", &cfg)
            .await
            .expect("connect must succeed");
        let tools = client.list_tools().await.expect("list_tools must succeed");

        assert_eq!(
            serde_json::to_string(&tools[0].input_schema).unwrap(),
            serde_json::to_string(&expected_schema).unwrap(),
            "input_schema must survive the round trip byte-identical"
        );
    }

    /// R2.3/Finding 4 — a server that returns `Mcp-Session-Id` on
    /// `initialize` gets it echoed on the next request, no config flag needed.
    #[tokio::test]
    async fn rmcp_stateful_session_id_echoed_on_next_request() {
        let server = MockServer::start().await;
        Mock::given(wiremock::matchers::any())
            .respond_with(mock(vec![tool("hf_whoami")], Some("sess-abc123"), None))
            .mount(&server)
            .await;

        let cfg = config(server.uri(), 5);
        let client = RmcpHttpClient::connect_for_test("huggingface", &cfg)
            .await
            .expect("connect must succeed");
        client.list_tools().await.expect("list_tools must succeed");

        let requests = server.received_requests().await.expect("logging enabled");
        let list_tools_request = requests
            .iter()
            .find(|r| {
                r.body_json::<ClientJsonRpcMessage>()
                    .ok()
                    .is_some_and(|m| match m {
                        ClientJsonRpcMessage::Request(req) => req.request.method() == "tools/list",
                        _ => false,
                    })
            })
            .expect("a tools/list request must have been sent");

        assert_eq!(
            list_tools_request
                .headers
                .get("mcp-session-id")
                .and_then(|v| v.to_str().ok()),
            Some("sess-abc123"),
            "the session id issued on initialize must be echoed on later requests"
        );
    }

    /// R2.4 — a hung server does not block the caller past `timeout_seconds`.
    #[tokio::test]
    async fn rmcp_call_tool_times_out_at_configured_seconds() {
        let server = MockServer::start().await;
        Mock::given(wiremock::matchers::any())
            .respond_with(mock(Vec::new(), None, Some(Duration::from_secs(5))))
            .mount(&server)
            .await;

        let cfg = config(server.uri(), 1);
        let client = RmcpHttpClient::connect_for_test("slow-server", &cfg)
            .await
            .expect("connect must succeed (initialize is not delayed)");

        let started = Instant::now();
        let result = client.call_tool("anything", json!({})).await;
        let elapsed = started.elapsed();

        assert!(matches!(result, Err(McpError::Timeout { seconds: 1, .. })));
        assert!(
            elapsed < Duration::from_secs(3),
            "must not wait near the mock's 5s delay; waited {elapsed:?}"
        );
    }

    /// R4.8 — `sampling` is never advertised: the serialized `initialize`
    /// params carry no `sampling` key.
    #[tokio::test]
    async fn mcp_handshake_advertises_no_sampling_capability() {
        let server = MockServer::start().await;
        let responder = mock(Vec::new(), None, None);
        let seen = responder.seen.clone();
        Mock::given(wiremock::matchers::any())
            .respond_with(responder)
            .mount(&server)
            .await;

        let cfg = config(server.uri(), 5);
        RmcpHttpClient::connect_for_test("no-sampling", &cfg)
            .await
            .expect("connect must succeed");

        let bodies = seen.lock().unwrap();
        let init_body = bodies
            .iter()
            .find(|b| b.contains("\"method\":\"initialize\""))
            .expect("an initialize request must have been sent");
        assert!(
            !init_body.contains("sampling"),
            "initialize params must never mention sampling: {init_body}"
        );
    }
}
