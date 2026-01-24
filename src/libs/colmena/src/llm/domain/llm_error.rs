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
