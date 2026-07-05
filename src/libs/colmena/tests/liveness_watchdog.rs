//! Liveness watchdog tests: Progress heartbeats during silent nodes and
//! (Task 5) idle-abort of hung nodes. Pure in-memory — stub registry, no DB.

use async_trait::async_trait;
use colmena::dag_engine::application::liveness::LivenessSettings;
use colmena::dag_engine::application::ports::NodeRegistryPort;
use colmena::dag_engine::application::run_use_case::DagRunUseCase;
use colmena::dag_engine::domain::error::DagError;
use colmena::dag_engine::domain::events::DagExecutionEvent;
use colmena::dag_engine::domain::graph::Graph;
use colmena::dag_engine::domain::node::{ExecutableNode, NodeInputs};
use colmena::dag_engine::domain::observer::{ExecutionObserver, NodeEvent};
use colmena::dag_engine::domain::state::{DagRunState, DagRunStatus, DagStateRepository};
use futures::StreamExt;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::error::Error as StdError;
use std::sync::{Arc, Mutex};
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

/// Emits only *turn-boundary* events (wrapped as `SubgraphChildEvent`, exactly
/// like a nested sub-agent's LlmMessageStart/Finish/LlmUsage bubbling up) every
/// `tick_millis`, `ticks` times. These advance the idle-abort clock (`last_any`)
/// but must NOT reset the heartbeat clock (`last_forwarded`) — so the run keeps
/// heart-beating while never being idle-aborted. Regression for Fase E.
struct BorderChatterNode {
    tick_millis: u64,
    ticks: u32,
}

