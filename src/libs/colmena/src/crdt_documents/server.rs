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
    change_tracker_store::NewEvent, projection, yjs_protocol, ArtifactId, CrdtDocumentsRuntime,
};
use axum::{
    body::Bytes,
    extract::Json as JsonExtract,
    extract::{ws::WebSocketUpgrade, Path, Query, State},
    http::{header, StatusCode},
    response::{Html, IntoResponse, Json, Response},
    routing::{delete, get, post},
    Router,
};
use std::str::FromStr;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tokio::fs;

pub fn router(runtime: Arc<CrdtDocumentsRuntime>) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/minimal", get(minimal))
        .route("/spike.xlsx", get(fixture_xlsx))
        .route("/documents/:id/yjs", get(ws_handler))
        // Alias: y-websocket's WebsocketProvider constructs URLs as
        // `${serverUrl}/${encodeURIComponent(roomname)}`. With the canonical
        // `/documents/:id/yjs` path, the demo HTMLs would either need to
        // hard-code an artifact-bearing URL with an empty roomname (causes
        // trailing-slash mismatch on axum) or use URL-encoded slashes (which
        // axum's matchit router rejects). The simpler escape is to expose a
        // y-websocket-friendly `/yjs/:id` alias that demos can keep using
        // without rewiring the JS. ADP integration uses the canonical path.
        .route("/yjs/:id", get(ws_handler))
        .route("/documents/:id/projection.json", get(projection_handler))
        .route("/documents", post(create_handler).get(list_handler))
        .route("/documents/:id", delete(delete_handler))
        .route("/documents/:id/import", post(import_handler))
        .route("/documents/:id/export.xlsx", get(export_handler))
        .route("/documents/:id/changes", get(changes_handler))
        .route("/documents/:id/events", post(record_event_handler))
        .route(
            "/documents/:id/cursor",
            get(get_cursor_handler).post(set_cursor_handler),
        )
        .route("/documents/by-session/:sid", get(by_session_handler))
        .with_state(runtime)
}

// ── REST CRUD handlers ────────────────────────────────────────────────────────

#[derive(serde::Deserialize)]
struct CreateRequest {
    name: String,
    /// Optional agent session id. When present, the new artifact is also
    /// registered in the change-tracker store via `touch_artifact` so that
    /// `/documents/by-session/:sid` can surface it.
    #[serde(default)]
    agent_session_id: Option<String>,
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
    if let Some(sid) = req.agent_session_id.as_deref() {
        // Best-effort: a store-write failure should not fail artifact creation.
        let _ = runtime
            .store
            .touch_artifact(sid, &id, Some(req.name.as_str()))
            .await;
    }
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

async fn list_handler(State(runtime): State<Arc<CrdtDocumentsRuntime>>) -> impl IntoResponse {
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

// ── Change-tracker REST handlers (B-T7) ───────────────────────────────────

async fn changes_handler(
    Path(id_str): Path<String>,
    Query(params): Query<std::collections::HashMap<String, String>>,
    State(runtime): State<Arc<CrdtDocumentsRuntime>>,
) -> Response {
    let id = match ArtifactId::from_str(&id_str) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid artifact id").into_response(),
    };
    let since: u64 = params
        .get("since")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let limit: u32 = params
        .get("limit")
        .and_then(|s| s.parse().ok())
        .unwrap_or(50);
    let sheet_id = params.get("sheet_id").map(String::as_str);
    let exclude_origin = params.get("exclude_origin").map(String::as_str);
    match runtime
        .store
        .events_since(&id, since, sheet_id, exclude_origin, limit)
        .await
    {
        Ok(evs) => {
            let max_id = evs.iter().map(|e| e.id).max().unwrap_or(since);
            let truncated = (evs.len() as u32) >= limit;
            Json(serde_json::json!({
                "current_event_id": max_id,
                "events": evs.iter().map(|e| serde_json::json!({
                    "id": e.id,
                    "artifact_id": e.artifact_id,
                    "sheet_id": e.sheet_id,
                    "origin": e.origin,
                    "summary": e.summary,
                    "created_at": e.created_at,
                })).collect::<Vec<_>>(),
                "truncated": truncated,
            }))
            .into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")).into_response(),
    }
}

#[derive(serde::Deserialize)]
struct RecordEventBody {
    #[serde(default)]
    sheet_id: Option<String>,
    origin: String,
    summary: String,
}

async fn record_event_handler(
    Path(id_str): Path<String>,
    State(runtime): State<Arc<CrdtDocumentsRuntime>>,
    JsonExtract(body): JsonExtract<RecordEventBody>,
) -> Response {
    let id = match ArtifactId::from_str(&id_str) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid artifact id").into_response(),
    };
    match runtime
        .store
        .insert_event(NewEvent {
            artifact_id: id,
            sheet_id: body.sheet_id,
            origin: body.origin,
            summary: body.summary,
        })
        .await
    {
        Ok(event_id) => Json(serde_json::json!({ "id": event_id })).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")).into_response(),
    }
}

#[derive(serde::Deserialize)]
struct CursorBody {
    agent_session_id: String,
    last_event_id: u64,
}

async fn set_cursor_handler(
    Path(id_str): Path<String>,
    State(runtime): State<Arc<CrdtDocumentsRuntime>>,
    JsonExtract(body): JsonExtract<CursorBody>,
) -> Response {
    let id = match ArtifactId::from_str(&id_str) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid artifact id").into_response(),
    };
    match runtime
        .store
        .upsert_cursor(&body.agent_session_id, &id, body.last_event_id)
        .await
    {
        Ok(()) => StatusCode::OK.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")).into_response(),
    }
}

