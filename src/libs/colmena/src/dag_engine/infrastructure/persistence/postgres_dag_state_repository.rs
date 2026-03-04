use crate::dag_engine::domain::error::DagError;
use crate::dag_engine::domain::state::{DagRunState, DagRunStatus, DagStateRepository};
use async_trait::async_trait;
use sqlx::{PgPool, Row};
use std::collections::HashMap;

pub struct PostgresDagStateRepository {
    pool: PgPool,
}

impl PostgresDagStateRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl DagStateRepository for PostgresDagStateRepository {
    async fn get_by_id(&self, run_id: &str) -> Result<Option<DagRunState>, DagError> {
        let row_opt = sqlx::query(
            "SELECT run_id, graph_json, all_outputs, status FROM dag_runs WHERE run_id = $1"
        )
        .bind(run_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| DagError::StateError(format!("Database error on get: {}", e)))?;

        match row_opt {
            Some(row) => {
                let status_str: String = row.get("status");
                let status = status_str.parse::<DagRunStatus>().unwrap_or(DagRunStatus::Failed);
                
                let all_outputs_json: serde_json::Value = row.get("all_outputs");
                let all_outputs = if let Some(obj) = all_outputs_json.as_object() {
                    obj.into_iter()
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect()
                } else {
                    HashMap::new()
                };

                let state = DagRunState {
                    run_id: row.get("run_id"),
                    graph_json: row.get("graph_json"),
                    all_outputs,
                    status,
                };
                Ok(Some(state))
            }
            None => Ok(None),
        }
    }

    async fn save(&self, state: &DagRunState) -> Result<(), DagError> {
        let status_str = state.status.to_string();
        
        // Serde cannot automatically serialize HashMap<String, Value> to JSONB for sqlx directly without wrapper
        // So we explicitly convert it to Value first
        let all_outputs_json = match serde_json::to_value(&state.all_outputs) {
            Ok(v) => v,
            Err(e) => return Err(DagError::StateError(format!("Serialization error: {}", e))),
        };

        sqlx::query(
            r#"
            INSERT INTO dag_runs (run_id, graph_json, all_outputs, status, updated_at)
            VALUES ($1, $2, $3, $4, NOW())
            ON CONFLICT (run_id) DO UPDATE SET
            graph_json = EXCLUDED.graph_json,
            all_outputs = EXCLUDED.all_outputs,
            status = EXCLUDED.status,
            updated_at = NOW()
            "#
        )
        .bind(&state.run_id)
        .bind(&state.graph_json)
        .bind(&all_outputs_json)
        .bind(&status_str)
        .execute(&self.pool)
        .await
        .map_err(|e| DagError::StateError(format!("Database error on save: {}", e)))?;

        Ok(())
    }
}

#[async_trait]
impl crate::dag_engine::domain::state::DagTaskMemoryRepository for PostgresDagStateRepository {
    async fn add_task(&self, task: &crate::dag_engine::domain::state::DagTask) -> Result<(), DagError> {
        let id_uuid = uuid::Uuid::parse_str(&task.id).unwrap_or_else(|_| uuid::Uuid::new_v4());
        let result_json = task.result.as_ref().map(|v| serde_json::to_value(v).unwrap_or(serde_json::Value::Null));

        sqlx::query(
            "INSERT INTO dag_task_memory (id, run_id, task_name, assigned_to, completed, result) VALUES ($1, $2, $3, $4, $5, $6)"
        )
        .bind(id_uuid)
        .bind(&task.run_id)
        .bind(&task.task_name)
        .bind(&task.assigned_to)
        .bind(task.completed)
        .bind(result_json)
        .execute(&self.pool)
        .await
        .map_err(|e| DagError::StateError(format!("Database error on add_task: {}", e)))?;

        Ok(())
    }

    async fn update_task_result(&self, task_id: &str, result: serde_json::Value) -> Result<(), DagError> {
        let id_uuid = uuid::Uuid::parse_str(task_id).map_err(|_| DagError::StateError("Invalid UUID for task_id".to_string()))?;
        sqlx::query(
            "UPDATE dag_task_memory SET completed = TRUE, result = $1, updated_at = NOW() WHERE id = $2"
        )
        .bind(result)
        .bind(id_uuid)
        .execute(&self.pool)
        .await
        .map_err(|e| DagError::StateError(format!("Database error on update_task_result: {}", e)))?;

        Ok(())
    }

    async fn get_tasks_for_run(&self, run_id: &str) -> Result<Vec<crate::dag_engine::domain::state::DagTask>, DagError> {
        let rows = sqlx::query(
            "SELECT id, run_id, task_name, assigned_to, completed, result FROM dag_task_memory WHERE run_id = $1 ORDER BY created_at ASC"
        )
        .bind(run_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| DagError::StateError(format!("Database error on get_tasks_for_run: {}", e)))?;

        let mut tasks = Vec::new();
        for row in rows {
            let id_uuid: uuid::Uuid = row.get("id");
            let result_val: Option<serde_json::Value> = row.get("result");

            tasks.push(crate::dag_engine::domain::state::DagTask {
                id: id_uuid.to_string(),
                run_id: row.get("run_id"),
                task_name: row.get("task_name"),
                assigned_to: row.get("assigned_to"),
                completed: row.get("completed"),
                result: result_val,
            });
        }
        Ok(tasks)
    }

    async fn get_first_uncompleted_task(&self, run_id: &str) -> Result<Option<crate::dag_engine::domain::state::DagTask>, DagError> {
        let row_opt = sqlx::query(
            "SELECT id, run_id, task_name, assigned_to, completed, result FROM dag_task_memory WHERE run_id = $1 AND completed = FALSE ORDER BY created_at ASC LIMIT 1"
        )
        .bind(run_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| DagError::StateError(format!("Database error on get_first_uncompleted_task: {}", e)))?;

        match row_opt {
            Some(row) => {
                let id_uuid: uuid::Uuid = row.get("id");
                let result_val: Option<serde_json::Value> = row.get("result");
                Ok(Some(crate::dag_engine::domain::state::DagTask {
                    id: id_uuid.to_string(),
                    run_id: row.get("run_id"),
                    task_name: row.get("task_name"),
                    assigned_to: row.get("assigned_to"),
                    completed: row.get("completed"),
                    result: result_val,
                }))
            },
            None => Ok(None)
        }
    }

    async fn delete_task(&self, task_id: &str) -> Result<(), DagError> {
        let id_uuid = uuid::Uuid::parse_str(task_id).map_err(|_| DagError::StateError("Invalid UUID for task_id".to_string()))?;
        sqlx::query("DELETE FROM dag_task_memory WHERE id = $1")
            .bind(id_uuid)
            .execute(&self.pool)
            .await
            .map_err(|e| DagError::StateError(format!("Database error on delete_task: {}", e)))?;
        Ok(())
    }

    async fn clear_tasks_for_run(&self, run_id: &str) -> Result<(), DagError> {
        sqlx::query("DELETE FROM dag_task_memory WHERE run_id = $1")
            .bind(run_id)
            .execute(&self.pool)
            .await
            .map_err(|e| DagError::StateError(format!("Database error on clear_tasks_for_run: {}", e)))?;
        Ok(())
    }
}
