//! End-to-end tests for cooperative hard-stop / graph cancellation.
//!
//! Requires `DATABASE_URL` to be set and reachable. Each test cleans up its
//! own `dag_runs` rows. Run with `cargo test -- --ignored`.
//!
//! The DAG stream is poll-driven (an `async_stream` generator): it only makes
//! progress when polled. That makes "cancel between nodes" deterministic — once
//! we receive a `NodeFinish` the generator is parked at the yield, so cancelling
//! before the next poll is observed at the top of the next loop iteration.

use colmena::dag_engine::domain::events::DagExecutionEvent;
use colmena::dag_engine::domain::graph::Graph;
use colmena::dag_engine::engine::{ColmenaEngine, EngineConfig};
use colmena::dag_engine::sse_mapper::SseMapper;
use futures::StreamExt;
use serde_json::json;
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn engine() -> ColmenaEngine {
    dotenvy::dotenv().ok();
    let cfg = EngineConfig::from_env().await.unwrap();
    ColmenaEngine::new(cfg).await.unwrap()
}

/// 3-node chain: mock_input(5) → exponential(^3) → log. All nodes emit
/// non-null outputs so the chain advances node-by-node.
fn power_graph() -> Graph {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../tests/graphs/basic/power.json"
    );
    let raw = std::fs::read_to_string(path).expect("power.json must exist");
    serde_json::from_str(&raw).expect("valid graph JSON")
}

