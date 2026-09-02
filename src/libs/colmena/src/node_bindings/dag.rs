use crate::node_bindings::stream::{DagPartStream, DagStreamHandle};
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

/// Like [`run_dag`] but accepts an already-serialized JSON string of the graph,
/// rather than a file path. Used by the TS facade to support in-memory graph objects.
#[napi]
pub async fn run_dag_from_json(
    graph_json: String,
    resume_id: Option<String>,
    resume_answer: Option<String>,
    inject_payload: Option<Value>,
    include_extra_info: Option<bool>,
    agent_session_id: Option<String>,
) -> Result<Value> {
    let result = crate::dag_engine::api::run_dag_from_str(
        graph_json,
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

/// Validate a graph object; throws `DagError` if it is not a valid graph.
/// Checks that a graph object deserialises into the engine's `Graph`.
///
/// A *shape* check only, and weaker than loading the graph with
/// `dag_engine run`: it does not call `Graph::validate`, so a node id
/// containing `/`, a malformed `node_schema`, an invalid `memory_mode` and a
/// misconfigured `mcp` block all pass here and fail there. It says nothing
/// about the contents of a node's `config` — use `dag_engine lint` for that.
#[napi]
pub fn validate_graph(graph: Value) -> Result<()> {
    let _: crate::dag_engine::domain::graph::Graph = serde_json::from_value(graph)
        .map_err(|e| Error::new(Status::InvalidArg, format!("invalid graph: {}", e)))?;
    Ok(())
}

// ==================== DAG Streaming Bindings ====================

/// Stream a DAG file's execution as SSE-mapped events. Returns a handle whose
/// `pull()` method yields the next `{ type: ... }` event, or `null` at completion.
#[napi]
pub async fn stream_dag(
    file_path: String,
    resume_id: Option<String>,
    resume_answer: Option<String>,
    inject_payload: Option<Value>,
    include_extra_info: Option<bool>,
    agent_session_id: Option<String>,
) -> Result<DagStreamHandle> {
    let extra = include_extra_info.unwrap_or(false);
    let stream = crate::dag_engine::api::stream_dag(
        file_path,
        resume_id,
        resume_answer,
        inject_payload,
        extra,
        agent_session_id,
    )
    .await
    .map_err(|e| Error::new(Status::GenericFailure, e.to_string()))?;
    Ok(DagStreamHandle::new(Box::pin(stream) as DagPartStream))
}

/// Like [`stream_dag`] but accepts an already-serialized JSON string of the graph,
/// rather than a file path. Used by the TS facade to support in-memory graph objects.
#[napi]
pub async fn stream_dag_from_json(
    graph_json: String,
    resume_id: Option<String>,
    resume_answer: Option<String>,
    inject_payload: Option<Value>,
    include_extra_info: Option<bool>,
    agent_session_id: Option<String>,
) -> Result<DagStreamHandle> {
    let extra = include_extra_info.unwrap_or(false);
    let stream = crate::dag_engine::api::stream_dag_from_str(
        graph_json,
        resume_id,
        resume_answer,
        inject_payload,
        extra,
        agent_session_id,
    )
    .await
    .map_err(|e| Error::new(Status::GenericFailure, e.to_string()))?;
    Ok(DagStreamHandle::new(Box::pin(stream) as DagPartStream))
}
