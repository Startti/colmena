use crate::dag_engine::domain::node::{ExecutableNode, NodeInputs};
use serde_json::{json, Value};
use std::error::Error as StdError;
use std::sync::Arc;
use uuid::Uuid;

pub struct TaskMemoryWriterNode {
    task_memory_repo: Option<Arc<dyn crate::dag_engine::domain::state::DagTaskMemoryRepository>>,
}

impl TaskMemoryWriterNode {
    pub fn new(task_memory_repo: Option<Arc<dyn crate::dag_engine::domain::state::DagTaskMemoryRepository>>) -> Self {
        Self { task_memory_repo }
    }
}

#[async_trait::async_trait]
impl ExecutableNode for TaskMemoryWriterNode {
    async fn execute(
        &self,
        inputs: &NodeInputs,
        config: &Value,
        _state: &mut Value,
        _observer: Option<Arc<dyn crate::dag_engine::domain::observer::ExecutionObserver>>,
    ) -> Result<Value, Box<dyn StdError + Send + Sync>> {
        let run_id = _state.get("run_id").and_then(|v| v.as_str()).unwrap_or("unknown_run").to_string();

        if let Some(repo) = &self.task_memory_repo {
            // 1. Update the result of the completed task
            if let Some(task_id_val) = inputs.get("task_id").or_else(|| config.get("task_id")) {
                if let Some(task_id) = task_id_val.as_str() {
                    let result_val = inputs.get("result").or_else(|| config.get("result")).cloned().unwrap_or(Value::Null);
                    repo.update_task_result(task_id, result_val).await?;
                }
            }

            // 2. Process Critic modifications (Add tasks)
            if let Some(add_val) = inputs.get("add_tasks").or_else(|| config.get("add_tasks")) {
                if let Some(add_array) = add_val.as_array() {
                    for task_val in add_array {
                        if let Some(task_obj) = task_val.as_object() {
                            let task_name = task_obj.get("task").and_then(|v| v.as_str()).unwrap_or("Unknown").to_string();
                            let assigned_to = task_obj.get("assigned_to").and_then(|v| v.as_str()).unwrap_or("Unknown").to_string();
                            
                            let new_task = crate::dag_engine::domain::state::DagTask {
                                id: Uuid::new_v4().to_string(),
                                run_id: run_id.clone(),
                                task_name,
                                assigned_to,
                                completed: false,
                                result: None,
                                phase: 1,
                                parallel: false,
                            };
                            repo.add_task(&new_task).await?;
                        }
                    }
                }
            }

            // 3. Process Critic modifications (Delete tasks)
            if let Some(delete_val) = inputs.get("delete_tasks").or_else(|| config.get("delete_tasks")) {
                if let Some(delete_array) = delete_val.as_array() {
                    for id_val in delete_array {
                        if let Some(id_str) = id_val.as_str() {
                            let _ = repo.delete_task(id_str).await;
                        }
                    }
                }
            }

            // 4. Return all current tasks and final loop status
            let mut all_tasks_json = Vec::new();
            if let Ok(tasks) = repo.get_tasks_for_run(&run_id).await {
                for t in tasks {
                    all_tasks_json.push(json!({
                        "id": t.id,
                        "task_name": t.task_name,
                        "assigned_to": t.assigned_to,
                        "completed": t.completed,
                        "result": t.result
                    }));
                }
            }

            // Check if we need to suspend
            let suspend = inputs.get("suspend").and_then(|v| v.as_bool()).unwrap_or(false);
            if suspend {
                return Ok(json!({
                    "__colmena_status": "SUSPENDED",
                    "output": "Suspended by Critic Node",
                    "all_tasks": all_tasks_json
                }));
            }

            Ok(json!({
                "output": {
                    "all_tasks": all_tasks_json,
                }
            }))
        } else {
             Err("TaskMemoryWriterNode requires a Task Memory Repository".into())
        }
    }

    fn description(&self) -> Option<&str> {
        Some("Updates the PostgreSQL Task Memory with task results and applies Critic mutations (add/delete/suspend).")
    }

    fn schema(&self) -> Value {
        json!({
            "type": "task_memory_writer",
            "inputs": {
                "task_id": "The ID of the task to update.",
                "result": "The result object from the worker.",
                "add_tasks": "Array of new task objects to append.",
                "delete_tasks": "Array of task IDs to delete.",
                "suspend": "Boolean to suspend the DAG loop."
            }
        })
    }
}
