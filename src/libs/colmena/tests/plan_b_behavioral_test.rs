//! Plan B — Task 7: integration tests for the two behavioral changes.
//!
//! Test one (D6 no-autoinject) — when attachments exist for a session, the
//! catalog block rendered from the registry MUST contain Plan B usage hints
//! (`load_attachment(...)` to read, `$attachment:...` to forward). This is the
//! at-integration-layer composition of `SqliteAttachmentRegistry` plus
//! `attachment_catalog::render_catalog`. The unit tests cover each piece in
//! isolation; this test confirms they compose correctly via a real SQLite
//! round-trip.
//!
//! Test two (D7 ephemeral load_attachment) — when the agent loop encounters a
//! `load_attachment` sentinel, only a short marker user message is persisted
//! to the conversation history; the synthetic `user_with_files` carrying the
//! bytes stays in-memory for the next LLM turn. The unit test in
//! `agent_service.rs` covers this with a mocked `ConversationRepository`; this
//! integration test re-verifies it against the real
//! `InMemoryConversationRepository` to confirm cross-module composition holds
//! (real repo, not a mockall::mock).
//!
//! No external services or API keys required — both tests are deterministic.

use std::sync::Arc;

use async_trait::async_trait;
use colmena::llm::application::attachment_catalog::render_catalog;
use colmena::llm::application::{AgentRunParams, AgentService, LoadAttachmentResolver};
use colmena::llm::domain::attachments::{
    AttachmentRegistry, AttachmentSource, UpsertAttachmentInput,
};
use colmena::llm::domain::tools::FunctionCall;
use colmena::llm::domain::{
    AgentSessionId, ConversationKey, ConversationRepository, FileData, FileSource, LlmConfig,
    LlmError, LlmProvider, LlmRepository, LlmRequest, LlmRequestId, LlmResponse, LlmStreamChunk,
    NodeIdPath, ProviderFileRef, ProviderKind, SessionId, ToolCall, ToolDefinition, ToolExecutor,
    ToolResult,
};
use colmena::llm::infrastructure::persistence::{
    InMemoryConversationRepository, SqliteAttachmentRegistry,
};
use futures::stream;
use std::pin::Pin;
use std::sync::Mutex;

// ---------------------------------------------------------------------------
// Test 1 — D6 composition: registry round-trip + catalog rendering.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn catalog_renders_from_real_registry_with_plan_b_usage_hints() {
    // Real SQLite (in-memory) — exercises persistence + retrieval round-trip.
    let registry = SqliteAttachmentRegistry::new("sqlite::memory:")
        .await
        .expect("sqlite registry ok");

    let agent_session_id = "plan_b_d6_session";

    // Seed two attachments with different origins to exercise the catalog's
    // origin-humanization branches.
    registry
        .upsert(UpsertAttachmentInput {
            agent_session_id: agent_session_id.to_string(),
            document_id: "doc-uploaded".to_string(),
            provider: ProviderKind::OpenAi,
            provider_file_id: "pf-1".to_string(),
            mime_type: "application/pdf".to_string(),
            filename: "contract.pdf".to_string(),
            size_bytes: Some(2048),
            label: None,
            description: Some("Master service agreement".to_string()),
            source: AttachmentSource::Inline,
            storage_key: Some("sk-1".to_string()),
            origin: Some("user_upload".to_string()),
        })
        .await
        .expect("upsert uploaded ok");

    registry
        .upsert(UpsertAttachmentInput {
            agent_session_id: agent_session_id.to_string(),
            document_id: "doc-generated".to_string(),
            provider: ProviderKind::OpenAi,
            provider_file_id: "pf-2".to_string(),
            mime_type: "image/png".to_string(),
            filename: "rendered.png".to_string(),
            size_bytes: Some(8192),
            label: None,
            description: None,
            source: AttachmentSource::Path("sk-2".to_string()),
            storage_key: Some("sk-2".to_string()),
            origin: Some("generated_by:image_generation".to_string()),
        })
        .await
        .expect("upsert generated ok");

    // List back through the real registry — same path the LLM node uses to
    // build the catalog when assembling the system message.
    let attachments = registry
        .list_for_session(agent_session_id)
        .await
        .expect("list_for_session ok");
    assert_eq!(attachments.len(), 2, "expected two attachments in session");

    let catalog = render_catalog(&attachments).expect("catalog should render");

    // Plan B usage hints — these are the load-bearing strings the LLM relies
    // on to know how to invoke `load_attachment` and `$attachment:` forwards.
    assert!(
        catalog.contains("[doc-uploaded]"),
        "catalog must include the uploaded document_id; got:\n{catalog}"
    );
    assert!(
        catalog.contains("[doc-generated]"),
        "catalog must include the generated document_id; got:\n{catalog}"
    );
    assert!(
        catalog.contains("load_attachment(\"doc-uploaded\") to read"),
        "catalog must teach load_attachment usage; got:\n{catalog}"
    );
    assert!(
        catalog.contains("\"$attachment:doc-uploaded\" to forward"),
        "catalog must teach $attachment: forward usage; got:\n{catalog}"
    );
    assert!(
        catalog.contains("origin: uploaded by user"),
        "catalog must humanize user_upload origin; got:\n{catalog}"
    );
    assert!(
        catalog.contains("origin: generated by image_generation"),
        "catalog must humanize generated_by:* origins; got:\n{catalog}"
    );
}

