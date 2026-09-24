//! The structure a suspended run depends on, and how two versions of it differ.
//!
//! A suspended child resumes with the graph its source names *now* — this
//! turn's keys, token and skill paths — not the copy stored in its row. What
//! the resume needs from the stored state (the queue, the outputs, the pending
//! tool call in memory) is keyed by node id, and whether a queued node is
//! ready depends on its incoming edges. So both graphs must share their
//! skeleton: the same node ids with the same `type`, and the same edges.
//! Everything else — `config`, `timezone`/`location`/`locale`, `trigger_on`,
//! the call limits — may change, and that is the point.

use crate::dag_engine::domain::graph::Graph;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

/// Stable prefix of the error a resume fails with when the child's graph
/// changed shape (or its source names no graph). Like `SUBGRAPH_DEPTH_EXCEEDED:`,
/// the text is the only surface the model and the embedder have.
pub const SUBGRAPH_RESUME_INCOMPATIBLE: &str = "SUBGRAPH_RESUME_INCOMPATIBLE:";

/// How many ids each list of a [`SkeletonDiff`] names before summarising.
const MAX_LISTED: usize = 5;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphSkeleton {
    /// node id → node `type`
    nodes: BTreeMap<String, String>,
    /// `(from, to, cyclic)`, an absent `cyclic` being `false`
    edges: BTreeSet<(String, String, bool)>,
}

impl GraphSkeleton {
    pub fn of(graph: &Graph) -> Self {
        Self {
            nodes: graph
                .nodes
                .iter()
                .map(|(id, node)| (id.clone(), node.node_type.clone()))
                .collect(),
            edges: graph
                .edges
                .iter()
                .map(|e| (e.from.clone(), e.to.clone(), e.cyclic.unwrap_or(false)))
                .collect(),
        }
    }

    /// `None` when `fresh` has the same skeleton as `self` (the stored one).
    pub fn diff(&self, fresh: &GraphSkeleton) -> Option<SkeletonDiff> {
        let removed = self
            .nodes
            .keys()
            .filter(|id| !fresh.nodes.contains_key(*id))
            .cloned()
            .collect();
        let added = fresh
            .nodes
            .keys()
            .filter(|id| !self.nodes.contains_key(*id))
            .cloned()
            .collect();
        let type_changed = self
            .nodes
            .iter()
            .filter_map(|(id, stored)| {
                fresh
                    .nodes
                    .get(id)
                    .filter(|now| *now != stored)
                    .map(|now| (id.clone(), stored.clone(), now.clone()))
            })
            .collect();
        let edges_changed = self.edges.symmetric_difference(&fresh.edges).count();
        let diff = SkeletonDiff {
            removed,
            added,
            type_changed,
            edges_changed,
        };
        (!diff.is_empty()).then_some(diff)
    }
}

/// What changed between a stored skeleton and a fresh one. Its `Display` is
/// the refusal text: node ids and types only, never a value from `config`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkeletonDiff {
    pub removed: Vec<String>,
    pub added: Vec<String>,
    /// `(id, stored type, fresh type)`
    pub type_changed: Vec<(String, String, String)>,
    /// Edges in one skeleton and not in the other (a changed edge counts twice).
    pub edges_changed: usize,
}

impl SkeletonDiff {
    fn is_empty(&self) -> bool {
        self.removed.is_empty()
            && self.added.is_empty()
            && self.type_changed.is_empty()
            && self.edges_changed == 0
    }
}

/// `a, b, c, d, e, +2 more`
fn listed<T>(items: &[T], show: impl Fn(&T) -> String) -> String {
    let mut out: Vec<String> = items.iter().take(MAX_LISTED).map(show).collect();
    if items.len() > MAX_LISTED {
        out.push(format!("+{} more", items.len() - MAX_LISTED));
    }
    out.join(", ")
}

