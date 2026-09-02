//! Assembling every declared MCP server into one turn's tool set.
//!
//! Two halves. [`fold_catalog`] is pure — a catalog in, definitions and routes
//! out — and is where the naming, collision and routing rules live. [`wire`] is
//! the I/O around it: it resolves credentials, reaches every declared server,
//! and folds what comes back.
//!
//! **Degrading is the contract, not a fallback.** A third-party server that is
//! down must not take the agent with it: the operator declared MCP as one of
//! several capabilities and the other tools still work. So [`wire`] returns no
//! `Result` at all — a server that fails to bind or to answer `tools/list`
//! contributes no tools, its alias lands in `unavailable`, and the agent runs
//! on. Silence would be the wrong kind of degrading, though: a model that keeps
//! its old belief about what it can do will promise work it can no longer
//! perform, so [`unavailable_notice`] states the loss in the system message.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use futures::future::join_all;

use crate::dag_engine::application::secure_value_service::SecureValueService;
use crate::dag_engine::domain::tool_configuration::McpServerSpec;
use crate::dag_engine::infrastructure::mcp_registry::McpConnectionRegistry;
use crate::llm::domain::mcp::McpToolDescriptor;
use crate::llm::domain::tools::ToolDefinition;

use super::bind::{bind, McpBinding};
use super::expose::{drop_colliding, exposed_definitions};

/// One server's fan-out result: its binding and catalog, or why it dropped out.
type Fetched<'a> = (
    &'a String,
    Result<(McpBinding, Arc<Vec<McpToolDescriptor>>), String>,
);

/// One turn's worth of MCP wiring.
pub struct McpWiring {
    /// Tools the provider will see, already de-collided across every server.
    pub definitions: Vec<ToolDefinition>,
    /// Exposed name -> the server and tool a call must be routed back to.
    pub routes: BTreeMap<String, McpRoute>,
    /// Operator-facing warnings. Meant for the log, not for the model.
    pub notes: Vec<String>,
    /// Aliases whose server contributed nothing this turn.
    pub unavailable: Vec<String>,
    /// Bindings for the servers that DID answer, so dispatch can route a call
    /// back without resolving credentials a second time.
    pub bindings: BTreeMap<String, McpBinding>,
}

/// Bind, list and expose every declared server, degrading past the ones that
/// fail.
///
/// `claimed` must arrive holding every name Colmena has already given out, and
/// is EXTENDED per server — see [`fold_catalog`], whose contract depends on it.
///
/// Servers are folded in `BTreeMap` order, so which one wins a contested name is
/// deterministic across runs rather than a function of who answered first.
pub async fn wire(
    registry: &McpConnectionRegistry,
    specs: &BTreeMap<String, McpServerSpec>,
    claimed: &mut HashSet<String>,
    secure_values: Option<&SecureValueService>,
    session_id: &str,
    agent_session_id: Option<&str>,
) -> McpWiring {
    let mut out = McpWiring {
        definitions: Vec::new(),
        routes: BTreeMap::new(),
        notes: Vec::new(),
        unavailable: Vec::new(),
        bindings: BTreeMap::new(),
    };

    // Reach every server CONCURRENTLY. Sequentially, N servers that are slow but
    // alive add up: each is bounded only by its own `timeout_seconds` (default
    // 30) and nothing bounds the sum, so five of them could add over two minutes
    // to EVERY turn — this runs before the model is invoked. Fanned out, the
    // cost is the slowest single server rather than their total.
    //
    // Only the I/O is concurrent. The fold below stays sequential and in
    // `BTreeMap` order, because `claimed` is order-dependent: which server wins
    // a contested name must not depend on which one answered first.
    let fetched: Vec<Fetched> = join_all(specs.iter().map(|(alias, spec)| async move {
        let binding = match bind(alias, spec, secure_values, session_id, agent_session_id).await {
            Ok(b) => b,
            // Includes the credential refusals: a reference that did not resolve
            // is a configuration fault, but it must not be fatal here either, or
            // one stale secret takes down an agent whose other tools are fine.
            Err(e) => return (alias, Err(format!("could not be prepared: {e}"))),
        };
        match registry
            .tools(&binding.key, binding.config(), || binding.connect())
            .await
        {
            Ok(catalog) => (alias, Ok((binding, catalog))),
            Err(e) => (alias, Err(format!("did not list its tools: {e}"))),
        }
    }))
    .await;

    for (alias, result) in fetched {
        let (binding, catalog) = match result {
            Ok(pair) => pair,
            Err(why) => {
                out.notes.push(format!("MCP server '{alias}' {why}"));
                out.unavailable.push(alias.clone());
                continue;
            }
        };

        let folded = fold_catalog(alias, &catalog, claimed);
        out.notes.extend(folded.notes);
        out.routes.extend(folded.routes);
        out.definitions.extend(folded.definitions);
        out.bindings.insert(alias.clone(), binding);
    }

    out
}

