use crate::llm::domain::{LlmError, MessageRole, ProviderKind};
use crate::shared::infrastructure::{ConfigResolver, ServiceContainerFactory};
use futures::StreamExt;
use pyo3::prelude::*;
use pyo3::{create_exception, exceptions::PyException, types::PyDict};
use std::collections::HashMap;

create_exception!(colmena, LlmException, PyException);

impl From<LlmError> for PyErr {
    fn from(err: LlmError) -> PyErr {
        LlmException::new_err(err.to_string())
    }
}

#[pyclass]
pub struct ColmenaLlm {
    containers: HashMap<String, crate::shared::infrastructure::ServiceContainer>,
}

#[pymethods]
impl ColmenaLlm {
    #[new]
    pub fn new() -> PyResult<Self> {
        ConfigResolver::load_env()?;
        let mut containers = HashMap::new();
        for (provider, container) in ServiceContainerFactory::create_all() {
            containers.insert(provider.to_string(), container);
        }
        Ok(Self { containers })
    }

    #[pyo3(signature = (messages, provider, api_key=None, model=None, temperature=None, max_tokens=None, top_p=None, frequency_penalty=None, presence_penalty=None))]
    pub fn call(&self, py: Python, messages: Vec<String>, provider: &str, api_key: Option<String>, model: Option<String>, temperature: Option<f32>, max_tokens: Option<u32>, top_p: Option<f32>, frequency_penalty: Option<f32>, presence_penalty: Option<f32>) -> PyResult<String> {
        let provider_kind = ProviderKind::from_str(provider)?;
        let container = self.containers.get(provider).ok_or_else(|| LlmException::new_err(format!("Provider {} not found", provider)))?;
        let config = ConfigResolver::create_config(provider_kind, api_key, model, temperature, max_tokens, top_p, frequency_penalty, presence_penalty)?;

        py.allow_threads(move || {
            let rt = tokio::runtime::Runtime::new().map_err(|e| LlmException::new_err(e.to_string()))?;
            rt.block_on(async {
                container.llm_call.execute(messages, config).await
                    .map(|res| res.content().to_string())
                    .map_err(PyErr::from)
            })
        })
    }

    #[pyo3(signature = (system_message, messages, provider, api_key=None, model=None, temperature=None, max_tokens=None, top_p=None, frequency_penalty=None, presence_penalty=None))]
    pub fn call_with_context(&self, py: Python, system_message: Option<String>, messages: Vec<String>, provider: &str, api_key: Option<String>, model: Option<String>, temperature: Option<f32>, max_tokens: Option<u32>, top_p: Option<f32>, frequency_penalty: Option<f32>, presence_penalty: Option<f32>) -> PyResult<String> {
        let provider_kind = ProviderKind::from_str(provider)?;
        let container = self.containers.get(provider).ok_or_else(|| LlmException::new_err(format!("Provider {} not found", provider)))?;
        let config = ConfigResolver::create_config(provider_kind, api_key, model, temperature, max_tokens, top_p, frequency_penalty, presence_penalty)?;

        py.allow_threads(move || {
            let rt = tokio::runtime::Runtime::new().map_err(|e| LlmException::new_err(e.to_string()))?;
            rt.block_on(async {
                container.llm_call.execute_with_context(system_message, messages, config).await
                    .map(|res| res.content().to_string())
                    .map_err(PyErr::from)
            })
        })
    }

    /// Call LLM with conversation format accepting role+message pairs (legacy)
    ///
    /// Args:
    ///     conversation: List of (role, message) tuples where role is "system", "user", or "assistant"
    ///                  Example: [("system", "You are a helpful assistant"), ("user", "Hello")]
    #[pyo3(signature = (conversation, provider, api_key=None, model=None, temperature=None, max_tokens=None, top_p=None, frequency_penalty=None, presence_penalty=None))]
    pub fn call_conversation(&self, py: Python, conversation: Vec<(String, String)>, provider: &str, api_key: Option<String>, model: Option<String>, temperature: Option<f32>, max_tokens: Option<u32>, top_p: Option<f32>, frequency_penalty: Option<f32>, presence_penalty: Option<f32>) -> PyResult<String> {
        let provider_kind = ProviderKind::from_str(provider)?;
        let container = self.containers.get(provider).ok_or_else(|| LlmException::new_err(format!("Provider {} not found", provider)))?;
        let conversation_with_roles: Result<Vec<(MessageRole, String)>, PyErr> = conversation.into_iter().map(|(role_str, message)| MessageRole::from_str(&role_str).map(|role| (role, message)).map_err(|e| LlmException::new_err(e.to_string()))).collect();
        let config = ConfigResolver::create_config(provider_kind, api_key, model, temperature, max_tokens, top_p, frequency_penalty, presence_penalty)?;

        py.allow_threads(move || {
            let rt = tokio::runtime::Runtime::new().map_err(|e| LlmException::new_err(e.to_string()))?;
            rt.block_on(async {
                container.llm_call.execute_conversation(conversation_with_roles?, config).await
                    .map(|res| res.content().to_string())
                    .map_err(PyErr::from)
            })
        })
    }

