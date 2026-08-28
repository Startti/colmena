//! `rmcp`-backed [`McpClientPort`]. **The only file in the crate allowed to
//! name an `rmcp` type** (design §1, ADR-1).
//!
//! No `session.rs`: `rmcp`'s `StreamableHttpClientWorker` already stores
//! whatever session id a server returns on `initialize` and echoes it on
//! every later request, sending none when the server never issues one
//! (R2.3). Reimplementing that would be the "parallel mechanism" this
//! design forbids for containment (§6); the tests below prove both server
//! shapes work end-to-end instead.
//!
//! ## Timeout ownership — explicit cancel notification, not a dropped future (2b)
//!
//! Full empirical writeup: `docs/CHANGELOG_2026-08.md` §21. Short version:
//! slice 2a's outer `tokio::time::timeout` around `list_all_tools`/`call_tool`
//! enforced the deadline but leaked one `local_responder_pool` entry per
//! timeout (dropping the future never cancels the in-flight rmcp request).
//! The design's fix — `send_cancellable_request` +
//! `PeerRequestOptions::timeout`, letting `RequestHandle::await_response`'s
//! own internal race own the cleanup — does NOT hold empirically against
//! rmcp 3.1.4 + `transport-streamable-http-client-reqwest`: its internal
//! timer does fire on schedule, but it then `.await`s a cancel notification
//! that is serialized behind the still-in-flight original request, so the
//! caller does not regain control until that request finishes anyway.
//!
//! `send_request` still uses `send_cancellable_request` (the only path that
//! returns a `Peer`/`RequestId` we can clone before consuming the handle),
//! but wraps `await_response()` in our own `tokio::time::timeout`, and on
//! expiry fires the cancel notification as a DETACHED `tokio::spawn`ed task
//! ([`RmcpHttpClient::spawn_cancel_notification`]) instead of awaiting it —
//! that detachment is what actually returns control to the caller at the
//! promised deadline. `list_tools` pages `tools/list` itself (no longer
//! calls `Peer::list_all_tools`) so each page is bounded and cancelled
//! independently, and the page count itself is capped (a server controls
//! `next_cursor`, so without a ceiling the loop would spin forever with every
//! individual page dutifully bounded). Worst case is therefore
//! `max_pages * timeout_seconds`, and every page is cancelled on its own
//! timeout so the pool never grows unbounded. `connect`'s handshake keeps
//! its own outer timeout around `ServiceExt::serve`, unrelated to this.

use std::time::Duration;

use async_trait::async_trait;
use rmcp::model::{
    CallToolRequest, CallToolRequestParams, CallToolResult, CancelledNotificationParam,
    ClientCapabilities, ClientInfo, ClientRequest, ContentBlock, Implementation, ListToolsRequest,
    ListToolsResult, Notification, PaginatedRequestParams, RequestId, ResourceContents,
    ServerResult, Tool,
};
use rmcp::service::{Peer, PeerRequestOptions, RoleClient, RunningService};
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::{ServiceError, ServiceExt};
use serde_json::Value;

use crate::llm::domain::mcp::{
    McpClientPort, McpError, McpServerConfig, McpToolDescriptor, McpToolResult,
    MCP_MAX_TOOLS_PER_SERVER,
};
use crate::llm::domain::text_bounds::head_truncate;

