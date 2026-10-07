//! `POST /v2/run`, the server side of a mounts call (Linux, dark: the route
//! exists only on a server that has a staging root, which is read only with
//! `COLMENA_LARGE_TABULAR` on). Authentication, readiness and the in-flight
//! bound are the `/v1/run` gate, applied to this route by the router.
//!
//! The request body is read as a bounded stream (see [`super::wire`]) straight
//! into the call's own data directory: nothing is buffered beyond one chunk, a
//! file's bytes are checked against its declared size, and the total against a
//! cap. The volume comes from `SubprocessExecutor::stage_call`, the budgeted
//! path, BEFORE the data is read; it is released when the handler ends however it
//! ends (a client that disconnects drops the handler, which drops the volume). The
//! code runs with mounts, `/out` is read back through the hardened collector, and
//! the kept files are streamed out while the volume is still mounted.
//!
//! Logs carry fields only: never code, inputs, outputs, paths or headers.

use super::collect::{collect_out, CollectLimits};
use super::stage::{create_file, make_dir, StageLimits};
use super::wire::{
    frame, CallHeader, Dropped, FileEntry, OutEntry, Reader, Refusal, ResponseHeader, RunStatus,
    HEADER_TIMEOUT, IDLE_TIMEOUT, MAX_FILES_IN, TRANSFER_MAX, WIRE_V2,
};
use crate::dag_engine::domain::python_executor::{PythonRunError, PythonRunRequest};
use crate::dag_engine::infrastructure::python_exec::server::AppState;
use crate::dag_engine::infrastructure::python_exec::staging::{
    valid_out_mb, StageError, StagedCall,
};
use crate::dag_engine::log_policy::T_PYTHON_EXEC;
use crate::tabular_prepare::manifest::{parse_part_path, MANIFEST_MAX_BYTES, MANIFEST_PATH};
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use std::collections::HashSet;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const CHUNK: usize = 1024 * 1024;

fn refuse(code: StatusCode, refusal: &str, reason: Option<String>, retry: bool) -> Response {
    let body = serde_json::to_vec(&Refusal {
        refusal: refusal.to_string(),
        reason,
    })
    .unwrap_or_default();
    let mut response = (code, body).into_response();
    let h = response.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        "application/json".parse().expect("static"),
    );
    if retry {
        h.insert(header::RETRY_AFTER, "1".parse().expect("static"));
    }
    response
}

fn bad(code: StatusCode, what: &str) -> Response {
    refuse(code, what, None, false)
}

/// The volume of a call, released off the async worker however the handler ends.
struct Volume(Option<StagedCall>);

impl Drop for Volume {
    fn drop(&mut self) {
        let Some(call) = self.0.take() else { return };
        match tokio::runtime::Handle::try_current() {
            Ok(rt) => {
                rt.spawn_blocking(move || drop(call));
            }
            Err(_) => drop(call),
        }
    }
}

/// A request path the server will create: the manifest or a canonical part.
fn allowed_path(path: &str) -> Option<Option<usize>> {
    if path == MANIFEST_PATH {
        return Some(None);
    }
    parse_part_path(path).map(|(table, _)| Some(table))
}

fn wire_failure(e: super::wire::WireError) -> Response {
    use super::wire::WireError::*;
    match e {
        Stalled | Deadline => bad(StatusCode::REQUEST_TIMEOUT, "timeout"),
        Early | Malformed | Transport => bad(StatusCode::BAD_REQUEST, "bad_request"),
    }
}

/// Reads the files of the request into `data_dir`. Returns the bytes written.
async fn receive<S, E>(
    reader: &mut Reader<S>,
    data_dir: &std::path::Path,
    limits: StageLimits,
) -> Result<u64, Response>
where
    S: futures::Stream<Item = Result<Bytes, E>> + Unpin,
{
    let too_large = || bad(StatusCode::PAYLOAD_TOO_LARGE, "too_large");
    let mut seen: HashSet<String> = HashSet::new();
    let mut tables: HashSet<usize> = HashSet::new();
    let mut total = 0u64;
    loop {
        let raw = match reader.frame().await {
            Ok(Some(raw)) => raw,
            Ok(None) => break,
            Err(e) => return Err(wire_failure(e)),
        };
        let entry: FileEntry = serde_json::from_slice(&raw)
            .map_err(|_| bad(StatusCode::BAD_REQUEST, "bad_request"))?;
        let table =
            allowed_path(&entry.path).ok_or_else(|| bad(StatusCode::BAD_REQUEST, "bad_request"))?;
        if seen.len() >= MAX_FILES_IN || !seen.insert(entry.path.clone()) {
            return Err(bad(StatusCode::BAD_REQUEST, "bad_request"));
        }
        let cap = match table {
            None => MANIFEST_MAX_BYTES as u64,
            Some(_) => limits.part_bytes,
        };
        // Declared size first: a file over its cap, or over the call's total, is
        // refused before one of its bytes is read.
        if entry.size > cap || total.saturating_add(entry.size) > limits.total_bytes {
            return Err(too_large());
        }
        if let Some(t) = table {
            if tables.insert(t) {
                make_dir(&data_dir.join(format!("t{t}")))
                    .await
                    .map_err(|_| bad(StatusCode::SERVICE_UNAVAILABLE, "busy"))?;
            }
        }
        let mut file = create_file(&data_dir.join(&entry.path))
            .await
            .map_err(|_| bad(StatusCode::SERVICE_UNAVAILABLE, "busy"))?;
        reader
            .copy_exact(entry.size, &mut file)
            .await
            .map_err(wire_failure)?;
        file.flush()
            .await
            .map_err(|_| bad(StatusCode::SERVICE_UNAVAILABLE, "busy"))?;
        total += entry.size;
    }
    reader.expect_end().await.map_err(wire_failure)?;
    Ok(total)
}

