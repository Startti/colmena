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

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use futures::future::join_all;

use crate::dag_engine::application::secure_value_service::SecureValueService;
use crate::dag_engine::domain::tool_configuration::McpServerSpec;
use crate::dag_engine::infrastructure::mcp_registry::McpConnectionRegistry;
use crate::llm::domain::mcp::McpToolDescriptor;
use crate::llm::domain::tools::ToolDefinition;

use super::allowlist::{allowed_hosts_from_env, host_for_log, url_is_allowed};
use super::bind::{bind, McpBinding};
use super::expose::{drop_colliding, exposed_definitions};

/// Why a server dropped out of a turn. A stable label for the log, distinct
/// from the human-facing note text (which is free-form and may change).
#[derive(Clone, Copy)]
enum FetchFailure {
    /// Credentials did not resolve, or the binding could not be built.
    Prepare,
    /// The binding was fine, but `tools/list` failed.
    ToolsList,
    /// The fan-out produced no entry at all for this alias.
    NoResult,
    /// The server's host is not in `COLMENA_MCP_ALLOWED_HOSTS`. Checked
    /// BEFORE `bind`, so a refused host never causes a credential to be
    /// resolved or decrypted.
    NotAllowed,
}

impl FetchFailure {
    fn label(self) -> &'static str {
        match self {
            Self::Prepare => "prepare",
            Self::ToolsList => "tools_list",
            Self::NoResult => "no_result",
            Self::NotAllowed => "not_allowed",
        }
    }
}

/// One server's fan-out result: its binding and catalog, or why it dropped
/// out — alongside how long the attempt took, for the log.
type Fetched<'a> = (
    &'a String,
    u64,
    Result<(McpBinding, Arc<Vec<McpToolDescriptor>>), (FetchFailure, String)>,
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
/// deterministic across runs rather than a function of who answered first. See
/// [`assemble`], which is where that order actually comes from.
pub async fn wire(
    registry: &McpConnectionRegistry,
    specs: &BTreeMap<String, McpServerSpec>,
    claimed: &mut HashSet<String>,
    secure_values: Option<&SecureValueService>,
    session_id: &str,
    agent_session_id: Option<&str>,
) -> McpWiring {
    // Reach every server CONCURRENTLY. Sequentially, N servers that are slow but
    // alive add up: each is bounded only by its own `timeout_seconds` (default
    // 30) and nothing bounds the sum, so five of them could add over two minutes
    // to EVERY turn — this runs before the model is invoked. Fanned out, the
    // cost is the slowest single server rather than their total.
    //
    // Only the I/O is concurrent. `assemble` folds what comes back sequentially
    // and in `BTreeMap` order, and takes that order from `specs` rather than
    // from this vector — so nothing here needs to preserve it.
    let allowed_hosts = allowed_hosts_from_env();

    let allowed_hosts = &allowed_hosts;
    let fetched: Vec<Fetched> = join_all(specs.iter().map(|(alias, spec)| async move {
        let started = std::time::Instant::now();

        // Checked BEFORE `bind`, deliberately: a refused host must not cause
        // credentials to be resolved or decrypted. An empty allowlist (unset
        // or blank env var) allows everything, so this is a no-op unless an
        // operator opted in.
        if !url_is_allowed(&spec.url, allowed_hosts) {
            let ms = started.elapsed().as_millis() as u64;
            return (
                alias,
                ms,
                Err((
                    FetchFailure::NotAllowed,
                    "was not contacted: its host is not in COLMENA_MCP_ALLOWED_HOSTS".to_string(),
                )),
            );
        }

        let binding = match bind(alias, spec, secure_values, session_id, agent_session_id).await {
            Ok(b) => b,
            // Includes the credential refusals: a reference that did not resolve
            // is a configuration fault, but it must not be fatal here either, or
            // one stale secret takes down an agent whose other tools are fine.
            Err(e) => {
                let ms = started.elapsed().as_millis() as u64;
                return (
                    alias,
                    ms,
                    Err((FetchFailure::Prepare, format!("could not be prepared: {e}"))),
                );
            }
        };
        let result = registry
            .tools(&binding.key, binding.config(), || binding.connect())
            .await;
        let ms = started.elapsed().as_millis() as u64;
        match result {
            Ok(catalog) => (alias, ms, Ok((binding, catalog))),
            Err(e) => (
                alias,
                ms,
                Err((
                    FetchFailure::ToolsList,
                    format!("did not list its tools: {e}"),
                )),
            ),
        }
    }))
    .await;

    assemble(specs, fetched, claimed)
}

