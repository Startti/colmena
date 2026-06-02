//! R1.1 — two WS clients (no browser) hitting the server must converge on
//! the same `yrs::Doc` state.

use colmena::crdt_documents::{
    tool_executor::{apply_set_cell_in_proc, apply_set_cell_via_ws},
    ArtifactId, CrdtDocumentsRuntime,
};
use std::{net::SocketAddr, sync::Arc, time::Duration};

#[tokio::test]
async fn two_ws_agents_and_one_inproc_converge() {
    // Build ephemeral storage for this test.
    let dir = std::env::temp_dir().join(format!("crdt_converge_{}", ulid::Ulid::new()));
    let cfg = serde_json::json!({ "storage_root": dir.to_str().unwrap() });
    let runtime = Arc::new(CrdtDocumentsRuntime::from_config(&cfg).await.unwrap());

    // Pre-register an artifact so the WS handler finds it on first connect.
    let id = ArtifactId::new();
    let entry = runtime.registry.get_or_create(&id, "test");

    // Start the server on a random port.
    let app = colmena::crdt_documents::server::router(runtime.clone());
    let listener = tokio::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let ws_url = format!("ws://{addr}/documents/{}/yjs", id);

    // Agent A via WS.
    apply_set_cell_via_ws(
        &ws_url,
        "s1",
        "A1",
        &serde_json::Value::String("from-A".into()),
    )
    .await
    .expect("agent A");

    // Agent B via WS (different connection).
    apply_set_cell_via_ws(
        &ws_url,
        "s1",
        "B1",
        &serde_json::Value::Number(serde_json::Number::from(42)),
    )
    .await
    .expect("agent B");

    // Agent C: in-proc directly mutates the registered doc.
    apply_set_cell_in_proc(&entry.doc, "s1", "C1", &serde_json::Value::Bool(true));

    // Let the WS round-trips settle.
    tokio::time::sleep(Duration::from_millis(200)).await;

    let projection = colmena::crdt_documents::projection::project(&entry.doc);
    let cells = &projection["sheets"][0]["cells"];
    assert_eq!(cells["A1"], serde_json::Value::String("from-A".into()));
    assert_eq!(cells["B1"], serde_json::json!(42.0));
    assert_eq!(cells["C1"], serde_json::Value::Bool(true));

    let _ = std::fs::remove_dir_all(&dir);
}
