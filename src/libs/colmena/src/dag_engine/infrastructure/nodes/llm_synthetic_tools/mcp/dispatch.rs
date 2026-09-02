//! Routing a model's tool call back to the server that owns it.
//!
//! The return trip for [`super::wire`]: the model calls an exposed name, this
//! finds the server behind it, forwards the arguments verbatim, and hands the
//! result back through [`super::contain`].
//!
//! **A failure here is a tool error, never a node error.** The model asked for
//! something and must be told it did not work, in a form it can react to — the
//! same reason a `python_script` returns its traceback instead of killing the
//! run. Failing the `llm_call` would also throw away the turn's other tool
//! results, which have nothing to do with this server.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::Value;

use crate::dag_engine::infrastructure::mcp_registry::McpConnectionRegistry;
use crate::llm::domain::mcp::McpClientPort;

use super::bind::McpBinding;
use super::contain::{contain, nonce_for};
use super::wire::McpRoute;

/// Everything needed to route one turn's MCP calls.
pub struct McpDispatcher {
    registry: Arc<McpConnectionRegistry>,
    routes: BTreeMap<String, McpRoute>,
    bindings: BTreeMap<String, McpBinding>,
}

impl McpDispatcher {
    pub fn new(
        registry: Arc<McpConnectionRegistry>,
        routes: BTreeMap<String, McpRoute>,
        bindings: BTreeMap<String, McpBinding>,
    ) -> Self {
        Self {
            registry,
            routes,
            bindings,
        }
    }

    /// Whether this exposed name belongs to an MCP server.
    ///
    /// A membership test against the routes built at exposure, NOT a guess from
    /// the name's shape. A built-in tool is free to contain `__`, and inferring
    /// ownership from the string would let MCP hijack its dispatch.
    ///
    /// Whoever wires this into the executor must consult it BEFORE the built-in
    /// branches, so that a name MCP does not own falls through to them untouched.
    /// Stated as a requirement rather than as fact: no executor calls this yet.
    pub fn owns(&self, exposed_name: &str) -> bool {
        self.routes.contains_key(exposed_name)
    }

    /// Call the server behind `exposed_name` and return content fit for the
    /// model.
    ///
    /// Every return path goes through [`contain`], including the ones that never
    /// reach the network: a caller cannot tell from the outside which failed,
    /// and an uncontained string here would be the one hole in the fence.
    ///
    /// `tool_call_id` seeds the delimiter nonce, so every result in a turn is
    /// fenced with a different token and content copied out of one result cannot
    /// forge the closing marker of another.
    pub async fn call(&self, exposed_name: &str, arguments: Value, tool_call_id: &str) -> String {
        let nonce = nonce_for(tool_call_id);

        let Some(route) = self.routes.get(exposed_name) else {
            // Unreachable through `owns`, but a caller that skipped it must not
            // get silence — and must not get an unfenced string either.
            return contain(
                "colmena",
                exposed_name,
                &nonce,
                "this tool is not an MCP tool",
                true,
            );
        };
        let Some(binding) = self.bindings.get(&route.alias) else {
            return contain(
                &route.alias,
                &route.tool,
                &nonce,
                "this server is not connected in this run, so the tool cannot be called",
                true,
            );
        };

        let client = match self
            .registry
            .client(&binding.key, || binding.connect())
            .await
        {
            Ok(c) => c,
            Err(e) => {
                return contain(
                    &route.alias,
                    &route.tool,
                    &nonce,
                    &format!("could not reach the server: {e}"),
                    true,
                )
            }
        };

        call_and_contain(
            client.as_ref(),
            &route.alias,
            &route.tool,
            &nonce,
            arguments,
        )
        .await
    }
}

