use crate::llm::domain::{LlmError, LlmMessage, MessageRole, ProviderKind};
use crate::shared::infrastructure::{ConfigResolver, ServiceContainerFactory};
use futures::StreamExt;
use pyo3::prelude::*;
use pyo3::{
    create_exception,
    exceptions::{PyException, PyStopAsyncIteration},
    types::PyDict,
};
use pyo3_asyncio_0_21::tokio::future_into_py;
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

    fn __anext__<'py>(slf: PyRefMut<'_, Self>, py: Python<'py>) -> PyResult<Option<PyObject>> {
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

#[pyclass]
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
        messages: Vec<&PyDict>,
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

        py.allow_threads(move || {
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
        messages: Vec<&PyDict>,
        provider: &str,
        options: Option<LlmConfigOptions>,
    ) -> PyResult<PyObject> {
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
        py.allow_threads(|| {
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

#[pyfunction]
#[pyo3(signature = (file_path))]
fn run_dag(py: Python, file_path: String) -> PyResult<String> {
    py.allow_threads(move || {
        let rt =
            tokio::runtime::Runtime::new().map_err(|e| DagException::new_err(e.to_string()))?;

        rt.block_on(async {
            match crate::dag_engine::api::run_dag(file_path).await {
                Ok(result) => serde_json::to_string_pretty(&result)
                    .map_err(|e| DagException::new_err(e.to_string())),
                Err(e) => Err(DagException::new_err(e.to_string())),
            }
        })
    })
}

#[pyfunction]
#[pyo3(signature = (file_path, host="0.0.0.0".to_string(), port=8080))]
fn serve_dag(py: Python, file_path: String, host: String, port: u16) -> PyResult<()> {
    py.allow_threads(move || {
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
#[allow(deprecated)]
fn colmena(_py: Python, m: &PyModule) -> PyResult<()> {
    // LLM bindings
    m.add_class::<ColmenaLlm>()?;
    m.add_class::<LlmConfigOptions>()?;
    m.add("LlmException", _py.get_type_bound::<LlmException>())?;

    // DAG Engine bindings
    m.add_function(wrap_pyfunction!(run_dag, m)?)?;
    m.add_function(wrap_pyfunction!(serve_dag, m)?)?;
    m.add("DagException", _py.get_type_bound::<DagException>())?;

    Ok(())
}
