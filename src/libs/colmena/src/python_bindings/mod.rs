mod crdt_documents;

use crate::llm::domain::{LlmError, LlmMessage, MessageRole, ProviderKind};
use crate::shared::infrastructure::{ConfigResolver, ServiceContainerFactory};
use futures::StreamExt;
use pyo3::prelude::*;
use pyo3::{
    create_exception,
    exceptions::{PyException, PyStopAsyncIteration},
    types::PyDict,
};
use pyo3_async_runtimes::tokio::future_into_py;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::Mutex;

create_exception!(colmena, LlmException, PyException);

impl From<LlmError> for PyErr {
    fn from(err: LlmError) -> PyErr {
        LlmException::new_err(err.to_string())
    }
}

#[pyclass]
struct PyLlmStream {
    stream: Arc<Mutex<crate::llm::domain::LlmStream>>,
}

#[pymethods]
impl PyLlmStream {
    fn __aiter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __anext__<'py>(slf: PyRefMut<'_, Self>, py: Python<'py>) -> PyResult<Option<Py<PyAny>>> {
        let stream = Arc::clone(&slf.stream);
        let future = async move {
            let mut stream = stream.lock().await;
            if let Some(result) = stream.next().await {
                match result {
                    Ok(chunk) => Ok(chunk.content().to_string()),
                    Err(e) => Err(LlmException::new_err(e.to_string())),
                }
            } else {
                Err(PyStopAsyncIteration::new_err(()))
            }
        };

        Ok(Some(future_into_py(py, future)?.into()))
    }
}

#[pyclass(from_py_object)]
#[derive(Clone, Default)]
pub struct LlmConfigOptions {
    #[pyo3(get, set)]
    pub api_key: Option<String>,
    #[pyo3(get, set)]
    pub model: Option<String>,
    #[pyo3(get, set)]
    pub temperature: Option<f32>,
    #[pyo3(get, set)]
    pub max_tokens: Option<u32>,
    #[pyo3(get, set)]
    pub top_p: Option<f32>,
    #[pyo3(get, set)]
    pub frequency_penalty: Option<f32>,
    #[pyo3(get, set)]
    pub presence_penalty: Option<f32>,
}

#[pymethods]
impl LlmConfigOptions {
    #[new]
    fn new() -> Self {
        Default::default()
    }
}

#[pyclass]
pub struct ColmenaLlm {
    containers: HashMap<String, Arc<crate::shared::infrastructure::ServiceContainer>>,
}

#[pymethods]
impl ColmenaLlm {
    #[new]
    pub fn new() -> PyResult<Self> {
        ConfigResolver::load_env()?;
        let mut containers = HashMap::new();
        for (provider, container) in ServiceContainerFactory::create_all() {
            containers.insert(provider.to_string(), Arc::new(container));
        }
        Ok(Self { containers })
    }

    #[pyo3(signature = (messages, provider, options=None))]
    pub fn call(
        &self,
        py: Python,
        messages: Vec<Bound<'_, PyDict>>,
        provider: &str,
        options: Option<LlmConfigOptions>,
    ) -> PyResult<String> {
        let provider_kind = ProviderKind::from_str(provider)?;
        let container = self
            .containers
            .get(provider)
            .ok_or_else(|| LlmException::new_err(format!("Provider {} not found", provider)))?;

        // Parse messages from dictionaries
        let llm_messages: Result<Vec<LlmMessage>, PyErr> = messages
            .into_iter()
            .enumerate()
            .map(|(i, msg_dict)| {
                let role_str: String = match msg_dict.get_item("role")? {
                    Some(role_val) => role_val.extract()?,
                    None => {
                        return Err(LlmException::new_err(format!(
                            "Missing 'role' key in message: {}",
                            i + 1
                        )))
                    }
                };

                let content: String = match msg_dict.get_item("content")? {
                    Some(content_val) => content_val.extract()?,
                    None => {
                        return Err(LlmException::new_err(format!(
                            "Missing 'content' key in message {}",
                            i + 1
                        )))
                    }
                };

                let role = MessageRole::from_str(&role_str)
                    .map_err(|e| LlmException::new_err(e.to_string()))?;

                LlmMessage::new(role, content).map_err(|e| LlmException::new_err(e.to_string()))
            })
            .collect();

        let options = options.unwrap_or_default();
        let config = ConfigResolver::create_config(
            provider_kind,
            options.api_key,
            options.model,
            options.temperature,
            options.max_tokens,
            options.top_p,
            options.frequency_penalty,
            options.presence_penalty,
        )?;

        py.detach(move || {
            let rt =
                tokio::runtime::Runtime::new().map_err(|e| LlmException::new_err(e.to_string()))?;
            rt.block_on(async {
                container
                    .llm_call
                    .execute(llm_messages?, config)
                    .await
                    .map(|res| res.content().to_string())
                    .map_err(PyErr::from)
            })
        })
    }

