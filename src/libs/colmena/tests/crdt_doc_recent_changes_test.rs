//! End-to-end test for subsystem B (recent changes + discovery).
//!
//! Spins up a real CRDT documents server with a file-backed sqlite store,
//! connects an agent as ws_peer with a session_id, simulates the
//! auto-summary flow, and verifies cursor advancement + drill-down
//! filtering + own-event filtering.

use colmena::crdt_documents::{
    server::router as server_router, ArtifactId, CrdtDocumentsRuntime, WsPeerArtifact,
};
use colmena::dag_engine::infrastructure::nodes::llm_synthetic_tools::{
    build_recent_changes_block,
    crdt_doc_context::CrdtDocsContext,
    crdt_doc_tools::{
        dispatch_crdt_doc_get_recent_changes, execute_get_recent_changes,
        execute_list_my_artifacts, execute_set_cell, GetRecentChangesArgs, ListMyArtifactsArgs,
        SetCellArgs,
    },
};
use serde_json::json;
use std::net::SocketAddr;
use std::sync::Arc;

async fn make_runtime() -> (Arc<CrdtDocumentsRuntime>, std::path::PathBuf) {
    let tmp = std::env::temp_dir().join(format!("rcf_{}", ulid::Ulid::new()));
    std::fs::create_dir_all(&tmp).unwrap();
    let cfg = json!({"storage_backend": "localfs", "storage_root": tmp.to_str().unwrap()});
    let rt = Arc::new(CrdtDocumentsRuntime::from_config(&cfg).await.unwrap());
    (rt, tmp)
}

