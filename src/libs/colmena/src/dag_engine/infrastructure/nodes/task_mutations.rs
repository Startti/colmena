//! Shared task-memory mutations for the orchestrator critic loop.
//!
//! Both [`ExtractionNode`](super::extraction::ExtractionNode) and
//! [`TaskMemoryWriterNode`](super::task_memory_writer::TaskMemoryWriterNode)
//! apply the same critic modifications (`add_tasks` / `delete_tasks`) and then
//! read back the session's task list. Keeping that logic in one place is what
//! stops a fix from landing in only one of the two copies.

use crate::dag_engine::domain::error::DagError;
use crate::dag_engine::domain::state::{DagTask, DagTaskMemoryRepository};
use serde_json::{json, Value};
use std::sync::Arc;

/// Applies the critic's `add_tasks` and `delete_tasks` mutations to task memory.
///
/// Both arguments are the raw JSON the critic produced; a `None` or a non-array
/// value is a no-op. Infrastructure failures propagate — a task that could not be
/// created or deleted must never be reported as applied.
///
/// One failure is deliberately *not* fatal: a `delete_tasks` entry whose id is
/// malformed ([`DagError::InvalidTaskId`]) is an invented id, not a broken
/// database. Those ids are returned so the caller can report them as skipped.
/// Reporting them is the point — the defect being fixed here was the silence,
/// not the survival.
pub async fn apply_critic_mutations(
    repo: &Arc<dyn DagTaskMemoryRepository>,
    session_id: &str,
    add_tasks: Option<&Value>,
    delete_tasks: Option<&Value>,
) -> Result<Vec<String>, DagError> {
    if let Some(add_array) = add_tasks.and_then(|v| v.as_array()) {
        for task_val in add_array {
            if let Some(task_obj) = task_val.as_object() {
                let task_name = task_obj
                    .get("task")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Unknown")
                    .to_string();
                let assigned_to = task_obj
                    .get("assigned_to")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Unknown")
                    .to_string();

                let new_task = DagTask {
                    id: uuid::Uuid::new_v4().to_string(),
                    session_id: session_id.to_string(),
                    task_name,
                    assigned_to,
                    completed: false,
                    result: None,
                    phase: 1,
                    parallel: false,
                    context: None,
                    is_bridge: false,
                };
                repo.add_task(&new_task).await?;
            }
        }
    }

    let mut skipped_deletes = Vec::new();
    if let Some(delete_array) = delete_tasks.and_then(|v| v.as_array()) {
        for id_val in delete_array {
            if let Some(id_str) = id_val.as_str() {
                match repo.delete_task(id_str).await {
                    Ok(()) => {}
                    Err(DagError::InvalidTaskId(id)) => skipped_deletes.push(id),
                    Err(e) => return Err(e),
                }
            }
        }
    }

    Ok(skipped_deletes)
}

/// Reads back every task of the session, shaped for downstream nodes.
///
/// A read failure propagates: an empty task list is a statement that the session
/// has no work left, and the orchestrator routes on it. It must never stand in
/// for "the database did not answer".
pub async fn fetch_session_tasks(
    repo: &Arc<dyn DagTaskMemoryRepository>,
    session_id: &str,
) -> Result<Vec<Value>, DagError> {
    let tasks = repo.get_tasks_for_run(session_id).await?;
    Ok(tasks
        .into_iter()
        .map(|t| {
            json!({
                "id": t.id,
                "task_name": t.task_name,
                "assigned_to": t.assigned_to,
                "completed": t.completed,
                "result": t.result
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dag_engine::domain::state::DagPhaseSummary;
    use async_trait::async_trait;

    /// A task-memory double whose two interesting calls fail on demand.
    ///
    /// `add_task` always succeeds so each test isolates the call it targets.
    /// `delete_error` distinguishes the two failure classes the helper must tell
    /// apart: an invented id versus a database that is not answering.
    struct StubRepo {
        delete_error: Option<DagError>,
        read_fails: bool,
    }

    impl StubRepo {
        fn failing_delete(e: DagError) -> Arc<dyn DagTaskMemoryRepository> {
            Arc::new(Self {
                delete_error: Some(e),
                read_fails: false,
            })
        }

        fn failing_read() -> Arc<dyn DagTaskMemoryRepository> {
            Arc::new(Self {
                delete_error: None,
                read_fails: true,
            })
        }
    }

    #[async_trait]
    impl DagTaskMemoryRepository for StubRepo {
        async fn add_task(&self, _task: &DagTask) -> Result<(), DagError> {
            Ok(())
        }
        async fn update_task_result(&self, _id: &str, _result: Value) -> Result<(), DagError> {
            Ok(())
        }
        async fn get_tasks_for_run(&self, _session_id: &str) -> Result<Vec<DagTask>, DagError> {
            if self.read_fails {
                return Err(DagError::StateError("connection refused".into()));
            }
            Ok(vec![])
        }
        async fn get_first_uncompleted_task(
            &self,
            _session_id: &str,
        ) -> Result<Option<DagTask>, DagError> {
            Ok(None)
        }
        async fn delete_task(&self, task_id: &str) -> Result<(), DagError> {
            match &self.delete_error {
                Some(DagError::InvalidTaskId(_)) => {
                    Err(DagError::InvalidTaskId(task_id.to_string()))
                }
                Some(DagError::StateError(m)) => Err(DagError::StateError(m.clone())),
                Some(_) | None => Ok(()),
            }
        }
        async fn clear_tasks_for_run(&self, _session_id: &str) -> Result<(), DagError> {
            Ok(())
        }
        async fn get_current_phase(&self, _session_id: &str) -> Result<Option<i32>, DagError> {
            Ok(None)
        }
        async fn get_uncompleted_tasks_for_phase(
            &self,
            _session_id: &str,
            _phase: i32,
        ) -> Result<Vec<DagTask>, DagError> {
            Ok(vec![])
        }
        async fn save_phase_summary(
            &self,
            _session_id: &str,
            _phase: i32,
            _summary: &str,
        ) -> Result<(), DagError> {
            Ok(())
        }
        async fn get_phase_summaries(
            &self,
            _session_id: &str,
        ) -> Result<Vec<DagPhaseSummary>, DagError> {
            Ok(vec![])
        }
    }

    #[tokio::test]
    async fn delete_failure_propagates_instead_of_reporting_success() {
        let repo = StubRepo::failing_delete(DagError::StateError("connection refused".into()));
        let deletes = json!(["task-1"]);

        let result = apply_critic_mutations(&repo, "s1", None, Some(&deletes)).await;

        assert!(
            result.is_err(),
            "a delete that never happened was reported as applied"
        );
    }

    #[tokio::test]
    async fn invalid_task_id_is_reported_as_skipped_not_fatal() {
        let repo = StubRepo::failing_delete(DagError::InvalidTaskId(String::new()));
        let deletes = json!(["not-a-uuid"]);

        let skipped = apply_critic_mutations(&repo, "s1", None, Some(&deletes))
            .await
            .expect("an id the critic invented must not abort the run");

        assert_eq!(
            skipped,
            vec!["not-a-uuid".to_string()],
            "the skipped delete has to be reported, not swallowed"
        );
    }

    #[tokio::test]
    async fn task_list_read_failure_propagates_instead_of_empty_list() {
        let repo = StubRepo::failing_read();

        let result = fetch_session_tasks(&repo, "s1").await;

        assert!(
            result.is_err(),
            "an unreachable database was reported as a session with zero tasks"
        );
    }
}
