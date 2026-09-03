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
/// Checks a graph object the way `dag_engine run` does when it loads a file.
///
/// Deserialises into the engine's `Graph` and then runs `Graph::validate()`, so
/// it rejects a node id containing `/`, a malformed `node_schema`, an invalid
/// `memory_mode` and a misconfigured `mcp` block — the same structural
/// invariants that would otherwise only surface at run time.
///
/// It still says nothing about the contents of a node's `config`: that is an
/// untyped value, so an invented field passes silently here. Use `lintGraph`
/// for that.
#[napi]
pub fn validate_graph(graph: Value) -> Result<()> {
    let g: crate::dag_engine::domain::graph::Graph = serde_json::from_value(graph)
        .map_err(|e| Error::new(Status::InvalidArg, format!("invalid graph: {}", e)))?;
    g.validate()
        .map_err(|e| Error::new(Status::InvalidArg, format!("invalid graph: {}", e)))?;
    Ok(())
}

/// Reports configuration problems in a graph object without running it.
///
/// Where `validateGraph` answers "will the engine load this", this answers the
/// question the author actually has: which of these config fields are real, and
/// which did I invent? Returns an array of findings, each with `severity`,
/// `code`, `nodeId`, `field`, `message` and `suggestion`.
///
/// Findings are advisory — nothing here stops a graph from running. Branch on
/// `code`, which is stable; `message` is free to change.
///
/// Takes the raw object rather than a parsed graph on purpose: deserialising
/// into `Graph` silently drops every undeclared key, so a node carrying
/// `defaultInputPort` is already gone by the time a `Graph` exists.
#[napi]
pub fn lint_graph(graph: Value) -> Result<Value> {
    use crate::dag_engine::domain::lint::{lint_graph_json, LintContext};

    let report = lint_graph_json(&graph, &LintContext::from_catalog())
        .map_err(|e| Error::new(Status::InvalidArg, format!("invalid graph: {}", e)))?;

    let findings: Vec<Value> = report
        .diagnostics
        .iter()
        .map(|d| {
            serde_json::json!({
                "severity": d.severity.to_string(),
                "code": d.code.as_str(),
                "nodeId": d.node_id,
                "field": d.field,
                "message": d.message,
                "suggestion": d.suggestion,
            })
        })
        .collect();
    Ok(Value::Array(findings))
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
