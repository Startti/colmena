//! Integration test for the WS-peer mode of `crdt_documents`.
//!
//! Spawns a real `crdt_documents` WS server on a random port, then runs
//! every v1 tool (`list_sheets`, `read`, `set_cell`, `set_range`,
//! `add_sheet`, `get_recent_changes`) against a `CrdtDocsContext::WsPeer`
//! pointed at that server. Asserts that:
//!
//! 1. Mutations the peer issues reach the server's in-memory Y.Doc
//!    (verified via projection).
//! 2. `get_recent_changes` returns the per-session narration of what
//!    THIS agent did.
//! 3. The peer can be shut down cleanly and the server still has the
//!    work (persistence is the server's job in peer mode; we verify
//!    state.yjs grew on disk).
//!
//! This is the colmena-side proxy for the production topology:
//! ADP worker (stateless) opens WS to crdt-documents service, executes
//! a graph that mutates Excel via the LLM tools, server fans out to
//! any connected browser.

use colmena::crdt_documents::{
    process_runtime, server::router as server_router, ArtifactId, CrdtDocumentsRuntime,
    WsPeerArtifact,
};
use colmena::dag_engine::infrastructure::nodes::llm_synthetic_tools::crdt_doc_context::CrdtDocsContext;
use colmena::dag_engine::infrastructure::nodes::llm_synthetic_tools::crdt_doc_tools::{
    execute_add_sheet, execute_get_recent_changes, execute_list_sheets, execute_read,
    execute_set_cell, execute_set_range, AddSheetArgs, GetRecentChangesArgs, ReadArgs, SetCellArgs,
    SetRangeArgs,
};
use serde_json::json;
use std::net::SocketAddr;
use std::sync::Arc;