async fn get_cursor_handler(
    Path(id_str): Path<String>,
    Query(params): Query<std::collections::HashMap<String, String>>,
    State(runtime): State<Arc<CrdtDocumentsRuntime>>,
) -> Response {
    let id = match ArtifactId::from_str(&id_str) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid artifact id").into_response(),
    };
    let sid = match params.get("agent_session_id") {
        Some(s) => s,
        None => {
            return (StatusCode::BAD_REQUEST, "agent_session_id required").into_response();
        }
    };
    match runtime.store.cursor_for(sid, &id).await {
        Ok(Some(c)) => Json(serde_json::json!({ "last_event_id": c })).into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, "no cursor").into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")).into_response(),
    }
}

async fn by_session_handler(
    Path(sid): Path<String>,
    Query(params): Query<std::collections::HashMap<String, String>>,
    State(runtime): State<Arc<CrdtDocumentsRuntime>>,
) -> Response {
    let limit: u32 = params
        .get("limit")
        .and_then(|s| s.parse().ok())
        .unwrap_or(50);
    match runtime.store.artifacts_for_session(&sid, limit).await {
        Ok(list) => Json(serde_json::json!({
            "artifacts": list.iter().map(|a| serde_json::json!({
                "artifact_id": a.artifact_id,
                "name": a.name,
                "created_at": a.created_at,
                "last_accessed_at": a.last_accessed_at,
            })).collect::<Vec<_>>(),
        }))
        .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")).into_response(),
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
    Query(params): Query<std::collections::HashMap<String, String>>,
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
    let tracker = runtime.tracker.clone();
    let id_for_cb = id.clone();
    // B-T13: capture peer attribution from URL query params so the server
    // can distinguish agent vs browser updates (and which agent session).
    // Defaulting to `"browser"` keeps demo HTMLs (which don't pass query
    // params) working unchanged.
    let peer_type = params
        .get("peer_type")
        .cloned()
        .unwrap_or_else(|| "browser".to_string());
    let session_id_opt = params.get("session_id").cloned();
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
                    dirty.store(true, Ordering::Release);
                    notify.notify_one();
                    // v1: coarse summary. narrate() would need the pre-update doc state to
                    // produce a per-cell diff; handle_socket currently calls post_update
                    // AFTER apply_update so we only have the post-state here. v1.1 should
                    // either capture state before apply in handle_socket and pass it in,
                    // or refactor handle_socket to invoke post_update with a pre-state
                    // clone.
                    let summary = format!("peer update ({} bytes)", update_bytes.len());
                    // B-T13: derive origin from the peer_type + session_id
                    // captured at WS upgrade. Agent peers get
                    // `agent:<session_id>` (or `agent:anonymous` if no
                    // session_id was provided); everything else falls back
                    // to the legacy `peer:browser` label.
                    let origin = match peer_type.as_str() {
                        "agent" => session_id_opt
                            .as_deref()
                            .map(|s| format!("agent:{s}"))
                            .unwrap_or_else(|| "agent:anonymous".to_string()),
                        _ => "peer:browser".to_string(),
                    };
                    // ChangeTracker is async since B-T4. This callback runs in
                    // a sync context (inside `handle_socket`'s update-observer
                    // closure on a single-thread tokio runtime). Spawn a
                    // fire-and-forget task so we don't block the WS reader.
                    // Tradeoff: the tracker is best-effort, and under high
                    // load an event could land out-of-order or be lost on
                    // runtime tear-down. Acceptable for v1 — the change
                    // narration is informational, not load-bearing.
                    let tracker = tracker.clone();
                    let id_for_cb = id_for_cb.clone();
                    tokio::spawn(async move {
                        tracker.record(&id_for_cb, None, &origin, &summary).await;
                    });
                };
                if let Err(e) = yjs_protocol::handle_socket(socket, doc, Some(post_update)).await {
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

// ── Export handler ────────────────────────────────────────────────────────────

async fn export_handler(
    Path(id_str): Path<String>,
    State(runtime): State<Arc<CrdtDocumentsRuntime>>,
) -> Response {
    let id_str = id_str.strip_suffix(".xlsx").unwrap_or(&id_str);
    let id = match ArtifactId::from_str(id_str) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid artifact id").into_response(),
    };
    let Some(entry) = runtime.registry.get(&id) else {
        return (StatusCode::NOT_FOUND, "artifact not found").into_response();
    };
    match crate::crdt_documents::xlsx_export::export_doc_to_xlsx(&entry.doc) {
        Ok(bytes) => (
            [(
                axum::http::header::CONTENT_TYPE,
                "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            )],
            bytes,
        )
            .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
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
