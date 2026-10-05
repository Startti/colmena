//! Test doubles shared by the small-file characterisation tests: a recording
//! Python executor, an empty node registry, a one-object storage and a
//! `DagToolExecutor` whose catalog holds a single attachment.
#![allow(dead_code)]

use async_trait::async_trait;
use colmena::dag_engine::application::ports::NodeRegistryPort;
use colmena::dag_engine::domain::node::ExecutableNode;
use colmena::dag_engine::domain::python_executor::{
    ExecutorKind, PythonExecutor, PythonRunError, PythonRunRequest, PythonRunResult,
};
use colmena::dag_engine::infrastructure::dag_tool_executor::DagToolExecutor;
use colmena::llm::domain::attachments::AttachmentSource;
use colmena::llm::domain::{ConversationAttachment, ProviderKind};
use colmena::storage::domain::{
    OutputStorageRepository, StorageError, StoreRequest, StoredBytes, StoredOutput, StoredStream,
};
use serde_json::json;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

// ── Test doubles ────────────────────────────────────────────────────────

/// Records every request it receives and answers with a fixed reply.
pub struct Recording {
    seen: Mutex<Vec<PythonRunRequest>>,
    reply: Result<PythonRunResult, PythonRunError>,
}

impl Recording {
    pub fn answering(reply: Result<PythonRunResult, PythonRunError>) -> Arc<Self> {
        Arc::new(Self {
            seen: Mutex::new(Vec::new()),
            reply,
        })
    }

    pub fn ok() -> Arc<Self> {
        Self::answering(Ok(PythonRunResult {
            output: Some(json!({"rows": 2})),
            stdout: "(2, 3)\n".to_string(),
        }))
    }

    pub fn requests(&self) -> Vec<PythonRunRequest> {
        self.seen.lock().unwrap().clone()
    }
}

#[async_trait]
impl PythonExecutor for Recording {
    fn kind(&self) -> ExecutorKind {
        ExecutorKind::InProcess
    }

    async fn run(&self, req: PythonRunRequest) -> Result<PythonRunResult, PythonRunError> {
        self.seen.lock().unwrap().push(req);
        self.reply.clone()
    }
}

pub struct EmptyRegistry;

impl NodeRegistryPort for EmptyRegistry {
    fn get_node(&self, _: &str) -> Option<Arc<dyn ExecutableNode>> {
        None
    }

    fn get_all_nodes(&self) -> HashMap<String, Arc<dyn ExecutableNode>> {
        HashMap::new()
    }
}

/// Serves one object whatever key is asked for.
pub struct OneObject {
    bytes: Vec<u8>,
    mime: String,
    filename: String,
}

#[async_trait]
impl OutputStorageRepository for OneObject {
    async fn store(&self, _req: StoreRequest) -> Result<StoredOutput, StorageError> {
        Err(StorageError::InvalidInput("not used".into()))
    }

    async fn read(&self, _key: &str) -> Result<StoredBytes, StorageError> {
        Ok(StoredBytes {
            bytes: self.bytes.clone(),
            mime_type: self.mime.clone(),
            filename: self.filename.clone(),
        })
    }

    async fn read_stream(&self, _key: &str) -> Result<StoredStream, StorageError> {
        Err(StorageError::InvalidInput("not used".into()))
    }

    async fn delete(&self, _key: &str) -> Result<(), StorageError> {
        Ok(())
    }
}

pub fn catalog_row(mime: &str, filename: &str) -> ConversationAttachment {
    ConversationAttachment {
        agent_session_id: "agent_1".to_string(),
        document_id: "doc-1".to_string(),
        provider: ProviderKind::OpenAi,
        provider_file_id: String::new(),
        mime_type: mime.to_string(),
        filename: filename.to_string(),
        size_bytes: None,
        label: None,
        description: None,
        source: AttachmentSource::Inline,
        registered_at: chrono::Utc::now(),
        refreshed_at: chrono::Utc::now(),
        storage_key: Some("sk-1".to_string()),
        origin: Some("user_upload".to_string()),
        last_used_at: None,
    }
}

/// An executor whose catalog holds `doc-1`, backed by `bytes`.
pub fn executor_with(bytes: Vec<u8>, mime: &str, filename: &str) -> DagToolExecutor {
    let storage = OneObject {
        bytes,
        mime: mime.to_string(),
        filename: filename.to_string(),
    };
    DagToolExecutor::new(Arc::new(EmptyRegistry), HashMap::new())
        .with_attachments(vec![catalog_row(mime, filename)])
        .with_attachment_storage(Arc::new(storage))
}
