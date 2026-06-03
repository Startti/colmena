//! End-to-end REST CRUD tests for the crdt_documents server.

use axum::body::{to_bytes, Body};
use axum::http::{Method, Request, StatusCode};
use colmena::crdt_documents::{ArtifactId, CrdtDocumentsRuntime};
use serde_json::json;
use std::sync::Arc;
use tower::ServiceExt;

async fn build_app() -> (axum::Router, Arc<CrdtDocumentsRuntime>, std::path::PathBuf) {
    let tmp = std::env::temp_dir().join(format!("rest_{}", ulid::Ulid::new()));
    let cfg = json!({ "storage_root": tmp.to_str().unwrap() });
    let rt = Arc::new(CrdtDocumentsRuntime::from_config(&cfg).await.unwrap());
    let app = colmena::crdt_documents::server::router(rt.clone());
    (app, rt, tmp)
}

#[tokio::test]
async fn create_then_list_then_delete() {
    let (app, _rt, tmp) = build_app().await;

    // Create.
    let body = serde_json::to_vec(&json!({ "name": "My Doc" })).unwrap();
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/documents")
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let body_bytes = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
    let created: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    let id_str = created["artifact_id"].as_str().unwrap();
    let id: ArtifactId = id_str.parse().unwrap();

    // List.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/documents")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body_bytes = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
    let listed: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(listed["artifacts"].as_array().unwrap().len(), 1);
    assert_eq!(listed["artifacts"][0]["name"], "My Doc");

    // Delete.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::DELETE)
                .uri(format!("/documents/{}", id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // List again — empty.
    let resp = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/documents")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body_bytes = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
    let listed: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(listed["artifacts"].as_array().unwrap().len(), 0);

    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn delete_invalid_id_returns_400() {
    let (app, _rt, tmp) = build_app().await;
    let resp = app
        .oneshot(
            Request::builder()
                .method(Method::DELETE)
                .uri("/documents/not-an-id")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let _ = std::fs::remove_dir_all(&tmp);
}

// ── Change-tracker REST endpoints (B-T7) ────────────────────────────────

async fn post_json(
    app: &axum::Router,
    uri: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let bytes = serde_json::to_vec(&body).unwrap();
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::from(bytes))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let body_bytes = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
    let v: serde_json::Value =
        serde_json::from_slice(&body_bytes).unwrap_or(serde_json::Value::Null);
    (status, v)
}

async fn get_json(app: &axum::Router, uri: &str) -> (StatusCode, serde_json::Value) {
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let body_bytes = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
    let v: serde_json::Value =
        serde_json::from_slice(&body_bytes).unwrap_or(serde_json::Value::Null);
    (status, v)
}

#[tokio::test]
async fn record_event_then_query_changes_and_cursor() {
    let (app, _rt, tmp) = build_app().await;

    // Create artifact with agent_session_id — must trigger touch_artifact so
    // /documents/by-session/:sid surfaces it.
    let (status, created) = post_json(
        &app,
        "/documents",
        json!({ "name": "X", "agent_session_id": "s1" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let aid = created["artifact_id"].as_str().unwrap().to_string();

    // POST event.
    let (status, ev) = post_json(
        &app,
        &format!("/documents/{aid}/events"),
        json!({ "sheet_id": "sh_a", "origin": "agent:s2", "summary": "set A1 = 42" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(ev["id"].as_u64().unwrap() > 0);

    // GET /documents/:id/changes?since=0 — should return the one event.
    let (status, changes) = get_json(&app, &format!("/documents/{aid}/changes?since=0")).await;
    assert_eq!(status, StatusCode::OK);
    let events = changes["events"].as_array().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["origin"], "agent:s2");
    assert_eq!(events[0]["sheet_id"], "sh_a");
    assert_eq!(
        changes["current_event_id"].as_u64().unwrap(),
        ev["id"].as_u64().unwrap()
    );

    // exclude_origin filter — same origin should yield empty.
    let (status, filtered) = get_json(
        &app,
        &format!("/documents/{aid}/changes?since=0&exclude_origin=agent:s2"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(filtered["events"].as_array().unwrap().len(), 0);

    // sheet_id filter — non-matching sheet → empty.
    let (status, filtered) = get_json(
        &app,
        &format!("/documents/{aid}/changes?since=0&sheet_id=sh_other"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(filtered["events"].as_array().unwrap().len(), 0);

    // GET cursor — none yet.
    let (status, _) = get_json(
        &app,
        &format!("/documents/{aid}/cursor?agent_session_id=s1"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // POST cursor.
    let (status, _) = post_json(
        &app,
        &format!("/documents/{aid}/cursor"),
        json!({ "agent_session_id": "s1", "last_event_id": 1 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // GET cursor — now present.
    let (status, cur) = get_json(
        &app,
        &format!("/documents/{aid}/cursor?agent_session_id=s1"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(cur["last_event_id"].as_u64().unwrap(), 1);

    // GET cursor without agent_session_id → 400.
    let (status, _) = get_json(&app, &format!("/documents/{aid}/cursor")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // /documents/by-session/:sid lists the artifact (touch_artifact during
    // create_handler).
    let (status, by_sid) = get_json(&app, "/documents/by-session/s1").await;
    assert_eq!(status, StatusCode::OK);
    let arts = by_sid["artifacts"].as_array().unwrap();
    assert_eq!(arts.len(), 1);
    assert_eq!(arts[0]["artifact_id"].as_str().unwrap(), aid);
    assert_eq!(arts[0]["name"], "X");

    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn changes_truncated_flag_set_when_limit_reached() {
    let (app, _rt, tmp) = build_app().await;
    let (_, created) = post_json(&app, "/documents", json!({ "name": "T" })).await;
    let aid = created["artifact_id"].as_str().unwrap().to_string();

    for i in 0..5 {
        post_json(
            &app,
            &format!("/documents/{aid}/events"),
            json!({ "origin": "test", "summary": format!("e{i}") }),
        )
        .await;
    }

    let (_, r) = get_json(&app, &format!("/documents/{aid}/changes?since=0&limit=3")).await;
    assert_eq!(r["events"].as_array().unwrap().len(), 3);
    assert_eq!(r["truncated"], true);

    let (_, r) = get_json(&app, &format!("/documents/{aid}/changes?since=0&limit=100")).await;
    assert_eq!(r["events"].as_array().unwrap().len(), 5);
    assert_eq!(r["truncated"], false);

    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn record_event_invalid_artifact_id_returns_400() {
    let (app, _rt, tmp) = build_app().await;
    let (status, _) = post_json(
        &app,
        "/documents/not-an-id/events",
        json!({ "origin": "x", "summary": "y" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let _ = std::fs::remove_dir_all(&tmp);
}
