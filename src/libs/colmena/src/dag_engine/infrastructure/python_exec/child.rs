//! What runs inside the per-call Python process: read one request, run it with
//! the same helper the in-process executor uses, write one response.

use super::frame;
use super::protocol::{self, WireRequest, WireResponse, WireStatus, WIRE_VERSION};
use crate::dag_engine::infrastructure::nodes::python_node::execute_sandboxed_helper;
use serde::{Deserialize, Serialize};
use std::io::{self, Read, Write};
use std::time::Instant;

pub const MAX_HEADER_BYTES: usize = 4096;

/// Per-call settings sent before the request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CallHeader {
    pub slot: u32,
    pub memory_mb: u64,
    pub cpu_secs: u64,
    pub max_request_bytes: usize,
}

fn invalid(e: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, e.to_string())
}

/// Reads one [`WireRequest`] from `stream` and writes back exactly one
/// [`WireResponse`].
///
/// A frame over `max_request_bytes` is refused before it is read into memory
/// ([`frame::is_frame_too_large`]); the caller still gets a proper
/// [`WireStatus::TooLarge`] response rather than a dropped connection. A
/// request for a protocol version other than [`WIRE_VERSION`] is answered
/// with a response carrying that same (unsupported) version, so the host's
/// own [`WireResponse::into_result`] check — which runs before it looks at
/// `status` — turns it into a clear error without this side having to
/// duplicate that wording. Anything else that isn't a well-formed
/// [`WireRequest`] (not valid JSON, or a stream that closes mid-frame) is a
/// transport-level failure and is returned as an `io::Error` instead of a
/// response, since there is nothing to answer with.
pub fn handle_request<S: Read + Write>(stream: &mut S, max_request_bytes: usize) -> io::Result<()> {
    let bytes = match frame::read_frame(stream, max_request_bytes) {
        Ok(bytes) => bytes,
        Err(e) if frame::is_frame_too_large(&e) => {
            let response = WireResponse::status_only(
                WireStatus::TooLarge,
                Some(protocol::input_too_large_message(max_request_bytes)),
            );
            return frame::write_frame(stream, &serde_json::to_vec(&response).map_err(invalid)?);
        }
        Err(e) => return Err(e),
    };
    let req: WireRequest = serde_json::from_slice(&bytes).map_err(invalid)?;
    drop(bytes);
    let response = if req.v != WIRE_VERSION {
        let mut response = WireResponse::status_only(
            WireStatus::PythonError,
            Some(format!(
                "PythonExecutorError: unsupported protocol version {}",
                req.v
            )),
        );
        response.v = req.v;
        response
    } else {
        let started = Instant::now();
        let result = execute_sandboxed_helper(
            &req.code,
            &req.mode,
            req.timeout_ms.div_ceil(1000),
            &req.inputs,
        );
        WireResponse::from_helper(result, started.elapsed().as_millis() as u64)
    };
    frame::write_frame(stream, &serde_json::to_vec(&response).map_err(invalid)?)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::dag_engine::domain::python_executor::PythonRunError;
    use std::os::unix::net::UnixStream;

    fn call(req: &WireRequest, max: usize) -> io::Result<WireResponse> {
        pyo3::Python::initialize();
        let (mut ours, mut theirs) = UnixStream::pair()?;
        let t = std::thread::spawn(move || handle_request(&mut theirs, max));
        frame::write_frame(&mut ours, &serde_json::to_vec(req).unwrap())?;
        let bytes = frame::read_frame(&mut ours, 1 << 20);
        t.join().unwrap()?;
        Ok(serde_json::from_slice(&bytes?).unwrap())
    }

    fn wire(code: &str, mode: &str) -> WireRequest {
        let mut inputs = serde_json::Map::new();
        inputs.insert("rows".into(), serde_json::json!([{"a": 1}, {"a": 2}]));
        WireRequest {
            v: WIRE_VERSION,
            code: code.into(),
            mode: mode.into(),
            timeout_ms: 5000,
            inputs,
        }
    }

    // Deliberately avoids pandas (unlike the sandbox's own module allowlist,
    // which does cover it): this test's job is to check that the child wires
    // stdout, inputs and output through `execute_sandboxed_helper` the same
    // way the in-process executor does, not to re-test pandas itself. CI's
    // Rust job never installs pandas into the embedded interpreter, so a
    // pandas import here would fail there the same way it fails without a
    // `.venv`-style `PYTHONPATH` locally.
    #[test]
    fn runs_the_same_helper() {
        let r = call(
            &wire(
                "print('n')\noutput = sum(r['a'] for r in rows)",
                "restricted",
            ),
            1 << 20,
        )
        .unwrap();
        assert_eq!(r.status, WireStatus::Ok);
        assert_eq!(r.output, Some(serde_json::json!(3)));
        assert_eq!(r.stdout, "n\n");
    }

    #[test]
    fn validation_errors_come_back_as_python_errors() {
        let r = call(&wire("import os", "restricted"), 1 << 20).unwrap();
        assert_eq!(r.status, WireStatus::PythonError);
        assert!(r.message.unwrap().starts_with("SandboxViolation:"));
    }

    /// The length prefix alone (16 bytes over the 16-byte cap) is enough to
    /// refuse the request: `handle_request` never reads or allocates the
    /// body, yet the caller still gets a well-formed response back instead
    /// of a closed connection.
    #[test]
    fn an_oversized_request_gets_a_too_large_response_before_parsing() {
        let r = call(&wire("output = 1", "none"), 16).unwrap();
        assert_eq!(r.status, WireStatus::TooLarge);
        assert_eq!(r.message, Some(protocol::input_too_large_message(16)));
    }

    /// A request for a version this child doesn't speak comes back as a
    /// response the host's own `into_result` already knows how to turn into
    /// a clear, actionable error — the child does not need to invent its own
    /// wording for it.
    #[test]
    fn a_request_for_another_version_is_a_clear_error() {
        let mut req = wire("output = 1", "none");
        req.v = WIRE_VERSION + 1;
        let r = call(&req, 1 << 20).unwrap();
        let err = r.into_result().unwrap_err();
        match err {
            PythonRunError::Internal(m) => {
                assert!(m.contains("unsupported protocol version"), "{m}");
            }
            other => panic!("unexpected {other:?}"),
        }
    }
}
