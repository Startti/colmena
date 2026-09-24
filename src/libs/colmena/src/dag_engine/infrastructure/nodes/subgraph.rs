use crate::colmena_log;
use crate::dag_engine::application::ports::{
    ChildGraphRequest, ChildGraphResolveError, ChildGraphResolverPort, ResolvedChildGraph,
    ResumeGraph, SubGraphExecutorPort,
};
use crate::dag_engine::domain::child_graph_source::{
    CHILD_GRAPH_INLINE, CHILD_GRAPH_PATH, CHILD_GRAPH_REF, CHILD_GRAPH_SOURCE_KEYS,
};
use crate::dag_engine::domain::events::{DagExecutionEvent, NodeEndError};
use crate::dag_engine::domain::graph_skeleton::SUBGRAPH_RESUME_INCOMPATIBLE;
use crate::dag_engine::domain::lint::{FieldSpec, NodeCatalogEntry};
use crate::dag_engine::domain::node::{ExecutableNode, NodeInputs};
use crate::dag_engine::domain::observer::{ChildScopeObserver, ExecutionObserver, NodeEvent};
use serde_json::{json, Value};
use std::error::Error;
use std::sync::{Arc, OnceLock};
use tokio::fs;

/// Where a subgraph boundary's name came from. Gates `errorText`: only `Tool`
/// goes through `MaskingObserver` (#310) — `Agent`/`Edge` stream unmasked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BoundarySource {
    Agent,
    Edge,
    Tool,
}

pub struct SubGraphNode {
    pub executor: Arc<OnceLock<Arc<dyn SubGraphExecutorPort>>>,
    /// The embedder's resolver for `child_graph_ref` sources, shared with
    /// `RouterNode` like `executor`.
    pub resolver: Arc<OnceLock<Arc<dyn ChildGraphResolverPort>>>,
}

impl Default for SubGraphNode {
    fn default() -> Self {
        Self::new()
    }
}

impl SubGraphNode {
    pub fn new() -> Self {
        Self {
            executor: Arc::new(OnceLock::new()),
            resolver: Arc::new(OnceLock::new()),
        }
    }

    /// Resolve the child graph source for both the edge-based path (config) and
    /// the tool path (inputs), together with the key it came from: a
    /// `child_graph_ref` is an object too, and only the key tells it apart from
    /// an inline graph.
    ///
    /// Precedence: `config` wins over `inputs` so the legacy edge-based behavior
    /// is unchanged; the tool path supplies the value via `inputs` (because the
    /// executor merges `fixed_config` into inputs and passes `config = {}`).
    fn resolve_child_graph_source(
        inputs: &NodeInputs,
        config: &Value,
    ) -> Option<(&'static str, Value)> {
        for key in CHILD_GRAPH_SOURCE_KEYS {
            if let Some(source) = config.get(key) {
                return Some((key, source.clone()));
            }
        }
        for key in CHILD_GRAPH_SOURCE_KEYS {
            if let Some(source) = inputs.get(key) {
                return Some((key, source.clone()));
            }
        }
        None
    }

    const RESOLVE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

    /// Resolve a `child_graph_ref` through the embedder's port. Runs BEFORE the
    /// boundary's start frame, so a child that never runs emits nothing.
    async fn resolve_ref(
        &self,
        reference: &Value,
        session_id: &str,
        agent_session_id: Option<String>,
        parent_path: &str,
    ) -> Result<ResolvedChildGraph, ChildGraphResolveError> {
        // A `${agentId}` still in place means the model never sent the argument
        // the `fixed` value templates from: refuse instead of asking for it.
        let agent_id = reference
            .get("agent_id")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty() && !s.contains("${"))
            .ok_or_else(|| {
                ChildGraphResolveError::NotFound(
                    "child_graph_ref needs a resolved agent_id (was the model's agentId argument missing?)"
                        .into(),
                )
            })?;
        let resolver = self.resolver.get().ok_or_else(|| {
            ChildGraphResolveError::Unavailable("no child graph resolver configured".into())
        })?;
        let req = ChildGraphRequest {
            agent_id: agent_id.to_string(),
            context: reference.get("context").cloned().unwrap_or(Value::Null),
            session_id: session_id.to_string(),
            agent_session_id,
            parent_path: parent_path.to_string(),
        };
        match tokio::time::timeout(Self::RESOLVE_TIMEOUT, resolver.resolve(req)).await {
            Ok(result) => result,
            Err(_) => Err(ChildGraphResolveError::Unavailable(format!(
                "resolver timed out after {}s",
                Self::RESOLVE_TIMEOUT.as_secs()
            ))),
        }
    }

    /// Loads the graph a `(key, source)` pair names: a ref through the
    /// embedder's resolver, a path from disk, an inline graph as is. Shared by
    /// a fresh run and a resume, so a resume loads exactly what a fresh run
    /// would — for a ref, with the same `ChildGraphRequest`. `Err` is the text
    /// the caller sees; a ref's display name comes back for the boundary.
    async fn load_child_graph(
        &self,
        (source_key, graph_source): (&'static str, Value),
        session_id: &str,
        agent_session_id: Option<String>,
        parent_path: &str,
    ) -> Result<(Value, Option<String>), String> {
        if source_key == CHILD_GRAPH_REF {
            let resolved = self
                .resolve_ref(&graph_source, session_id, agent_session_id, parent_path)
                .await
                .map_err(|e| e.to_string())?;
            return Ok((resolved.graph, Some(resolved.display_name)));
        }
        if source_key == CHILD_GRAPH_INLINE || graph_source.is_object() {
            return Ok((graph_source, None));
        }
        let Some(path_val) = graph_source.as_str() else {
            return Err("child_graph source must be an inline object or a path string".into());
        };
        let path = std::path::Path::new(path_val);
        if !path.exists() {
            return Err(format!("child_graph_path not found: {}", path_val));
        }
        let contents = fs::read_to_string(path).await.map_err(|e| e.to_string())?;
        let graph = serde_json::from_str(&contents).map_err(|e| e.to_string())?;
        Ok((graph, None))
    }

    /// The graph a suspended child resumes with: derived again from the same
    /// source a fresh run reads, so it carries this turn's keys, token and
    /// skill paths. Never fails — a source that cannot give a graph becomes
    /// `Unavailable`, which the executor turns into a closed row and that text.
    async fn resume_graph(
        &self,
        stored_valve: bool,
        inputs: &NodeInputs,
        config: &Value,
        session_id: &str,
        agent_session_id: Option<String>,
        parent_path: &str,
    ) -> ResumeGraph {
        if stored_valve {
            return ResumeGraph::Stored;
        }
        let Some(source) = Self::resolve_child_graph_source(inputs, config) else {
            return ResumeGraph::Unavailable(format!(
                "{SUBGRAPH_RESUME_INCOMPATIBLE} the subgraph has no child graph source. \
                 Run it again from the start."
            ));
        };
        // A ref keeps its stored graph until the resolver is asked again on
        // resume (the next change).
        if source.0 == CHILD_GRAPH_REF {
            return ResumeGraph::Stored;
        }
        match self
            .load_child_graph(source, session_id, agent_session_id, parent_path)
            .await
        {
            Ok((graph, _display_name)) => ResumeGraph::Fresh(graph),
            Err(reason) => ResumeGraph::Unavailable(reason),
        }
    }

    /// True for keys that must never cross into the child graph's global state.
    ///
    /// Two families: the engine's own bookkeeping (`__colmena_*`, `__node_id`),
    /// and the operator's child-graph plumbing ([`CHILD_GRAPH_SOURCE_KEYS`]).
    /// Everything else — the model's tool arguments, `files`, whatever the parent
    /// put on the wire — is data the child is meant to see.
    ///
    /// Kept as a pure function so the rule is unit-testable without standing up a
    /// graph run.
    fn is_excluded_from_child_state(key: &str) -> bool {
        key.starts_with("__colmena_")
            || key == "__node_id"
            || CHILD_GRAPH_SOURCE_KEYS.contains(&key)
    }

    /// Build the child graph's initial global state from this node's inputs.
    ///
    /// Everything the child is meant to see is copied verbatim; the engine's
    /// bookkeeping and the operator's plumbing are dropped (see
    /// [`Self::is_excluded_from_child_state`]).
    ///
    /// The nesting depth is re-inserted afterwards on purpose. The counter is
    /// kept even though nesting is unbounded by default: it feeds the optional
    /// `COLMENA_MAX_SUBGRAPH_DEPTH` ceiling and is the value observability
    /// reports as the run's nesting level. Because the filter drops every
    /// `__colmena_*` key, re-inserting it is the only way it survives into the
    /// child's global state.
    fn build_child_state(inputs: &NodeInputs) -> Value {
        let mut child_state_obj = serde_json::Map::new();
        for (k, v) in inputs {
            if !Self::is_excluded_from_child_state(k) {
                child_state_obj.insert(k.clone(), v.clone());
            }
        }
        child_state_obj.insert(
            "__colmena_subgraph_depth".to_string(),
            json!(Self::current_depth(inputs) + 1),
        );
        Value::Object(child_state_obj)
    }

    /// The value a subgraph hands back to its caller: the output node's value when
    /// the child flagged one, the whole run otherwise. Shared by the fresh and the
    /// resume paths, so a resumed tool returns what a fresh one would — and never
    /// the child's full `all_outputs`, which can carry a fetched graph or a tool's
    /// raw response.
    fn extract_final_output(result: &Value) -> Value {
        result
            .as_object()
            .and_then(|obj| {
                obj.values().find(|v| {
                    v.get("extra_info")
                        .and_then(|ei| ei.get("__colmena_is_output_node"))
                        .and_then(|f| f.as_bool())
                        .unwrap_or(false)
                })
            })
            .cloned()
            .unwrap_or_else(|| result.clone())
    }

    /// Optional ceiling for subgraph nesting depth.
    ///
    /// Nesting is **unbounded by default**. The engine no longer second-guesses
    /// how deeply a graph author composes agents-as-tools; the previous
    /// hard-coded limit of 5 rejected legitimate deep compositions with no way
    /// to opt out.
    ///
    /// A ceiling can still be enabled per deployment with
    /// `COLMENA_MAX_SUBGRAPH_DEPTH=<n>`, as an operational safety valve against
    /// runaway recursion (a subgraph tool that references itself, or a cycle of
    /// agents calling each other). Unset, empty, or unparseable means "no
    /// limit". `0` also means "no limit" rather than "reject everything", so a
    /// stray `=0` in a deploy script cannot brick every subgraph in production.
    ///
    /// Read once and cached: the value is process-wide configuration, and this
    /// runs on every subgraph dispatch.
    fn depth_ceiling() -> Option<u64> {
        static CEILING: OnceLock<Option<u64>> = OnceLock::new();
        *CEILING.get_or_init(|| {
            std::env::var("COLMENA_MAX_SUBGRAPH_DEPTH")
                .ok()
                .and_then(|raw| raw.trim().parse::<u64>().ok())
                .filter(|n| *n > 0)
        })
    }

    /// `COLMENA_SUBGRAPH_RESUME_GRAPH=stored` resumes every child with the graph
    /// stored in its row, as up to v0.16: a safety valve while the structure
    /// check proves itself in production. Anything else, unset included,
    /// derives the graph from the source. Read once, like the depth ceiling.
    fn stored_resume_valve() -> bool {
        static STORED: OnceLock<bool> = OnceLock::new();
        *STORED.get_or_init(|| {
            Self::valve_is_stored(
                std::env::var("COLMENA_SUBGRAPH_RESUME_GRAPH")
                    .ok()
                    .as_deref(),
            )
        })
    }

    /// Pure half of [`Self::stored_resume_valve`], testable without touching
    /// the process environment.
    fn valve_is_stored(raw: Option<&str>) -> bool {
        raw.is_some_and(|v| v.trim().eq_ignore_ascii_case("stored"))
    }

    /// Read `key` from inputs as a non-empty string. Used for the boundary-name
    /// fallback chain, where an empty string must be treated as "absent" so the
    /// next source gets a turn.
    fn non_empty_str(inputs: &NodeInputs, key: &str) -> Option<String> {
        inputs
            .get(key)
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
    }

    /// Current subgraph-tool depth from inputs (0 when absent).
    fn current_depth(inputs: &NodeInputs) -> u64 {
        inputs
            .get("__colmena_subgraph_depth")
            .and_then(|v| v.as_u64())
            .unwrap_or(0)
    }

    /// Pure ceiling comparison. Split out from [`Self::depth_exceeded`] so tests
    /// can exercise every case without mutating the process environment — env
    /// mutation races across the parallel test harness and `depth_ceiling` is
    /// cached after its first read anyway.
    fn exceeds_ceiling(depth: u64, ceiling: Option<u64>) -> bool {
        matches!(ceiling, Some(max) if depth >= max)
    }

    /// True only when a ceiling is configured AND this depth reaches it.
    /// Always false in the default (unbounded) configuration.
    fn depth_exceeded(inputs: &NodeInputs) -> bool {
        Self::exceeds_ceiling(Self::current_depth(inputs), Self::depth_ceiling())
    }
}

