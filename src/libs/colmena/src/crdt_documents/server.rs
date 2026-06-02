//! axum router for the CRDT documents server.
//!
//! Routes (v1):
//!   GET    /                                       — static HTML (Univer)
//!   GET    /minimal                                — diagnostic page (no Univer)
//!   GET    /spike.xlsx                             — fixture .xlsx (legacy from spike; kept for diagnostic page)
//!   WS     /documents/:id/yjs                      — Yjs sync v1 protocol
//!   GET    /documents/:id/projection.json          — current Yrs → IR projection
//!   POST   /documents                              — create a new artifact
//!   GET    /documents                              — list all in-memory artifacts
//!   DELETE /documents/:id                          — delete an artifact (stop writer + remove storage)

use crate::crdt_documents::{
    projection, yjs_protocol, ArtifactId, CrdtDocumentsRuntime,
};
use axum::{
    body::Bytes,
    extract::{ws::WebSocketUpgrade, Path, State},
    extract::Json as JsonExtract,
    http::{header, StatusCode},
    response::{Html, IntoResponse, Json, Response},
    routing::{delete, get, post},
    Router,
};
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use tokio::fs;

pub fn router(runtime: Arc<CrdtDocumentsRuntime>) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/minimal", get(minimal))
        .route("/spike.xlsx", get(fixture_xlsx))
        .route("/documents/:id/yjs", get(ws_handler))
        .route("/documents/:id/projection.json", get(projection_handler))
        .route("/documents", post(create_handler).get(list_handler))
        .route("/documents/:id", delete(delete_handler))
        .route("/documents/:id/import", post(import_handler))
        .with_state(runtime)
}

// ── REST CRUD handlers ────────────────────────────────────────────────────────

#[derive(serde::Deserialize)]
struct CreateRequest {
    name: String,
}

#[derive(serde::Serialize)]
struct CreateResponse {
    artifact_id: ArtifactId,
    created_at: i64,
}

async fn create_handler(
    State(runtime): State<Arc<CrdtDocumentsRuntime>>,
    JsonExtract(req): JsonExtract<CreateRequest>,
) -> impl IntoResponse {
    let id = ArtifactId::new();
    let entry = runtime.registry.get_or_create(&id, &req.name);
    (
        StatusCode::CREATED,
        Json(CreateResponse {
            artifact_id: id,
            created_at: entry.meta.created_at,
        }),
    )
}

#[derive(serde::Serialize)]
struct ListResponse {
    artifacts: Vec<crate::crdt_documents::ArtifactMeta>,
}

async fn list_handler(
    State(runtime): State<Arc<CrdtDocumentsRuntime>>,
) -> impl IntoResponse {
    Json(ListResponse {
        artifacts: runtime.registry.list(),
    })
}

async fn delete_handler(
    Path(id_str): Path<String>,
    State(runtime): State<Arc<CrdtDocumentsRuntime>>,
) -> Response {
    let id = match ArtifactId::from_str(&id_str) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid artifact id").into_response(),
    };
    match runtime.registry.delete(&id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

const INDEX_HTML: &str = include_str!("static/index.html");
const MINIMAL_HTML: &str = include_str!("static/minimal.html");

async fn index() -> Html<&'static str> {
    Html(INDEX_HTML)
}