#[async_trait]
impl ExecutableNode for BorderChatterNode {
    async fn execute(
        &self,
        _inputs: &NodeInputs,
        _config: &Value,
        _state: &mut Value,
        observer: Option<Arc<dyn ExecutionObserver>>,
    ) -> Result<Value, Box<dyn StdError + Send + Sync>> {
        // Rotate through the three boundary/accounting events, each wrapped in a
        // SubgraphChildEvent (as a nested sub-agent would deliver them).
        let boundary_events = [
            DagExecutionEvent::LlmMessageStart {
                node_id: "sub".to_string(),
            },
            DagExecutionEvent::LlmUsage {
                node_id: "sub".to_string(),
                prompt_tokens: 1,
                completion_tokens: 1,
                thinking_tokens: None,
                cache_read_tokens: None,
                cache_write_tokens: None,
            },
            DagExecutionEvent::LlmMessageFinish {
                node_id: "sub".to_string(),
                usage: None,
            },
        ];
        for i in 0..self.ticks {
            tokio::time::sleep(Duration::from_millis(self.tick_millis)).await;
            if let Some(obs) = &observer {
                let ev = &boundary_events[(i as usize) % boundary_events.len()];
                let raw = serde_json::to_value(ev).unwrap();
                obs.on_event(NodeEvent::SubgraphChildEvent(raw));
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

/// Fase E regression: a node whose only activity is nested turn-boundary events
/// (LlmMessageStart/Finish/LlmUsage wrapped as SubgraphChildEvent) must STILL
/// emit heartbeats (because `last_forwarded` is not reset by boundaries) AND must
/// NOT be idle-aborted (because `last_any` IS reset by them). Before the two-clock
/// split, the single `last_activity` was reset by these events, suppressing the
/// heartbeat while they never XADDed downstream → false "Stream timeout".
#[tokio::test]
async fn border_only_activity_still_heartbeats_and_never_idle_aborts() {
    // heartbeat 200ms < idle 800ms; boundary events every 100ms for ~1.2s.
    let liveness = LivenessSettings {
        heartbeat_interval: Some(Duration::from_millis(200)),
        idle_timeout: Some(Duration::from_millis(800)),
    };
    let uc = use_case(
        "border",
        Arc::new(BorderChatterNode {
            tick_millis: 100,
            ticks: 12,
        }),
        liveness,
    );
    let (events, err) = drain(uc, single_node_graph("border"), None).await;

    assert!(
        err.is_none(),
        "boundary-only activity must never be idle-aborted (last_any advances): {:?}",
        err
    );
    assert!(
        count_progress(&events) >= 2,
        "heartbeat must still fire during boundary-only activity (last_forwarded is not reset), got {} beats",
        count_progress(&events)
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, DagExecutionEvent::GraphFinish { .. })),
        "graph must finish normally"
    );
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

    // The idle watchdog fails the stream via a stream-level `Err` (not an
    // `Ok(DagExecutionEvent::Error { .. })` item) — this is what every drain-
    // style consumer (`ColmenaEngine::run_dag`, `run_subgraph`/
    // `resume_subgraph`) is written to handle. The `select!` arm itself can't
    // use `Err(...)?` directly (its per-arm async block returns `()`), so it
    // stashes the message and breaks out of the loop, where it's raised with
    // `?` — see run_use_case.rs's idle-abort arm.
    let msg = err.expect("idle-abort must surface as a stream-level Err");
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

/// Emits `LlmToolCallStart` for a "stuck_tool" call, then hangs forever
/// (well past any test timeout) without ever finishing the call or emitting
/// another event. Stand-in for a hung tool call so the idle-abort message's
/// tool-suffix and the FAILED-state persistence can be pinned down together.
struct StuckToolNode;

#[async_trait]
impl ExecutableNode for StuckToolNode {
    async fn execute(
        &self,
        _inputs: &NodeInputs,
        _config: &Value,
        _state: &mut Value,
        observer: Option<Arc<dyn ExecutionObserver>>,
    ) -> Result<Value, Box<dyn StdError + Send + Sync>> {
        if let Some(obs) = &observer {
            obs.on_event(NodeEvent::LlmToolCallStart {
                tool_id: "call_1".to_string(),
                tool_name: "stuck_tool".to_string(),
                tool_args: "{}".to_string(),
            });
        }
        tokio::time::sleep(Duration::from_secs(30)).await;
        Ok(json!({"ok": true}))
    }
    fn schema(&self) -> Value {
        json!({})
    }
}

/// Minimal in-memory `DagStateRepository` stub. Only `save` is exercised by
/// the idle-abort path; the other required methods are unreachable in these
/// tests and return the emptiest value the trait allows.
#[derive(Default)]
struct StubStateRepository {
    saved: Mutex<Vec<DagRunState>>,
}

#[async_trait]
impl DagStateRepository for StubStateRepository {
    async fn get_by_id(&self, _session_id: &str) -> Result<Option<DagRunState>, DagError> {
        Ok(None)
    }

    async fn save(&self, state: &DagRunState) -> Result<(), DagError> {
        self.saved.lock().unwrap().push(state.clone());
        Ok(())
    }

    async fn find_resume_entry(&self, _agent_session_id: &str) -> Result<Option<String>, DagError> {
        Ok(None)
    }

    async fn find_suspended_child(
        &self,
        _parent_session_id: &str,
    ) -> Result<Option<String>, DagError> {
        Ok(None)
    }
}

#[tokio::test]
async fn idle_abort_persists_failed_state_and_names_the_stuck_tool() {
    let liveness = LivenessSettings {
        heartbeat_interval: None,
        idle_timeout: Some(Duration::from_millis(400)),
    };
    let mut nodes: HashMap<String, Arc<dyn ExecutableNode>> = HashMap::new();
    nodes.insert("stuck".to_string(), Arc::new(StuckToolNode));
    let registry = Arc::new(TestRegistry { nodes });
    let repo = Arc::new(StubStateRepository::default());
    let uc = DagRunUseCase::new(registry, Some(repo.clone() as Arc<dyn DagStateRepository>))
        .with_liveness(liveness);

    let (_events, err) = drain(uc, single_node_graph("stuck"), None).await;

    let msg = err.expect("idle-abort of a hung tool call must surface as a stream-level Err");
    assert!(
        msg.contains("stuck_tool"),
        "error must name the in-flight tool: {}",
        msg
    );
    assert!(
        msg.contains("in flight"),
        "error must mark the tool as in flight: {}",
        msg
    );

    let saved = repo.saved.lock().unwrap();
    assert!(
        saved.iter().any(|s| s.status == DagRunStatus::Failed),
        "idle abort must persist a FAILED state, got statuses: {:?}",
        saved.iter().map(|s| s.status.clone()).collect::<Vec<_>>()
    );
}