    /// Call LLM with conversation format accepting message dictionaries (recommended)
    ///
    /// Args:
    ///     messages: List of message dictionaries with "role" and "content" keys
    ///               Example: [{"role": "system", "content": "You are helpful"},
    ///                        {"role": "user", "content": "Hello"}]
    ///               For images: {"role": "user", "content": [{"type": "text", "text": "What's in this image?"},
    ///                                                       {"type": "image", "image_url": "..."}]}
    #[pyo3(signature = (messages, provider, api_key=None, model=None, temperature=None, max_tokens=None, top_p=None, frequency_penalty=None, presence_penalty=None))]
    pub fn call_messages(&self, py: Python, messages: Vec<&PyDict>, provider: &str, api_key: Option<String>, model: Option<String>, temperature: Option<f32>, max_tokens: Option<u32>, top_p: Option<f32>, frequency_penalty: Option<f32>, presence_penalty: Option<f32>) -> PyResult<String> {
        let provider_kind = ProviderKind::from_str(provider)?;
        let container = self.containers.get(provider).ok_or_else(|| LlmException::new_err(format!("Provider {} not found", provider)))?;

        // Parse messages from dictionaries
        let conversation_with_roles: Result<Vec<(MessageRole, String)>, PyErr> = messages
            .into_iter()
            .map(|msg_dict| {
                let role_str: String = match msg_dict.get_item("role") {
                    Some(role_val) => role_val.extract()?,
                    None => return Err(LlmException::new_err("Missing 'role' key in message")),
                };

                let content: String = match msg_dict.get_item("content") {
                    Some(content_val) => content_val.extract()?,
                    None => return Err(LlmException::new_err("Missing 'content' key in message")),
                };

                MessageRole::from_str(&role_str)
                    .map(|role| (role, content))
                    .map_err(|e| LlmException::new_err(e.to_string()))
            })
            .collect();

        let config = ConfigResolver::create_config(provider_kind, api_key, model, temperature, max_tokens, top_p, frequency_penalty, presence_penalty)?;

        py.allow_threads(move || {
            let rt = tokio::runtime::Runtime::new().map_err(|e| LlmException::new_err(e.to_string()))?;
            rt.block_on(async {
                container.llm_call.execute_conversation(conversation_with_roles?, config).await
                    .map(|res| res.content().to_string())
                    .map_err(PyErr::from)
            })
        })
    }

    #[pyo3(signature = (messages, provider, api_key=None, model=None, temperature=None, max_tokens=None, top_p=None, frequency_penalty=None, presence_penalty=None))]
    pub fn stream(&self, py: Python, messages: Vec<String>, provider: &str, api_key: Option<String>, model: Option<String>, temperature: Option<f32>, max_tokens: Option<u32>, top_p: Option<f32>, frequency_penalty: Option<f32>, presence_penalty: Option<f32>) -> PyResult<PyObject> {
        let provider_kind = ProviderKind::from_str(provider)?;
        let container = self.containers.get(provider).ok_or_else(|| LlmException::new_err(format!("Provider {} not found", provider)))?;
        let config = ConfigResolver::create_config(provider_kind, api_key, model, temperature, max_tokens, top_p, frequency_penalty, presence_penalty)?;

        let stream_result = py.allow_threads(move || {
            let rt = tokio::runtime::Runtime::new().map_err(|e| LlmException::new_err(e.to_string()))?;
            rt.block_on(async {
                container.llm_stream.execute(messages, config).await
            }).map_err(PyErr::from)
        })?;

        let generator = PyStreamGenerator::new(stream_result);
        Ok(generator.into_py(py))
    }

    pub fn health_check(&self, py: Python, provider: &str) -> PyResult<bool> {
        let container = self.containers.get(provider).ok_or_else(|| LlmException::new_err(format!("Provider {} not found", provider)))?;
        py.allow_threads(|| {
            let rt = tokio::runtime::Runtime::new().map_err(|e| LlmException::new_err(e.to_string()))?;
            rt.block_on(async {
                container.llm_health_check.execute().await
                    .map(|status| status.is_healthy())
                    .map_err(PyErr::from)
            })
        })
    }

    pub fn get_providers(&self) -> PyResult<Vec<String>> {
        Ok(self.containers.keys().cloned().collect())
    }
}

#[pyclass]
struct PyStreamGenerator {
    chunks: Vec<String>,
    index: usize,
}

impl PyStreamGenerator {
    fn new(stream: crate::llm::domain::LlmStream) -> Self {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let chunks = rt.block_on(async {
            let mut chunks = Vec::new();
            let mut stream = stream;
            while let Some(chunk_result) = stream.next().await {
                if let Ok(chunk) = chunk_result {
                    if !chunk.content().is_empty() {
                        chunks.push(chunk.content().to_string());
                    }
                    if chunk.is_final() {
                        break;
                    }
                } else {
                    break;
                }
            }
            chunks
        });
        Self { chunks, index: 0 }
    }
}

#[pymethods]
impl PyStreamGenerator {
    fn __iter__(slf: PyRef<Self>) -> PyRef<Self> {
        slf
    }

    fn __next__(mut slf: PyRefMut<Self>) -> Option<String> {
        if slf.index < slf.chunks.len() {
            let chunk = slf.chunks[slf.index].clone();
            slf.index += 1;
            Some(chunk)
        } else {
            None
        }
    }
}

#[pymodule]
fn colmena(_py: Python, m: &PyModule) -> PyResult<()> {
    m.add_class::<ColmenaLlm>()?;
    m.add("LlmException", _py.get_type::<LlmException>())?;
    Ok(())
}
