use pyo3::prelude::*;
use pyo3::{exceptions::PyException, create_exception};
use crate::llm::domain::{LlmError, ProviderKind, MessageRole};
use crate::shared::infrastructure::{ServiceContainerFactory, ConfigResolver};
use futures::StreamExt;
use std::collections::HashMap;

// Custom Python exception for LLM errors
create_exception!(colmena, LlmException, PyException);

// Implement conversion from LlmError to PyErr
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
        // Load environment variables
        ConfigResolver::load_env().map_err(|e| LlmException::new_err(e.to_string()))?;

        // Create service containers for all providers
        let mut containers = HashMap::new();
        for (provider, container) in ServiceContainerFactory::create_all() {
            containers.insert(provider.to_string(), container);
        }

        Ok(Self { containers })
    }

    /// Make a synchronous call to an LLM
    ///
    /// Args:
    ///     messages: List of message strings (treated as user messages)
    ///     provider: Provider name ("openai", "gemini", "anthropic")
    ///     api_key: Optional API key (will use environment variable if not provided)
    ///     model: Optional model name (will use default if not provided)
    ///     temperature: Optional temperature (0.0-2.0)
    ///     max_tokens: Optional maximum tokens
    ///     top_p: Optional top_p (0.0-1.0)
    ///     frequency_penalty: Optional frequency penalty (-2.0-2.0)
    ///     presence_penalty: Optional presence penalty (-2.0-2.0)
    ///
    /// Returns:
    ///     Response content as string
    #[pyo3(signature = (messages, provider, api_key=None, model=None, temperature=None, max_tokens=None, top_p=None, frequency_penalty=None, presence_penalty=None))]
    pub fn call(
        &self,
        py: Python,
        messages: Vec<String>,
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

        let container = self.containers.get(provider)
            .ok_or_else(|| LlmException::new_err(format!("Provider {} not found", provider)))?;

        py.allow_threads(|| {
            let rt = tokio::runtime::Runtime::new()
                .map_err(|e| LlmException::new_err(e.to_string()))?;

            rt.block_on(async {
                let response = container.llm_call.execute(
                    messages,
                    provider_kind,
                    api_key,
                    model,
                    temperature,
                    max_tokens,
                    top_p,
                    frequency_penalty,
                    presence_penalty,
                ).await.map_err(|e| LlmException::new_err(e.to_string()))?;

                Ok(response.content().to_string())
            })
        })
    }

    /// Make a call with system message and conversation context
    ///
    /// Args:
    ///     system_message: Optional system message
    ///     messages: List of message strings (treated as user messages)
    ///     provider: Provider name ("openai", "gemini", "anthropic")
    ///     api_key: Optional API key
    ///     model: Optional model name
    ///     temperature: Optional temperature
    ///     max_tokens: Optional maximum tokens
    ///     top_p: Optional top_p
    ///     frequency_penalty: Optional frequency penalty
    ///     presence_penalty: Optional presence penalty
    ///
    /// Returns:
    ///     Response content as string
    #[pyo3(signature = (system_message, messages, provider, api_key=None, model=None, temperature=None, max_tokens=None, top_p=None, frequency_penalty=None, presence_penalty=None))]
    pub fn call_with_context(
        &self,
        py: Python,
        system_message: Option<String>,
        messages: Vec<String>,
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

        let container = self.containers.get(provider)
            .ok_or_else(|| LlmException::new_err(format!("Provider {} not found", provider)))?;

        py.allow_threads(|| {
            let rt = tokio::runtime::Runtime::new()
                .map_err(|e| LlmException::new_err(e.to_string()))?;

            rt.block_on(async {
                let response = container.llm_call.execute_with_context(
                    system_message,
                    messages,
                    provider_kind,
                    api_key,
                    model,
                    temperature,
                    max_tokens,
                    top_p,
                    frequency_penalty,
                    presence_penalty,
                ).await.map_err(|e| LlmException::new_err(e.to_string()))?;

                Ok(response.content().to_string())
            })
        })
    }

    /// Make a call with full conversation history
    ///
    /// Args:
    ///     conversation: List of (role, message) tuples where role is "system", "user", or "assistant"
    ///     provider: Provider name ("openai", "gemini", "anthropic")
    ///     api_key: Optional API key
    ///     model: Optional model name
    ///     temperature: Optional temperature
    ///     max_tokens: Optional maximum tokens
    ///     top_p: Optional top_p
    ///     frequency_penalty: Optional frequency penalty
    ///     presence_penalty: Optional presence penalty
    ///
    /// Returns:
    ///     Response content as string
    #[pyo3(signature = (conversation, provider, api_key=None, model=None, temperature=None, max_tokens=None, top_p=None, frequency_penalty=None, presence_penalty=None))]
    pub fn call_conversation(
        &self,
        py: Python,
        conversation: Vec<(String, String)>,
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

        let container = self.containers.get(provider)
            .ok_or_else(|| LlmException::new_err(format!("Provider {} not found", provider)))?;

        // Convert conversation
        let conversation_with_roles: Result<Vec<(MessageRole, String)>, String> = conversation
            .into_iter()
            .map(|(role_str, message)| {
                MessageRole::from_str(&role_str).map(|role| (role, message))
            })
            .collect();

        let conversation_with_roles = conversation_with_roles
            .map_err(|e| LlmException::new_err(e))?;

        py.allow_threads(|| {
            let rt = tokio::runtime::Runtime::new()
                .map_err(|e| LlmException::new_err(e.to_string()))?;

            rt.block_on(async {
                let response = container.llm_call.execute_conversation(
                    conversation_with_roles,
                    provider_kind,
                    api_key,
                    model,
                    temperature,
                    max_tokens,
                    top_p,
                    frequency_penalty,
                    presence_penalty,
                ).await.map_err(|e| LlmException::new_err(e.to_string()))?;

                Ok(response.content().to_string())
            })
        })
    }

    /// Make a streaming call to an LLM
    ///
    /// Args:
    ///     messages: List of message strings (treated as user messages)
    ///     provider: Provider name ("openai", "gemini", "anthropic")
    ///     api_key: Optional API key
    ///     model: Optional model name
    ///     temperature: Optional temperature
    ///     max_tokens: Optional maximum tokens
    ///     top_p: Optional top_p
    ///     frequency_penalty: Optional frequency penalty
    ///     presence_penalty: Optional presence penalty
    ///
    /// Returns:
    ///     Generator yielding text chunks
    #[pyo3(signature = (messages, provider, api_key=None, model=None, temperature=None, max_tokens=None, top_p=None, frequency_penalty=None, presence_penalty=None))]
    pub fn stream(
        &self,
        py: Python,
        messages: Vec<String>,
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

        let container = self.containers.get(provider)
            .ok_or_else(|| LlmException::new_err(format!("Provider {} not found", provider)))?;

        let stream_result = py.allow_threads(|| {
            let rt = tokio::runtime::Runtime::new()
                .map_err(|e| LlmException::new_err(e.to_string()))?;

            rt.block_on(async {
                container.llm_stream.execute(
                    messages,
                    provider_kind,
                    api_key,
                    model,
                    temperature,
                    max_tokens,
                    top_p,
                    frequency_penalty,
                    presence_penalty,
                ).await.map_err(|e| LlmException::new_err(e.to_string()))
            })
        })?;

        // Create a Python generator
        let generator = PyStreamGenerator::new(stream_result);
        Ok(generator.into_py(py))
    }

    /// Check health of a provider
    ///
    /// Args:
    ///     provider: Provider name ("openai", "gemini", "anthropic")
    ///
    /// Returns:
    ///     True if healthy, False otherwise
    pub fn health_check(&self, py: Python, provider: &str) -> PyResult<bool> {
        let container = self.containers.get(provider)
            .ok_or_else(|| LlmException::new_err(format!("Provider {} not found", provider)))?;

        py.allow_threads(|| {
            let rt = tokio::runtime::Runtime::new()
                .map_err(|e| LlmException::new_err(e.to_string()))?;

            rt.block_on(async {
                let status = container.llm_health_check.execute().await
                    .map_err(|e| LlmException::new_err(e.to_string()))?;

                Ok(status.is_healthy())
            })
        })
    }

    /// Get list of available providers
    pub fn get_providers(&self) -> PyResult<Vec<String>> {
        Ok(self.containers.keys().cloned().collect())
    }
}

// Helper struct for streaming
#[pyclass]
struct PyStreamGenerator {
    // We'll store the stream in a way that Python can iterate over it
    // This is a simplified version - in production you'd want proper async iteration
    chunks: Vec<String>,
    index: usize,
}

impl PyStreamGenerator {
    fn new(stream: crate::llm::domain::LlmStream) -> Self {
        // For simplicity, we'll collect all chunks immediately
        // In a real implementation, you'd want proper async streaming
        let rt = tokio::runtime::Runtime::new().unwrap();
        let chunks = rt.block_on(async {
            let mut chunks = Vec::new();
            let mut stream = stream;
            while let Some(chunk_result) = stream.next().await {
                match chunk_result {
                    Ok(chunk) => {
                        if !chunk.content().is_empty() {
                            chunks.push(chunk.content().to_string());
                        }
                        if chunk.is_final() {
                            break;
                        }
                    }
                    Err(_) => break,
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

/// A Python module implemented in Rust.
#[pymodule]
fn colmena(_py: Python, m: &PyModule) -> PyResult<()> {
    m.add_class::<ColmenaLlm>()?;
    m.add("LlmException", _py.get_type::<LlmException>())?;
    Ok(())
}