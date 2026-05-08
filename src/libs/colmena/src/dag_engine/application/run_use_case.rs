use crate::colmena_log;
use crate::dag_engine::application::ports::{NodeRegistryPort, SubGraphExecutorPort};
use crate::dag_engine::application::secure_value_service::SecureValueService;
use crate::dag_engine::domain::error::DagError;
use crate::dag_engine::domain::graph::{Edge, Graph};
use crate::dag_engine::domain::node::NodeInputs;

use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use crate::dag_engine::domain::state::{DagRunState, DagRunStatus, DagStateRepository};

/// El "Caso de Uso" que orquesta la ejecución de un grafo.
#[derive(Clone)]
pub struct DagRunUseCase {
    registry: Arc<dyn NodeRegistryPort>,
    state_repository: Option<Arc<dyn DagStateRepository>>,
    secure_value_service: Option<Arc<SecureValueService>>,
    /// Optional bus notified when a conversation is considered finished.
    /// Registries (web nodes) subscribe to eagerly evict scoped state.
    /// Fire-site is deferred to Plan C (see TODO near `DagRunStatus::Completed`).
    conversation_lifecycle: Option<crate::web::domain::ConversationLifecycleBus>,
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
            conversation_lifecycle: None,
        }
    }

    /// Attaches a conversation lifecycle bus so that session-bearing web registries
    /// can be notified when the engine considers a conversation finished.
    ///
    /// **Order matters:** this is a builder-style method (takes `self` and returns `Self`),
    /// so it must be called AFTER `with_secure_values_and_service`. Calling that
    /// constructor afterwards produces a fresh instance and will silently drop the bus.
    pub fn with_conversation_lifecycle(
        mut self,
        bus: crate::web::domain::ConversationLifecycleBus,
    ) -> Self {
        self.conversation_lifecycle = Some(bus);
        self
    }

    /// Creates a new DagRunUseCase with a pre-built SecureValueService (shared with the registry).
    ///
    /// Esta es la única vía oficial para inyectar secure values. La aplicación
    /// no debe tocar `infrastructure/`: el caller construye el adapter
    /// concreto (`PostgresSecureValueRepository`) y arma el service afuera.
    ///
    /// Note: this is a fresh constructor — it does not preserve a previously attached
    /// `ConversationLifecycleBus`. Chain `.with_conversation_lifecycle(...)` after this
    /// call if you need lifecycle notifications.
    pub fn with_secure_values_and_service(
        registry: Arc<dyn NodeRegistryPort>,
        state_repository: Option<Arc<dyn DagStateRepository>>,
        secure_value_service: Arc<SecureValueService>,
    ) -> Self {
        Self {
            registry,
            state_repository,
            secure_value_service: Some(secure_value_service),
            conversation_lifecycle: None,
        }
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
    ) -> impl futures::Stream<
        Item = Result<crate::dag_engine::domain::events::DagExecutionEvent, DagError>,
    > {
        async_stream::try_stream! {
            use crate::dag_engine::domain::events::DagExecutionEvent;
            use crate::dag_engine::domain::observer::NodeEvent;

            let mut all_outputs: HashMap<String, Value> = HashMap::new();
            let mut active_queue: VecDeque<String> = VecDeque::new();
            let mut session_id = uuid::Uuid::new_v4().to_string();
            let mut active_agent_session_id: Option<String> = agent_session_id.clone();
            let mut parent_session_id_for_save: Option<String> = None;
            let mut execution_history: Vec<(String, String)> = Vec::new();
            let mut global_calls: HashMap<String, u32> = HashMap::new();
            let mut caller_specific_calls: HashMap<String, HashMap<String, u32>> = HashMap::new();
            let mut global_shared_state = serde_json::json!({});

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

            if !global_shared_state.is_object() {
                global_shared_state = serde_json::json!({});
            }
            if let Some(obj) = global_shared_state.as_object_mut() {
                obj.insert("session_id".to_string(), Value::String(session_id.clone()));

                let mut graph_nodes_meta = serde_json::Map::new();
                for (nid, node) in &graph.nodes {
                    graph_nodes_meta.insert(nid.clone(), node.config.clone());
                }
                obj.insert("__graph_nodes".to_string(), Value::Object(graph_nodes_meta));
            }

            let mut current_caller: Option<String> = None;

            // Usage tracking: accumulate token counts and model/provider per node_id.
            // node_meta: node_id → (model, provider, node_type)
            // usage_accumulator: node_id → (prompt, completion, thinking, cache_read, cache_write)
            let mut node_meta: HashMap<String, (Option<String>, Option<String>, String)> = HashMap::new();
            let mut usage_accumulator: HashMap<String, (u32, u32, u32, u32, u32)> = HashMap::new();

            // Start cyclic execution loop
            while let Some(node_id) = active_queue.pop_front() {

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
                        let mut inputs_value = serde_json::to_value(&inputs)
                            .unwrap_or(Value::Object(Default::default()));
                        if let Err(e) = svc.inject_secrets(&mut inputs_value, &session_id).await {
                            eprintln!("⚠️ Failed to inject secrets: {}", e);
                        }
                        if let Ok(injected_inputs) = serde_json::from_value::<NodeInputs>(inputs_value) {
                            inputs = injected_inputs;
                        }
                        // Config secrets are stored at the agent/workspace level
                        // (identified by active_agent_session_id when available),
                        // so look them up using that identifier rather than the
                        // ephemeral per-run session_id.
                        let config_secret_session = active_agent_session_id
                            .as_deref()
                            .unwrap_or(&session_id);
                        if let Err(e) = svc.inject_secrets(&mut node_config_value, config_secret_session).await {
                            eprintln!("⚠️ Failed to inject secrets in config: {}", e);
                        }
                    }
                }

                if let Some(ans) = &resume_answer {
                    inputs.insert("__colmena_resume_answer".to_string(), Value::String(ans.clone()));
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
                inputs.insert(
                    "__colmena_agent_session_id".to_string(),
                    match &active_agent_session_id {
                        Some(a) => Value::String(a.clone()),
                        None => Value::Null,
                    },
                );

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
                    node_meta.insert(node_id.clone(), (model, provider, node_config.node_type.clone()));
                }

                yield DagExecutionEvent::NodeStart {
                    node_id: node_id.clone(),
                    node_type: node_config.node_type.clone(),
                    inputs: serde_json::to_value(&inputs).unwrap_or(Value::Null),
                    config: node_config_value.clone(),
                };

                let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
                let observer = Arc::new(ChannelObserver { tx });

                let output = {
                    let execution_future = node_impl.execute(&inputs, &node_config_value, &mut global_shared_state, Some(observer));
                    tokio::pin!(execution_future);

                    let mut output_opt = None;
                    loop {
                        tokio::select! {
                            res = &mut execution_future, if output_opt.is_none() => {
                                output_opt = Some(res);
                            }
                            event_opt = rx.recv() => {
                                match event_opt {
                                    Some(event) => {
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
                                            NodeEvent::LlmToolCallStart { tool_id, tool_name, tool_args } => yield DagExecutionEvent::LlmToolCallStart { node_id: node_id.clone(), tool_id, tool_name, tool_args },
                                            NodeEvent::LlmToolCallFinish { tool_id, success, output } => yield DagExecutionEvent::LlmToolCallFinish { node_id: node_id.clone(), tool_id, success, output },
                                            NodeEvent::SkillLoaded { tool_id, skill_name, reference, source, size_bytes } => yield DagExecutionEvent::SkillLoaded { node_id: node_id.clone(), tool_id, skill_name, reference, source, size_bytes },
                                            NodeEvent::ToolDescribed { tool_id, tool_name } => yield DagExecutionEvent::ToolDescribed { node_id: node_id.clone(), tool_id, tool_name },
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
                                                            node_meta.insert(cid.clone(), (model, provider, ctype.clone()));
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
                                                        other => yield DagExecutionEvent::SubgraphWrapped { inner: Box::new(other) },
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

                    let output_result = output_opt.unwrap_or_else(|| {
                        Err(Box::new(DagError::NodeExecution("Future did not complete but channel closed".to_string())))
                    });
                    output_result.map_err(|e| DagError::NodeExecution(e.to_string()))?
                };

                // STEP 2: Hash output if secure: true (after executing)
                let mut processed_output = output.clone();
                if let Some(svc) = &self.secure_value_service {
                    match svc
                        .hash_output(&processed_output, &node_config.config, &session_id, &node_id)
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

                if node_config.node_type == "subgraph" {
                    yield DagExecutionEvent::SubgraphNodeFinish {
                        node_id: node_id.clone(),
                        output: processed_output.clone(),
                    };
                } else {
                    yield DagExecutionEvent::NodeFinish {
                        node_id: node_id.clone(),
                        output: processed_output.clone(),
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
                            graph_json: serde_json::to_value(&graph).unwrap_or(Value::Null),
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

                    yield DagExecutionEvent::GraphFinish { output: final_output };
                    return;
                }

                all_outputs.insert(node_id.to_string(), processed_output.clone());

                // --- DYNAMICALLY PUSH TO QUEUE BASED ON EDGES ---
                // If a node emitted Value::Null intentionally (skip stub), do not traverse its descendants
                if !processed_output.is_null() {
                    let outgoing_edges = graph.edges.iter().filter(|e| e.from == node_id || e.from.starts_with(&format!("{}.", node_id)));
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

                        if has_data {
                            let target_node_id = edge.to.split('.').next().unwrap_or("");
                            if graph.nodes.contains_key(target_node_id)
                                && !active_queue.contains(&target_node_id.to_string()) {
                                    colmena_log!("DEBUG [Queue Push]: Enqueuing {} -> {}", node_id, target_node_id);
                                    active_queue.push_back(target_node_id.to_string());
                                }
                        }
                    }
                }

                current_caller = Some(node_id.clone());
            }

            // Completed
            if let Some(repo) = &self.state_repository {
                let state = DagRunState {
                    session_id: session_id.clone(),
                    agent_session_id: active_agent_session_id.clone(),
                    parent_session_id: parent_session_id_for_save.clone(),
                    graph_json: serde_json::to_value(&graph).unwrap_or(Value::Null),
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
            // TODO(plan-C): when conversations span multiple runs and a final
            // "conversation closed" signal is available here, fire:
            //   if let Some(bus) = &self.conversation_lifecycle {
            //       bus.notify_conversation_closed(&conversation_id).await;
            //   }
            // Today a DAG run !== a conversation, so firing on every completion
            // would be premature.

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

            // CLEANUP: Delete all secure values for this session
            if let Some(svc) = &self.secure_value_service {
                if let Err(e) = svc.cleanup(&session_id).await {
                    eprintln!("⚠️ Failed to cleanup secure values: {}", e);
                }
            }

            // Emit per-node usage summary before finishing
            if !usage_accumulator.is_empty() {
                let entries: Vec<Value> = usage_accumulator.iter().map(|(nid, (pt, ct, tt, cr, cw))| {
                    let (model, provider, ntype) = node_meta.get(nid)
                        .map(|(m, p, t)| (m.clone(), p.clone(), t.clone()))
                        .unwrap_or((None, None, String::new()));
                    let mut obj = serde_json::Map::new();
                    obj.insert("node_id".into(), json!(nid));
                    obj.insert("node_type".into(), json!(ntype));
                    obj.insert("model".into(), json!(model));
                    obj.insert("provider".into(), json!(provider));
                    obj.insert("prompt_tokens".into(), json!(pt));
                    obj.insert("completion_tokens".into(), json!(ct));
                    if *tt > 0 { obj.insert("thinking_tokens".into(), json!(tt)); }
                    if *cr > 0 { obj.insert("cache_read_tokens".into(), json!(cr)); }
                    if *cw > 0 { obj.insert("cache_write_tokens".into(), json!(cw)); }
                    obj.insert("total_tokens".into(), json!(pt + ct + tt));
                    Value::Object(obj)
                }).collect();
                yield DagExecutionEvent::GraphUsageSummary { entries };
            }

            yield DagExecutionEvent::GraphFinish { output: final_aggregated_output.clone() };
        }
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
        let graph: Graph = serde_json::from_value(graph_json.clone())
            .map_err(|e| DagError::NodeExecution(format!("Invalid sub-graph JSON: {}", e)))?;
        graph
            .validate()
            .map_err(|e| DagError::NodeExecution(format!("Invalid sub-graph: {}", e)))?;

        // Mapear globales del hijo a la tabla para el inicio
        if let Some(repo) = &self.state_repository {
            let initial_state = DagRunState {
                session_id: session_id.to_string(),
                agent_session_id: agent_session_id.clone(),
                parent_session_id: parent_session_id.clone(),
                graph_json,
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
        let mut stream = Box::pin(self.clone().execute_stream(
            graph,
            Some(session_id.to_string()),
            None,
            true,
            path_prefix,
            agent_session_id,
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

    async fn resume_subgraph(
        &self,
        session_id: &str,
        answer: String,
        observer: Option<Arc<dyn crate::dag_engine::domain::observer::ExecutionObserver>>,
        agent_session_id: Option<String>,
        path_prefix: Option<String>,
    ) -> Result<Value, DagError> {
        let state = if let Some(repo) = &self.state_repository {
            repo.get_by_id(session_id).await?.ok_or_else(|| {
                DagError::NodeExecution(format!(
                    "Child session {} not found for resume",
                    session_id
                ))
            })?
        } else {
            return Err(DagError::NodeExecution(
                "State repository missing for resume".to_string(),
            ));
        };

        let graph: Graph = serde_json::from_value(state.graph_json)
            .map_err(|e| DagError::NodeExecution(format!("Invalid sub-graph state JSON: {}", e)))?;
        graph
            .validate()
            .map_err(|e| DagError::NodeExecution(format!("Invalid sub-graph: {}", e)))?;

        use futures::StreamExt;
        let mut stream = Box::pin(self.clone().execute_stream(
            graph,
            Some(session_id.to_string()),
            Some(answer),
            true,
            path_prefix,
            agent_session_id.or(state.agent_session_id),
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
