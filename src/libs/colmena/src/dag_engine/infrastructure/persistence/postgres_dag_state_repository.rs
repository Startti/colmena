use crate::dag_engine::domain::error::DagError;
use crate::dag_engine::domain::state::{DagRunState, DagRunStatus, DagStateRepository, DagTask, DagPhaseSummary};
use async_trait::async_trait;
use serde_json::Value;
use sqlx::{PgPool, Row};
use std::collections::HashMap;

pub struct PostgresDagStateRepository {
    pool: PgPool,
}

impl PostgresDagStateRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Apply schema migrations at startup (idempotent).
    pub async fn migrate(&self) -> Result<(), DagError> {
        // Ensure dag_runs table has full state columns
        let migration_queries = vec![
            "ALTER TABLE dag_runs ADD COLUMN IF NOT EXISTS active_queue JSONB NOT NULL DEFAULT '[]'::jsonb",
            "ALTER TABLE dag_runs ADD COLUMN IF NOT EXISTS execution_history JSONB NOT NULL DEFAULT '[]'::jsonb",
            "ALTER TABLE dag_runs ADD COLUMN IF NOT EXISTS global_calls JSONB NOT NULL DEFAULT '{}'::jsonb",
            "ALTER TABLE dag_runs ADD COLUMN IF NOT EXISTS caller_specific_calls JSONB NOT NULL DEFAULT '{}'::jsonb",
            "ALTER TABLE dag_runs ADD COLUMN IF NOT EXISTS global_shared_state JSONB NOT NULL DEFAULT '{}'::jsonb",
        ];

        for query in migration_queries {
            sqlx::query(query)
                .execute(&self.pool)
                .await
                .map_err(|e| DagError::StateError(format!("Migration error: {}", e)))?;
        }

        // Ensure dag_task_memory has phase + parallel columns
        sqlx::query(
            "ALTER TABLE dag_task_memory ADD COLUMN IF NOT EXISTS phase INT NOT NULL DEFAULT 1"
        )
        .execute(&self.pool)
        .await
        .map_err(|e| DagError::StateError(format!("Migration error (phase col): {}", e)))?;

        sqlx::query(
            "ALTER TABLE dag_task_memory ADD COLUMN IF NOT EXISTS parallel BOOLEAN NOT NULL DEFAULT FALSE"
        )
        .execute(&self.pool)
        .await
        .map_err(|e| DagError::StateError(format!("Migration error (parallel col): {}", e)))?;

        // Create dag_phase_summaries table
        sqlx::query(
            r#"CREATE TABLE IF NOT EXISTS dag_phase_summaries (
                id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
                session_id TEXT NOT NULL,
                phase      INT  NOT NULL,
                summary    TEXT NOT NULL,
                created_at TIMESTAMPTZ DEFAULT NOW()
            )"#
        )
        .execute(&self.pool)
        .await
        .map_err(|e| DagError::StateError(format!("Migration error (dag_phase_summaries): {}", e)))?;

        Ok(())
    }
}

// ─── Helper: build DagTask from a sqlx Row ────────────────────────────────────
fn row_to_task(row: &sqlx::postgres::PgRow) -> DagTask {
    let id_uuid: uuid::Uuid = row.get("id");
    let result_val: Option<serde_json::Value> = row.get("result");
    let phase: i32 = row.try_get("phase").unwrap_or(1);
    let parallel: bool = row.try_get("parallel").unwrap_or(false);

    DagTask {
        id: id_uuid.to_string(),
        session_id: row.get("session_id"),
        task_name: row.get("task_name"),
        assigned_to: row.get("assigned_to"),
        completed: row.get("completed"),
        result: result_val,
        phase,
        parallel,
    }
}

// ─── DagStateRepository ───────────────────────────────────────────────────────

