//! The config/input keys a `subgraph` reads its child graph from.
//!
//! Lives in the domain so every layer that has to recognise them — the node, the
//! tool-configuration memory check, preflight — derives from one list instead of
//! repeating string literals.
//!
//! They arrive either in the node's `config` (edge path) or in its `inputs` (tool
//! path, where `DagToolExecutor` merges the tool's `fixed_config` into inputs and
//! passes `config = {}`). Either way they are plumbing, never data for the child.
//!
//! `SubGraphNode::resolve_child_graph_source` reads them, and
//! `SubGraphNode::is_excluded_from_child_state` keeps them out of the child's
//! global state — otherwise the child's own `input` node passes them through and
//! they end up inside an LLM prompt, secrets already resolved.
//!
//! Both uses derive from [`CHILD_GRAPH_SOURCE_KEYS`] on purpose: a new source key
//! has to become invisible to the child by construction, not by remembering a
//! second list.

/// An inline child graph (`{ "nodes": …, "edges": … }`).
pub const CHILD_GRAPH_INLINE: &str = "child_graph_inline";
/// A path to a child graph JSON file.
pub const CHILD_GRAPH_PATH: &str = "child_graph_path";
/// `{ "agent_id": "<id>", "context": { … } }`, resolved at run time through
/// `ChildGraphResolverPort`. The resolved graph is never emitted nor returned.
pub const CHILD_GRAPH_REF: &str = "child_graph_ref";

/// Precedence order: the first key found wins (config before inputs).
pub const CHILD_GRAPH_SOURCE_KEYS: [&str; 3] =
    [CHILD_GRAPH_INLINE, CHILD_GRAPH_PATH, CHILD_GRAPH_REF];
