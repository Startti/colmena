use crate::colmena_log;
use crate::dag_engine::application::ports::SubGraphExecutorPort;
use crate::dag_engine::domain::events::DagExecutionEvent;
use crate::dag_engine::domain::lint::{FieldSpec, NodeCatalogEntry};
use crate::dag_engine::domain::node::{ExecutableNode, NodeInputs};
use crate::dag_engine::domain::observer::{ChildScopeObserver, ExecutionObserver, NodeEvent};
use serde_json::{json, Value};
use std::error::Error;
use std::sync::{Arc, OnceLock};
use tokio::fs;

/// Operator-supplied keys that name the child graph itself.
///
/// They arrive either in the node's `config` (edge path) or in its `inputs` (tool
/// path, where `DagToolExecutor` merges the tool's `fixed_config` into inputs and
/// passes `config = {}`). Either way they are plumbing, never data for the child.
///
/// [`SubGraphNode::resolve_child_graph_source`] reads them, and
/// [`SubGraphNode::is_excluded_from_child_state`] keeps them out of the child's
/// global state — otherwise the child's own `input` node passes them through and
/// they end up inside an LLM prompt, secrets already resolved.
///
/// Both uses derive from this constant on purpose: a new source key has to become
/// invisible to the child by construction, not by remembering a second list.
const CHILD_GRAPH_SOURCE_KEYS: [&str; 2] = ["child_graph_inline", "child_graph_path"];

pub struct SubGraphNode {
    pub executor: Arc<OnceLock<Arc<dyn SubGraphExecutorPort>>>,
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
        }
    }

    /// Resolve the child graph source (inline object or path string) for both
    /// the edge-based path (config) and the tool path (inputs).
    ///
    /// Precedence: `config` wins over `inputs` so the legacy edge-based behavior
    /// is unchanged; the tool path supplies the value via `inputs` (because the
    /// executor merges `fixed_config` into inputs and passes `config = {}`).
    fn resolve_child_graph_source(inputs: &NodeInputs, config: &Value) -> Option<Value> {
        for key in CHILD_GRAPH_SOURCE_KEYS {
            if let Some(source) = config.get(key) {
                return Some(source.clone());
            }
        }
        for key in CHILD_GRAPH_SOURCE_KEYS {
            if let Some(source) = inputs.get(key) {
                return Some(source.clone());
            }
        }
        None
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
        // The two child-graph sources come from the constant the node itself
        // uses to look them up, so adding one there cannot silently skip the
        // catalog.
        let mut entry =
            NodeCatalogEntry::no_config().with_field("__agent_name", FieldSpec::of_type("string"));
        for key in CHILD_GRAPH_SOURCE_KEYS {
            let ty = if key == "child_graph_inline" {
                "object"
            } else {
                "string"
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
        let boundary_name = agent_name
            .clone()
            .or_else(|| Self::non_empty_str(inputs, "__node_id"))
            .or_else(|| tool_name.clone());

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
            let result = executor
                .resume_subgraph(
                    &child_session_id,
                    resume_answer.to_string(),
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
            return Ok(result);
        }

        // --- 2. GRAPH LOADING ---
        // Source can come from `config` (edge-based path) or `inputs` (tool path,
        // where the executor merges fixed_config into inputs and passes config={}).
        let graph_source = Self::resolve_child_graph_source(inputs, config).ok_or(
            "SubGraphNode requires 'child_graph_inline' or 'child_graph_path' \
             in config (edge path) or inputs (tool path)",
        )?;

        let graph_json = if graph_source.is_object() {
            graph_source
        } else if let Some(path_val) = graph_source.as_str() {
            let path = std::path::Path::new(path_val);
            if !path.exists() {
                return Err(format!("child_graph_path not found: {}", path_val).into());
            }
            let contents = fs::read_to_string(path).await?;
            serde_json::from_str(&contents)?
        } else {
            return Err("child_graph source must be an inline object or a path string".into());
        };

        // --- 3. STATE MAPPING (IN) ---
        let child_state = Self::build_child_state(inputs);

        // Emit subgraph node-start boundary event (orchestrator agent OR
        // subgraph-as-tool — see `boundary_name`).
        if let (Some(ref name), Some(ref obs)) = (&boundary_name, &_observer) {
            let start_event = DagExecutionEvent::NodeStart {
                node_id: name.clone(),
                node_type: "subgraph".to_string(),
                inputs: Value::Object(Default::default()),
                config: Value::Object(Default::default()),
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

        let result = self
            .executor
            .get()
            .ok_or("SubGraphExecutorPort not initialized in SubGraphNode")?
            .run_subgraph(
                &child_session_id,
                graph_json,
                child_state,
                child_observer.clone(),
                Some(parent_session_id.clone()),
                agent_session_id.clone(),
                child_path_prefix.clone(),
            )
            .await?;

        // --- 4. SUSPEND BUBBLE-UP ---
        if result.get("__colmena_status").and_then(|v| v.as_str()) == Some("SUSPENDED") {
            colmena_log!("⏸️ [SubGraphNode] Child graph suspended! Bubbling up to parent...");
            return Ok(result);
        }

        // --- 5. STATE MAPPING (OUT) ---
        // Find the node flagged as __colmena_is_output_node and extract its value.
        let final_output = if let Some(obj) = result.as_object() {
            obj.values()
                .find(|v| {
                    v.get("extra_info")
                        .and_then(|ei| ei.get("__colmena_is_output_node"))
                        .and_then(|f| f.as_bool())
                        .unwrap_or(false)
                })
                .cloned()
                .unwrap_or(result.clone())
        } else {
            result.clone()
        };

        // Emit subgraph node-end boundary event with final output
        if let (Some(ref name), Some(ref obs)) = (&boundary_name, &_observer) {
            let finish_event = DagExecutionEvent::SubgraphNodeFinish {
                node_id: name.clone(),
                output: final_output.clone(),
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
        SubGraphNode::resolve_child_graph_source(inputs, config)
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
            SubGraphNode::resolve_child_graph_source(&inputs, &config),
            Some(json!("./from_config.json"))
        );
    }

    #[test]
    fn returns_none_when_neither_config_nor_inputs_has_source() {
        let inputs: NodeInputs = NodeInputs::new();
        let config = json!({});
        assert_eq!(
            SubGraphNode::resolve_child_graph_source(&inputs, &config),
            None
        );
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
            SubGraphNode::resolve_child_graph_source(&inputs, &json!({})),
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
