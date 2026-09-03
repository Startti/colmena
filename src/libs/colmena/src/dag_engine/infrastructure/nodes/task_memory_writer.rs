use super::task_mutations;
use crate::dag_engine::domain::lint::{FieldSpec, NodeCatalogEntry};
use crate::dag_engine::domain::node::{ExecutableNode, NodeInputs};
use serde_json::{json, Value};
use std::error::Error as StdError;
use std::sync::Arc;

pub struct TaskMemoryWriterNode {
    task_memory_repo: Option<Arc<dyn crate::dag_engine::domain::state::DagTaskMemoryRepository>>,
}

impl TaskMemoryWriterNode {
    pub fn new(
        task_memory_repo: Option<
            Arc<dyn crate::dag_engine::domain::state::DagTaskMemoryRepository>,
        >,
    ) -> Self {
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
        let session_id = _state
            .get("session_id")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown_run")
            .to_string();

        if let Some(repo) = &self.task_memory_repo {
            // 1. Update the result of the completed task
            if let Some(task_id_val) = inputs.get("task_id").or_else(|| config.get("task_id")) {
                if let Some(task_id) = task_id_val.as_str() {
                    let result_val = inputs
                        .get("result")
                        .or_else(|| config.get("result"))
                        .cloned()
                        .unwrap_or(Value::Null);
                    repo.update_task_result(task_id, result_val).await?;
                }
            }

            // 2-3. Apply the critic's modifications (add + delete). A failure
            // propagates: a task that was not written must never be reported as
            // applied.
            let skipped_deletes = task_mutations::apply_critic_mutations(
                repo,
                &session_id,
                inputs.get("add_tasks").or_else(|| config.get("add_tasks")),
                inputs
                    .get("delete_tasks")
                    .or_else(|| config.get("delete_tasks")),
            )
            .await?;
            if !skipped_deletes.is_empty() {
                tracing::warn!(
                    target: "colmena::task_memory_writer",
                    skipped_deletes = ?skipped_deletes,
                    "critic asked to delete task ids that are not valid identifiers"
                );
            }

            // 4. Return all current tasks and final loop status. This list is the
            // node's `default_output`, so a read failure propagates rather than
            // shipping an empty list the orchestrator would route on.
            let all_tasks_json = task_mutations::fetch_session_tasks(repo, &session_id).await?;

            let suspend = inputs
                .get("suspend")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let mut extra_info = json!({});
            if !skipped_deletes.is_empty() {
                extra_info["skipped_deletes"] = json!(skipped_deletes);
            }

            if suspend {
                extra_info["__colmena_status"] = json!("SUSPENDED");
                extra_info["all_tasks"] = json!(all_tasks_json);
                return Ok(json!({
                    "result": "Suspended by Critic Node",
                    "extra_info": extra_info
                }));
            }

            Ok(json!({
                "result": all_tasks_json,
                "extra_info": extra_info
            }))
        } else {
            Err("TaskMemoryWriterNode requires a Task Memory Repository".into())
        }
    }

    fn description(&self) -> Option<&str> {
        Some("Updates the PostgreSQL Task Memory with task results and applies Critic mutations (add/delete/suspend).")
    }

    fn default_output(&self) -> Option<&str> {
        Some("result")
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
            },
            "outputs": {
                "result": "array — every task of the session after the mutation (also the default_output); the string \"Suspended by Critic Node\" when `suspend` is true",
                "extra_info": "object — empty on the normal path; carries `__colmena_status: \"SUSPENDED\"` and `all_tasks` on suspend, and `skipped_deletes` (array of ids) when a `delete_tasks` id was not a valid identifier"
            }
        })
    }

    fn config_schema(&self) -> Option<NodeCatalogEntry> {
        Some(
            NodeCatalogEntry::no_config()
                .with_field("task_id", FieldSpec::of_type("string"))
                .with_field("result", FieldSpec::of_type("any"))
                .with_field("add_tasks", FieldSpec::of_type("array"))
                .with_field("delete_tasks", FieldSpec::of_type("array"))
                .with_field("suspend", FieldSpec::of_type("boolean")),
        )
    }
}