/// Fold what the fan-out brought back into one turn's wiring.
///
/// Split out from [`wire`] so the ordering rule below can be tested without a
/// live server on either end.
///
/// **The fold iterates `specs`, not `fetched`, and that is the whole point.**
/// `claimed` is threaded through [`fold_catalog`], so fold order decides which
/// server wins a contested tool name — it has to be `BTreeMap` order, never the
/// order servers happened to answer in. Reading the results in arrival order
/// would be correct today only because `join_all` resolves to its inputs'
/// order; making the `BTreeMap` the loop itself means no future swap of the
/// combinator (`FuturesUnordered` being the obvious one) can quietly hand the
/// decision to network timing.
fn assemble(
    specs: &BTreeMap<String, McpServerSpec>,
    fetched: Vec<Fetched>,
    claimed: &mut HashSet<String>,
) -> McpWiring {
    let mut out = McpWiring {
        definitions: Vec::new(),
        routes: BTreeMap::new(),
        notes: Vec::new(),
        unavailable: Vec::new(),
        bindings: BTreeMap::new(),
    };

    let mut by_alias: HashMap<&String, _> =
        fetched.into_iter().map(|(a, ms, r)| (a, (ms, r))).collect();

    for alias in specs.keys() {
        let host = specs
            .get(alias)
            .map_or_else(|| "<unparseable>".to_string(), |s| host_for_log(&s.url));

        // Every alias has an entry — one future was spawned per spec key — so
        // this is unreachable from `wire`. Taken rather than unwrapped anyway,
        // because a panic here would cost the whole turn over a condition that
        // cannot happen; and reported rather than skipped, because a caller
        // that hands over a partial vector must degrade like an unreachable
        // server, not in silence. A dropped alias the model is never told about
        // leaves it promising work it can no longer do.
        let Some((ms, result)) = by_alias.remove(alias) else {
            out.notes
                .push(format!("MCP server '{alias}' returned no result"));
            out.unavailable.push(alias.clone());
            tracing::warn!(
                target: "colmena::mcp",
                event = "mcp.server_unavailable",
                alias = %alias,
                host = %host,
                reason = FetchFailure::NoResult.label(),
                "an MCP server contributed no tools this turn"
            );
            continue;
        };
        let (binding, catalog) = match result {
            Ok(pair) => pair,
            Err((failure, why)) => {
                out.notes.push(format!("MCP server '{alias}' {why}"));
                out.unavailable.push(alias.clone());
                tracing::warn!(
                    target: "colmena::mcp",
                    event = "mcp.server_unavailable",
                    alias = %alias,
                    host = %host,
                    reason = failure.label(),
                    ms = ms,
                    "an MCP server contributed no tools this turn"
                );
                continue;
            }
        };

        let folded = fold_catalog(alias, &catalog, claimed);
        let tools = folded.definitions.len();
        out.notes.extend(folded.notes);
        out.routes.extend(folded.routes);
        out.definitions.extend(folded.definitions);
        out.bindings.insert(alias.clone(), binding);
        tracing::debug!(
            target: "colmena::mcp",
            event = "mcp.server_ready",
            alias = %alias,
            host = %host,
            tools = tools,
            ms = ms,
            "an MCP server answered and its tools were exposed"
        );
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

    /// The fold order must be the `BTreeMap`'s, NOT the order servers answered
    /// in — otherwise which server owns a contested tool name is decided by
    /// network timing, and two runs of the same graph expose different tools.
    ///
    /// Two aliases are needed that collide despite `normalize` prefixing the
    /// alias: `normalize` maps every non-`[A-Za-z0-9_-]` character to `_`, so
    /// `a.b` and `a_b` both expose `a_b__search`. `a.b` sorts FIRST (`.` is
    /// 0x2E, `_` is 0x5F), so it must win — and the fetched vector below is
    /// deliberately handed over in the opposite order, which is exactly the
    /// state a `FuturesUnordered` would produce if `a_b` answered first.
    #[tokio::test]
    async fn the_first_alias_in_btreemap_order_wins_a_contested_name() {
        let spec: McpServerSpec = serde_json::from_value(json!({
            "url": "https://mcp.example.com/mcp"
        }))
        .expect("spec parses");

        let mut specs = BTreeMap::new();
        specs.insert("a.b".to_string(), spec.clone());
        specs.insert("a_b".to_string(), spec.clone());

        // `bind` only resolves headers and derives a pool key — there are none
        // here and it never opens a connection, so both bindings are real.
        let dotted = bind("a.b", &spec, None, "s1", None)
            .await
            .expect("binding a.b needs no credentials");
        let underscored = bind("a_b", &spec, None, "s1", None)
            .await
            .expect("binding a_b needs no credentials");

        let catalog = Arc::new(vec![descriptor("search")]);
        let (dotted_alias, underscored_alias) = {
            let mut keys = specs.keys();
            (
                keys.next().expect("two aliases"),
                keys.next().expect("two aliases"),
            )
        };
        assert_eq!(dotted_alias, "a.b", "the sort order this test rests on");

        // Arrival order: the LATER-sorting alias first.
        let fetched: Vec<Fetched> = vec![
            (underscored_alias, 0, Ok((underscored, catalog.clone()))),
            (dotted_alias, 0, Ok((dotted, catalog.clone()))),
        ];

        let mut claimed = HashSet::new();
        let w = assemble(&specs, fetched, &mut claimed);

        assert_eq!(
            w.definitions.len(),
            1,
            "both servers normalise to one name, so only one tool is exposed"
        );
        assert_eq!(w.definitions[0].name, "a_b__search");
        assert_eq!(
            w.routes.get("a_b__search").map(|r| r.alias.as_str()),
            Some("a.b"),
            "the contested name must go to the first alias in BTreeMap order, \
             not to the server that answered first"
        );
    }

    /// An alias with no fetch result must be REPORTED, not skipped in silence.
    ///
    /// Unreachable from [`wire`], which spawns one future per spec key. It is
    /// reachable for any other caller of [`assemble`], and the module's contract
    /// is that a server contributing nothing still lands in `unavailable` — a
    /// model never told it lost a capability goes on promising work it cannot
    /// do. Without this test the branch is a behaviour claim nothing checks.
    #[tokio::test]
    async fn an_alias_with_no_result_is_reported_rather_than_dropped() {
        let spec: McpServerSpec = serde_json::from_value(json!({
            "url": "https://mcp.example.com/mcp"
        }))
        .expect("spec parses");

        let mut specs = BTreeMap::new();
        specs.insert("present".to_string(), spec.clone());
        specs.insert("missing".to_string(), spec.clone());

        let binding = bind("present", &spec, None, "s1", None)
            .await
            .expect("binding needs no credentials");
        let present_alias = specs.keys().find(|a| *a == "present").expect("present");

        // Deliberately partial: nothing at all for the "missing" alias.
        let fetched: Vec<Fetched> = vec![(
            present_alias,
            0,
            Ok((binding, Arc::new(vec![descriptor("search")]))),
        )];

        let mut claimed = HashSet::new();
        let w = assemble(&specs, fetched, &mut claimed);

        assert_eq!(
            w.unavailable,
            vec!["missing".to_string()],
            "the alias that produced nothing must reach the model's notice"
        );
        assert!(
            w.notes.iter().any(|n| n.contains("missing")),
            "and the operator must be told: {:?}",
            w.notes
        );
        assert_eq!(
            w.definitions.len(),
            1,
            "the healthy server still contributes its tool"
        );
        assert!(
            w.bindings.contains_key("present") && !w.bindings.contains_key("missing"),
            "only the server that answered is dispatchable"
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

    /// The label is what a dashboard or alert keys on — it must not silently
    /// change shape.
    #[test]
    fn fetch_failure_labels_are_stable() {
        assert_eq!(FetchFailure::Prepare.label(), "prepare");
        assert_eq!(FetchFailure::ToolsList.label(), "tools_list");
        assert_eq!(FetchFailure::NoResult.label(), "no_result");
        assert_eq!(FetchFailure::NotAllowed.label(), "not_allowed");
    }
}
