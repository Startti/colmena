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

    #[error("Internal error: {message}")]
    InternalError { message: String },
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
}