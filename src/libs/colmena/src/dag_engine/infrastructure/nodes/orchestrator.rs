use crate::colmena_log;
use crate::dag_engine::application::ports::NodeRegistryPort;
use crate::dag_engine::domain::events::DagExecutionEvent;
use crate::dag_engine::domain::node::{ExecutableNode, NodeInputs};
use crate::dag_engine::domain::observer::{ExecutionObserver, NodeEvent};
use crate::dag_engine::domain::state::{DagTask, DagTaskMemoryRepository};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::error::Error as StdError;
use std::sync::{Arc, Weak};

/// Template for the phase reactor schema constraints injected by the orchestrator.
/// Uses `{agents_list}` as a placeholder for the available agents.
const PHASE_REACTOR_TEMPLATE: &str = include_str!("prompts/orchestrator_phase_reactor.md");

/// Anti-hallucination grounding rules appended to both reactor system messages
/// after the schema template / user-provided system message.
const ORCHESTRATOR_GROUNDING: &str = include_str!("prompts/orchestrator_grounding.md");

/// Default system message used as a fallback when the user has not provided one
/// for the final reactor. The grounding rules are appended on top of this.
const LLM_DEFAULT_SYSTEM: &str = include_str!("prompts/llm_default_system.md");

// Config key names — these are the keys used in the orchestrator JSON config block
// and also serve as the node_id for internal SSE events. Defined here as constants
// to keep the config lookup and the SSE emission in sync with a single source of truth.
const KEY_PLANNER: &str = "planner";
const KEY_PHASE_REACTOR: &str = "phase_reactor";
const KEY_FINAL_REACTOR: &str = "final_reactor";
const KEY_CRITIC: &str = "critic";

// Node type names for SSE events (match the Rust node registry names).
const NODE_TYPE_PLANNER: &str = "planner";
const NODE_TYPE_REACTOR: &str = "reactor";
const NODE_TYPE_CRITIC: &str = "critic";

fn emit_internal_node_start(
    observer: &Option<Arc<dyn ExecutionObserver>>,
    node_id: &str,
    node_type: &str,
    inputs: Value,
) {
    if let Some(obs) = observer {
        let event = DagExecutionEvent::NodeStart {
            node_id: node_id.to_string(),
            node_type: node_type.to_string(),
            inputs,
            config: Value::Object(Default::default()),
        };
        if let Ok(raw) = serde_json::to_value(&event) {
            obs.on_event(NodeEvent::SubgraphChildEvent(raw));
        }
    }
}

fn emit_internal_node_finish(
    observer: &Option<Arc<dyn ExecutionObserver>>,
    node_id: &str,
    output: Value,
) {
    if let Some(obs) = observer {
        let event = DagExecutionEvent::NodeFinish {
            node_id: node_id.to_string(),
            output,
        };
        if let Ok(raw) = serde_json::to_value(&event) {
            obs.on_event(NodeEvent::SubgraphChildEvent(raw));
        }
    }
}

// ── DirectThinkingObserver ───────────────────────────────────────────────────
// Fires LlmToken as ThinkingToken DIRECTLY to the parent (no SubgraphChildEvent
// wrapping). Used for the orchestrator's internal LLMs (planner, critic,
// phase_reactor, final_reactor) so their tokens reach run_use_case as plain
// NodeEvent::ThinkingToken → DagExecutionEvent::ThinkingToken → "thinking-delta".
// The node_id identifies the internal sub-node (e.g. "planner", "critic") so the
// frontend can match thinking-delta events to the corresponding subgraph-node-start.
struct DirectThinkingObserver {
    inner: Arc<dyn ExecutionObserver>,
    node_id: String,
    node_type: String,
}

impl ExecutionObserver for DirectThinkingObserver {
    fn on_event(&self, event: NodeEvent) {
        match event {
            NodeEvent::LlmToken { token } => {
                self.inner.on_event(NodeEvent::ThinkingToken {
                    node_id: self.node_id.clone(),
                    node_type: self.node_type.clone(),
                    token,
                });
            }
            NodeEvent::LlmUsage { .. }
            | NodeEvent::ReasoningStart { .. }
            | NodeEvent::ReasoningDelta { .. }
            | NodeEvent::ReasoningEnd { .. } => {
                self.inner.on_event(event);
            }
            _ => {}
        }
    }
}

fn direct_thinking_observer(
    node_id: &str,
    node_type: &str,
    observer: &Option<Arc<dyn ExecutionObserver>>,
) -> Option<Arc<dyn ExecutionObserver>> {
    observer.as_ref().map(|obs| {
        Arc::new(DirectThinkingObserver {
            inner: obs.clone(),
            node_id: node_id.to_string(),
            node_type: node_type.to_string(),
        }) as Arc<dyn ExecutionObserver>
    })
}

/// Returned by `handle_phase_completion` and `finalize_execution` to signal whether
/// execution should continue or propagate a suspend to the DAG engine.
enum OrchestratorSuspend {
    Done(Value),
    Suspended(Value),
}

/// A single question in a multi-question Human-in-the-Loop suspend.
/// `question_type` is either "open" (free text) or "choice" (predefined options).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct SuspendQuestion {
    id: String,
    question: String,
    #[serde(rename = "type")]
    question_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    options: Option<Vec<String>>,
}

pub struct OrchestratorNode {
    task_memory_repo: Option<Arc<dyn DagTaskMemoryRepository>>,
    registry: Weak<dyn NodeRegistryPort>,
}

impl OrchestratorNode {
    pub fn new(
        task_memory_repo: Option<Arc<dyn DagTaskMemoryRepository>>,
        registry: Weak<dyn NodeRegistryPort>,
    ) -> Self {
        Self {
            task_memory_repo,
            registry,
        }
    }

