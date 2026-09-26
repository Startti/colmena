//! Messages exchanged with an isolated Python executor (local or remote).
//! A response is untrusted data: it is size-capped and parsed, nothing more.

use crate::dag_engine::domain::python_executor::{
    PythonRunError, PythonRunRequest, PythonRunResult,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::time::Duration;

pub const WIRE_VERSION: u32 = 1;

pub const CRASHED_MESSAGE: &str = "Python execution error: the Python process ended without returning a result (it may have exceeded its memory or CPU limit)";
pub const MALFORMED_MESSAGE: &str =
    "Python execution error: the Python process returned a malformed result";

pub fn input_too_large_message(limit_bytes: usize) -> String {
    format!(
        "Python execution error: the input exceeds the Python executor limit of {} MiB",
        limit_bytes / (1024 * 1024)
    )
}

pub fn result_too_large_message(limit_bytes: usize) -> String {
    format!(
        "Python execution error: the result exceeds the Python executor limit of {} MiB",
        limit_bytes / (1024 * 1024)
    )
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireRequest {
    pub v: u32,
    pub code: String,
    pub mode: String,
    pub timeout_ms: u64,
    pub inputs: Map<String, Value>,
}

impl WireRequest {
    pub fn new(req: PythonRunRequest, timeout: Duration) -> Self {
        Self {
            v: WIRE_VERSION,
            code: req.code,
            mode: req.mode,
            timeout_ms: timeout.as_millis() as u64,
            inputs: req.inputs,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WireStatus {
    Ok,
    PythonError,
    Timeout,
    Crashed,
    TooLarge,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WireResponse {
    pub v: u32,
    pub status: WireStatus,
    /// JSON cannot tell "never assigned" from "assigned None"; this can.
    #[serde(default)]
    pub output_set: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<Value>,
    #[serde(default)]
    pub stdout: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default)]
    pub exec_ms: u64,
}

impl WireResponse {
    pub fn from_helper(result: Result<PythonRunResult, String>, exec_ms: u64) -> Self {
        match result {
            Ok(r) => Self {
                v: WIRE_VERSION,
                status: WireStatus::Ok,
                output_set: r.output.is_some(),
                output: r.output,
                stdout: r.stdout,
                message: None,
                exec_ms,
            },
            // Same as the in-process callers: an error carries no stdout.
            Err(message) => Self {
                message: Some(message),
                ..Self::status_only(WireStatus::PythonError, None)
            }
            .with_exec_ms(exec_ms),
        }
    }

    pub fn status_only(status: WireStatus, message: Option<String>) -> Self {
        Self {
            v: WIRE_VERSION,
            status,
            output_set: false,
            output: None,
            stdout: String::new(),
            message,
            exec_ms: 0,
        }
    }

    fn with_exec_ms(mut self, exec_ms: u64) -> Self {
        self.exec_ms = exec_ms;
        self
    }

    pub fn into_result(self) -> Result<PythonRunResult, PythonRunError> {
        if self.v != WIRE_VERSION {
            return Err(PythonRunError::Internal(format!(
                "PythonExecutorError: unsupported protocol version {}",
                self.v
            )));
        }
        match self.status {
            WireStatus::Ok => Ok(PythonRunResult {
                output: self.output_set.then(|| self.output.unwrap_or(Value::Null)),
                stdout: self.stdout,
            }),
            WireStatus::PythonError => {
                Err(PythonRunError::Python(self.message.unwrap_or_default()))
            }
            WireStatus::Timeout => Err(PythonRunError::Timeout),
            WireStatus::Crashed => Err(PythonRunError::Python(CRASHED_MESSAGE.to_string())),
            WireStatus::TooLarge => Err(PythonRunError::Python(
                self.message
                    .unwrap_or_else(|| MALFORMED_MESSAGE.to_string()),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn output_set_distinguishes_unset_from_none() {
        let unset = WireResponse::from_helper(
            Ok(PythonRunResult {
                output: None,
                stdout: "".into(),
            }),
            1,
        );
        let none = WireResponse::from_helper(
            Ok(PythonRunResult {
                output: Some(Value::Null),
                stdout: "".into(),
            }),
            1,
        );
        let back = |r: WireResponse| {
            serde_json::from_slice::<WireResponse>(&serde_json::to_vec(&r).unwrap())
                .unwrap()
                .into_result()
                .unwrap()
        };
        assert_eq!(back(unset).output, None);
        assert_eq!(back(none).output, Some(Value::Null));
    }

    #[test]
    fn python_errors_travel_verbatim_without_stdout() {
        let r = WireResponse::from_helper(
            Err("Python execution error: ZeroDivisionError: x".into()),
            1,
        );
        assert_eq!(r.stdout, "");
        assert_eq!(
            r.into_result().unwrap_err(),
            PythonRunError::Python("Python execution error: ZeroDivisionError: x".into())
        );
    }

    #[test]
    fn statuses_map_to_run_errors() {
        assert_eq!(
            WireResponse::status_only(WireStatus::Timeout, None)
                .into_result()
                .unwrap_err(),
            PythonRunError::Timeout
        );
        assert_eq!(
            WireResponse::status_only(WireStatus::Crashed, None)
                .into_result()
                .unwrap_err(),
            PythonRunError::Python(CRASHED_MESSAGE.into())
        );
        let mut r = WireResponse::status_only(WireStatus::Timeout, None);
        r.v = 2;
        assert!(matches!(r.into_result(), Err(PythonRunError::Internal(_))));
    }

    #[test]
    fn request_carries_the_deadline_in_ms() {
        let req = PythonRunRequest {
            code: "c".into(),
            mode: "none".into(),
            timeout: None,
            inputs: Default::default(),
        };
        let w = WireRequest::new(req, std::time::Duration::from_secs(30));
        assert_eq!(
            serde_json::to_value(&w).unwrap()["timeout_ms"],
            json!(30000)
        );
    }
}