/// The half of [`McpDispatcher::call`] that runs once a client exists.
///
/// Split out for one reason: taking `&dyn McpClientPort` gives tests a seam.
/// Without it the three branches that carry every REAL result — a transport
/// failure, a success, and a server-reported error — are reachable only through
/// a live connection, and a mutation inverting the success/error flag (which
/// selects the truncation ceiling for third-party content) would pass the whole
/// suite.
async fn call_and_contain(
    client: &dyn McpClientPort,
    alias: &str,
    tool: &str,
    nonce: &str,
    arguments: Value,
) -> String {
    match client.call_tool(tool, arguments).await {
        // `is_error` is the server saying the CALL failed, not the transport. It
        // is still server-authored text, so it is contained exactly like a
        // success — an error message is a perfectly good place to hide an
        // injection. The flag only picks which ceiling bounds the body.
        Ok(result) => contain(alias, tool, nonce, &result.content, result.is_error),
        Err(e) => contain(alias, tool, nonce, &format!("the call failed: {e}"), true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dispatcher(routes: &[(&str, &str, &str)]) -> McpDispatcher {
        let map = routes
            .iter()
            .map(|(exposed, alias, tool)| {
                (
                    exposed.to_string(),
                    McpRoute {
                        alias: alias.to_string(),
                        tool: tool.to_string(),
                    },
                )
            })
            .collect();
        McpDispatcher::new(Arc::new(McpConnectionRegistry::new()), map, BTreeMap::new())
    }

    use crate::llm::domain::mcp::{
        McpError, McpToolDescriptor, McpToolResult, MCP_MAX_ERROR_BYTES, MCP_MAX_RESULT_BYTES,
    };
    use async_trait::async_trait;

    /// A server that answers however the test needs it to.
    struct FakeServer {
        answer: Result<McpToolResult, McpError>,
        seen: std::sync::Mutex<Option<Value>>,
    }

    #[async_trait]
    impl McpClientPort for FakeServer {
        async fn list_tools(&self) -> Result<Vec<McpToolDescriptor>, McpError> {
            Ok(Vec::new())
        }
        async fn call_tool(
            &self,
            _name: &str,
            arguments: Value,
        ) -> Result<McpToolResult, McpError> {
            *self.seen.lock().expect("not poisoned") = Some(arguments);
            match &self.answer {
                Ok(r) => Ok(r.clone()),
                Err(e) => Err(e.clone()),
            }
        }
        fn server_label(&self) -> &str {
            "fake"
        }
    }

    /// Deliberately NOT the same pair everywhere. Every test used to pass the
    /// identical `"srv"`/`"thing"`, so a `call_and_contain` that ignored its
    /// arguments and hardcoded those two values passed the whole suite.
    const OTHER_ALIAS: &str = "docs-mirror";
    const OTHER_TOOL: &str = "fetch-page";

    fn answering(content: &str, is_error: bool) -> FakeServer {
        FakeServer {
            answer: Ok(McpToolResult {
                content: content.to_string(),
                is_error,
            }),
            seen: std::sync::Mutex::new(None),
        }
    }

    /// A real result must come back fenced, with the server's content inside.
    #[tokio::test]
    async fn a_successful_result_is_returned_inside_the_fence() {
        let out = call_and_contain(
            &answering("the answer", false),
            "srv",
            "thing",
            "abcd1234",
            serde_json::json!({}),
        )
        .await;

        assert!(out.contains("the answer"), "content missing: {out}");
        assert!(out.ends_with("<<<END_UNTRUSTED_MCP id=abcd1234>>>"));
    }

    /// The flag picks which ceiling bounds third-party text. Inverting it would
    /// apply the wrong one, and until this test existed nothing noticed.
    #[tokio::test]
    async fn the_error_flag_selects_the_error_ceiling() {
        let huge = "e".repeat(MCP_MAX_RESULT_BYTES * 2);

        let as_error = call_and_contain(
            &answering(&huge, true),
            "srv",
            "t",
            "n1",
            serde_json::json!({}),
        )
        .await;
        let as_success = call_and_contain(
            &answering(&huge, false),
            "srv",
            "t",
            "n1",
            serde_json::json!({}),
        )
        .await;

        assert!(
            as_error.len() < MCP_MAX_ERROR_BYTES + 512,
            "a server-reported error was not bounded by the error ceiling: {}",
            as_error.len()
        );
        assert!(
            as_success.len() > MCP_MAX_RESULT_BYTES - 2048,
            "a success was bounded by something stricter than the result ceiling: {}",
            as_success.len()
        );
    }

    /// The model's arguments are the whole point of the call. Nothing asserted
    /// they arrived, so dropping or replacing them survived the suite.
    #[tokio::test]
    async fn the_models_arguments_reach_the_server_unchanged() {
        let server = answering("ok", false);
        let args = serde_json::json!({ "library": "serde", "topic": "derive" });

        call_and_contain(&server, "srv", "thing", "abcd1234", args.clone()).await;

        let seen = server.seen.lock().expect("not poisoned").clone();
        assert_eq!(seen, Some(args), "the server did not receive the arguments");
    }

    /// The framing names the server and the tool in that order. Nothing pinned
    /// the positions, so swapping the two arguments survived the suite — and the
    /// operator-chosen alias and the third-party tool name would trade places in
    /// a sentence Colmena speaks in its own voice.
    #[tokio::test]
    async fn the_framing_names_the_server_and_the_tool_in_their_own_slots() {
        let out = call_and_contain(
            &answering("ok", false),
            "srv",
            "thing",
            "abcd1234",
            serde_json::json!({}),
        )
        .await;

        assert!(
            out.contains(r#"MCP server "srv", tool "thing""#),
            "alias and tool are not in their own slots: {out}"
        );

        // A second, different pair. Without it the property holds only for the
        // one pair every other test happens to use, and a hardcoded
        // `contain("srv", "thing", ...)` would satisfy the assertion above.
        let other = call_and_contain(
            &answering("ok", false),
            OTHER_ALIAS,
            OTHER_TOOL,
            "abcd1234",
            serde_json::json!({}),
        )
        .await;
        assert!(
            other.contains(&format!(
                r#"MCP server "{OTHER_ALIAS}", tool "{OTHER_TOOL}""#
            )),
            "the names are not forwarded, only the default pair works: {other}"
        );
    }

    /// A transport failure is the model's problem to react to, not the node's,
    /// and it is server-adjacent text so it is fenced too.
    #[tokio::test]
    async fn a_transport_failure_comes_back_contained() {
        let broken = FakeServer {
            answer: Err(McpError::InvalidConfig {
                detail: "socket closed".to_string(),
            }),
            seen: std::sync::Mutex::new(None),
        };

        let out =
            call_and_contain(&broken, "srv", "thing", "abcd1234", serde_json::json!({})).await;

        assert!(out.contains("the call failed"), "reason missing: {out}");
        assert!(out.ends_with("<<<END_UNTRUSTED_MCP id=abcd1234>>>"));
    }

    /// The lookup that gates the network step, and the network step itself.
    ///
    /// `bind` does NOT connect — it only resolves credentials and derives the
    /// pool identity — so a binding can be built without a server and the
    /// connect failure exercised for real. Every other test leaves `bindings`
    /// empty and returns at the earlier branch, which left both the lookup by
    /// `route.alias` and the connect arm unproven.
    #[tokio::test]
    async fn a_bound_server_is_looked_up_by_alias_and_its_connect_failure_is_contained() {
        let spec = serde_json::from_value(serde_json::json!({
            // Reserved by RFC 6761: guaranteed not to resolve. The 1s timeout
            // keeps this fast in a sandbox whose resolver blocks.
            "url": "https://invalid./mcp",
            "timeout_seconds": 1
        }))
        .expect("spec parses");
        let binding = super::super::bind::bind("srv", &spec, None, "s1", None)
            .await
            .expect("bind resolves without connecting");

        let mut bindings = BTreeMap::new();
        bindings.insert("srv".to_string(), binding);
        let routes = [(
            "srv__thing".to_string(),
            McpRoute {
                alias: "srv".to_string(),
                tool: "thing".to_string(),
            },
        )]
        .into_iter()
        .collect();
        let d = McpDispatcher::new(Arc::new(McpConnectionRegistry::new()), routes, bindings);

        let out = d.call("srv__thing", serde_json::json!({}), "call_7").await;

        assert!(
            out.contains("could not reach the server"),
            "the lookup did not reach the connect step: {out}"
        );
        assert!(
            !out.contains("not connected"),
            "the binding was not found by its alias: {out}"
        );
        assert!(
            out.ends_with(&format!(
                "<<<END_UNTRUSTED_MCP id={}>>>",
                nonce_for("call_7")
            )),
            "the connect-failure path returned an unfenced string: {out}"
        );
    }

    /// Ownership is membership, not string shape. A built-in may contain `__`,
    /// and inferring ownership from the name would let MCP steal its dispatch.
    #[test]
    fn ownership_is_decided_by_the_route_table_not_by_the_name() {
        let d = dispatcher(&[("ctx7__resolve_library_id", "ctx7", "resolve-library-id")]);

        assert!(d.owns("ctx7__resolve_library_id"));
        assert!(
            !d.owns("gsheets__run_python"),
            "a built-in shaped like an MCP name must not be claimed"
        );
        assert!(!d.owns("ctx7__never_exposed"));
    }

    /// A server that is not connected must produce something the model can act
    /// on — and it must still be fenced, because this path returns a string the
    /// model reads just like any other tool result.
    #[tokio::test]
    async fn an_unconnected_server_returns_a_contained_error() {
        let d = dispatcher(&[("srv__thing", "srv", "thing")]);

        let out = d.call("srv__thing", serde_json::json!({}), "call_1").await;

        let nonce = nonce_for("call_1");
        assert!(
            out.ends_with(&format!("<<<END_UNTRUSTED_MCP id={nonce}>>>")),
            "the failure path returned an unfenced string: {out}"
        );
        assert!(
            out.contains("not connected"),
            "the reason is missing: {out}"
        );
    }

    /// The same for a name that is not routed at all: unreachable through
    /// `owns`, but it must not be the one hole in the fence.
    #[tokio::test]
    async fn an_unrouted_name_is_still_contained() {
        let d = dispatcher(&[("srv__thing", "srv", "thing")]);

        let out = d
            .call("srv__missing", serde_json::json!({}), "call_9")
            .await;

        let nonce = nonce_for("call_9");
        assert!(
            out.ends_with(&format!("<<<END_UNTRUSTED_MCP id={nonce}>>>")),
            "the unrouted path returned an unfenced string: {out}"
        );
    }

    /// Two calls in one turn must not share a fence, or content copied out of
    /// one result could close the other's block.
    #[tokio::test]
    async fn two_calls_in_one_turn_get_different_fences() {
        let d = dispatcher(&[("srv__thing", "srv", "thing")]);

        let a = d.call("srv__thing", serde_json::json!({}), "call_a").await;
        let b = d.call("srv__thing", serde_json::json!({}), "call_b").await;

        assert_ne!(
            nonce_for("call_a"),
            nonce_for("call_b"),
            "the fixture ids must differ or this proves nothing"
        );
        assert!(!a.ends_with(&format!(
            "<<<END_UNTRUSTED_MCP id={}>>>",
            nonce_for("call_b")
        )));
        assert!(!b.ends_with(&format!(
            "<<<END_UNTRUSTED_MCP id={}>>>",
            nonce_for("call_a")
        )));
    }
}