    #[pyo3(signature = (messages, provider, options=None))]
    pub fn stream(
        &self,
        py: Python,
        messages: Vec<Bound<'_, PyDict>>,
        provider: &str,
        options: Option<LlmConfigOptions>,
    ) -> PyResult<Py<PyAny>> {
        let provider_kind = ProviderKind::from_str(provider)?;
        let container = self
            .containers
            .get(provider)
            .cloned()
            .ok_or_else(|| LlmException::new_err(format!("Provider {} not found", provider)))?;

        // Parse messages from dictionaries
        let llm_messages: Result<Vec<LlmMessage>, PyErr> = messages
            .into_iter()
            .enumerate()
            .map(|(i, msg_dict)| {
                let role_str: String = match msg_dict.get_item("role")? {
                    Some(role_val) => role_val.extract()?,
                    None => {
                        return Err(LlmException::new_err(format!(
                            "Missing 'role' key in message: {}",
                            i + 1
                        )))
                    }
                };

                let content: String = match msg_dict.get_item("content")? {
                    Some(content_val) => content_val.extract()?,
                    None => {
                        return Err(LlmException::new_err(format!(
                            "Missing 'content' key in message {}",
                            i + 1
                        )))
                    }
                };

                let role = MessageRole::from_str(&role_str)
                    .map_err(|e| LlmException::new_err(e.to_string()))?;

                LlmMessage::new(role, content).map_err(|e| LlmException::new_err(e.to_string()))
            })
            .collect();
        let llm_messages = llm_messages?;

        let options = options.unwrap_or_default();
        let config = ConfigResolver::create_config(
            provider_kind,
            options.api_key,
            options.model,
            options.temperature,
            options.max_tokens,
            options.top_p,
            options.frequency_penalty,
            options.presence_penalty,
        )?;

        future_into_py(py, async move {
            let stream_result = container.llm_stream.execute(llm_messages, config).await;

            match stream_result {
                Ok(stream) => {
                    let py_stream = PyLlmStream {
                        stream: Arc::new(Mutex::new(stream)),
                    };
                    Ok(py_stream)
                }
                Err(e) => Err(PyErr::from(e)),
            }
        })
        .map(|bound| bound.into())
    }

    pub fn health_check(&self, py: Python, provider: &str) -> PyResult<bool> {
        let container = self
            .containers
            .get(provider)
            .ok_or_else(|| LlmException::new_err(format!("Provider {} not found", provider)))?;
        py.detach(|| {
            let rt =
                tokio::runtime::Runtime::new().map_err(|e| LlmException::new_err(e.to_string()))?;
            rt.block_on(async {
                container
                    .llm_health_check
                    .execute()
                    .await
                    .map(|status| status.is_healthy())
                    .map_err(PyErr::from)
            })
        })
    }

    pub fn get_providers(&self) -> PyResult<Vec<String>> {
        Ok(self.containers.keys().cloned().collect())
    }
}

// ==================== DAG Engine Bindings ====================

create_exception!(colmena, DagException, PyException);

/// Owned, `'static` stream of SSE-mapped DAG parts (each a `serde_json::Value`).
type DagPartStream = std::pin::Pin<
    Box<
        dyn futures::Stream<
                Item = Result<serde_json::Value, crate::dag_engine::domain::error::DagError>,
            > + Send,
    >,
>;

/// Async iterator over a running DAG's SSE-mapped events. Each `__anext__`
/// yields the next `{ "type": ... }` part as a Python dict; raises
/// `StopAsyncIteration` when the graph finishes. Built by `stream_dag`.
#[pyclass]
struct PyDagStream {
    stream: Arc<Mutex<DagPartStream>>,
}

#[pymethods]
impl PyDagStream {
    fn __aiter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __anext__<'py>(slf: PyRefMut<'_, Self>, py: Python<'py>) -> PyResult<Option<Py<PyAny>>> {
        let stream = Arc::clone(&slf.stream);
        let future = async move {
            let mut stream = stream.lock().await;
            if let Some(result) = stream.next().await {
                match result {
                    Ok(part) => Python::attach(|py| {
                        pythonize::pythonize(py, &part)
                            .map(|b| b.unbind())
                            .map_err(|e| DagException::new_err(e.to_string()))
                    }),
                    Err(e) => Err(DagException::new_err(e.to_string())),
                }
            } else {
                Err(PyStopAsyncIteration::new_err(()))
            }
        };
        Ok(Some(future_into_py(py, future)?.into()))
    }
}