// ---------------------------------------------------------------------------
// Test 2 — D7 cross-module: real conversation repo + AgentService::run.
// ---------------------------------------------------------------------------

/// Hand-rolled LLM stub. Replays a queued series of responses; the integration
/// test crate cannot use `mockall::automock` because `LlmRepository` only
/// derives it under `#[cfg(test)]`.
struct ScriptedLlm {
    responses: Mutex<Vec<LlmResponse>>,
}

impl ScriptedLlm {
    fn new(responses: Vec<LlmResponse>) -> Self {
        Self {
            responses: Mutex::new(responses),
        }
    }
}

#[async_trait]
impl LlmRepository for ScriptedLlm {
    async fn call(&self, _request: LlmRequest) -> Result<LlmResponse, LlmError> {
        let mut q = self.responses.lock().unwrap();
        if q.is_empty() {
            panic!("ScriptedLlm: ran out of scripted responses");
        }
        Ok(q.remove(0))
    }

    async fn stream(
        &self,
        _request: LlmRequest,
    ) -> Result<
        Pin<Box<dyn futures::Stream<Item = Result<LlmStreamChunk, LlmError>> + Send>>,
        LlmError,
    > {
        Ok(Box::pin(stream::empty()))
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }

    fn provider_name(&self) -> &'static str {
        "scripted"
    }
}

/// ToolExecutor that always returns the LOAD_ATTACHMENT sentinel — emulates a
/// `load_attachment` tool implementation handing control off to the agent
/// service's sentinel handler.
struct LoadAttachmentSentinelExec;

#[async_trait]
impl ToolExecutor for LoadAttachmentSentinelExec {
    async fn execute(&self, tc: &ToolCall) -> Result<ToolResult, LlmError> {
        Ok(ToolResult {
            tool_call_id: tc.id.clone(),
            output: r#"{"__colmena_status":"LOAD_ATTACHMENT","document_id":"doc-int-1"}"#
                .to_string(),
            success: true,
            error: None,
        })
    }
    async fn available_tools(&self) -> Vec<ToolDefinition> {
        vec![]
    }
}

/// Fake resolver returning a known FileData for `doc-int-1`.
struct FakeResolver;

