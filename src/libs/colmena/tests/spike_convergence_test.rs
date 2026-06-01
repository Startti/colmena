//! R1.1 — two WS clients (no browser) hitting the spike server must
//! converge on the same `yrs::Doc` state.

use colmena::dag_engine::spike::{
    agent_peer::{apply_set_cell_in_proc, apply_set_cell_via_ws},
    doc_registry::DocRegistry,
    server::{router, SpikeState},
};
use std::{net::SocketAddr, path::PathBuf, sync::Arc, time::Duration};

#[tokio::test]
async fn two_ws_agents_and_one_inproc_converge() {
    // Start the server on a random port.
    let state = SpikeState {
        registry: Arc::new(DocRegistry::new()),
        dump_dir: PathBuf::from(std::env::temp_dir().join("spike_it_dump")),
    };
    let app = router(state.clone());
    let listener = tokio::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let artifact = "converge_test";
    let ws_url = format!("ws://{addr}/yjs/{artifact}");

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
    let doc = state.registry.get_or_create(artifact);
    apply_set_cell_in_proc(&doc, "s1", "C1", &serde_json::Value::Bool(true));

    // Let the WS round-trips settle.
    tokio::time::sleep(Duration::from_millis(200)).await;

    let projection = colmena::dag_engine::spike::projection::project(&doc);
    let cells = &projection["sheets"][0]["cells"];
    assert_eq!(cells["A1"], serde_json::Value::String("from-A".into()));
    assert_eq!(cells["B1"], serde_json::json!(42.0));
    assert_eq!(cells["C1"], serde_json::Value::Bool(true));
}
