use crate::dag_engine::application::ports::NodeRegistryPort;
use crate::dag_engine::domain::node::{ExecutableNode, NodeInputs};
use crate::dag_engine::domain::state::{DagTask, DagTaskMemoryRepository};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::error::Error as StdError;
use std::sync::{Arc, Weak};

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
    ) -> Result<Value, Box<dyn StdError + Send + Sync>> {
        println!(
            "🏁 [OrchestratorNode] Phase {} complete. Processing reaction...",
            phase
        );

        let all_tasks = repo.get_tasks_for_run(session_id).await?;
        let phase_tasks: Vec<_> = all_tasks.iter().filter(|t| t.phase == phase).collect();

        let phase_output;

        if let Some(reactor_cfg) = config.get("phase_reactor") {
            println!("⚡ [OrchestratorNode] Internal Phase Reactor starting...");
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
            let available_agents: Vec<String> = config
                .get("agents")
                .and_then(|a| a.as_object())
                .map(|obj| obj.keys().cloned().collect())
                .unwrap_or_default();

            let mut reactor_cfg_owned = reactor_cfg.clone();
            inject_reactor_schema_constraints(&mut reactor_cfg_owned, &available_agents);

            // Opción 3: Inyectar historial de agentes ya ejecutados en session.
            // El reactor sabrá que esos agentes ya corrieron y evitará re-proponerlos
            // como recovery tasks a menos que sea genuinamente necesario.
            let all_tasks_so_far = repo.get_tasks_for_run(session_id).await.unwrap_or_default();
            let completed_history: Vec<String> = all_tasks_so_far
                .iter()
                .filter(|t| t.completed)
                .map(|t| format!("- {} (agent: {}, phase: {})", t.task_name, t.assigned_to, t.phase))
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

            println!(
                "📨 [OrchestratorNode] REACTOR INPUTS (phase {}):\n{}\n{}",
                phase,
                "─".repeat(60),
                serde_json::to_string_pretty(&reactor_inputs).unwrap_or_else(|_| format!("{:?}", reactor_inputs))
            );

            let reactor_res = reactor_node
                .execute(&reactor_inputs, &reactor_cfg_owned, state, observer.clone())
                .await?;

            println!(
                "📬 [OrchestratorNode] REACTOR RAW RESULT (phase {}):\n{}\n{}\n{}",
                phase,
                "─".repeat(60),
                serde_json::to_string_pretty(&reactor_res).unwrap_or_else(|_| format!("{:?}", reactor_res)),
                "─".repeat(60)
            );

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

            let (mut summary_str, new_tasks_val) = extract_reactor_output_with_fallback(&reactor_result, reactor_add_tasks);

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
                repo.save_phase_summary(session_id, phase, &summary_str).await?;
                println!("💾 [OrchestratorNode] Phase {} summary saved.", phase);
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
                    if !available_agents.contains(&assigned_to) {
                        println!(
                            "⚠️  [OrchestratorNode] Reactor hallucinated agent '{}' — discarding task '{}'.",
                            assigned_to, task_name
                        );
                        continue;
                    }

                    // Deduplicar: si ya existe una tarea (completada o no) con el mismo
                    // nombre y agente, no insertar. Esto evita loops donde el reactor
                    // propone la misma tarea que ya fue ejecutada en una fase anterior.
                    let already_exists = existing_tasks.iter().any(|t| {
                        t.assigned_to == assigned_to && t.task_name == task_name
                    });
                    if already_exists {
                        println!(
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

                    let new_task = DagTask {
                        id: uuid::Uuid::new_v4().to_string(),
                        session_id: session_id.to_string(),
                        task_name: task_name.clone(),
                        assigned_to: assigned_to.clone(),
                        completed: false,
                        result: None,
                        phase: next_phase,
                        parallel,
                        context,
                    };
                    repo.add_task(&new_task).await?;
                    sowed += 1;
                }

                if sowed > 0 {
                    println!(
                        "🌱 [OrchestratorNode] Sowed {} new recovery tasks into phase {} via Reactor.",
                        sowed, next_phase
                    );
                }
            }

            phase_output = Value::String(summary_str);
        } else {
            // ── Step 4: Fallback sin reactor ─────────────────────────────────
            println!("🔗 [OrchestratorNode] No reactor found. Concatenating phase results (truncated fallback)...");
            println!("⚠️  [OrchestratorNode] For production flows, configure 'phase_reactor' to enable context summarization and dynamic replanning.");

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
                format!("{}\n\n[...truncated — configure phase_reactor for full context]", cut)
            } else {
                full_concat
            };

            phase_output = Value::String(truncated);
        }

        Ok(json!({
            "phase_tasks": build_tasks_json(phase_tasks.into_iter().cloned().collect()),
            "phase_output": phase_output,
            "extra_info": {
                "__colmena_loop_status": "FINISHED_PHASE",
                "current_phase": phase
            }
        }))
    }

    async fn finalize_execution(
        &self,
        repo: &Arc<dyn DagTaskMemoryRepository>,
        session_id: &str,
        config: &Value,
        state: &mut Value,
        observer: Option<Arc<dyn crate::dag_engine::domain::observer::ExecutionObserver>>,
    ) -> Result<Value, Box<dyn StdError + Send + Sync>> {
        println!("✅ [OrchestratorNode] All phases complete. Finalizing...");

        let all_tasks = repo.get_tasks_for_run(session_id).await?;
        let all_tasks_json = build_tasks_json(all_tasks);
        let phase_summaries = repo.get_phase_summaries(session_id).await?;

        let mut final_response = Value::Null;

        if let Some(final_reactor_cfg) = config.get("final_reactor") {
            println!("⚡ [OrchestratorNode] Internal Final Reactor starting...");
            let registry = self.registry.upgrade().ok_or("Registry already dropped")?;
            let reactor_node = registry
                .get_node("reactor")
                .ok_or("Reactor node not found")?;

            let mut reactor_inputs = HashMap::new();
            for summary in &phase_summaries {
                reactor_inputs.insert(
                    format!("texts.phase_{}", summary.phase),
                    Value::String(summary.summary.clone()),
                );
            }

            let reactor_res = reactor_node
                .execute(&reactor_inputs, final_reactor_cfg, state, observer)
                .await?;
            final_response = reactor_res.get("result").cloned().unwrap_or(Value::Null);
        }

        let phase_summaries_json: Vec<Value> = phase_summaries
            .iter()
            .map(|s| {
                json!({
                    "phase": s.phase,
                    "summary": s.summary
                })
            })
            .collect();

        Ok(json!({
            "all_tasks": all_tasks_json,
            "final_response": final_response,
            "extra_info": {
                "__colmena_loop_status": "FINISHED",
                "phase_summaries": phase_summaries_json
            }
        }))
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
            println!("\n═══════════════════════════════════════");
            println!("🚦 [OrchestratorNode] VERBOSE — session_id: {}", session_id);
            println!("───────────────────────────────────────");
            println!("Inputs: {:?}", inputs);
            println!("═══════════════════════════════════════\n");
        }

        if let Some(repo) = &self.task_memory_repo {
            // ── 1. Auto-Planificación ─────────────────────────────────────────
            // Si hay config de 'planner' y la DB está vacía, ejecutamos el planner interno
            if let Some(planner_cfg) = config.get("planner") {
                let existing_tasks = repo.get_tasks_for_run(&session_id).await?;
                if existing_tasks.is_empty() {
                    println!("🗂️ [OrchestratorNode] Internal Planning started...");

                    // Derivar agentes automáticamente de config["agents"]
                    let mut agent_names = Vec::new();
                    if let Some(agents_obj) = config.get("agents").and_then(|a| a.as_object()) {
                        for name in agents_obj.keys() {
                            agent_names.push(Value::String(name.clone()));
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
                        // que describe por qué existe la tarea y cuál es la intención del usuario.
                        let context_instruction = "\n\nFor each task in your plan, you MUST include a \"context\" field: \
                            a 1-2 sentence explanation of why this task exists and what the user's intent is \
                            (e.g. \"The user wants to know what clothing to pack for a ski trip in cold conditions.\"). \
                            This context will be shown to the agent alongside the task instruction.";
                        let existing_sm = obj
                            .get("system_message")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        obj.insert(
                            "system_message".to_string(),
                            Value::String(format!("{}{}", existing_sm, context_instruction)),
                        );
                    }

                    let registry = self.registry.upgrade().ok_or("Registry already dropped")?;
                    let planner_node = registry
                        .get_node("planner")
                        .ok_or("Planner node not found")?;

                    // Ejecutamos el planner. El PlannerNode escribe el plan en 'state' y retorna { result: { items: [...] } }
                    let planner_result = planner_node
                        .execute(inputs, &internal_planner_cfg, _state, _observer.clone())
                        .await?;

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
                                };
                                repo.add_task(&new_task).await?;
                            }
                        }
                        // Print plan in human-readable format for debugging
                        println!("💾 [OrchestratorNode] DB seeded with {} tasks from internal planner.", items.len());
                        println!("📋 [OrchestratorNode] PLAN:");
                        println!("{}", "─".repeat(60));
                        let mut phase_map: std::collections::BTreeMap<i64, Vec<String>> = std::collections::BTreeMap::new();
                        for task_val in items {
                            let phase_num = task_val.get("phase").and_then(|v| v.as_i64()).unwrap_or(1);
                            let task_name = task_val.get("task").and_then(|v| v.as_str()).unwrap_or("?");
                            let agent = task_val.get("assigned_to").and_then(|v| v.as_str()).unwrap_or("?");
                            let parallel = task_val.get("parallel").and_then(|v| v.as_bool()).unwrap_or(false);
                            let ctx = task_val.get("context").and_then(|v| v.as_str()).unwrap_or("");
                            let parallel_tag = if parallel { " [parallel]" } else { "" };
                            let entry = format!("  [{agent}]{parallel_tag}: {task_name}\n    → {ctx}");
                            phase_map.entry(phase_num).or_default().push(entry);
                        }
                        for (ph, tasks) in &phase_map {
                            println!("Phase {}:", ph);
                            for t in tasks { println!("{}", t); }
                        }
                        println!("{}", "─".repeat(60));
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
                        return self
                            .finalize_execution(repo, &session_id, config, _state, _observer)
                            .await;
                    }

                    Some(current_phase) => {
                        // Safety net: si se supera el límite de fases, forzar finalización.
                        // Evita loops infinitos cuando el reactor propone tareas indefinidamente.
                        if current_phase > max_phases {
                            println!(
                                "⚠️  [OrchestratorNode] max_phases ({}) reached at phase {}. Forcing finalization.",
                                max_phases, current_phase
                            );
                            return self
                                .finalize_execution(repo, &session_id, config, _state, _observer)
                                .await;
                        }

                        let incomplete_in_phase = repo
                            .get_uncompleted_tasks_for_phase(&session_id, current_phase)
                            .await?;

                        if incomplete_in_phase.is_empty() {
                            // Fase terminada -> Reaccionar y continuar al siguiente ciclo
                            self
                                .handle_phase_completion(
                                    repo,
                                    &session_id,
                                    current_phase,
                                    config,
                                    _state,
                                    _observer.clone(),
                                )
                                .await?;
                            // Continuar el loop: puede haber más fases (incluyendo las sembradas por el reactor)
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
                            println!(
                                "🚀 [OrchestratorNode] Internal Execution: Task '{}' -> Agent '{}'",
                                task.task_name, task.assigned_to
                            );

                            // Preparar inputs para el agente
                            let mut task_inputs = inputs.clone();
                            task_inputs
                                .insert("task".to_string(), Value::String(task.task_name.clone()));

                            // ── Context Builder ──────────────────────────────────────
                            // Construir un prompt enriquecido con contexto histórico para agentes
                            // en fases avanzadas, evitando explotar la ventana de contexto con
                            // resultados crudos de la fase anterior.
                            let phase_summaries_for_ctx = repo.get_phase_summaries(&session_id).await.unwrap_or_default();
                            let enriched_prompt = build_enriched_prompt(
                                &phase_summaries_for_ctx,
                                &task.task_name,
                                task.context.as_deref(),
                            );

                            println!(
                                "📨 [OrchestratorNode] PROMPT → agent '{}' | task '{}'\n{}\n{}",
                                task.assigned_to,
                                task.task_name,
                                "─".repeat(60),
                                enriched_prompt
                            );

                            task_inputs
                                .insert("prompt".to_string(), Value::String(enriched_prompt));

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

                            // Ejecutar Agente (LlmNode)
                            let llm_node = registry
                                .get_node("llm_call")
                                .ok_or("llm_call node not found")?;
                            let agent_result = llm_node
                                .execute(&task_inputs, agent_node_cfg, _state, _observer.clone())
                                .await?;

                            println!(
                                "📬 [OrchestratorNode] RAW RESULT ← agent '{}' | task '{}'\n{}\n{}\n{}",
                                task.assigned_to,
                                task.task_name,
                                "─".repeat(60),
                                serde_json::to_string_pretty(&agent_result).unwrap_or_else(|_| format!("{:?}", agent_result)),
                                "─".repeat(60)
                            );

                            // ── Crítica ──
                            let mut is_ok = true;
                            if let Some(critic_cfg) = config.get("critic") {
                                println!(
                                    "🔎 [OrchestratorNode] Internal Critique for task '{}'...",
                                    task.task_name
                                );
                                let critic_node =
                                    registry.get_node("critic").ok_or("critic node not found")?;

                                let mut critic_inputs = task_inputs.clone();
                                critic_inputs.insert("agent_result".to_string(), agent_result.clone());
                                critic_inputs.insert(
                                    format!("texts.{}", task.assigned_to),
                                    agent_result.clone(),
                                );

                                let critic_res = critic_node
                                    .execute(&critic_inputs, critic_cfg, _state, _observer.clone())
                                    .await?;
                                is_ok = critic_res
                                    .get("extra_info")
                                    .and_then(|e| e.get("task_ok"))
                                    .and_then(|v| v.as_bool())
                                    .unwrap_or(true);
                            }

                            if is_ok {
                                repo.update_task_result(&task.id, agent_result).await?;
                                println!(
                                    "✅ [OrchestratorNode] Task '{}' completed successfully.",
                                    task.task_name
                                );
                            } else {
                                println!(
                                    "❌ [OrchestratorNode] Task '{}' rejected by critic.",
                                    task.task_name
                                );
                                // TODO: Lógica de re-intento o bifurcación. Por ahora solo dejamos incompleta.
                            }
                        }
                        // After running tasks, check if this phase is now fully complete
                        let remaining = repo
                            .get_uncompleted_tasks_for_phase(&session_id, current_phase)
                            .await?;
                        if remaining.is_empty() {
                            self.handle_phase_completion(
                                repo,
                                &session_id,
                                current_phase,
                                config,
                                _state,
                                _observer.clone(),
                            )
                            .await?;
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
                };
                repo.add_task(&new_task).await?;
            }
        }
    }
    Ok(())
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
) -> String {
    let mut parts = Vec::new();

    // Section 1: task context — personalized intent synthesized by the Planner or Reactor.
    // Replaces the raw user prompt: more concise and directly relevant to this specific task.
    if let Some(ctx) = task_context {
        if !ctx.is_empty() {
            parts.push(format!("=== CONTEXTO DE ESTA TAREA ===\n{}", ctx));
        }
    }

    // Section 2: phase summaries (what happened in previous phases)
    if !phase_summaries.is_empty() {
        let summaries_text: Vec<String> = phase_summaries
            .iter()
            .map(|s| format!("Fase {}: {}", s.phase, s.summary))
            .collect();
        parts.push(format!(
            "=== LO QUE HA OCURRIDO HASTA AHORA ===\n{}",
            summaries_text.join("\n")
        ));
    }

    // Section 3: current task instruction
    parts.push(format!(
        "=== LO QUE TIENES QUE HACER AHORA TÚ ===\n{}",
        task_name
    ));

    parts.join("\n\n")
}