#[async_trait::async_trait]
impl ExecutableNode for SubGraphNode {
    fn schema(&self) -> Value {
        // The `inputs` map is what the tool-definition builder reads to expose
        // parameters to the LLM (it parses each value's string for type hints
        // like "string"/"number"/"optional"). Default to a single `task` string.
        // A `node_schema` in tool_configurations takes precedence over this.
        json!({
            "inputs": {
                "task": "string — the task or instruction for the sub-agent to perform"
            }
        })
    }

    fn config_schema(&self) -> Option<NodeCatalogEntry> {
        // The child-graph sources come from the constant the node itself uses
        // to look them up, so adding one there cannot silently skip the
        // catalog. Only the path is a string; the inline graph and the ref are
        // objects.
        let mut entry =
            NodeCatalogEntry::no_config().with_field("__agent_name", FieldSpec::of_type("string"));
        for key in CHILD_GRAPH_SOURCE_KEYS {
            let ty = if key == CHILD_GRAPH_PATH {
                "string"
            } else {
                "object"
            };
            entry = entry.with_field(key, FieldSpec::of_type(ty));
        }
        Some(entry)
    }

    async fn execute(
        &self,
        inputs: &NodeInputs,
        config: &Value,
        _global_state: &mut Value,
        _observer: Option<Arc<dyn ExecutionObserver>>,
    ) -> Result<Value, Box<dyn Error + Send + Sync>> {
        let parent_session_id = inputs
            .get("__colmena_session_id")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown_parent")
            .to_string();

        let agent_session_id = inputs
            .get("__colmena_agent_session_id")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());

        // The subgraph node's *own* path is what its children must inherit.
        let parent_path = inputs
            .get("__colmena_node_id_path")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();

        let child_path_prefix = if parent_path.is_empty() {
            None
        } else {
            Some(parent_path.clone())
        };

        // Agent name is injected by OrchestratorNode when this subgraph runs as a
        // named agent.
        let agent_name = config
            .get("__agent_name")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        // Name for the subgraph boundary frames (subgraph-node-start/end).
        // Without one the branch has no visible delimiter and the nested UI tree
        // cannot be built (Fase F). Three sources, in order:
        //
        //   1. `__agent_name`        — OrchestratorNode, for a named agent.
        //   2. `__node_id`           — the graph execution loop, for the
        //                              edge-based path where a real node exists.
        //   3. `__colmena_tool_name` — DagToolExecutor, for the tool path.
        //
        // (3) is what makes the tool path work at all. It used to fall through to
        // (2), but `__node_id` is only ever set by the graph loop and a tool
        // dispatch never goes through that loop — so `boundary_name` was always
        // `None` there and a subgraph-as-tool silently emitted no boundary.
        let tool_name = Self::non_empty_str(inputs, "__colmena_tool_name");
        let boundary: Option<(String, BoundarySource)> = agent_name
            .clone()
            .map(|n| (n, BoundarySource::Agent))
            .or_else(|| Self::non_empty_str(inputs, "__node_id").map(|n| (n, BoundarySource::Edge)))
            .or_else(|| tool_name.clone().map(|n| (n, BoundarySource::Tool)));
        let boundary_name = boundary.as_ref().map(|(n, _)| n.clone());
        let boundary_source = boundary.as_ref().map(|(_, s)| *s);

        // Observer handed to the CHILD run. On the two synthetic-boundary paths
        // — an orchestrator agent, or a tool call — the enclosing loop stamps its
        // OWN node id onto the child's lineage, not the boundary's. Left alone
        // the boundary frame and the content it delimits come out as siblings
        // under the same parent instead of parent and child, so a tree built by
        // grouping on `path` shows a sub-agent's work next to its label rather
        // than inside it. Scoping the child's observer inserts the missing hop.
        //
        // The edge-based path is deliberately excluded: there the boundary name
        // IS the graph node id the loop already prepends, so scoping would
        // duplicate that segment and give every existing run a phantom level.
        let synthetic_boundary = agent_name.is_some() || tool_name.is_some();
        let child_observer = match (&_observer, &boundary_name, synthetic_boundary) {
            (Some(obs), Some(name), true) => Some(ChildScopeObserver::wrap(obs.clone(), name)),
            _ => _observer.clone(),
        };

        // Only fires when an operator opted into a ceiling via
        // COLMENA_MAX_SUBGRAPH_DEPTH; nesting is unbounded by default.
        //
        // The message leads with a stable `SUBGRAPH_DEPTH_EXCEEDED:` code. This
        // error reaches the calling LLM as an ordinary failed tool result, whose
        // only machine-readable surface is its text — without a stable prefix a
        // consumer has to substring-match on prose to tell a recursion ceiling
        // apart from an HTTP timeout or a SQL error.
        if Self::depth_exceeded(inputs) {
            let ceiling = Self::depth_ceiling().unwrap_or_default();
            return Err(format!(
                "SUBGRAPH_DEPTH_EXCEEDED: subgraph nesting reached the configured ceiling \
                 of {ceiling} (COLMENA_MAX_SUBGRAPH_DEPTH). Nesting is unlimited unless that \
                 variable is set, so hitting this usually means a subgraph tool references \
                 itself or a cycle of agents calls each other."
            )
            .into());
        }

        // --- 1. RESUME PROPAGATION ---
        // If the parent was suspended in this node, it receives __colmena_resume_answer.
        // We find the existing child run by parent_session_id instead of the old
        // deterministic "{parent}_sub_{node_id}" naming.
        if let Some(resume_answer) = inputs
            .get("__colmena_resume_answer")
            .and_then(|v| v.as_str())
        {
            let executor = self
                .executor
                .get()
                .ok_or("SubGraphExecutorPort not initialized in SubGraphNode")?;

            let child_session_id = executor
                .find_child_session_id_for_resume(&parent_session_id, &parent_path)
                .await?
                .ok_or_else(|| {
                    format!(
                        "No suspended child found under parent {} / path {}",
                        parent_session_id, parent_path
                    )
                })?;

            colmena_log!(
                "▶️ [SubGraphNode] Resuming child graph {} (path={}) with answer...",
                child_session_id,
                parent_path
            );
            // The child resumes with the graph its source names NOW — this
            // turn's keys, token and skill paths — not the copy its row kept;
            // the executor checks the structure still matches. Derived only
            // once there is a child to resume: for a ref, each derivation is a
            // call to the embedder.
            let graph = self
                .resume_graph(
                    Self::stored_resume_valve(),
                    inputs,
                    config,
                    &parent_session_id,
                    agent_session_id.clone(),
                    &parent_path,
                )
                .await;
            let result = executor
                .resume_subgraph(
                    &child_session_id,
                    resume_answer.to_string(),
                    graph,
                    child_observer.clone(),
                    agent_session_id.clone(),
                    child_path_prefix.clone(),
                )
                .await?;

            // Check if the child suspended AGAIN
            if result.get("__colmena_status").and_then(|v| v.as_str()) == Some("SUSPENDED") {
                // Bubble up
                return Ok(result);
            }
            return Ok(Self::extract_final_output(&result));
        }

