//! What runs inside the per-call Python process: read one request, run it with
//! the same helper the in-process executor uses, write one response.

use super::frame;
use super::protocol::{self, WireRequest, WireResponse, WireStatus, WIRE_VERSION};
use crate::dag_engine::infrastructure::nodes::python_node::execute_sandboxed_helper;
use serde::{Deserialize, Serialize};
use std::io::{self, Read, Write};
use std::time::Instant;

/// Cap on the `CallHeader` frame a host sends before the first request on a
/// stream. It is enforced the same way a request body is — a length prefix
/// checked against a limit before anything is read into memory
/// (`frame::read_frame`) — just with this size in place of a per-call
/// `max_request_bytes`.
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

/// A `{ "v": u32 }`-only view of a request body, parsed before the full
/// [`WireRequest`] shape is: a request for another version doesn't have to
/// match this side's current field list to be recognized as one.
#[derive(Deserialize)]
struct VersionOnly {
    v: u32,
}

/// Reads one [`WireRequest`] from `stream` and writes back exactly one
/// [`WireResponse`].
///
/// A frame over `max_request_bytes` is refused before it is read into memory
/// ([`frame::is_frame_too_large`]), and the caller gets a well-formed
/// [`WireStatus::TooLarge`] response rather than a dropped connection — but
/// only once the host manages to write that response's own (small) frame
/// back to it. A host whose *original* oversized write itself failed (the
/// far side stops reading once it has seen the length prefix exceed its own
/// cap, which surfaces to the writer as a `BrokenPipe` rather than a
/// completed write) never receives this response at all. A host is
/// expected to pre-check a request's size against the cap it advertised in
/// [`CallHeader`] before writing it, or to attempt a read after a failed
/// write; either way, the stream is single-use once a `TooLarge` happens —
/// the unread body bytes are still sitting in it.
///
/// A request for a protocol version other than [`WIRE_VERSION`] is answered
/// with a response stamped with THIS side's own [`WIRE_VERSION`] — never
/// the request's — so a host that sent a different version sees a `v` on
/// the reply that disagrees with the one it sent. That disagreement is
/// exactly what the host's own `WireResponse::into_result` check (which
/// runs before it looks at `status`) is built to catch, turning the reply
/// into a clear internal error instead of misreading it as a Python-side
/// one. The version is checked from a `{ "v": u32 }`-only parse of the
/// body, so a request of another version still gets this answer even when
/// the rest of its shape doesn't match [`WireRequest`] at all.
///
/// Anything else that isn't well-formed (not valid JSON, or a stream that
/// closes mid-frame) is a transport-level failure and is returned as an
/// `io::Error` instead of a response, since there is nothing to answer
/// with — as is a panic unwinding out of the helper this calls. Either way
/// nothing is written back, so a peer reading the stream only sees it
/// close: the same shape of failure `WireStatus::Crashed`/`CRASHED_MESSAGE`
/// exist to describe. The process hosting this call is expected to exit
/// after such an `Err` or a panic rather than read another request off the
/// same stream.
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
    let version = serde_json::from_slice::<VersionOnly>(&bytes)
        .map_err(invalid)?
        .v;
    if version != WIRE_VERSION {
        let response = WireResponse::status_only(
            WireStatus::PythonError,
            Some(format!(
                "PythonExecutorError: unsupported protocol version {}",
                version
            )),
        );
        return frame::write_frame(stream, &serde_json::to_vec(&response).map_err(invalid)?);
    }
    let req: WireRequest = serde_json::from_slice(&bytes).map_err(invalid)?;
    drop(bytes);
    let started = Instant::now();
    let result = execute_sandboxed_helper(
        &req.code,
        &req.mode,
        req.timeout_ms.div_ceil(1000),
        &req.inputs,
    );
    let response = WireResponse::from_helper(result, started.elapsed().as_millis() as u64);
    frame::write_frame(stream, &serde_json::to_vec(&response).map_err(invalid)?)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
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

    /// The request this test sends serializes to well over 16 bytes (a full
    /// `WireRequest` here comes out to around 90), so a 16-byte cap refuses
    /// it on the length prefix alone: `handle_request` never reads or
    /// allocates the body, yet the caller still gets a well-formed response
    /// back instead of a closed connection. The exact MiB rendering of a
    /// 16-byte cap (`0 MiB`) is not the point of this test, so only the
    /// message's prefix is pinned.
    #[test]
    fn an_oversized_request_gets_a_too_large_response_before_parsing() {
        let r = call(&wire("output = 1", "none"), 16).unwrap();
        assert_eq!(r.status, WireStatus::TooLarge);
        assert!(r.message.unwrap().starts_with("Python execution error:"));
    }

    /// The child answers a version mismatch with a response stamped with
    /// ITS OWN version, never the request's. `WireResponse::into_result`
    /// (`protocol.rs`) treats a response's `v` as unsupported precisely
    /// when it differs from the host's own `WIRE_VERSION` — so under a
    /// real skew (a host that only speaks a different version asks this
    /// child something), the child's reply disagreeing with the version
    /// the request claimed is exactly the signal that check is built to
    /// catch. Request and response necessarily share this one binary's
    /// `WIRE_VERSION` constant, so the two can't literally diverge the way
    /// a real skew would; the assertions below instead pin the invariant
    /// that makes the host's check correct for any host whose own version
    /// is what this request claimed: the reply carries the child's own
    /// version, and it disagrees with the request's.
    #[test]
    fn a_request_for_another_version_is_a_clear_error() {
        let mut req = wire("output = 1", "none");
        req.v = WIRE_VERSION + 1;
        let r = call(&req, 1 << 20).unwrap();
        assert_eq!(r.v, WIRE_VERSION);
        assert_ne!(r.v, req.v);
    }

    /// A version mismatch is caught from a `{ "v": u32 }`-only parse before
    /// the full `WireRequest` shape is even attempted, so a request whose
    /// other fields don't match this side's current shape at all still
    /// gets the clear version error instead of a parse failure.
    #[test]
    fn a_version_mismatch_is_detected_before_the_full_shape_is_parsed() {
        let bytes =
            serde_json::to_vec(&serde_json::json!({ "v": 999, "unexpected": "shape" })).unwrap();
        let (mut ours, mut theirs) = UnixStream::pair().unwrap();
        let t = std::thread::spawn(move || handle_request(&mut theirs, 1 << 20));
        frame::write_frame(&mut ours, &bytes).unwrap();
        let out = frame::read_frame(&mut ours, 1 << 20).unwrap();
        t.join().unwrap().unwrap();
        let r: WireResponse = serde_json::from_slice(&out).unwrap();
        assert_eq!(r.v, WIRE_VERSION);
        assert_eq!(r.status, WireStatus::PythonError);
        assert!(r
            .message
            .unwrap()
            .contains("unsupported protocol version 999"));
    }

    /// A stream that closes mid-frame (the length prefix promises more than
    /// ever arrives) is a transport failure: `handle_request` returns an
    /// `Err` and writes nothing back, so the peer's own read sees EOF
    /// instead of a response.
    #[test]
    fn a_truncated_request_is_a_transport_error_with_no_response() {
        let (mut ours, mut theirs) = UnixStream::pair().unwrap();
        ours.write_all(&50u32.to_be_bytes()).unwrap();
        ours.write_all(&[0u8; 10]).unwrap();
        ours.shutdown(std::net::Shutdown::Write).unwrap();
        let t = std::thread::spawn(move || handle_request(&mut theirs, 1 << 20));
        let err = t.join().unwrap().unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
        let mut buf = [0u8; 1];
        assert_eq!(ours.read(&mut buf).unwrap(), 0);
    }

    /// A well-formed frame whose body isn't valid JSON at all is the same
    /// kind of transport failure as a truncated one: `handle_request`
    /// returns an `Err` and writes nothing back.
    #[test]
    fn a_malformed_json_body_is_a_transport_error_with_no_response() {
        let (mut ours, mut theirs) = UnixStream::pair().unwrap();
        let t = std::thread::spawn(move || handle_request(&mut theirs, 1 << 20));
        frame::write_frame(&mut ours, b"not json").unwrap();
        let err = t.join().unwrap().unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        let mut buf = [0u8; 1];
        assert_eq!(ours.read(&mut buf).unwrap(), 0);
    }
}
