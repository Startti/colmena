use crate::dag_engine::domain::node::{ExecutableNode, NodeInputs};
use serde_json::{json, Value};
use std::error::Error as StdError;
use std::sync::Arc;

pub struct OrchestratorNode {
    task_memory_repo: Option<Arc<dyn crate::dag_engine::domain::state::DagTaskMemoryRepository>>,
}

impl OrchestratorNode {
    pub fn new(task_memory_repo: Option<Arc<dyn crate::dag_engine::domain::state::DagTaskMemoryRepository>>) -> Self {
        Self { task_memory_repo }
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
        let run_id = _state.get("run_id").and_then(|v| v.as_str()).unwrap_or("unknown_run").to_string();
        let verbose = config.get("verbose").and_then(|v| v.as_bool()).unwrap_or(false);

        if verbose {
            println!("\n═══════════════════════════════════════");
            println!("🚦 [OrchestratorNode] VERBOSE — run_id: {}", run_id);
            println!("───────────────────────────────────────");
            println!("Inputs: {:?}", inputs);
            println!("═══════════════════════════════════════\n");
        }

        if let Some(repo) = &self.task_memory_repo {
            // ── 1. Seed DB with initial plan if empty ──────────────────────────
            let existing_tasks = repo.get_tasks_for_run(&run_id).await?;

            if existing_tasks.is_empty() {
                if let Some(plan_val) = _state.get("plan").or_else(|| inputs.get("plan")).or_else(|| config.get("plan")) {
                    let plan_array_opt = plan_val.as_array()
                        .or_else(|| plan_val.get("items").and_then(|i| i.as_array()));

                    if let Some(plan_array) = plan_array_opt {
                        for task_val in plan_array {
                            if let Some(task_obj) = task_val.as_object() {
                                println!("🐛 [OrchestratorNode] RAW TASK FROM PLANNER: {}", serde_json::to_string(task_obj).unwrap_or_default());
                                let task_name  = task_obj.get("task").and_then(|v| v.as_str()).unwrap_or("Unknown").to_string();
                                let assigned   = task_obj.get("assigned_to").and_then(|v| v.as_str()).unwrap_or("Unknown").to_string();
                                let phase = task_obj.get("phase").and_then(|v| {
                                    if let Some(num) = v.as_i64() { Some(num) }
                                    else if let Some(s) = v.as_str() { s.parse::<i64>().ok() }
                                    else { None }
                                }).unwrap_or(1) as i32;

                                let parallel = task_obj.get("parallel").and_then(|v| {
                                    if let Some(b) = v.as_bool() { Some(b) }
                                    else if let Some(s) = v.as_str() { Some(s.eq_ignore_ascii_case("true")) }
                                    else { None }
                                }).unwrap_or(false);
                                
                                println!("🐛 [OrchestratorNode] PARSED -> phase: {}, parallel: {}", phase, parallel);

                                let new_task = crate::dag_engine::domain::state::DagTask {
                                    id: uuid::Uuid::new_v4().to_string(),
                                    run_id: run_id.clone(),
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
                }
            }

            // ── 2. Find current phase ──────────────────────────────────────────
            let current_phase_opt = repo.get_current_phase(&run_id).await?;

            match current_phase_opt {
                None => {
                    // ── All phases done → FINISHED ─────────────────────────────
                    println!("✅ [OrchestratorNode] All phases complete → FINISHED");

                    let all_tasks_json = build_tasks_json(repo.get_tasks_for_run(&run_id).await?);
                    let phase_summaries = repo.get_phase_summaries(&run_id).await?;
                    let phase_summaries_json: Vec<Value> = phase_summaries.iter().map(|s| json!({
                        "phase": s.phase,
                        "summary": s.summary
                    })).collect();

                    return Ok(json!({
                        "output": {
                            "result": { "all_tasks": all_tasks_json },
                            "extra_info": {
                                "__colmena_loop_status": "FINISHED",
                                "phase_summaries": phase_summaries_json
                            }
                        }
                    }));
                }

                Some(current_phase) => {
                    let incomplete_in_phase = repo.get_uncompleted_tasks_for_phase(&run_id, current_phase).await?;

                    if incomplete_in_phase.is_empty() {
                        // Phase exists but all tasks are done → FINISHED_PHASE
                        // (This should not happen because get_current_phase only returns phases with incomplete tasks,
                        //  but guard just in case.)
                        let phase_tasks = build_tasks_json(
                            repo.get_tasks_for_run(&run_id).await?.into_iter()
                                .filter(|t| t.phase == current_phase)
                                .collect()
                        );
                        println!("🏁 [OrchestratorNode] Phase {} complete → FINISHED_PHASE", current_phase);
                        return Ok(json!({
                            "output": {
                                "result": { "phase_tasks": phase_tasks },
                                "extra_info": {
                                    "__colmena_loop_status": "FINISHED_PHASE",
                                    "current_phase": current_phase
                                }
                            }
                        }));
                    }

                    // ── 3. Determine dispatch strategy ─────────────────────────
                    let any_parallel = incomplete_in_phase.iter().any(|t| t.parallel);

                    let tasks_to_dispatch: Vec<_> = if any_parallel {
                        // Parallel mode: dispatch ALL incomplete tasks of this phase at once
                        incomplete_in_phase
                    } else {
                        // Sequential mode: dispatch just the first task
                        incomplete_in_phase.into_iter().take(1).collect()
                    };

                    // Mark all dispatched tasks as completed instantly (so next turn they are skipped)
                    let mut agents_map = serde_json::Map::new();
                    let mut first_task_id = None;
                    let mut first_task_json = None;

                    for task in &tasks_to_dispatch {
                        agents_map.insert(task.assigned_to.clone(), json!({
                            "task": task.task_name,
                            "id":   task.id
                        }));

                        if first_task_id.is_none() {
                            first_task_id = Some(task.id.clone());
                            first_task_json = Some(json!({
                                "id":          task.id,
                                "task":        task.task_name,
                                "assigned_to": task.assigned_to,
                                "completed":   task.completed,
                                "phase":       task.phase,
                                "parallel":    task.parallel
                            }));
                        }

                        if any_parallel {
                            println!("🚦 [OrchestratorNode] [Phase {}|parallel] Routing task to agent: '{}'", current_phase, task.assigned_to);
                        } else {
                            println!("🚦 [OrchestratorNode] [Phase {}|sequential] Routing task to agent: '{}'", current_phase, task.assigned_to);
                        }
                    }

                    // ── 4. After dispatch, check if phase is now fully done ────
                    let remaining = repo.get_uncompleted_tasks_for_phase(&run_id, current_phase).await?;

                    if remaining.is_empty() {
                        // Phase now fully dispatched → signal FINISHED_PHASE on this same turn
                        // BUT we still need agents to know what to do this turn, so we return NEXT_TURN
                        // and the FINISHED_PHASE will be detected on the *next* turn when agents have run.
                        // (Agents run, write results, then next turn we detect phase complete.)
                    }

                    // Omit all_tasks from the general turn output so the final_reactor is not queued early
                    Ok(json!({
                        "output": {
                            "result": { 
                                "dispatched_agents": Value::Object(agents_map)
                            },
                            "extra_info": {
                                "__colmena_loop_status": "NEXT_TURN",
                                "active_task_id": first_task_id,
                                "active_task": first_task_json
                            }
                        }
                    }))
                }
            }
        } else {
            // ── Static plan (no DB) — fallback to old behavior ────────────────
            if let Some(plan_val) = inputs.get("plan").or_else(|| config.get("plan")) {
                let plan_array_opt = plan_val.as_array()
                    .or_else(|| plan_val.get("items").and_then(|i| i.as_array()));

                if let Some(plan_array) = plan_array_opt {
                    let all_completed = plan_array.iter().all(|t| {
                        t.get("completed").and_then(|v| v.as_bool()).unwrap_or(false)
                    });

                    if all_completed {
                        return Ok(json!({
                            "output": {
                                "result": { "all_tasks": plan_array },
                                "extra_info": {
                                    "__colmena_loop_status": "FINISHED"
                                }
                            }
                        }));
                    }

                    if let Some(task) = plan_array.iter().find(|t| {
                        !t.get("completed").and_then(|v| v.as_bool()).unwrap_or(false)
                    }) {
                        let agent = task.get("assigned_to").and_then(|v| v.as_str()).unwrap_or("unknown");
                        println!("🚦 [OrchestratorNode] Routing task to agent: '{}'", agent);
                        
                        return Ok(json!({
                            "output": {
                                "result": { 
                                    "dispatched_agents": { agent: { "task": task.get("task") } },
                                    "all_tasks": plan_array
                                },
                                "extra_info": {
                                    "__colmena_loop_status": "NEXT_TURN",
                                    "active_task": task
                                }
                            }
                        }));
                    }
                }
            }

            Ok(json!({
                "output": {
                    "result": { "all_tasks": [] },
                    "extra_info": {
                        "__colmena_loop_status": "FINISHED"
                    }
                }
            }))
        }
    }

    fn description(&self) -> Option<&str> {
        Some("Routes tasks to agents phase by phase. Supports parallel dispatch within a phase and FINISHED_PHASE/FINISHED signaling.")
    }

    fn schema(&self) -> Value {
        json!({
            "type": "orchestrator",
            "outputs": {
                "__colmena_loop_status": "NEXT_TURN | FINISHED_PHASE | FINISHED",
                "current_phase": "integer",
                "agents": "object (agent_id → { task, id })",
                "phase_tasks": "array — tasks of the just-completed phase (on FINISHED_PHASE)",
                "all_tasks": "array — all tasks (on FINISHED)",
                "phase_summaries": "array — phase summaries (on FINISHED)"
            }
        })
    }
}

// Helper: convert a Vec<DagTask> to a JSON array with result content
fn build_tasks_json(tasks: Vec<crate::dag_engine::domain::state::DagTask>) -> Value {
    let arr: Vec<Value> = tasks.iter().map(|t| {
        let result_content = t.result.as_ref()
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
    }).collect();

    Value::Array(arr)
}