#[pyfunction]
#[pyo3(signature = (graph, resume_id=None, resume_answer=None, inject_payload=None, include_extra_info=false, agent_session_id=None))]
fn run_dag(
    py: Python,
    graph: pyo3::Bound<'_, pyo3::PyAny>,
    resume_id: Option<String>,
    resume_answer: Option<String>,
    inject_payload: Option<pyo3::Bound<'_, pyo3::PyAny>>,
    include_extra_info: bool,
    agent_session_id: Option<String>,
) -> PyResult<String> {
    // `graph` is either a path to a JSON file (str) or an in-memory graph (dict).
    enum GraphSource {
        Path(String),
        Json(String),
    }
    let source = if let Ok(path) = graph.extract::<String>() {
        GraphSource::Path(path)
    } else {
        let value: serde_json::Value = pythonize::depythonize(&graph).map_err(|e| {
            DagException::new_err(format!(
                "graph must be a file-path string or a graph dict: {}",
                e
            ))
        })?;
        GraphSource::Json(
            serde_json::to_string(&value).map_err(|e| DagException::new_err(e.to_string()))?,
        )
    };

    let inject_payload_val: Option<serde_json::Value> = match inject_payload {
        Some(obj) => {
            Some(pythonize::depythonize(&obj).map_err(|e| DagException::new_err(e.to_string()))?)
        }
        None => None,
    };
    py.detach(move || {
        let rt =
            tokio::runtime::Runtime::new().map_err(|e| DagException::new_err(e.to_string()))?;

        rt.block_on(async {
            let exec = match source {
                GraphSource::Path(p) => {
                    crate::dag_engine::api::run_dag(
                        p,
                        resume_id,
                        resume_answer,
                        inject_payload_val,
                        include_extra_info,
                        agent_session_id,
                    )
                    .await
                }
                GraphSource::Json(j) => {
                    crate::dag_engine::api::run_dag_from_str(
                        j,
                        resume_id,
                        resume_answer,
                        inject_payload_val,
                        include_extra_info,
                        agent_session_id,
                    )
                    .await
                }
            };
            match exec {
                Ok(result) => serde_json::to_string_pretty(&result)
                    .map_err(|e| DagException::new_err(e.to_string())),
                Err(e) => Err(DagException::new_err(e.to_string())),
            }
        })
    })
}

/// Streams a DAG's execution as SSE-mapped events (the `{ "type": ... }` parts
/// the HTTP server emits), one Python dict per `__anext__`. Returns an awaitable
/// that resolves to an async iterator:
///
/// ```python
/// stream = await colmena.stream_dag("graph.json", agent_session_id="s1")
/// async for event in stream:
///     if event["type"] == "text-delta":
///         print(event["delta"], end="")
/// ```
///
/// `graph` is a file-path string or an in-memory graph dict (same as `run_dag`).
#[pyfunction]
#[pyo3(signature = (graph, resume_id=None, resume_answer=None, inject_payload=None, include_extra_info=false, agent_session_id=None))]
fn stream_dag<'py>(
    py: Python<'py>,
    graph: pyo3::Bound<'py, pyo3::PyAny>,
    resume_id: Option<String>,
    resume_answer: Option<String>,
    inject_payload: Option<pyo3::Bound<'py, pyo3::PyAny>>,
    include_extra_info: bool,
    agent_session_id: Option<String>,
) -> PyResult<pyo3::Bound<'py, pyo3::PyAny>> {
    // `graph` is either a path to a JSON file (str) or an in-memory graph (dict).
    enum GraphSource {
        Path(String),
        Json(String),
    }
    let source = if let Ok(path) = graph.extract::<String>() {
        GraphSource::Path(path)
    } else {
        let value: serde_json::Value = pythonize::depythonize(&graph).map_err(|e| {
            DagException::new_err(format!(
                "graph must be a file-path string or a graph dict: {}",
                e
            ))
        })?;
        GraphSource::Json(
            serde_json::to_string(&value).map_err(|e| DagException::new_err(e.to_string()))?,
        )
    };

    let inject_payload_val: Option<serde_json::Value> = match inject_payload {
        Some(obj) => {
            Some(pythonize::depythonize(&obj).map_err(|e| DagException::new_err(e.to_string()))?)
        }
        None => None,
    };

    future_into_py(py, async move {
        // The two api fns return distinct `impl Stream` opaque types, so box each
        // arm to the shared `DagPartStream` before the match resolves.
        let boxed: DagPartStream = match source {
            GraphSource::Path(p) => {
                let s = crate::dag_engine::api::stream_dag(
                    p,
                    resume_id,
                    resume_answer,
                    inject_payload_val,
                    include_extra_info,
                    agent_session_id,
                )
                .await
                .map_err(|e| DagException::new_err(e.to_string()))?;
                Box::pin(s)
            }
            GraphSource::Json(j) => {
                let s = crate::dag_engine::api::stream_dag_from_str(
                    j,
                    resume_id,
                    resume_answer,
                    inject_payload_val,
                    include_extra_info,
                    agent_session_id,
                )
                .await
                .map_err(|e| DagException::new_err(e.to_string()))?;
                Box::pin(s)
            }
        };
        Ok(PyDagStream {
            stream: Arc::new(Mutex::new(boxed)),
        })
    })
}