    async fn handle_phase_completion(
        &self,
        repo: &Arc<dyn DagTaskMemoryRepository>,
        session_id: &str,
        phase: i32,
        config: &Value,
        state: &mut Value,
        observer: Option<Arc<dyn crate::dag_engine::domain::observer::ExecutionObserver>>,
    ) -> Result<OrchestratorSuspend, Box<dyn StdError + Send + Sync>> {
        colmena_log!(
            "🏁 [OrchestratorNode] Phase {} complete. Processing reaction...",
            phase
        );

        let all_tasks = repo.get_tasks_for_run(session_id).await?;
        let phase_tasks: Vec<_> = all_tasks.iter().filter(|t| t.phase == phase).collect();

        let phase_output;

        if let Some(reactor_cfg) = config.get(KEY_PHASE_REACTOR) {
            colmena_log!("⚡ [OrchestratorNode] Internal Phase Reactor starting...");
            let registry = self.registry.upgrade().ok_or("Registry already dropped")?;
            let reactor_node = registry
                .get_node("reactor")
                .ok_or("Reactor node not found")?;

            let mut reactor_inputs = HashMap::new();
            reactor_inputs.insert("phase".to_string(), json!(phase));

            for task in &phase_tasks {
                if let Some(res) = &task.result {
                    // LlmNode stores results as { "result": "...", "extra_info": ... }
                    // Fall back to "content" for nodes that use the older convention.
                    let text = res
                        .get("result")
                        .and_then(|c| c.as_str())
                        .or_else(|| res.get("content").and_then(|c| c.as_str()))
                        .unwrap_or("");
                    if !text.is_empty() {
                        reactor_inputs.insert(
                            format!("texts.{}", task.assigned_to),
                            Value::String(text.to_string()),
                        );
                    }
                }
            }

            // ── Step 2: Forzar Schema del Reactor ────────────────────────────
            // Inyectar en el system_message del reactor las instrucciones de formato
            // y la lista de agentes disponibles para evitar alucinaciones en new_tasks.
            let mut available_agents: Vec<(String, String)> = Vec::new();
            if let Some(agents_obj) = config.get("agents").and_then(|a| a.as_object()) {
                for (name, props) in agents_obj {
                    let desc = props
                        .get("description")
                        .and_then(|v| v.as_str())
                        .unwrap_or("No description provided");
                    available_agents.push((name.clone(), desc.to_string()));
                }
            }

            let mut reactor_cfg_owned = reactor_cfg.clone();
            inject_reactor_schema_constraints(&mut reactor_cfg_owned, &available_agents);

            // Opción 3: Inyectar historial de agentes ya ejecutados en session.
            // El reactor sabrá que esos agentes ya corrieron y evitará re-proponerlos
            // como recovery tasks a menos que sea genuinamente necesario.
            let all_tasks_so_far = repo.get_tasks_for_run(session_id).await.unwrap_or_default();
            let completed_history: Vec<String> = all_tasks_so_far
                .iter()
                .filter(|t| t.completed)
                .map(|t| {
                    format!(
                        "- {} (agent: {}, phase: {})",
                        t.task_name, t.assigned_to, t.phase
                    )
                })
                .collect();

            if !completed_history.is_empty() {
                let history_note = format!(
                    "\n\nTASKS ALREADY COMPLETED IN THIS SESSION (do NOT propose these again unless strictly necessary):\n{}",
                    completed_history.join("\n")
                );
                if let Some(obj) = reactor_cfg_owned.as_object_mut() {
                    let existing_sm = obj
                        .get("system_message")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    obj.insert(
                        "system_message".to_string(),
                        Value::String(format!("{}{}", existing_sm, history_note)),
                    );
                }
            }

            // Inject Q&A from a previous phase_reactor suspend (user answered, reactor re-runs with context)
            if let Some(qa) = state.get("__orchestrator_phase_reactor_qa") {
                if let Some(ctx) = qa.get("qa_context").and_then(|v| v.as_str()) {
                    if let Some(obj) = reactor_cfg_owned.as_object_mut() {
                        let existing_sm = obj
                            .get("system_message")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        obj.insert(
                            "system_message".to_string(),
                            Value::String(format!(
                                "{}\n\nUSER CLARIFICATION (answer to your previous question):\n{}",
                                existing_sm, ctx
                            )),
                        );
                    }
                    colmena_log!(
                        "💬 [OrchestratorNode] Phase {} reactor re-running with user clarification injected.",
                        phase
                    );
                }
            }
            // Clean up the stash after injecting
            if let Some(obj) = state.as_object_mut() {
                obj.remove("__orchestrator_phase_reactor_qa");
            }

            colmena_log!(
                "📨 [OrchestratorNode] REACTOR INPUTS (phase {}):\n{}\n{}",
                phase,
                "─".repeat(60),
                serde_json::to_string_pretty(&reactor_inputs)
                    .unwrap_or_else(|_| format!("{:?}", reactor_inputs))
            );

            emit_internal_node_start(
                &observer,
                KEY_PHASE_REACTOR,
                NODE_TYPE_REACTOR,
                json!({
                    "phase": phase,
                    "model": reactor_cfg_owned.get("model").cloned().unwrap_or(Value::Null),
                    "provider": reactor_cfg_owned.get("provider").cloned().unwrap_or(Value::Null),
                }),
            );
            let phase_reactor_obs =
                direct_thinking_observer(KEY_PHASE_REACTOR, NODE_TYPE_REACTOR, &observer);
            let reactor_res = reactor_node
                .execute(
                    &reactor_inputs,
                    &reactor_cfg_owned,
                    state,
                    phase_reactor_obs,
                )
                .await?;
            emit_internal_node_finish(&observer, KEY_PHASE_REACTOR, reactor_res.clone());

            colmena_log!(
                "📬 [OrchestratorNode] REACTOR RAW RESULT (phase {}):\n{}\n{}\n{}",
                phase,
                "─".repeat(60),
                serde_json::to_string_pretty(&reactor_res)
                    .unwrap_or_else(|_| format!("{:?}", reactor_res)),
                "─".repeat(60)
            );

            // ── Suspend check ─────────────────────────────────────────────────
            let reactor_suspended = reactor_res
                .get("extra_info")
                .and_then(|e| e.get("suspend"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            if reactor_suspended {
                let question_text = reactor_res
                    .get("extra_info")
                    .and_then(|e| e.get("question"))
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .unwrap_or("Please clarify before continuing.")
                    .to_string();
                colmena_log!(
                    "⏸️  [OrchestratorNode] Phase {} reactor suspended. Question: {}",
                    phase,
                    question_text
                );
                let questions = vec![SuspendQuestion {
                    id: "phase_reactor_clarification".to_string(),
                    question: question_text,
                    question_type: "open".to_string(),
                    options: None,
                }];
                if allow_suspend_for(reactor_cfg) {
                    return Ok(OrchestratorSuspend::Suspended(make_suspend_response(
                        state,
                        KEY_PHASE_REACTOR,
                        phase,
                        None,
                        questions,
                    )));
                } else {
                    debug_print_questions("phase_reactor", &questions);
                }
            }

            // ── Step 2b: Persistir el Summary ────────────────────────────────
            // ReactorNode returns:
            //   { "result": <response_text_string>, "extra_info": { "task_ok": bool, "add_tasks": [...], ... } }
            // The orchestrator's inject_reactor_schema_constraints appended a custom JSON schema
            // to the reactor's system_message, so the LLM response is JSON but ReactorNode
            // only surfaces the plain `response` field as `result`.
            // We read summary from `result` (the response text) and new_tasks from `extra_info.add_tasks`.
            let reactor_result = reactor_res.get("result").cloned().unwrap_or(Value::Null);
            let reactor_add_tasks = reactor_res
                .get("extra_info")
                .and_then(|e| e.get("add_tasks"))
                .cloned();

            let (mut summary_str, new_tasks_val) =
                extract_reactor_output_with_fallback(&reactor_result, reactor_add_tasks);

            // If the reactor returned an empty summary (e.g. task_ok=false with no response text),
            // build a minimal summary from the phase task results so context is not lost.
            if summary_str.trim().is_empty() {
                let mut fallback_parts = Vec::new();
                for task in &phase_tasks {
                    if let Some(res) = &task.result {
                        let text = res.get("result").and_then(|c| c.as_str()).unwrap_or("");
                        if !text.is_empty() {
                            fallback_parts.push(format!("[{}]: {}", task.assigned_to, text));
                        }
                    }
                }
                if !fallback_parts.is_empty() {
                    summary_str = fallback_parts.join(" | ");
                }
            }

            if !summary_str.trim().is_empty() {
                repo.save_phase_summary(session_id, phase, &summary_str)
                    .await?;
                colmena_log!("💾 [OrchestratorNode] Phase {} summary saved.", phase);
            }

            // ── Step 3: Dynamic Replanning ────────────────────────────────────
            // Si el reactor detectó brechas, inserta nuevas tareas en la siguiente fase.
            // Deduplicación: no insertar si ya existe una tarea incompleta con el mismo
            // task_name + assigned_to para evitar loops infinitos cuando el reactor
            // repite la misma recomendación en cada fase.
            if let Some(new_tasks_arr) = new_tasks_val.as_ref().and_then(|v| v.as_array()) {
                let next_phase = phase + 1;
                let mut sowed = 0usize;

                // Cargar todas las tareas existentes para deduplicar
                let existing_tasks = repo.get_tasks_for_run(session_id).await.unwrap_or_default();

                for task_val in new_tasks_arr {
                    let task_name = task_val
                        .get("task")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let assigned_to = task_val
                        .get("assigned_to")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();

                    if task_name.is_empty() || assigned_to.is_empty() {
                        continue;
                    }

                    // Validar que el agente existe en config["agents"]
                    if !available_agents
                        .iter()
                        .any(|(name, _)| name == &assigned_to)
                    {
                        colmena_log!(
                            "⚠️  [OrchestratorNode] Reactor hallucinated agent '{}' — discarding task '{}'.",
                            assigned_to, task_name
                        );
                        continue;
                    }

                    // Deduplicar: si ya existe una tarea (completada o no) con el mismo
                    // nombre y agente, no insertar. Esto evita loops donde el reactor
                    // propone la misma tarea que ya fue ejecutada en una fase anterior.
                    let already_exists = existing_tasks
                        .iter()
                        .any(|t| t.assigned_to == assigned_to && t.task_name == task_name);
                    if already_exists {
                        colmena_log!(
                            "⏭️  [OrchestratorNode] Skipping duplicate recovery task '{}' for '{}' — already exists (completed={}).",
                            task_name, assigned_to,
                            existing_tasks.iter().find(|t| t.assigned_to == assigned_to && t.task_name == task_name).map(|t| t.completed).unwrap_or(false)
                        );
                        continue;
                    }

                    let parallel = task_val
                        .get("parallel")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);

                    let context = task_val
                        .get("context")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());

                    let is_bridge = task_val
                        .get("bridge")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);

                    // Bridge tasks run in the current phase (same phase that just completed)
                    // so their results are available before the next phase starts.
                    let task_phase = if is_bridge { phase } else { next_phase };

                    let new_task = DagTask {
                        id: uuid::Uuid::new_v4().to_string(),
                        session_id: session_id.to_string(),
                        task_name: task_name.clone(),
                        assigned_to: assigned_to.clone(),
                        completed: false,
                        result: None,
                        phase: task_phase,
                        parallel,
                        context,
                        is_bridge,
                    };
                    repo.add_task(&new_task).await?;
                    if is_bridge {
                        colmena_log!(
                            "🌉 [OrchestratorNode] Bridge task '{}' → '{}' seeded in phase {} (runs before phase {}).",
                            task_name, assigned_to, task_phase, next_phase
                        );
                    }
                    sowed += 1;
                }

                if sowed > 0 {
                    colmena_log!(
                        "🌱 [OrchestratorNode] Sowed {} new task(s) via Reactor into phase {}.",
                        sowed,
                        next_phase
                    );
                }
            }

            phase_output = Value::String(summary_str);
        } else {
            // ── Step 4: Fallback sin reactor ─────────────────────────────────
            colmena_log!("🔗 [OrchestratorNode] No reactor found. Concatenating phase results (truncated fallback)...");
            colmena_log!("⚠️  [OrchestratorNode] For production flows, configure 'phase_reactor' to enable context summarization and dynamic replanning.");

            emit_internal_node_start(
                &observer,
                "phase_summary",
                "phase_summary",
                json!({"phase": phase}),
            );

            let mut results = Vec::new();
            for task in &phase_tasks {
                if let Some(res) = &task.result {
                    // LlmNode stores { "result": "...", "extra_info": ... }
                    let text = res.get("result").and_then(|c| c.as_str()).unwrap_or("");
                    results.push(format!("# Agent: {}\n\n{}", task.assigned_to, text));
                }
            }
            let full_concat = results.join("\n\n---\n\n");

            // Truncar a 4000 caracteres para no explotar la ventana de contexto
            // en fases siguientes cuando no hay reactor que resuma.
            const FALLBACK_TRUNCATION_LIMIT: usize = 4000;
            let truncated = if full_concat.len() > FALLBACK_TRUNCATION_LIMIT {
                let cut = &full_concat[..FALLBACK_TRUNCATION_LIMIT];
                format!(
                    "{}\n\n[...truncated — configure phase_reactor for full context]",
                    cut
                )
            } else {
                full_concat
            };

            emit_internal_node_finish(&observer, "phase_summary", Value::String(truncated.clone()));
            // Persist the fallback summary so the final_reactor can see it.
            // Without this, final_reactor builds an empty user_message and the
            // downstream LlmMessage::user() construction fails.
            if !truncated.trim().is_empty() {
                repo.save_phase_summary(session_id, phase, &truncated)
                    .await?;
            }
            phase_output = Value::String(truncated);
        }

