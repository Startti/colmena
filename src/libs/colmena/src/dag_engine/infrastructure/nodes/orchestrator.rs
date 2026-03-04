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

        // Verbose flag for debugging.
        let verbose = config.get("verbose").and_then(|v| v.as_bool()).unwrap_or(false);

        let mut target_agent = None;
        let mut active_task = None;
        let mut loop_status = "NEXT_TURN";
        let mut active_task_id = None;

        if verbose {
            println!("\n═══════════════════════════════════════");
            println!("🚦 [OrchestratorNode] VERBOSE — run_id: {}", run_id);
            println!("───────────────────────────────────────");
            println!("Inputs: {:?}", inputs);
            println!("═══════════════════════════════════════\n");
        }

        // 1. Check if we have a Database Repository injected
        if let Some(repo) = &self.task_memory_repo {
            // DATABASE-BACKED ORCHESTRATION
            let mut existing_tasks = repo.get_tasks_for_run(&run_id).await?;

            // Setup: If the DB is empty for this run, but we received a `plan` array from the LLM Planner,
            // we must commit this initial plan into the DB!
            if existing_tasks.is_empty() {
                if let Some(plan_val) = inputs.get("plan").or_else(|| config.get("plan")) {
                    let plan_array_opt = plan_val.as_array()
                        .or_else(|| plan_val.get("items").and_then(|i| i.as_array()));
                    
                    if let Some(plan_array) = plan_array_opt {
                        for task_val in plan_array {
                            if let Some(task_obj) = task_val.as_object() {
                                let task_name = task_obj.get("task").and_then(|v| v.as_str()).unwrap_or("Unknown").to_string();
                                let assigned_to = task_obj.get("assigned_to").and_then(|v| v.as_str()).unwrap_or("Unknown").to_string();
                                
                                let new_task = crate::dag_engine::domain::state::DagTask {
                                    id: uuid::Uuid::new_v4().to_string(),
                                    run_id: run_id.clone(),
                                    task_name,
                                    assigned_to,
                                    completed: false,
                                    result: None,
                                };
                                repo.add_task(&new_task).await?;
                            }
                        }
                    }
                }
            }

            // Execution: Fetch the first pending task
            if let Some(mut task) = repo.get_first_uncompleted_task(&run_id).await? {
                // Mark it as completed so the next loop turn doesn't pick it up again
                // (Workers/Critics can update the 'result' field later)
                repo.update_task_result(&task.id, json!({"status": "dispatched"})).await?;
                
                target_agent = Some(task.assigned_to.clone());
                active_task_id = Some(task.id.clone());
                active_task = Some(json!({
                    "id": task.id,
                    "task": task.task_name,
                    "assigned_to": task.assigned_to,
                    "completed": true
                }));
            } else {
                // No uncompleted tasks left!
                loop_status = "FINISHED";
                
                let mut output_tasks = Vec::new();
                if let Ok(tasks) = repo.get_tasks_for_run(&run_id).await {
                    for t in tasks {
                        output_tasks.push(json!({
                            "id": t.id,
                            "task_name": t.task_name,
                            "assigned_to": t.assigned_to,
                            "completed": t.completed,
                            "result": t.result
                        }));
                    }
                }
                
                return Ok(json!({
                    "output": {
                        "__colmena_loop_status": loop_status,
                        "all_tasks": output_tasks
                    }
                }));
            }
        } else {
            // STATIC ARRAY MEMORY FALLBACK (For backwards compatibility / No-DB testing)
            let mut plan_array = if let Some(plan_val) = inputs.get("plan").or_else(|| config.get("plan")) {
                let plan_array_opt = plan_val.as_array()
                    .or_else(|| plan_val.get("items").and_then(|i| i.as_array()));
                plan_array_opt.ok_or("Orchestrator error: 'plan' must be an array or an object with an 'items' array")?.clone()
            } else {
                return Err("Orchestrator error: Missing 'plan' input array".into());
            };

            let mut all_completed = true;
            for task_val in plan_array.iter_mut() {
                if let Some(task_obj) = task_val.as_object_mut() {
                    let is_completed = task_obj.get("completed").and_then(|v| v.as_bool()).unwrap_or(false);
                    if !is_completed {
                        all_completed = false;
                        task_obj.insert("completed".to_string(), json!(true));
                        target_agent = task_obj.get("assigned_to").and_then(|v| v.as_str()).map(|s| s.to_string());
                        active_task = Some(task_val.clone());
                        break;
                    }
                }
            }
            if all_completed {
                loop_status = "FINISHED";
                return Ok(json!({
                    "output": {
                        "__colmena_loop_status": loop_status,
                        "all_tasks": plan_array
                    }
                }));
            }
        }

        // 3. Prepare the routing payload
        let mut agent_routing = serde_json::Map::new();
        if let Some(agent_name) = target_agent.clone() {
            if let Some(task) = active_task.clone() {
                if verbose {
                    println!("🚦 [OrchestratorNode] Routing task to agent: '{}'", agent_name);
                }
                agent_routing.insert(agent_name, task);
            }
        }

        // 4. Output the updated routing payload and loop status
        Ok(json!({
            "output": {
                "__colmena_loop_status": loop_status,
                "agents": agent_routing,
                "active_agent": target_agent,
                "active_task": active_task,
                "active_task_id": active_task_id
            }
        }))
    }

    fn description(&self) -> Option<&str> {
        Some("Iterates through an array of tasks (a plan) marking one as completed per turn and mapping its payload to the designated agent.")
    }

    fn schema(&self) -> Value {
        json!({
            "type": "orchestrator",
            "inputs": {
                "plan": "Array of task objects. Example: [{\"assigned_to\": \"agent_name\", \"completed\": false, \"data\": {}}]"
            },
            "outputs": {
                "__colmena_loop_status": "NEXT_TURN or FINISHED depending on the plan completion.",
                "plan": "The updated array of tasks.",
                "agents": "Object mapping active task data to the 'assigned_to' agent field.",
                "active_agent": "String name of the agent processing this turn.",
                "active_task": "The currently executing task object."
            }
        })
    }
}