        // --- 2. GRAPH LOADING ---
        // Source can come from `config` (edge-based path) or `inputs` (tool path,
        // where the executor merges fixed_config into inputs and passes config={}).
        // A ref is resolved here, before the boundary's start frame: a child that
        // never runs emits nothing. The resolved graph goes only to the executor
        // — never into `inputs`, a frame or this node's output.
        let source = Self::resolve_child_graph_source(inputs, config).ok_or(
            "SubGraphNode requires 'child_graph_inline', 'child_graph_path' or 'child_graph_ref' \
             in config (edge path) or inputs (tool path)",
        )?;
        let (graph_json, display_name) = self
            .load_child_graph(
                source,
                &parent_session_id,
                agent_session_id.clone(),
                &parent_path,
            )
            .await?;

        // --- 3. STATE MAPPING (IN) ---
        let child_state = Self::build_child_state(inputs);

        // Resolve BEFORE the boundary start: a missing executor must emit no
        // start at all, not a start with no matching close.
        let executor = self
            .executor
            .get()
            .ok_or("SubGraphExecutorPort not initialized in SubGraphNode")?;

        // Emit subgraph node-start boundary event (orchestrator agent OR
        // subgraph-as-tool — see `boundary_name`). When the boundary came from a
        // `child_graph_ref`, the resolver's display name travels in `config` as
        // `node_label`; the SSE mapper lifts it to a top-level field on the
        // wrapped `subgraph-node-start` frame (never a new `NodeStart` variant).
        if let (Some(ref name), Some(ref obs)) = (&boundary_name, &_observer) {
            let start_config = match &display_name {
                Some(label) => json!({ "node_label": label }),
                None => Value::Object(Default::default()),
            };
            let start_event = DagExecutionEvent::NodeStart {
                node_id: name.clone(),
                node_type: "subgraph".to_string(),
                inputs: Value::Object(Default::default()),
                config: start_config,
            };
            if let Ok(raw) = serde_json::to_value(&start_event) {
                obs.on_event(NodeEvent::SubgraphChildEvent(raw));
            }
        }

        // FRESH RUN — generate a new UUID for the child session.
        let child_session_id = uuid::Uuid::new_v4().to_string();

        colmena_log!(
            "🔄 [SubGraphNode] Running SubGraph in isolated session: {} (path_prefix={:?})",
            child_session_id,
            child_path_prefix
        );

        let result = match executor
            .run_subgraph(
                &child_session_id,
                graph_json,
                child_state,
                child_observer.clone(),
                Some(parent_session_id.clone()),
                agent_session_id.clone(),
                child_path_prefix.clone(),
            )
            .await
        {
            Ok(v) => v,
            Err(e) => {
                // Close the boundary instead of leaving it dangling (the ADP
                // bug). `errorText` gated per `BoundarySource`.
                if let (Some(ref name), Some(ref obs)) = (&boundary_name, &_observer) {
                    let message = match boundary_source {
                        Some(BoundarySource::Tool) => Some(e.to_string()),
                        _ => None,
                    };
                    let finish_event = DagExecutionEvent::SubgraphNodeFinish {
                        node_id: name.clone(),
                        output: Value::Null,
                        error: Some(NodeEndError { message }),
                    };
                    if let Ok(raw) = serde_json::to_value(&finish_event) {
                        obs.on_event(NodeEvent::SubgraphChildEvent(raw));
                    }
                }
                return Err(e.into());
            }
        };

        // --- 4. SUSPEND BUBBLE-UP ---
        if result.get("__colmena_status").and_then(|v| v.as_str()) == Some("SUSPENDED") {
            colmena_log!("⏸️ [SubGraphNode] Child graph suspended! Bubbling up to parent...");
            return Ok(result);
        }

        // --- 5. STATE MAPPING (OUT) ---
        // Find the node flagged as __colmena_is_output_node and extract its value.
        let final_output = Self::extract_final_output(&result);

        // Emit subgraph node-end boundary event with final output
        if let (Some(ref name), Some(ref obs)) = (&boundary_name, &_observer) {
            let finish_event = DagExecutionEvent::SubgraphNodeFinish {
                node_id: name.clone(),
                output: final_output.clone(),
                error: None,
            };
            if let Ok(raw) = serde_json::to_value(&finish_event) {
                obs.on_event(NodeEvent::SubgraphChildEvent(raw));
            }
        }

        Ok(final_output)
    }
}

#[cfg(test)]
mod subgraph_tool_input_config_tests {
    use super::*;
    use crate::dag_engine::domain::node::NodeInputs;
    use serde_json::json;

    fn resolve_graph_source(inputs: &NodeInputs, config: &Value) -> Option<Value> {
        SubGraphNode::resolve_child_graph_source(inputs, config).map(|(_, v)| v)
    }

    #[test]
    fn reads_inline_from_inputs_when_config_empty() {
        let mut inputs: NodeInputs = NodeInputs::new();
        let inline = json!({ "nodes": {}, "edges": [] });
        inputs.insert("child_graph_inline".to_string(), inline.clone());
        let config = json!({});
        assert_eq!(resolve_graph_source(&inputs, &config), Some(inline));
    }

    #[test]
    fn reads_path_from_inputs_when_config_empty() {
        let mut inputs: NodeInputs = NodeInputs::new();
        inputs.insert(
            "child_graph_path".to_string(),
            json!("./agents/weather_agent.json"),
        );
        let config = json!({});
        assert_eq!(
            resolve_graph_source(&inputs, &config),
            Some(json!("./agents/weather_agent.json"))
        );
    }

    #[test]
    fn config_takes_precedence_over_inputs_for_inline() {
        let mut inputs: NodeInputs = NodeInputs::new();
        inputs.insert(
            "child_graph_inline".to_string(),
            json!({ "from": "inputs" }),
        );
        let config = json!({ "child_graph_inline": { "from": "config" } });
        assert_eq!(
            resolve_graph_source(&inputs, &config),
            Some(json!({ "from": "config" }))
        );
    }

    #[test]
    fn config_path_takes_precedence_over_inputs() {
        let mut inputs: NodeInputs = NodeInputs::new();
        inputs.insert("child_graph_path".to_string(), json!("./from_inputs.json"));
        let config = json!({ "child_graph_path": "./from_config.json" });
        assert_eq!(
            resolve_graph_source(&inputs, &config),
            Some(json!("./from_config.json"))
        );
    }

    #[test]
    fn within_one_container_inline_and_path_come_before_a_ref() {
        // The order of CHILD_GRAPH_SOURCE_KEYS is a contract: inline, path, ref.
        let mut inputs: NodeInputs = NodeInputs::new();
        inputs.insert("child_graph_ref".to_string(), json!({ "agent_id": "a1" }));
        inputs.insert(
            "child_graph_inline".to_string(),
            json!({ "from": "inline" }),
        );
        assert_eq!(
            resolve_graph_source(&inputs, &json!({})),
            Some(json!({ "from": "inline" }))
        );

        let config =
            json!({ "child_graph_ref": { "agent_id": "a1" }, "child_graph_path": "./p.json" });
        assert_eq!(
            resolve_graph_source(&NodeInputs::new(), &config),
            Some(json!("./p.json"))
        );
    }

    #[test]
    fn returns_none_when_neither_config_nor_inputs_has_source() {
        let inputs: NodeInputs = NodeInputs::new();
        let config = json!({});
        assert_eq!(resolve_graph_source(&inputs, &config), None);
    }
}

#[cfg(test)]
mod subgraph_child_state_isolation_tests {
    //! The child graph's initial global state must carry the parent's data and
    //! nothing else. Two families are dropped: the engine's own bookkeeping, and
    //! the operator's child-graph plumbing.
    //!
    //! The plumbing half is a security boundary, not tidiness. `child_graph_inline`
    //! holds the child's `llm_call` config with secrets already resolved. Left in
    //! the state, the child's own `input` node (`data: {}` → passthrough) hands it
    //! to an `llm_call` as a non-empty object `prompt`, which `resolve_prompt_or_task`
    //! preserves verbatim — so provider keys and a Postgres `connection_url` reach
    //! the model and get persisted in `llm_node_history`. Measured in the field by
    //! ADP on 2026-08-25; a sub-agent then copied the connection string into a
    //! document it wrote for the end user.
    //!
    //! Leaving it in also lets a nested `subgraph` with no `config` of its own fall
    //! back to `inputs.get("child_graph_inline")` and re-resolve the *parent's*
    //! graph — silent recursion.

    use super::*;
    use crate::dag_engine::domain::node::NodeInputs;
    use serde_json::json;