        Ok(OrchestratorSuspend::Done(json!({
            "phase_tasks": build_tasks_json(phase_tasks.into_iter().cloned().collect()),
            "phase_output": phase_output,
            "extra_info": {
                "__colmena_loop_status": "FINISHED_PHASE",
                "current_phase": phase
            }
        })))
    }

    async fn finalize_execution(
        &self,
        repo: &Arc<dyn DagTaskMemoryRepository>,
        session_id: &str,
        config: &Value,
        _state: &mut Value,
        observer: Option<Arc<dyn crate::dag_engine::domain::observer::ExecutionObserver>>,
        inputs: &NodeInputs,
    ) -> Result<OrchestratorSuspend, Box<dyn StdError + Send + Sync>> {
        colmena_log!("✅ [OrchestratorNode] All phases complete. Finalizing...");

        let all_tasks = repo.get_tasks_for_run(session_id).await?;
        let all_tasks_json = build_tasks_json(all_tasks);
        let phase_summaries = repo.get_phase_summaries(session_id).await?;

        let final_reactor_cfg = config
            .get(KEY_FINAL_REACTOR)
            .ok_or("OrchestratorNode: 'final_reactor' is required in config. It produces the final response to the user's query.")?;

        // ── Direct LLM call (plain text, no JSON schema) ────────────────────
        colmena_log!("⚡ [OrchestratorNode] Internal Final LLM starting...");

        let provider_str = final_reactor_cfg
            .get("provider")
            .and_then(|v| v.as_str())
            .ok_or("final_reactor: missing 'provider' in config")?;
        let provider_kind = match provider_str.to_lowercase().as_str() {
            "openai" => crate::llm::domain::ProviderKind::OpenAi,
            "gemini" => crate::llm::domain::ProviderKind::Gemini,
            "anthropic" => crate::llm::domain::ProviderKind::Anthropic,
            _ => return Err(format!("final_reactor: invalid provider '{}'", provider_str).into()),
        };
        let api_key_raw = final_reactor_cfg
            .get("api_key")
            .and_then(|v| v.as_str())
            .ok_or("final_reactor: missing 'api_key' in config")?;
        let api_key = resolve_env_var(api_key_raw)?;
        let model = final_reactor_cfg
            .get("model")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        // System message — respect user-provided system_message if any, otherwise
        // use the generic workflow default. In both cases append the grounding
        // rules so the final reactor can only use facts present in the context.
        let user_system = final_reactor_cfg
            .get("system_message")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .unwrap_or(LLM_DEFAULT_SYSTEM);
        let system_msg = format!("{}\n\n{}", user_system, ORCHESTRATOR_GROUNDING);

        // Build user message: original prompt + phase summaries.
        // Upstream input nodes may label the field `prompt` or `user_message`
        // (the latter is the default variableName in the UI builder); accept both.
        let mut context_parts = Vec::new();
        let user_prompt = inputs
            .get("prompt")
            .and_then(|v| v.as_str())
            .or_else(|| inputs.get("user_message").and_then(|v| v.as_str()))
            .unwrap_or("");
        if !user_prompt.is_empty() {
            context_parts.push(format!("# User's Original Question\n\n{}", user_prompt));
        }
        for summary in &phase_summaries {
            context_parts.push(format!(
                "# Phase {} Results\n\n{}",
                summary.phase, summary.summary
            ));
        }
        let user_message = context_parts.join("\n\n---\n\n");

        // Configure LLM
        let provider = crate::llm::domain::LlmProvider::new(provider_kind.clone(), api_key, model)?;
        let mut llm_config = crate::llm::domain::LlmConfig::new(provider);
        if let Some(temp) = final_reactor_cfg
            .get("temperature")
            .and_then(|v| v.as_f64())
        {
            llm_config = llm_config.with_temperature(temp as f32)?;
        }
        if let Some(budget) = final_reactor_cfg
            .get("thinking_budget")
            .and_then(|v| v.as_u64())
        {
            llm_config = llm_config.with_thinking_budget(budget as u32);
        }

        let llm_repo = crate::llm::infrastructure::LlmProviderFactory::create(provider_kind);
        let conversation_repo = Arc::new(
            crate::llm::infrastructure::persistence::in_memory_conversation_repository::InMemoryConversationRepository::new(),
        );
        let agent_service = crate::llm::application::AgentService::new(llm_repo, conversation_repo);
        let tid_str = uuid::Uuid::new_v4().to_string();
        let tid = crate::llm::domain::ConversationKey {
            session_id: crate::llm::domain::SessionId(tid_str.clone()),
            agent_session_id: None,
            node_id: crate::llm::domain::NodeIdPath(tid_str),
        };

        let messages = vec![
            crate::llm::domain::LlmMessage::system(system_msg)?,
            crate::llm::domain::LlmMessage::user(user_message)?,
        ];

        // Streaming callback — always stream, emits llm_token (user-facing response).
        // The final_reactor is the user-facing response of the orchestrator, so its
        // tokens are forwarded to the parent observer as plain `LlmToken` (NOT
        // `ThinkingToken`). The run_use_case stamps them with the orchestrator's own
        // `node_id`, so the SseMapper renders them as top-level `text-delta`
        // (or `subgraph-text-delta` automatically when the orchestrator is wrapped
        // inside a `subgraph` node).
        let final_obs = observer.clone();
        let on_token: Option<Box<dyn Fn(crate::llm::domain::LlmStreamPart) + Send + Sync>> =
            if let Some(obs) = final_obs {
                Some(Box::new(
                    move |part: crate::llm::domain::LlmStreamPart| match part {
                        crate::llm::domain::LlmStreamPart::Content(token) => {
                            obs.on_event(NodeEvent::LlmToken { token })
                        }
                        crate::llm::domain::LlmStreamPart::Usage(usage) => {
                            obs.on_event(NodeEvent::LlmUsage {
                                prompt_tokens: usage.prompt_tokens,
                                completion_tokens: usage.completion_tokens,
                                thinking_tokens: usage.thinking_tokens,
                                cache_read_tokens: usage.cache_read_tokens,
                                cache_write_tokens: usage.cache_write_tokens,
                            })
                        }
                        crate::llm::domain::LlmStreamPart::LlmMessageStart => {
                            obs.on_event(NodeEvent::LlmMessageStart)
                        }
                        crate::llm::domain::LlmStreamPart::LlmMessageFinish(usage) => {
                            obs.on_event(NodeEvent::LlmMessageFinish(usage))
                        }
                        _ => {}
                    },
                ))
            } else {
                None
            };

        // Empty tool executor (no tools for final response)
        struct FinalEmptyToolExecutor;
        #[async_trait::async_trait]
        impl crate::llm::domain::ToolExecutor for FinalEmptyToolExecutor {
            async fn execute(
                &self,
                _tc: &crate::llm::domain::ToolCall,
            ) -> Result<crate::llm::domain::ToolResult, crate::llm::domain::LlmError> {
                Err(crate::llm::domain::LlmError::ToolExecutionFailed {
                    message: "No tools".into(),
                })
            }
            async fn available_tools(&self) -> Vec<crate::llm::domain::ToolDefinition> {
                vec![]
            }
        }

        // No `subgraph-node-start` framing for the final_reactor: its tokens flow
        // as the orchestrator's own user-facing text stream (text-delta /
        // subgraph-text-delta), not as a separate internal sub-node.

        let params = crate::llm::application::AgentRunParams {
            session_id: &tid,
            prompt: None, // messages already pre-populated
            messages: Some(messages),
            config: llm_config,
            tools: vec![],
            tool_executor: &FinalEmptyToolExecutor,
            max_iterations: Some(1),
            on_token,
            tools_provider: None,
        };

        let response = agent_service.run(params).await?;
        let final_text = response.content().to_string();

        colmena_log!(
            "✅ [OrchestratorNode] Final response generated ({} chars).",
            final_text.len()
        );

        let phase_summaries_json: Vec<Value> = phase_summaries
            .iter()
            .map(|s| {
                json!({
                    "phase": s.phase,
                    "summary": s.summary
                })
            })
            .collect();

        Ok(OrchestratorSuspend::Done(json!({
            "all_tasks": all_tasks_json,
            "final_response": final_text,
            "extra_info": {
                "__colmena_loop_status": "FINISHED",
                "phase_summaries": phase_summaries_json
            }
        })))
    }
}

