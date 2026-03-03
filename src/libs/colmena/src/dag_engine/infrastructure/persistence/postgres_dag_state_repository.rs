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
