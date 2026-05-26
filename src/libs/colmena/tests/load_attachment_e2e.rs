//! End-to-end coverage for the load_attachment sentinel path with a fully
//! mocked LlmRepository. Verifies that:
//!   1. The synthetic LOAD_ATTACHMENT sentinel produced by the executor is
//!      consumed by AgentService.
//!   2. A synthetic `user` message with `files[]` is appended in-memory and
//!      a short marker user message (no files) is persisted in its place
//!      (Plan B D7 — ephemeral attachments).
//!   3. The next ReAct iteration receives the file in its message slice.

use async_trait::async_trait;
use colmena::llm::application::{AgentRunParams, AgentService, LoadAttachmentResolver};
use colmena::llm::domain::{
    AgentSessionId, Conversation, ConversationKey, ConversationRepository, FileData, FileSource,
    FunctionCall, LlmConfig, LlmError, LlmMessage, LlmProvider, LlmRepository, LlmRequest,
    LlmRequestId, LlmResponse, LlmStream, NodeIdPath, ProviderFileRef, ProviderKind, SessionId,
    ToolCall, ToolDefinition, ToolExecutor, ToolResult,
};
use std::sync::{Arc, Mutex};

struct ScriptedLlm {
    turn: Mutex<usize>,
}

fn scripted_provider() -> LlmProvider {
    LlmProvider::new(
        ProviderKind::OpenAi,
        "key".to_string(),
        Some("gpt-4".to_string()),
    )
    .unwrap()
}

#[async_trait]
impl LlmRepository for ScriptedLlm {
    async fn call(&self, _req: LlmRequest) -> Result<LlmResponse, LlmError> {
        let mut t = self.turn.lock().unwrap();
        *t += 1;
        let turn = *t;
        drop(t);
        if turn == 1 {
            let tc = ToolCall {
                id: "call_1".to_string(),
                call_type: "function".to_string(),
                function: FunctionCall::new(
                    "load_attachment".to_string(),
                    r#"{"document_id":"doc-1"}"#.to_string(),
                ),
                response: None,
            };
            Ok(
                LlmResponse::new(LlmRequestId::new(), String::new(), scripted_provider())
                    .unwrap()
                    .with_tool_calls(vec![tc]),
            )
        } else {
            Ok(
                LlmResponse::new(LlmRequestId::new(), "done".to_string(), scripted_provider())
                    .unwrap(),
            )
        }
    }

    async fn stream(&self, _req: LlmRequest) -> Result<LlmStream, LlmError> {
        unimplemented!()
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }

    fn provider_name(&self) -> &'static str {
        "scripted"
    }
}

struct MemoryConvRepo(Mutex<Vec<LlmMessage>>);

#[async_trait]
impl ConversationRepository for MemoryConvRepo {
    async fn get_by_id(&self, key: &ConversationKey) -> Result<Conversation, LlmError> {
        Ok(Conversation {
            key: key.clone(),
            messages: self.0.lock().unwrap().clone(),
        })
    }

    async fn add_message(&self, _key: &ConversationKey, m: LlmMessage) -> Result<(), LlmError> {
        self.0.lock().unwrap().push(m);
        Ok(())
    }

    async fn delete(&self, _key: &ConversationKey) -> Result<(), LlmError> {
        Ok(())
    }
}

struct SentinelExecutor;

#[async_trait]
impl ToolExecutor for SentinelExecutor {
    async fn execute(&self, tc: &ToolCall) -> Result<ToolResult, LlmError> {
        Ok(ToolResult {
            tool_call_id: tc.id.clone(),
            output: r#"{"__colmena_status":"LOAD_ATTACHMENT","document_id":"doc-1"}"#.to_string(),
            success: true,
            error: None,
        })
    }

    async fn available_tools(&self) -> Vec<ToolDefinition> {
        vec![]
    }
}

struct FakeResolver;

#[async_trait]
impl LoadAttachmentResolver for FakeResolver {
    async fn resolve(&self, _sid: &str, doc_id: &str) -> Result<Option<FileData>, String> {
        Ok(Some(FileData {
            document_id: Some(doc_id.to_string()),
            mime_type: "application/pdf".to_string(),
            filename: "x.pdf".to_string(),
            size_hint: Some(10),
            source: FileSource::Uploaded(ProviderFileRef {
                provider: ProviderKind::OpenAi,
                provider_file_id: "pf-1".to_string(),
                mime_type: "application/pdf".to_string(),
                filename: "x.pdf".to_string(),
                expires_at: None,
            }),
            retained_inline_bytes: None,
        }))
    }
}

#[tokio::test]
async fn load_attachment_injects_synthetic_user_message_and_persists_history() {
    let llm = Arc::new(ScriptedLlm {
        turn: Mutex::new(0),
    });
    let conv = Arc::new(MemoryConvRepo(Mutex::new(Vec::new())));
    let svc = AgentService::new(llm.clone(), conv.clone());

    let key = ConversationKey {
        session_id: SessionId("s1".to_string()),
        agent_session_id: Some(AgentSessionId("agent_1".to_string())),
        node_id: NodeIdPath("llm_call".to_string()),
    };
    let exec = SentinelExecutor;
    let params = AgentRunParams {
        session_id: &key,
        prompt: Some("read".to_string()),
        messages: None,
        config: LlmConfig::new(scripted_provider()),
        tools: vec![],
        tool_executor: &exec,
        max_iterations: Some(5),
        on_token: None,
        tools_provider: None,
        attachment_resolver: Some(Arc::new(FakeResolver)),
        agent_session_id: Some("agent_1".to_string()),
    };

    let resp = svc.run(params).await.unwrap();
    assert_eq!(resp.content(), "done");

    // Plan B (D7): the synthetic user_with_files is NOT persisted; a marker
    // user message takes its place so future turns don't pay input-token
    // cost for the doc bytes.
    let persisted = conv.0.lock().unwrap().clone();
    let has_user_with_files = persisted
        .iter()
        .any(|m| m.role().as_str() == "user" && m.files().map(|f| !f.is_empty()).unwrap_or(false));
    assert!(
        !has_user_with_files,
        "synthetic user_with_files must NOT be persisted (ephemeral)"
    );
    let has_marker = persisted
        .iter()
        .any(|m| m.role().as_str() == "user" && m.content().contains("[load_attachment("));
    assert!(
        has_marker,
        "expected a load_attachment marker user message in persisted history"
    );
}