async fn minimal() -> Html<&'static str> {
    Html(MINIMAL_HTML)
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
    Path(id_str): Path<String>,
    State(runtime): State<Arc<CrdtDocumentsRuntime>>,
) -> Response {
    let id = match ArtifactId::from_str(&id_str) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid artifact id").into_response(),
    };
    // Auto-create on first WS hit: convenient for the demo HTML (user pastes a
    // new URL → doc appears). Task 9 (POST /documents) will be the canonical
    // create path; this auto-create remains the demo-ergonomic shortcut.
    let entry = runtime.registry.get_or_create(&id, "(untitled)");
    let doc = entry.doc.clone();
    let dirty = entry.dirty.clone();
    let notify = entry.notify.clone();
    ws.on_upgrade(move |socket| async move {
        // yrs::Subscription (Arc<dyn Drop>) is !Send. Drive the socket on a
        // dedicated thread with its own single-threaded tokio runtime so we
        // don't require Send on the future.
        let (done_tx, done_rx) = tokio::sync::oneshot::channel::<()>();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("ws thread rt");
            rt.block_on(async move {
                let post_update = move |update_bytes: &[u8]| {
                    // TODO(Task 17 — ChangeTracker): pass update_bytes to
                    // runtime.tracker.record(&id, "peer:browser", narrate(...)) so the
                    // LLM's get_recent_changes tool sees per-update summaries.
                    let _ = update_bytes; // silence unused-variable until Task 17
                    dirty.store(true, Ordering::Release);
                    notify.notify_one();
                };
                if let Err(e) =
                    yjs_protocol::handle_socket(socket, doc, Some(post_update)).await
                {
                    tracing::warn!("ws handler ended with error: {e}");
                }
                let _ = done_tx.send(());
            });
        });
        // Keep the tokio task alive until the thread finishes.
        let _ = done_rx.await;
    })
}

async fn projection_handler(
    Path(id_with_suffix): Path<String>,
    State(runtime): State<Arc<CrdtDocumentsRuntime>>,
) -> Response {
    // axum 0.7 captures `:id.json` as the full segment; strip the suffix.
    let id_str = id_with_suffix
        .strip_suffix(".json")
        .unwrap_or(&id_with_suffix);
    let id = match ArtifactId::from_str(id_str) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid artifact id").into_response(),
    };
    match runtime.registry.get(&id) {
        Some(entry) => Json(projection::project(&entry.doc)).into_response(),
        None => (StatusCode::NOT_FOUND, "artifact not found").into_response(),
    }
}

// ── Import handler ────────────────────────────────────────────────────────────

#[derive(serde::Serialize)]
struct ImportResponse {
    sheets_imported: u32,
    cells_imported: u64,
}

async fn import_handler(
    Path(id_str): Path<String>,
    State(runtime): State<Arc<CrdtDocumentsRuntime>>,
    body: Bytes,
) -> Response {
    let id = match ArtifactId::from_str(&id_str) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid artifact id").into_response(),
    };
    let entry = runtime.registry.get_or_create(&id, "(imported)");
    match crate::crdt_documents::xlsx_import::import_xlsx_into_doc(&entry.doc, &body) {
        Ok(stats) => {
            // Tell the snapshot writer to flush soon.
            entry.dirty.store(true, Ordering::Release);
            entry.notify.notify_one();
            Json(ImportResponse {
                sheets_imported: stats.sheets_imported,
                cells_imported: stats.cells_imported,
            })
            .into_response()
        }
        Err(e) => (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use axum::http::Request;
    use serde_json::json;
    use tower::ServiceExt;

    async fn fresh_runtime() -> (Arc<CrdtDocumentsRuntime>, std::path::PathBuf) {
        let tmp = std::env::temp_dir().join(format!("srv_test_{}", ulid::Ulid::new()));
        let cfg = json!({ "storage_root": tmp.to_str().unwrap() });
        let rt = Arc::new(CrdtDocumentsRuntime::from_config(&cfg).await.unwrap());
        (rt, tmp)
    }

    #[tokio::test]
    async fn projection_returns_404_for_unknown_artifact() {
        let (rt, tmp) = fresh_runtime().await;
        let app = router(rt);
        let id = ArtifactId::new();
        let resp = app
            .oneshot(
                Request::builder()
                    .uri(format!("/documents/{}/projection.json", id))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn projection_returns_empty_for_registered_artifact() {
        let (rt, tmp) = fresh_runtime().await;
        let id = ArtifactId::new();
        let _ = rt.registry.get_or_create(&id, "test");
        let app = router(rt);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri(format!("/documents/{}/projection.json", id))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v, json!({ "sheets": [] }));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn projection_rejects_invalid_id() {
        let (rt, tmp) = fresh_runtime().await;
        let app = router(rt);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/documents/not-an-id/projection.json")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