/// Checks a graph dict the way `dag_engine run` does when it loads a file.
///
/// Deserialises into the engine's `Graph` and then runs `Graph::validate()`, so
/// it rejects a node id containing `/`, a malformed `node_schema`, an invalid
/// `memory_mode` and a misconfigured `mcp` block — the same structural
/// invariants that would otherwise only surface at run time.
///
/// It still says nothing about the contents of a node's `config`: that is an
/// untyped value, so an invented field passes silently here. Use
/// `dag_engine lint` for that.
#[pyfunction]
fn validate_graph(graph: pyo3::Bound<'_, pyo3::PyAny>) -> PyResult<()> {
    let v: serde_json::Value =
        pythonize::depythonize(&graph).map_err(|e| DagException::new_err(e.to_string()))?;
    let g: crate::dag_engine::domain::graph::Graph = serde_json::from_value(v)
        .map_err(|e| DagException::new_err(format!("invalid graph: {}", e)))?;
    g.validate()
        .map_err(|e| DagException::new_err(format!("invalid graph: {}", e)))?;
    Ok(())
}

/// Read-only handle to a `HashMapNodeRegistry`; exposes inspection helpers
/// that smoke tests rely on (no DB connection required).
#[pyclass]
struct Registry {
    inner: Arc<crate::dag_engine::infrastructure::registry::HashMapNodeRegistry>,
}

#[pymethods]
impl Registry {
    fn node_types(&self) -> Vec<String> {
        use crate::dag_engine::application::ports::NodeRegistryPort;
        let mut keys: Vec<String> = self.inner.get_all_nodes().keys().cloned().collect();
        keys.sort();
        keys
    }

    fn toolkit_catalog(
        &self,
        py: Python<'_>,
        node_type: &str,
        config: pyo3::Bound<'_, pyo3::PyAny>,
    ) -> PyResult<Py<PyAny>> {
        use crate::dag_engine::application::ports::NodeRegistryPort;
        let cfg: serde_json::Value =
            pythonize::depythonize(&config).map_err(|e| DagException::new_err(e.to_string()))?;
        let tk = self
            .inner
            .get_toolkit_node(node_type)
            .ok_or_else(|| DagException::new_err(format!("not a toolkit node: {}", node_type)))?;
        let entries: Vec<serde_json::Value> = tk
            .sub_tool_catalog(&cfg)
            .into_iter()
            .map(|s| {
                serde_json::json!({
                    "name": s.name.as_ref(),
                    "description": s.description,
                    "required": s.required,
                })
            })
            .collect();
        let value = serde_json::Value::Array(entries);
        pythonize::pythonize(py, &value)
            .map(|b| b.unbind())
            .map_err(|e| DagException::new_err(e.to_string()))
    }
}

/// Stub task-memory repository used only by `default_registry` so the
/// inspection-only smoke harness does not need a database. Every method is a
/// no-op; these registries are not used to execute graphs.
struct SmokeTaskMemory;