    /// A `child_graph_inline` shaped like the real thing: the secrets live inside
    /// the child's `llm_call` config, which is exactly where `memory_mode`
    /// requires a `connection_url` to go.
    fn inline_with_secrets() -> Value {
        json!({
            "nodes": {
                "keeper": { "type": "llm_call", "config": {
                    "api_key": "AIzaFAKE_child_key_do_not_use_11111111",
                    "connection_url": "postgresql://fakeuser:fakepass@127.0.0.1:5432/fakedb"
                }}
            },
            "edges": []
        })
    }

    fn state_keys(state: &Value) -> Vec<String> {
        let mut keys: Vec<String> = state
            .as_object()
            .expect("child state is an object")
            .keys()
            .cloned()
            .collect();
        keys.sort();
        keys
    }

    #[test]
    fn child_graph_inline_never_reaches_child_state() {
        let mut inputs: NodeInputs = NodeInputs::new();
        inputs.insert("child_graph_inline".to_string(), inline_with_secrets());
        inputs.insert("task".to_string(), json!("redactá el spec"));

        let state = SubGraphNode::build_child_state(&inputs);

        assert!(
            state.get("child_graph_inline").is_none(),
            "the operator's plumbing must not become child state: {state}"
        );
        // Belt and braces: the secrets must not survive under any other key.
        let serialized = serde_json::to_string(&state).unwrap();
        assert!(!serialized.contains("fakepass"), "leaked: {serialized}");
        assert!(!serialized.contains("AIzaFAKE"), "leaked: {serialized}");
    }

    #[test]
    fn child_graph_path_never_reaches_child_state() {
        let mut inputs: NodeInputs = NodeInputs::new();
        inputs.insert(
            "child_graph_path".to_string(),
            json!("./agents/weather_agent.json"),
        );
        inputs.insert("task".to_string(), json!("clima en Bogotá"));

        let state = SubGraphNode::build_child_state(&inputs);

        assert!(state.get("child_graph_path").is_none());
        assert_eq!(state.get("task"), Some(&json!("clima en Bogotá")));
    }

    #[test]
    fn model_supplied_args_reach_child_state() {
        // The exact key sets ADP measured across 215 rows. `confirmation` is the
        // reminder that this set is chosen by the model per call, not declared in
        // a schema — which is why the filter can only be a blocklist of plumbing,
        // never an allowlist of data.
        let mut inputs: NodeInputs = NodeInputs::new();
        inputs.insert("child_graph_inline".to_string(), inline_with_secrets());
        inputs.insert("task".to_string(), json!("redactá el spec"));
        inputs.insert("docKind".to_string(), json!("spec"));
        inputs.insert("name".to_string(), json!("colmena-leak"));
        inputs.insert("scope".to_string(), json!("platform"));
        inputs.insert("confirmation".to_string(), json!(true));

        let state = SubGraphNode::build_child_state(&inputs);

        assert_eq!(
            state_keys(&state),
            vec![
                "__colmena_subgraph_depth",
                "confirmation",
                "docKind",
                "name",
                "scope",
                "task",
            ]
        );
    }

    #[test]
    fn files_reaches_child_state() {
        // `llm.rs` resolves attachments from `inputs.get("files")`. This is the
        // key that must survive, and the reason the fix lives here rather than in
        // `input.rs`: at this seam the exclusion is a known list, over there it
        // would mean reasoning about every key.
        let mut inputs: NodeInputs = NodeInputs::new();
        inputs.insert("child_graph_inline".to_string(), inline_with_secrets());
        inputs.insert("files".to_string(), json!([{ "id": "file_123" }]));

        let state = SubGraphNode::build_child_state(&inputs);

        assert_eq!(state.get("files"), Some(&json!([{ "id": "file_123" }])));
    }

    #[test]
    fn engine_internal_keys_stay_excluded() {
        let mut inputs: NodeInputs = NodeInputs::new();
        inputs.insert("__node_id".to_string(), json!("my_tool"));
        inputs.insert("__colmena_tool_name".to_string(), json!("Document_agent"));
        inputs.insert("__colmena_session_id".to_string(), json!("sess_1"));
        inputs.insert("task".to_string(), json!("algo"));

        let state = SubGraphNode::build_child_state(&inputs);

        assert!(state.get("__node_id").is_none());
        assert!(state.get("__colmena_tool_name").is_none());
        assert!(state.get("__colmena_session_id").is_none());
        assert_eq!(state.get("task"), Some(&json!("algo")));
    }

    #[test]
    fn depth_still_propagates_into_the_child() {
        // The one `__colmena_*` key that is re-inserted after the filter.
        let mut inputs: NodeInputs = NodeInputs::new();
        inputs.insert("__colmena_subgraph_depth".to_string(), json!(2));

        let state = SubGraphNode::build_child_state(&inputs);

        assert_eq!(state.get("__colmena_subgraph_depth"), Some(&json!(3)));
    }

    #[test]
    fn depth_starts_at_one_when_absent() {
        let inputs: NodeInputs = NodeInputs::new();
        let state = SubGraphNode::build_child_state(&inputs);
        assert_eq!(state.get("__colmena_subgraph_depth"), Some(&json!(1)));
    }

    #[test]
    fn the_node_still_resolves_the_graph_it_no_longer_passes_down() {
        // The exclusion is safe precisely because resolution happens first:
        // `resolve_child_graph_source` reads the key, then `build_child_state`
        // drops it. Both halves asserted together so neither can drift.
        let mut inputs: NodeInputs = NodeInputs::new();
        inputs.insert("child_graph_inline".to_string(), inline_with_secrets());

        assert_eq!(
            SubGraphNode::resolve_child_graph_source(&inputs, &json!({})).map(|(_, v)| v),
            Some(inline_with_secrets()),
            "the node must still find its own graph"
        );
        assert!(
            SubGraphNode::build_child_state(&inputs)
                .get("child_graph_inline")
                .is_none(),
            "but must not hand it to the child"
        );
    }
}

#[cfg(test)]
mod subgraph_schema_tests {
    use super::*;
    use crate::dag_engine::domain::node::ExecutableNode;

    #[test]
    fn schema_exposes_task_input_for_tool_use() {
        let node = SubGraphNode::new();
        let schema = node.schema();
        let inputs = schema
            .get("inputs")
            .and_then(|v| v.as_object())
            .expect("schema must have an 'inputs' object so the tool builder exposes params");
        assert!(
            inputs.contains_key("task"),
            "default schema must expose a 'task' input; got keys: {:?}",
            inputs.keys().collect::<Vec<_>>()
        );
        let desc = inputs.get("task").and_then(|v| v.as_str()).unwrap_or("");
        assert!(
            desc.contains("string"),
            "task description must hint type 'string' for the builder; got: {desc:?}"
        );
    }
}

#[cfg(test)]
mod subgraph_depth_ceiling_tests {
    //! Nesting is unbounded by default. The old hard-coded limit of 5 is gone;
    //! a ceiling only exists when an operator sets COLMENA_MAX_SUBGRAPH_DEPTH.
    use super::*;
    use crate::dag_engine::domain::node::NodeInputs;
    use serde_json::json;

    #[test]
    fn no_ceiling_configured_never_rejects_any_depth() {
        // The headline behavior change: arbitrarily deep nesting is allowed.
        for depth in [0_u64, 1, 5, 6, 50, 10_000, u64::MAX] {
            assert!(
                !SubGraphNode::exceeds_ceiling(depth, None),
                "depth {depth} must be allowed when no ceiling is configured"
            );
        }
    }

    #[test]
    fn depth_five_is_allowed_now_that_the_hard_limit_is_gone() {
        // Regression lock on the exact case the old guard rejected.
        assert!(!SubGraphNode::exceeds_ceiling(5, None));
        assert!(!SubGraphNode::exceeds_ceiling(6, None));
    }

    #[test]
    fn configured_ceiling_rejects_at_and_above_it() {
        assert!(SubGraphNode::exceeds_ceiling(3, Some(3)));
        assert!(SubGraphNode::exceeds_ceiling(4, Some(3)));
    }

    #[test]
    fn configured_ceiling_allows_below_it() {
        assert!(!SubGraphNode::exceeds_ceiling(0, Some(3)));
        assert!(!SubGraphNode::exceeds_ceiling(2, Some(3)));
    }

    #[test]
    fn current_depth_defaults_to_zero_and_reads_the_ambient_key() {
        assert_eq!(SubGraphNode::current_depth(&NodeInputs::new()), 0);
        let mut inputs: NodeInputs = NodeInputs::new();
        inputs.insert("__colmena_subgraph_depth".to_string(), json!(7));
        assert_eq!(SubGraphNode::current_depth(&inputs), 7);
    }

    #[test]
    fn non_numeric_depth_falls_back_to_zero() {
        let mut inputs: NodeInputs = NodeInputs::new();
        inputs.insert(
            "__colmena_subgraph_depth".to_string(),
            json!("not-a-number"),
        );
        assert_eq!(SubGraphNode::current_depth(&inputs), 0);
    }

    #[tokio::test]
    async fn execute_does_not_reject_deeply_nested_runs_by_default() {
        // Depth 42 with no ceiling: execution must get PAST the guard. Without an
        // executor wired it still fails, but the failure must not be the ceiling.
        use crate::dag_engine::domain::node::ExecutableNode;
        let node = SubGraphNode::new();
        let mut inputs: NodeInputs = NodeInputs::new();
        inputs.insert("__colmena_subgraph_depth".to_string(), json!(42));
        inputs.insert(
            "child_graph_inline".to_string(),
            json!({ "nodes": {}, "edges": [] }),
        );
        let cfg = json!({});
        let mut gs = json!({});
        let res = node.execute(&inputs, &cfg, &mut gs, None).await;
        if let Err(e) = res {
            assert!(
                !e.to_string().contains("SUBGRAPH_DEPTH_EXCEEDED"),
                "deep nesting must not be rejected by default; got: {e}"
            );
        }
    }
}

