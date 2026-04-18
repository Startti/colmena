//! Error types for the SQL node validation and execution pipeline.

use std::fmt;

/// Errors produced by the SQL node pipeline (validation, critic, execution).
#[derive(Debug)]
pub enum SqlNodeError {
    /// Query blocked by static validator rules.
    Blocked { rule: String, message: String },
    /// Query blocked by LLM critic (security concern).
    CriticRejected { reason: String },
    /// Could not connect to PostgreSQL or pool creation failed.
    ConnectionError(String),
    /// Query execution failed at the PostgreSQL level.
    ExecutionError(String),
    /// Permission configuration is invalid.
    ConfigError(String),
}

impl fmt::Display for SqlNodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Blocked { rule, message } => {
                write!(f, "BLOCKED by static validator ({}): {}", rule, message)
            }
            Self::CriticRejected { reason } => {
                write!(f, "BLOCKED by LLM critic: {}", reason)
            }
            Self::ConnectionError(msg) => write!(f, "SQL connection error: {}", msg),
            Self::ExecutionError(msg) => write!(f, "SQL execution error: {}", msg),
            Self::ConfigError(msg) => write!(f, "SQL config error: {}", msg),
        }
    }
}

impl std::error::Error for SqlNodeError {}
