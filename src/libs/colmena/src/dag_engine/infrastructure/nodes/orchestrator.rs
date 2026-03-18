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

        let mut phase_output = Value::Null;

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
                    let text = res.get("content").and_then(|c| c.as_str()).unwrap_or("");
                    reactor_inputs.insert(
                        format!("texts.{}", task.assigned_to),
                        Value::String(text.to_string()),
                    );
                }
            }

            let reactor_res = reactor_node
                .execute(&reactor_inputs, reactor_cfg, state, observer)
                .await?;
            phase_output = reactor_res.get("result").cloned().unwrap_or(Value::Null);
        } else {
            // FALLBACK: Concatenar resultados
            println!("🔗 [OrchestratorNode] No reactor found. Concatenating phase results...");
            let mut results = Vec::new();
            for task in &phase_tasks {
                if let Some(res) = &task.result {
                    let text = res.get("content").and_then(|c| c.as_str()).unwrap_or("");
                    results.push(format!("# Agent: {}\n\n{}", task.assigned_to, text));
                }
            }
            phase_output = Value::String(results.join("\n\n---\n\n"));
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

                                let new_task = DagTask {
                                    id: uuid::Uuid::new_v4().to_string(),
                                    session_id: session_id.clone(),
                                    task_name,
                                    assigned_to: assigned,
                                    completed: false,
                                    result: None,
                                    phase,
                                    parallel,
                                };
                                repo.add_task(&new_task).await?;
                            }
                        }
                        println!(
                            "💾 [OrchestratorNode] DB seeded with {} tasks from internal planner.",
                            items.len()
                        );
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

            // ── 2. Despacho, Crítica y Ejecución Interna ──────────────────────
            let current_phase_opt = repo.get_current_phase(&session_id).await?;

            match current_phase_opt {
                None => {
                    // Todas las fases terminadas
                    return self
                        .finalize_execution(repo, &session_id, config, _state, _observer)
                        .await;
                }

                Some(current_phase) => {
                    let incomplete_in_phase = repo
                        .get_uncompleted_tasks_for_phase(&session_id, current_phase)
                        .await?;

                    if incomplete_in_phase.is_empty() {
                        // Fase terminada -> Reaccionar
                        return self
                            .handle_phase_completion(
                                repo,
                                &session_id,
                                current_phase,
                                config,
                                _state,
                                _observer,
                            )
                            .await;
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
                        task_inputs
                            .insert("prompt".to_string(), Value::String(task.task_name.clone()));

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
                            // También pasamos el resultado bajo la llave que espera el critic si es necesario
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

                    // Después de ejecutar, volvemos a evaluar si la fase terminó
                    let remaining = repo
                        .get_uncompleted_tasks_for_phase(&session_id, current_phase)
                        .await?;
                    if remaining.is_empty() {
                        return self
                            .handle_phase_completion(
                                repo,
                                &session_id,
                                current_phase,
                                config,
                                _state,
                                _observer,
                            )
                            .await;
                    }

                    Ok(json!({
                        "extra_info": {
                            "__colmena_loop_status": "NEXT_TURN",
                            "current_phase": current_phase
                        }
                    }))
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
                        } else if let Some(s) = v.as_str() {
                            Some(s.eq_ignore_ascii_case("true"))
                        } else {
                            None
                        }
                    })
                    .unwrap_or(false);

                let new_task = DagTask {
                    id: uuid::Uuid::new_v4().to_string(),
                    session_id: session_id.to_string(),
                    task_name,
                    assigned_to: assigned,
                    completed: false,
                    result: None,
                    phase,
                    parallel,
                };
                repo.add_task(&new_task).await?;
            }
        }
    }
    Ok(())
}

// Helper: convert a Vec<DagTask> to a JSON array with result content
fn build_tasks_json(tasks: Vec<crate::dag_engine::domain::state::DagTask>) -> Value {
    let arr: Vec<Value> = tasks
        .iter()
        .map(|t| {
            let result_content = t
                .result
                .as_ref()
                .and_then(|r| r.get("content"))
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