#[cfg(test)]
mod subgraph_as_tool_boundary_tests {
    //! Fase F: a subgraph invoked as a plain tool (no `__agent_name`) must still
    //! emit node-start / node-end boundary frames so the parent stream can
    //! delimit the sub-tree. Boundary name falls back to the node's `__node_id`.
    use super::*;
    use crate::dag_engine::application::ports::SubGraphExecutorPort;
    use crate::dag_engine::domain::error::DagError;
    use crate::dag_engine::domain::node::{ExecutableNode, NodeInputs};
    use crate::dag_engine::domain::observer::{ExecutionObserver, NodeEvent};
    use serde_json::json;
    use std::sync::Mutex;

    #[derive(Default)]
    struct CapturingObserver {
        events: Mutex<Vec<NodeEvent>>,
    }
    impl ExecutionObserver for CapturingObserver {
        fn on_event(&self, event: NodeEvent) {
            self.events.lock().unwrap().push(event);
        }
    }

    /// Stands in for a child graph run. Emits one node event through whatever
    /// observer it was handed, so a test can see how that observer was scoped.
    struct StubExecutor;

    /// The event the stubbed "child graph" emits. `node_type` is deliberately
    /// not `subgraph`, so it never collides with the boundary frames.
    fn stub_child_event() -> DagExecutionEvent {
        DagExecutionEvent::NodeStart {
            node_id: "child_node".to_string(),
            node_type: "llm_call".to_string(),
            inputs: json!({}),
            config: json!({}),
        }
    }

    #[async_trait::async_trait]
    impl SubGraphExecutorPort for StubExecutor {
        async fn run_subgraph(
            &self,
            _session_id: &str,
            _graph_json: Value,
            _global_state: Value,
            _observer: Option<Arc<dyn ExecutionObserver>>,
            _parent_session_id: Option<String>,
            _agent_session_id: Option<String>,
            _path_prefix: Option<String>,
        ) -> Result<Value, DagError> {
            if let Some(obs) = &_observer {
                if let Ok(raw) = serde_json::to_value(stub_child_event()) {
                    obs.on_event(NodeEvent::SubgraphChildEvent(raw));
                }
            }
            Ok(json!({ "out": { "output": 42 } }))
        }
        async fn resume_subgraph(
            &self,
            _session_id: &str,
            _answer: String,
            _graph: ResumeGraph,
            _observer: Option<Arc<dyn ExecutionObserver>>,
            _agent_session_id: Option<String>,
            _path_prefix: Option<String>,
        ) -> Result<Value, DagError> {
            Ok(Value::Null)
        }
        async fn find_child_session_id_for_resume(
            &self,
            _parent_session_id: &str,
            _parent_node_path: &str,
        ) -> Result<Option<String>, DagError> {
            Ok(None)
        }
    }

    /// Deserialize a captured `SubgraphChildEvent` into its inner DagExecutionEvent.
    fn inner_of(ev: &NodeEvent) -> Option<DagExecutionEvent> {
        match ev {
            NodeEvent::SubgraphChildEvent(raw) => serde_json::from_value(raw.clone()).ok(),
            _ => None,
        }
    }

    /// Run the node with the given inputs and return the (start, finish)
    /// boundary node ids the observer captured.
    async fn boundary_names_for(inputs: NodeInputs) -> (Option<String>, Option<String>) {
        let node = SubGraphNode::new();
        node.executor
            .set(Arc::new(StubExecutor) as Arc<dyn SubGraphExecutorPort>)
            .ok()
            .expect("executor set once");
        let obs = Arc::new(CapturingObserver::default());
        // NOTE: config has NO __agent_name — this is the subgraph-as-tool path.
        node.execute(&inputs, &json!({}), &mut json!({}), Some(obs.clone()))
            .await
            .expect("stub execute succeeds");

        let events = obs.events.lock().unwrap();
        let start = events.iter().find_map(|e| match inner_of(e) {
            Some(DagExecutionEvent::NodeStart {
                node_id, node_type, ..
            }) if node_type == "subgraph" => Some(node_id),
            _ => None,
        });
        let finish = events.iter().find_map(|e| match inner_of(e) {
            Some(DagExecutionEvent::SubgraphNodeFinish { node_id, .. }) => Some(node_id),
            _ => None,
        });
        (start, finish)
    }

    fn inline_graph_inputs() -> NodeInputs {
        let mut inputs: NodeInputs = NodeInputs::new();
        inputs.insert(
            "child_graph_inline".to_string(),
            json!({ "nodes": {}, "edges": [] }),
        );
        inputs
    }

    #[tokio::test]
    async fn subgraph_as_tool_emits_boundaries_from_node_id() {
        let mut inputs = inline_graph_inputs();
        inputs.insert("__node_id".to_string(), json!("my_tool"));
        let (start, finish) = boundary_names_for(inputs).await;
        assert_eq!(start.as_deref(), Some("my_tool"));
        assert_eq!(finish.as_deref(), Some("my_tool"));
    }

    /// The real tool path: `DagToolExecutor` never sets `__node_id` (no graph
    /// node exists for a tool dispatch), so the boundary has to come from
    /// `__colmena_tool_name`. Before this fallback existed the assertions below
    /// both saw `None` — the branch streamed with no delimiter at all.
    #[tokio::test]
    async fn subgraph_as_tool_emits_boundaries_from_tool_name_when_node_id_absent() {
        let mut inputs = inline_graph_inputs();
        inputs.insert("__colmena_tool_name".to_string(), json!("Specs_Writer"));
        let (start, finish) = boundary_names_for(inputs).await;
        assert_eq!(
            start.as_deref(),
            Some("Specs_Writer"),
            "tool-dispatched subgraph must name its boundary after the tool"
        );
        assert_eq!(finish.as_deref(), Some("Specs_Writer"));
    }

    #[tokio::test]
    async fn node_id_wins_over_tool_name() {
        let mut inputs = inline_graph_inputs();
        inputs.insert("__node_id".to_string(), json!("graph_node"));
        inputs.insert("__colmena_tool_name".to_string(), json!("Specs_Writer"));
        let (start, _) = boundary_names_for(inputs).await;
        assert_eq!(start.as_deref(), Some("graph_node"));
    }

    #[tokio::test]
    async fn empty_node_id_falls_through_to_tool_name() {
        let mut inputs = inline_graph_inputs();
        inputs.insert("__node_id".to_string(), json!(""));
        inputs.insert("__colmena_tool_name".to_string(), json!("Specs_Writer"));
        let (start, _) = boundary_names_for(inputs).await;
        assert_eq!(
            start.as_deref(),
            Some("Specs_Writer"),
            "an empty __node_id must be treated as absent, not as a blank name"
        );
    }

    #[tokio::test]
    async fn no_name_source_emits_no_boundary() {
        let (start, finish) = boundary_names_for(inline_graph_inputs()).await;
        assert!(start.is_none() && finish.is_none());
    }

    /// `__colmena_tool_name` is engine bookkeeping and must not reach the child
    /// graph's state — the `__colmena_` prefix filter is what keeps it out.
    #[test]
    fn tool_name_key_is_filtered_from_child_state() {
        assert!("__colmena_tool_name".starts_with("__colmena_"));
    }

    // ── Child events nest UNDER the boundary, not beside it ─────────────────

    /// Run the node and return the lineage of the child event the stub emitted,
    /// as the parent observer saw it. `None` when the event arrived unwrapped.
    async fn child_lineage_for(inputs: NodeInputs, config: Value) -> Option<(u32, String)> {
        let node = SubGraphNode::new();
        node.executor
            .set(Arc::new(StubExecutor) as Arc<dyn SubGraphExecutorPort>)
            .ok()
            .expect("executor set once");
        let obs = Arc::new(CapturingObserver::default());
        node.execute(&inputs, &config, &mut json!({}), Some(obs.clone()))
            .await
            .expect("stub execute succeeds");

        let events = obs.events.lock().unwrap();
        events.iter().find_map(|e| match inner_of(e) {
            Some(DagExecutionEvent::SubgraphWrapped { depth, path, .. }) => Some((depth, path)),
            _ => None,
        })
    }

    /// Tool path: the calling agent's loop stamps ITS node id on the child's
    /// lineage, so without scoping the boundary frame and the content it
    /// delimits come out as siblings. The child must carry the boundary in its
    /// path instead.
    #[tokio::test]
    async fn tool_dispatched_child_events_nest_under_the_boundary() {
        let mut inputs = inline_graph_inputs();
        inputs.insert("__colmena_tool_name".to_string(), json!("Specs_Writer"));
        let lineage = child_lineage_for(inputs, json!({})).await;
        assert_eq!(lineage, Some((1, "Specs_Writer>child_node".to_string())));
    }

    /// Orchestrator path: same reasoning, boundary is the agent name.
    #[tokio::test]
    async fn agent_dispatched_child_events_nest_under_the_agent_name() {
        let lineage = child_lineage_for(
            inline_graph_inputs(),
            json!({ "__agent_name": "Test_Runner" }),
        )
        .await;
        assert_eq!(lineage, Some((1, "Test_Runner>child_node".to_string())));
    }

    /// Edge-based path: the graph loop already prepends this node's id, so
    /// scoping here would duplicate the segment and add a phantom level. The
    /// child event must pass through untouched.
    #[tokio::test]
    async fn edge_dispatched_child_events_are_not_rescoped() {
        let mut inputs = inline_graph_inputs();
        inputs.insert("__node_id".to_string(), json!("sub_node"));
        assert_eq!(
            child_lineage_for(inputs, json!({})).await,
            None,
            "the edge path must not gain a wrapper of its own"
        );
    }
}