#[async_trait::async_trait]
impl ExecutableNode for OrchestratorNode {
    async fn execute(
        &self,
        inputs: &NodeInputs,
        config: &Value,
        _state: &mut Value,
        _observer: Option<Arc<dyn crate::dag_engine::domain::observer::ExecutionObserver>>,
    ) -> Result<Value, Box<dyn StdError + Send + Sync>> {
        let session_id = _state
            .get("session_id")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown_run")
            .to_string();
        let verbose = config
            .get("verbose")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        if verbose {
            colmena_log!("\n═══════════════════════════════════════");
            colmena_log!("🚦 [OrchestratorNode] VERBOSE — session_id: {}", session_id);
            colmena_log!("───────────────────────────────────────");
            colmena_log!("Inputs: {:?}", inputs);
            colmena_log!("═══════════════════════════════════════\n");
        }

        // ── Detect resume context ─────────────────────────────────────────────
        // When run_use_case resumes from a suspend, it injects __colmena_resume_answer
        // into every node's inputs. We also read __orchestrator_suspend from _state
        // (global_shared_state) to know at which point the orchestrator suspended.
        let resume_answer: Option<String> = inputs
            .get("__colmena_resume_answer")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let mut suspend_meta = read_suspend_meta(_state);

        // Capture this immutably BEFORE any clear_suspend_meta calls below — we
        // need it later (Task 4 Step 2) when building task_inputs to decide
        // whether to preserve __colmena_resume_answer for cascading into the
        // agent's subgraph_node.
        let resuming_agent_suspend: bool = matches!(
            (&resume_answer, &suspend_meta),
            (Some(_), Some((sa, _, _, _))) if sa == "agent"
        );

        // If resuming from a suspend, handle based on where we suspended.
        if let (Some(ref ans), Some((ref sa, _, _, ref questions))) =
            (&resume_answer, &suspend_meta)
        {
            if sa == KEY_CRITIC {
                colmena_log!("▶️  [OrchestratorNode] Resuming from critic suspend. Preparing Q&A context for agent re-execution.");
                // Prepare Q&A context but DO NOT clear suspend_meta — the resuming_critic guard in the task loop will handle the cleanup
                let qa_str = questions_to_context_string(questions, ans);
                if let Some(obj) = _state.as_object_mut() {
                    obj.insert(
                        "__orchestrator_qa_context".to_string(),
                        json!({ "qa_context": qa_str }),
                    );
                }
                // NOTE: Suspend meta cleanup happens in resuming_critic guard, not here
            } else if sa == KEY_PLANNER {
                colmena_log!("▶️  [OrchestratorNode] Resuming from planner suspend. Injecting Q&A as planner context and phase 0 summary.");
                let new_qa = questions_to_context_string(questions, ans);
                // Accumulate with any previous planner Q&A rounds so the planner sees the full history
                let accumulated_qa = {
                    let prev = _state
                        .get("__orchestrator_planner_qa")
                        .and_then(|v| v.get("qa_context"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    if prev.is_empty() {
                        new_qa.clone()
                    } else {
                        format!("{}\n\n---\n\n{}", prev, new_qa)
                    }
                };
                // Save as phase 0 summary so ALL agents in the run can see it
                if let Some(repo) = &self.task_memory_repo {
                    if let Err(e) = repo
                        .save_phase_summary(
                            &session_id,
                            0,
                            &format!("[PLANNER Q&A] {}", accumulated_qa),
                        )
                        .await
                    {
                        colmena_log!("⚠️  [OrchestratorNode] Failed to save planner Q&A as phase 0 summary: {}", e);
                    }
                }
                // Stash accumulated Q&A for planner re-injection into system_message
                if let Some(obj) = _state.as_object_mut() {
                    obj.insert(
                        "__orchestrator_planner_qa".to_string(),
                        json!({ "qa_context": accumulated_qa }),
                    );
                }
                clear_suspend_meta(_state);
                // NO return — fall-through to re-execute the planner (existing_tasks is still empty)
            } else if sa == "agent" {
                colmena_log!(
                    "▶️  [OrchestratorNode] Resuming from agent suspend. Re-dispatching pending tasks; \
                     the dispatch loop will preserve __colmena_resume_answer in task_inputs \
                     (resuming_agent_suspend flag set above)."
                );
                clear_suspend_meta(_state);
                // No injection here. The answer rides on __colmena_resume_answer
                // through task_inputs (Task 4) into subgraph_node.execute, which
                // detects it and enters its resume path → find_suspended_child →
                // cascade down to the actual SuspendNode at the leaf.
                let _ = ans; // intentionally unused in this arm
            }
        }

        if let Some(repo) = &self.task_memory_repo {
            // ── 1. Auto-Planificación ─────────────────────────────────────────
            // Si hay config de 'planner' y la DB está vacía, ejecutamos el planner interno
            if let Some(planner_cfg) = config.get(KEY_PLANNER) {
                let existing_tasks = repo.get_tasks_for_run(&session_id).await?;
                if existing_tasks.is_empty() {
                    colmena_log!("🗂️ [OrchestratorNode] Internal Planning started...");

                    // Derivar agentes automáticamente de config["agents"] y recolectar descripciones
                    let mut agent_names = Vec::new();
                    let mut agent_descriptions = String::new();
                    if let Some(agents_obj) = config.get("agents").and_then(|a| a.as_object()) {
                        for (name, props) in agents_obj {
                            agent_names.push(Value::String(name.clone()));
                            let desc = props
                                .get("description")
                                .and_then(|v| v.as_str())
                                .unwrap_or("No description provided");
                            agent_descriptions.push_str(&format!("- {}: {}\n", name, desc));
                        }
                    }

                    let mut internal_planner_cfg = planner_cfg.clone();
                    if let Some(obj) = internal_planner_cfg.as_object_mut() {
                        obj.insert("agents".to_string(), Value::Array(agent_names));
                        // Asegurar que el planner use la misma API KEY que el resto si no tiene una propia
                        if obj.get("api_key").is_none() {
                            if let Some(ak) = config.get("api_key") {
                                obj.insert("api_key".to_string(), ak.clone());
                            }
                        }
                        // Añadir instrucción para que cada tarea incluya un campo "context"
                        // y alimentar las descripciones
                        let context_instruction = format!("\n\nAVAILABLE AGENTS (Use these assignments and consider their expertise):\n{}\n\n\
                            For each task in your plan, you MUST include a \"context\" field: \
                            a 1-2 sentence explanation of why this task exists and what the user's intent is \
                            (e.g. \"The user wants to know what clothing to pack for a ski trip in cold conditions.\"). \
                            This context will be shown to the agent alongside the task instruction.", agent_descriptions);
                        let mut existing_sm = obj
                            .get("system_message")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        existing_sm.push_str(&context_instruction);

                        // Inject Q&A context from a previous planner suspend if present
                        if let Some(qa) = _state.get("__orchestrator_planner_qa") {
                            if let Some(ctx) = qa.get("qa_context").and_then(|v| v.as_str()) {
                                existing_sm.push_str(&format!(
                                    "\n\nUSER CLARIFICATION BEFORE PLANNING:\n{}",
                                    ctx
                                ));
                            }
                        }

                        obj.insert("system_message".to_string(), Value::String(existing_sm));
                    }

                    // Clean up planner Q&A stash after injecting into config
                    if let Some(obj) = _state.as_object_mut() {
                        obj.remove("__orchestrator_planner_qa");
                    }

                    let registry = self.registry.upgrade().ok_or("Registry already dropped")?;
                    let planner_node = registry
                        .get_node(KEY_PLANNER)
                        .ok_or("Planner node not found")?;

                    // Ejecutamos el planner. El PlannerNode escribe el plan en 'state' y retorna { result: { items: [...] } }
                    // o { result: { questions: [...] } } si necesita clarificación.
                    emit_internal_node_start(
                        &_observer,
                        KEY_PLANNER,
                        NODE_TYPE_PLANNER,
                        json!({
                            "prompt": inputs.get("prompt").cloned().unwrap_or(Value::Null),
                            "model": internal_planner_cfg.get("model").cloned().unwrap_or(Value::Null),
                            "provider": internal_planner_cfg.get("provider").cloned().unwrap_or(Value::Null),
                        }),
                    );
                    let planner_obs =
                        direct_thinking_observer(KEY_PLANNER, NODE_TYPE_PLANNER, &_observer);
                    let planner_result = planner_node
                        .execute(inputs, &internal_planner_cfg, _state, planner_obs)
                        .await?;
                    emit_internal_node_finish(&_observer, KEY_PLANNER, planner_result.clone());

                    // Detectar si el planner quiere suspender para pedir más info
                    let planner_questions_opt: Option<Vec<SuspendQuestion>> = planner_result
                        .get("result")
                        .and_then(|r| r.get("questions"))
                        .and_then(|q| serde_json::from_value(q.clone()).ok())
                        .filter(|v: &Vec<SuspendQuestion>| !v.is_empty());

                    if let Some(planner_questions) = planner_questions_opt {
                        let planner_allow_suspend_default = json!({});
                        let planner_cfg_raw = config
                            .get(KEY_PLANNER)
                            .unwrap_or(&planner_allow_suspend_default);
                        if allow_suspend_for(planner_cfg_raw) {
                            colmena_log!("⏸️  [OrchestratorNode] Planner requested clarification before generating tasks.");
                            return Ok(make_suspend_response(
                                _state,
                                KEY_PLANNER,
                                0,
                                None,
                                planner_questions,
                            ));
                        } else {
                            debug_print_questions("planner", &planner_questions);
                        }
                    }

                    // Sembrar la DB con el plan obtenido
                    if let Some(items) = planner_result
                        .get("result")
                        .and_then(|r| r.get("items"))
                        .and_then(|i| i.as_array())
                    {
                        for task_val in items {
                            if let Some(task_obj) = task_val.as_object() {
                                let task_name = task_obj
                                    .get("task")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("Unknown")
                                    .to_string();
                                let assigned = task_obj
                                    .get("assigned_to")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("Unknown")
                                    .to_string();
                                let phase =
                                    task_obj.get("phase").and_then(|v| v.as_i64()).unwrap_or(1)
                                        as i32;
                                let parallel = task_obj
                                    .get("parallel")
                                    .and_then(|v| v.as_bool())
                                    .unwrap_or(false);

                                let context = task_obj
                                    .get("context")
                                    .and_then(|v| v.as_str())
                                    .map(|s| s.to_string());

                                let new_task = DagTask {
                                    id: uuid::Uuid::new_v4().to_string(),
                                    session_id: session_id.clone(),
                                    task_name,
                                    assigned_to: assigned,
                                    completed: false,
                                    result: None,
                                    phase,
                                    parallel,
                                    context,
                                    is_bridge: false,
                                };
                                repo.add_task(&new_task).await?;
                            }
                        }
                        // Print plan in human-readable format for debugging
                        colmena_log!(
                            "💾 [OrchestratorNode] DB seeded with {} tasks from internal planner.",
                            items.len()
                        );
                        colmena_log!("📋 [OrchestratorNode] PLAN:");
                        colmena_log!("{}", "─".repeat(60));
                        let mut phase_map: std::collections::BTreeMap<i64, Vec<String>> =
                            std::collections::BTreeMap::new();
                        for task_val in items {
                            let phase_num =
                                task_val.get("phase").and_then(|v| v.as_i64()).unwrap_or(1);
                            let task_name =
                                task_val.get("task").and_then(|v| v.as_str()).unwrap_or("?");
                            let agent = task_val
                                .get("assigned_to")
                                .and_then(|v| v.as_str())
                                .unwrap_or("?");
                            let parallel = task_val
                                .get("parallel")
                                .and_then(|v| v.as_bool())
                                .unwrap_or(false);
                            let ctx = task_val
                                .get("context")
                                .and_then(|v| v.as_str())
                                .unwrap_or("");
                            let parallel_tag = if parallel { " [parallel]" } else { "" };
                            let entry =
                                format!("  [{agent}]{parallel_tag}: {task_name}\n    → {ctx}");
                            phase_map.entry(phase_num).or_default().push(entry);
                        }
                        for (ph, tasks) in &phase_map {
                            colmena_log!("Phase {}:", ph);
                            for t in tasks {
                                println!("{}", t);
                            }
                        }
                        colmena_log!("{}", "─".repeat(60));
                    }
                }
            } else {
                // Si no hay planner interno, intentamos sembrar desde los inputs tradicionales (retro-compatibilidad)
                let existing_tasks = repo.get_tasks_for_run(&session_id).await?;
                if existing_tasks.is_empty() {
                    if let Some(plan_val) = _state
                        .get("plan")
                        .or_else(|| inputs.get("plan"))
                        .or_else(|| config.get("plan"))
                    {
                        // ... (lógica anterior de siembra manual) ...
                        // (Mejor mantenemos la lógica de siembra manual por si acaso)
                        seed_db_manually(repo, &session_id, plan_val).await?;
                    }
                }
            }

            // ── 2. Despacho, Crítica y Ejecución Interna (loop completo) ─────────
            // El orquestador ejecuta TODAS las fases en una sola llamada a execute().
            // Esto evita depender de un self-loop en el grafo DAG para continuar entre fases.
            let max_phases = config
                .get("max_phases")
                .and_then(|v| v.as_i64())
                .unwrap_or(10) as i32;

            loop {
                let current_phase_opt = repo.get_current_phase(&session_id).await?;

                match current_phase_opt {
                    None => {
                        // Todas las fases terminadas
                        match self
                            .finalize_execution(
                                repo,
                                &session_id,
                                config,
                                _state,
                                _observer,
                                inputs,
                            )
                            .await?
                        {
                            OrchestratorSuspend::Suspended(s) => return Ok(s),
                            OrchestratorSuspend::Done(v) => return Ok(v),
                        }
                    }

                    Some(current_phase) => {
                        // Safety net: si se supera el límite de fases, forzar finalización.
                        // Evita loops infinitos cuando el reactor propone tareas indefinidamente.
                        if current_phase > max_phases {
                            colmena_log!(
                                "⚠️  [OrchestratorNode] max_phases ({}) reached at phase {}. Forcing finalization.",
                                max_phases, current_phase
                            );
                            match self
                                .finalize_execution(
                                    repo,
                                    &session_id,
                                    config,
                                    _state,
                                    _observer,
                                    inputs,
                                )
                                .await?
                            {
                                OrchestratorSuspend::Suspended(s) => return Ok(s),
                                OrchestratorSuspend::Done(v) => return Ok(v),
                            }
                        }

                        let incomplete_in_phase = repo
                            .get_uncompleted_tasks_for_phase(&session_id, current_phase)
                            .await?;

                        if incomplete_in_phase.is_empty() {
                            // Guard: si venimos de un suspend de phase_reactor para ESTA fase,
                            // inyectar Q&A y re-ejecutar el reactor con ese contexto.
                            let resuming_phase_reactor = resume_answer.is_some()
                                && matches!(&suspend_meta, Some((sa, sp, _, _)) if sa == KEY_PHASE_REACTOR && *sp == current_phase);

                            if resuming_phase_reactor {
                                let ans = resume_answer.as_deref().unwrap();
                                let questions = suspend_meta.as_ref().unwrap().3.clone();
                                let ctx_str = questions_to_context_string(&questions, ans);
                                // Stash Q&A so handle_phase_completion injects it into the reactor's system_message
                                if let Some(obj) = _state.as_object_mut() {
                                    obj.insert(
                                        "__orchestrator_phase_reactor_qa".to_string(),
                                        json!({ "qa_context": ctx_str }),
                                    );
                                }
                                clear_suspend_meta(_state);
                                colmena_log!(
                                    "▶️  [OrchestratorNode] Resumed phase {} reactor — re-executing reactor with user clarification.",
                                    current_phase
                                );
                                // Fall-through to handle_phase_completion — reactor re-runs with Q&A injected
                            }

                            // ── Bridge tasks check ───────────────────────────────────────
                            // If the reactor already ran for this phase (flag set), all remaining
                            // incomplete tasks were bridge tasks that have now completed.
                            // Generate the bridge summary and advance without re-running the reactor.
                            let reactor_done_key = format!("__orch_reactor_done_{}", current_phase);
                            let reactor_already_ran = _state.get(&reactor_done_key).is_some();

                            if reactor_already_ran {
                                // Collect completed bridge task results for this phase
                                let all_phase_tasks = repo.get_tasks_for_run(&session_id).await?;
                                let bridge_done: Vec<_> = all_phase_tasks
                                    .iter()
                                    .filter(|t| {
                                        t.phase == current_phase && t.is_bridge && t.completed
                                    })
                                    .collect();

                                if !bridge_done.is_empty() {
                                    let mut parts = Vec::new();
                                    for t in &bridge_done {
                                        if let Some(res) = &t.result {
                                            let text = res
                                                .get("result")
                                                .and_then(|v| v.as_str())
                                                .unwrap_or("");
                                            if !text.is_empty() {
                                                parts.push(format!(
                                                    "[Bridge — {}]: {}",
                                                    t.assigned_to, text
                                                ));
                                            }
                                        }
                                    }
                                    if !parts.is_empty() {
                                        let bridge_summary = format!(
                                            "[BRIDGE RESULTS — phase {}]\n{}",
                                            current_phase,
                                            parts.join("\n\n")
                                        );
                                        repo.save_phase_summary(
                                            &session_id,
                                            current_phase,
                                            &bridge_summary,
                                        )
                                        .await?;
                                        colmena_log!(
                                            "💾 [OrchestratorNode] Bridge summary saved for phase {} ({} task(s)).",
                                            current_phase, bridge_done.len()
                                        );
                                    }
                                }

                                // Clear the reactor-done flag and advance to the next phase
                                if let Some(obj) = _state.as_object_mut() {
                                    obj.remove(&reactor_done_key);
                                }
                                colmena_log!(
                                    "🌉 [OrchestratorNode] Phase {} bridge complete. Advancing to next phase.",
                                    current_phase
                                );
                                continue;
                            }

                            // ── Normal path: run phase reactor ───────────────────────────
                            match self
                                .handle_phase_completion(
                                    repo,
                                    &session_id,
                                    current_phase,
                                    config,
                                    _state,
                                    _observer.clone(),
                                )
                                .await?
                            {
                                OrchestratorSuspend::Suspended(s) => return Ok(s),
                                OrchestratorSuspend::Done(_) => {}
                            }

                            // If the reactor seeded bridge tasks in this phase, set the done flag
                            // so the next iteration (after bridge tasks execute) skips the reactor.
                            let has_bridge = repo
                                .get_tasks_for_run(&session_id)
                                .await?
                                .into_iter()
                                .any(|t| t.phase == current_phase && t.is_bridge && !t.completed);

                            if has_bridge {
                                if let Some(obj) = _state.as_object_mut() {
                                    obj.insert(reactor_done_key, json!(true));
                                }
                            }

                            // Continuar el loop: bridge tasks o siguiente fase
                            continue;
                        }

                        // Determinar estrategia de despacho
                        let any_parallel = incomplete_in_phase.iter().any(|t| t.parallel);
                        let tasks_to_run: Vec<_> = if any_parallel {
                            incomplete_in_phase
                        } else {
                            incomplete_in_phase.into_iter().take(1).collect()
                        };

                        let registry = self.registry.upgrade().ok_or("Registry already dropped")?;

                        for task in tasks_to_run {
                            // Guard: si venimos de un suspend de critic_max_retries para ESTA tarea.
                            // Debe comprobarse ANTES del guard de "critic".
                            let resuming_critic_max_retries = resume_answer.is_some()
                                && matches!(&suspend_meta, Some((sa, _, Some(tid), _)) if sa == "critic_max_retries" && tid == &task.id);

                            if resuming_critic_max_retries {
                                let ans_raw = resume_answer.as_deref().unwrap();
                                let ans = ans_raw.to_lowercase();
                                let questions = suspend_meta.as_ref().unwrap().3.clone();
                                clear_suspend_meta(_state);
                                // Consume local variable so subsequent loop iterations don't re-trigger
                                suspend_meta = None;

                                let accept = ans.contains("accept")
                                    || ans.contains("acepta")
                                    || ans.contains("aceptar")
                                    || ans.contains("aprueba")
                                    || ans.contains("aprobar")
                                    || ans.contains("ok")
                                    || ans.contains("está bien")
                                    || ans.contains("esta bien");
                                let skip = ans.contains("skip")
                                    || ans.contains("continuar")
                                    || ans.contains("continue");
                                let cancel = ans.contains("cancel") || ans.contains("cancelar");
                                // Default: if not clearly accept/skip/cancel → treat as retry with instructions
                                let retry = !accept && !skip && !cancel;

                                if accept {
                                    // User accepts the result as-is — use the stashed agent result
                                    let stash_key = format!("__orch_pending_{}", task.id);
                                    let retry_key = format!("__orch_retries_{}", task.id);
                                    let feedback_key =
                                        format!("__orch_critic_feedback_{}", task.id);
                                    let stashed = _state.get(&stash_key).cloned().unwrap_or(
                                        json!({"result": "[Accepted by user]", "extra_info": {}}),
                                    );
                                    repo.update_task_result(&task.id, stashed).await?;
                                    if let Some(obj) = _state.as_object_mut() {
                                        obj.remove(&stash_key);
                                        obj.remove(&retry_key);
                                        obj.remove(&feedback_key);
                                    }
                                    colmena_log!(
                                        "✅ [OrchestratorNode] Task '{}' accepted by user. Using existing result.",
                                        task.task_name
                                    );
                                    continue;
                                }

                                if skip || cancel {
                                    let note = if cancel {
                                        "[CANCELLED by user]"
                                    } else {
                                        "[SKIPPED by user after max retries]"
                                    };
                                    repo.update_task_result(
                                        &task.id,
                                        json!({ "result": note, "extra_info": {} }),
                                    )
                                    .await?;
                                    let stash_key = format!("__orch_pending_{}", task.id);
                                    let retry_key = format!("__orch_retries_{}", task.id);
                                    let feedback_key =
                                        format!("__orch_critic_feedback_{}", task.id);
                                    if let Some(obj) = _state.as_object_mut() {
                                        obj.remove(&stash_key);
                                        obj.remove(&retry_key);
                                        obj.remove(&feedback_key);
                                    }
                                    colmena_log!(
                                        "⏭️  [OrchestratorNode] Task '{}' {}. Moving on.",
                                        task.task_name,
                                        if cancel { "cancelled" } else { "skipped" }
                                    );
                                    continue;
                                }

                                if retry {
                                    let retry_key = format!("__orch_retries_{}", task.id);
                                    if let Some(obj) = _state.as_object_mut() {
                                        obj.remove(&retry_key);
                                        let qa_str =
                                            questions_to_context_string(&questions, ans_raw);
                                        obj.insert(
                                            "__orchestrator_qa_context".to_string(),
                                            json!({ "qa_context": qa_str }),
                                        );
                                    }
                                    colmena_log!(
                                        "🔄 [OrchestratorNode] Task '{}' retrying with user clarification.",
                                        task.task_name
                                    );
                                    // NO continue — fall-through para re-ejecutar el agente
                                }
                            }

                            // Guard: si venimos de un suspend del critic para ESTA tarea,
                            // descartar el stash y re-ejecutar el agente con el contexto del usuario.
                            let resuming_critic = resume_answer.is_some()
                                && !resuming_critic_max_retries
                                && matches!(&suspend_meta, Some((sa, _, Some(tid), _)) if sa == KEY_CRITIC && tid == &task.id);

                            if resuming_critic {
                                let ans = resume_answer.as_deref().unwrap();
                                let questions = suspend_meta.as_ref().unwrap().3.clone();
                                // Descartar stash — el agente se re-ejecutará con USER CLARIFICATION
                                let stash_key = format!("__orch_pending_{}", task.id);
                                if let Some(obj) = _state.as_object_mut() {
                                    obj.remove(&stash_key);
                                    let qa_str = questions_to_context_string(&questions, ans);
                                    obj.insert(
                                        "__orchestrator_qa_context".to_string(),
                                        json!({ "qa_context": qa_str }),
                                    );
                                }
                                clear_suspend_meta(_state);
                                // Consume the local variable so subsequent loop iterations
                                // don't re-trigger this guard — without this, every retry
                                // cycle would wrongly re-inject the same Q&A context.
                                suspend_meta = None;
                                colmena_log!(
                                    "▶️  [OrchestratorNode] Resumed critic for task '{}'. Re-running agent with USER CLARIFICATION.",
                                    task.task_name
                                );
                                // NO continue — fall-through para re-ejecutar el agente con contexto
                            }

                            colmena_log!(
                                "🚀 [OrchestratorNode] Internal Execution: Task '{}' -> Agent '{}'",
                                task.task_name,
                                task.assigned_to
                            );

                            // Build the single prompt text that the agent receives.
                            // task + context are combined here — the agent only needs `prompt`.
                            let mut task_inputs = inputs.clone();
                            // For legacy suspend sources (planner, critic, phase_reactor), the orchestrator
                            // already consumed the resume_answer at its own level (e.g. injected into the
                            // planner's system_message). Stripping the key here prevents fresh agents
                            // from accidentally entering their resume path.
                            //
                            // For agent-suspend resumes (spec §7), however, the answer is FOR the
                            // SuspendNode at the leaf — it must flow through the subgraph_node so that
                            // node enters its resume path and cascades down to find_suspended_child.
                            if !resuming_agent_suspend {
                                task_inputs.remove("__colmena_resume_answer");
                            }
                            // Unique __node_id per agent so SubGraphNode generates distinct
                            // child_session_ids: "{session_id}_sub_{agent_name}"
                            task_inputs.insert(
                                "__node_id".to_string(),
                                Value::String(task.assigned_to.clone()),
                            );
                            let phase_summaries_for_ctx = repo
                                .get_phase_summaries(&session_id)
                                .await
                                .unwrap_or_default();
                            let qa_ctx: Option<String> = _state
                                .get("__orchestrator_qa_context")
                                .and_then(|v| v.get("qa_context"))
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string());
                            let critic_feedback_ctx: Option<String> = {
                                let feedback_key = format!("__orch_critic_feedback_{}", task.id);
                                _state
                                    .get(&feedback_key)
                                    .and_then(|v| v.as_str())
                                    .map(|s| s.to_string())
                            };
                            let prompt = build_enriched_prompt(
                                &phase_summaries_for_ctx,
                                &task.task_name,
                                task.context.as_deref(),
                                qa_ctx.as_deref(),
                                critic_feedback_ctx.as_deref(),
                            );

                            colmena_log!(
                                "📨 [OrchestratorNode] PROMPT → agent '{}' | task '{}'\n{}\n{}",
                                task.assigned_to,
                                task.task_name,
                                "─".repeat(60),
                                prompt
                            );

                            task_inputs.insert("prompt".to_string(), Value::String(prompt));

                            // Obtener config del agente del bloque 'agents'
                            let agent_node_cfg = config
                                .get("agents")
                                .and_then(|a| a.get(&task.assigned_to))
                                .ok_or_else(|| {
                                    format!(
                                        "Configuration for agent '{}' not found in orchestrator config",
                                        task.assigned_to
                                    )
                                })?;

                            // Agents must be subgraphs (child_graph_path or child_graph_inline).
                            // Direct LLM agent configs are no longer supported — use child_graph_inline
                            // with a minimal `in → llm_call → out` graph instead.
                            if agent_node_cfg.get("child_graph_path").is_none()
                                && agent_node_cfg.get("child_graph_inline").is_none()
                            {
                                return Err(format!(
                                    "Agent '{}' must be a subgraph: add 'child_graph_path' or \
                                     'child_graph_inline' to its config. Direct LLM agent configs \
                                     are no longer supported.",
                                    task.assigned_to
                                )
                                .into());
                            }

                            colmena_log!(
                                "🗺️  [OrchestratorNode] Agent '{}' delegating to SubGraphNode.",
                                task.assigned_to
                            );

                            let mut subgraph_cfg = agent_node_cfg.clone();
                            if let Some(obj) = subgraph_cfg.as_object_mut() {
                                obj.insert(
                                    "__agent_name".to_string(),
                                    Value::String(task.assigned_to.clone()),
                                );
                            }

                            let subgraph_node = registry
                                .get_node("subgraph")
                                .ok_or("subgraph node not found")?;

                            let agent_result = subgraph_node
                                .execute(&task_inputs, &subgraph_cfg, _state, _observer.clone())
                                .await?;

                            colmena_log!(
                                "📬 [OrchestratorNode] RAW RESULT ← agent '{}' | task '{}'\n{}\n{}\n{}",
                                task.assigned_to,
                                task.task_name,
                                "─".repeat(60),
                                serde_json::to_string_pretty(&agent_result).unwrap_or_else(|_| format!("{:?}", agent_result)),
                                "─".repeat(60)
                            );

                            // ── Agent suspend propagation (spec §5) ──
                            // If the agent's subgraph internally suspended, do NOT run the critic, do NOT
                            // mark the task completed, do NOT continue the phase. Just propagate up.
                            //
                            // Note on allow_suspend: agents already have a SUSPENDED dag_runs row at this
                            // point — we cannot ignore the suspend without leaving an orphan. We log and
                            // propagate regardless of the flag.
                            if agent_result
                                .get("__colmena_status")
                                .and_then(|v| v.as_str())
                                == Some("SUSPENDED")
                            {
                                if !allow_suspend_for(&subgraph_cfg) {
                                    colmena_log!(
                                        "⚠️  [OrchestratorNode] Agent '{}' has allow_suspend=false, but its \
                                         subgraph already suspended. Flag has no safe effect for agents — \
                                         propagating suspend.",
                                        task.assigned_to
                                    );
                                }
                                colmena_log!(
                                    "⏸️  [OrchestratorNode] Agent '{}' suspended (task '{}'). Propagating up.",
                                    task.assigned_to, task.task_name
                                );
                                return Ok(propagate_agent_suspend(
                                    &agent_result,
                                    &task,
                                    current_phase,
                                    _state,
                                ));
                            }

                            // ── Crítica ──
                            let stash_key = format!("__orch_pending_{}", task.id);
                            let mut is_ok = true;
                            if let Some(critic_cfg) = config.get(KEY_CRITIC) {
                                let max_retries: u32 = critic_cfg
                                    .get("max_retries")
                                    .and_then(|v| v.as_u64())
                                    .unwrap_or(3)
                                    as u32;

                                let retry_key = format!("__orch_retries_{}", task.id);
                                let current_retries: u32 =
                                    _state.get(&retry_key).and_then(|v| v.as_u64()).unwrap_or(0)
                                        as u32;

                                colmena_log!(
                                    "🔎 [OrchestratorNode] Internal Critique for task '{}' (attempt {}/{})...",
                                    task.task_name, current_retries + 1, max_retries
                                );
                                let critic_node = registry
                                    .get_node(KEY_CRITIC)
                                    .ok_or("critic node not found")?;

                                let mut critic_inputs = task_inputs.clone();
                                critic_inputs
                                    .insert("agent_result".to_string(), agent_result.clone());
                                critic_inputs.insert(
                                    format!("texts.{}", task.assigned_to),
                                    agent_result.clone(),
                                );

                                // Stash agent_result in _state before calling critic so it
                                // survives suspension and can be recovered on resume.
                                if let Some(obj) = _state.as_object_mut() {
                                    obj.insert(stash_key.clone(), agent_result.clone());
                                }

                                let critic_node_id = format!("critic_{}", task.assigned_to);
                                emit_internal_node_start(
                                    &_observer,
                                    &critic_node_id,
                                    NODE_TYPE_CRITIC,
                                    json!({
                                        "task": task.task_name,
                                        "model": critic_cfg.get("model").cloned().unwrap_or(Value::Null),
                                        "provider": critic_cfg.get("provider").cloned().unwrap_or(Value::Null),
                                    }),
                                );
                                let critic_obs = direct_thinking_observer(
                                    &critic_node_id,
                                    NODE_TYPE_CRITIC,
                                    &_observer,
                                );
                                let critic_res = critic_node
                                    .execute(&critic_inputs, critic_cfg, _state, critic_obs)
                                    .await?;
                                emit_internal_node_finish(
                                    &_observer,
                                    &critic_node_id,
                                    critic_res.clone(),
                                );

                                // Check if critic wants to suspend and ask the user
                                let critic_suspended = critic_res
                                    .get("extra_info")
                                    .and_then(|e| e.get("suspend"))
                                    .and_then(|v| v.as_bool())
                                    .unwrap_or(false);

                                if critic_suspended {
                                    let question_text = critic_res
                                        .get("extra_info")
                                        .and_then(|e| e.get("question"))
                                        .and_then(|v| v.as_str())
                                        .filter(|s| !s.is_empty())
                                        .unwrap_or("Please review and confirm the agent result.")
                                        .to_string();
                                    colmena_log!(
                                        "⏸️  [OrchestratorNode] Critic suspended for task '{}'. Question: {}",
                                        task.task_name, question_text
                                    );
                                    let critic_questions = vec![SuspendQuestion {
                                        id: "critic_review".to_string(),
                                        question: question_text,
                                        question_type: "open".to_string(),
                                        options: None,
                                    }];
                                    if allow_suspend_for(critic_cfg) {
                                        return Ok(make_suspend_response(
                                            _state,
                                            KEY_CRITIC,
                                            current_phase,
                                            Some(&task.id),
                                            critic_questions,
                                        ));
                                    } else {
                                        debug_print_questions("critic", &critic_questions);
                                    }
                                }

                                is_ok = critic_res
                                    .get("extra_info")
                                    .and_then(|e| e.get("task_ok"))
                                    .and_then(|v| v.as_bool())
                                    .unwrap_or(true);

                                // ── Capture Critic feedback for next agent attempt ────────
                                if !is_ok {
                                    let critic_feedback = critic_res
                                        .get("extra_info")
                                        .and_then(|e| e.get("feedback"))
                                        .and_then(|v| v.as_str())
                                        .filter(|s| !s.is_empty())
                                        .map(|s| s.to_string());
                                    if let Some(fb) = critic_feedback {
                                        let feedback_key =
                                            format!("__orch_critic_feedback_{}", task.id);
                                        if let Some(obj) = _state.as_object_mut() {
                                            obj.insert(feedback_key, Value::String(fb));
                                        }
                                    }
                                }

                                // ── Max Retries suspend ───────────────────────────────────
                                if !is_ok {
                                    let new_retries = current_retries + 1;
                                    if let Some(obj) = _state.as_object_mut() {
                                        obj.insert(retry_key.clone(), json!(new_retries));
                                    }

                                    if new_retries >= max_retries && allow_suspend_for(critic_cfg) {
                                        colmena_log!(
                                            "⏸️  [OrchestratorNode] Task '{}' exceeded max_retries ({}). Suspending for user decision.",
                                            task.task_name, max_retries
                                        );
                                        let max_retry_questions = vec![
                                            SuspendQuestion {
                                                id: "action".to_string(),
                                                question: format!(
                                                    "El agente para '{}' falló {} veces seguidas. ¿Qué hacemos?",
                                                    task.task_name, new_retries
                                                ),
                                                question_type: "choice".to_string(),
                                                options: Some(vec![
                                                    "accept — Aprobar el resultado actual tal como está".to_string(),
                                                    "skip — Continuar sin esta tarea".to_string(),
                                                    "retry — Reintentar con instrucciones adicionales (escríbelas a continuación)".to_string(),
                                                    "cancel — Cancelar esta tarea definitivamente".to_string(),
                                                ]),
                                            },
                                            SuspendQuestion {
                                                id: "instructions".to_string(),
                                                question: "Si elegiste 'retry', escribe aquí las instrucciones adicionales para el agente:".to_string(),
                                                question_type: "open".to_string(),
                                                options: None,
                                            },
                                        ];
                                        return Ok(make_suspend_response(
                                            _state,
                                            "critic_max_retries",
                                            current_phase,
                                            Some(&task.id),
                                            max_retry_questions,
                                        ));
                                    }
                                } else {
                                    // Task approved — clear retry counter
                                    if let Some(obj) = _state.as_object_mut() {
                                        obj.remove(&retry_key);
                                    }
                                }
                            }

                            if is_ok {
                                repo.update_task_result(&task.id, agent_result).await?;
                                // Clean up stash, Q&A context and critic feedback after task approved
                                if let Some(obj) = _state.as_object_mut() {
                                    obj.remove(&stash_key);
                                    obj.remove("__orchestrator_qa_context");
                                    obj.remove(&format!("__orch_critic_feedback_{}", task.id));
                                }
                                colmena_log!(
                                    "✅ [OrchestratorNode] Task '{}' completed successfully.",
                                    task.task_name
                                );
                            } else {
                                colmena_log!(
                                    "❌ [OrchestratorNode] Task '{}' rejected by critic.",
                                    task.task_name
                                );
                            }
                        }
                        // After running tasks, check if this phase is now fully complete
                        let remaining = repo
                            .get_uncompleted_tasks_for_phase(&session_id, current_phase)
                            .await?;
                        if remaining.is_empty() {
                            match self
                                .handle_phase_completion(
                                    repo,
                                    &session_id,
                                    current_phase,
                                    config,
                                    _state,
                                    _observer.clone(),
                                )
                                .await?
                            {
                                OrchestratorSuspend::Suspended(s) => return Ok(s),
                                OrchestratorSuspend::Done(_) => {}
                            }
                        }
                        // Continue the loop: next iteration picks up the new current phase
                    }
                }
            }
        } else {
            // Sin DB - Fallback a comportamiento estático (simplificado)
            Ok(json!({ "error": "Autonomous Orchestrator requires task_memory_repo" }))
        }
    }

    fn description(&self) -> Option<&str> {
        Some("Autonomous Orchestrator that manages the full Plan -> Execute -> Critique -> React lifecycle internally.")
    }

    fn default_output(&self) -> Option<&str> {
        Some("result")
    }

    fn schema(&self) -> Value {
        json!({
            "type": "orchestrator",
            "config": {
                "planner": { "provider": "string", "model": "string", "system_message": "string" },
                "agents": { "agent_id": { "provider": "string", "model": "string", "system_message": "string" } },
                "critic": { "provider": "string", "model": "string", "system_message": "string" },
                "phase_reactor": { "provider": "string", "model": "string", "system_message": "string" },
                "final_reactor": { "provider": "string", "model": "string", "system_message": "string" }
            },
            "outputs": {
                "__colmena_loop_status": "NEXT_TURN | FINISHED_PHASE | FINISHED",
                "current_phase": "integer",
                "phase_tasks": "array",
                "all_tasks": "array",
                "final_response": "string"
            }
        })
    }
}

async fn seed_db_manually(
    repo: &Arc<dyn DagTaskMemoryRepository>,
    session_id: &str,
    plan_val: &Value,
) -> Result<(), Box<dyn StdError + Send + Sync>> {
    let plan_array_opt = plan_val
        .as_array()
        .or_else(|| plan_val.get("items").and_then(|i| i.as_array()));

    if let Some(plan_array) = plan_array_opt {
        for task_val in plan_array {
            if let Some(task_obj) = task_val.as_object() {
                let task_name = task_obj
                    .get("task")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Unknown")
                    .to_string();
                let assigned = task_obj
                    .get("assigned_to")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Unknown")
                    .to_string();
                let phase = task_obj
                    .get("phase")
                    .and_then(|v| {
                        if let Some(num) = v.as_i64() {
                            Some(num)
                        } else if let Some(s) = v.as_str() {
                            s.parse::<i64>().ok()
                        } else {
                            None
                        }
                    })
                    .unwrap_or(1) as i32;

                let parallel = task_obj
                    .get("parallel")
                    .and_then(|v| {
                        if let Some(b) = v.as_bool() {
                            Some(b)
                        } else {
                            v.as_str().map(|s| s.eq_ignore_ascii_case("true"))
                        }
                    })
                    .unwrap_or(false);

                let context = task_obj
                    .get("context")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());

                let new_task = DagTask {
                    id: uuid::Uuid::new_v4().to_string(),
                    session_id: session_id.to_string(),
                    task_name,
                    assigned_to: assigned,
                    completed: false,
                    result: None,
                    phase,
                    parallel,
                    context,
                    is_bridge: false,
                };
                repo.add_task(&new_task).await?;
            }
        }
    }
    Ok(())
}

