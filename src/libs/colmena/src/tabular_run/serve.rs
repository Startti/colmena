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
use super::volume::Volume;
use super::wire::{
    try_frame, CallHeader, Dropped, FileEntry, OutEntry, Reader, Refusal, ResponseHeader,
    RunStatus, HEADER_TIMEOUT, IDLE_TIMEOUT, MAX_FILES_IN, MIN_BYTES_PER_SEC, OUTPUT_WIRE_MAX,
    RATE_GRACE, STDOUT_WIRE_MAX, TRANSFER_MAX, WIRE_V2,
};
use crate::dag_engine::domain::python_executor::{PythonRunError, PythonRunRequest};
use crate::dag_engine::infrastructure::python_exec::server::AppState;
use crate::dag_engine::infrastructure::python_exec::staging::{valid_out_mb, StageError};
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

/// The deadline a call may have: what it asked for, never above the server's cap.
fn call_timeout(asked_ms: u64, max: Duration) -> Duration {
    Duration::from_millis(asked_ms).min(max)
}

/// Whether the executor's budget has room for a volume of `out_mb` now. A guess
/// for a probe (a volume taken a moment later still wins), made without mounting.
fn would_stage(
    exec: &crate::dag_engine::infrastructure::python_exec::subprocess::SubprocessExecutor,
    out_mb: u64,
) -> bool {
    use crate::dag_engine::infrastructure::python_exec::staging::{
        STAGED_OUT_MIB_MAX, STAGED_VOLUMES_MAX,
    };
    let (volumes, mib) = exec.staged_in_flight();
    volumes < STAGED_VOLUMES_MAX && mib.saturating_add(out_mb) <= STAGED_OUT_MIB_MAX
}

/// How a reason `mounts_unavailable` gave becomes an answer. The reason is
/// cleaned with the charset every other path uses (letters, digits, `_`, 64): the
/// template's word is not trusted to be plain. A template that is not ready is
/// the executor's moment, not a decision to turn mounts off.
fn mounts_refusal(reason: &str) -> (StatusCode, &'static str, Option<String>) {
    let plain = !reason.is_empty()
        && reason.len() <= 64
        && reason
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_');
    match (reason, plain) {
        ("no_staging_root", _) => (StatusCode::NOT_IMPLEMENTED, "no_staging_root", None),
        ("template_not_ready", _) => (StatusCode::SERVICE_UNAVAILABLE, "executor_not_ready", None),
        (r, true) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "mounts_disabled",
            Some(r.to_string()),
        ),
        _ => (StatusCode::SERVICE_UNAVAILABLE, "mounts_disabled", None),
    }
}

