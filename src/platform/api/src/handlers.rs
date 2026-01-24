use axum::{
    extract::{State, Json},
    http::StatusCode,
    response::IntoResponse,
};
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;
use chrono::Utc;
use platform_shared::JobRequest;
use std::sync::Arc;
// use tokio::sync::Mutex;

// AppState to hold Redis client
#[derive(Clone)]
pub struct AppState {
    pub redis_client: redis::Client,
}

#[derive(Deserialize)]
pub struct CreateExecutionRequest {
    pub dag_json: Value,
    pub inputs: Value,
}

#[derive(Serialize)]
pub struct CreateExecutionResponse {
    pub job_id: String,
    pub status: String,
}

pub async fn create_execution(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<CreateExecutionRequest>,
) -> impl IntoResponse {
    let job_id = Uuid::new_v4().to_string();
    
    let job = JobRequest {
        job_id: job_id.clone(),
        dag_json: payload.dag_json,
        inputs: payload.inputs,
        created_at: Utc::now().timestamp_millis(),
    };

    let job_json = match serde_json::to_string(&job) {
        Ok(s) => s,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to serialize job: {}", e)).into_response(),
    };

    // Get async connection
    let mut conn = match state.redis_client.get_async_connection().await {
        Ok(c) => c,
        Err(e) => return (StatusCode::SERVICE_UNAVAILABLE, format!("Redis unavailable: {}", e)).into_response(),
    };

    // LPUSH to job_queue
    match conn.lpush::<_, _, ()>("job_queue", job_json).await {
        Ok(_) => {
            tracing::info!("Job {} enqueued successfully", job_id);
            (StatusCode::ACCEPTED, Json(CreateExecutionResponse {
                job_id,
                status: "queued".to_string(),
            })).into_response()
        },
        Err(e) => {
            tracing::error!("Failed to enqueue job {}: {}", job_id, e);
            (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to enqueue job: {}", e)).into_response()
        }
    }
}
