use crate::colmena_log;
use crate::dag_engine::application::liveness::LivenessSettings;
use crate::dag_engine::application::ports::{NodeRegistryPort, ResumeGraph, SubGraphExecutorPort};
use crate::dag_engine::application::preflight;
use crate::dag_engine::application::secure_value_service::{MaskingObserver, SecureValueService};
use crate::dag_engine::domain::error::DagError;
use crate::dag_engine::domain::graph::{Edge, Graph};
use crate::dag_engine::domain::graph_skeleton::{GraphSkeleton, SUBGRAPH_RESUME_INCOMPATIBLE};
use crate::dag_engine::domain::node::NodeInputs;

use serde_json::{json, Value};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use crate::dag_engine::domain::state::{DagRunState, DagRunStatus, DagStateRepository};

/// El "Caso de Uso" que orquesta la ejecución de un grafo.
#[derive(Clone)]
pub struct DagRunUseCase {
    registry: Arc<dyn NodeRegistryPort>,
    state_repository: Option<Arc<dyn DagStateRepository>>,
    secure_value_service: Option<Arc<SecureValueService>>,
    liveness: LivenessSettings,
    /// Starting `global_shared_state` handed to a child run in memory.
    ///
    /// `run_subgraph` persists the child's starting state and `execute_stream`
    /// reads it back through the state repository. That round-trip is also the
    /// only thing carrying `__colmena_subgraph_depth` into the child, because
    /// the child-state builder in `SubGraphNode` strips every `__colmena_*` key
    /// before handing the map over.
    ///
    /// With no state repository configured nothing is persisted and nothing is
    /// read back, so the depth silently reset to 0 at each subgraph boundary and
    /// the recursion ceiling stopped applying — no warning, no error. Seeding in
    /// memory makes the handover independent of persistence.
    seed_state: Option<Value>,
    /// Set only by `run_subgraph`/`resume_subgraph` on the cloned child use
    /// case — never derived from `path_prefix`. Gates the close-on-error
    /// below; a root run's failing node stays closed only by `error`.
    nested_run: bool,
}

impl DagRunUseCase {
    pub fn new(
        registry: Arc<dyn NodeRegistryPort>,
        state_repository: Option<Arc<dyn DagStateRepository>>,
    ) -> Self {
        Self {
            registry,
            state_repository,
            secure_value_service: None,
            liveness: LivenessSettings::default(),
            seed_state: None,
            nested_run: false,
        }
    }

    /// Creates a new DagRunUseCase with a pre-built SecureValueService (shared with the registry).
    ///
    /// Esta es la única vía oficial para inyectar secure values. La aplicación
    /// no debe tocar `infrastructure/`: el caller construye el adapter
    /// concreto (`PostgresSecureValueRepository`) y arma el service afuera.
    pub fn with_secure_values_and_service(
        registry: Arc<dyn NodeRegistryPort>,
        state_repository: Option<Arc<dyn DagStateRepository>>,
        secure_value_service: Arc<SecureValueService>,
    ) -> Self {
        Self {
            registry,
            state_repository,
            secure_value_service: Some(secure_value_service),
            liveness: LivenessSettings::default(),
            seed_state: None,
            nested_run: false,
        }
    }

    /// Folds an in-memory seed into a run's starting `global_shared_state`.
    ///
    /// Keys already present win: a resumed run's persisted state is newer than
    /// whatever seed its parent handed over. When a state repository IS
    /// configured this is a no-op, because the persisted state that was just
    /// read back already contains the same entries.
    fn fold_seed_state(state: &mut Value, seed: Option<Value>) {
        let (Some(Value::Object(seed)), Some(obj)) = (seed, state.as_object_mut()) else {
            return;
        };
        for (k, v) in seed {
            obj.entry(k).or_insert(v);
        }
    }

    /// Seeds the starting `global_shared_state` for the next run, in memory.
    ///
    /// Used by `run_subgraph` so a child inherits its parent's ambient state
    /// (notably `__colmena_subgraph_depth`) without depending on a database
    /// round-trip. See [`Self::seed_state`].
    pub fn with_seed_state(mut self, state: Value) -> Self {
        self.seed_state = Some(state);
        self
    }

    /// Marks this use case as running a nested (child) graph.
    #[allow(clippy::wrong_self_convention)] // "treat self as a nested run", not a type conversion
    pub(crate) fn as_nested_run(mut self) -> Self {
        self.nested_run = true;
        self
    }

    /// Overrides the liveness knobs (heartbeat / idle watchdog). Callers that
    /// build the use case directly get `LivenessSettings::default()`.
    pub fn with_liveness(mut self, liveness: LivenessSettings) -> Self {
        self.liveness = liveness;
        self
    }

