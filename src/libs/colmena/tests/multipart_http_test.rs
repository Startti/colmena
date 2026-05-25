//! End-to-end integration test for http_request multipart streaming.
//! Spins up two wiremock servers — one upstream (sources) and one downstream
//! (target) — and runs the node through HttpNode::execute.

use colmena::dag_engine::domain::node::ExecutableNode;
use colmena::dag_engine::infrastructure::nodes::http::HttpNode;
use serde_json::{json, Value};
use std::collections::HashMap;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn end_to_end_three_url_parts_multipart_upload() {
    let upstream = MockServer::start().await;
    let downstream = MockServer::start().await;

    let payloads: Vec<(&str, Vec<u8>, &str)> = vec![
        ("/a", vec![0xAAu8; 100], "application/pdf"),
        ("/b", vec![0xBBu8; 500], "application/pdf"),
        ("/c", vec![0xCCu8; 2_000], "application/pdf"),
    ];
    for (p, body, ct) in &payloads {
        Mock::given(method("HEAD"))
            .and(path(*p))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Content-Length", body.len().to_string())
                    .insert_header("Content-Type", *ct),
            )
            .mount(&upstream)
            .await;
        Mock::given(method("GET"))
            .and(path(*p))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(body.clone()))
            .mount(&upstream)
            .await;
    }

    Mock::given(method("POST"))
        .and(path("/kb/upload"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "uploaded": 3 })))
        .mount(&downstream)
        .await;

    let body = json!({
        "files": [
            format!("{}/a", upstream.uri()),
            format!("{}/b", upstream.uri()),
            format!("{}/c", upstream.uri()),
        ],
        "description": "uploaded by agent"
    });

    let config = json!({
        "base_url": downstream.uri(),
        "endpoint": "/kb/upload",
        "method": "POST",
        "headers": { "Content-Type": "multipart/form-data" },
        "allow_http_urls": true,
        "body": body
    });

    let node = HttpNode::new();
    let out = node
        .execute(
            &HashMap::<String, Value>::new(),
            &config,
            &mut json!({}),
            None,
        )
        .await
        .expect("multipart end-to-end ok");
    assert_eq!(out["status"], 200);
    assert_eq!(out["body"]["uploaded"], 3);

    // Inspect the downstream request: must be multipart with boundary.
    let received = downstream.received_requests().await.unwrap();
    let req = received
        .iter()
        .find(|r| r.url.path() == "/kb/upload")
        .expect("downstream POST received");
    let ct = req.headers.get("content-type").unwrap().to_str().unwrap();
    assert!(ct.starts_with("multipart/form-data"), "got {ct}");
    assert!(ct.contains("boundary="), "boundary missing in {ct}");

    // The body should contain all three payloads (sanity — full multipart
    // parsing is overkill; we just confirm bytes survived end-to-end).
    let body_bytes = &req.body;
    assert!(body_bytes.windows(100).any(|w| w == &[0xAAu8; 100][..]));
    assert!(body_bytes.windows(500).any(|w| w == &[0xBBu8; 500][..]));
    // 2_000-byte window check is O(n*m) but n is small here.
    assert!(body_bytes
        .windows(2_000)
        .any(|w| w == &vec![0xCCu8; 2_000][..]));
    // Text part should be inline.
    assert!(body_bytes
        .windows(b"uploaded by agent".len())
        .any(|w| w == b"uploaded by agent"));
}

#[tokio::test]
async fn end_to_end_oversized_upstream_aborts_before_downstream_post() {
    let upstream = MockServer::start().await;
    let downstream = MockServer::start().await;

    Mock::given(method("HEAD"))
        .and(path("/big"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Length", "1000000")
                .insert_header("Content-Type", "application/octet-stream"),
        )
        .mount(&upstream)
        .await;

    Mock::given(method("POST"))
        .and(path("/kb/upload"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&downstream)
        .await;

    let config = json!({
        "base_url": downstream.uri(),
        "endpoint": "/kb/upload",
        "method": "POST",
        "headers": { "Content-Type": "multipart/form-data" },
        "allow_http_urls": true,
        "max_file_size_bytes": 100,
        "body": { "files": format!("{}/big", upstream.uri()) }
    });

    let node = HttpNode::new();
    let err = node
        .execute(
            &HashMap::<String, Value>::new(),
            &config,
            &mut json!({}),
            None,
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains("FileTooLarge"), "got {err}");

    // Downstream must NOT have received any POST.
    let received = downstream.received_requests().await.unwrap();
    assert!(
        received.iter().all(|r| r.url.path() != "/kb/upload"),
        "downstream received an upload despite upstream being oversized"
    );
}