/// Injects schema constraints into a reactor config's system_message so the LLM
/// knows the required output format and which agents are valid for new_tasks.
fn inject_reactor_schema_constraints(reactor_cfg: &mut Value, available_agents: &[String]) {
    let schema_instruction = format!(
        "\n\n---\nIMPORTANT: You MUST respond with valid JSON only. No prose, no markdown fences.\n\
        Required format:\n\
        {{\n\
          \"summary\": \"<concise summary of this phase's results>\",\n\
          \"requires_replanning\": <true|false>,\n\
          \"new_tasks\": [\n\
            {{\n\
              \"task\": \"<specific task instruction for the agent>\",\n\
              \"context\": \"<why this task is needed and what the user's intent is>\",\n\
              \"assigned_to\": \"<agent_name>\",\n\
              \"parallel\": <true|false>\n\
            }}\n\
          ]\n\
        }}\n\
        Available agents for new_tasks (ONLY use these): [{}]\n\
        If no new tasks are needed, set new_tasks to [].",
        available_agents.join(", ")
    );

    if let Some(obj) = reactor_cfg.as_object_mut() {
        let existing = obj
            .get("system_message")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        obj.insert(
            "system_message".to_string(),
            Value::String(format!("{}{}", existing, schema_instruction)),
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
                let new_tasks = obj
                    .get("new_tasks")
                    .cloned()
                    .or(fallback_add_tasks);
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
    println!("⚠️  [OrchestratorNode] Reactor returned null/unexpected result. No summary or new_tasks.");
    (String::new(), fallback_add_tasks)
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