#[tokio::test]
async fn six_tools_round_trip_via_ws_peer() {
    // ── Server side ──────────────────────────────────────────────────
    let dump = std::env::temp_dir().join(format!("ws_peer_int_{}", ulid::Ulid::new()));
    std::fs::create_dir_all(&dump).unwrap();
    let cfg = json!({
        "storage_backend": "localfs",
        "storage_root": dump.to_str().unwrap(),
    });
    let server_runtime = Arc::new(CrdtDocumentsRuntime::from_config(&cfg).await.unwrap());

    // Pre-create the artifact so the peer has something to sync.
    let aid = ArtifactId::new();
    let _seed = server_runtime
        .registry
        .get_or_create(&aid, "integration test");

    // NOTE: process_runtime::set_global is INTENTIONALLY skipped here. The
    // whole point of ws_peer mode is that the peer doesn't share an
    // in-process runtime with the server — it talks via WS only. Setting
    // the singleton would short-circuit the peer path in llm.rs (which we
    // are not testing here anyway).
    //
    // If a prior test in this crate's integration suite has already
    // installed a singleton, our peer-mode context construction below
    // ignores it. This is the desired isolation.
    let _ = process_runtime::is_installed();

    // Start the WS+REST server on a random port.
    let app = server_router(server_runtime.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    // Tiny pause so axum is ready for WS upgrades.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // ── Peer side (the "agent" in the stateless worker) ──────────────
    let server_url = format!("ws://{}/yjs", addr);
    let http_base = format!("http://{}", addr);
    let peer = WsPeerArtifact::connect(&server_url, aid.clone(), "agent", Some("test_session"))
        .await
        .expect("peer connect");
    assert!(peer.is_alive());
    let ctx =
        CrdtDocsContext::new_ws_peer(&peer, Some("test_session".to_string()), http_base.clone());

    // 1) Initial list_sheets — no sheets yet (the seed artifact is empty).
    let v = execute_list_sheets(&ctx);
    let sheets = v["sheets"].as_array().expect("sheets array").clone();
    assert_eq!(sheets.len(), 0, "fresh artifact must have zero sheets");

    // 2) add_sheet "Inventory" → get sheet_id.
    let v = execute_add_sheet(
        &ctx,
        AddSheetArgs {
            name: "Inventory".into(),
        },
    )
    .await;
    let inventory_id = v["sheet_id"]
        .as_str()
        .expect("sheet_id present")
        .to_string();
    assert!(inventory_id.starts_with("sh_"));

    // 3) add_sheet "Pricing" → second sheet so list_sheets has multiple.
    let v = execute_add_sheet(
        &ctx,
        AddSheetArgs {
            name: "Pricing".into(),
        },
    )
    .await;
    let pricing_id = v["sheet_id"].as_str().unwrap().to_string();

    // 4) list_sheets — should now show both.
    let v = execute_list_sheets(&ctx);
    let sheets = v["sheets"].as_array().unwrap();
    assert_eq!(sheets.len(), 2);
    let names: Vec<&str> = sheets.iter().filter_map(|s| s["name"].as_str()).collect();
    assert!(names.contains(&"Inventory"));
    assert!(names.contains(&"Pricing"));

    // 5) set_cell on Inventory A1.
    let v = execute_set_cell(
        &ctx,
        SetCellArgs {
            sheet_id: inventory_id.clone(),
            addr: "A1".into(),
            value: json!("Product"),
        },
    )
    .await;
    assert_eq!(v["ok"], json!(true));

    // 6) set_range on Inventory A2:B4 (3 rows × 2 cols = 6 cells).
    let v = execute_set_range(
        &ctx,
        SetRangeArgs {
            sheet_id: inventory_id.clone(),
            start_addr: "A2".into(),
            values_2d: vec![
                vec![json!("Apple"), json!(10)],
                vec![json!("Pear"), json!(20)],
                vec![json!("Plum"), json!(15)],
            ],
        },
    )
    .await;
    assert_eq!(v["cells_written"], json!(6));

    // Give the WS round-trip a moment to land on the server.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // 7) read back the data we just wrote.
    let v = execute_read(
        &ctx,
        ReadArgs {
            sheet_id: inventory_id.clone(),
            range: None,
        },
    );
    let cells = v["cells"].as_object().expect("cells object");
    assert_eq!(cells.get("A1").and_then(|v| v.as_str()), Some("Product"));
    assert_eq!(cells.get("A2").and_then(|v| v.as_str()), Some("Apple"));
    assert_eq!(cells.get("B2").and_then(|v| v.as_f64()), Some(10.0));
    assert_eq!(cells.get("A4").and_then(|v| v.as_str()), Some("Plum"));

    // 8) read with range filter — only A2:A4.
    let v = execute_read(
        &ctx,
        ReadArgs {
            sheet_id: inventory_id.clone(),
            range: Some("A2:A4".into()),
        },
    );
    let cells = v["cells"].as_object().unwrap();
    assert_eq!(cells.len(), 3);
    assert!(cells.contains_key("A2"));
    assert!(cells.contains_key("A3"));
    assert!(cells.contains_key("A4"));
    assert!(!cells.contains_key("B2"));

    // 9) get_recent_changes — after B-T13, the agent's WS-peer mutations
    //    are attributed by the server with origin "agent:test_session"
    //    (from the peer_type=agent&session_id=test_session URL query
    //    params on the WS upgrade). The own-origin filter then correctly
    //    hides them from the agent itself. So the call should either
    //    return no events, OR only events whose origin is not the
    //    agent's own session.
    let v = execute_get_recent_changes(
        &ctx,
        GetRecentChangesArgs {
            since_event_id: None,
            sheet_id: None,
            limit: None,
            artifact_id: None,
        },
    )
    .await;
    let events = v["events"].as_array().expect("events array");
    // After B-T13 the round-trip may produce zero visible events from the
    // agent's POV (all are own-origin and filtered). `current_event_id`
    // is then `null` from the tool's perspective; we treat that as 0
    // so the cursor-based read below still works.
    let last_event_id = v["current_event_id"].as_u64().unwrap_or(0);

    // None of the events should be tagged with the agent's own
    // session id — the filter must hide those.
    for ev in events {
        let origin = ev["origin"].as_str().expect("origin is string");
        assert_ne!(
            origin, "agent:test_session",
            "own-session events must be filtered out, got: {ev:?}"
        );
    }

    // 10) Seed an additional event tagged with the agent's own session
    //     and verify it does NOT appear in the next read — confirming
    //     the own-origin filter.
    let own_seeded = server_runtime
        .store
        .insert_event(colmena::crdt_documents::change_tracker_store::NewEvent {
            artifact_id: aid.clone(),
            sheet_id: None,
            origin: "agent:test_session".to_string(),
            summary: "own-session event that must be hidden".to_string(),
        })
        .await
        .expect("seed own event");
    assert!(own_seeded > last_event_id);

    let v = execute_get_recent_changes(
        &ctx,
        GetRecentChangesArgs {
            since_event_id: Some(last_event_id),
            sheet_id: None,
            limit: None,
            artifact_id: None,
        },
    )
    .await;
    assert!(
        v["events"]
            .as_array()
            .expect("events array")
            .iter()
            .all(|e| e["origin"] != json!("agent:test_session")),
        "agent:test_session events must be filtered out: {:?}",
        v["events"]
    );

    // 11) Seed a peer-side agent event and verify it surfaces (origin
    //     is "agent:other_peer", which does NOT match the own filter).
    let peer_event_id = server_runtime
        .store
        .insert_event(colmena::crdt_documents::change_tracker_store::NewEvent {
            artifact_id: aid.clone(),
            sheet_id: Some(inventory_id.clone()),
            origin: "agent:other_peer".to_string(),
            summary: "peer wrote a cell".to_string(),
        })
        .await
        .expect("seed peer event");

    let v = execute_get_recent_changes(
        &ctx,
        GetRecentChangesArgs {
            since_event_id: Some(own_seeded),
            sheet_id: None,
            limit: None,
            artifact_id: None,
        },
    )
    .await;
    let events = v["events"].as_array().expect("events array");
    assert!(
        events.iter().any(|e| e["id"] == json!(peer_event_id)),
        "expected peer event to appear, got: {events:?}"
    );

    // 12) get_recent_changes with cursor at peer_event_id — should be empty.
    let v = execute_get_recent_changes(
        &ctx,
        GetRecentChangesArgs {
            since_event_id: Some(peer_event_id),
            sheet_id: None,
            limit: None,
            artifact_id: None,
        },
    )
    .await;
    assert!(v["events"].as_array().expect("events array").is_empty());

    // ── Server-side verification ─────────────────────────────────────
    // Projection on the server's authoritative Y.Doc should now reflect
    // every mutation the peer issued.
    let server_entry = server_runtime.registry.get(&aid).expect("artifact present");
    let proj = colmena::crdt_documents::projection::project(&server_entry.doc);
    let server_sheets = proj["sheets"].as_array().expect("sheets array on server");
    assert_eq!(server_sheets.len(), 2);
    let inventory = server_sheets
        .iter()
        .find(|s| s["name"] == json!("Inventory"))
        .expect("Inventory present on server");
    let inventory_cells = inventory["cells"].as_object().unwrap();
    assert_eq!(inventory_cells.len(), 7); // A1 + A2:B4
    assert_eq!(
        inventory_cells.get("A4").and_then(|v| v.as_str()),
        Some("Plum")
    );

    // We have to silence pricing_id (unused, but useful as a test marker
    // — its sheet is empty by design).
    assert!(!pricing_id.is_empty());

    // ── Cleanup ──────────────────────────────────────────────────────
    let mut peer = peer;
    peer.shutdown().await;
    assert!(!peer.is_alive());

    // After peer shutdown, the server's snapshot writer eventually
    // flushes state.yjs. Drain explicitly via the runtime shutdown.
    server_runtime.shutdown().await;

    let state_file = dump.join(aid.as_str()).join("state.yjs");
    let state_size = std::fs::metadata(&state_file)
        .expect("state.yjs must exist after server.shutdown")
        .len();
    assert!(
        state_size > 100,
        "state.yjs should contain real workbook data, got {state_size} bytes at {state_file:?}"
    );

    let _ = std::fs::remove_dir_all(&dump);
}