/// Backoff between the single retry attempt and the original one (R2.5).
/// Small and fixed — this is not exponential backoff for a retry budget of
/// one, just a brief pause so a genuinely transient failure (a mid-flight
/// connection reset) has a moment to clear before the retry.
const MCP_RETRY_BACKOFF: Duration = Duration::from_millis(75);

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
        let mut tools = Vec::new();
        let mut cursor: Option<String> = None;
        // A server controls `next_cursor`, so the loop needs its own ceiling:
        // one that keeps handing back a cursor would otherwise spin forever,
        // each page dutifully bounded and the whole call unbounded.
        //
        // The number is borrowed from the tool ceiling for want of a better
        // one; it bounds PAGES, not tools or bytes. A single page carrying an
        // enormous `tools` array still passes. Capping that belongs to the
        // exposure slice, which is where `MCP_MAX_TOOLS_PER_SERVER` acquires
        // its real meaning — today nothing else reads it.
        let max_pages = MCP_MAX_TOOLS_PER_SERVER.max(1);
        let mut pages = 0usize;
        loop {
            pages += 1;
            if pages > max_pages {
                return Err(McpError::Protocol {
                    server: self.server_label.clone(),
                    detail: format!(
                        "tools/list kept returning a cursor after {max_pages} pages; refusing to \
                         page further"
                    ),
                });
            }
            let page_cursor = cursor.clone();
            let result = self
                .retry_transient(|| async {
                    let mut params = PaginatedRequestParams::default();
                    params.cursor = page_cursor.clone();
                    let request =
                        ClientRequest::ListToolsRequest(ListToolsRequest::with_param(params));
                    self.send_request(request).await
                })
                .await?;
            let ListToolsResult {
                tools: page,
                next_cursor,
                ..
            } = match result {
                ServerResult::ListToolsResult(r) => r,
                other => return Err(self.unexpected_response("tools/list", &other)),
            };
            tools.extend(page.iter().map(descriptor_from_tool));
            cursor = next_cursor;
            if cursor.is_none() {
                break;
            }
        }
        Ok(tools)
    }

    async fn call_tool(&self, name: &str, arguments: Value) -> Result<McpToolResult, McpError> {
        let params = self.build_call_params(name, arguments)?;
        // NOT retried, deliberately. A transport error can arrive AFTER the
        // server already ran the tool — a connection reset while reading the
        // response is indistinguishable, at this layer, from one before the
        // request was sent. MCP gives no way to declare a tool idempotent, and
        // the tools worth exposing are exactly the ones with side effects, so a
        // blind retry can bill a card or send a message twice for one call the
        // model made once. `list_tools` is retried because it only reads.
        let result = {
            let request = ClientRequest::CallToolRequest(CallToolRequest::new(params.clone()));
            self.send_request(request).await?
        };
        match result {
            ServerResult::CallToolResult(r) => Ok(mcp_result_from(r)),
            other => Err(self.unexpected_response("tools/call", &other)),
        }
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

    /// Sends one JSON-RPC request through `rmcp`'s cancellable-request path
    /// and enforces `self.timeout` with an explicit cancel notification on
    /// expiry — see the module doc ("Timeout ownership") for why this is an
    /// outer `tokio::time::timeout` plus an explicit notification, rather
    /// than the `PeerRequestOptions.timeout` field alone.
    async fn send_request(&self, request: ClientRequest) -> Result<ServerResult, McpError> {
        let handle = self
            .running
            .send_cancellable_request(request, PeerRequestOptions::no_options())
            .await
            .map_err(|e| self.service_error(e))?;
        let peer = handle.peer.clone();
        let request_id = handle.id.clone();
        match tokio::time::timeout(self.timeout, handle.await_response()).await {
            Ok(inner) => inner.map_err(|e| self.service_error(e)),
            Err(_) => {
                Self::spawn_cancel_notification(peer, request_id);
                Err(self.timeout_error())
            }
        }
    }

    /// Fires the `notifications/cancelled` message rmcp's own (private)
    /// `RequestHandle::send_timeout_cancel_notification` would send on an
    /// internal timeout — as a DETACHED background task, not awaited. See
    /// the module doc ("Timeout ownership") and `docs/CHANGELOG_2026-08.md`
    /// §21 for why: awaiting it inline serializes behind the still-hung
    /// request we are trying to walk away from, which is the same problem
    /// rmcp's own internal cancel path has. Detaching is what actually
    /// returns control to the caller at `self.timeout`; the responder-pool
    /// entry is still cleaned up, just on this task's own time.
    fn spawn_cancel_notification(peer: Peer<RoleClient>, request_id: RequestId) {
        tokio::spawn(async move {
            let notification: rmcp::model::CancelledNotification =
                Notification::new(CancelledNotificationParam::new(
                    Some(request_id),
                    Some("client timeout".to_string()),
                ));
            let _ = peer.send_notification(notification.into()).await;
        });
    }

    /// One bounded retry with a fixed backoff (R2.5), scoped ONLY to
    /// `McpError::Transport` — a transient connection-level failure
    /// (reset/DNS/TLS). Everything else is left alone on purpose:
    /// - `Timeout` is not proven transient the way a connection reset is —
    ///   retrying it would double the wait on a server that is simply slow.
    /// - `Protocol`/`Handshake` indicate a malformed exchange, not a blip;
    ///   retrying would resend the same malformed request.
    /// - A `tools/call` that completes with `isError: true` is `Ok(..)`
    ///   from `call_tool`'s perspective (design §2a: it is a legitimate
    ///   model-correctable failure, R4.5) — it never reaches this helper's
    ///   `Err` arm at all, so it is structurally impossible to retry here.
    async fn retry_transient<T, F, Fut>(&self, mut op: F) -> Result<T, McpError>
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = Result<T, McpError>>,
    {
        match op().await {
            Err(McpError::Transport { .. }) => {
                tokio::time::sleep(MCP_RETRY_BACKOFF).await;
                op().await
            }
            other => other,
        }
    }

    fn timeout_error(&self) -> McpError {
        McpError::Timeout {
            server: self.server_label.clone(),
            seconds: self.timeout.as_secs(),
        }
    }

    fn unexpected_response(&self, method: &str, result: &ServerResult) -> McpError {
        McpError::Protocol {
            server: self.server_label.clone(),
            detail: format!("unexpected response shape for '{method}': {result:?}"),
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

/// Renders one content block as the text a tool-result message can carry.
///
/// Text is preserved verbatim. Everything else becomes a named placeholder
/// rather than being dropped: the model must be able to tell that an image,
/// an audio clip or a binary resource was part of the answer, and what it
/// was, even though this transport can only carry text. Silently discarding
/// them produces a truncated answer that reads as complete — the failure mode
/// this function exists to prevent.
///
/// Placeholders deliberately never embed the encoded payload. A base64 blob
/// forwarded into the model's context would cost a fortune in tokens and say
/// nothing; the mime type and byte count say everything useful about it.
/// Ceiling, in bytes, for a single rendered content block.
///
/// A placeholder interpolates strings the SERVER controls — mime type, uri,
/// resource name. Without a ceiling a hostile or merely sloppy server turns
/// "omitted, here is what it was" into an arbitrarily large context flood,
/// which is exactly the failure the placeholder was meant to avoid. Text
/// blocks are bounded by the same ceiling for the same reason.
///
/// This belongs alongside the other `MCP_MAX_*` caps in
/// `llm::domain::mcp`; it lives here until the exposure slice next touches
/// that file.
const MCP_MAX_CONTENT_BLOCK_BYTES: usize = 4 * 1024;

fn content_block_to_text(block: ContentBlock) -> String {
    let rendered = render_content_block(block);
    if rendered.len() > MCP_MAX_CONTENT_BLOCK_BYTES {
        head_truncate(&rendered, MCP_MAX_CONTENT_BLOCK_BYTES)
    } else {
        rendered
    }
}

/// The unbounded rendering. Every caller must go through
/// [`content_block_to_text`], which applies the ceiling.
fn render_content_block(block: ContentBlock) -> String {
    match block {
        ContentBlock::Text(text) => text.text,
        ContentBlock::Image(img) => format!(
            "[image content omitted: {} ({} base64 bytes)]",
            img.mime_type,
            img.data.len()
        ),
        ContentBlock::Audio(audio) => format!(
            "[audio content omitted: {} ({} base64 bytes)]",
            audio.mime_type,
            audio.data.len()
        ),
        ContentBlock::Resource(embedded) => match &embedded.resource {
            // An embedded TEXT resource is real content, not something to
            // elide — it is exactly what the caller asked for, delivered
            // under a uri instead of inline.
            ResourceContents::TextResourceContents { .. } => embedded.get_text(),
            ResourceContents::BlobResourceContents {
                uri,
                mime_type,
                blob,
                ..
            } => format!(
                "[resource content omitted: {uri}, {}, {} base64 bytes]",
                mime_type.as_deref().unwrap_or("unknown mime type"),
                blob.len()
            ),
            // `ResourceContents` is `#[non_exhaustive]` too.
            _ => "[resource content omitted: unrecognized resource shape]".to_string(),
        },
        ContentBlock::ResourceLink(link) => {
            format!("[resource link omitted: {} ({})]", link.uri, link.name)
        }
        // `ContentBlock` is `#[non_exhaustive]` in `rmcp` — this arm is dead
        // today, since all five current variants are matched above, but it
        // keeps the crate compiling rather than failing to build the day
        // `rmcp` adds a sixth. Same honest-placeholder policy as the rest.
        _ => "[content block omitted: unrecognized block type]".to_string(),
    }
}

/// Folds a tool result's content blocks into the single `String` a tool-result
/// message carries.
///
/// Every block contributes exactly one line via [`content_block_to_text`], so
/// no part of a tool's answer is lost without a trace.
fn mcp_result_from(result: CallToolResult) -> McpToolResult {
    let content = result
        .content
        .into_iter()
        .map(content_block_to_text)
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
        /// R2.5 — the first N `tools/call` attempts get a synthetic
        /// transport-level failure (non-JSON 500) before the mock starts
        /// answering normally; used to prove the bounded retry.
        fail_transport_first_n_calls: usize,
        /// R2.5/R4.5 — when true, `tools/call` answers with a well-formed
        /// `isError: true` result instead of success, to prove that path is
        /// never retried.
        call_is_error: bool,
        /// How many `tools/list` pages to hand out before stopping. `None`
        /// means the single-page shape most tests want; `Some(n)` returns a
        /// `next_cursor` on the first `n` pages; `Some(usize::MAX)` never
        /// stops, to exercise the loop's own ceiling.
        list_pages: Option<usize>,
        /// Every request body this mock received, in order, so tests can
        /// assert on the wire rather than on a return value.
        seen: Arc<Mutex<Vec<String>>>,
    }

    impl McpMock {
        /// A non-JSON, non-success body for the first N attempts at `method`.
        /// rmcp's transport cannot parse it as JSON-RPC and surfaces a
        /// transport-level failure, which our adapter maps to
        /// `McpError::Transport` — a stand-in for a connection reset.
        ///
        /// Keyed by method so both the retried operation (`tools/list`) and the
        /// deliberately un-retried one (`tools/call`) can be exercised.
        fn synthetic_transient_failure(&self, method: &str) -> Option<ResponseTemplate> {
            let needle = format!("\"method\":\"{method}\"");
            let prior_attempts = self
                .seen
                .lock()
                .unwrap()
                .iter()
                .filter(|b| b.contains(&needle))
                .count()
                - 1; // exclude the request just pushed above
            (prior_attempts < self.fail_transport_first_n_calls).then(|| {
                ResponseTemplate::new(500)
                    .set_body_raw(b"synthetic transient failure".to_vec(), "text/plain")
            })
        }

        fn with_fail_transport_first_n_calls(mut self, n: usize) -> Self {
            self.fail_transport_first_n_calls = n;
            self
        }

        /// Hand out `next_cursor` on the first `n` `tools/list` pages.
        /// `usize::MAX` never stops — for the ceiling test.
        fn with_list_pages(mut self, n: usize) -> Self {
            self.list_pages = Some(n);
            self
        }

        fn with_call_is_error(mut self, is_error: bool) -> Self {
            self.call_is_error = is_error;
            self
        }
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
                    if let Some(failure) = self.synthetic_transient_failure("tools/list") {
                        return failure;
                    }
                    // Drive pagination from the cursor the CLIENT sent, read
                    // off the raw request body, not from a request counter.
                    // Counting requests would hand out page 2 even to a client
                    // that dropped the cursor, so the test would pass while the
                    // very thing it names was broken.
                    let sent_cursor: Option<String> =
                        serde_json::from_slice::<serde_json::Value>(&request.body)
                            .ok()
                            .and_then(|v| {
                                v.get("params")?.get("cursor")?.as_str().map(str::to_string)
                            });
                    let page_index = match sent_cursor.as_deref() {
                        None => 0,
                        Some(c) => c
                            .strip_prefix("page-")
                            .and_then(|n| n.parse::<usize>().ok())
                            .unwrap_or(0),
                    };
                    let next_cursor = match self.list_pages {
                        Some(n) if n == usize::MAX => Some(format!("page-{}", page_index + 1)),
                        Some(n) if page_index < n => Some(format!("page-{}", page_index + 1)),
                        _ => None,
                    };
                    let mut page = ListToolsResult::with_all_items(self.tools.clone());
                    page.next_cursor = next_cursor;
                    let body =
                        ServerJsonRpcMessage::response(ServerResult::ListToolsResult(page), id);
                    ResponseTemplate::new(200).set_body_json(&body)
                }
                "tools/call" => {
                    if let Some(failure) = self.synthetic_transient_failure("tools/call") {
                        return failure;
                    }
                    let content = vec![ContentBlock::text(if self.call_is_error {
                        "boom: invalid arguments"
                    } else {
                        "ok"
                    })];
                    let result = if self.call_is_error {
                        CallToolResult::error(content)
                    } else {
                        CallToolResult::success(content)
                    };
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
            fail_transport_first_n_calls: 0,
            call_is_error: false,
            list_pages: None,
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

    // -----------------------------------------------------------------
    /// `list_tools` threads the cursor across pages and accumulates all of
    /// them. The single-page shape every other test uses would never exercise
    /// this, so the loop's cursor handling had no coverage at all.
    #[tokio::test]
    async fn rmcp_list_tools_accumulates_across_pages() {
        let server = MockServer::start().await;
        // Two pages: the first answers with a cursor, the second without.
        Mock::given(wiremock::matchers::any())
            .respond_with(mock(vec![tool("read_wiki_structure")], None, None).with_list_pages(1))
            .mount(&server)
            .await;

        let cfg = config(server.uri(), 5);
        let client = RmcpHttpClient::connect_for_test("paged", &cfg)
            .await
            .expect("connect must succeed");
        let tools = client.list_tools().await.expect("list_tools must succeed");

        assert_eq!(
            tools.len(),
            2,
            "both pages must be accumulated, got: {tools:?}"
        );

        let list_requests = server
            .received_requests()
            .await
            .expect("logging enabled")
            .iter()
            .filter(|r| String::from_utf8_lossy(&r.body).contains("\"method\":\"tools/list\""))
            .count();
        assert_eq!(list_requests, 2, "one request per page");
    }

    /// A server controls `next_cursor`, so the pagination loop needs its own
    /// ceiling: without one, a server that keeps handing back a cursor spins
    /// forever with every individual page dutifully bounded — unbounded total
    /// work driven entirely by the remote side.
    #[tokio::test]
    async fn rmcp_list_tools_refuses_to_page_forever() {
        let server = MockServer::start().await;
        Mock::given(wiremock::matchers::any())
            .respond_with(
                mock(vec![tool("t")], None, None).with_list_pages(usize::MAX), // never stops
            )
            .mount(&server)
            .await;

        let cfg = config(server.uri(), 5);
        let client = RmcpHttpClient::connect_for_test("endless", &cfg)
            .await
            .expect("connect must succeed");

        let err = client
            .list_tools()
            .await
            .expect_err("an endless cursor must be refused, not followed forever");
        assert!(
            matches!(err, McpError::Protocol { .. }),
            "expected Protocol, got: {err:?}"
        );
        assert!(
            err.to_string().contains("refusing to page further"),
            "the error must say why it stopped: {err}"
        );
    }

    // Retry / no-retry split (R2.5)
    // -----------------------------------------------------------------

    /// R2.5 — `tools/list` IS retried once on a transient transport failure:
    /// it only reads, so running it twice costs a round trip and nothing else.
    #[tokio::test]
    async fn rmcp_transient_transport_error_retries_list_tools_once_then_succeeds() {
        let server = MockServer::start().await;
        let responder = mock(vec![tool("read_wiki_structure")], None, None)
            .with_fail_transport_first_n_calls(1);
        let seen = responder.seen.clone();
        Mock::given(wiremock::matchers::any())
            .respond_with(responder)
            .mount(&server)
            .await;

        let cfg = config(server.uri(), 5);
        let client = RmcpHttpClient::connect_for_test("flaky", &cfg)
            .await
            .expect("connect must succeed");

        let tools = client
            .list_tools()
            .await
            .expect("the retry must transparently succeed");
        assert_eq!(tools.len(), 1);

        let list_attempts = seen
            .lock()
            .unwrap()
            .iter()
            .filter(|b| b.contains("\"method\":\"tools/list\""))
            .count();
        assert_eq!(
            list_attempts, 2,
            "exactly one retry: the first attempt failed transport-level, the second succeeded"
        );
    }

    /// R2.5 — `tools/call` is NEVER retried, even on a transport error.
    ///
    /// A reset can arrive AFTER the server already ran the tool; at this layer
    /// that is indistinguishable from one before the request was sent. MCP has
    /// no way to declare a tool idempotent, and the tools worth exposing are
    /// exactly the ones with side effects — so a blind retry could bill a card
    /// or send a message twice for one call the model made once. The failure is
    /// surfaced to the model, which can decide to try again knowing the risk.
    #[tokio::test]
    async fn rmcp_call_tool_is_never_retried_on_transport_error() {
        let server = MockServer::start().await;
        let responder = mock(Vec::new(), None, None).with_fail_transport_first_n_calls(1);
        let seen = responder.seen.clone();
        Mock::given(wiremock::matchers::any())
            .respond_with(responder)
            .mount(&server)
            .await;

        let cfg = config(server.uri(), 5);
        let client = RmcpHttpClient::connect_for_test("flaky", &cfg)
            .await
            .expect("connect must succeed");

        let err = client
            .call_tool("charge_card", json!({}))
            .await
            .expect_err("a transport failure on tools/call must surface, not retry");
        assert!(
            matches!(err, McpError::Transport { .. }),
            "expected Transport, got: {err:?}"
        );

        let call_attempts = seen
            .lock()
            .unwrap()
            .iter()
            .filter(|b| b.contains("\"method\":\"tools/call\""))
            .count();
        assert_eq!(
            call_attempts, 1,
            "tools/call must reach the server exactly once — a second attempt could \
             re-run a side effect the first one already performed"
        );
    }

    /// R2.5 — a `tools/call` that completes with `isError: true` is a
    /// legitimate, model-correctable failure and reaches the caller as a
    /// successful result carrying the flag, never as a retry.
    ///
    /// Note this holds *structurally*: `call_tool` does not go through
    /// `retry_transient` at all, so there is no retry path to skip. The test
    /// pins the observable behavior — the server sees exactly one `tools/call`.
    #[tokio::test]
    async fn rmcp_is_error_true_response_is_not_retried() {
        let server = MockServer::start().await;
        let responder = mock(Vec::new(), None, None).with_call_is_error(true);
        let seen = responder.seen.clone();
        Mock::given(wiremock::matchers::any())
            .respond_with(responder)
            .mount(&server)
            .await;

        let cfg = config(server.uri(), 5);
        let client = RmcpHttpClient::connect_for_test("erroring", &cfg)
            .await
            .expect("connect must succeed");

        let result = client
            .call_tool("anything", json!({}))
            .await
            .expect("an isError:true tools/call response is Ok, not Err (R4.5)");
        assert!(result.is_error);
        assert_eq!(result.content, "boom: invalid arguments");

        let call_attempts = seen
            .lock()
            .unwrap()
            .iter()
            .filter(|b| b.contains("\"method\":\"tools/call\""))
            .count();
        assert_eq!(
            call_attempts, 1,
            "isError:true must never trigger a retry — it is not a transport failure"
        );
    }

    /// R2.6 — every non-text content block becomes a named, bounded
    /// placeholder instead of being silently dropped. Pure unit test against
    /// `content_block_to_text`, no network involved.
    #[test]
    fn every_non_text_content_block_becomes_a_named_placeholder() {
        use rmcp::model::ResourceContents;

        let cases: Vec<(ContentBlock, &str)> = vec![
            (ContentBlock::audio("QUJD", "audio/wav"), "audio/wav"),
            (
                ContentBlock::resource(ResourceContents::BlobResourceContents {
                    uri: "file:///report.pdf".to_string(),
                    mime_type: Some("application/pdf".to_string()),
                    blob: "QUJD".to_string(),
                    meta: None,
                }),
                "application/pdf",
            ),
            (
                ContentBlock::resource_link(rmcp::model::Resource::new(
                    "https://example.com/doc",
                    "doc",
                )),
                "https://example.com/doc",
            ),
        ];

        for (block, must_name) in cases {
            let text = super::content_block_to_text(block);
            assert!(
                text.contains(must_name),
                "the placeholder must name what was elided; got: {text}"
            );
            assert!(
                text.starts_with('['),
                "a placeholder must be visibly a placeholder, not passable as content: {text}"
            );
            assert!(
                text.len() < 200,
                "a placeholder must stay bounded, got {} bytes",
                text.len()
            );
            assert!(
                !text.contains("QUJD"),
                "the encoded payload must never be forwarded: {text}"
            );
        }
    }

    /// R2.6 — an embedded TEXT resource is real content, not something to
    /// elide: it must be preserved losslessly, unlike its blob sibling.
    #[test]
    fn embedded_text_resource_is_preserved_not_placeholdered() {
        let text = super::content_block_to_text(ContentBlock::embedded_text(
            "file:///notes.md",
            "the actual note body",
        ));
        assert_eq!(text, "the actual note body");
    }

    /// R2.6 — protocol-level proof: a `tools/call` response mixing a text
    /// block and an image block must surface both in `call_tool`'s output —
    /// the text losslessly, the image as a named placeholder — never drop
    /// the image or forward its raw base64 payload.
    #[tokio::test]
    async fn rmcp_call_tool_non_text_block_becomes_placeholder_not_silently_dropped() {
        let server = MockServer::start().await;
        struct ImageMock {
            seen: Arc<Mutex<Vec<String>>>,
        }
        impl Respond for ImageMock {
            fn respond(&self, request: &Request) -> ResponseTemplate {
                if request.method.as_str() == "GET" {
                    return ResponseTemplate::new(405);
                }
                self.seen
                    .lock()
                    .unwrap()
                    .push(String::from_utf8_lossy(&request.body).to_string());
                let msg: ClientJsonRpcMessage = request.body_json().unwrap();
                let ClientJsonRpcMessage::Request(req) = msg else {
                    return ResponseTemplate::new(202);
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
                        ResponseTemplate::new(200).set_body_json(&body)
                    }
                    "tools/call" => {
                        let result = CallToolResult::success(vec![
                            ContentBlock::text("here is the chart:"),
                            ContentBlock::image("base64data==", "image/png"),
                        ]);
                        let body = ServerJsonRpcMessage::response(
                            ServerResult::CallToolResult(result),
                            id,
                        );
                        ResponseTemplate::new(200).set_body_json(&body)
                    }
                    other => panic!("unexpected method: {other}"),
                }
            }
        }
        Mock::given(wiremock::matchers::any())
            .respond_with(ImageMock {
                seen: Arc::new(Mutex::new(Vec::new())),
            })
            .mount(&server)
            .await;

        let cfg = config(server.uri(), 5);
        let client = RmcpHttpClient::connect_for_test("image-server", &cfg)
            .await
            .expect("connect must succeed");
        let result = client
            .call_tool("render", json!({}))
            .await
            .expect("call_tool must succeed");

        assert!(
            result.content.contains("here is the chart:"),
            "the text block must survive losslessly: {}",
            result.content
        );
        assert!(
            result.content.contains("image/png") && result.content.contains("omitted"),
            "the image block must become a named, bounded placeholder, not vanish: {}",
            result.content
        );
        assert!(
            !result.content.contains("base64data=="),
            "the raw base64 payload must never be forwarded verbatim into the model's context"
        );
    }

    /// R2.6 — the placeholder's own bound. Every field it interpolates is
    /// SERVER-controlled, so "named" must not become "unbounded": a server
    /// handing back a megabyte-long uri or resource name would otherwise turn
    /// an elision into the very context flood the elision exists to prevent.
    #[test]
    fn a_hostile_server_cannot_make_a_placeholder_unbounded() {
        let flood = "A".repeat(64 * 1024);

        let cases = vec![
            ContentBlock::resource_link(rmcp::model::Resource::new(
                format!("https://example.com/{flood}"),
                flood.clone(),
            )),
            ContentBlock::image("QUJD", flood.clone()),
            // A text block is server-controlled too, and bounded the same way.
            ContentBlock::text(flood.clone()),
        ];

        for block in cases {
            let text = super::content_block_to_text(block);
            assert!(
                text.len() <= super::MCP_MAX_CONTENT_BLOCK_BYTES,
                "a server-controlled field escaped the ceiling: {} bytes",
                text.len()
            );
            assert!(
                text.contains("[truncated: showing first"),
                "an elided block must say it was elided, not silently shrink: {}",
                &text[..text.len().min(120)]
            );
        }
    }

    // Not covered here, by design: the malformed-initialize and 0/1/N-tools
    // protocol cases (R2.7), and the live network tests against real MCP
    // servers. Those ship in the next slice.
}
