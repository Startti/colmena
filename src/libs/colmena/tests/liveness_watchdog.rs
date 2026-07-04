//! Liveness watchdog tests: Progress heartbeats during silent nodes and
//! (Task 5) idle-abort of hung nodes. Pure in-memory — stub registry, no DB.

use async_trait::async_trait;
use colmena::dag_engine::application::liveness::LivenessSettings;
use colmena::dag_engine::application::ports::NodeRegistryPort;
use colmena::dag_engine::application::run_use_case::DagRunUseCase;
use colmena::dag_engine::domain::events::DagExecutionEvent;
use colmena::dag_engine::domain::graph::Graph;
use colmena::dag_engine::domain::node::{ExecutableNode, NodeInputs};
use colmena::dag_engine::domain::observer::{ExecutionObserver, NodeEvent};
use futures::StreamExt;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::error::Error as StdError;
use std::sync::Arc;
use std::time::Duration;

/// Sleeps silently for `millis`, then returns. The "hung / slow tool" stand-in.
struct SleepyNode {
    millis: u64,
}

#[async_trait]
impl ExecutableNode for SleepyNode {
    async fn execute(
        &self,
        _inputs: &NodeInputs,
        _config: &Value,
        _state: &mut Value,
        _observer: Option<Arc<dyn ExecutionObserver>>,
    ) -> Result<Value, Box<dyn StdError + Send + Sync>> {
        tokio::time::sleep(Duration::from_millis(self.millis)).await;
        Ok(json!({"ok": true}))
    }
    fn schema(&self) -> Value {
        json!({})
    }
}

/// Emits an LlmToken every `tick_millis`, `ticks` times. The "alive and
/// chatty" stand-in — its events must reset the liveness clocks.
struct ChattyNode {
    tick_millis: u64,
    ticks: u32,
}

#[async_trait]
impl ExecutableNode for ChattyNode {
    async fn execute(
        &self,
        _inputs: &NodeInputs,
        _config: &Value,
        _state: &mut Value,
        observer: Option<Arc<dyn ExecutionObserver>>,
    ) -> Result<Value, Box<dyn StdError + Send + Sync>> {
        for _ in 0..self.ticks {
            tokio::time::sleep(Duration::from_millis(self.tick_millis)).await;
            if let Some(obs) = &observer {
                obs.on_event(NodeEvent::LlmToken {
                    token: "tick".to_string(),
                });
            }
        }
        Ok(json!({"ok": true}))
    }
    fn schema(&self) -> Value {
        json!({})
    }
}

struct TestRegistry {
    nodes: HashMap<String, Arc<dyn ExecutableNode>>,
}

impl NodeRegistryPort for TestRegistry {
    fn get_node(&self, node_type: &str) -> Option<Arc<dyn ExecutableNode>> {
        self.nodes.get(node_type).cloned()
    }
    fn get_all_nodes(&self) -> HashMap<String, Arc<dyn ExecutableNode>> {
        self.nodes.clone()
    }
}

fn single_node_graph(node_type: &str) -> Graph {
    serde_json::from_value(json!({
        "nodes": { "n1": { "type": node_type, "config": {} } },
        "edges": []
    }))
    .expect("valid graph JSON")
}

fn use_case(
    node_type: &str,
    node: Arc<dyn ExecutableNode>,
    liveness: LivenessSettings,
) -> DagRunUseCase {
    let mut nodes: HashMap<String, Arc<dyn ExecutableNode>> = HashMap::new();
    nodes.insert(node_type.to_string(), node);
    DagRunUseCase::new(Arc::new(TestRegistry { nodes }), None).with_liveness(liveness)
}

/// Drains the stream, returning (ok_events, first_error_message).
async fn drain(
    uc: DagRunUseCase,
    graph: Graph,
    cancel: Option<tokio_util::sync::CancellationToken>,
) -> (Vec<DagExecutionEvent>, Option<String>) {
    let stream = uc.execute_stream(graph, None, None, false, None, None, cancel);
    tokio::pin!(stream);
    let mut events = Vec::new();
    let mut err = None;
    while let Some(item) = stream.next().await {
        match item {
            Ok(ev) => events.push(ev),
            Err(e) => {
                err = Some(e.to_string());
                break;
            }
        }
    }
    (events, err)
}

fn count_progress(events: &[DagExecutionEvent]) -> usize {
    events
        .iter()
        .filter(|e| matches!(e, DagExecutionEvent::Progress { .. }))
        .count()
}

