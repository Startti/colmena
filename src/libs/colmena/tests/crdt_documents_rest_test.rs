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