#[async_trait]
impl LoadAttachmentResolver for FakeResolver {
    async fn resolve(
        &self,
        _agent_session_id: &str,
        document_id: &str,
    ) -> Result<Option<FileData>, String> {
        if document_id == "doc-int-1" {
            Ok(Some(FileData {
                document_id: Some(document_id.to_string()),
                mime_type: "application/pdf".to_string(),
                filename: "doc.pdf".to_string(),
                size_hint: Some(42),
                source: FileSource::Uploaded(ProviderFileRef {
                    provider: ProviderKind::OpenAi,
                    provider_file_id: "pf-int-1".to_string(),
                    mime_type: "application/pdf".to_string(),
                    filename: "doc.pdf".to_string(),
                    expires_at: None,
                }),
                retained_inline_bytes: None,
            }))
        } else {
            Ok(None)
        }
    }
}

#[tokio::test]
async fn agent_run_with_in_memory_repo_persists_marker_not_bytes() {
    // Real conversation repo (no mock).
    let conv_repo: Arc<InMemoryConversationRepository> =
        Arc::new(InMemoryConversationRepository::new());

    // Script:
    //   Turn 1: LLM emits a load_attachment tool call.
    //   Turn 2: LLM emits final text.
    let provider = LlmProvider::new(
        ProviderKind::OpenAi,
        "key".to_string(),
        Some("gpt-4".to_string()),
    )
    .unwrap();

    let tc = ToolCall {
        id: "call_int_1".to_string(),
        call_type: "function".to_string(),
        function: FunctionCall::new(
            "load_attachment".to_string(),
            r#"{"document_id":"doc-int-1"}"#.to_string(),
        ),
        response: None,
    };

    let turn1 = LlmResponse::new(
        LlmRequestId::from_string("req-int-1".to_string()).unwrap(),
        "".to_string(),
        provider.clone(),
    )
    .unwrap()
    .with_tool_calls(vec![tc]);

    let turn2 = LlmResponse::new(
        LlmRequestId::from_string("req-int-2".to_string()).unwrap(),
        "ok done".to_string(),
        provider.clone(),
    )
    .unwrap();

    let llm: Arc<dyn LlmRepository> = Arc::new(ScriptedLlm::new(vec![turn1, turn2]));
    let svc = AgentService::new(llm, conv_repo.clone());

    let session_key = ConversationKey {
        session_id: SessionId("s_int".to_string()),
        agent_session_id: Some(AgentSessionId("agent_int".to_string())),
        node_id: NodeIdPath("llm_call".to_string()),
    };

    let exec = LoadAttachmentSentinelExec;
    let params = AgentRunParams {
        session_id: &session_key,
        prompt: Some("please read the doc".to_string()),
        messages: None,
        config: LlmConfig::new(provider),
        tools: vec![],
        tool_executor: &exec,
        max_iterations: Some(5),
        on_token: None,
        tools_provider: None,
        attachment_resolver: Some(Arc::new(FakeResolver)),
        agent_session_id: Some("agent_int".to_string()),
    };

    let resp = svc.run(params).await.expect("agent run ok");
    assert_eq!(resp.content(), "ok done");

    // Inspect what the REAL conversation repo persisted.
    let persisted = conv_repo
        .get_by_id(&session_key)
        .await
        .expect("get_by_id ok")
        .messages;

    // Plan B (D7): no persisted user message may carry file bytes.
    let has_user_with_files = persisted
        .iter()
        .any(|m| m.role().as_str() == "user" && m.files().map(|f| !f.is_empty()).unwrap_or(false));
    assert!(
        !has_user_with_files,
        "persisted history must NOT contain user_with_files; messages: {:?}",
        persisted
            .iter()
            .map(|m| (m.role().as_str().to_string(), m.content().to_string()))
            .collect::<Vec<_>>()
    );

    // Plan B (D7): a marker user message referencing the document_id MUST be
    // persisted so a follow-up turn can recall the load_attachment occurred.
    let has_marker = persisted.iter().any(|m| {
        m.role().as_str() == "user"
            && m.content().contains("[load_attachment(")
            && m.content().contains("doc-int-1")
    });
    assert!(
        has_marker,
        "expected a load_attachment marker user message in persisted history; got: {:?}",
        persisted
            .iter()
            .map(|m| (m.role().as_str().to_string(), m.content().to_string()))
            .collect::<Vec<_>>()
    );
}