#[tokio::test]
async fn silent_node_emits_heartbeats_and_completes() {
    let liveness = LivenessSettings {
        heartbeat_interval: Some(Duration::from_millis(100)),
        idle_timeout: None,
    };
    let uc = use_case("sleepy", Arc::new(SleepyNode { millis: 350 }), liveness);
    let (events, err) = drain(uc, single_node_graph("sleepy"), None).await;

    assert!(err.is_none(), "no error expected, got {:?}", err);
    let beats = count_progress(&events);
    assert!(
        beats >= 2,
        "expected >= 2 heartbeats during 350ms silence, got {}",
        beats
    );
    for e in &events {
        if let DagExecutionEvent::Progress { node_id, .. } = e {
            assert_eq!(node_id, "n1");
        }
    }
    assert!(
        events
            .iter()
            .any(|e| matches!(e, DagExecutionEvent::NodeFinish { .. })),
        "node must still finish normally"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, DagExecutionEvent::GraphFinish { .. })),
        "graph must finish normally"
    );
}

#[tokio::test]
async fn real_activity_resets_the_heartbeat_clock() {
    // Events every 80ms, heartbeat at 200ms of silence → never beats.
    let liveness = LivenessSettings {
        heartbeat_interval: Some(Duration::from_millis(200)),
        idle_timeout: None,
    };
    let uc = use_case(
        "chatty",
        Arc::new(ChattyNode {
            tick_millis: 80,
            ticks: 6,
        }),
        liveness,
    );
    let (events, err) = drain(uc, single_node_graph("chatty"), None).await;

    assert!(err.is_none());
    assert_eq!(
        count_progress(&events),
        0,
        "chatty node must not trigger heartbeats"
    );
    assert!(events
        .iter()
        .any(|e| matches!(e, DagExecutionEvent::GraphFinish { .. })));
}

#[tokio::test]
async fn disabled_liveness_changes_nothing() {
    let uc = use_case(
        "sleepy",
        Arc::new(SleepyNode { millis: 300 }),
        LivenessSettings::disabled(),
    );
    let (events, err) = drain(uc, single_node_graph("sleepy"), None).await;

    assert!(err.is_none());
    assert_eq!(count_progress(&events), 0);
    assert!(events
        .iter()
        .any(|e| matches!(e, DagExecutionEvent::GraphFinish { .. })));
}

#[tokio::test]
async fn hung_node_is_aborted_after_idle_timeout() {
    let liveness = LivenessSettings {
        heartbeat_interval: Some(Duration::from_millis(100)),
        idle_timeout: Some(Duration::from_millis(400)),
    };
    // Node "hangs" for 30s; the watchdog must kill the run at ~400ms.
    let uc = use_case("sleepy", Arc::new(SleepyNode { millis: 30_000 }), liveness);
    let started = std::time::Instant::now();
    let (events, err) = drain(uc, single_node_graph("sleepy"), None).await;

    // The idle watchdog fails the stream via a terminal `Error` event (not a
    // stream-level `Err`) — see run_use_case.rs's idle-abort arm for why
    // `Err(...)?` can't be used inside this `select!` arm.
    assert!(
        err.is_none(),
        "idle-abort must not surface as a stream Err: {:?}",
        err
    );
    let msg = events
        .iter()
        .find_map(|e| match e {
            DagExecutionEvent::Error { message } => Some(message.clone()),
            _ => None,
        })
        .expect("stream must end with a terminal Error event");
    assert!(msg.contains("n1"), "error must name the node: {}", msg);
    assert!(
        msg.contains("no events"),
        "error must describe the silence: {}",
        msg
    );
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "abort must happen at the idle deadline, not at node completion"
    );
    assert!(
        count_progress(&events) >= 1,
        "heartbeats must precede the abort"
    );
}

#[tokio::test]
async fn activity_prevents_idle_abort() {
    // Events every 80ms, idle deadline 300ms → node survives well past 300ms total.
    let liveness = LivenessSettings {
        heartbeat_interval: None,
        idle_timeout: Some(Duration::from_millis(300)),
    };
    let uc = use_case(
        "chatty",
        Arc::new(ChattyNode {
            tick_millis: 80,
            ticks: 8,
        }),
        liveness,
    );
    let (events, err) = drain(uc, single_node_graph("chatty"), None).await;

    assert!(
        err.is_none(),
        "chatty node must never be idle-aborted: {:?}",
        err
    );
    assert!(events
        .iter()
        .any(|e| matches!(e, DagExecutionEvent::GraphFinish { .. })));
}

#[tokio::test]
async fn user_cancel_wins_over_idle_watchdog() {
    let liveness = LivenessSettings {
        heartbeat_interval: None,
        idle_timeout: Some(Duration::from_millis(800)),
    };
    let uc = use_case("sleepy", Arc::new(SleepyNode { millis: 30_000 }), liveness);
    let token = tokio_util::sync::CancellationToken::new();
    let t = token.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(150)).await;
        t.cancel();
    });
    let (events, err) = drain(uc, single_node_graph("sleepy"), Some(token)).await;

    assert!(
        err.is_none(),
        "cancel must yield Cancelled, not an error: {:?}",
        err
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, DagExecutionEvent::Cancelled { .. })),
        "terminal event must be Cancelled"
    );
}