fn unique_chat(prefix: &str) -> String {
    format!(
        "{}_{}",
        prefix,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

async fn status_for_chat(chat: &str) -> Option<String> {
    dotenvy::dotenv().ok();
    let url = std::env::var("DATABASE_URL").unwrap();
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    let row: Option<(String,)> =
        sqlx::query_as("SELECT status FROM dag_runs WHERE agent_session_id = $1 LIMIT 1")
            .bind(chat)
            .fetch_optional(&pool)
            .await
            .unwrap();
    row.map(|r| r.0)
}

async fn cleanup(chat: &str) {
    dotenvy::dotenv().ok();
    let url = std::env::var("DATABASE_URL").unwrap();
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    sqlx::query("DELETE FROM dag_runs WHERE agent_session_id = $1")
        .bind(chat)
        .execute(&pool)
        .await
        .ok();
}

/// A pre-cancelled token stops the run before the first node executes: no
/// NodeFinish events, a terminal Cancelled event, and a persisted CANCELLED row.
#[tokio::test]
#[ignore = "requires DATABASE_URL — run with `cargo test -- --ignored`"]
async fn precancelled_token_stops_before_first_node() {
    let chat = unique_chat("cancel_pre");
    cleanup(&chat).await;
    let eng = engine().await;

    let token = CancellationToken::new();
    token.cancel(); // already cancelled before we run

    let mut s = Box::pin(eng.execute_stream_cancellable(
        power_graph(),
        None,
        None,
        false,
        None,
        Some(chat.clone()),
        token,
    ));

    let mut node_finishes = 0;
    let mut saw_cancelled = false;
    while let Some(item) = s.next().await {
        match item.expect("stream must not error") {
            DagExecutionEvent::NodeFinish { .. } => node_finishes += 1,
            DagExecutionEvent::Cancelled { .. } => saw_cancelled = true,
            _ => {}
        }
    }
    drop(s);

    assert_eq!(node_finishes, 0, "no node should run when pre-cancelled");
    assert!(saw_cancelled, "must emit a terminal Cancelled event");
    assert_eq!(status_for_chat(&chat).await.as_deref(), Some("CANCELLED"));

    cleanup(&chat).await;
    eng.shutdown().await;
}

/// Cancelling after the first NodeFinish stops the run between nodes: the
/// remaining nodes do not finish, a Cancelled event is emitted, and the run is
/// persisted as CANCELLED.
#[tokio::test]
#[ignore = "requires DATABASE_URL — run with `cargo test -- --ignored`"]
async fn cancel_between_nodes_stops_remaining_nodes() {
    let chat = unique_chat("cancel_mid");
    cleanup(&chat).await;
    let eng = engine().await;

    let token = CancellationToken::new();
    let mut s = Box::pin(eng.execute_stream_cancellable(
        power_graph(),
        None,
        None,
        false,
        None,
        Some(chat.clone()),
        token.clone(),
    ));

    let mut node_finishes = 0;
    let mut saw_cancelled = false;
    while let Some(item) = s.next().await {
        match item.expect("stream must not error") {
            DagExecutionEvent::NodeFinish { .. } => {
                node_finishes += 1;
                if node_finishes == 1 {
                    // Park is at the yield; cancel before polling the next node.
                    token.cancel();
                }
            }
            DagExecutionEvent::Cancelled { .. } => saw_cancelled = true,
            _ => {}
        }
    }
    drop(s);

    assert_eq!(
        node_finishes, 1,
        "only the first node should finish before cancellation"
    );
    assert!(saw_cancelled, "must emit a terminal Cancelled event");
    assert_eq!(status_for_chat(&chat).await.as_deref(), Some("CANCELLED"));

    cleanup(&chat).await;
    eng.shutdown().await;
}

/// Without a cancel token the run completes normally — the cancellation path is
/// inert when not wired.
#[tokio::test]
#[ignore = "requires DATABASE_URL — run with `cargo test -- --ignored`"]
async fn no_token_completes_normally() {
    let chat = unique_chat("cancel_none");
    cleanup(&chat).await;
    let eng = engine().await;

    let mut s =
        Box::pin(eng.execute_stream(power_graph(), None, None, false, None, Some(chat.clone())));

    let mut saw_cancelled = false;
    let mut saw_finish = false;
    while let Some(item) = s.next().await {
        match item.expect("stream must not error") {
            DagExecutionEvent::Cancelled { .. } => saw_cancelled = true,
            DagExecutionEvent::GraphFinish { .. } => saw_finish = true,
            _ => {}
        }
    }
    drop(s);

    assert!(!saw_cancelled, "no cancellation without a token");
    assert!(saw_finish, "run must finish normally");
    assert_eq!(status_for_chat(&chat).await.as_deref(), Some("COMPLETED"));

    cleanup(&chat).await;
    eng.shutdown().await;
}

/// MID-NODE hard-stop against a REAL slow node (Point B).
///
/// Spins a wiremock server whose `/slow` endpoint takes 10s, runs a graph with
/// a real `http_request` node hitting it, then cancels ~800ms in. If the
/// in-flight reqwest future is actually dropped, the whole run ends in ~1s
/// (well under the 10s delay) — proving the node was aborted mid-flight, not
/// awaited to completion. Also maps the stream through the real `SseMapper`,
/// writes the resulting SSE frames to `/tmp/colmena_e2e/`, and prints a report.
#[tokio::test]
#[ignore = "requires DATABASE_URL — run with `cargo test -- --ignored`"]
async fn cancel_mid_node_aborts_inflight_http_and_emits_sse() {
    let chat = unique_chat("cancel_http");
    cleanup(&chat).await;

    // 10s-delayed upstream.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/slow"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(Duration::from_secs(10))
                .set_body_json(json!({ "ok": true })),
        )
        .mount(&server)
        .await;

    let graph: Graph = serde_json::from_value(json!({
        "nodes": {
            "slow": {
                "type": "http_request",
                "config": {
                    "base_url": server.uri(),
                    "endpoint": "/slow",
                    "method": "GET",
                    "allow_http_urls": true
                }
            }
        },
        "edges": []
    }))
    .unwrap();

    let eng = engine().await;
    let token = CancellationToken::new();

    // Canceller: fire ~800ms in, while the http node is mid-request.
    let canceller = token.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(800)).await;
        canceller.cancel();
    });

    let started = Instant::now();
    let mut mapper = SseMapper::new();
    let mut sse_frames: Vec<String> = Vec::new();
    let mut saw_cancelled = false;
    let mut saw_node_finish = false;

    let mut s = Box::pin(eng.execute_stream_cancellable(
        graph,
        None,
        None,
        false,
        None,
        Some(chat.clone()),
        token,
    ));
    while let Some(item) = s.next().await {
        let ev = item.expect("stream must not error");
        if matches!(ev, DagExecutionEvent::Cancelled { .. }) {
            saw_cancelled = true;
        }
        if matches!(ev, DagExecutionEvent::NodeFinish { .. }) {
            saw_node_finish = true;
        }
        for part in mapper.map(&ev) {
            sse_frames.push(part.to_string());
        }
    }
    let elapsed = started.elapsed();
    drop(s);

    // Persist an artifact + report (project convention).
    let dir = "/tmp/colmena_e2e";
    std::fs::create_dir_all(dir).ok();
    let sse_path = format!("{}/hard_stop_midnode.sse", dir);
    std::fs::write(&sse_path, sse_frames.join("\n")).ok();
    eprintln!("──────── HARD-STOP MID-NODE E2E ────────");
    eprintln!("upstream delay   : 10s");
    eprintln!("cancel fired at  : ~800ms");
    eprintln!("total elapsed    : {:?}", elapsed);
    eprintln!("node finished    : {}", saw_node_finish);
    eprintln!("cancelled event  : {}", saw_cancelled);
    eprintln!(
        "SSE frames       : {} (saved to {})",
        sse_frames.len(),
        sse_path
    );
    eprintln!(
        "last SSE frame   : {}",
        sse_frames.last().cloned().unwrap_or_default()
    );
    eprintln!("────────────────────────────────────────");

    assert!(saw_cancelled, "must emit a terminal Cancelled event");
    assert!(!saw_node_finish, "the slow node must NOT finish");
    assert!(
        elapsed < Duration::from_secs(5),
        "run took {:?} — the in-flight request was NOT aborted (expected ≪10s)",
        elapsed
    );
    // SSE wire format: a `cancelled` frame and a `finish` terminator.
    assert!(
        sse_frames
            .iter()
            .any(|f| f.contains("\"type\":\"cancelled\"")),
        "SSE must contain a cancelled frame"
    );
    assert!(
        sse_frames
            .iter()
            .any(|f| f.contains("\"finishReason\":\"cancelled\"")),
        "SSE must contain a finish terminator with finishReason=cancelled"
    );
    assert_eq!(status_for_chat(&chat).await.as_deref(), Some("CANCELLED"));

    cleanup(&chat).await;
    eng.shutdown().await;
}