/// Resolves environment variable references in the form `${VAR_NAME}`.
fn resolve_env_var(value: &str) -> Result<String, String> {
    if value.starts_with("${") && value.ends_with('}') {
        let var_name = &value[2..value.len() - 1];
        std::env::var(var_name).map_err(|_| {
            format!(
                "OrchestratorNode: Environment variable '{}' not found",
                var_name
            )
        })
    } else {
        Ok(value.to_string())
    }
}

/// Builds an enriched prompt for an agent that includes:
/// 1. The user's original request (from inputs["prompt"], ignoring internal engine keys).
/// 2. The semantic context of this task (why it exists), as set by the Planner or Reactor.
/// 3. Summaries from previous phases for historical awareness.
/// 4. The specific task instruction this agent must execute.
///
/// This avoids injecting raw phase results into later agents, keeping context windows manageable.
fn build_enriched_prompt(
    phase_summaries: &[crate::dag_engine::domain::state::DagPhaseSummary],
    task_name: &str,
    task_context: Option<&str>,
    qa_context: Option<&str>,
    critic_feedback: Option<&str>,
) -> String {
    let mut parts = Vec::new();

    // Section 1: User clarification Q&A (highest priority — shown first so the agent
    // can act on the user's answer before processing anything else).
    if let Some(ctx) = qa_context {
        if !ctx.is_empty() {
            parts.push(format!("=== USER CLARIFICATION ===\n{}", ctx));
        }
    }

    // Section 2: task context — personalized intent synthesized by the Planner or Reactor.
    if let Some(ctx) = task_context {
        if !ctx.is_empty() {
            parts.push(format!("=== TASK CONTEXT ===\n{}", ctx));
        }
    }

    // Section 3: phase summaries (what happened in previous phases)
    if !phase_summaries.is_empty() {
        let summaries_text: Vec<String> = phase_summaries
            .iter()
            .map(|s| format!("Phase {}: {}", s.phase, s.summary))
            .collect();
        parts.push(format!(
            "=== WHAT HAS HAPPENED SO FAR ===\n{}",
            summaries_text.join("\n")
        ));
    }

    // Section 4: Critic feedback from previous failed attempt.
    // Only present on retries — tells the agent exactly what was wrong so it can correct it.
    if let Some(fb) = critic_feedback {
        if !fb.is_empty() {
            parts.push(format!("=== PREVIOUS ATTEMPT — WHY IT FAILED ===\n{}", fb));
        }
    }

    // Section 5: current task instruction
    parts.push(format!("=== YOUR CURRENT TASK ===\n{}", task_name));

    parts.join("\n\n")
}