#[async_trait::async_trait]
impl crate::dag_engine::domain::state::DagTaskMemoryRepository for SmokeTaskMemory {
    async fn add_task(
        &self,
        _task: &crate::dag_engine::domain::state::DagTask,
    ) -> Result<(), crate::dag_engine::domain::error::DagError> {
        Ok(())
    }
    async fn update_task_result(
        &self,
        _task_id: &str,
        _result: serde_json::Value,
    ) -> Result<(), crate::dag_engine::domain::error::DagError> {
        Ok(())
    }
    async fn get_tasks_for_run(
        &self,
        _session_id: &str,
    ) -> Result<
        Vec<crate::dag_engine::domain::state::DagTask>,
        crate::dag_engine::domain::error::DagError,
    > {
        Ok(vec![])
    }
    async fn get_first_uncompleted_task(
        &self,
        _session_id: &str,
    ) -> Result<
        Option<crate::dag_engine::domain::state::DagTask>,
        crate::dag_engine::domain::error::DagError,
    > {
        Ok(None)
    }
    async fn delete_task(
        &self,
        _task_id: &str,
    ) -> Result<(), crate::dag_engine::domain::error::DagError> {
        Ok(())
    }
    async fn clear_tasks_for_run(
        &self,
        _session_id: &str,
    ) -> Result<(), crate::dag_engine::domain::error::DagError> {
        Ok(())
    }
    async fn get_current_phase(
        &self,
        _session_id: &str,
    ) -> Result<Option<i32>, crate::dag_engine::domain::error::DagError> {
        Ok(None)
    }
    async fn get_uncompleted_tasks_for_phase(
        &self,
        _session_id: &str,
        _phase: i32,
    ) -> Result<
        Vec<crate::dag_engine::domain::state::DagTask>,
        crate::dag_engine::domain::error::DagError,
    > {
        Ok(vec![])
    }
    async fn save_phase_summary(
        &self,
        _session_id: &str,
        _phase: i32,
        _summary: &str,
    ) -> Result<(), crate::dag_engine::domain::error::DagError> {
        Ok(())
    }
    async fn get_phase_summaries(
        &self,
        _session_id: &str,
    ) -> Result<
        Vec<crate::dag_engine::domain::state::DagPhaseSummary>,
        crate::dag_engine::domain::error::DagError,
    > {
        Ok(vec![])
    }
}

/// Builds a fresh `HashMapNodeRegistry` with no live database connections.
/// `PgPoolRegistry::new` is in-memory; pools are pinned lazily on first use,
/// which is fine because smoke tests only inspect the registry.
#[pyfunction]
fn default_registry() -> PyResult<Registry> {
    use crate::dag_engine::infrastructure::pool_registry::{PgPoolRegistry, PoolConfig};
    use crate::dag_engine::infrastructure::registry::HashMapNodeRegistry;
    use crate::dag_engine::infrastructure::sql_port_factory::SqlPortFactory;
    use crate::llm::infrastructure::ConversationRepositoryFactory;

    let pools = Arc::new(PgPoolRegistry::new(PoolConfig::defaults()));
    let conv = Arc::new(ConversationRepositoryFactory::new(pools.clone()));
    let sql = Arc::new(SqlPortFactory::new(pools));
    let task_memory: Arc<dyn crate::dag_engine::domain::state::DagTaskMemoryRepository> =
        Arc::new(SmokeTaskMemory);
    let inner = HashMapNodeRegistry::new(conv, sql, Some(task_memory));
    Ok(Registry { inner })
}

#[pyfunction]
#[pyo3(signature = (file_path, host="0.0.0.0".to_string(), port=8080))]
fn serve_dag(py: Python, file_path: String, host: String, port: u16) -> PyResult<()> {
    py.detach(move || {
        let rt =
            tokio::runtime::Runtime::new().map_err(|e| DagException::new_err(e.to_string()))?;

        rt.block_on(async {
            crate::dag_engine::api::serve_dag(file_path, host, port)
                .await
                .map_err(|e| DagException::new_err(e.to_string()))
        })
    })
}

#[pymodule]
fn colmena(_py: Python, m: &Bound<'_, PyModule>) -> PyResult<()> {
    // LLM bindings
    m.add_class::<ColmenaLlm>()?;
    m.add_class::<LlmConfigOptions>()?;
    m.add("LlmException", _py.get_type::<LlmException>())?;

    // DAG Engine bindings
    m.add_function(wrap_pyfunction!(run_dag, m)?)?;
    m.add_function(wrap_pyfunction!(stream_dag, m)?)?;
    m.add_function(wrap_pyfunction!(serve_dag, m)?)?;
    m.add_function(wrap_pyfunction!(validate_graph, m)?)?;
    m.add_function(wrap_pyfunction!(default_registry, m)?)?;
    m.add_class::<Registry>()?;
    m.add("DagException", _py.get_type::<DagException>())?;

    // CRDT documents bindings (v1)
    crdt_documents::register(m)?;

    Ok(())
}
