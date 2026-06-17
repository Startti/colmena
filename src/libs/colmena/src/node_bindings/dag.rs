use napi::bindgen_prelude::*;
use napi_derive::napi;
use serde_json::Value;

// ==================== DAG Engine Bindings ====================

#[napi]
pub async fn run_dag(
    file_path: String,
    resume_id: Option<String>,
    resume_answer: Option<String>,
    inject_payload: Option<Value>,
    include_extra_info: Option<bool>,
    agent_session_id: Option<String>,
) -> Result<Value> {
    let result = crate::dag_engine::api::run_dag(
        file_path,
        resume_id,
        resume_answer,
        inject_payload,
        include_extra_info.unwrap_or(false),
        agent_session_id,
    )
    .await
    .map_err(|e| Error::new(Status::GenericFailure, e.to_string()))?;

    Ok(result)
}

#[napi]
pub async fn serve_dag(file_path: String, host: Option<String>, port: Option<u16>) -> Result<()> {
    let host = host.unwrap_or_else(|| "0.0.0.0".to_string());
    let port = port.unwrap_or(8080);

    crate::dag_engine::api::serve_dag(file_path, host, port)
        .await
        .map_err(|e| Error::new(Status::GenericFailure, e.to_string()))
}