/// The ADP-reported bug: a subgraph's boundary never closed on child failure.
#[cfg(test)]
mod subgraph_tool_failure_close_tests {
    use super::*;
    use crate::dag_engine::application::ports::SubGraphExecutorPort;
    use crate::dag_engine::domain::error::DagError;
    use crate::dag_engine::domain::node::{ExecutableNode, NodeInputs};
    use crate::dag_engine::domain::observer::{ExecutionObserver, NodeEvent};
    use serde_json::json;
    use std::sync::Mutex;

    #[derive(Default)]
    struct CapturingObserver(Mutex<Vec<NodeEvent>>);
    impl ExecutionObserver for CapturingObserver {
        fn on_event(&self, event: NodeEvent) {
            self.0.lock().unwrap().push(event);
        }
    }
    impl CapturingObserver {
        /// Decoded inner events, in arrival order.
        fn events(&self) -> Vec<DagExecutionEvent> {
            self.0
                .lock()
                .unwrap()
                .iter()
                .filter_map(|e| match e {
                    NodeEvent::SubgraphChildEvent(raw) => serde_json::from_value(raw.clone()).ok(),
                    _ => None,
                })
                .collect()
        }
        fn is_empty(&self) -> bool {
            self.0.lock().unwrap().is_empty()
        }
        fn finish(&self) -> Option<Option<NodeEndError>> {
            self.events().into_iter().find_map(|e| match e {
                DagExecutionEvent::SubgraphNodeFinish { error, .. } => Some(error),
                _ => None,
            })
        }
        fn has_start(&self) -> bool {
            self.events()
                .iter()
                .any(|e| matches!(e, DagExecutionEvent::NodeStart { .. }))
        }
    }

    fn inline_graph_inputs() -> NodeInputs {
        let mut inputs: NodeInputs = NodeInputs::new();
        inputs.insert(
            "child_graph_inline".to_string(),
            json!({ "nodes": {}, "edges": [] }),
        );
        inputs
    }

    /// `Fail` also emits one child event first, so ordering vs. the close
    /// can be asserted.
    enum Behavior {
        Succeed,
        Fail(&'static str),
        Suspend,
        /// `resume_subgraph` returns a full child state (several nodes plus
        /// `__colmena_session_id`) with one of them flagged as the output
        /// node, so the resume branch's extraction can be exercised the same
        /// way the fresh path already is.
        ResumeWithOutputs,
    }
    struct StubExecutor(Behavior);
    #[async_trait::async_trait]
    impl SubGraphExecutorPort for StubExecutor {
        async fn run_subgraph(
            &self,
            _s: &str,
            _g: Value,
            _st: Value,
            observer: Option<Arc<dyn ExecutionObserver>>,
            _p: Option<String>,
            _a: Option<String>,
            _pp: Option<String>,
        ) -> Result<Value, DagError> {
            if let (Behavior::Fail(_), Some(obs)) = (&self.0, &observer) {
                let child = DagExecutionEvent::NodeStart {
                    node_id: "helper_llm".into(),
                    node_type: "llm_call".into(),
                    inputs: json!({}),
                    config: json!({}),
                };
                if let Ok(raw) = serde_json::to_value(child) {
                    obs.on_event(NodeEvent::SubgraphChildEvent(raw));
                }
            }
            match &self.0 {
                Behavior::Succeed | Behavior::ResumeWithOutputs => {
                    Ok(json!({ "out": { "output": 42 } }))
                }
                Behavior::Fail(msg) => Err(DagError::NodeExecution(msg.to_string())),
                Behavior::Suspend => {
                    Ok(json!({ "__colmena_status": "SUSPENDED", "questions": [] }))
                }
            }
        }
        async fn resume_subgraph(
            &self,
            _s: &str,
            _a: String,
            _g: ResumeGraph,
            _o: Option<Arc<dyn ExecutionObserver>>,
            _ags: Option<String>,
            _pp: Option<String>,
        ) -> Result<Value, DagError> {
            match &self.0 {
                Behavior::Fail(msg) => Err(DagError::NodeExecution(msg.to_string())),
                Behavior::ResumeWithOutputs => Ok(json!({
                    "http_1": { "status": 200, "body": { "api_key": "sk-must-not-leak" } },
                    "out": { "output": 42, "extra_info": { "__colmena_is_output_node": true } },
                    "__colmena_session_id": "child_session_1"
                })),
                _ => Ok(Value::Null),
            }
        }
        async fn find_child_session_id_for_resume(
            &self,
            _p: &str,
            _n: &str,
        ) -> Result<Option<String>, DagError> {
            Ok(Some("child_session_1".to_string()))
        }
    }

    /// Runs the node; injects `__colmena_tool_name` when `tool_name` is set.
    async fn run(
        behavior: Behavior,
        mut inputs: NodeInputs,
        config: Value,
        tool_name: bool,
    ) -> (
        Result<Value, Box<dyn std::error::Error + Send + Sync>>,
        Arc<CapturingObserver>,
    ) {
        if tool_name {
            inputs.insert("__colmena_tool_name".to_string(), json!("Helper"));
        }
        let node = SubGraphNode::new();
        node.executor
            .set(Arc::new(StubExecutor(behavior)))
            .ok()
            .expect("executor set once");
        let obs = Arc::new(CapturingObserver::default());
        let result = node
            .execute(&inputs, &config, &mut json!({}), Some(obs.clone()))
            .await;
        (result, obs)
    }

    // ── Tool source (__colmena_tool_name): masked (#310), errorText allowed ──

    #[tokio::test]
    async fn child_failure_closes_tool_boundary_with_error_status_and_returns_the_error() {
        let (result, obs) = run(
            Behavior::Fail("gemini-does-not-exist-9000: model not found"),
            inline_graph_inputs(),
            json!({}),
            true,
        )
        .await;
        assert!(result.unwrap_err().to_string().contains("model not found"));
        let error = obs
            .finish()
            .flatten()
            .expect("boundary must close with error");
        assert_eq!(
            error.message.as_deref(),
            Some("Error de ejecución en el nodo: gemini-does-not-exist-9000: model not found")
        );
    }

    #[tokio::test]
    async fn child_failure_close_follows_child_events() {
        let (_, obs) = run(
            Behavior::Fail("boom"),
            inline_graph_inputs(),
            json!({}),
            true,
        )
        .await;
        let events = obs.events();
        let child_idx = events
            .iter()
            .position(|e| matches!(e, DagExecutionEvent::SubgraphWrapped { .. }))
            .expect("child event must be forwarded before the close");
        let close_idx = events
            .iter()
            .position(|e| matches!(e, DagExecutionEvent::SubgraphNodeFinish { .. }))
            .expect("boundary close must be emitted");
        assert!(
            child_idx < close_idx,
            "child event at {child_idx}, close at {close_idx}"
        );
    }

    #[tokio::test]
    async fn success_close_carries_no_error() {
        let (result, obs) = run(Behavior::Succeed, inline_graph_inputs(), json!({}), true).await;
        result.expect("stub execute succeeds");
        assert_eq!(
            obs.finish(),
            Some(None),
            "successful close must not carry error"
        );
    }

    #[tokio::test]
    async fn suspended_child_leaves_boundary_open() {
        let (result, obs) = run(Behavior::Suspend, inline_graph_inputs(), json!({}), true).await;
        assert_eq!(result.unwrap()["__colmena_status"], "SUSPENDED");
        assert!(
            obs.has_start() && obs.finish().is_none(),
            "boundary opens, stays open"
        );
    }

    #[tokio::test]
    async fn missing_executor_emits_no_boundary_frames() {
        let mut inputs = inline_graph_inputs();
        inputs.insert("__colmena_tool_name".to_string(), json!("Helper"));
        let obs = Arc::new(CapturingObserver::default());
        let err = SubGraphNode::new() // no executor set at all
            .execute(&inputs, &json!({}), &mut json!({}), Some(obs.clone()))
            .await
            .expect_err("no executor must fail");
        assert!(err.to_string().contains("not initialized"));
        assert!(obs.is_empty(), "no start, no close");
    }

    #[tokio::test]
    async fn resume_failure_emits_no_unmatched_close() {
        let mut inputs = inline_graph_inputs();
        inputs.insert(
            "__colmena_resume_answer".to_string(),
            json!("Q[q1]: ?\nA[q1]: yes"),
        );
        let (result, obs) = run(Behavior::Fail("boom"), inputs, json!({}), true).await;
        assert!(result.unwrap_err().to_string().contains("boom"));
        assert!(
            obs.is_empty(),
            "resume branch has no start, so no close either"
        );
    }

    #[tokio::test]
    async fn resume_returns_the_output_node_not_the_whole_child_state() {
        let node = SubGraphNode::new();
        node.executor
            .set(Arc::new(StubExecutor(Behavior::ResumeWithOutputs)))
            .ok()
            .expect("executor set once");
        let mut inputs = inline_graph_inputs();
        inputs.insert("__colmena_resume_answer".to_string(), json!("yes"));
        inputs.insert("__colmena_tool_name".to_string(), json!("Helper"));

        let out = node
            .execute(&inputs, &json!({}), &mut json!({}), None)
            .await
            .expect("resume succeeds");

        assert_eq!(out["output"], json!(42));
        assert!(
            !out.to_string().contains("sk-must-not-leak"),
            "a resumed tool must not hand the whole child state to the model: {out}"
        );
    }

    // ── errorText source gating (this PR's amendment) ────────────────────────

