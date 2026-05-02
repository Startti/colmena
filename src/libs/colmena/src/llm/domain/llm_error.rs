use thiserror::Error;

#[derive(Debug, Error, PartialEq)]
pub enum LlmError {
    #[error("Invalid API key")]
    InvalidApiKey,

    #[error("Provider not supported: {provider}")]
    UnsupportedProvider { provider: String },

    #[error("Request failed: {message}")]
    RequestFailed { message: String },

    // Specific Configuration Errors
    #[error("Temperature must be between 0.0 and 2.0")]
    InvalidTemperature,
    #[error("Max tokens must be greater than 0")]
    MaxTokensIsZero,
    #[error("Top_p must be between 0.0 and 1.0")]
    InvalidTopP,
    #[error("Frequency penalty must be between -2.0 and 2.0")]
    InvalidFrequencyPenalty,
    #[error("Presence penalty must be between -2.0 and 2.0")]
    InvalidPresencePenalty,

    #[error("Network error: {message}")]
    NetworkError { message: String },

    #[error("Parsing error: {message}")]
    ParsingError { message: String },
    #[error("Rate limit exceeded")]
    RateLimitExceeded,

    #[error("Invalid model: {model}")]
    InvalidModel { model: String },

    #[error("Empty message list")]
    EmptyMessages,

    #[error("Message content cannot be empty")]
    EmptyMessageContent,

    #[error(
        "Consecutive messages with the same role are not supported. Role '{role}' at indices {index1} and {index2}"
    )]
    ConsecutiveRoles {
        role: String,
        index1: usize,
        index2: usize,
    },

    #[error("Invalid message role: {role}")]
    InvalidMessageRole { role: String },

    #[error("Too many system messages ({count}): {provider} supports maximum {max_allowed}")]
    TooManySystemMessages {
        count: usize,
        provider: String,
        max_allowed: usize,
    },

    #[error("Provider limitation: {provider} does not support {feature}")]
    ProviderLimitation { provider: String, feature: String },

    #[error("Internal error: {message}")]
    InternalError { message: String },

    // Tool-related errors
    #[error("Tool not found: {name}")]
    ToolNotFound { name: String },

    #[error("Tool execution failed: {message}")]
    ToolExecutionFailed { message: String },

    #[error("Invalid tool call: {reason}")]
    InvalidToolCall { reason: String },

    #[error("Max iterations reached: {max} iterations exceeded in ReAct loop")]
    MaxIterationsReached { max: usize },

    // File handling errors (Files API integration)
    #[error("data field exceeds 30 MB limit (got {size} bytes); emitter must use url for large files")]
    DataFieldTooLarge { size: u64 },

    #[error("path file exceeds 30 MB limit (got {size} bytes); use url for large files")]
    PathFieldTooLarge { size: u64 },

    #[error("url field requires id field to enable cache lookup")]
    UrlWithoutDocumentId,

    #[error("signed URL fetch failed with status {status}")]
    SignedUrlFetchFailed { status: u16 },

    #[error("file upload to {provider} Files API failed: {message}")]
    FileApiUploadFailed { provider: String, message: String },

    #[error("provider rejected file with id {provider_file_id}: not found")]
    ProviderFileNotFound { provider_file_id: String },

    #[error("all files in the request failed to materialize")]
    AllFilesFailedToResolve,
}

impl LlmError {
    pub fn request_failed(message: impl Into<String>) -> Self {
        Self::RequestFailed {
            message: message.into(),
        }
    }

    pub fn network_error(message: impl Into<String>) -> Self {
        Self::NetworkError {
            message: message.into(),
        }
    }

    pub fn parsing_error(message: impl Into<String>) -> Self {
        Self::ParsingError {
            message: message.into(),
        }
    }

    pub fn internal_error(message: impl Into<String>) -> Self {
        Self::InternalError {
            message: message.into(),
        }
    }

    pub fn invalid_message_role(role: impl Into<String>) -> Self {
        Self::InvalidMessageRole { role: role.into() }
    }

    pub fn too_many_system_messages(
        count: usize,
        provider: impl Into<String>,
        max_allowed: usize,
    ) -> Self {
        Self::TooManySystemMessages {
            count,
            provider: provider.into(),
            max_allowed,
        }
    }

    pub fn provider_limitation(provider: impl Into<String>, feature: impl Into<String>) -> Self {
        Self::ProviderLimitation {
            provider: provider.into(),
            feature: feature.into(),
        }
    }

    pub fn tool_not_found(name: impl Into<String>) -> Self {
        Self::ToolNotFound { name: name.into() }
    }

    pub fn tool_execution_failed(message: impl Into<String>) -> Self {
        Self::ToolExecutionFailed {
            message: message.into(),
        }
    }

    pub fn invalid_tool_call(reason: impl Into<String>) -> Self {
        Self::InvalidToolCall {
            reason: reason.into(),
        }
    }

    pub fn max_iterations_reached(max: usize) -> Self {
        Self::MaxIterationsReached { max }
    }
}

#[cfg(test)]
mod files_error_tests {
    use super::*;

    #[test]
    fn data_field_too_large_message() {
        let e = LlmError::DataFieldTooLarge { size: 50_000_000 };
        assert!(format!("{}", e).contains("30"));
        assert!(format!("{}", e).contains("50000000"));
    }

    #[test]
    fn url_without_document_id_message() {
        let e = LlmError::UrlWithoutDocumentId;
        assert!(format!("{}", e).to_lowercase().contains("id"));
    }

    #[test]
    fn provider_file_not_found_carries_id() {
        let e = LlmError::ProviderFileNotFound { provider_file_id: "file_abc".into() };
        assert!(format!("{}", e).contains("file_abc"));
    }
}
