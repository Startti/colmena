//! Regression coverage for the run-loop secure-value leak: `execute_stream`
//! decrypted handles for node execution but streamed the decrypted value
//! back out in node-start/node-finish frames, node-observer events, node
//! error text, and `GraphFinish`. Node `A` gets a handle in its config; node
//! `B` gets `A`'s raw output over a plain edge — no handle of its own.

use async_trait::async_trait;
use colmena::dag_engine::application::ports::NodeRegistryPort;
use colmena::dag_engine::application::run_use_case::DagRunUseCase;
use colmena::dag_engine::application::secure_value_service::SecureValueService;
use colmena::dag_engine::domain::error::DagError;
use colmena::dag_engine::domain::events::DagExecutionEvent;
use colmena::dag_engine::domain::graph::Graph;
use colmena::dag_engine::domain::node::{ExecutableNode, NodeInputs};
use colmena::dag_engine::domain::observer::{ExecutionObserver, NodeEvent};
use colmena::dag_engine::domain::secure_value_repository::SecureValueRepository;
use futures::StreamExt;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::error::Error as StdError;
use std::sync::{Arc, Mutex};

const HANDLE: &str = "<sv_tok_1>";
const SECRET: &str = "LEAKTOK_q8w2e5";

/// With `config.token` set: records it, streams it via `LlmToken`, echoes it
/// (or fails quoting it, when `fail`). With no `config.token` (node B):
/// echoes its resolved inputs unchanged — a downstream node with no handle.
struct EchoNode {
    seen: Arc<Mutex<Vec<String>>>,
    fail: bool,
}

#[async_trait]
impl ExecutableNode for EchoNode {
    async fn execute(
        &self,
        inputs: &NodeInputs,
        config: &Value,
        _state: &mut Value,
        observer: Option<Arc<dyn ExecutionObserver>>,
    ) -> Result<Value, Box<dyn StdError + Send + Sync>> {
        let Some(token) = config.get("token").and_then(|v| v.as_str()) else {
            return Ok(serde_json::to_value(inputs).unwrap_or(Value::Null));
        };
        let received = token.to_string();
        self.seen.lock().unwrap().push(received.clone());
        if let Some(obs) = &observer {
            obs.on_event(NodeEvent::LlmToken {
                token: received.clone(),
            });
        }
        if self.fail {
            return Err(format!("bad token {received}").into());
        }
        Ok(json!({ "eco": received }))
    }
    fn schema(&self) -> Value {
        json!({})
    }
}

struct TestRegistry {
    seen: Arc<Mutex<Vec<String>>>,
    fail: bool,
}

impl NodeRegistryPort for TestRegistry {
    fn get_node(&self, node_type: &str) -> Option<Arc<dyn ExecutableNode>> {
        (node_type == "echo").then(|| {
            Arc::new(EchoNode {
                seen: self.seen.clone(),
                fail: self.fail,
            }) as Arc<dyn ExecutableNode>
        })
    }
    fn get_all_nodes(&self) -> HashMap<String, Arc<dyn ExecutableNode>> {
        HashMap::new()
    }
}

/// Decrypts exactly `<sv_tok_1>` → `LEAKTOK_q8w2e5`; every other method is a
/// no-op (session/agent scoping is irrelevant here).
struct StubSecureValueRepository;

#[async_trait]
impl SecureValueRepository for StubSecureValueRepository {
    async fn persist(
        &self,
        _: &str,
        _: Option<&str>,
        _: &str,
        _: &str,
        _: &str,
        _: &str,
    ) -> Result<(), DagError> {
        Ok(())
    }
    async fn decrypt(
        &self,
        _: &str,
        _: Option<&str>,
        hash_key: &str,
    ) -> Result<Option<String>, DagError> {
        Ok((hash_key == HANDLE).then(|| SECRET.to_string()))
    }
    async fn cleanup(&self, _: &str) -> Result<(), DagError> {
        Ok(())
    }
    async fn cleanup_expired(&self) -> Result<u64, DagError> {
        Ok(0)
    }
    async fn cleanup_expired_for_run(&self, _: &str, _: Option<&str>) -> Result<u64, DagError> {
        Ok(0)
    }
}

/// Drives a root run to completion (or first error); returns every event
/// yielded, the terminal error (if any), and what node A actually received.
async fn run(
    graph_json: Value,
    fail: bool,
) -> (
    Vec<DagExecutionEvent>,
    Option<DagError>,
    Arc<Mutex<Vec<String>>>,
) {
    let graph: Graph = serde_json::from_value(graph_json).expect("valid graph");
    let seen = Arc::new(Mutex::new(Vec::new()));
    let registry = Arc::new(TestRegistry {
        seen: seen.clone(),
        fail,
    });
    let repo: Arc<dyn SecureValueRepository> = Arc::new(StubSecureValueRepository);
    let service = Arc::new(SecureValueService::new(repo));
    let use_case = DagRunUseCase::with_secure_values_and_service(registry, None, service);
    let mut stream = Box::pin(use_case.execute_stream(graph, None, None, false, None, None, None));
    let mut events = Vec::new();
    let mut error = None;
    while let Some(res) = stream.next().await {
        match res {
            Ok(event) => events.push(event),
            Err(e) => {
                error = Some(e);
                break;
            }
        }
    }
    (events, error, seen)
}

#[tokio::test]
async fn secrets_are_masked_in_every_stream_frame_but_nodes_execute_with_real_values() {
    let graph = json!({
        "nodes": {
            "A": { "type": "echo", "config": { "token": HANDLE } },
            "B": { "type": "echo", "config": {} }
        },
        "edges": [{ "from": "A", "to": "B" }]
    });
    let (events, error, seen) = run(graph, false).await;
    assert!(error.is_none(), "run must not error: {error:?}");
    // (a) the node itself still executes with the real decrypted value.
    assert_eq!(seen.lock().unwrap().as_slice(), [SECRET]);

    // (b)+(c) node-start (A and B), LlmToken, node-finish: masked, not dropped.
    let all = serde_json::to_string(&events).unwrap();
    assert!(!all.contains(SECRET) && all.contains(HANDLE), "{all}");
    // (d) the root GraphFinish carries the handle.
    let finish = match events.last() {
        Some(DagExecutionEvent::GraphFinish { output }) => output.to_string(),
        other => panic!("GraphFinish must be last, got {other:?}"),
    };
    assert!(
        !finish.contains(SECRET) && finish.contains(HANDLE),
        "{finish}"
    );
}

#[tokio::test]
async fn a_node_error_does_not_leak_the_secret_it_quotes() {
    let graph = json!({
        "nodes": { "A": { "type": "echo", "config": { "token": HANDLE } } },
        "edges": []
    });
    let (events, error, seen) = run(graph, true).await;
    // The node still ran with the real value before failing.
    assert_eq!(seen.lock().unwrap().as_slice(), [SECRET]);

    let msg = error.expect("node must fail").to_string();
    assert!(!msg.contains(SECRET) && msg.contains(HANDLE), "{msg}");
    let all = serde_json::to_string(&events).unwrap();
    assert!(!all.contains(SECRET), "{all}");
}