#[async_trait]
impl DagStateRepository for PostgresDagStateRepository {
    async fn get_by_id(&self, session_id: &str) -> Result<Option<DagRunState>, DagError> {
        let row_opt = sqlx::query(
            "SELECT session_id, graph_json, all_outputs, status, active_queue, execution_history, global_calls, caller_specific_calls, global_shared_state FROM dag_runs WHERE session_id = $1"
        )
        .bind(session_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| DagError::StateError(format!("Database error on get: {}", e)))?;

        match row_opt {
            Some(row) => {
                let status_str: String = row.get("status");
                let status = status_str.parse::<DagRunStatus>().unwrap_or(DagRunStatus::Failed);

                let all_outputs_json: serde_json::Value = row.get("all_outputs");
                let all_outputs: HashMap<String, Value> = serde_json::from_value(all_outputs_json)
                    .unwrap_or_default();

                let active_queue_json: serde_json::Value = row.get("active_queue");
                let active_queue = serde_json::from_value(active_queue_json).unwrap_or_default();

                let execution_history_json: serde_json::Value = row.get("execution_history");
                let execution_history = serde_json::from_value(execution_history_json).unwrap_or_default();

                let global_calls_json: serde_json::Value = row.get("global_calls");
                let global_calls = serde_json::from_value(global_calls_json).unwrap_or_default();

                let caller_specific_calls_json: serde_json::Value = row.get("caller_specific_calls");
                let caller_specific_calls = serde_json::from_value(caller_specific_calls_json).unwrap_or_default();

                let global_shared_state: serde_json::Value = row.get("global_shared_state");

                Ok(Some(DagRunState {
                    session_id: row.get("session_id"),
                    graph_json: row.get("graph_json"),
                    all_outputs,
                    status,
                    global_shared_state,
                    active_queue,
                    execution_history,
                    global_calls,
                    caller_specific_calls,
                }))
            }
            None => Ok(None),
        }
    }

    async fn save(&self, state: &DagRunState) -> Result<(), DagError> {
        let status_str = state.status.to_string();
        let all_outputs_json = serde_json::to_value(&state.all_outputs)
            .map_err(|e| DagError::StateError(format!("Serialization error: {}", e)))?;
        
        let active_queue_json = serde_json::to_value(&state.active_queue)
            .map_err(|e| DagError::StateError(format!("Serialization error (queue): {}", e)))?;
            
        let execution_history_json = serde_json::to_value(&state.execution_history)
            .map_err(|e| DagError::StateError(format!("Serialization error (history): {}", e)))?;
            
        let global_calls_json = serde_json::to_value(&state.global_calls)
            .map_err(|e| DagError::StateError(format!("Serialization error (global_calls): {}", e)))?;
            
        let caller_specific_calls_json = serde_json::to_value(&state.caller_specific_calls)
            .map_err(|e| DagError::StateError(format!("Serialization error (caller_calls): {}", e)))?;

        sqlx::query(
            r#"INSERT INTO dag_runs (session_id, graph_json, all_outputs, status, active_queue, execution_history, global_calls, caller_specific_calls, global_shared_state, updated_at)
               VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, NOW())
               ON CONFLICT (session_id) DO UPDATE SET
                 graph_json = EXCLUDED.graph_json,
                 all_outputs = EXCLUDED.all_outputs,
                 status = EXCLUDED.status,
                 active_queue = EXCLUDED.active_queue,
                 execution_history = EXCLUDED.execution_history,
                 global_calls = EXCLUDED.global_calls,
                 caller_specific_calls = EXCLUDED.caller_specific_calls,
                 global_shared_state = EXCLUDED.global_shared_state,
                 updated_at = NOW()"#
        )
        .bind(&state.session_id)
        .bind(&state.graph_json)
        .bind(&all_outputs_json)
        .bind(&status_str)
        .bind(&active_queue_json)
        .bind(&execution_history_json)
        .bind(&global_calls_json)
        .bind(&caller_specific_calls_json)
        .bind(&state.global_shared_state)
        .execute(&self.pool)
        .await
        .map_err(|e| DagError::StateError(format!("Database error on save: {}", e)))?;

        Ok(())
    }
}

// ─── DagTaskMemoryRepository ──────────────────────────────────────────────────

#[async_trait]
impl crate::dag_engine::domain::state::DagTaskMemoryRepository for PostgresDagStateRepository {

    async fn add_task(&self, task: &DagTask) -> Result<(), DagError> {
        let id_uuid = uuid::Uuid::parse_str(&task.id).unwrap_or_else(|_| uuid::Uuid::new_v4());
        let result_json = task.result.as_ref()
            .map(|v| serde_json::to_value(v).unwrap_or(serde_json::Value::Null));

        sqlx::query(
            "INSERT INTO dag_task_memory (id, session_id, task_name, assigned_to, completed, result, phase, parallel) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)"
        )
        .bind(id_uuid)
        .bind(&task.session_id)
        .bind(&task.task_name)
        .bind(&task.assigned_to)
        .bind(task.completed)
        .bind(result_json)
        .bind(task.phase)
        .bind(task.parallel)
        .execute(&self.pool)
        .await
        .map_err(|e| DagError::StateError(format!("Database error on add_task: {}", e)))?;

        Ok(())
    }

