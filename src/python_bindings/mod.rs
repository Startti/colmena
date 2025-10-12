use crate::llm::domain::{LlmError, LlmMessage, MessageRole, ProviderKind};
use crate::shared::infrastructure::{ConfigResolver, ServiceContainerFactory};
use futures::StreamExt;
use pyo3::prelude::*;
use pyo3::{create_exception, exceptions::{PyException, PyStopAsyncIteration}, types::PyDict};
use pyo3_asyncio_0_21::tokio::future_into_py;
use std::collections::HashMap;
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
pub struct ColmenaLlm {
    containers: HashMap<String, Arc<crate::shared::infrastructure::ServiceContainer>>,
}

// START ASYNC MOCK STREAMING FOR TESTING
#[pyclass]
struct AsyncMockStreamIterator {
    iter: Arc<Mutex<std::vec::IntoIter<String>>>,
}

#[pymethods]
impl AsyncMockStreamIterator {
    fn __aiter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __anext__<'py>(slf: PyRefMut<'_, Self>, py: Python<'py>) -> PyResult<Option<PyObject>> {
        let iter = Arc::clone(&slf.iter);
        let future = async move {
            let mut iter = iter.lock().await;
            // Simulate I/O delay
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            let next = iter.next();
            if let Some(ref s) = next {
                println!("[Rust] Yielding async: {}", s);
            }
            Ok(next)
        };
        Ok(Some(future_into_py(py, future)?.into()))
    }
}
// END ASYNC MOCK STREAMING FOR TESTING

// START MOCK STREAMING FOR TESTING
#[pyclass]
struct MockStreamIterator {
    iter: std::vec::IntoIter<String>,
}

#[pymethods]
impl MockStreamIterator {
    fn __iter__(slf: PyRef<Self>) -> PyRef<Self> {
        slf
    }

    fn __next__(mut slf: PyRefMut<Self>) -> Option<String> {
        let next = slf.iter.next();
        if let Some(ref s) = next {
            println!("[Rust] Yielding: {}", s);
        }
        next
    }
}
// END MOCK STREAMING FOR TESTING

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

    #[pyo3(signature = (messages, provider, api_key=None, model=None, temperature=None, max_tokens=None, top_p=None, frequency_penalty=None, presence_penalty=None))]
    #[allow(clippy::too_many_arguments)]
    pub fn call(
        &self,
        py: Python,
        messages: Vec<&PyDict>,
        provider: &str,
        api_key: Option<String>,
        model: Option<String>,
        temperature: Option<f32>,
        max_tokens: Option<u32>,
        top_p: Option<f32>,
        frequency_penalty: Option<f32>,
        presence_penalty: Option<f32>,
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

                let role = MessageRole::from_str(&role_str).map_err(|e| LlmException::new_err(e.to_string()))?;
                
                LlmMessage::new(role, content).map_err(|e| LlmException::new_err(e.to_string()))
            })
            .collect();

        let config = ConfigResolver::create_config(
            provider_kind,
            api_key,
            model,
            temperature,
            max_tokens,
            top_p,
            frequency_penalty,
            presence_penalty,
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

    #[pyo3(signature = (messages, provider, api_key=None, model=None, temperature=None, max_tokens=None, top_p=None, frequency_penalty=None, presence_penalty=None))]
    pub fn stream(
        &self,
        py: Python,
        messages: Vec<&PyDict>,
        provider: &str,
        api_key: Option<String>,
        model: Option<String>,
        temperature: Option<f32>,
        max_tokens: Option<u32>,
        top_p: Option<f32>,
        frequency_penalty: Option<f32>,
        presence_penalty: Option<f32>,
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

        let config = ConfigResolver::create_config(
            provider_kind,
            api_key,
            model,
            temperature,
            max_tokens,
            top_p,
            frequency_penalty,
            presence_penalty,
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

    // START MOCK STREAMING FOR TESTING
    pub fn mock_stream(&self, py: Python) -> PyResult<PyObject> {
        let data = vec![
            "this".to_string(),
            "is".to_string(),
            "an".to_string(),
            "stremaing".to_string(),
            "mock".to_string(),
        ];
        let iterator = MockStreamIterator {
            iter: data.into_iter(),
        };
        Ok(iterator.into_py(py))
    }
    // END MOCK STREAMING FOR TESTING

    // START ASYNC MOCK STREAMING FOR TESTING
    pub fn mock_stream_async(&self, py: Python) -> PyResult<PyObject> {
        let data = vec![
            "this".to_string(),
            "is".to_string(),
            "an".to_string(),
            "async".to_string(),
            "mock".to_string(),
        ];
        let iterator = AsyncMockStreamIterator {
            iter: Arc::new(Mutex::new(data.into_iter())),
        };
        future_into_py(py, async {
            Ok(iterator)
        }).map(|bound| bound.into())
    }
    // END ASYNC MOCK STREAMING FOR TESTING

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

#[pymodule]
#[allow(deprecated)]
fn colmena(_py: Python, m: &PyModule) -> PyResult<()> {
    m.add_class::<ColmenaLlm>()?;
    m.add("LlmException", _py.get_type_bound::<LlmException>())?;
    Ok(())
}
