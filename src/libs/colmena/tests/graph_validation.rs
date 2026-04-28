//! Smoke test: feeding a graph with an invalid node id is rejected before execution.

use colmena::dag_engine::domain::graph::Graph;

#[test]
fn graph_with_slash_in_node_id_fails_validation() {
    let raw = serde_json::json!({
        "nodes": {
            "bad/id": { "type": "math", "config": {} }
        },
        "edges": []
    });
    let g: Graph = serde_json::from_value(raw).unwrap();
    let err = g.validate().expect_err("validation must fail");
    assert!(err.to_string().contains("bad/id"));
}