/// What the model is told about servers that contributed nothing.
///
/// `None` when nothing is missing, so a healthy turn adds no tokens and leaves
/// the cached prompt prefix untouched — an empty string would still change it
/// and cost a cache write every turn.
///
/// Says only that the server is unavailable. It does NOT name the tools that are
/// missing: the catalog is exactly what could not be fetched, so any list would
/// be invented.
pub fn unavailable_notice(unavailable: &[String]) -> Option<String> {
    if unavailable.is_empty() {
        return None;
    }
    let names = unavailable
        .iter()
        .map(|a| format!("'{a}'"))
        .collect::<Vec<_>>()
        .join(", ");
    Some(format!(
        "Unavailable this turn: the MCP server(s) {names} did not respond, so none \
         of their tools can be called right now. Do not promise or attempt work \
         that depends on them; say plainly that the capability is unavailable, and \
         use your other tools where they suffice."
    ))
}

/// Where an exposed tool name came from.
///
/// The model calls `alias__tool`; the server only answers to its own name. This
/// carries the second so a call can be sent back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpRoute {
    /// The operator-chosen alias of the server that owns it.
    pub alias: String,
    /// The server's own tool name, verbatim — hyphens, dots and all.
    pub tool: String,
}

/// One server's catalog, folded.
pub struct Folded {
    /// Tools the provider will see, already de-collided.
    pub definitions: Vec<ToolDefinition>,
    /// Exposed name -> the server and tool a call must be routed back to.
    ///
    /// Built here rather than reversed at dispatch time, because [`normalize`]
    /// is lossy: `foo.bar` and `foo/bar` both become `alias__foo_bar`, so the
    /// exposed name does not determine the original.
    pub routes: BTreeMap<String, McpRoute>,
    /// Operator-facing warnings: tools skipped, names already claimed. Meant
    /// for the log, not for the model.
    pub notes: Vec<String>,
}

