//! Folding one MCP server's catalog into tools the model can see.
//!
//! The pure half of assembly: no network, no credentials, no pool. Given a
//! server's catalog and the names Colmena has already handed out, it produces
//! the tool definitions that survive, the routes needed to send a call back,
//! and the notes an operator needs to understand what was dropped.
//!
//! Split from the I/O that fetches the catalog, and not only for review size:
//! the fetching half builds its own connection from a binding, so there is no
//! seam to hand it a fake server. Everything below would otherwise be reachable
//! only through a live network call.

use std::collections::{BTreeMap, HashSet};

use crate::llm::domain::mcp::McpToolDescriptor;
use crate::llm::domain::tools::ToolDefinition;

use super::expose::{drop_colliding, exposed_definitions};

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