    async fn update_task_result(&self, task_id: &str, result: serde_json::Value) -> Result<(), DagError> {
        let id_uuid = uuid::Uuid::parse_str(task_id)
            .map_err(|_| DagError::StateError("Invalid UUID for task_id".to_string()))?;

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

    async fn get_tasks_for_run(&self, session_id: &str) -> Result<Vec<DagTask>, DagError> {
        let rows = sqlx::query(
            "SELECT id, session_id, task_name, assigned_to, completed, result, phase, parallel \
             FROM dag_task_memory WHERE session_id = $1 ORDER BY phase ASC, created_at ASC"
        )
        .bind(session_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| DagError::StateError(format!("Database error on get_tasks_for_run: {}", e)))?;

        Ok(rows.iter().map(row_to_task).collect())
    }

    async fn get_first_uncompleted_task(&self, session_id: &str) -> Result<Option<DagTask>, DagError> {
        let row_opt = sqlx::query(
            "SELECT id, session_id, task_name, assigned_to, completed, result, phase, parallel \
             FROM dag_task_memory WHERE session_id = $1 AND completed = FALSE \
             ORDER BY phase ASC, created_at ASC LIMIT 1"
        )
        .bind(session_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| DagError::StateError(format!("Database error on get_first_uncompleted_task: {}", e)))?;

        Ok(row_opt.as_ref().map(row_to_task))
    }

    async fn delete_task(&self, task_id: &str) -> Result<(), DagError> {
        let id_uuid = uuid::Uuid::parse_str(task_id)
            .map_err(|_| DagError::StateError("Invalid UUID for task_id".to_string()))?;

        sqlx::query("DELETE FROM dag_task_memory WHERE id = $1")
            .bind(id_uuid)
            .execute(&self.pool)
            .await
            .map_err(|e| DagError::StateError(format!("Database error on delete_task: {}", e)))?;

        Ok(())
    }

    async fn clear_tasks_for_run(&self, session_id: &str) -> Result<(), DagError> {
        sqlx::query("DELETE FROM dag_task_memory WHERE session_id = $1")
            .bind(session_id)
            .execute(&self.pool)
            .await
            .map_err(|e| DagError::StateError(format!("Database error on clear_tasks_for_run: {}", e)))?;

        Ok(())
    }

    // ── Phase-aware routing ────────────────────────────────────────────────────

    async fn get_current_phase(&self, session_id: &str) -> Result<Option<i32>, DagError> {
        // MIN() aggregate always returns exactly one row (NULL if no matching rows)
        let row = sqlx::query(
            "SELECT MIN(phase) as min_phase FROM dag_task_memory \
             WHERE session_id = $1 AND completed = FALSE"
        )
        .bind(session_id)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| DagError::StateError(format!("Database error on get_current_phase: {}", e)))?;

        // MIN returns NULL if no rows matched → map to None
        let min_phase: Option<i32> = row.try_get("min_phase").unwrap_or(None);
        Ok(min_phase)
    }

    async fn get_uncompleted_tasks_for_phase(&self, session_id: &str, phase: i32) -> Result<Vec<DagTask>, DagError> {
        let rows = sqlx::query(
            "SELECT id, session_id, task_name, assigned_to, completed, result, phase, parallel \
             FROM dag_task_memory WHERE session_id = $1 AND phase = $2 AND completed = FALSE \
             ORDER BY created_at ASC"
        )
        .bind(session_id)
        .bind(phase)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| DagError::StateError(format!("Database error on get_uncompleted_tasks_for_phase: {}", e)))?;

        Ok(rows.iter().map(row_to_task).collect())
    }

    // ── Phase summaries ────────────────────────────────────────────────────────

    async fn save_phase_summary(&self, session_id: &str, phase: i32, summary: &str) -> Result<(), DagError> {
        sqlx::query(
            "INSERT INTO dag_phase_summaries (session_id, phase, summary) VALUES ($1, $2, $3)"
        )
        .bind(session_id)
        .bind(phase)
        .bind(summary)
        .execute(&self.pool)
        .await
        .map_err(|e| DagError::StateError(format!("Database error on save_phase_summary: {}", e)))?;

        Ok(())
    }

    async fn get_phase_summaries(&self, session_id: &str) -> Result<Vec<DagPhaseSummary>, DagError> {
        let rows = sqlx::query(
            "SELECT session_id, phase, summary FROM dag_phase_summaries \
             WHERE session_id = $1 ORDER BY phase ASC"
        )
        .bind(session_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| DagError::StateError(format!("Database error on get_phase_summaries: {}", e)))?;

        Ok(rows.iter().map(|row| DagPhaseSummary {
            session_id: row.get("session_id"),
            phase: row.get("phase"),
            summary: row.get("summary"),
        }).collect())
    }
}