#[tokio::test]
async fn recent_changes_round_trip_via_ws_peer() {
    // --- Server ---------------------------------------------------------
    let dump = std::env::temp_dir().join(format!("b_int_{}", ulid::Ulid::new()));
    std::fs::create_dir_all(&dump).unwrap();
    // File-backed sqlite (NOT `:memory:`) so the multi-connection pool the
    // runtime spins up sees a single shared DB. `sqlite::memory:` would
    // give each connection its own empty DB and break the test in
    // confusing ways (see `change_tracker_store::tests::sqlx_sqlite_round_trip`
    // which pins max_connections=1 for the same reason).
    let db_path = dump.join("b.sqlite3");
    let database_url = format!("sqlite://{}?mode=rwc", db_path.display());
    let cfg = json!({
        "storage_backend": "localfs",
        "storage_root": dump.to_str().unwrap(),
        "database_url": database_url,
    });
    let server_runtime = Arc::new(CrdtDocumentsRuntime::from_config(&cfg).await.unwrap());
    let aid = ArtifactId::new();
    let _ = server_runtime.registry.get_or_create(&aid, "B test");

    let app = server_router(server_runtime.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let session_id = "session_b_test".to_string();
    let server_url = format!("ws://{}/yjs", addr);

    // Pre-seed: another peer (browser) made changes before our turn.
    server_runtime
        .store
        .insert_event(colmena::crdt_documents::change_tracker_store::NewEvent {
            artifact_id: aid.clone(),
            sheet_id: Some("Inventory".into()),
            origin: "peer:browser".into(),
            summary: "set Inventory!A1 = hello".into(),
        })
        .await
        .unwrap();
    server_runtime
        .store
        .insert_event(colmena::crdt_documents::change_tracker_store::NewEvent {
            artifact_id: aid.clone(),
            sheet_id: Some("Inventory".into()),
            origin: "peer:browser".into(),
            summary: "set Inventory!A2 = world".into(),
        })
        .await
        .unwrap();

    // --- Peer (agent in stateless worker) -------------------------------
    let peer = WsPeerArtifact::connect(&server_url, aid.clone(), "agent", Some(&session_id))
        .await
        .unwrap();
    let http_base = format!("http://{}", addr);
    let ctx = CrdtDocsContext::new_ws_peer(&peer, Some(session_id.clone()), &http_base);

    // 1) Auto-summary should show 2 peer:browser changes on Inventory.
    let block = build_recent_changes_block(&ctx).await.expect("block");
    assert!(block.contains("2 events, 1 peer"), "block was: {block}");
    assert!(
        block.contains("Inventory: 2 changes by peer:browser"),
        "block was: {block}"
    );

    // 2) Drill-down via tool with sheet filter.
    let v = execute_get_recent_changes(
        &ctx,
        GetRecentChangesArgs {
            since_event_id: None,
            sheet_id: Some("Inventory".into()),
            limit: None,
            artifact_id: None,
        },
    )
    .await;
    let events = v["events"].as_array().expect("events array");
    assert_eq!(events.len(), 2, "expected 2 peer events");

    // 3) Agent does its own mutation (set_cell). After B-T13 the server
    //    attributes the resulting WS-roundtripped event with our
    //    `agent:session_b_test` origin (not peer:browser).
    execute_set_cell(
        &ctx,
        SetCellArgs {
            sheet_id: "Inventory".into(),
            addr: "B1".into(),
            value: json!("agent wrote this"),
        },
    )
    .await;

    // Brief pause so the WS round-trip lands on the server.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // 4) Verify the agent's own event is filtered out of the summary.
    let block_again = build_recent_changes_block(&ctx).await.expect("block");
    assert!(
        !block_again.contains(&format!("agent:{}", session_id)),
        "agent's own events must be filtered: {block_again}"
    );
    // Peer:browser events should still appear.
    assert!(
        block_again.contains("peer:browser"),
        "peer:browser events still visible: {block_again}"
    );

    // 5) Simulate end-of-turn cursor update (what llm.rs does in B-T12).
    //    The agent's own record_event also calls ctx.record_event_id, so
    //    max_event_id_observed > 0.
    let max = ctx.max_event_id_observed();
    assert!(
        max > 0,
        "agent should have observed at least its own event id"
    );
    ctx.backend()
        .upsert_cursor(&session_id, &aid, max)
        .await
        .unwrap();

    // 6) After cursor advance, summary should be None (no NEW events from
    //    others since we last looked).
    let block_next = build_recent_changes_block(&ctx).await;
    assert!(
        block_next.is_none(),
        "no new events since cursor → block should be None, was: {block_next:?}"
    );

    // 7) Discovery tool. The session may not have any artifacts touched
    //    (depends on whether server.post_documents was called with
    //    agent_session_id — in this test, we used registry.get_or_create
    //    directly so it doesn't). Just verify the tool runs without
    //    error.
    let v = execute_list_my_artifacts(&ctx, ListMyArtifactsArgs { limit: None }).await;
    assert!(v["artifacts"].is_array(), "artifacts should be an array");

    // --- Cleanup --------------------------------------------------------
    let mut peer = peer;
    peer.shutdown().await;
    server_runtime.shutdown().await;
    let _ = std::fs::remove_dir_all(&dump);
}

#[tokio::test]
async fn get_recent_changes_artifact_filter_works() {
    // Cross-artifact audit: ctx is on A, but agent queries events of B.
    use colmena::crdt_documents::change_tracker_store::NewEvent;
    let (rt, tmp) = make_runtime().await;
    let aid_a = ArtifactId::new();
    let aid_b = ArtifactId::new();
    let _ = rt.registry.get_or_create(&aid_a, "A");
    let _ = rt.registry.get_or_create(&aid_b, "B");
    rt.store
        .insert_event(NewEvent {
            artifact_id: aid_b.clone(),
            sheet_id: None,
            origin: "agent:other_session".to_string(),
            summary: "did something in B".to_string(),
        })
        .await
        .unwrap();
    let ctx = CrdtDocsContext::new_local(rt.clone(), aid_a, Some("s".to_string()));
    let result = dispatch_crdt_doc_get_recent_changes(
        &ctx,
        json!({
            "artifact_id": aid_b.to_string(),
        }),
    )
    .await;
    let events = result["events"].as_array().expect("events");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["summary"], "did something in B");
    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn get_recent_changes_backward_compat_no_arg() {
    // No artifact_id → audits the ctx artifact (B behaviour unchanged).
    use colmena::crdt_documents::change_tracker_store::NewEvent;
    let (rt, tmp) = make_runtime().await;
    let aid_a = ArtifactId::new();
    let _ = rt.registry.get_or_create(&aid_a, "A");
    rt.store
        .insert_event(NewEvent {
            artifact_id: aid_a.clone(),
            sheet_id: None,
            origin: "agent:other_session".to_string(),
            summary: "did something in A".to_string(),
        })
        .await
        .unwrap();
    let ctx = CrdtDocsContext::new_local(rt.clone(), aid_a.clone(), Some("s".to_string()));
    let result = dispatch_crdt_doc_get_recent_changes(&ctx, json!({})).await;
    let events = result["events"].as_array().expect("events");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["summary"], "did something in A");
    let _ = std::fs::remove_dir_all(&tmp);
}