/// Injects schema constraints into a reactor config's system_message so the LLM
/// knows the required output format and which agents are valid for new_tasks.
fn inject_reactor_schema_constraints(
    reactor_cfg: &mut Value,
    available_agents: &[(String, String)],
) {
    let agents_list = available_agents
        .iter()
        .map(|(n, d)| format!("- {}: {}", n, d))
        .collect::<Vec<_>>()
        .join("\n        ");

    let schema_instruction = PHASE_REACTOR_TEMPLATE.replace("{agents_list}", &agents_list);

    if let Some(obj) = reactor_cfg.as_object_mut() {
        let existing = obj
            .get("system_message")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        obj.insert(
            "system_message".to_string(),
            Value::String(format!(
                "{}{}\n\n{}",
                existing, schema_instruction, ORCHESTRATOR_GROUNDING
            )),
        );
    }
}

/// Parses the reactor's output.
///
/// ReactorNode returns `{ "result": <response_string>, "extra_info": { "add_tasks": [...], ... } }`.
/// The `result` field is the summary text; new_tasks come from `extra_info.add_tasks`.
///
/// `fallback_add_tasks` is the value already extracted from `extra_info.add_tasks` by the caller.
///
/// Returns (summary_string, Option<new_tasks_value>).
fn extract_reactor_output_with_fallback(
    reactor_result: &Value,
    fallback_add_tasks: Option<Value>,
) -> (String, Option<Value>) {
    // Case 1: result is a plain string (normal ReactorNode output)
    if let Some(raw_str) = reactor_result.as_str() {
        // The orchestrator injected a custom JSON schema, so the LLM response_text
        // might itself be JSON (if the model followed the injected schema literally).
        if let Ok(parsed) = serde_json::from_str::<Value>(raw_str) {
            if let Some(obj) = parsed.as_object() {
                let summary = obj
                    .get("summary")
                    .and_then(|v| v.as_str())
                    .unwrap_or(raw_str)
                    .to_string();
                // Prefer new_tasks from the parsed JSON; fall back to add_tasks from extra_info
                let new_tasks = obj.get("new_tasks").cloned().or(fallback_add_tasks);
                return (summary, new_tasks);
            }
        }
        // Plain text summary — use add_tasks from extra_info for replanning
        return (raw_str.to_string(), fallback_add_tasks);
    }

    // Case 2: result is a JSON object (unusual but handle gracefully)
    if let Some(obj) = reactor_result.as_object() {
        let summary = obj
            .get("summary")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let new_tasks = obj.get("new_tasks").cloned().or(fallback_add_tasks);
        return (summary, new_tasks);
    }

    // Case 3: Null or unexpected shape
    colmena_log!(
        "⚠️  [OrchestratorNode] Reactor returned null/unexpected result. No summary or new_tasks."
    );
    (String::new(), fallback_add_tasks)
}

