//! Port for running Python code. The engine hands over code plus JSON inputs
//! and receives the JSON `output` global plus captured stdout. Executors decide
//! where the code runs: this process, an isolated child process, or a remote
//! service. See docs/developer_guide/53_python_executors.md.

use async_trait::async_trait;
use serde_json::{Map, Value};
use std::time::Duration;

/// What the code produced: the `output` global (when assigned) and stdout.
#[derive(Debug, Clone, PartialEq)]
pub struct PythonRunResult {
    /// `None` when the code never assigned `output`; `Some(Value::Null)` when
    /// it assigned `None`.
    pub output: Option<Value>,
    pub stdout: String,
}

/// One execution. `code` already carries any prelude/postlude the caller adds.
#[derive(Debug, Clone)]
pub struct PythonRunRequest {
    pub code: String,
    /// `"none"` or `"restricted"`, same values as the `python_script` node.
    pub mode: String,
    /// `None` means no deadline of its own (`python_script` in `none` mode).
    pub timeout: Option<Duration>,
    pub inputs: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum PythonRunError {
    /// Text produced while running the code (validation, syntax, exception,
    /// conversion, resource limit). Shown to the model as is.
    #[error("{0}")]
    Python(String),
    /// The deadline passed. Callers keep their own timeout message.
    #[error("python execution timed out")]
    Timeout,
    /// The executor itself failed (join error, unavailable, misconfigured).
    #[error("{0}")]
    Internal(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutorKind {
    InProcess,
    Subprocess,
    Remote,
}

impl ExecutorKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ExecutorKind::InProcess => "inprocess",
            ExecutorKind::Subprocess => "subprocess",
            ExecutorKind::Remote => "remote",
        }
    }
}

#[async_trait]
pub trait PythonExecutor: Send + Sync {
    fn kind(&self) -> ExecutorKind;
    async fn run(&self, req: PythonRunRequest) -> Result<PythonRunResult, PythonRunError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_names_are_the_env_values() {
        assert_eq!(ExecutorKind::InProcess.as_str(), "inprocess");
        assert_eq!(ExecutorKind::Subprocess.as_str(), "subprocess");
        assert_eq!(ExecutorKind::Remote.as_str(), "remote");
    }

    #[test]
    fn python_error_displays_its_text_verbatim() {
        let e = PythonRunError::Python("SandboxViolation: x".into());
        assert_eq!(e.to_string(), "SandboxViolation: x");
    }
}