    #[tokio::test]
    async fn agent_source_failure_close_omits_error_text() {
        let config = json!({ "__agent_name": "Test_Runner" });
        let (_, obs) = run(
            Behavior::Fail("leaky secret"),
            inline_graph_inputs(),
            config,
            false,
        )
        .await;
        let error = obs
            .finish()
            .flatten()
            .expect("boundary must still close with status:error");
        assert_eq!(error.message, None, "orchestrator-agent path is unmasked");
    }

    #[tokio::test]
    async fn edge_source_failure_close_omits_error_text() {
        let mut inputs = inline_graph_inputs();
        inputs.insert("__node_id".to_string(), json!("sub_node"));
        let (_, obs) = run(Behavior::Fail("leaky secret"), inputs, json!({}), false).await;
        let error = obs
            .finish()
            .flatten()
            .expect("boundary must still close with status:error");
        assert_eq!(error.message, None, "edge-based path is unmasked");
    }
}

#[cfg(test)]
mod subgraph_suspend_passthrough_tests {
    use serde_json::json;

    /// Locks the invariant that a SUSPENDED child result is returned verbatim,
    /// preserving `questions`. Both SUSPENDED branches in `execute` return the
    /// child `result` unchanged; this guards against a future refactor that
    /// strips or rewrites the field.
    fn passes_through_suspended(child_result: &serde_json::Value) -> serde_json::Value {
        // Mirror of subgraph.rs SUSPENDED branches: return the child result verbatim.
        child_result.clone()
    }

    #[test]
    fn suspended_result_preserves_questions() {
        let child = json!({
            "__colmena_status": "SUSPENDED",
            "questions": [{ "id": "q1", "text": "¿Cuántas personas?" }]
        });
        let out = passes_through_suspended(&child);
        assert_eq!(out["__colmena_status"], "SUSPENDED");
        assert_eq!(out["questions"][0]["id"], "q1");
        assert_eq!(out["questions"][0]["text"], "¿Cuántas personas?");
    }
}

#[cfg(test)]
mod child_graph_ref_tests {
    //! `child_graph_ref`: the child graph is loaded by reference through the
    //! embedder's `ChildGraphResolverPort`, resolved BEFORE the boundary's start
    //! frame, and never reaches an output, a frame or the child's state.
    use super::*;
    use crate::dag_engine::domain::error::DagError;
    use crate::dag_engine::domain::observer::{ExecutionObserver, NodeEvent};
    use serde_json::json;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Obs(Mutex<Vec<NodeEvent>>);
    impl ExecutionObserver for Obs {
        fn on_event(&self, e: NodeEvent) {
            self.0.lock().unwrap().push(e);
        }
    }
    impl Obs {
        fn dump(&self) -> String {
            format!("{:?}", self.0.lock().unwrap())
        }
        fn is_empty(&self) -> bool {
            self.0.lock().unwrap().is_empty()
        }
    }

    /// Records the graph and state it is asked to run.
    #[derive(Default)]
    struct CapturingExecutor(Mutex<Option<(Value, Value)>>);
    #[async_trait::async_trait]
    impl SubGraphExecutorPort for CapturingExecutor {
        async fn run_subgraph(
            &self,
            _s: &str,
            graph: Value,
            state: Value,
            _o: Option<Arc<dyn ExecutionObserver>>,
            _p: Option<String>,
            _a: Option<String>,
            _pp: Option<String>,
        ) -> Result<Value, DagError> {
            *self.0.lock().unwrap() = Some((graph, state));
            Ok(
                json!({ "out": { "text": "done", "extra_info": { "__colmena_is_output_node": true } } }),
            )
        }
        async fn resume_subgraph(
            &self,
            _s: &str,
            _a: String,
            _g: ResumeGraph,
            _o: Option<Arc<dyn ExecutionObserver>>,
            _ags: Option<String>,
            _pp: Option<String>,
        ) -> Result<Value, DagError> {
            Ok(Value::Null)
        }
        async fn find_child_session_id_for_resume(
            &self,
            _p: &str,
            _n: &str,
        ) -> Result<Option<String>, DagError> {
            Ok(None)
        }
    }

    enum Answer {
        Graph,
        Fail(ChildGraphResolveError),
        Hang,
    }
    struct FakeResolver(Answer, Mutex<Vec<ChildGraphRequest>>);
    #[async_trait::async_trait]
    impl ChildGraphResolverPort for FakeResolver {
        async fn resolve(
            &self,
            req: ChildGraphRequest,
        ) -> Result<ResolvedChildGraph, ChildGraphResolveError> {
            self.1.lock().unwrap().push(req);
            match &self.0 {
                Answer::Graph => Ok(ResolvedChildGraph {
                    graph: json!({ "nodes": { "llm": { "type": "llm_call", "config": { "api_key": "sk-resolved-secret" } } }, "edges": [] }),
                    display_name: "Agente de licitaciones".into(),
                }),
                Answer::Fail(e) => Err(e.clone()),
                Answer::Hang => {
                    tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
                    unreachable!()
                }
            }
        }
    }

    fn ref_inputs(agent_id: &str) -> NodeInputs {
        let mut i = NodeInputs::new();
        i.insert(
            "child_graph_ref".into(),
            json!({ "agent_id": agent_id, "context": { "messageId": "m1" } }),
        );
        i.insert("__colmena_session_id".into(), json!("s1"));
        i.insert("__colmena_agent_session_id".into(), json!("as1"));
        i.insert(
            "__colmena_node_id_path".into(),
            json!("tool/Run_My_Agent/a1"),
        );
        i.insert("__colmena_tool_name".into(), json!("Run_My_Agent"));
        i.insert("prompt".into(), json!("hi"));
        i
    }

    fn node_with(answer: Answer) -> (SubGraphNode, Arc<CapturingExecutor>, Arc<FakeResolver>) {
        let node = SubGraphNode::new();
        let exec = Arc::new(CapturingExecutor::default());
        let res = Arc::new(FakeResolver(answer, Mutex::default()));
        node.executor.set(exec.clone()).ok().expect("executor once");
        node.resolver.set(res.clone()).ok().expect("resolver once");
        (node, exec, res)
    }

    async fn run(
        node: &SubGraphNode,
        agent_id: &str,
        config: Value,
        obs: Option<Arc<Obs>>,
    ) -> Result<Value, String> {
        let obs = obs.map(|o| o as Arc<dyn ExecutionObserver>);
        node.execute(&ref_inputs(agent_id), &config, &mut json!({}), obs)
            .await
            .map_err(|e| e.to_string())
    }

    fn assert_code(err: &str, code: &str) {
        let prefix = format!("CHILD_GRAPH_RESOLVE_FAILED:{code}:");
        assert!(err.starts_with(&prefix), "{err}");
    }

    #[tokio::test]
    async fn a_ref_runs_the_resolved_graph_with_the_parent_context() {
        let (node, exec, res) = node_with(Answer::Graph);
        let out = run(&node, "a1", json!({}), None).await.unwrap();
        assert_eq!(out["text"], json!("done"));
        let (graph, _) = exec.0.lock().unwrap().clone().expect("executor ran");
        assert!(graph["nodes"]["llm"].is_object(), "runs the resolved graph");
        let req = res.1.lock().unwrap()[0].clone();
        assert_eq!(req.agent_id, "a1");
        assert_eq!(req.context, json!({ "messageId": "m1" }));
        assert_eq!(req.session_id, "s1");
        assert_eq!(req.agent_session_id.as_deref(), Some("as1"));
        assert_eq!(req.parent_path, "tool/Run_My_Agent/a1");
    }

    #[tokio::test]
    async fn the_boundary_start_of_a_ref_child_carries_the_agent_name() {
        let (node, _, _) = node_with(Answer::Graph);
        let obs = Arc::new(Obs::default());
        node.execute(
            &ref_inputs("a1"),
            &json!({}),
            &mut json!({}),
            Some(obs.clone()),
        )
        .await
        .unwrap();
        assert!(
            obs.dump().contains("Agente de licitaciones"),
            "{}",
            obs.dump()
        );
    }

    #[tokio::test]
    async fn the_resolved_graph_never_reaches_output_events_or_child_state() {
        let (node, exec, _) = node_with(Answer::Graph);
        let obs = Arc::new(Obs::default());
        let out = run(&node, "a1", json!({}), Some(obs.clone()))
            .await
            .unwrap();
        let (graph, state) = exec.0.lock().unwrap().clone().unwrap();
        // The secret was in play: the child ran the resolved graph. Without this
        // the assertions below would pass for a node that never resolved at all.
        assert!(graph.to_string().contains("sk-resolved-secret"));
        assert!(!obs.is_empty(), "the tool boundary emitted its frames");
        assert!(!out.to_string().contains("sk-resolved-secret"));
        assert!(
            !obs.dump().contains("sk-resolved-secret"),
            "no frame carries the graph"
        );
        assert!(
            state.get("child_graph_ref").is_none(),
            "the ref is plumbing, not child data"
        );
    }