/// The reason out of the executor's refusal text, when it is that refusal.
fn mounts_disabled_reason(text: &str) -> Option<String> {
    let rest = text
        .split("run mounts are disabled on this executor (")
        .nth(1)?;
    let reason = rest.split(')').next()?;
    let plain = !reason.is_empty()
        && reason.len() <= 64
        && reason
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_');
    plain.then(|| reason.to_string())
}

fn request_id(headers: &axum::http::HeaderMap) -> String {
    let value = headers
        .get("x-colmena-request-id")
        .and_then(|v| v.to_str().ok());
    value
        .unwrap_or("-")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || "._:-".contains(*c))
        .take(64)
        .collect()
}

pub async fn run_mounts(State(st): State<AppState>, req: Request) -> Response {
    let started = Instant::now();
    let id = request_id(req.headers());
    // Refused before a byte of data is read.
    if let Some(reason) = st.exec.mounts_unavailable().await {
        let (code, refusal) = match reason.as_str() {
            "no_staging_root" => (StatusCode::NOT_IMPLEMENTED, "no_staging_root"),
            _ => (StatusCode::SERVICE_UNAVAILABLE, "mounts_disabled"),
        };
        tracing::info!(target: T_PYTHON_EXEC, request_id = %id, outcome = refusal, "python serve mounts run");
        return refuse(code, refusal, Some(reason), false);
    }
    let mut reader = Reader::new(
        req.into_body().into_data_stream(),
        IDLE_TIMEOUT,
        TRANSFER_MAX,
    );
    let header: CallHeader = match tokio::time::timeout(HEADER_TIMEOUT, reader.json()).await {
        Ok(Ok(h)) => h,
        Ok(Err(e)) => return wire_failure(e),
        Err(_) => return bad(StatusCode::REQUEST_TIMEOUT, "timeout"),
    };
    if header.v != WIRE_V2
        || !matches!(header.mode.as_str(), "none" | "restricted")
        || !valid_out_mb(header.out_mb)
    {
        return bad(StatusCode::BAD_REQUEST, "bad_request");
    }
    let staged = match st.exec.stage_call(header.out_mb) {
        Ok(s) => s,
        Err(StageError::OverBudget { .. }) => {
            tracing::info!(target: T_PYTHON_EXEC, request_id = %id, outcome = "busy", "python serve mounts run");
            return refuse(StatusCode::SERVICE_UNAVAILABLE, "busy", None, true);
        }
        Err(StageError::NoStagingRoot) => {
            return refuse(StatusCode::NOT_IMPLEMENTED, "no_staging_root", None, false)
        }
        Err(_) => return refuse(StatusCode::SERVICE_UNAVAILABLE, "busy", None, true),
    };
    let volume = Volume(Some(staged));
    let data_dir = volume.0.as_ref().expect("held").data_dir();
    let received = match receive(&mut reader, &data_dir, StageLimits::default()).await {
        Ok(n) => n,
        Err(response) => {
            tracing::info!(target: T_PYTHON_EXEC, request_id = %id, outcome = "upload_refused", "python serve mounts run");
            return response;
        }
    };
    let timeout = Duration::from_millis(header.timeout_ms).min(st.max_timeout);
    let run = PythonRunRequest {
        code: header.code,
        mode: header.mode,
        timeout: Some(timeout),
        inputs: header.inputs,
    };
    let mounts = volume.0.as_ref().expect("held").mounts();
    let outcome = st.exec.run_staged(run, mounts).await;
    let mut head = ResponseHeader {
        v: WIRE_V2,
        status: RunStatus::Ok,
        message: None,
        output: None,
        stdout: String::new(),
        files: vec![],
        dropped: vec![],
        too_many_entries: false,
    };
    let mut files = vec![];
    let label = match outcome {
        Ok(done) => {
            head.output = done.output;
            head.stdout = done.stdout;
            // The child is dead and the volume still mounted: read what it wrote.
            let out_dir = volume.0.as_ref().expect("held").out_dir();
            match collect_out(&out_dir, CollectLimits::default()) {
                Ok(found) => {
                    head.too_many_entries = found.too_many_entries;
                    head.dropped = found
                        .rejected
                        .iter()
                        .map(|r| Dropped {
                            name: r.name.clone(),
                            reason: format!("{:?}", r.reason),
                        })
                        .collect();
                    head.files = found
                        .files
                        .iter()
                        .map(|f| OutEntry {
                            name: f.name.clone(),
                            size: f.size,
                        })
                        .collect();
                    files = found.files;
                    "completed"
                }
                Err(_) => {
                    head.status = RunStatus::Internal;
                    head.message = Some("the output could not be read".into());
                    "output_unreadable"
                }
            }
        }
        Err(PythonRunError::Python(text)) => {
            head.status = RunStatus::PythonError;
            head.message = Some(text);
            "python_error"
        }
        Err(PythonRunError::Timeout) => {
            head.status = RunStatus::Timeout;
            "timeout"
        }
        Err(PythonRunError::Internal(text)) => {
            if let Some(reason) = mounts_disabled_reason(&text) {
                return refuse(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "mounts_disabled",
                    Some(reason),
                    false,
                );
            }
            head.status = RunStatus::Internal;
            head.message = Some("the executor failed".into());
            "internal"
        }
    };
    tracing::info!(
        target: T_PYTHON_EXEC, request_id = %id, outcome = label, received,
        files = files.len(), duration_ms = started.elapsed().as_millis() as u64,
        "python serve mounts run"
    );
    let header_bytes = frame(&serde_json::to_vec(&head).unwrap_or_default());
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(2);
    // The task owns the volume until the last byte is sent, or the client is gone.
    tokio::spawn(async move {
        let _volume = volume;
        let send = |item| async {
            matches!(
                tokio::time::timeout(IDLE_TIMEOUT, tx.send(item)).await,
                Ok(Ok(()))
            )
        };
        let deadline = tokio::time::Instant::now() + TRANSFER_MAX;
        if !send(Ok(header_bytes)).await {
            return;
        }
        for out in files {
            let size = out.size;
            let mut file = tokio::fs::File::from_std(out.into_file());
            let mut left = size;
            while left > 0 {
                let mut buf = vec![0u8; left.min(CHUNK as u64) as usize];
                let n = match file.read(&mut buf).await {
                    Ok(n) if n > 0 => n,
                    _ => {
                        // Shorter than checked: the body ends in an error.
                        let _ = tx
                            .send(Err(std::io::Error::other("output ended early")))
                            .await;
                        return;
                    }
                };
                buf.truncate(n);
                left -= n as u64;
                if tokio::time::Instant::now() >= deadline || !send(Ok(Bytes::from(buf))).await {
                    return;
                }
            }
        }
    });
    let stream = tokio_stream::wrappers::ReceiverStream::new(rx);
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/octet-stream")],
        Body::from_stream(stream),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_manifest_and_canonical_part_paths_are_allowed() {
        assert_eq!(allowed_path("manifest.json"), Some(None));
        assert_eq!(allowed_path("t0/part-00000.parquet"), Some(Some(0)));
        assert_eq!(allowed_path("t12/part-00031.parquet"), Some(Some(12)));
        for bad in [
            "../manifest.json",
            "/etc/passwd",
            "t0/../part-00000.parquet",
            "t00/part-00000.parquet",
            "t0/part-0.parquet",
            "t0/part-00000.parquet/",
            "a/b",
            "",
            "manifest.json ",
        ] {
            assert_eq!(allowed_path(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn the_mounts_disabled_reason_is_read_only_when_it_is_plain() {
        let text = "PythonExecutorError: run mounts are disabled on this executor (arrow_pool_not_system); see its startup log";
        assert_eq!(
            mounts_disabled_reason(text).as_deref(),
            Some("arrow_pool_not_system")
        );
        assert_eq!(
            mounts_disabled_reason("PythonExecutorError: it crashed"),
            None
        );
        let tricky = "run mounts are disabled on this executor (gs://secret/key)";
        assert_eq!(mounts_disabled_reason(tricky), None);
    }

    #[test]
    fn a_request_id_is_cleaned_for_the_log() {
        let mut h = axum::http::HeaderMap::new();
        h.insert("x-colmena-request-id", "a b;c-d_1".parse().unwrap());
        assert_eq!(request_id(&h), "abc-d_1");
        assert_eq!(request_id(&axum::http::HeaderMap::new()), "-");
    }
}