/// Expose one server's catalog, dropping whatever cannot be exposed safely.
///
/// `claimed` carries the names Colmena has already given out and is EXTENDED
/// with this server's survivors. That is what stops a second server from
/// displacing the first: without it, [`drop_colliding`] would only ever compare
/// against built-ins, and two servers exposing the same tool name would both
/// reach the provider — which Gemini rejects outright.
///
/// The caller is responsible for calling this with EVERY name Colmena has
/// already claimed. `drop_colliding`'s contract is that an MCP tool always
/// loses a contested name, and that is only true if `claimed` is complete.
pub fn fold_catalog(
    alias: &str,
    catalog: &[McpToolDescriptor],
    claimed: &mut HashSet<String>,
) -> Folded {
    let (defs, origins, skipped) = exposed_definitions(alias, catalog);
    let (kept, collisions) = drop_colliding(defs, claimed);

    let mut notes = skipped;
    notes.extend(collisions);

    let mut routes = BTreeMap::new();
    for def in &kept {
        claimed.insert(def.name.clone());
        // Ask, do not re-derive. An earlier attempt matched `def.name` against
        // the catalog with `normalize` and took the first hit, which is WRONG:
        // `exposed_definitions` drops an oversized schema before it claims the
        // name, so a later tool normalising the same way becomes the exposed
        // one — and the name match would find the dropped tool first. The model
        // would be shown one tool's schema while its calls went to another.
        // Only the loop that built the definition knows which descriptor
        // survived, so it hands the answer over instead.
        if let Some(tool) = origins.get(&def.name) {
            routes.insert(
                def.name.clone(),
                McpRoute {
                    alias: alias.to_string(),
                    tool: tool.clone(),
                },
            );
        }
    }

    Folded {
        definitions: kept,
        routes,
        notes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::domain::mcp::normalize;
    use serde_json::json;

    fn descriptor(name: &str) -> McpToolDescriptor {
        McpToolDescriptor {
            name: name.to_string(),
            title: None,
            description: "does a thing".to_string(),
            input_schema: json!({ "type": "object" }),
        }
    }

    /// A server that cannot be reached must cost the agent its tools, not its
    /// run. `wire` has no error path by design, so this is the whole contract.
    #[tokio::test]
    async fn an_unreachable_server_degrades_instead_of_failing() {
        let registry = McpConnectionRegistry::new();
        let mut specs = BTreeMap::new();
        // Reserved by RFC 6761: guaranteed not to resolve, so this exercises a
        // real connection failure rather than a mocked one. The 1s timeout
        // matters — with the 30s default this would hang for half a minute in a
        // sandbox whose resolver blocks instead of erroring.
        specs.insert(
            "dead".to_string(),
            serde_json::from_value(json!({
                "url": "https://invalid./mcp",
                "timeout_seconds": 1
            }))
            .expect("spec parses"),
        );
        let mut claimed = HashSet::new();

        let w = wire(&registry, &specs, &mut claimed, None, "s1", None).await;

        assert!(w.definitions.is_empty(), "a dead server exposes no tools");
        assert_eq!(w.unavailable, vec!["dead".to_string()]);
        assert!(
            w.bindings.is_empty(),
            "a server that never answered must not be dispatchable"
        );
        assert!(!w.notes.is_empty(), "the operator must be told why");
    }

    /// A credential that does not resolve is an operator fault, but it must
    /// degrade like any other failure rather than killing the node.
    #[tokio::test]
    async fn an_unresolvable_credential_degrades_rather_than_failing() {
        let registry = McpConnectionRegistry::new();
        let mut specs = BTreeMap::new();
        specs.insert(
            "needs-secret".to_string(),
            serde_json::from_value(json!({
                "url": "https://mcp.example.com/mcp",
                "headers": { "Authorization": "<sv_missing>" }
            }))
            .expect("spec parses"),
        );
        let mut claimed = HashSet::new();

        // No secure-value service: `bind` refuses the reference.
        let w = wire(&registry, &specs, &mut claimed, None, "s1", None).await;

        assert_eq!(w.unavailable, vec!["needs-secret".to_string()]);
        assert!(w.definitions.is_empty());
        let joined = w.notes.join(" ");
        assert!(
            joined.contains("Authorization"),
            "the note must name the header so an operator can act: {joined}"
        );
        assert!(
            !joined.contains("<sv_missing>"),
            "the note must not echo the reference: {joined}"
        );
    }

    /// The model has to learn it lost a capability, or it will keep promising
    /// work it can no longer do.
    #[test]
    fn the_notice_names_every_unavailable_server() {
        let n = unavailable_notice(&["alpha".to_string(), "beta".to_string()])
            .expect("a notice is produced when something is missing");
        assert!(n.contains("'alpha'"), "missing alpha: {n}");
        assert!(n.contains("'beta'"), "missing beta: {n}");
    }

    /// A healthy turn must add nothing at all — an empty string would still
    /// change the prompt prefix and cost a cache write every turn.
    #[test]
    fn a_healthy_turn_adds_no_notice() {
        assert!(unavailable_notice(&[]).is_none());
    }

    /// The route is what makes a call routable at all. `normalize` is lossy, so
    /// a hyphenated server name must still lead back to its verbatim original.
    #[test]
    fn a_route_leads_back_to_the_servers_own_tool_name() {
        let catalog = vec![descriptor("resolve-library-id")];
        let mut claimed = HashSet::new();

        let f = fold_catalog("ctx7", &catalog, &mut claimed);

        assert_eq!(f.definitions.len(), 1);
        let exposed = f.definitions[0].name.clone();
        assert_ne!(
            exposed, "resolve-library-id",
            "the exposed name is normalised, so the route cannot be a no-op"
        );
        assert_eq!(
            f.routes.get(&exposed),
            Some(&McpRoute {
                alias: "ctx7".to_string(),
                tool: "resolve-library-id".to_string(),
            }),
            "the route must carry the verbatim server name, not the exposed one"
        );
    }

    /// Two names that normalise alike: only one is exposed, and its route must
    /// point at the one that actually won, not at its twin.
    #[test]
    fn a_dropped_twin_leaves_no_route_behind() {
        let catalog = vec![descriptor("foo.bar"), descriptor("foo/bar")];
        let mut claimed = HashSet::new();

        let f = fold_catalog("srv", &catalog, &mut claimed);

        assert_eq!(f.definitions.len(), 1, "the twin must not be exposed");
        assert_eq!(f.routes.len(), 1, "and it must not leave a route");
        let r = f.routes.values().next().expect("one route");
        assert_eq!(
            r.tool, "foo.bar",
            "the FIRST of the two is what exposure kept"
        );
    }

    /// A second server must not take a name the first already holds, and the
    /// loser must not leave a route pointing at a tool the model cannot see.
    #[test]
    fn a_second_server_cannot_displace_the_first() {
        let mut claimed = HashSet::new();
        let a = fold_catalog("srv", &[descriptor("search")], &mut claimed);
        let b = fold_catalog("srv", &[descriptor("search")], &mut claimed);

        assert_eq!(a.definitions.len(), 1);
        assert!(
            b.definitions.is_empty(),
            "the contested name stays with the first claimant"
        );
        assert!(b.routes.is_empty(), "a dropped tool must leave no route");
        assert!(!b.notes.is_empty(), "the operator must be told");
    }

    fn descriptor_with_schema(name: &str, schema: serde_json::Value) -> McpToolDescriptor {
        McpToolDescriptor {
            name: name.to_string(),
            title: None,
            description: format!("tool {name}"),
            input_schema: schema,
        }
    }

    /// The route must point at the tool that PRODUCED the definition, not at
    /// any tool sharing its normalised name.
    ///
    /// `exposed_definitions` drops an oversized schema BEFORE it claims the
    /// name, so a later tool normalising the same way becomes the exposed one.
    /// Matching on the name alone then finds the earlier, dropped tool — and the
    /// model would be shown one tool's schema while its calls went to another.
    #[test]
    fn a_route_never_points_at_a_tool_that_was_dropped() {
        let huge = serde_json::json!({
            "type": "object",
            "properties": { "pad": { "type": "string", "description": "x".repeat(40 * 1024) } }
        });
        let catalog = vec![
            descriptor_with_schema("foo.bar", huge),
            descriptor_with_schema("foo/bar", serde_json::json!({ "type": "object" })),
        ];
        let mut claimed = HashSet::new();

        let f = fold_catalog("srv", &catalog, &mut claimed);

        assert_eq!(
            f.definitions.len(),
            1,
            "only the small-schema tool is exposed"
        );
        let exposed = f.definitions[0].name.clone();
        assert_eq!(
            f.routes.get(&exposed).map(|r| r.tool.as_str()),
            Some("foo/bar"),
            "the route must name the tool that was actually exposed"
        );
    }

    /// A built-in must win too — that is the containment boundary, not a
    /// tie-break, so it cannot depend on MCP servers being folded in any
    /// particular order relative to each other.
    #[test]
    fn a_built_in_name_is_never_taken() {
        let mut claimed = HashSet::new();
        let taken = normalize("srv", "search");
        claimed.insert(taken.clone());

        let f = fold_catalog("srv", &[descriptor("search")], &mut claimed);

        assert!(
            f.definitions.is_empty(),
            "MCP took a name Colmena had already claimed"
        );
        assert!(f.routes.is_empty(), "and it left a route behind");
    }
}
