//! Test harness: run the real `llm_call` node, as the engine's registry builds
//! it, against a recording model. Shared by the large tabular turn tests.

use crate::dag_engine::domain::state::DagTaskMemoryRepository;
use crate::dag_engine::infrastructure::persistence::PostgresDagStateRepository;
use crate::dag_engine::infrastructure::pool_registry::{PgPoolRegistry, PoolConfig};
use crate::dag_engine::infrastructure::registry::HashMapNodeRegistry;
use crate::dag_engine::infrastructure::sql_port_factory::SqlPortFactory;
use crate::llm::domain::{LlmError, LlmRepository, LlmRequest, LlmResponse, LlmStream};
use crate::llm::infrastructure::{
    ConversationRepositoryFactory, OverrideGuard, ScriptedAdapter, ScriptedResponse,
};
use crate::storage::domain::{
    OutputStorageRepository, StorageError, StoreRequest, StoredBytes, StoredOutput, StoredStream,
};
use crate::storage::infrastructure::LocalCacheStorageAdapter;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// A model that answers "ok" and records what the node sent it.
pub(super) struct RecordingModel {
    inner: ScriptedAdapter,
    /// Every message body and every volatile system suffix, in order.
    texts: Mutex<Vec<String>>,
    files: AtomicUsize,
}

impl RecordingModel {
    pub(super) fn new(replies: usize) -> Arc<Self> {
        Self::scripted(
            (0..replies)
                .map(|_| ScriptedResponse::Text("ok".into()))
                .collect(),
        )
    }

    /// A model that plays `script`, one reply per call.
    pub(super) fn scripted(script: Vec<ScriptedResponse>) -> Arc<Self> {
        Arc::new(Self {
            inner: ScriptedAdapter::new(script),
            texts: Mutex::default(),
            files: AtomicUsize::new(0),
        })
    }

    fn record(&self, request: &LlmRequest) {
        let mut texts = self.texts.lock().unwrap();
        for m in request.messages() {
            texts.push(m.content().to_string());
            self.files
                .fetch_add(m.files().map_or(0, |f| f.len()), Ordering::SeqCst);
        }
        if let Some(suffix) = request.config().volatile_system_suffix() {
            texts.push(suffix.to_string());
        }
    }

    /// Everything the model was shown, as one string.
    pub(super) fn seen(&self) -> String {
        self.texts.lock().unwrap().join("\n")
    }

    /// Files attached to any message the model received.
    pub(super) fn files_seen(&self) -> usize {
        self.files.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl LlmRepository for RecordingModel {
    async fn call(&self, request: LlmRequest) -> Result<LlmResponse, LlmError> {
        self.record(&request);
        self.inner.call(request).await
    }
    async fn stream(&self, request: LlmRequest) -> Result<LlmStream, LlmError> {
        self.record(&request);
        self.inner.stream(request).await
    }
    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
    fn provider_name(&self) -> &'static str {
        "recording"
    }
}

/// The node registry an engine builds, with the switch off until set.
pub(super) fn registry() -> Arc<HashMapNodeRegistry> {
    registry_with_storage(None)
}

/// [`registry`] with the host's storage wired, as an engine does.
pub(super) fn registry_with_storage(
    storage: Option<Arc<dyn OutputStorageRepository>>,
) -> Arc<HashMapNodeRegistry> {
    let pools = Arc::new(PgPoolRegistry::new(PoolConfig::defaults()));
    let repos = Arc::new(ConversationRepositoryFactory::new(pools.clone()));
    // The reactor node insists on a task-memory store. The real Postgres one on
    // a lazy pool never connects: `llm_call` does not touch it.
    let pool = sqlx::postgres::PgPoolOptions::new()
        .connect_lazy("postgres://unused:unused@127.0.0.1:1/unused")
        .expect("a lazy pool does not connect");
    let task_memory: Arc<dyn DagTaskMemoryRepository> =
        Arc::new(PostgresDagStateRepository::new(pool));
    HashMapNodeRegistry::new_with_secure_values(
        repos,
        Arc::new(SqlPortFactory::new(pools)),
        Some(task_memory),
        None,
        storage,
        None,
        None,
    )
}

/// One `llm_call` turn of agent session `agent_1` carrying `files`. The
/// attachment registry is the SQLite database at `db_url`.
pub(super) async fn run_turn(
    reg: &Arc<HashMapNodeRegistry>,
    db_url: &str,
    files: Vec<Value>,
    model: &Arc<RecordingModel>,
) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
    run_turn_with_tools(reg, db_url, files, Value::Null, model).await
}

/// [`run_turn`] with a `tool_configurations` block (`Null`: none).
pub(super) async fn run_turn_with_tools(
    reg: &Arc<HashMapNodeRegistry>,
    db_url: &str,
    files: Vec<Value>,
    tool_configurations: Value,
    model: &Arc<RecordingModel>,
) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
    use crate::dag_engine::application::ports::NodeRegistryPort;
    let node = reg.get_node("llm_call").expect("llm_call is registered");
    let _guard = OverrideGuard::install(model.clone());
    let inputs: HashMap<String, Value> = HashMap::from([
        ("__colmena_session_id".to_string(), json!("s1")),
        ("__colmena_agent_session_id".to_string(), json!("agent_1")),
        ("files".to_string(), Value::Array(files)),
    ]);
    let mut config = json!({
        "provider": "openai", "model": "m", "api_key": "k", "stream": false,
        "prompt": "go", "connection_url": db_url,
    });
    if !tool_configurations.is_null() {
        config["tool_configurations"] = tool_configurations;
    }
    node.execute(&inputs, &config, &mut Value::Null, None).await
}

/// A real in-memory storage adapter that counts what is done to it.
#[derive(Default)]
pub(super) struct CountingStorage {
    inner: LocalCacheStorageAdapter,
    reads: AtomicUsize,
}

impl CountingStorage {
    /// Whole-object and streamed reads together.
    pub(super) fn reads(&self) -> usize {
        self.reads.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl OutputStorageRepository for CountingStorage {
    async fn store(&self, req: StoreRequest) -> Result<StoredOutput, StorageError> {
        self.inner.store(req).await
    }
    async fn read(&self, key: &str) -> Result<StoredBytes, StorageError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.inner.read(key).await
    }
    async fn read_stream(&self, key: &str) -> Result<StoredStream, StorageError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.inner.read_stream(key).await
    }
    async fn delete(&self, key: &str) -> Result<(), StorageError> {
        self.inner.delete(key).await
    }
}