impl fmt::Display for SkeletonDiff {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut parts = Vec::new();
        if !self.removed.is_empty() {
            parts.push(format!("removed: {}", listed(&self.removed, String::clone)));
        }
        if !self.added.is_empty() {
            parts.push(format!("added: {}", listed(&self.added, String::clone)));
        }
        if !self.type_changed.is_empty() {
            parts.push(format!(
                "type changed: {}",
                listed(&self.type_changed, |(id, stored, now)| format!(
                    "{id} ({stored} → {now})"
                ))
            ));
        }
        if self.edges_changed > 0 {
            parts.push(format!("edges changed: {}", self.edges_changed));
        }
        write!(
            f,
            "{SUBGRAPH_RESUME_INCOMPATIBLE} the child graph changed since it asked ({}). \
             Run it again from the start.",
            parts.join("; ")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn graph(v: Value) -> Graph {
        serde_json::from_value(v).expect("valid graph")
    }

    /// `entrada → pregunta → agente`, with the kind of config a resume must be
    /// free to refresh: a key, a prompt, an instance-local skills path.
    fn base() -> Value {
        json!({
            "nodes": {
                "entrada":  { "type": "input", "config": {} },
                "pregunta": { "type": "suspend", "config": { "id": "q" } },
                "agente":   { "type": "llm_call", "config": {
                    "api_key": "sk-old",
                    "system_message": "v1",
                    "skills": { "paths": ["/tmp/colmena-skills-cache/a"] }
                } }
            },
            "edges": [
                { "from": "entrada", "to": "pregunta" },
                { "from": "pregunta", "to": "agente" }
            ]
        })
    }

    fn diff(stored: Value, fresh: Value) -> Option<SkeletonDiff> {
        GraphSkeleton::of(&graph(stored)).diff(&GraphSkeleton::of(&graph(fresh)))
    }

    fn rename_pregunta_to_confirmar(g: &mut Value) {
        let q = g["nodes"]
            .as_object_mut()
            .unwrap()
            .remove("pregunta")
            .unwrap();
        g["nodes"]["confirmar"] = q;
    }

    #[test]
    fn the_same_graph_has_no_diff() {
        assert_eq!(diff(base(), base()), None);
    }

    #[test]
    fn config_graph_context_and_limits_may_change_freely() {
        let mut fresh = base();
        fresh["nodes"]["agente"]["config"] = json!({
            "api_key": "sk-new",
            "system_message": "v2",
            "skills": { "paths": ["/tmp/colmena-skills-cache/b"] }
        });
        fresh["nodes"]["agente"]["max_total_calls"] = json!(3);
        fresh["timezone"] = json!("Europe/Madrid");
        fresh["locale"] = json!("es-ES");
        assert_eq!(diff(base(), fresh), None);
    }

    #[test]
    fn an_absent_cyclic_flag_is_false() {
        let mut fresh = base();
        fresh["edges"][0]["cyclic"] = json!(false);
        assert_eq!(diff(base(), fresh), None);
    }

    #[test]
    fn a_removed_or_added_node_is_named() {
        let mut fresh = base();
        rename_pregunta_to_confirmar(&mut fresh);
        let d = diff(base(), fresh).expect("differs");
        assert_eq!(d.removed, vec!["pregunta".to_string()]);
        assert_eq!(d.added, vec!["confirmar".to_string()]);
        assert!(d.type_changed.is_empty());
    }

    #[test]
    fn a_changed_type_names_both_types() {
        let mut fresh = base();
        fresh["nodes"]["agente"]["type"] = json!("http_request");
        let d = diff(base(), fresh).expect("differs");
        assert_eq!(
            d.type_changed,
            vec![(
                "agente".to_string(),
                "llm_call".to_string(),
                "http_request".to_string()
            )]
        );
    }

    #[test]
    fn edges_differ_by_endpoints_and_by_cyclic() {
        let mut added = base();
        added["edges"]
            .as_array_mut()
            .unwrap()
            .push(json!({ "from": "entrada", "to": "agente" }));
        assert_eq!(diff(base(), added).map(|d| d.edges_changed), Some(1));

        let mut removed = base();
        removed["edges"].as_array_mut().unwrap().pop();
        assert_eq!(diff(base(), removed).map(|d| d.edges_changed), Some(1));

        let mut cyclic = base();
        cyclic["edges"][1]["cyclic"] = json!(true);
        assert_eq!(
            diff(base(), cyclic).map(|d| d.edges_changed),
            Some(2),
            "the plain edge goes and the cyclic one comes"
        );
    }

    #[test]
    fn the_message_leads_with_the_prefix_and_names_ids_and_types_only() {
        let mut fresh = base();
        rename_pregunta_to_confirmar(&mut fresh);
        fresh["nodes"]["agente"]["type"] = json!("http_request");
        fresh["nodes"]["agente"]["config"]["api_key"] = json!("sk-must-not-leak");
        fresh["edges"].as_array_mut().unwrap().pop();
        let text = diff(base(), fresh).expect("differs").to_string();
        assert_eq!(
            text,
            "SUBGRAPH_RESUME_INCOMPATIBLE: the child graph changed since it asked \
             (removed: pregunta; added: confirmar; type changed: agente (llm_call → http_request); \
             edges changed: 1). Run it again from the start."
        );
        assert!(!text.contains("sk-"), "{text}");
    }

    #[test]
    fn long_lists_are_capped() {
        let mut nodes = serde_json::Map::new();
        for i in 0..8 {
            nodes.insert(format!("n{i}"), json!({ "type": "input" }));
        }
        let text = diff(
            json!({ "nodes": {}, "edges": [] }),
            json!({ "nodes": nodes, "edges": [] }),
        )
        .expect("differs")
        .to_string();
        assert!(
            text.contains("added: n0, n1, n2, n3, n4, +3 more)"),
            "{text}"
        );
    }
}