/// The response header made to fit a frame: stdout and the result are cut, and the
/// run is still reported as the run it was, never as a failure to answer.
fn fit_header(head: &mut ResponseHeader) {
    if head.stdout.len() > STDOUT_WIRE_MAX {
        let mut end = STDOUT_WIRE_MAX;
        while !head.stdout.is_char_boundary(end) {
            end -= 1;
        }
        head.stdout.truncate(end);
        head.stdout.push_str("\n[truncated]");
    }
    if let Some(output) = &head.output {
        if output.to_string().len() > OUTPUT_WIRE_MAX {
            head.output = Some(serde_json::Value::String(
                "[the result was too large to return: aggregate it or write it with emit_table]"
                    .into(),
            ));
        }
    }
    if let Some(m) = &head.message {
        if m.len() > STDOUT_WIRE_MAX {
            let mut end = STDOUT_WIRE_MAX;
            while !m.is_char_boundary(end) {
                end -= 1;
            }
            head.message = Some(m[..end].to_string());
        }
    }
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
        let (code, refusal, reason) = mounts_refusal(&reason);
        tracing::info!(target: T_PYTHON_EXEC, request_id = %id, outcome = refusal, "python serve mounts run");
        return refuse(code, refusal, reason, false);
    }
    // A peer that trickles under the idle limit still holds a volume: past a
    // grace it must keep a minimum rate.
    let mut reader = Reader::new(
        req.into_body().into_data_stream(),
        IDLE_TIMEOUT,
        TRANSFER_MAX,
    )
    .with_min_rate(MIN_BYTES_PER_SEC, RATE_GRACE);
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
    if header.probe {
        // Answered from the budget alone: no volume is mounted, so a probe can neither
        // take a share a real call needs nor leave one to be released later.
        return match would_stage(&st.exec, header.out_mb) {
            true => StatusCode::NO_CONTENT.into_response(),
            false => refuse(StatusCode::SERVICE_UNAVAILABLE, "busy", None, true),
        };
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
        // The volume could not be made (the staging volume's I/O): not "busy".
        Err(_) => return refuse(StatusCode::SERVICE_UNAVAILABLE, "volume_io", None, false),
    };
    let volume = Volume::new(staged);
    let data_dir = volume.get().data_dir();
    let received = match receive(&mut reader, &data_dir, StageLimits::default()).await {
        Ok(n) => n,
        Err(response) => {
            tracing::info!(target: T_PYTHON_EXEC, request_id = %id, outcome = "upload_refused", "python serve mounts run");
            return response;
        }
    };
    let timeout = call_timeout(header.timeout_ms, st.max_timeout);
    let run = PythonRunRequest {
        code: header.code,
        mode: header.mode,
        timeout: Some(timeout),
        inputs: header.inputs,
    };
    let mounts = volume.get().mounts();
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
            let out_dir = volume.get().out_dir();
            let listing = tokio::task::spawn_blocking(move || {
                collect_out(&out_dir, CollectLimits::default())
            })
            .await
            .unwrap_or_else(|_| Err(std::io::Error::other("collection panicked")));
            match listing {
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
    fit_header(&mut head);
    let header_bytes = match serde_json::to_vec(&head)
        .map_err(|_| ())
        .and_then(|j| try_frame(&j).map_err(|_| ()))
    {
        Ok(b) => b,
        // Cannot happen after `fit_header` (fixed fields, capped sections); if it did,
        // the run is not reported as completed.
        Err(()) => {
            return refuse(
                StatusCode::INTERNAL_SERVER_ERROR,
                "bad_request",
                None,
                false,
            )
        }
    };
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
        let begun = tokio::time::Instant::now();
        let deadline = begun + TRANSFER_MAX;
        let mut sent_bytes = 0u64;
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
                // A reader slower than the minimum rate (past the grace) holds the volume
                // for the whole transfer limit: it is cut off like a slow uploader.
                sent_bytes += n as u64;
                let elapsed = begun.elapsed();
                let too_slow = elapsed > RATE_GRACE
                    && (sent_bytes as f64) < elapsed.as_secs_f64() * MIN_BYTES_PER_SEC as f64;
                if too_slow
                    || tokio::time::Instant::now() >= deadline
                    || !send(Ok(Bytes::from(buf))).await
                {
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

    /// The template's word is cleaned before it is passed on, and a template that
    /// is not ready is not "mounts disabled".
    #[test]
    fn a_mounts_unavailable_reason_becomes_the_right_distinct_answer() {
        assert_eq!(mounts_refusal("no_staging_root").1, "no_staging_root");
        assert_eq!(mounts_refusal("template_not_ready").1, "executor_not_ready");
        let (code, refusal, reason) = mounts_refusal("arrow_pool_not_system");
        assert_eq!(
            (code, refusal, reason.as_deref()),
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "mounts_disabled",
                Some("arrow_pool_not_system")
            )
        );
        for hostile in [
            "gs://secret/key",
            "has space",
            &"x".repeat(65),
            "line\nbreak",
            "",
        ] {
            let (_, refusal, reason) = mounts_refusal(hostile);
            assert_eq!((refusal, reason), ("mounts_disabled", None), "{hostile:?}");
        }
    }

    /// A response header that cannot fit a frame is cut to fit, and still reports
    /// the run as what it was.
    #[test]
    fn a_response_header_with_huge_stdout_and_result_is_cut_to_fit_a_frame() {
        let mut head = ResponseHeader {
            v: WIRE_V2,
            status: RunStatus::Ok,
            message: Some("m".repeat(500_000)),
            output: Some(serde_json::json!(vec!["x".repeat(100); 20_000])),
            stdout: "é".repeat(200_000),
            files: vec![],
            dropped: vec![],
            too_many_entries: false,
        };
        fit_header(&mut head);
        assert_eq!(head.status, RunStatus::Ok);
        assert!(head.stdout.ends_with("[truncated]") && head.stdout.len() <= STDOUT_WIRE_MAX + 20);
        assert!(head
            .output
            .as_ref()
            .unwrap()
            .as_str()
            .unwrap()
            .contains("emit_table"));
        let json = serde_json::to_vec(&head).unwrap();
        assert!(try_frame(&json).is_ok(), "{} bytes", json.len());
    }

    // ---- what the server accepts of an upload, with no jail and no privileges ----

    use super::super::wire::{end_frame, frame, HEADER_MAX};
    use futures::stream;

    async fn upload(
        entries: &[(&str, u64)],
        body: Vec<u8>,
        limits: StageLimits,
    ) -> Result<u64, StatusCode> {
        let mut bytes = vec![];
        for (path, size) in entries {
            let e = serde_json::to_vec(&FileEntry {
                path: path.to_string(),
                size: *size,
            })
            .unwrap();
            bytes.extend_from_slice(&frame(&e));
            bytes.extend_from_slice(&body[..(*size as usize).min(body.len())]);
        }
        bytes.extend_from_slice(&end_frame());
        let chunks: Vec<Result<Bytes, ()>> = bytes
            .chunks(8192)
            .map(|c| Ok(Bytes::copy_from_slice(c)))
            .collect();
        let mut reader = Reader::new(
            stream::iter(chunks),
            Duration::from_secs(2),
            Duration::from_secs(20),
        );
        let dir = tempfile::tempdir().unwrap();
        receive(&mut reader, dir.path(), limits)
            .await
            .map_err(|r| r.status())
    }

    #[tokio::test]
    async fn an_upload_over_the_total_or_a_part_cap_is_413_before_its_bytes() {
        let limits = StageLimits {
            total_bytes: 100,
            part_bytes: 60,
            ..StageLimits::default()
        };
        assert_eq!(
            upload(
                &[("t0/part-00000.parquet", 60), ("t0/part-00001.parquet", 40)],
                vec![1; 100],
                limits
            )
            .await,
            Ok(100)
        );
        assert_eq!(
            upload(
                &[("t0/part-00000.parquet", 60), ("t0/part-00001.parquet", 41)],
                vec![1; 100],
                limits
            )
            .await,
            Err(StatusCode::PAYLOAD_TOO_LARGE)
        );
        assert_eq!(
            upload(&[("t0/part-00000.parquet", 61)], vec![1; 100], limits).await,
            Err(StatusCode::PAYLOAD_TOO_LARGE)
        );
        // The real defaults: a gibibyte in all.
        let big = StageLimits::default();
        assert_eq!(big.total_bytes, 1 << 30);
        assert_eq!(
            big.part_bytes * 8,
            big.total_bytes,
            "eight parts at the cap are the whole gibibyte"
        );
    }

    #[tokio::test]
    async fn too_many_files_a_table_index_out_of_range_and_an_oversized_frame_are_refused() {
        let many: Vec<String> = (0..=MAX_FILES_IN)
            .map(|i| format!("t0/part-{i:05}.parquet"))
            .collect();
        let entries: Vec<(&str, u64)> = many.iter().map(|p| (p.as_str(), 0)).collect();
        assert_eq!(
            upload(&entries, vec![], StageLimits::default()).await,
            Err(StatusCode::BAD_REQUEST)
        );
        assert_eq!(
            upload(
                &[("t256/part-00000.parquet", 0)],
                vec![],
                StageLimits::default()
            )
            .await,
            Err(StatusCode::BAD_REQUEST)
        );
        assert_eq!(
            upload(
                &[("t255/part-00000.parquet", 0)],
                vec![],
                StageLimits::default()
            )
            .await,
            Ok(0)
        );
        let chunks: Vec<Result<Bytes, ()>> = vec![Ok(Bytes::from(
            ((HEADER_MAX as u32) + 1).to_be_bytes().to_vec(),
        ))];
        let mut reader = Reader::new(
            stream::iter(chunks),
            Duration::from_secs(1),
            Duration::from_secs(5),
        );
        let dir = tempfile::tempdir().unwrap();
        let err = receive(&mut reader, dir.path(), StageLimits::default())
            .await
            .unwrap_err();
        assert_eq!(err.status(), StatusCode::BAD_REQUEST);
    }

    /// A call asks for its deadline and the server caps it; the output size is
    /// validated by the executor's own bounds.
    #[test]
    fn a_deadline_and_an_output_size_over_the_servers_caps_are_clamped_or_refused() {
        let max = Duration::from_secs(60);
        assert_eq!(call_timeout(5_000, max), Duration::from_secs(5));
        assert_eq!(call_timeout(u64::MAX, max), max);
        for bad in [0, 1025, u64::MAX] {
            assert!(!valid_out_mb(bad), "{bad}");
        }
        assert!(valid_out_mb(1) && valid_out_mb(1024));
    }
}
