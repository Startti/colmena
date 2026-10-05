//! Characterisation of the event sequence of a small-file tool run (slice C0b
//! of the large tabular files chain): the ordered stream parts and SSE frames
//! of one `attachment_run_python` call, which today contain no progress frame.
//!
//! Pins what the code does NOW; nothing here changes behaviour.

mod parallel_turn_model;
mod small_file_support;

use colmena::dag_engine::domain::events::DagExecutionEvent;
use colmena::dag_engine::infrastructure::nodes::llm_synthetic_tools::attachment_run_python::build_attachment_run_python_tool_definition;
use colmena::dag_engine::infrastructure::python_exec::scope;
use colmena::dag_engine::sse_mapper::SseMapper;
use colmena::llm::application::{AgentRunParams, AgentService};
use colmena::llm::domain::{
    AgentSessionId, ConversationKey, LlmConfig, LlmProvider, LlmStreamPart, NodeIdPath,
    ProviderKind, SessionId,
};
use colmena::llm::infrastructure::persistence::InMemoryConversationRepository;
use parallel_turn_model::{Call, ParallelTurnModel};
use serde_json::Value;
use small_file_support::{executor_with, Recording};
use std::sync::{Arc, Mutex};

const CSV_MIME: &str = "text/csv";

// ── C0.8: the event sequence of a small-file tool run ───────────────────

const RUN_CALLS: [Call; 1] = [(
    "call_run",
    "attachment_run_python",
    r#"{"attachment_id":"doc-1","code":"print(df.shape)"}"#,
)];

fn part_label(part: &LlmStreamPart) -> &'static str {
    match part {
        LlmStreamPart::Content(_) => "content",
        LlmStreamPart::ToolCallChunk(_) => "tool-call-chunk",
        LlmStreamPart::Usage(_) => "usage",
        LlmStreamPart::LlmToolCallStart(_) => "tool-call-start",
        LlmStreamPart::LlmToolCallFinish(_) => "tool-call-finish",
        LlmStreamPart::LlmMessageStart => "message-start",
        LlmStreamPart::LlmMessageFinish(_) => "message-finish",
        LlmStreamPart::ThinkingStart => "thinking-start",
        LlmStreamPart::ThinkingContent(_) => "thinking-content",
        LlmStreamPart::ThinkingEnd => "thinking-end",
        LlmStreamPart::UserMessageConsumed { .. } => "user-message-consumed",
    }
}

#[tokio::test]
async fn a_small_file_tool_run_emits_the_known_event_sequence_and_no_progress() {
    let model = Arc::new(ParallelTurnModel::new(&RUN_CALLS));
    let service = AgentService::new(
        model.clone(),
        Arc::new(InMemoryConversationRepository::new()),
    );
    let tools = executor_with(b"a,b\n1,2\n".to_vec(), CSV_MIME, "t.csv");
    let key = ConversationKey {
        session_id: SessionId("s1".to_string()),
        agent_session_id: Some(AgentSessionId("agent_1".to_string())),
        node_id: NodeIdPath("agent".to_string()),
    };
    let parts: Arc<Mutex<Vec<LlmStreamPart>>> = Arc::default();
    let sink = parts.clone();
    let params = AgentRunParams {
        session_id: &key,
        prompt: Some("analyse".to_string()),
        messages: None,
        config: LlmConfig::new(
            LlmProvider::new(ProviderKind::OpenAi, "key".into(), Some("gpt-4".into())).unwrap(),
        ),
        tools: vec![build_attachment_run_python_tool_definition()],
        tool_executor: &tools,
        max_tool_repeats: Some(5),
        max_turns: None,
        on_token: Some(Box::new(move |p| sink.lock().unwrap().push(p))),
        tools_provider: None,
        attachment_resolver: None,
        agent_session_id: Some("agent_1".to_string()),
        lazy_catalog_names: None,
    };

    let recording = Recording::ok();
    let response = scope(recording.clone(), service.run(params))
        .await
        .expect("the run finishes");
    assert_eq!(response.content(), "Listo.");
    assert_eq!(recording.requests().len(), 1, "the tool ran once");
    assert_eq!(
        model.results_seen(),
        ["call_run"],
        "the model read its result"
    );

    let parts = parts.lock().unwrap().clone();
    let labels: Vec<&str> = parts.iter().map(part_label).collect();
    assert_eq!(
        labels,
        [
            "message-start",
            "tool-call-chunk",
            "message-finish",
            "tool-call-start",
            "tool-call-finish",
            "message-start",
            "content",
            "message-finish",
        ]
    );

    // The same run as the SSE frames the client reads: the tool frames, in
    // order, and no `tool-progress` anywhere.
    let mut mapper = SseMapper::new();
    let mut frames: Vec<Value> = Vec::new();
    for part in &parts {
        let event = match part {
            LlmStreamPart::ToolCallChunk(c) => Some(DagExecutionEvent::LlmToolCall {
                node_id: "agent".to_string(),
                tool_id: c.id.clone(),
                tool_name: c.name.clone(),
                args_chunk: c.args_chunk.clone(),
            }),
            LlmStreamPart::LlmToolCallStart(tc) => Some(DagExecutionEvent::LlmToolCallStart {
                node_id: "agent".to_string(),
                tool_id: tc.id.clone(),
                tool_name: tc.function.name.clone(),
                tool_args: tc.function.arguments.clone(),
                child_scope: None,
            }),
            LlmStreamPart::LlmToolCallFinish(r) => Some(DagExecutionEvent::LlmToolCallFinish {
                node_id: "agent".to_string(),
                tool_id: r.tool_call_id.clone(),
                success: r.success,
                output: r.output.clone(),
                child_scope: None,
                cancelled: false,
            }),
            _ => None,
        };
        if let Some(event) = event {
            frames.extend(mapper.map(&event));
        }
    }
    let kinds: Vec<&str> = frames.iter().map(|f| f["type"].as_str().unwrap()).collect();
    assert_eq!(
        kinds,
        [
            "tool-input-start",
            "tool-input-delta",
            "tool-input-available",
            "tool-output-available"
        ]
    );
    assert!(!kinds.contains(&"tool-progress"));
    assert!(frames
        .iter()
        .all(|f| f["toolCallId"] == "call_run" && f.get("childScope").is_none()));
}
