//! axum router for the spike. Endpoints:
//!   GET  /                         — static HTML (Univer + y-websocket loader)
//!   GET  /spike.xlsx               — fixture .xlsx (Task 11)
//!   WS   /yjs/:artifact_id         — Yjs sync protocol
//!   GET  /projection/:id.json      — current Yrs → IR projection
//!   POST /spike/agent-op           — in-proc mutation (sanity-check route)

use crate::dag_engine::spike::{doc_registry::DocRegistry, projection, yjs_protocol};
use axum::{
    extract::{ws::WebSocketUpgrade, Path, State},
    http::{header, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::Value;
use std::{path::PathBuf, sync::Arc};
use tokio::fs;

/// Shared server state.
#[derive(Clone)]
pub struct SpikeState {
    pub registry: Arc<DocRegistry>,
    /// Directory for projection dumps. Defaults to `/tmp/spike`.
    pub dump_dir: PathBuf,
}

const INDEX_HTML: &str = include_str!("static/index.html");

pub fn router(state: SpikeState) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/spike.xlsx", get(fixture_xlsx))
        .route("/yjs/:artifact_id", get(ws_handler))
        .route("/projection/:artifact_id.json", get(projection_handler))
        .route("/spike/agent-op", post(agent_op_handler))
        .with_state(state)
}

async fn index() -> Html<&'static str> {
    Html(INDEX_HTML)
}

async fn fixture_xlsx() -> Response {
    let path = std::env::var("COLMENA_SPIKE_FIXTURE_XLSX")
        .unwrap_or_else(|_| "spike/fixtures/test.xlsx".to_string());
    match fs::read(&path).await {
        Ok(bytes) => (
            [(
                header::CONTENT_TYPE,
                "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            )],
            bytes,
        )
            .into_response(),
        Err(e) => (
            StatusCode::NOT_FOUND,
            format!("fixture not found at {path}: {e}"),
        )
            .into_response(),
    }
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    Path(artifact_id): Path<String>,
    State(state): State<SpikeState>,
) -> Response {
    let doc = state.registry.get_or_create(&artifact_id);
    let dump_dir = state.dump_dir.clone();
    // yrs::Subscription (returned by observe_update_v1) holds Arc<dyn Drop>
    // which is !Send.  We work around this by driving the socket on a
    // dedicated thread with its own single-threaded tokio runtime.  This is
    // intentional spike code — production would keep a shared multi-threaded
    // runtime and a Send-safe observer channel.
    ws.on_upgrade(move |socket| async move {
        let (done_tx, done_rx) = tokio::sync::oneshot::channel::<()>();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("ws thread rt");
            rt.block_on(async move {
                if let Err(e) = yjs_protocol::handle_socket(socket, doc.clone()).await {
                    tracing::warn!("ws handler ended with error: {e}");
                }
                dump_projection(&dump_dir, &artifact_id, &doc);
                let _ = done_tx.send(());
            });
        });
        // Await the thread's completion so the tokio task stays alive.
        let _ = done_rx.await;
    })
}

async fn projection_handler(
    Path(artifact_id): Path<String>,
    State(state): State<SpikeState>,
) -> impl IntoResponse {
    // Strip the .json extension from the route param (axum 0.7 captures the
    // entire segment including any dot-suffix, so /projection/abc.json yields
    // artifact_id = "abc.json").
    let id = artifact_id
        .strip_suffix(".json")
        .unwrap_or(&artifact_id)
        .to_string();
    let doc = state.registry.get_or_create(&id);
    Json(projection::project(&doc))
}

#[derive(Deserialize)]
struct AgentOp {
    artifact: String,
    sheet: String,
    addr: String,
    value: Value,
}

async fn agent_op_handler(
    State(state): State<SpikeState>,
    Json(op): Json<AgentOp>,
) -> impl IntoResponse {
    let doc = state.registry.get_or_create(&op.artifact);
    crate::dag_engine::spike::agent_peer::apply_set_cell_in_proc(
        &doc, &op.sheet, &op.addr, &op.value,
    );
    dump_projection(&state.dump_dir, &op.artifact, &doc);
    StatusCode::NO_CONTENT
}

fn dump_projection(dump_dir: &PathBuf, artifact_id: &str, doc: &Arc<yrs::Doc>) {
    if std::fs::create_dir_all(dump_dir).is_err() {
        return;
    }
    let path = dump_dir.join(format!("{artifact_id}.json"));
    let v = projection::project(doc);
    if let Ok(bytes) = serde_json::to_vec_pretty(&v) {
        let _ = std::fs::write(path, bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use axum::http::Request;
    use tower::ServiceExt;

    fn fresh_state() -> SpikeState {
        SpikeState {
            registry: Arc::new(DocRegistry::new()),
            dump_dir: std::env::temp_dir().join("spike_test_dump"),
        }
    }

    #[tokio::test]
    async fn projection_endpoint_returns_empty_workbook_for_new_artifact() {
        let app = router(fresh_state());
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/projection/abc.json")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let body = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
        let v: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v, serde_json::json!({ "sheets": [] }));
    }

    #[tokio::test]
    async fn agent_op_then_projection_reflects_cell() {
        let state = fresh_state();
        let app = router(state.clone());
        let body = serde_json::json!({
            "artifact": "abc", "sheet": "s1", "addr": "A1", "value": "Hola"
        });
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/spike/agent-op")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 204);

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/projection/abc.json")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
        let v: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            v["sheets"][0]["cells"]["A1"],
            serde_json::Value::String("Hola".into())
        );
    }
}