    /// Evaluates if a node has exceeded its call limits
    fn check_limits(
        node_id: &str,
        caller_id: Option<&str>,
        graph: &Graph,
        global_calls: &mut HashMap<String, u32>,
        caller_specific_calls: &mut HashMap<String, HashMap<String, u32>>,
    ) -> Result<(), DagError> {
        if let Some(node_config) = graph.nodes.get(node_id) {
            let current_global = *global_calls.get(node_id).unwrap_or(&0);
            if let Some(max_global) = node_config.max_total_calls {
                if current_global >= max_global {
                    return Err(DagError::NodeExecution(format!(
                        "Node {} reached max_total_calls limit of {}",
                        node_id, max_global
                    )));
                }
            }

            if let Some(caller) = caller_id {
                if let Some(limits_from) = &node_config.max_calls_from {
                    if let Some(limit) = limits_from.get(caller) {
                        let current_specific = caller_specific_calls
                            .get(caller)
                            .and_then(|m| m.get(node_id))
                            .copied()
                            .unwrap_or(0);
                        if current_specific >= *limit {
                            return Err(DagError::NodeExecution(format!(
                                "Node {} reached max_calls_from limit of {} from caller {}",
                                node_id, limit, caller
                            )));
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Método principal que ejecuta el grafo (Bloqueante).
    pub async fn execute(
        &self,
        _graph: Graph,
        _resume_session_id: Option<String>,
        _resume_answer: Option<String>,
        _include_extra_info: bool,
    ) -> Result<Value, DagError> {
        // Collect state from stream output (For simplicity, `execute` relies upon `execute_stream`)
        unimplemented!("execute() is deprecated! Call execute_stream() and drain it wrapper style if needed. (Colmena single-turn API actually consumes execute_stream directly now).");
    }

    /// Recursively strips `extra_info` fields.
    pub fn strip_extra_info(val: &mut Value) {
        if let Value::Object(map) = val {
            let mut preserved_flags = std::collections::HashMap::new();
            if let Some(Value::Object(extra)) = map.get("extra_info") {
                if let Some(status) = extra.get("__colmena_status") {
                    preserved_flags.insert("__colmena_status", status.clone());
                }
                if let Some(loop_status) = extra.get("__colmena_loop_status") {
                    preserved_flags.insert("__colmena_loop_status", loop_status.clone());
                }
                if let Some(is_output_node) = extra.get("__colmena_is_output_node") {
                    preserved_flags.insert("__colmena_is_output_node", is_output_node.clone());
                }
            }

            if map.contains_key("extra_info") {
                map.remove("extra_info");
            }

            for (k, v) in preserved_flags {
                map.insert(k.to_string(), v);
            }

            for (_, v) in map.iter_mut() {
                Self::strip_extra_info(v);
            }
        } else if let Value::Array(arr) = val {
            for item in arr.iter_mut() {
                Self::strip_extra_info(item);
            }
        }
    }

    /// Executes the graph and streams events for each step.
    // Mirrors the established positional streaming API; `cancel_token` is the
    // minimal cooperative-cancellation handle. Bundling into a params struct
    // would be a larger, inconsistent refactor of this hot path.
    #[allow(clippy::too_many_arguments)]
    pub fn execute_stream(
        self,
        graph: Graph,
        resume_session_id: Option<String>,
        resume_answer: Option<String>,
        include_extra_info: bool,
        // Path prefix injected by the parent subgraph node, if any.
        // For root runs this is `None` and node_id_path = node_id.
        path_prefix: Option<String>,
        // Conversation handle. `None` for legacy runs.
        agent_session_id: Option<String>,
        // Cooperative cancellation handle. When fired, the loop stops between
        // nodes (dropping the in-flight node future), persists `Cancelled`, marks
        // running descendants, and yields a terminal `Cancelled` event. `None`
        // disables cancellation (e.g. CLI/serve internal call sites).
        cancel_token: Option<tokio_util::sync::CancellationToken>,
    ) -> impl futures::Stream<
        Item = Result<crate::dag_engine::domain::events::DagExecutionEvent, DagError>,
    > {
        async_stream::try_stream! {
            use crate::dag_engine::domain::events::{DagExecutionEvent, NodeEndError};
            use crate::dag_engine::domain::observer::NodeEvent;

            // Mirrors the success close below; shared by both failure closes.
            fn finish_event(node_id: &str, node_type: &str, output: Value, error: Option<NodeEndError>) -> DagExecutionEvent {
                let node_id = node_id.to_string();
                if node_type == "subgraph" {
                    DagExecutionEvent::SubgraphNodeFinish { node_id, output, error }
                } else {
                    DagExecutionEvent::NodeFinish { node_id, output, error }
                }
            }

            let mut all_outputs: HashMap<String, Value> = HashMap::new();
            // (decrypted → handle) for every secret injected in this run. Masks
            // streamed frames only; nodes and persisted state keep real values.
            let mut run_secrets: HashMap<String, String> = HashMap::new();
            let mut active_queue: VecDeque<String> = VecDeque::new();
            let mut session_id = uuid::Uuid::new_v4().to_string();
            let mut active_agent_session_id: Option<String> = agent_session_id.clone();
            let mut parent_session_id_for_save: Option<String> = None;
            let mut execution_history: Vec<(String, String)> = Vec::new();
            let mut global_calls: HashMap<String, u32> = HashMap::new();
            let mut caller_specific_calls: HashMap<String, HashMap<String, u32>> = HashMap::new();
            let mut global_shared_state = serde_json::json!({});

            // Structural validation, on every entry. This is the only place every
            // caller converges: the CLI validates before calling, but the library
            // entry points (`execute_stream`, `execute_stream_cancellable`,
            // `run_dag`, `stream_sse_parts`) took a `Graph` and ran it unchecked —
            // which is the path ADP's worker uses, so a graph with a node id
            // containing `/`, a malformed `node_schema`, an invalid `memory_mode`
            // or a misconfigured `mcp` block reached execution in production.
            //
            // Two of those four already failed later anyway (`node_schema` is
            // re-parsed and `memory_mode` re-checked when tools are built), so this
            // mostly moves the error earlier and gives it a better message. The
            // other two failed silently — a misconfigured MCP server was simply
            // ignored, which reads to the operator as "the model ignored my
            // server". Cheap and network-free, so it runs before the pre-flight.
            //
            // Disable via COLMENA_GRAPH_VALIDATION=off (safety valve, same shape as
            // COLMENA_PREFLIGHT_HEALTH).
            if std::env::var("COLMENA_GRAPH_VALIDATION").ok().as_deref() != Some("off") {
                graph.validate()?;
            } else {
                tracing::debug!(
                    target: "colmena::engine",
                    "graph structural validation disabled via COLMENA_GRAPH_VALIDATION=off"
                );
            }

            // Pre-flight: validate keys of the providers this graph will use (cached,
            // blocking). Runs on every entry (fresh/resume/subgraph re-enter here) —
            // the TTL cache is what makes per-turn ADP runs cheap (first turn checks,
            // rest hit cache). Disable via COLMENA_PREFLIGHT_HEALTH=off.
            preflight::validate_graph_providers(&graph).await?;

            // ── Lifecycle decision (spec §4.1) ─────────────────────────────────────
            //
            // Branch 1: explicit session_id provided → direct resume.
            // Branch 2: only agent_session_id provided → search for SUSPENDED leaf,
            //           else fresh root run under that chat.
            // Branch 3: neither → legacy fresh-run path.
            //
            match (&resume_session_id, &agent_session_id) {
                (Some(id), maybe_agent) => {
                    // Branch 1: direct resume by run UUID.
                    if let Some(repo) = &self.state_repository {
                        if let Some(state) = repo.get_by_id(id).await? {
                            // Conflict check: passed agent must match stored agent (when both present).
                            if let (Some(passed), Some(stored)) =
                                (maybe_agent, &state.agent_session_id)
                            {
                                if passed != stored {
                                    Err(DagError::NodeExecution(format!(
                                        "session_id {} belongs to agent_session_id {} but caller passed {}",
                                        id, stored, passed
                                    )))?;
                                }
                            }

                            all_outputs = state.all_outputs;
                            active_queue = state.active_queue;
                            session_id = state.session_id;
                            execution_history = state.execution_history;
                            global_calls = state.global_calls;
                            caller_specific_calls = state.caller_specific_calls;
                            global_shared_state = state.global_shared_state;
                            active_agent_session_id = state.agent_session_id;
                            parent_session_id_for_save = state.parent_session_id;
                        } else {
                            // Row not found — caller knows the id but it's not in the table.
                            // Treat as fresh start with that id.
                            session_id = id.clone();
                            active_agent_session_id = maybe_agent.clone();
                        }
                    } else {
                        session_id = id.clone();
                        active_agent_session_id = maybe_agent.clone();
                    }
                }
                (None, Some(agent)) => {
                    // Branch 2: resolve by chat handle.
                    if let Some(repo) = &self.state_repository {
                        match repo.find_resume_entry(agent).await? {
                            Some(leaf_id) => {
                                if let Some(state) = repo.get_by_id(&leaf_id).await? {
                                    all_outputs = state.all_outputs;
                                    active_queue = state.active_queue;
                                    session_id = state.session_id;
                                    execution_history = state.execution_history;
                                    global_calls = state.global_calls;
                                    caller_specific_calls = state.caller_specific_calls;
                                    global_shared_state = state.global_shared_state;
                                    active_agent_session_id = state.agent_session_id;
                                    parent_session_id_for_save = state.parent_session_id;
                                }
                            }
                            None => {
                                // No suspended leaf — fresh root run under this chat.
                                active_agent_session_id = Some(agent.clone());
                            }
                        }
                    } else {
                        active_agent_session_id = Some(agent.clone());
                    }
                }
                (None, None) => {
                    // Branch 3: pure legacy. session_id stays as a new UUID;
                    // agent_session_id remains None.
                }
            }

            // If queue is still empty, initialize with nodes that have 0 incoming dependencies
            if active_queue.is_empty() {
                for node_id in graph.nodes.keys() {
                    let in_degree = graph.edges.iter().filter(|e| {
                        // Exact match or matches "node_id." (JSON pointer)
                        e.to == *node_id || e.to.starts_with(&format!("{}.", node_id))
                    }).count();

                    if in_degree == 0 {
                        active_queue.push_back(node_id.clone());
                    }
                }
            }

            // Build the resuming-node-ids set BEFORE the main loop.
            // The loop's `all_outputs.remove(&node_id)` at the top of each
            // iteration destroys the SUSPENDED marker once a node re-executes,
            // so we snapshot the set up front.
            //
            // A node is "resuming" iff its persisted output has
            // `__colmena_status: "SUSPENDED"` (recursive). Used below
            // to gate `__colmena_resume_answer` injection.
            //
            // Spec: docs/superpowers/specs/2026-06-05-suspend-resume-answer-routing-fix-design.md §4.1
            let resuming_node_ids: HashSet<String> =
                Self::compute_resuming_node_ids(&all_outputs, &resume_answer);

            if !global_shared_state.is_object() {
                global_shared_state = serde_json::json!({});
            }

            Self::fold_seed_state(&mut global_shared_state, self.seed_state.clone());

            if let Some(obj) = global_shared_state.as_object_mut() {
                obj.insert("session_id".to_string(), Value::String(session_id.clone()));

                let mut graph_nodes_meta = serde_json::Map::new();
                for (nid, node) in &graph.nodes {
                    graph_nodes_meta.insert(nid.clone(), node.config.clone());
                }
                obj.insert("__graph_nodes".to_string(), Value::Object(graph_nodes_meta));
            }

            let mut current_caller: Option<String> = None;

            // Nodes the engine declined to enqueue, with the first reason seen.
            // Collected rather than reported on sight: a node with several
            // incoming branches can be passed over by one of them and still run
            // via another, and a cyclic graph re-walks the same untaken branch
            // every turn. The run decides at the end who actually never ran.
            let mut potential_skips: HashMap<String, &'static str> = HashMap::new();

            // Set when the run abandons its queue on an execution limit rather
            // than draining it.
            let mut stopped_early = false;

            // Usage tracking: accumulate token counts and model/provider per node_id.
            // node_meta: node_id → NodeMeta (model, provider, node_type, provider_key_id)
            // usage_accumulator: node_id → (prompt, completion, thinking, cache_read, cache_write)
            let mut node_meta: HashMap<String, NodeMeta> = HashMap::new();
            let mut usage_accumulator: HashMap<String, (u32, u32, u32, u32, u32)> = HashMap::new();

            // Start cyclic execution loop
            while let Some(node_id) = active_queue.pop_front() {

                // ── Hard-stop check (between nodes) ────────────────────────────────
                // If cancellation was requested, stop before starting this node:
                // persist a terminal CANCELLED state (with the remaining queue and
                // partial outputs), clean up any RUNNING descendants, emit the
                // terminal `Cancelled` event, and end the stream.
                if cancel_token.as_ref().is_some_and(|t| t.is_cancelled()) {
                    let mut remaining = active_queue.clone();
                    remaining.push_front(node_id.clone());
                    if let Some(repo) = &self.state_repository {
                        let state = DagRunState {
                            session_id: session_id.clone(),
                            agent_session_id: active_agent_session_id.clone(),
                            parent_session_id: parent_session_id_for_save.clone(),
                            graph_json: GraphSkeleton::at_rest_json(&graph),
                            all_outputs: all_outputs.clone(),
                            global_shared_state: global_shared_state.clone(),
                            execution_history: execution_history.clone(),
                            global_calls: global_calls.clone(),
                            caller_specific_calls: caller_specific_calls.clone(),
                            active_queue: remaining,
                            status: DagRunStatus::Cancelled,
                        };
                        repo.save(&state).await?;
                        let _ = repo.cancel_running_descendants(&session_id).await;
                    }
                    yield DagExecutionEvent::Cancelled {
                        reason: None,
                        partial_output: serde_json::to_value(&all_outputs).unwrap_or(Value::Null),
                    };
                    return;
                }

                // 1. Check if node has all required inputs before acting on it
                // Ignore cyclic loopback edges because they shouldn't block the node from starting its very first turn
                let incoming_edges: Vec<_> = graph.edges.iter().filter(|e| {
                    (e.to == node_id || e.to.starts_with(&format!("{}.", node_id))) && !e.cyclic.unwrap_or(false)
                }).collect();

                let mut is_ready = true;
                for edge in &incoming_edges {
                    let parts_from: Vec<&str> = edge.from.splitn(2, '.').collect();
                    let source_node_id = parts_from[0];

                    // If a node explicitly depends on an upstream node's output, that upstream node MUST have completed at least once
                    if let Some(output) = all_outputs.get(source_node_id) {
                        if parts_from.len() > 1 {
                            // If user specified a sub-path (e.g. node.field), ensure the field actually exists in the output
                            let pointer = format!("/{}", parts_from[1].replace('.', "/"));
                            if output.pointer(&pointer).is_none() {
                                is_ready = false;
                                break;
                            }
                        }
                    } else {
                        is_ready = false;
                        break;
                    }
                }

                if !is_ready {
                    // Re-queue the node to the back to wait for dependencies (only if dependencies are still running)
                    // (To avoid infinite loops on dead ends, we should arguably only re-queue if upstream is in active_queue)
                    let upstream_running = incoming_edges.iter().any(|e| {
                        let sid = e.from.split('.').next().unwrap();
                        active_queue.contains(&sid.to_string())
                    });

                    if upstream_running {
                        active_queue.push_back(node_id.clone());
                    } else {
                        colmena_log!("⚠️ [RunUseCase] Dropping node '{}' from queue because its upstream dependencies never fired.", node_id);
                        potential_skips
                            .entry(node_id.clone())
                            .or_insert("upstream_never_fired");
                    }
                    continue;
                }

                let node_config = match graph.nodes.get(&node_id) {
                    Some(cfg) => cfg,
                    None => continue, // Stale edge pointer edge-case
                };

                // (trigger_on legacy skipping logic has been natively replaced by dynamic active_queue routing)
                // Check Call Limits
                if let Err(e) = Self::check_limits(&node_id, current_caller.as_deref(), &graph, &mut global_calls, &mut caller_specific_calls) {
                    colmena_log!("🚨 [RunUseCase] Execution limit reached: {}", e);
                    // The queue is abandoned here, so whatever is still in it did
                    // not go unvisited for a routing reason. Remember that, or the
                    // report below would blame ordinary routing for a truncation.
                    stopped_early = true;
                    break;
                }

                // Update trackers
                *global_calls.entry(node_id.clone()).or_insert(0) += 1;
                if let Some(caller) = &current_caller {
                    *caller_specific_calls.entry(caller.clone()).or_default().entry(node_id.clone()).or_default() += 1;
                    execution_history.push((caller.clone(), node_id.clone()));
                }

                // Purge previous output allowing refresh
                all_outputs.remove(&node_id);

                let node_impl = self
                    .registry
                    .get_node(&node_config.node_type)
                    .ok_or_else(|| DagError::NodeTypeNotFound(node_config.node_type.clone()))?;

                let mut inputs = self.build_inputs_for(&node_id, &graph.edges, &all_outputs, &graph)?;

                // STEP 1: Inject secrets for non-LLM nodes (before executing)
                let mut node_config_value = node_config.config.clone();
                if node_config.node_type != "llm" {
                    if let Some(svc) = &self.secure_value_service {
                        let agent_for_inject = active_agent_session_id.as_deref();
                        let mut inputs_value = serde_json::to_value(&inputs)
                            .unwrap_or(Value::Object(Default::default()));
                        match svc
                            .inject_secrets(&mut inputs_value, &session_id, agent_for_inject)
                            .await
                        {
                            Ok(map) => run_secrets.extend(map),
                            Err(e) => eprintln!("⚠️ Failed to inject secrets: {}", e),
                        }
                        if let Ok(injected_inputs) = serde_json::from_value::<NodeInputs>(inputs_value) {
                            inputs = injected_inputs;
                        }
                        match svc
                            .inject_secrets(&mut node_config_value, &session_id, agent_for_inject)
                            .await
                        {
                            Ok(map) => run_secrets.extend(map),
                            Err(e) => eprintln!("⚠️ Failed to inject secrets in config: {}", e),
                        }
                    }
                }

                // Inject __colmena_resume_answer only for nodes that were SUSPENDED
                // in the persisted snapshot. See spec §3.1 and §4.1.2.
                if let Some(ans) = &resume_answer {
                    if resuming_node_ids.contains(&node_id) {
                        inputs.insert(
                            "__colmena_resume_answer".to_string(),
                            Value::String(ans.clone()),
                        );
                    } else {
                        tracing::trace!(
                            target: "colmena::dag_engine",
                            node_id = %node_id,
                            "resume_answer present but node was not in SUSPENDED set; skipping injection"
                        );
                    }
                }
                let node_id_path = match &path_prefix {
                    Some(prefix) => format!("{}/{}", prefix, node_id),
                    None => node_id.clone(),
                };

                inputs.insert("__colmena_session_id".to_string(), Value::String(session_id.clone()));
                inputs.insert("__node_id".to_string(), Value::String(node_id.clone()));
                inputs.insert(
                    "__colmena_node_id_path".to_string(),
                    Value::String(node_id_path.clone()),
                );
                if let Some(a) = active_agent_session_id.as_deref() {
                    inputs.insert(
                        "__colmena_agent_session_id".to_string(),
                        Value::String(a.to_string()),
                    );
                }
                if let Some(tz) = graph.timezone.as_deref() {
                    inputs.insert(
                        "__colmena_timezone".to_string(),
                        Value::String(tz.to_string()),
                    );
                }
                if let Some(loc) = graph.location.as_deref() {
                    inputs.insert(
                        "__colmena_location".to_string(),
                        Value::String(loc.to_string()),
                    );
                }
                if let Some(lc) = graph.locale.as_deref() {
                    inputs.insert(
                        "__colmena_locale".to_string(),
                        Value::String(lc.to_string()),
                    );
                }

                // INJECT GLOBAL SHARED STATE (so nodes can use {{key}} out of the box).
                // We override None, Null, AND empty objects — the latter arise when an
                // InputNode with an empty config outputs `{}` and build_inputs_for assigns
                // that empty object to a default_input field (e.g. "prompt"), which would
                // otherwise block injection of the real value from global state.
                if let Some(obj) = global_shared_state.as_object() {
                    for (k, v) in obj {
                        let should_inject = match inputs.get(k) {
                            None => true,
                            Some(Value::Null) => true,
                            Some(Value::Object(o)) if o.is_empty() => true,
                            _ => false,
                        };
                        if should_inject {
                            inputs.insert(k.clone(), v.clone());
                        }
                    }
                }

                // Record model/provider for usage summary
                {
                    let model = node_config.config.get("model").and_then(|v| v.as_str()).map(|s| s.to_string());
                    let provider = node_config.config.get("provider").and_then(|v| v.as_str()).map(|s| s.to_string());
                    let provider_key_id = node_config.config.get("provider_key_id").and_then(|v| v.as_str()).map(|s| s.to_string());
                    node_meta.insert(node_id.clone(), NodeMeta { model, provider, node_type: node_config.node_type.clone(), provider_key_id });
                }

                // Masked CLONES only — the node itself still executes below with
                // the real `inputs`/`node_config_value`.
                let mut masked_start_inputs = serde_json::to_value(&inputs).unwrap_or(Value::Null);
                let mut masked_start_config = node_config_value.clone();
                SecureValueService::mask_secrets(&mut masked_start_inputs, &run_secrets);
                SecureValueService::mask_secrets(&mut masked_start_config, &run_secrets);
                yield DagExecutionEvent::NodeStart {
                    node_id: node_id.clone(),
                    node_type: node_config.node_type.clone(),
                    inputs: masked_start_inputs,
                    config: masked_start_config,
                };

                let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
                let channel_observer: Arc<dyn crate::dag_engine::domain::observer::ExecutionObserver> =
                    Arc::new(ChannelObserver { tx });
                let observer = MaskingObserver::wrap(Some(channel_observer), &run_secrets);

                // Snapshot the shared state before the node mutably borrows it, so the
                // mid-node cancellation arm can persist a coherent (pre-node) state.
                // Only taken when a cancel token is wired, to avoid the clone otherwise.
                let global_state_snapshot = if cancel_token.is_some() || self.liveness.idle_timeout.is_some() {
                    global_shared_state.clone()
                } else {
                    Value::Null
                };

                let output = {
                    let execution_future = node_impl.execute(&inputs, &node_config_value, &mut global_shared_state, observer);
                    tokio::pin!(execution_future);

                    // ── Liveness clocks (spec: SPEC_STREAM_MIDRUN_LIVENESS) ─────
                    // TWO clocks, deliberately separate:
                    //   • `last_forwarded` advances only on events that produce a
                    //     forwarded/XADDed part (content, tool calls, reasoning,
                    //     node boundaries). Governs the heartbeat. Turn-boundary /
                    //     accounting markers (LlmUsage, LlmMessageStart/Finish,
                    //     TurnStart) do NOT advance it — a stream emitting only
                    //     those is silent to the user and must keep heart-beating.
                    //   • `last_any` advances on EVERY event received via `rx`.
                    //     Governs the idle-abort watchdog, so a node that is
                    //     genuinely producing *some* signal is never killed as hung.
                    // Heartbeats are yielded directly (never through `rx`), so they
                    // cannot feed the clocks they are driven by.
                    let hb_interval = self.liveness.heartbeat_interval;
                    let idle_timeout = self.liveness.idle_timeout;
                    let mut last_forwarded = tokio::time::Instant::now();
                    let mut last_any = last_forwarded;
                    let mut last_beat = last_forwarded;
                    let mut last_tool: Option<String> = None;
                    let mut idle_abort_msg: Option<String> = None;

                    let mut output_opt = None;
                    loop {
                        tokio::select! {
                            res = &mut execution_future, if output_opt.is_none() => {
                                output_opt = Some(res);
                            }
                            // ── Hard-stop check (mid-node) ─────────────────────────
                            // Cancellation requested while the node is in flight. We
                            // drop `execution_future` by returning, which aborts the
                            // in-flight async work (reqwest/sqlx abort at their next
                            // await; blocking spawn_blocking threads run to their own
                            // timeout but their result is discarded). Persist CANCELLED
                            // and emit the terminal event.
                            _ = async {
                                match &cancel_token {
                                    Some(t) => t.cancelled().await,
                                    None => std::future::pending::<()>().await,
                                }
                            } => {
                                let mut remaining = active_queue.clone();
                                remaining.push_front(node_id.clone());
                                if let Some(repo) = &self.state_repository {
                                    let state = DagRunState {
                                        session_id: session_id.clone(),
                                        agent_session_id: active_agent_session_id.clone(),
                                        parent_session_id: parent_session_id_for_save.clone(),
                                        graph_json: GraphSkeleton::at_rest_json(&graph),
                                        all_outputs: all_outputs.clone(),
                                        global_shared_state: global_state_snapshot.clone(),
                                        execution_history: execution_history.clone(),
                                        global_calls: global_calls.clone(),
                                        caller_specific_calls: caller_specific_calls.clone(),
                                        active_queue: remaining,
                                        status: DagRunStatus::Cancelled,
                                    };
                                    // Best-effort: `?` can't propagate out of a select! arm,
                                    // and a save failure must not prevent us from stopping.
                                    if let Err(e) = repo.save(&state).await {
                                        eprintln!("⚠️ Failed to persist CANCELLED state: {}", e);
                                    }
                                    let _ = repo.cancel_running_descendants(&session_id).await;
                                }
                                yield DagExecutionEvent::Cancelled {
                                    reason: None,
                                    partial_output: serde_json::to_value(&all_outputs).unwrap_or(Value::Null),
                                };
                                return;
                            }
                            // ── Liveness heartbeat (mid-node) ──────────────────
                            // Nothing real for `hb_interval`: emit Progress so
                            // downstream no-event watchdogs (platform API, 60s)
                            // see the run is alive.
                            _ = async {
                                match hb_interval {
                                    Some(iv) => tokio::time::sleep_until(
                                        std::cmp::max(last_forwarded, last_beat) + iv
                                    ).await,
                                    None => std::future::pending::<()>().await,
                                }
                            }, if output_opt.is_none() => {
                                last_beat = tokio::time::Instant::now();
                                yield DagExecutionEvent::Progress {
                                    node_id: node_id.clone(),
                                    idle_secs: last_forwarded.elapsed().as_secs(),
                                };
                            }
                            // ── Idle watchdog (mid-node) ───────────────────────
                            // No real event for `idle_timeout`: treat the node as
                            // hung. Drop the future (same interruption semantics
                            // as hard-stop), persist FAILED, fail the stream with
                            // a descriptive error. Heartbeats never feed
                            // `last_activity`, so they cannot mask a hang.
                            _ = async {
                                match idle_timeout {
                                    Some(to) => tokio::time::sleep_until(last_any + to).await,
                                    None => std::future::pending::<()>().await,
                                }
                            }, if output_opt.is_none() => {
                                let idle_secs = idle_timeout.map(|t| t.as_secs()).unwrap_or(0);
                                let tool_suffix = last_tool
                                    .as_ref()
                                    .map(|t| format!(" (tool '{}' in flight)", t))
                                    .unwrap_or_default();
                                let msg = format!(
                                    "node '{}' produced no events for {}s{} — aborted by liveness watchdog (COLMENA_IDLE_TIMEOUT_SECS)",
                                    node_id, idle_secs, tool_suffix
                                );
                                if let Some(repo) = &self.state_repository {
                                    let mut remaining = active_queue.clone();
                                    remaining.push_front(node_id.clone());
                                    let state = DagRunState {
                                        session_id: session_id.clone(),
                                        agent_session_id: active_agent_session_id.clone(),
                                        parent_session_id: parent_session_id_for_save.clone(),
                                        graph_json: GraphSkeleton::at_rest_json(&graph),
                                        all_outputs: all_outputs.clone(),
                                        global_shared_state: global_state_snapshot.clone(),
                                        execution_history: execution_history.clone(),
                                        global_calls: global_calls.clone(),
                                        caller_specific_calls: caller_specific_calls.clone(),
                                        active_queue: remaining,
                                        status: DagRunStatus::Failed,
                                    };
                                    if let Err(e) = repo.save(&state).await {
                                        eprintln!("⚠️ Failed to persist FAILED state after idle abort: {}", e);
                                    }
                                    let _ = repo.cancel_running_descendants(&session_id).await;
                                }
                                // `Err(...)?` cannot be used here — this arm's body runs
                                // inside the `select!`'s per-arm async block, whose return
                                // type is `()`, not the `try_stream!` macro's
                                // `Result`-returning outer async block (unlike the
                                // `output_result...?` usage after the loop, which sits
                                // directly in the outer body). Stash the message and break
                                // out of the loop so it can be raised as a stream-level
                                // `Err` there, matching every other abort path (hard-stop,
                                // node error) that drain consumers already handle.
                                idle_abort_msg = Some(msg);
                                break;
                            }
                            event_opt = rx.recv() => {
                                match event_opt {
                                    Some(event) => {
                                        // Every event keeps the idle watchdog at bay…
                                        let now = tokio::time::Instant::now();
                                        last_any = now;
                                        // …but only content/progress events reset the
                                        // heartbeat clock (turn boundaries / usage do not).
                                        if node_event_advances_heartbeat(&event) {
                                            last_forwarded = now;
                                        }
                                        match event {
                                            NodeEvent::LlmToken { token } => yield DagExecutionEvent::LlmToken { node_id: node_id.clone(), token },
                                            NodeEvent::ThinkingToken { node_id: thinking_node_id, node_type: thinking_node_type, token } => yield DagExecutionEvent::ThinkingToken { node_id: thinking_node_id, node_type: thinking_node_type, token },
                                            NodeEvent::LlmToolCall { tool_id, tool_name, args_chunk } => yield DagExecutionEvent::LlmToolCall { node_id: node_id.clone(), tool_id, tool_name, args_chunk },
                                            NodeEvent::LlmUsage { prompt_tokens, completion_tokens, thinking_tokens, cache_read_tokens, cache_write_tokens } => {
                                                let entry = usage_accumulator.entry(node_id.clone()).or_insert((0, 0, 0, 0, 0));
                                                entry.0 += prompt_tokens;
                                                entry.1 += completion_tokens;
                                                entry.2 += thinking_tokens.unwrap_or(0);
                                                entry.3 += cache_read_tokens.unwrap_or(0);
                                                entry.4 += cache_write_tokens.unwrap_or(0);
                                                yield DagExecutionEvent::LlmUsage { node_id: node_id.clone(), prompt_tokens, completion_tokens, thinking_tokens, cache_read_tokens, cache_write_tokens };
                                            }
                                            NodeEvent::LlmToolCallStart { tool_id, tool_name, tool_args } => {
                                                last_tool = Some(tool_name.clone());
                                                yield DagExecutionEvent::LlmToolCallStart { node_id: node_id.clone(), tool_id, tool_name, tool_args }
                                            }
                                            NodeEvent::LlmToolCallFinish { tool_id, success, output } => {
                                                last_tool = None;
                                                yield DagExecutionEvent::LlmToolCallFinish { node_id: node_id.clone(), tool_id, success, output }
                                            }
                                            NodeEvent::SkillLoaded { tool_id, skill_name, reference, source, size_bytes } => yield DagExecutionEvent::SkillLoaded { node_id: node_id.clone(), tool_id, skill_name, reference, source, size_bytes },
                                            NodeEvent::ToolDescribed { tool_id, tool_name } => yield DagExecutionEvent::ToolDescribed { node_id: node_id.clone(), tool_id, tool_name },
                                            NodeEvent::BatchProgress { node_id, total, completed, ok, err, in_flight } => {
                                                yield DagExecutionEvent::BatchProgress { node_id, total, completed, ok, err, in_flight }
                                            }
                                            NodeEvent::BatchItemFinished { node_id, index, key, status } => {
                                                yield DagExecutionEvent::BatchItemFinished { node_id, index, key, status }
                                            }
                                            NodeEvent::LlmMessageStart => yield DagExecutionEvent::LlmMessageStart { node_id: node_id.clone() },
                                            NodeEvent::LlmMessageFinish(usage) => yield DagExecutionEvent::LlmMessageFinish { node_id: node_id.clone(), usage: usage.map(|u| serde_json::json!(u)) },
                                            NodeEvent::ReasoningStart { id } => yield DagExecutionEvent::ReasoningStart { node_id: node_id.clone(), id },
                                            NodeEvent::ReasoningDelta { id, token } => yield DagExecutionEvent::ReasoningDelta { node_id: node_id.clone(), id, token },
                                            NodeEvent::ReasoningEnd { id } => yield DagExecutionEvent::ReasoningEnd { node_id: node_id.clone(), id },
                                            NodeEvent::SubgraphChildEvent(raw) => {
                                                // Re-yield child events preserving their original node IDs.
                                                // GraphFinish is suppressed — SubgraphNodeFinish (below) serves that role.
                                                // Also intercept NodeStart and LlmUsage to populate tracking maps.
                                                if let Ok(child_event) = serde_json::from_value::<DagExecutionEvent>(raw) {
                                                    // Extract tracking data before moving child_event into yield
                                                    match &child_event {
                                                        DagExecutionEvent::NodeStart { node_id: cid, node_type: ctype, inputs, config } => {
                                                            let model = inputs.get("model").or_else(|| config.get("model"))
                                                                .and_then(|v| v.as_str()).map(|s| s.to_string());
                                                            let provider = inputs.get("provider").or_else(|| config.get("provider"))
                                                                .and_then(|v| v.as_str()).map(|s| s.to_string());
                                                            let provider_key_id = config.get("provider_key_id")
                                                                .and_then(|v| v.as_str()).map(|s| s.to_string());
                                                            node_meta.insert(cid.clone(), NodeMeta { model, provider, node_type: ctype.clone(), provider_key_id });
                                                        }
                                                        DagExecutionEvent::LlmUsage { node_id: cid, prompt_tokens, completion_tokens, thinking_tokens, cache_read_tokens, cache_write_tokens } => {
                                                            let entry = usage_accumulator.entry(cid.clone()).or_insert((0, 0, 0, 0, 0));
                                                            entry.0 += prompt_tokens;
                                                            entry.1 += completion_tokens;
                                                            entry.2 += thinking_tokens.unwrap_or(0);
                                                            entry.3 += cache_read_tokens.unwrap_or(0);
                                                            entry.4 += cache_write_tokens.unwrap_or(0);
                                                        }
                                                        DagExecutionEvent::GraphFinish { .. } => {
                                                            // suppressed
                                                        }
                                                        _ => {}
                                                    }
                                                    match child_event {
                                                        DagExecutionEvent::GraphFinish { .. } => {}
                                                        // Grandchild+ event that already crossed one subgraph
                                                        // boundary: FLATTEN instead of re-nesting. Bump `depth`
                                                        // and prefix this node onto the lineage `path`. The old
                                                        // code produced `SubgraphWrapped { SubgraphWrapped { .. } }`,
                                                        // which the mapper could not unwrap (dropped as `_ => None`).
                                                        DagExecutionEvent::SubgraphWrapped { inner, depth, path } => {
                                                            let new_path = if path.is_empty() {
                                                                node_id.clone()
                                                            } else {
                                                                format!("{}>{}", node_id, path)
                                                            };
                                                            yield DagExecutionEvent::SubgraphWrapped {
                                                                inner,
                                                                depth: depth + 1,
                                                                path: new_path,
                                                            };
                                                        }
                                                        // Base child event from a direct child subgraph: wrap at
                                                        // depth 1 with path `<this node>>​<child node>`.
                                                        other => {
                                                            let new_path = match other.node_id() {
                                                                Some(cid) => format!("{}>{}", node_id, cid),
                                                                None => node_id.clone(),
                                                            };
                                                            yield DagExecutionEvent::SubgraphWrapped {
                                                                inner: Box::new(other),
                                                                depth: 1,
                                                                path: new_path,
                                                            };
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    None => break,
                                }
                            }
                        }
                    }

                    if let Some(msg) = idle_abort_msg {
                        // No decrypted secret here, so no masking needed.
                        if self.nested_run {
                            let error = Some(NodeEndError { message: Some(msg.clone()) });
                            yield finish_event(&node_id, &node_config.node_type, Value::Null, error);
                        }
                        Err(DagError::NodeExecution(msg))?;
                    }

                    let output_result = output_opt.unwrap_or_else(|| {
                        Err(Box::new(DagError::NodeExecution("Future did not complete but channel closed".to_string())))
                    });
                    match output_result {
                        Ok(v) => v,
                        Err(e) => {
                            // Mask once; the same masked string closes the node
                            // (if nested) AND becomes the propagated DagError —
                            // never the raw pre-mask text on either path.
                            let mut msg = Value::String(e.to_string());
                            SecureValueService::mask_secrets(&mut msg, &run_secrets);
                            let masked = msg.as_str().unwrap_or_default().to_string();
                            if self.nested_run {
                                let error = Some(NodeEndError { message: Some(masked.clone()) });
                                yield finish_event(&node_id, &node_config.node_type, Value::Null, error);
                            }
                            Err(DagError::NodeExecution(masked))?
                        }
                    }
                };

                // STEP 2: Hash output if secure: true (after executing)
                let mut processed_output = output.clone();
                if let Some(svc) = &self.secure_value_service {
                    match svc
                        .hash_output(
                            &processed_output,
                            &node_config.config,
                            &session_id,
                            active_agent_session_id.as_deref(),
                            &node_id,
                        )
                        .await
                    {
                        Ok(hashed) => {
                            processed_output = hashed;
                        }
                        Err(e) => {
                            eprintln!("⚠️ Failed to hash secure values: {}", e);
                        }
                    }
                }

                // Masked CLONE only — `processed_output` itself stays real for
                // `all_outputs`, persisted state, and the SUSPENDED check below.
                let mut masked_finish_output = processed_output.clone();
                SecureValueService::mask_secrets(&mut masked_finish_output, &run_secrets);
                if node_config.node_type == "subgraph" {
                    yield DagExecutionEvent::SubgraphNodeFinish {
                        node_id: node_id.clone(),
                        output: masked_finish_output,
                        error: None,
                    };
                } else {
                    yield DagExecutionEvent::NodeFinish {
                        node_id: node_id.clone(),
                        output: masked_finish_output,
                        error: None,
                    };
                }

                // Handle SUSPENDED
                if Self::find_status_by_key(&processed_output, "__colmena_status") == Some("SUSPENDED".to_string()) {
                    let mut final_output = processed_output.clone();
                    if !include_extra_info {
                        Self::strip_extra_info(&mut final_output);
                    }
                    if let Some(final_obj) = final_output.as_object_mut() {
                        final_obj.insert("session_id".to_string(), Value::String(session_id.clone()));
                    }

                    all_outputs.insert(node_id.to_string(), final_output.clone());

                    if let Some(repo) = &self.state_repository {
                        // Crucial: When suspending, we must put the node back at the BEGINNING
                        // of the queue so it re-executes first when resuming (receiving the answer).
                        let mut resume_queue = active_queue.clone();
                        resume_queue.push_front(node_id.clone());

                        let state = DagRunState {
                            session_id: session_id.clone(),
                            agent_session_id: active_agent_session_id.clone(),
                            parent_session_id: parent_session_id_for_save.clone(),
                            graph_json: GraphSkeleton::at_rest_json(&graph),
                            all_outputs: all_outputs.clone(),
                            global_shared_state: global_shared_state.clone(),
                            execution_history: execution_history.clone(),
                            global_calls: global_calls.clone(),
                            caller_specific_calls: caller_specific_calls.clone(),
                            active_queue: resume_queue,
                            status: DagRunStatus::Suspended,
                        };
                        repo.save(&state).await?;
                    }

                    // Root run only: a child's GraphFinish is the parent subgraph
                    // node's return value, under a session that may not decrypt a handle.
                    let mut masked_graph_finish = final_output.clone();
                    if path_prefix.is_none() {
                        SecureValueService::mask_secrets(&mut masked_graph_finish, &run_secrets);
                    }
                    yield DagExecutionEvent::GraphFinish { output: masked_graph_finish };
                    return;
                }

                all_outputs.insert(node_id.to_string(), processed_output.clone());

                // --- DYNAMICALLY PUSH TO QUEUE BASED ON EDGES ---
                // If a node emitted Value::Null intentionally (skip stub), do not traverse its descendants
                if processed_output.is_null() {
                    // Deliberate "skip stub": a node emitting `null` stops the
                    // branch. Legitimate control flow, but the downstream nodes
                    // never run, so say so instead of leaving a hole.
                    for edge in graph.edges.iter().filter(|e| e.from == node_id || e.from.starts_with(&format!("{}.", node_id))) {
                        let target_node_id = edge.to.split('.').next().unwrap_or("").to_string();
                        if !target_node_id.is_empty() {
                            potential_skips
                                .entry(target_node_id)
                                .or_insert("upstream_null_output");
                        }
                    }
                }

                if !processed_output.is_null() {
                    let outgoing_edges: Vec<_> = graph.edges.iter().filter(|e| e.from == node_id || e.from.starts_with(&format!("{}.", node_id))).collect();
                    for edge in outgoing_edges {

                        // Check if the specific JSON path exists in the emitted output
                        let parts_from: Vec<&str> = edge.from.splitn(2, '.').collect();
                        let has_data = if parts_from.len() == 1 {
                            true // Passing the entire output object
                        } else {
                            let json_pointer = parts_from[1].replace('.', "/");
                            let ptr_exists = processed_output.pointer(&format!("/{}", json_pointer)).is_some_and(|v| !v.is_null());
                            colmena_log!("DEBUG [Queue Edge]: edge.from='{}' -> pointer='{}' -> has_data={}", edge.from, json_pointer, ptr_exists);
                            ptr_exists
                        };

                        let target_node_id = edge.to.split('.').next().unwrap_or("");
                        // A target that is never enqueued simply does not run.
                        // Both causes below are silent by design (conditional
                        // routing, and a stale `to`), so report instead of failing.
                        let skip_reason = if !has_data {
                            Some("pointer_unresolved")
                        } else if !graph.nodes.contains_key(target_node_id) {
                            Some("unknown_target")
                        } else {
                            None
                        };
                        match skip_reason {
                            Some(reason) => {
                                if !target_node_id.is_empty() {
                                    potential_skips
                                        .entry(target_node_id.to_string())
                                        .or_insert(reason);
                                }
                            }
                            None => {
                                if !active_queue.contains(&target_node_id.to_string()) {
                                    colmena_log!("DEBUG [Queue Push]: Enqueuing {} -> {}", node_id, target_node_id);
                                    active_queue.push_back(target_node_id.to_string());
                                }
                            }
                        }
                    }
                }

                current_caller = Some(node_id.clone());
            }

            // A node is skipped iff the whole run went by without it producing
            // an output. The source of truth is the graph itself, not what an
            // edge happened to mark: a node can be passed over by one branch and
            // still run via another (so marking is not enough to report), and a
            // node behind an already-skipped one is never marked at all (so
            // marking is not enough to *find* it). `potential_skips` only
            // supplies the precise cause when one was observed — on a run that
            // resumes from a suspend, the marks were made in an earlier run and
            // are gone, but the node is still correctly reported.
            // Sorted so the tail of the stream is stable.
            let no_cause_reason = if stopped_early {
                "run_stopped_early"
            } else {
                "never_reached"
            };
            let mut confirmed_skips: Vec<(String, &'static str)> = graph
                .nodes
                .keys()
                .map(|node_id| {
                    let reason = potential_skips
                        .get(node_id)
                        .copied()
                        .unwrap_or(no_cause_reason);
                    (node_id.clone(), reason)
                })
                // …plus anything an edge pointed at that is not a node at all,
                // which `graph.nodes` by definition cannot surface.
                .chain(
                    potential_skips
                        .iter()
                        .filter(|(node_id, _)| !graph.nodes.contains_key(*node_id))
                        .map(|(node_id, reason)| (node_id.clone(), *reason)),
                )
                .filter(|(node_id, _)| !all_outputs.contains_key(node_id))
                .collect();
            confirmed_skips.sort();
            for (node_id, reason) in confirmed_skips {
                yield DagExecutionEvent::NodeSkipped {
                    node_id,
                    reason: reason.to_string(),
                };
            }

            // Completed
            if let Some(repo) = &self.state_repository {
                let state = DagRunState {
                    session_id: session_id.clone(),
                    agent_session_id: active_agent_session_id.clone(),
                    parent_session_id: parent_session_id_for_save.clone(),
                    graph_json: GraphSkeleton::at_rest_json(&graph),
                    all_outputs: all_outputs.clone(),
                    global_shared_state: global_shared_state.clone(),
                    execution_history: execution_history.clone(),
                    global_calls: global_calls.clone(),
                    caller_specific_calls: caller_specific_calls.clone(),
                    active_queue: VecDeque::new(),
                    status: DagRunStatus::Completed,
                };
                repo.save(&state).await?;
            }

            let mut final_aggregated_output = serde_json::to_value(&all_outputs).unwrap_or(Value::Null);
            if !include_extra_info {
                if let Some(output_map) = final_aggregated_output.as_object_mut() {
                    for (nid, node_output) in output_map.iter_mut() {
                        let node_wants_extra = graph.nodes.get(nid).and_then(|nc| nc.config.get("include_extra_info")).and_then(|v| v.as_bool()).unwrap_or(false);
                        if !node_wants_extra {
                            Self::strip_extra_info(node_output);
                        }
                    }
                }
            }
            if let Some(obj) = final_aggregated_output.as_object_mut() {
                obj.insert("__colmena_session_id".to_string(), Value::String(session_id.clone()));
            }

            // SWEEP: Delete only EXPIRED secure values for this run's scope
            // (session_id OR agent_session_id when set). Live rows survive so the
            // next turn of a multi-turn conversation can still read them. See
            // docs/superpowers/specs/2026-05-11-secure-values-sliding-ttl-design.md.
            if let Some(svc) = &self.secure_value_service {
                match svc
                    .cleanup_expired_for_run(&session_id, active_agent_session_id.as_deref())
                    .await
                {
                    Ok(rows) if rows > 0 => {
                        tracing::info!(
                            target: "colmena::run_use_case",
                            rows_deleted = rows,
                            session_id = %session_id,
                            "secure_values: expired rows swept at run end"
                        );
                    }
                    Ok(_) => {}
                    Err(e) => {
                        tracing::warn!(
                            target: "colmena::run_use_case",
                            error = %e,
                            "secure_values: cleanup_expired_for_run failed (non-fatal)"
                        );
                    }
                }
            }

            // Emit per-node usage summary before finishing
            if !usage_accumulator.is_empty() {
                let entries: Vec<Value> = usage_accumulator
                    .iter()
                    .map(|(nid, counts)| usage_entry(nid, *counts, node_meta.get(nid)))
                    .collect();
                yield DagExecutionEvent::GraphUsageSummary { entries };
            }

            // See the SUSPENDED-path GraphFinish above: child runs stay unmasked.
            let mut masked_final_aggregated = final_aggregated_output.clone();
            if path_prefix.is_none() {
                SecureValueService::mask_secrets(&mut masked_final_aggregated, &run_secrets);
            }
            yield DagExecutionEvent::GraphFinish { output: masked_final_aggregated };
        }
    }

    /// Compute the set of node ids whose persisted output has
    /// `__colmena_status: "SUSPENDED"` (recursive search via
    /// `find_status_by_key`). Returns an empty set when `resume_answer`
    /// is `None` — there's no run to resume, so nothing to inject into.
    ///
    /// See spec
    /// `docs/superpowers/specs/2026-06-05-suspend-resume-answer-routing-fix-design.md`
    /// §4.1.1.
    fn compute_resuming_node_ids(
        all_outputs: &HashMap<String, Value>,
        resume_answer: &Option<String>,
    ) -> HashSet<String> {
        if resume_answer.is_none() {
            return HashSet::new();
        }
        all_outputs
            .iter()
            .filter_map(|(nid, out)| {
                if Self::find_status_by_key(out, "__colmena_status")
                    == Some("SUSPENDED".to_string())
                {
                    Some(nid.clone())
                } else {
                    None
                }
            })
            .collect()
    }

    fn find_status_by_key(val: &Value, key: &str) -> Option<String> {
        if let Some(obj) = val.as_object() {
            if let Some(status) = obj.get(key).and_then(|v| v.as_str()) {
                return Some(status.to_string());
            }
            for v in obj.values() {
                if let Some(s) = Self::find_status_by_key(v, key) {
                    return Some(s);
                }
            }
        } else if let Some(arr) = val.as_array() {
            for v in arr {
                if let Some(s) = Self::find_status_by_key(v, key) {
                    return Some(s);
                }
            }
        }
        None
    }

    fn build_inputs_for(
        &self,
        current_node_id: &str,
        all_edges: &[Edge],
        all_outputs: &HashMap<String, Value>,
        graph: &Graph,
    ) -> Result<NodeInputs, DagError> {
        let mut inputs: NodeInputs = HashMap::new();
        let incoming_edges = all_edges
            .iter()
            .filter(|edge| edge.to.starts_with(current_node_id));

        for edge in incoming_edges {
            let parts_to: Vec<&str> = edge.to.splitn(2, '.').collect();
            let parts_from: Vec<&str> = edge.from.splitn(2, '.').collect();
            if parts_from.is_empty() {
                continue;
            }
            let source_node_id = parts_from[0];

            // --- Step 1: Resolve source output field ---
            // If edge.from has no dot, use the source node's default_output (if any)
            let source_field_opt = if parts_from.len() == 1 {
                // Check if source node has a default_output
                if let Some(source_node_cfg) = graph.nodes.get(source_node_id) {
                    if let Some(source_node_impl) =
                        self.registry.get_node(&source_node_cfg.node_type)
                    {
                        source_node_impl.default_output().map(|s| s.to_string())
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                Some(parts_from[1].to_string())
            };

            // Extract the value from all_outputs using the resolved field
            let value_to_pass = if let Some(source_output_value) = all_outputs.get(source_node_id) {
                match &source_field_opt {
                    Some(field) => {
                        let json_pointer = field.replace('.', "/");
                        let extracted = source_output_value
                            .pointer(&format!("/{}", json_pointer))
                            .cloned();

                        // If the field wasn't found but the source output is an object,
                        // it might mean the node is already flattened output (fallback)
                        if extracted.is_none() && source_output_value.is_object() {
                            source_output_value.clone()
                        } else {
                            extracted.unwrap_or(Value::Null)
                        }
                    }
                    None => {
                        // No default_output and no explicit field — use entire output
                        source_output_value.clone()
                    }
                }
            } else {
                Value::Null
            };

            // --- Step 2: Resolve target input field ---
            // If edge.to has no dot, use the target node's default_input (if any)
            if parts_to.len() == 2 {
                // Explicit target field — insert directly
                inputs.insert(parts_to[1].to_string(), value_to_pass);
            } else {
                // No explicit field — check for default_input
                let inserted = if let Some(target_node_cfg) = graph.nodes.get(current_node_id) {
                    if let Some(target_node_impl) =
                        self.registry.get_node(&target_node_cfg.node_type)
                    {
                        if let Some(field) = target_node_impl.default_input() {
                            // Smart extraction: if the source value is an object and the target
                            // has a default_input that matches a key in that object, extract it
                            // directly. This allows simple edges (e.g. "start_trigger → llm_call")
                            // to work without needing explicit field notation.
                            let val_to_insert = if value_to_pass.is_object() {
                                value_to_pass
                                    .get(field)
                                    .cloned()
                                    .unwrap_or_else(|| value_to_pass.clone())
                            } else {
                                value_to_pass.clone()
                            };
                            inputs.insert(field.to_string(), val_to_insert);
                            true
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                } else {
                    false
                };

                // If no default_input, try auto-flattening as last resort
                if !inserted {
                    if let Some(obj) = value_to_pass.as_object() {
                        // Object — merge all its keys
                        for (k, v) in obj {
                            inputs.insert(k.clone(), v.clone());
                        }
                    } else {
                        // Non-object — use source node ID as key
                        inputs.insert(source_node_id.to_string(), value_to_pass);
                    }
                }
            }
        }
        Ok(inputs)
    }
}

struct ChannelObserver {
    tx: tokio::sync::mpsc::UnboundedSender<crate::dag_engine::domain::observer::NodeEvent>,
}
impl crate::dag_engine::domain::observer::ExecutionObserver for ChannelObserver {
    fn on_event(&self, event: crate::dag_engine::domain::observer::NodeEvent) {
        let _ = self.tx.send(event);
    }
}

/// Per-node metadata tracked for the usage summary: which model/provider ran
/// it, its node type, and (for `llm_call` nodes) the embedder-supplied
/// billing id it was configured with.
#[derive(Debug, Clone, Default)]
pub(crate) struct NodeMeta {
    pub model: Option<String>,
    pub provider: Option<String>,
    pub node_type: String,
    /// Opaque, non-secret billing id the embedder writes next to `api_key` in
    /// an `llm_call` node's config (`docs/node_configurations.json`). Echoed
    /// verbatim on that node's usage entry so the embedder can attribute
    /// consumption to the key that paid for it — never inferred or validated
    /// by the engine.
    pub provider_key_id: Option<String>,
}

/// Build one `usage-summary` entry for a node. Pure and independently
/// testable: `meta` absent (a node the run never saw a `NodeStart`/config for)
/// falls back to `NodeMeta::default()`, matching the prior tuple-map
/// `unwrap_or` behavior. `provider_key_id` is included only when configured —
/// a node without one carries no such field, rather than `null`.
pub(crate) fn usage_entry(
    node_id: &str,
    counts: (u32, u32, u32, u32, u32),
    meta: Option<&NodeMeta>,
) -> Value {
    let (pt, ct, tt, cr, cw) = counts;
    let meta = meta.cloned().unwrap_or_default();
    let mut obj = serde_json::Map::new();
    obj.insert("node_id".into(), json!(node_id));
    obj.insert("node_type".into(), json!(meta.node_type));
    obj.insert("model".into(), json!(meta.model));
    obj.insert("provider".into(), json!(meta.provider));
    if let Some(key) = meta.provider_key_id {
        obj.insert("provider_key_id".into(), json!(key));
    }
    obj.insert("prompt_tokens".into(), json!(pt));
    obj.insert("completion_tokens".into(), json!(ct));
    if tt > 0 {
        obj.insert("thinking_tokens".into(), json!(tt));
    }
    // Always emitted, `0` included: an absent cache field could not be
    // told apart from a provider that never reports one. Kept as two
    // fields because read and write bill at rates >10x apart.
    obj.insert("cache_read_tokens".into(), json!(cr));
    obj.insert("cache_write_tokens".into(), json!(cw));
    // Cache tokens count toward the total — they were processed and billed.
    obj.insert("total_tokens".into(), json!(pt + ct + tt + cr + cw));
    Value::Object(obj)
}

/// Whether a `NodeEvent` received via the observer channel should advance the
/// liveness **heartbeat** clock (`last_forwarded`). Mirrors
/// [`DagExecutionEvent::advances_heartbeat_clock`]: content and progress events
/// do; pure turn-boundary / accounting markers (`LlmUsage`, `LlmMessageStart`,
/// `LlmMessageFinish`) do not. For a `SubgraphChildEvent` the decision follows
/// the wrapped base event; an undeserializable payload conservatively does not
/// advance the clock. The idle-abort clock (`last_any`) is separate and always
/// advances.
fn node_event_advances_heartbeat(event: &crate::dag_engine::domain::observer::NodeEvent) -> bool {
    use crate::dag_engine::domain::events::DagExecutionEvent;
    use crate::dag_engine::domain::observer::NodeEvent;
    match event {
        NodeEvent::LlmUsage { .. }
        | NodeEvent::LlmMessageStart
        | NodeEvent::LlmMessageFinish(_) => false,
        NodeEvent::SubgraphChildEvent(raw) => {
            serde_json::from_value::<DagExecutionEvent>(raw.clone())
                .map(|e| e.advances_heartbeat_clock())
                .unwrap_or(false)
        }
        _ => true,
    }
}

/// What `resume_subgraph` does with the graph it was handed.
enum ResumePlan {
    Run(Graph),
    /// Refuse before running anything: the row is closed and this text goes
    /// back verbatim (it leads with a stable prefix).
    Refuse(String),
}

impl DagRunUseCase {
    /// Decides what a suspended child resumes with, before anything runs.
    /// Only structure is compared (`GraphSkeleton`): a fresh graph may bring
    /// new keys, tokens, skill paths or prompts, which is the point. The row
    /// only needs to hold that skeleton — since v0.19 it holds nothing else
    /// (`GraphSkeleton::at_rest_json`) — and older rows with the whole graph
    /// compare the same way. An unreadable STORED graph is an error as
    /// before, not a refusal.
    fn plan_resume(stored: &Value, requested: ResumeGraph) -> Result<ResumePlan, DagError> {
        Ok(match requested {
            ResumeGraph::Unavailable(reason) => ResumePlan::Refuse(reason),
            ResumeGraph::Fresh(fresh) => {
                // Parse STORED first: its (unclosed) error wins if both are
                // broken, over a Fresh-side refusal that would close the row.
                let stored = serde_json::from_value::<Graph>(stored.clone()).map_err(|e| {
                    DagError::NodeExecution(format!("Invalid sub-graph state JSON: {}", e))
                })?;
                match serde_json::from_value::<Graph>(fresh) {
                    // Fixed text: a serde error can quote a value from the graph.
                    Err(_) => ResumePlan::Refuse(format!(
                        "{SUBGRAPH_RESUME_INCOMPATIBLE} the child graph source no longer holds \
                         a valid graph. Run it again from the start."
                    )),
                    Ok(fresh) => {
                        match GraphSkeleton::of(&stored).diff(&GraphSkeleton::of(&fresh)) {
                            Some(diff) => ResumePlan::Refuse(diff.to_string()),
                            None => ResumePlan::Run(fresh),
                        }
                    }
                }
            }
        })
    }

    /// A refused resume closes the child's row: its answer is spent and nobody
    /// resumes it again, while a row left SUSPENDED counts as a second chain in
    /// `find_resume_entry` and can be picked by `find_suspended_child` on a
    /// later turn. Its own SUSPENDED descendants are closed for the same
    /// reason — a leaf left SUSPENDED under a FAILED parent breaks the same
    /// lookup. Uses `fail_if_suspended`'s conditional UPDATE instead of a
    /// full-row read-modify-write, so a concurrent writer's commit is never
    /// lost and a row that is no longer SUSPENDED (already closed, or terminal
    /// for an unrelated reason) is never flipped — nor are its descendants
    /// touched: a row this call didn't close isn't this call's to unwind.
    /// The row(s) keep the graph they had — a fresh graph (and its secrets)
    /// never reaches storage through a refusal. Failures are logged, not
    /// returned: the refusal is what the caller needs to see.
    async fn close_refused(repo: &dyn DagStateRepository, session_id: &str) {
        match repo.fail_if_suspended(session_id).await {
            Ok(true) => {
                if let Err(e) = repo.fail_suspended_descendants(session_id).await {
                    eprintln!(
                        "⚠️ Failed to close refused child {}'s suspended descendants: {}",
                        session_id, e
                    );
                }
            }
            Ok(false) => {}
            Err(e) => {
                eprintln!("⚠️ Failed to close refused child run {}: {}", session_id, e);
            }
        }
    }
}

#[async_trait::async_trait]
impl SubGraphExecutorPort for DagRunUseCase {
    async fn run_subgraph(
        &self,
        session_id: &str,
        graph_json: Value,
        global_state: Value,
        observer: Option<Arc<dyn crate::dag_engine::domain::observer::ExecutionObserver>>,
        parent_session_id: Option<String>,
        agent_session_id: Option<String>,
        path_prefix: Option<String>,
    ) -> Result<Value, DagError> {
        let graph: Graph = serde_json::from_value(graph_json)
            .map_err(|e| DagError::NodeExecution(format!("Invalid sub-graph JSON: {}", e)))?;
        graph
            .validate()
            .map_err(|e| DagError::NodeExecution(format!("Invalid sub-graph: {}", e)))?;

        // Handed to the child both ways: persisted below (so a resume can find
        // it) AND seeded in memory on the cloned use case (so the handover works
        // even with no state repository configured).
        let seed = global_state.clone();

        // Mapear globales del hijo a la tabla para el inicio
        if let Some(repo) = &self.state_repository {
            let initial_state = DagRunState {
                session_id: session_id.to_string(),
                agent_session_id: agent_session_id.clone(),
                parent_session_id: parent_session_id.clone(),
                graph_json: GraphSkeleton::at_rest_json(&graph),
                all_outputs: HashMap::new(),
                global_shared_state: global_state,
                execution_history: Vec::new(),
                global_calls: HashMap::new(),
                caller_specific_calls: HashMap::new(),
                active_queue: VecDeque::new(),
                status: DagRunStatus::Running,
            };
            repo.save(&initial_state).await?;
        }

        use futures::StreamExt;
        let mut stream = Box::pin(
            self.clone()
                .with_seed_state(seed)
                .as_nested_run()
                .execute_stream(
                    graph,
                    Some(session_id.to_string()),
                    None,
                    true,
                    path_prefix,
                    agent_session_id,
                    // Subgraph children are interrupted via drop-propagation from the
                    // root, then cleaned up by cancel_running_descendants. No token here.
                    None,
                ),
        );

        let mut final_out = Value::Null;
        while let Some(res) = stream.next().await {
            let event = res?;
            if let crate::dag_engine::domain::events::DagExecutionEvent::GraphFinish {
                ref output,
            } = event
            {
                final_out = output.clone();
            } else if let Some(obs) = &observer {
                if let Ok(raw) = serde_json::to_value(&event) {
                    obs.on_event(
                        crate::dag_engine::domain::observer::NodeEvent::SubgraphChildEvent(raw),
                    );
                }
            }
        }

        Ok(final_out)
    }

    async fn resume_subgraph(
        &self,
        session_id: &str,
        answer: String,
        graph: ResumeGraph,
        observer: Option<Arc<dyn crate::dag_engine::domain::observer::ExecutionObserver>>,
        agent_session_id: Option<String>,
        path_prefix: Option<String>,
    ) -> Result<Value, DagError> {
        let Some(repo) = &self.state_repository else {
            return Err(DagError::NodeExecution(
                "State repository missing for resume".to_string(),
            ));
        };
        let state = repo.get_by_id(session_id).await?.ok_or_else(|| {
            DagError::NodeExecution(format!("Child session {} not found for resume", session_id))
        })?;

        let graph = match Self::plan_resume(&state.graph_json, graph)? {
            ResumePlan::Run(graph) => graph,
            ResumePlan::Refuse(reason) => {
                colmena_log!(
                    "⛔ [SubGraph] Resume of child {} refused: {}",
                    session_id,
                    reason
                );
                Self::close_refused(&**repo, &state.session_id).await;
                return Err(DagError::ResumeRefused(reason));
            }
        };
        graph
            .validate()
            .map_err(|e| DagError::NodeExecution(format!("Invalid sub-graph: {}", e)))?;

        use futures::StreamExt;
        let mut stream = Box::pin(self.clone().as_nested_run().execute_stream(
            graph,
            Some(session_id.to_string()),
            Some(answer),
            true,
            path_prefix,
            agent_session_id.or(state.agent_session_id),
            None,
        ));

        let mut final_out = Value::Null;
        while let Some(res) = stream.next().await {
            let event = res?;
            if let crate::dag_engine::domain::events::DagExecutionEvent::GraphFinish {
                ref output,
            } = event
            {
                final_out = output.clone();
            } else if let Some(obs) = &observer {
                if let Ok(raw) = serde_json::to_value(&event) {
                    obs.on_event(
                        crate::dag_engine::domain::observer::NodeEvent::SubgraphChildEvent(raw),
                    );
                }
            }
        }

        Ok(final_out)
    }

    async fn find_child_session_id_for_resume(
        &self,
        parent_session_id: &str,
        _parent_node_path: &str,
    ) -> Result<Option<String>, DagError> {
        if let Some(repo) = &self.state_repository {
            repo.find_suspended_child(parent_session_id).await
        } else {
            Ok(None)
        }
    }
}

#[cfg(test)]
mod seed_state_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn seeds_missing_keys() {
        let mut state = json!({ "kept": 1 });
        DagRunUseCase::fold_seed_state(
            &mut state,
            Some(json!({ "__colmena_subgraph_depth": 3, "other": "x" })),
        );
        assert_eq!(state["kept"], 1);
        assert_eq!(state["__colmena_subgraph_depth"], 3);
        assert_eq!(state["other"], "x");
    }

    /// A resumed run's persisted state is newer than the parent's seed.
    #[test]
    fn existing_keys_are_not_overwritten() {
        let mut state = json!({ "__colmena_subgraph_depth": 9 });
        DagRunUseCase::fold_seed_state(&mut state, Some(json!({ "__colmena_subgraph_depth": 1 })));
        assert_eq!(state["__colmena_subgraph_depth"], 9);
    }

    #[test]
    fn no_seed_is_a_noop() {
        let mut state = json!({ "a": 1 });
        DagRunUseCase::fold_seed_state(&mut state, None);
        assert_eq!(state, json!({ "a": 1 }));
    }

    #[test]
    fn non_object_seed_and_non_object_state_are_ignored() {
        let mut state = json!({ "a": 1 });
        DagRunUseCase::fold_seed_state(&mut state, Some(json!("not an object")));
        assert_eq!(state, json!({ "a": 1 }));

        let mut scalar = json!(5);
        DagRunUseCase::fold_seed_state(&mut scalar, Some(json!({ "a": 1 })));
        assert_eq!(scalar, json!(5));
    }
}

#[cfg(test)]
mod usage_entry_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_usage_entry_carries_the_llm_calls_provider_key_id() {
        let meta = NodeMeta {
            model: Some("m".into()),
            provider: Some("google".into()),
            node_type: "llm_call".into(),
            provider_key_id: Some("key-1".into()),
        };
        let e = usage_entry("llm", (10, 5, 0, 0, 0), Some(&meta));
        assert_eq!(e["provider_key_id"], json!("key-1"));
    }

    /// Without a `provider_key_id` (either no meta at all, or meta whose field
    /// is `None`) the entry carries NO such key — not `null`. A consumer that
    /// checks presence via `.get("provider_key_id").is_some()` must see it
    /// absent, not a JSON null.
    #[test]
    fn a_usage_entry_without_a_provider_key_id_omits_the_field() {
        let without_meta = usage_entry("llm", (10, 5, 0, 0, 0), None);
        assert!(
            without_meta.get("provider_key_id").is_none(),
            "{without_meta}"
        );

        let meta_no_key = NodeMeta {
            model: Some("m".into()),
            provider: Some("google".into()),
            node_type: "llm_call".into(),
            provider_key_id: None,
        };
        let e = usage_entry("llm", (10, 5, 0, 0, 0), Some(&meta_no_key));
        assert!(e.get("provider_key_id").is_none(), "{e}");
    }

    /// Regression guard for the tuple→struct refactor: model/provider/node_type
    /// and the token math must all still land where the old inline map put
    /// them.
    #[test]
    fn a_usage_entry_still_carries_the_pre_existing_fields() {
        let meta = NodeMeta {
            model: Some("gemini-2.5-flash".into()),
            provider: Some("google".into()),
            node_type: "llm_call".into(),
            provider_key_id: None,
        };
        let e = usage_entry("llm", (10, 5, 2, 1, 1), Some(&meta));
        assert_eq!(e["node_id"], json!("llm"));
        assert_eq!(e["node_type"], json!("llm_call"));
        assert_eq!(e["model"], json!("gemini-2.5-flash"));
        assert_eq!(e["provider"], json!("google"));
        assert_eq!(e["prompt_tokens"], json!(10));
        assert_eq!(e["completion_tokens"], json!(5));
        assert_eq!(e["thinking_tokens"], json!(2));
        assert_eq!(e["cache_read_tokens"], json!(1));
        assert_eq!(e["cache_write_tokens"], json!(1));
        assert_eq!(e["total_tokens"], json!(19));
    }

    #[test]
    fn a_usage_entry_omits_thinking_tokens_when_zero() {
        let e = usage_entry("llm", (10, 5, 0, 0, 0), None);
        assert!(e.get("thinking_tokens").is_none(), "{e}");
    }
}

#[cfg(test)]
mod resuming_node_ids_tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashMap;

    #[test]
    fn empty_when_resume_answer_is_none() {
        let mut all = HashMap::new();
        all.insert("n1".to_string(), json!({ "__colmena_status": "SUSPENDED" }));
        let set = DagRunUseCase::compute_resuming_node_ids(&all, &None);
        assert!(set.is_empty(), "fresh run must yield empty set");
    }

    #[test]
    fn includes_only_suspended_nodes() {
        let mut all = HashMap::new();
        all.insert(
            "suspended_one".to_string(),
            json!({ "__colmena_status": "SUSPENDED", "question": "x" }),
        );
        all.insert("ran_fine".to_string(), json!({ "output": 42 }));
        all.insert(
            "another_suspend".to_string(),
            json!({ "__colmena_status": "SUSPENDED" }),
        );
        let set = DagRunUseCase::compute_resuming_node_ids(&all, &Some("anything".to_string()));
        assert_eq!(set.len(), 2);
        assert!(set.contains("suspended_one"));
        assert!(set.contains("another_suspend"));
        assert!(!set.contains("ran_fine"));
    }

    #[test]
    fn finds_suspended_in_nested_output() {
        // Mirrors the orchestrator/subgraph wrap case where the SUSPENDED
        // marker is nested inside the parent's output structure.
        let mut all = HashMap::new();
        all.insert(
            "wrapper".to_string(),
            json!({
                "result": { "__colmena_status": "SUSPENDED" },
                "meta": { "child": "inner_node" }
            }),
        );
        let set = DagRunUseCase::compute_resuming_node_ids(&all, &Some("ans".to_string()));
        assert!(set.contains("wrapper"));
    }

    #[test]
    fn empty_all_outputs_yields_empty_set() {
        let all: HashMap<String, serde_json::Value> = HashMap::new();
        let set = DagRunUseCase::compute_resuming_node_ids(&all, &Some("ans".to_string()));
        assert!(set.is_empty());
    }
}

/// One level deeper than #313: a node failing *inside* a nested run never closed either.
#[cfg(test)]
mod nested_failure_close_tests {
    use super::*;
    use crate::dag_engine::domain::error::DagError as Err_;
    use crate::dag_engine::domain::events::DagExecutionEvent;
    use crate::dag_engine::domain::node::ExecutableNode;
    use crate::dag_engine::domain::observer::ExecutionObserver;
    use crate::dag_engine::domain::secure_value_repository::SecureValueRepository;
    use async_trait::async_trait;
    use futures::StreamExt;
    use std::error::Error as StdError;
    use std::time::Duration;

    /// One node, three scripted behaviors. `EchoBoom` reads `config` (where
    /// secrets get decrypted for a static field), quoting one in its error.
    enum ScriptedNode {
        Boom(&'static str),
        EchoBoom,
        Sleepy(u64),
    }
    #[async_trait]
    impl ExecutableNode for ScriptedNode {
        async fn execute(
            &self,
            _i: &NodeInputs,
            config: &Value,
            _s: &mut Value,
            _o: Option<Arc<dyn ExecutionObserver>>,
        ) -> Result<Value, Box<dyn StdError + Send + Sync>> {
            match self {
                ScriptedNode::Boom(msg) => Err((*msg).into()),
                ScriptedNode::EchoBoom => {
                    let secret = config.get("token").and_then(|v| v.as_str()).unwrap_or("?");
                    Err(format!("upstream call failed with token {secret}").into())
                }
                ScriptedNode::Sleepy(ms) => {
                    tokio::time::sleep(Duration::from_millis(*ms)).await;
                    Ok(json!({ "ok": true }))
                }
            }
        }
        fn schema(&self) -> Value {
            json!({})
        }
    }

    struct TestRegistry(HashMap<String, Arc<dyn ExecutableNode>>);
    impl NodeRegistryPort for TestRegistry {
        fn get_node(&self, node_type: &str) -> Option<Arc<dyn ExecutableNode>> {
            self.0.get(node_type).cloned()
        }
        fn get_all_nodes(&self) -> HashMap<String, Arc<dyn ExecutableNode>> {
            self.0.clone()
        }
    }

    fn registry_with(node_type: &str, node: Arc<dyn ExecutableNode>) -> Arc<TestRegistry> {
        let mut nodes: HashMap<String, Arc<dyn ExecutableNode>> = HashMap::new();
        nodes.insert(node_type.to_string(), node);
        Arc::new(TestRegistry(nodes))
    }

    fn single_node_graph(node_id: &str, node_type: &str) -> Graph {
        serde_json::from_value(json!({
            "nodes": { node_id: { "type": node_type, "config": {} } },
            "edges": []
        }))
        .expect("valid graph JSON")
    }

    fn run(
        uc: DagRunUseCase,
        graph: Graph,
    ) -> impl futures::Stream<Item = Result<DagExecutionEvent, Err_>> {
        uc.execute_stream(graph, None, None, false, None, None, None)
    }

    /// Drains a stream, returning (all Ok events, first Err's message).
    async fn drain(
        stream: impl futures::Stream<Item = Result<DagExecutionEvent, Err_>>,
    ) -> (Vec<DagExecutionEvent>, Option<String>) {
        tokio::pin!(stream);
        let mut events = Vec::new();
        let mut err = None;
        while let Some(item) = stream.next().await {
            match item {
                Ok(ev) => events.push(ev),
                Err(e) => {
                    err = Some(e.to_string());
                    break;
                }
            }
        }
        (events, err)
    }

    /// Every finish frame's `data` closing `id` (wire encoding: covers both variants).
    fn finishes(events: &[DagExecutionEvent], id: &str) -> Vec<Value> {
        events
            .iter()
            .filter_map(|e| serde_json::to_value(e).ok())
            .filter(|v| {
                v["event"].as_str().is_some_and(|t| t.ends_with("finish"))
                    && v["data"]["node_id"] == id
            })
            .map(|v| v["data"].clone())
            .collect()
    }

    fn finish_for(events: &[DagExecutionEvent], id: &str) -> Option<Value> {
        finishes(events, id).into_iter().next()
    }

    fn finish_count(events: &[DagExecutionEvent], id: &str) -> usize {
        finishes(events, id).len()
    }

    #[tokio::test]
    async fn nested_run_closes_failing_node_before_returning_err() {
        let uc = DagRunUseCase::new(
            registry_with("boom", Arc::new(ScriptedNode::Boom("boom"))),
            None,
        )
        .as_nested_run();
        let stream = run(uc, single_node_graph("n1", "boom"));
        let (events, err) = drain(stream).await;
        assert!(err.is_some(), "the failure must still propagate");
        let data = finish_for(&events, "n1").expect("nested run must close the failing node");
        assert_eq!(data["output"], Value::Null);
        assert!(!data["error"].is_null(), "close must carry status:error");
    }

    #[tokio::test]
    async fn root_run_failure_emits_no_finish_for_the_failing_node() {
        // No `.as_nested_run()` — this is a root run.
        let uc = DagRunUseCase::new(
            registry_with("boom", Arc::new(ScriptedNode::Boom("boom"))),
            None,
        );
        let stream = run(uc, single_node_graph("n1", "boom"));
        let (events, err) = drain(stream).await;
        assert!(err.is_some());
        assert_eq!(
            finish_count(&events, "n1"),
            0,
            "root run relies on the stream's own error frame, not a close"
        );
    }

    /// A failing edge-wired `subgraph` still needs the loop-level close (no
    /// real `SubGraphNode` here); full balance proven live by `edge_wired_subgraph_failure.json`.
    #[tokio::test]
    async fn nested_failing_subgraph_node_emits_its_loop_level_close() {
        let uc = DagRunUseCase::new(
            registry_with("subgraph", Arc::new(ScriptedNode::Boom("boom"))),
            None,
        )
        .as_nested_run();
        let stream = run(uc, single_node_graph("sg", "subgraph"));
        let (events, err) = drain(stream).await;
        assert!(err.is_some());
        let data = finish_for(&events, "sg").expect("the loop-level close must still fire");
        assert!(!data["error"].is_null(), "close must carry status:error");
        assert_eq!(
            finish_count(&events, "sg"),
            1,
            "exactly one loop-level close"
        );
    }

    #[tokio::test]
    async fn nested_idle_abort_closes_node_exactly_once() {
        let liveness = LivenessSettings {
            heartbeat_interval: None,
            idle_timeout: Some(Duration::from_millis(150)),
        };
        let uc = DagRunUseCase::new(
            registry_with("sleepy", Arc::new(ScriptedNode::Sleepy(3_000))),
            None,
        )
        .as_nested_run()
        .with_liveness(liveness);
        let stream = run(uc, single_node_graph("n1", "sleepy"));
        let (events, err) = drain(stream).await;
        assert!(err.is_some(), "idle-abort must still fail the stream");
        assert_eq!(
            finish_count(&events, "n1"),
            1,
            "exactly one close, not zero or two"
        );
        let data = finish_for(&events, "n1").unwrap();
        assert!(
            data["error"]["message"].is_string(),
            "the idle-abort message carries no secret, so it is not stripped"
        );
    }

    #[tokio::test]
    async fn failure_before_node_start_emits_no_close() {
        // No "unregistered" node type in the registry — NodeTypeNotFound fires
        // before any NodeStart is ever yielded.
        let uc = DagRunUseCase::new(
            registry_with("boom", Arc::new(ScriptedNode::Boom("boom"))),
            None,
        )
        .as_nested_run();
        let stream = run(uc, single_node_graph("n1", "unregistered"));
        let (events, err) = drain(stream).await;
        assert!(err.is_some());
        assert!(
            events.iter().all(|e| !matches!(
                e,
                DagExecutionEvent::NodeFinish { .. } | DagExecutionEvent::SubgraphNodeFinish { .. }
            )),
            "no start ever fired, so there must be nothing to close"
        );
    }

    /// Decrypts exactly one handle; everything else is unreachable in this test.
    struct OneSecretRepo;
    #[async_trait]
    impl SecureValueRepository for OneSecretRepo {
        async fn persist(
            &self,
            _: &str,
            _: Option<&str>,
            _: &str,
            _: &str,
            _: &str,
            _: &str,
        ) -> Result<(), Err_> {
            Ok(())
        }
        async fn decrypt(
            &self,
            _: &str,
            _: Option<&str>,
            handle: &str,
        ) -> Result<Option<String>, Err_> {
            Ok((handle == "<sv_tok_1>").then(|| "LEAKTOK_q8w2e5".to_string()))
        }
        async fn cleanup(&self, _: &str) -> Result<(), Err_> {
            Ok(())
        }
        async fn cleanup_expired(&self) -> Result<u64, Err_> {
            Ok(0)
        }
        async fn cleanup_expired_for_run(&self, _: &str, _: Option<&str>) -> Result<u64, Err_> {
            Ok(0)
        }
    }

    /// The close frame's `errorText` must be the SAME masked string as the propagated `DagError`.
    #[tokio::test]
    async fn nested_failing_node_close_carries_masked_error_text() {
        let svc = Arc::new(SecureValueService::new(Arc::new(OneSecretRepo)));
        let uc = DagRunUseCase::with_secure_values_and_service(
            registry_with("echo_boom", Arc::new(ScriptedNode::EchoBoom)),
            None,
            svc,
        )
        .as_nested_run();

        let graph: Graph = serde_json::from_value(json!({
            "nodes": { "n1": { "type": "echo_boom", "config": { "token": "<sv_tok_1>" } } },
            "edges": []
        }))
        .expect("valid graph JSON");

        let stream = run(uc, graph);
        let (events, err) = drain(stream).await;
        let err = err.expect("the failure must still propagate");
        assert!(
            !err.contains("LEAKTOK_q8w2e5"),
            "the propagated DagError leaked: {err}"
        );

        let data = finish_for(&events, "n1").expect("nested run must close the failing node");
        let message = data["error"]["message"].as_str().unwrap_or("");
        assert!(
            !message.contains("LEAKTOK_q8w2e5"),
            "the close frame leaked the decrypted secret: {message}"
        );
        assert!(
            message.contains("<sv_tok_1>"),
            "expected the masked handle, got: {message}"
        );
    }
}

#[cfg(test)]
mod resume_graph_tests {
    //! `resume_subgraph` with each `ResumeGraph`: a fresh graph runs when its
    //! skeleton matches the stored one; a changed one, or an unavailable
    //! source, is refused before anything runs and closes the child's row.
    use super::*;
    use crate::dag_engine::domain::node::ExecutableNode;
    use crate::dag_engine::domain::observer::ExecutionObserver;
    use async_trait::async_trait;
    use std::error::Error as StdError;
    use std::sync::Mutex;

    /// Hands back the config it ran with, so a test can tell which graph ran.
    struct EchoConfig;
    #[async_trait]
    impl ExecutableNode for EchoConfig {
        async fn execute(
            &self,
            _i: &NodeInputs,
            config: &Value,
            _s: &mut Value,
            _o: Option<Arc<dyn ExecutionObserver>>,
        ) -> Result<Value, Box<dyn StdError + Send + Sync>> {
            Ok(config.clone())
        }
        fn schema(&self) -> Value {
            json!({})
        }
    }

    struct EchoRegistry;
    impl NodeRegistryPort for EchoRegistry {
        fn get_node(&self, node_type: &str) -> Option<Arc<dyn ExecutableNode>> {
            (node_type == "echo").then(|| Arc::new(EchoConfig) as Arc<dyn ExecutableNode>)
        }
        fn get_all_nodes(&self) -> HashMap<String, Arc<dyn ExecutableNode>> {
            HashMap::new()
        }
    }

    #[derive(Default)]
    struct MemRepo(Mutex<HashMap<String, DagRunState>>);
    impl MemRepo {
        fn row(&self, id: &str) -> DagRunState {
            self.0.lock().unwrap()[id].clone()
        }
    }
    #[async_trait]
    impl DagStateRepository for MemRepo {
        async fn get_by_id(&self, id: &str) -> Result<Option<DagRunState>, DagError> {
            Ok(self.0.lock().unwrap().get(id).cloned())
        }
        async fn save(&self, s: &DagRunState) -> Result<(), DagError> {
            self.0
                .lock()
                .unwrap()
                .insert(s.session_id.clone(), s.clone());
            Ok(())
        }
        async fn find_resume_entry(&self, _: &str) -> Result<Option<String>, DagError> {
            Ok(None)
        }
        async fn find_suspended_child(&self, _: &str) -> Result<Option<String>, DagError> {
            Ok(None)
        }
        // `fail_if_suspended` is left at the trait default: it composes
        // `get_by_id`/`save` above, which this repo already implements
        // faithfully, so the default's guard logic is exactly what's under
        // test here (see `fail_if_suspended_leaves_a_completed_row_untouched`).
        async fn fail_suspended_descendants(&self, session_id: &str) -> Result<u64, DagError> {
            let mut guard = self.0.lock().unwrap();
            // Transitive walk over parent_session_id, breadth-first from
            // session_id (excluded — the caller closes that row itself via
            // `fail_if_suspended`).
            let mut frontier = vec![session_id.to_string()];
            let mut to_flip = Vec::new();
            while let Some(parent) = frontier.pop() {
                for (id, row) in guard.iter() {
                    if row.parent_session_id.as_deref() == Some(parent.as_str()) {
                        to_flip.push(id.clone());
                        frontier.push(id.clone());
                    }
                }
            }
            let mut flipped = 0u64;
            for id in to_flip {
                if let Some(row) = guard.get_mut(&id) {
                    if row.status == DagRunStatus::Suspended {
                        row.status = DagRunStatus::Failed;
                        flipped += 1;
                    }
                }
            }
            Ok(flipped)
        }
    }

    /// One `echo` node whose config says which version of the graph it is.
    fn graph_with(node_id: &str, stamp: &str) -> Value {
        json!({ "nodes": { node_id: { "type": "echo", "config": { "stamp": stamp } } }, "edges": [] })
    }

    /// A child suspended with `stored` as its graph and `sello` next in its queue.
    fn suspended_child(stored: Value) -> (DagRunUseCase, Arc<MemRepo>) {
        let repo = Arc::new(MemRepo::default());
        repo.0.lock().unwrap().insert(
            "child_1".into(),
            DagRunState {
                session_id: "child_1".into(),
                agent_session_id: Some("chat_1".into()),
                parent_session_id: Some("root_1".into()),
                graph_json: stored,
                all_outputs: HashMap::new(),
                status: DagRunStatus::Suspended,
                global_shared_state: json!({}),
                active_queue: VecDeque::from(["sello".to_string()]),
                execution_history: Vec::new(),
                global_calls: HashMap::new(),
                caller_specific_calls: HashMap::new(),
            },
        );
        let uc = DagRunUseCase::new(
            Arc::new(EchoRegistry),
            Some(repo.clone() as Arc<dyn DagStateRepository>),
        );
        (uc, repo)
    }

    async fn resume(uc: &DagRunUseCase, graph: ResumeGraph) -> Result<Value, DagError> {
        uc.resume_subgraph(
            "child_1",
            "Q[q]: ?\nA[q]: sí".into(),
            graph,
            None,
            None,
            None,
        )
        .await
    }

    /// A bare row for seeding extra chain members / unrelated rows in a test.
    fn bare_row(id: &str, parent: Option<&str>, status: DagRunStatus) -> DagRunState {
        DagRunState {
            session_id: id.into(),
            agent_session_id: Some("chat_1".into()),
            parent_session_id: parent.map(|s| s.to_string()),
            graph_json: json!({}),
            all_outputs: HashMap::new(),
            status,
            global_shared_state: json!({}),
            active_queue: VecDeque::new(),
            execution_history: Vec::new(),
            global_calls: HashMap::new(),
            caller_specific_calls: HashMap::new(),
        }
    }

    #[tokio::test]
    async fn a_fresh_graph_with_the_same_skeleton_resumes_with_its_new_config() {
        let (uc, repo) = suspended_child(graph_with("sello", "v1"));
        let out = resume(&uc, ResumeGraph::Fresh(graph_with("sello", "v2")))
            .await
            .expect("resumes");
        assert_eq!(out["sello"]["stamp"], json!("v2"));
        assert_eq!(repo.row("child_1").status, DagRunStatus::Completed);
    }

    #[tokio::test]
    async fn a_changed_skeleton_is_refused_and_closes_the_row_keeping_its_graph() {
        let (uc, repo) = suspended_child(graph_with("sello", "v1"));
        let err = resume(
            &uc,
            ResumeGraph::Fresh(graph_with("timbre", "sk-fresh-secret")),
        )
        .await
        .expect_err("refused");
        let text = err.to_string();
        assert!(text.starts_with("SUBGRAPH_RESUME_INCOMPATIBLE:"), "{text}");
        assert!(
            text.contains("removed: sello") && text.contains("added: timbre"),
            "{text}"
        );
        assert!(!text.contains("sk-fresh-secret"), "{text}");
        let row = repo.row("child_1");
        assert_eq!(row.status, DagRunStatus::Failed);
        assert_eq!(
            row.graph_json,
            graph_with("sello", "v1"),
            "the fresh graph never reaches storage"
        );
        // `close_refused` only flips `status`; the rest of the row survives.
        assert_eq!(row.agent_session_id, Some("chat_1".to_string()));
        assert_eq!(row.parent_session_id, Some("root_1".to_string()));
        assert_eq!(row.active_queue, VecDeque::from(["sello".to_string()]));
    }

    #[tokio::test]
    async fn an_unavailable_source_closes_the_row_and_returns_its_text_verbatim() {
        let (uc, repo) = suspended_child(graph_with("sello", "v1"));
        let reason = "CHILD_GRAPH_RESOLVE_FAILED:forbidden: not in their selector";
        let err = resume(&uc, ResumeGraph::Unavailable(reason.into()))
            .await
            .expect_err("refused");
        assert_eq!(
            err.to_string(),
            reason,
            "no «Error de ejecución en el nodo:» in front"
        );
        assert_eq!(repo.row("child_1").status, DagRunStatus::Failed);
    }

    fn at_rest(graph: Value) -> Value {
        GraphSkeleton::at_rest_json(&serde_json::from_value(graph).expect("graph"))
    }

    /// Since v0.19 a row keeps only the skeleton. A resume still runs the
    /// fresh graph, and the row it leaves behind is at rest again.
    #[tokio::test]
    async fn a_row_kept_at_rest_resumes_with_the_fresh_graph() {
        let (uc, repo) = suspended_child(at_rest(graph_with("sello", "v1")));
        let out = resume(&uc, ResumeGraph::Fresh(graph_with("sello", "v2")))
            .await
            .expect("resumes");
        assert_eq!(out["sello"]["stamp"], json!("v2"));
        let row = repo.row("child_1");
        assert_eq!(row.status, DagRunStatus::Completed);
        assert_eq!(row.graph_json, at_rest(graph_with("sello", "v2")));
    }

    /// Pins the fixed text in `plan_resume`'s `Err(_) => …` branch: a serde
    /// error can quote a value straight out of the graph.
    #[tokio::test]
    async fn a_fresh_graph_that_fails_to_parse_is_refused_without_leaking_the_bad_value() {
        let (uc, repo) = suspended_child(graph_with("sello", "v1"));
        let mut fresh = graph_with("sello", "v2");
        fresh["nodes"]["sello"]["max_total_calls"] = json!("sk-fresh-secret");
        let err = resume(&uc, ResumeGraph::Fresh(fresh))
            .await
            .expect_err("refused");
        let text = err.to_string();
        assert!(text.starts_with("SUBGRAPH_RESUME_INCOMPATIBLE:"), "{text}");
        assert!(!text.contains("sk-fresh-secret"), "{text}");
        assert_eq!(repo.row("child_1").status, DagRunStatus::Failed);
    }

    /// Adjustment 4: an unreadable STORED graph is today's error, not a
    /// refusal — parsing it short-circuits before `close_refused` runs.
    #[tokio::test]
    async fn an_unparsable_stored_graph_is_todays_error_and_does_not_close_the_row() {
        let (uc, repo) = suspended_child(json!({ "nodes": 1, "edges": [] }));
        let err = resume(&uc, ResumeGraph::Fresh(graph_with("sello", "v2")))
            .await
            .expect_err("today's error, not a refusal");
        let text = err.to_string();
        assert!(text.contains("Invalid sub-graph state JSON:"), "{text}");
        assert!(!text.starts_with("SUBGRAPH_RESUME_INCOMPATIBLE:"), "{text}");
        assert_eq!(repo.row("child_1").status, DagRunStatus::Suspended);
    }

    /// Adjustment 4's other no-close case: `Graph::validate()` runs *outside*
    /// `plan_resume`, so its failure is today's error, not a `ResumeRefused`.
    #[tokio::test]
    async fn a_fresh_graph_that_fails_validation_does_not_close_the_row() {
        let (uc, repo) = suspended_child(graph_with("router/inner", "v1"));
        let err = resume(&uc, ResumeGraph::Fresh(graph_with("router/inner", "v2")))
            .await
            .expect_err("today's validation error, not a refusal");
        let text = err.to_string();
        assert!(text.contains("Invalid sub-graph:"), "{text}");
        assert!(!text.starts_with("SUBGRAPH_RESUME_INCOMPATIBLE:"), "{text}");
        assert_eq!(repo.row("child_1").status, DagRunStatus::Suspended);
    }

    /// `plan_resume` parses STORED before `fresh`: when both are unparsable,
    /// the stored failure must win over a Fresh-side refusal that would close.
    #[tokio::test]
    async fn when_both_graphs_are_unparsable_the_stored_failure_wins_and_nothing_closes() {
        let (uc, repo) = suspended_child(json!({ "nodes": 1, "edges": [] }));
        let err = resume(&uc, ResumeGraph::Fresh(json!({ "nodes": 2, "edges": [] })))
            .await
            .expect_err("the stored failure, not a refusal");
        let text = err.to_string();
        assert!(text.contains("Invalid sub-graph state JSON:"), "{text}");
        assert!(!text.starts_with("SUBGRAPH_RESUME_INCOMPATIBLE:"), "{text}");
        assert_eq!(repo.row("child_1").status, DagRunStatus::Suspended);
    }

    /// A refused resume of `child_1` (parent `root_1`) closes `child_1` AND
    /// its own SUSPENDED descendant `grandchild_1` — otherwise a row left
    /// SUSPENDED under a FAILED parent counts as a second chain in
    /// `find_resume_entry`. `root_1` (the parent, not a descendant) and an
    /// unrelated SUSPENDED row elsewhere must stay untouched.
    #[tokio::test]
    async fn a_refused_resume_closes_its_suspended_descendants_but_not_unrelated_rows() {
        let (uc, repo) = suspended_child(graph_with("sello", "v1"));
        {
            let mut rows = repo.0.lock().unwrap();
            rows.insert(
                "grandchild_1".into(),
                bare_row("grandchild_1", Some("child_1"), DagRunStatus::Suspended),
            );
            rows.insert(
                "root_1".into(),
                bare_row("root_1", None, DagRunStatus::Suspended),
            );
            rows.insert(
                "unrelated_1".into(),
                bare_row("unrelated_1", Some("other_root"), DagRunStatus::Suspended),
            );
        }

        let err = resume(&uc, ResumeGraph::Fresh(graph_with("timbre", "v2")))
            .await
            .expect_err("refused");
        assert!(err.to_string().starts_with("SUBGRAPH_RESUME_INCOMPATIBLE:"));

        assert_eq!(repo.row("child_1").status, DagRunStatus::Failed);
        assert_eq!(repo.row("grandchild_1").status, DagRunStatus::Failed);
        assert_eq!(
            repo.row("root_1").status,
            DagRunStatus::Suspended,
            "the parent is not a descendant"
        );
        assert_eq!(
            repo.row("unrelated_1").status,
            DagRunStatus::Suspended,
            "an unrelated chain must stay untouched"
        );
    }

    /// `fail_if_suspended`'s guard (the trait default: `get_by_id` + `save`)
    /// never flips a row that isn't SUSPENDED.
    #[tokio::test]
    async fn fail_if_suspended_leaves_a_completed_row_untouched() {
        let repo = MemRepo::default();
        repo.0.lock().unwrap().insert(
            "done_1".into(),
            bare_row("done_1", None, DagRunStatus::Completed),
        );
        let flipped = repo.fail_if_suspended("done_1").await.unwrap();
        assert!(!flipped);
        assert_eq!(repo.row("done_1").status, DagRunStatus::Completed);
    }
}