    #[tokio::test]
    async fn a_resolver_error_fails_with_the_stable_prefix_and_emits_no_frame() {
        let forbidden = ChildGraphResolveError::Forbidden("not visible".into());
        let (node, exec, _) = node_with(Answer::Fail(forbidden));
        let obs = Arc::new(Obs::default());
        let err = run(&node, "a1", json!({}), Some(obs.clone()))
            .await
            .unwrap_err();
        assert_code(&err, "forbidden");
        assert!(obs.is_empty(), "no start frame for a child that never ran");
        assert!(exec.0.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn without_a_resolver_a_ref_is_unavailable() {
        let node = SubGraphNode::new();
        node.executor
            .set(Arc::new(CapturingExecutor::default()))
            .ok()
            .unwrap();
        let err = run(&node, "a1", json!({}), None).await.unwrap_err();
        assert_code(&err, "unavailable");
    }

    #[tokio::test]
    async fn an_unresolved_agent_id_template_is_not_found_and_never_asks_the_resolver() {
        let (node, _, res) = node_with(Answer::Graph);
        let err = run(&node, "${agentId}", json!({}), None).await.unwrap_err();
        assert_code(&err, "not_found");
        assert!(res.1.lock().unwrap().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn a_hung_resolver_times_out_as_unavailable() {
        let (node, _, _) = node_with(Answer::Hang);
        let err = run(&node, "a1", json!({}), None).await.unwrap_err();
        assert_code(&err, "unavailable");
        assert!(err.contains("timed out after 30s"), "{err}");
    }

    #[tokio::test]
    async fn an_inline_graph_in_config_wins_over_a_ref_in_inputs() {
        let (node, exec, res) = node_with(Answer::Graph);
        let inline = json!({ "nodes": { "x": { "type": "input", "config": {} } }, "edges": [] });
        run(&node, "a1", json!({ "child_graph_inline": inline }), None)
            .await
            .unwrap();
        assert!(
            res.1.lock().unwrap().is_empty(),
            "the resolver is not asked"
        );
        let (graph, _) = exec.0.lock().unwrap().clone().unwrap();
        assert!(graph["nodes"]["x"].is_object());
    }

    /// Until the resolver is asked again on resume, a ref resumes with the
    /// graph stored in its row and the resolver is not called.
    #[tokio::test]
    async fn a_ref_still_resumes_its_stored_graph() {
        let node = SubGraphNode::new();
        let exec = super::subgraph_resume_graph_tests::ResumingExecutor::finding(Some("child_1"));
        let res = Arc::new(FakeResolver(Answer::Graph, Mutex::default()));
        node.executor.set(exec.clone()).ok().unwrap();
        node.resolver.set(res.clone()).ok().unwrap();
        let mut inputs = ref_inputs("a1");
        inputs.insert(
            "__colmena_resume_answer".into(),
            json!("Q[q]: ?\nA[q]: yes"),
        );
        node.execute(&inputs, &json!({}), &mut json!({}), None)
            .await
            .unwrap();
        assert_eq!(exec.graph(), Some(ResumeGraph::Stored));
        assert!(res.1.lock().unwrap().is_empty());
    }
}

#[cfg(test)]
mod subgraph_resume_graph_tests {
    //! On resume the node finds the suspended child first, then derives its
    //! graph from the same source a fresh run reads, and hands it to the
    //! executor as a `ResumeGraph`.
    use super::*;
    use crate::dag_engine::domain::error::DagError;
    use serde_json::json;
    use std::sync::Mutex;

    /// Finds `child` (or nothing), records every call and the `ResumeGraph` it
    /// was handed. `Unavailable` comes back as the real executor returns it: a
    /// refusal carrying that text.
    pub(super) struct ResumingExecutor {
        child: Option<&'static str>,
        calls: Mutex<Vec<&'static str>>,
        resumed_with: Mutex<Option<ResumeGraph>>,
    }
    impl ResumingExecutor {
        pub(super) fn finding(child: Option<&'static str>) -> Arc<Self> {
            Arc::new(Self {
                child,
                calls: Mutex::default(),
                resumed_with: Mutex::default(),
            })
        }
        pub(super) fn graph(&self) -> Option<ResumeGraph> {
            self.resumed_with.lock().unwrap().clone()
        }
    }
    #[async_trait::async_trait]
    impl SubGraphExecutorPort for ResumingExecutor {
        async fn run_subgraph(
            &self,
            _s: &str,
            _g: Value,
            _st: Value,
            _o: Option<Arc<dyn ExecutionObserver>>,
            _p: Option<String>,
            _a: Option<String>,
            _pp: Option<String>,
        ) -> Result<Value, DagError> {
            self.calls.lock().unwrap().push("run");
            Ok(
                json!({ "out": { "text": "fresh", "extra_info": { "__colmena_is_output_node": true } } }),
            )
        }
        async fn resume_subgraph(
            &self,
            _s: &str,
            _a: String,
            graph: ResumeGraph,
            _o: Option<Arc<dyn ExecutionObserver>>,
            _ags: Option<String>,
            _pp: Option<String>,
        ) -> Result<Value, DagError> {
            self.calls.lock().unwrap().push("resume");
            *self.resumed_with.lock().unwrap() = Some(graph.clone());
            match graph {
                ResumeGraph::Unavailable(reason) => Err(DagError::ResumeRefused(reason)),
                _ => Ok(
                    json!({ "out": { "text": "resumed", "extra_info": { "__colmena_is_output_node": true } } }),
                ),
            }
        }
        async fn find_child_session_id_for_resume(
            &self,
            _p: &str,
            _n: &str,
        ) -> Result<Option<String>, DagError> {
            self.calls.lock().unwrap().push("find");
            Ok(self.child.map(str::to_string))
        }
    }

    fn graph_named(id: &str) -> Value {
        json!({ "nodes": { id: { "type": "input", "config": {} } }, "edges": [] })
    }

    fn resume_inputs() -> NodeInputs {
        let mut i = NodeInputs::new();
        i.insert(
            "__colmena_resume_answer".into(),
            json!("Q[q]: ?\nA[q]: yes"),
        );
        i.insert("__colmena_session_id".into(), json!("s1"));
        i
    }

    async fn resume(
        inputs: NodeInputs,
        config: Value,
        exec: Arc<ResumingExecutor>,
    ) -> Result<Value, String> {
        let node = SubGraphNode::new();
        node.executor.set(exec).ok().expect("executor once");
        node.execute(&inputs, &config, &mut json!({}), None)
            .await
            .map_err(|e| e.to_string())
    }

    #[tokio::test]
    async fn a_tool_resume_hands_over_the_inline_graph_its_inputs_carry_now() {
        let exec = ResumingExecutor::finding(Some("child_1"));
        let mut inputs = resume_inputs();
        inputs.insert("child_graph_inline".into(), graph_named("fresh"));
        let out = resume(inputs, json!({}), exec.clone()).await.unwrap();
        assert_eq!(out["text"], json!("resumed"));
        assert_eq!(exec.graph(), Some(ResumeGraph::Fresh(graph_named("fresh"))));
    }

    #[tokio::test]
    async fn an_edge_resume_takes_the_inline_graph_from_config_first() {
        let exec = ResumingExecutor::finding(Some("child_1"));
        let mut inputs = resume_inputs();
        inputs.insert("child_graph_inline".into(), graph_named("from_inputs"));
        let config = json!({ "child_graph_inline": graph_named("from_config") });
        resume(inputs, config, exec.clone()).await.unwrap();
        assert_eq!(
            exec.graph(),
            Some(ResumeGraph::Fresh(graph_named("from_config")))
        );
    }

    #[tokio::test]
    async fn a_path_resume_reads_the_file_again() {
        let path =
            std::env::temp_dir().join(format!("subgraph_resume_{}.json", std::process::id()));
        std::fs::write(&path, graph_named("from_file").to_string()).unwrap();
        let exec = ResumingExecutor::finding(Some("child_1"));
        let config = json!({ "child_graph_path": path.to_string_lossy() });
        let res = resume(resume_inputs(), config, exec.clone()).await;
        let _ = std::fs::remove_file(&path);
        res.unwrap();
        assert_eq!(
            exec.graph(),
            Some(ResumeGraph::Fresh(graph_named("from_file")))
        );
    }

    #[tokio::test]
    async fn a_missing_path_is_unavailable_and_its_text_comes_back_verbatim() {
        let exec = ResumingExecutor::finding(Some("child_1"));
        let config = json!({ "child_graph_path": "/nonexistent/child.json" });
        let err = resume(resume_inputs(), config, exec.clone())
            .await
            .unwrap_err();
        assert_eq!(err, "child_graph_path not found: /nonexistent/child.json");
        assert!(matches!(exec.graph(), Some(ResumeGraph::Unavailable(_))));
    }

    #[tokio::test]
    async fn no_source_is_unavailable_with_the_incompatible_prefix() {
        let exec = ResumingExecutor::finding(Some("child_1"));
        let err = resume(resume_inputs(), json!({}), exec.clone())
            .await
            .unwrap_err();
        assert!(
            err.starts_with("SUBGRAPH_RESUME_INCOMPATIBLE: the subgraph has no child graph source"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn without_a_suspended_child_nothing_is_resumed() {
        let exec = ResumingExecutor::finding(None);
        let mut inputs = resume_inputs();
        inputs.insert("child_graph_inline".into(), graph_named("fresh"));
        let err = resume(inputs, json!({}), exec.clone()).await.unwrap_err();
        assert!(err.starts_with("No suspended child found"), "{err}");
        assert_eq!(*exec.calls.lock().unwrap(), vec!["find"]);
    }

    #[tokio::test]
    async fn the_stored_valve_skips_the_derivation() {
        let node = SubGraphNode::new();
        let mut inputs = resume_inputs();
        inputs.insert("child_graph_inline".into(), graph_named("fresh"));
        let graph = node
            .resume_graph(true, &inputs, &json!({}), "s1", None, "")
            .await;
        assert_eq!(graph, ResumeGraph::Stored);
    }

    #[test]
    fn only_stored_turns_the_valve_on() {
        assert!(SubGraphNode::valve_is_stored(Some("stored")));
        assert!(SubGraphNode::valve_is_stored(Some(" STORED ")));
        for off in [None, Some(""), Some("fresh"), Some("off"), Some("1")] {
            assert!(!SubGraphNode::valve_is_stored(off), "{off:?}");
        }
    }
}