// ── Suspend/Resume helpers ────────────────────────────────────────────────────

/// Reads the orchestrator suspend metadata from `global_shared_state`.
/// Returns `(suspended_at, phase, task_id, questions)` if present.
fn read_suspend_meta(state: &Value) -> Option<(String, i32, Option<String>, Vec<SuspendQuestion>)> {
    let meta = state.get("__orchestrator_suspend")?.as_object()?;
    let suspended_at = meta.get("suspended_at")?.as_str()?.to_string();
    let phase = meta.get("phase")?.as_i64()? as i32;
    let task_id = meta
        .get("task_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let questions: Vec<SuspendQuestion> = meta
        .get("questions")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    Some((suspended_at, phase, task_id, questions))
}

/// Writes suspend metadata into `global_shared_state` and returns the SUSPENDED output value.
/// `questions` is a structured array of questions to show the user.
fn make_suspend_response(
    state: &mut Value,
    suspended_at: &str,
    phase: i32,
    task_id: Option<&str>,
    questions: Vec<SuspendQuestion>,
) -> Value {
    let qs_val = serde_json::to_value(&questions).unwrap_or(json!([]));
    if let Some(obj) = state.as_object_mut() {
        obj.insert(
            "__orchestrator_suspend".to_string(),
            json!({
                "suspended_at": suspended_at,
                "phase": phase,
                "task_id": task_id,
                "questions": qs_val
            }),
        );
    }
    json!({ "__colmena_status": "SUSPENDED", "questions": qs_val })
}

/// Propagates a SUSPENDED status from an agent's child subgraph back through
/// the orchestrator. The agent's child run is already SUSPENDED in dag_runs;
/// our job is to:
///   1. extract user-facing questions from `agent_result` (canonical
///      `questions` array if present, else wrap the legacy `question` string),
///   2. emit a SUSPENDED response of our own via `make_suspend_response`,
///   3. with `suspended_at = "agent"` so the resume detection block knows
///      this came from an agent dispatch.
///
/// Note: caller is responsible for NOT marking the task `completed=true` —
/// the task stays incomplete in `dag_task_memory`, so it gets re-dispatched
/// on resume; the subgraph node's resume path then cascades the answer down
/// to the existing SUSPENDED child via `find_suspended_child`.
fn propagate_agent_suspend(
    agent_result: &Value,
    task: &DagTask,
    phase: i32,
    state: &mut Value,
) -> Value {
    // Try canonical `questions` array first. Covers nested orchestrators
    // (which already produced canonical output) AND the new SuspendNode shape.
    // If deserialization fails (malformed metadata), log and fall through.
    let canonical_questions: Option<Vec<SuspendQuestion>> =
        agent_result.get("questions").and_then(|qs_val| {
            match serde_json::from_value::<Vec<SuspendQuestion>>(qs_val.clone()) {
                Ok(qs) => Some(qs),
                Err(e) => {
                    colmena_log!(
                        "⚠️  [propagate_agent_suspend] Agent '{}' emitted malformed 'questions' \
                         array (will fall back to legacy 'question' string): {}",
                        task.assigned_to,
                        e
                    );
                    None
                }
            }
        });

    let questions: Vec<SuspendQuestion> = if let Some(qs) = canonical_questions {
        qs
    } else if let Some(q_str) = agent_result.get("question").and_then(|v| v.as_str()) {
        // Fallback: agent emitted only legacy single-string. Wrap as one open question.
        vec![SuspendQuestion {
            id: format!("agent_{}", task.assigned_to),
            question: q_str.to_string(),
            question_type: "open".to_string(),
            options: None,
        }]
    } else {
        // Degenerate: SUSPENDED but no questions metadata at all.
        vec![SuspendQuestion {
            id: format!("agent_{}", task.assigned_to),
            question: format!("Agent '{}' requires user input.", task.assigned_to),
            question_type: "open".to_string(),
            options: None,
        }]
    };

    make_suspend_response(state, "agent", phase, Some(&task.id), questions)
}

/// Removes the orchestrator suspend metadata from `global_shared_state` after a successful resume.
fn clear_suspend_meta(state: &mut Value) {
    if let Some(obj) = state.as_object_mut() {
        obj.remove("__orchestrator_suspend");
    }
}

/// Returns `true` if the given component config allows suspending (default: `true`).
/// Set `allow_suspend: false` in a component's config to disable Human-in-the-Loop for it.
fn allow_suspend_for(component_cfg: &Value) -> bool {
    component_cfg
        .get("allow_suspend")
        .and_then(|v| v.as_bool())
        .unwrap_or(true)
}

/// Prints a debug log when `allow_suspend: false` blocks a suspension.
fn debug_print_questions(context: &str, questions: &[SuspendQuestion]) {
    colmena_log!(
        "🚫 [OrchestratorNode] Suspend blocked by allow_suspend=false in '{}'.",
        context
    );
    colmena_log!("   Questions that would have been asked:");
    for q in questions {
        match q.question_type.as_str() {
            "choice" => {
                let opts = q
                    .options
                    .as_ref()
                    .map(|o| o.join(" | "))
                    .unwrap_or_default();
                colmena_log!("   [{} / choice] {} Options: {}", q.id, q.question, opts);
            }
            _ => {
                colmena_log!("   [{} / open] {}", q.id, q.question);
            }
        }
    }
}

/// Converts a list of `SuspendQuestion`s and a plain-text answer into a single
/// readable Q&A string for injection into LLM prompts.
fn questions_to_context_string(questions: &[SuspendQuestion], answer: &str) -> String {
    let q_lines: Vec<String> = questions
        .iter()
        .map(|q| {
            if let Some(opts) = &q.options {
                format!(
                    "Q [{}] (options: {}): {}",
                    q.id,
                    opts.join(", "),
                    q.question
                )
            } else {
                format!("Q [{}]: {}", q.id, q.question)
            }
        })
        .collect();
    format!("{}\nA: {}", q_lines.join("\n"), answer)
}

// Helper: convert a Vec<DagTask> to a JSON array with result content
fn build_tasks_json(tasks: Vec<crate::dag_engine::domain::state::DagTask>) -> Value {
    let arr: Vec<Value> = tasks
        .iter()
        .map(|t| {
            let result_content = t
                .result
                .as_ref()
                .and_then(|r| r.get("result"))
                .cloned()
                .unwrap_or(Value::Null);

            json!({
                "id":          t.id,
                "task_name":   t.task_name,
                "assigned_to": t.assigned_to,
                "completed":   t.completed,
                "phase":       t.phase,
                "parallel":    t.parallel,
                "result": {
                    "content": result_content
                }
            })
        })
        .collect();

    Value::Array(arr)
}
