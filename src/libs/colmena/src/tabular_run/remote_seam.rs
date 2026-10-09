//! Mounted runs on the remote executor: the client side of `POST /v2/run`.
//!
//! The request body is built as the parts are read from storage: one part at a
//! time, one chunk in memory, through a channel of two chunks, so the HTTP
//! client's backpressure holds the storage read. Each file declares its size when
//! it starts and the bytes that arrive from storage are counted against it. The
//! response is read the same way and each kept output is spooled to an anonymous
//! temporary file (bounded by the collector's caps) before the sink streams it
//! on: the names and sizes are checked again here, because what a server sends
//! is not trusted more than what a sandbox wrote.
//!
//! Authentication is `/v1/run`'s, through the same `authorized` of
//! [`RemoteExecutor`]. Nothing of the host is on the wire: no storage key, no
//! signed URL; files go by the canonical part path. The streaming strategy (one
//! POST whose body is a stream) is this file and `wire`; if the deployed front
//! end cannot carry it, they are all that changes.
//!
//! There is no automatic retry: the body is a stream from storage, so a retry
//! would stage the data again, and a busy answer already tells the model to retry.

use super::collect::{checked_name, CollectLimits, OutFile, RejectReason, Rejection};
use super::mounted::{MountedCall, MountedError, MountedExecutor, MountedResult};
use super::refusal::{Budget, Invalid, RunRefusal, Unavailable};
use super::stage::Staged;
use super::wire::{
    end_frame, try_frame, CallHeader, Dropped, FileEntry, Reader, Refusal, ResponseHeader,
    RunStatus, IDLE_TIMEOUT, MIN_BYTES_PER_SEC, RATE_GRACE, TRANSFER_MAX, WIRE_V2,
};
use crate::dag_engine::domain::python_executor::{
    PythonExecutor, PythonRunError, PythonRunRequest, PythonRunResult,
};
use crate::dag_engine::infrastructure::python_exec::remote::{endpoint, RemoteExecutor};
use crate::tabular_prepare::manifest::MANIFEST_PATH;
use async_trait::async_trait;
use bytes::Bytes;
use futures::StreamExt;
use std::io::{Seek, SeekFrom};
use tokio::io::AsyncWriteExt;

const REJECTED: &str =
    "PythonExecutorError: the isolated Python executor rejected this caller's credentials";

fn refused(r: RunRefusal) -> MountedError {
    MountedError::Refused(r)
}

fn unavailable() -> MountedError {
    refused(RunRefusal::Unavailable(Unavailable::Executor))
}

/// What the server said about a refusal (its JSON body, if it sent one).
fn refusal_of(status: u16, retry_after: bool, body: Option<Refusal>) -> MountedError {
    let named = body.as_ref().map(|b| b.refusal.as_str());
    match (status, named) {
        (_, Some("busy")) | (503 | 429, None) if retry_after || named == Some("busy") => {
            refused(RunRefusal::OverBudget(Budget::Volumes))
        }
        (_, Some("mounts_disabled")) => {
            refused(RunRefusal::Unavailable(Unavailable::MountsDisabled))
        }
        (501, _) | (_, Some("no_staging_root")) => {
            refused(RunRefusal::Unavailable(Unavailable::NoStagingRoot))
        }
        // No such route: a server without a staging root, or one that predates it.
        (404, _) => refused(RunRefusal::Unavailable(Unavailable::Unsupported)),
        (413, _) | (_, Some("too_large")) => refused(RunRefusal::OverBudget(Budget::Data {
            limit_bytes: super::verify::DATA_MAX_BYTES,
        })),
        (401 | 403, _) => MountedError::Run(PythonRunError::Internal(REJECTED.to_string())),
        _ => unavailable(),
    }
}

/// Ends the body with an error, so the server sees an aborted upload and never
/// a short file it could take for the real one.
async fn abort(tx: &tokio::sync::mpsc::Sender<Result<Bytes, std::io::Error>>) {
    let _ =
        tokio::time::timeout(IDLE_TIMEOUT, tx.send(Err(std::io::Error::other("aborted")))).await;
}

/// Sends the request body: the header, then each file (declared size first),
/// then the end. Returns what was sent, or why it could not be.
async fn produce(
    tx: tokio::sync::mpsc::Sender<Result<Bytes, std::io::Error>>,
    header: Vec<u8>,
    call: &MountedCall<'_>,
) -> Result<Staged, RunRefusal> {
    let limits = call.limits;
    let send = |item: Bytes| {
        let tx = tx.clone();
        async move {
            matches!(
                tokio::time::timeout(IDLE_TIMEOUT, tx.send(Ok(item))).await,
                Ok(Ok(()))
            )
        }
    };
    let gone = || RunRefusal::Unavailable(Unavailable::Executor);
    let head_frame = try_frame(&header).map_err(|_| gone())?;
    if !send(head_frame).await {
        return Err(gone());
    }
    let manifest = call
        .plan
        .manifest()
        .to_json()
        .map_err(|_| RunRefusal::Invalid(Invalid::Manifest))?;
    let mut total = manifest.len() as u64;
    let entry = FileEntry {
        path: MANIFEST_PATH.into(),
        size: total,
    };
    let entry = serde_json::to_vec(&entry).map_err(|_| gone())?;
    if !send(try_frame(&entry).map_err(|_| gone())?).await || !send(Bytes::from(manifest)).await {
        return Err(gone());
    }
    let deadline = tokio::time::Instant::now() + limits.total_time;
    let mut parts = 0usize;
    for &t in call.tables {
        let table = call
            .plan
            .tables()
            .get(t)
            .ok_or(RunRefusal::Invalid(Invalid::Manifest))?;
        for p in 0..table.parts as usize {
            let rel = crate::tabular_prepare::manifest::part_path(t, p)
                .map_err(|_| RunRefusal::Invalid(Invalid::Manifest))?;
            let key = call.plan.part_key(t, p)?;
            let mut stream = call
                .storage
                .read_stream(&key)
                .await
                .map_err(|_| RunRefusal::Storage)?;
            let declared = stream.size_bytes;
            if declared > limits.part_bytes {
                abort(&tx).await;
                return Err(RunRefusal::OverBudget(Budget::Part {
                    limit_bytes: limits.part_bytes,
                }));
            }
            if total.saturating_add(declared) > limits.total_bytes {
                abort(&tx).await;
                return Err(RunRefusal::OverBudget(match call.tables.len() <= 1 {
                    true => Budget::Table {
                        limit_bytes: limits.total_bytes,
                    },
                    false => Budget::Data {
                        limit_bytes: limits.total_bytes,
                    },
                }));
            }
            let entry = serde_json::to_vec(&FileEntry {
                path: rel,
                size: declared,
            })
            .map_err(|_| gone())?;
            if !send(try_frame(&entry).map_err(|_| gone())?).await {
                return Err(gone());
            }
            let mut sent = 0u64;
            loop {
                let wait = limits
                    .idle
                    .min(deadline.saturating_duration_since(tokio::time::Instant::now()));
                let next = tokio::time::timeout(wait, stream.stream.next()).await;
                let Ok(next) = next else {
                    abort(&tx).await;
                    return Err(RunRefusal::Storage);
                };
                let Some(chunk) = next else { break };
                let Ok(chunk) = chunk else {
                    abort(&tx).await;
                    return Err(RunRefusal::Storage);
                };
                sent += chunk.len() as u64;
                // The declared size is not trusted: stop at the first byte over it.
                if sent > declared {
                    abort(&tx).await;
                    return Err(RunRefusal::Invalid(Invalid::Parts));
                }
                if !send(chunk).await {
                    return Err(gone());
                }
            }
            // Ended early but cleanly: the storage cut the transfer (retryable).
            if sent != declared {
                abort(&tx).await;
                return Err(RunRefusal::Storage);
            }
            total += declared;
            parts += 1;
        }
    }
    if !send(end_frame()).await {
        return Err(gone());
    }
    Ok(Staged {
        tables: call.tables.to_vec(),
        parts,
        bytes: total,
    })
}

/// Reads the kept outputs of a 200 response, each spooled and checked, and hands
/// them to the sink ONLY after the response has ended correctly: a response that
/// repeats a name (in any case), runs over a cap or carries extra bytes gives the
/// sink nothing. Returns the names handed on.
async fn receive<S, E>(
    reader: &mut Reader<S>,
    head: &ResponseHeader,
    call: &MountedCall<'_>,
) -> Result<Vec<String>, MountedError>
where
    S: futures::Stream<Item = Result<Bytes, E>> + Unpin,
{
    let limits = CollectLimits::default();
    if head.files.len() > limits.max_files {
        return Err(unavailable());
    }
    let mut total = 0u64;
    let mut names = std::collections::HashSet::new();
    let mut spooled: Vec<OutFile> = vec![];
    for entry in &head.files {
        // A server that sends more than the collector would keep is not followed.
        let named = checked_name(entry.name.as_bytes());
        total = total.saturating_add(entry.size);
        let Some((name, format)) = named
            .filter(|_| entry.size <= limits.file_bytes && total <= limits.total_bytes)
            .filter(|(name, _)| names.insert(name.to_lowercase()))
        else {
            return Err(unavailable());
        };
        let std_file = spool().map_err(|_| unavailable())?;
        let mut file = tokio::fs::File::from_std(std_file.try_clone().map_err(|_| unavailable())?);
        reader
            .copy_exact(entry.size, &mut file)
            .await
            .map_err(|_| unavailable())?;
        file.flush().await.map_err(|_| unavailable())?;
        let mut std_file = std_file;
        std_file
            .seek(SeekFrom::Start(0))
            .map_err(|_| unavailable())?;
        spooled.push(OutFile::spooled(name, format, entry.size, std_file));
    }
    reader.expect_end().await.map_err(|_| unavailable())?;
    let mut emitted = vec![];
    if let Some(sink) = call.sink {
        for out in spooled {
            let name = out.name.clone();
            sink.accept(out).await.map_err(refused)?;
            emitted.push(name);
        }
    }
    Ok(emitted)
}

/// An anonymous file for one output: created new and private, then unlinked at
/// once, so nothing names it and it vanishes with its last descriptor.
fn spool() -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    let path =
        std::env::temp_dir().join(format!("colmena-spool-{}", uuid::Uuid::new_v4().simple()));
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&path)?;
    std::fs::remove_file(&path)?;
    Ok(file)
}

fn dropped(list: &[Dropped]) -> Vec<Rejection> {
    list.iter()
        .map(|d| Rejection {
            // A name is shown only if it passes the same check a local one does.
            name: d
                .name
                .as_deref()
                .and_then(|n| checked_name(n.as_bytes()))
                .map(|(n, _)| n),
            reason: RejectReason::from_wire(&d.reason),
        })
        .collect()
}

/// The most a small answer body (a refusal) is read: a server that sends more is
/// not followed, and it is read within the idle limit.
const SMALL_BODY_MAX: usize = 16 * 1024;

/// A small response body, capped in size and in time; `None` when it is longer
/// than the cap, stalls or fails (the status alone then decides).
async fn small_body(mut resp: reqwest::Response) -> Option<Vec<u8>> {
    let read = async {
        let mut out = Vec::new();
        while let Some(chunk) = resp.chunk().await.ok()? {
            if out.len() + chunk.len() > SMALL_BODY_MAX {
                return None;
            }
            out.extend_from_slice(&chunk);
        }
        Some(out)
    };
    tokio::time::timeout(IDLE_TIMEOUT, read)
        .await
        .ok()
        .flatten()
}

/// What the whole call may take besides its code: the probe, the upload, the
/// download and the waits between. The call's own clock (the tool's) is shorter or
/// equal; this one guarantees the future ends even if nothing else does.
fn call_limit(run: std::time::Duration) -> std::time::Duration {
    IDLE_TIMEOUT + TRANSFER_MAX + run + IDLE_TIMEOUT + TRANSFER_MAX + IDLE_TIMEOUT
}

#[async_trait]
impl MountedExecutor for RemoteExecutor {
    async fn run_with_mounts(
        &self,
        req: PythonRunRequest,
        call: MountedCall<'_>,
    ) -> Result<MountedResult, MountedError> {
        let run = req
            .timeout
            .unwrap_or(self.transport().2)
            .min(self.transport().2);
        tokio::time::timeout(call_limit(run), self.run_inner(req, call))
            .await
            .unwrap_or_else(|_| Err(unavailable()))
    }
}

impl RemoteExecutor {
    async fn run_inner(
        &self,
        req: PythonRunRequest,
        call: MountedCall<'_>,
    ) -> Result<MountedResult, MountedError> {
        // The header must fit a frame: checked before anything else, so a call too
        // large to send is the model's to shorten, not an executor that is "unavailable".
        let sized = CallHeader {
            v: WIRE_V2,
            code: req.code.clone(),
            mode: req.mode.clone(),
            timeout_ms: 0,
            inputs: req.inputs.clone(),
            out_mb: call.out_mb,
            probe: false,
        };
        if serde_json::to_vec(&sized).map_or(true, |j| try_frame(&j).is_err()) {
            return Err(MountedError::Run(PythonRunError::Python(
                "the code and its inputs are too large for a large-file call (over 1 MiB together); shorten the code".to_string(),
            )));
        }
        // Credentials and readiness are checked BEFORE any part is read from
        // storage: a server that refuses the caller answers while the body
        // is still unsent, and a refused upload can look like a dropped connection.
        self.warm()
            .await
            .map_err(|text| MountedError::Run(PythonRunError::Internal(text)))?;
        let (client, base, max_timeout) = self.transport();
        // A probe: the same header with `probe` set, answered before any data is
        // read. What the server would refuse (no route, no staging root, mounts
        // off, no free volume) is known BEFORE storage is read or a byte
        // uploaded; a refused upload can look like a dropped connection, which
        // says nothing. 204 is the one answer that lets the call go on. A volume
        // taken in between is still refused, then as "unavailable, retry".
        let timeout = req.timeout.unwrap_or(max_timeout).min(max_timeout);
        let head = CallHeader {
            v: WIRE_V2,
            code: String::new(),
            mode: req.mode.clone(),
            timeout_ms: 1000,
            inputs: serde_json::Map::new(),
            out_mb: call.out_mb,
            probe: true,
        };
        let mut body = try_frame(&serde_json::to_vec(&head).map_err(|_| unavailable())?)
            .map_err(|_| unavailable())?
            .to_vec();
        body.extend_from_slice(&end_frame());
        let probe = client
            .post(endpoint(base, "v2/run"))
            .body(body)
            .timeout(IDLE_TIMEOUT);
        let probe = self.authorized(probe).await.map_err(MountedError::Run)?;
        let probed = probe.send().await.map_err(|_| unavailable())?;
        if probed.status().as_u16() != 204 {
            let status = probed.status().as_u16();
            let retry = probed.headers().contains_key(reqwest::header::RETRY_AFTER);
            let body = small_body(probed)
                .await
                .and_then(|b| serde_json::from_slice::<Refusal>(&b).ok());
            return Err(refusal_of(status, retry, body));
        }
        let header = serde_json::to_vec(&CallHeader {
            v: WIRE_V2,
            code: req.code,
            mode: req.mode,
            timeout_ms: u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX),
            inputs: req.inputs,
            out_mb: call.out_mb,
            probe: false,
        })
        .map_err(|_| unavailable())?;
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(2);
        let body = reqwest::Body::wrap_stream(tokio_stream::wrappers::ReceiverStream::new(rx));
        let rb = client
            .post(endpoint(base, "v2/run"))
            .header("x-colmena-request-id", uuid::Uuid::new_v4().to_string())
            .body(body);
        let rb = self.authorized(rb).await.map_err(MountedError::Run)?;
        // Upload, then the run, then the download: each bounded, and the whole.
        let whole = TRANSFER_MAX + timeout + IDLE_TIMEOUT + TRANSFER_MAX;
        let request = tokio::time::timeout(whole, rb.send());
        let producing = produce(tx, header, &call);
        tokio::pin!(request, producing);
        let (mut staged, mut failure) = (None, None);
        let resp = loop {
            tokio::select! {
                sent = &mut request => break sent,
                made = &mut producing, if staged.is_none() && failure.is_none() => match made {
                    Ok(s) => staged = Some(s),
                    Err(e) => failure = Some(e),
                },
            }
        };
        if let Some(e) = failure {
            return Err(refused(e));
        }
        let resp = match resp {
            Ok(Ok(r)) => r,
            _ => return Err(unavailable()),
        };
        let status = resp.status().as_u16();
        if status != 200 {
            let retry = resp.headers().contains_key(reqwest::header::RETRY_AFTER);
            let body = small_body(resp)
                .await
                .and_then(|b| serde_json::from_slice::<Refusal>(&b).ok());
            return Err(refusal_of(status, retry, body));
        }
        // The upload may still be finishing when the answer starts: it is not
        // an answer to a call whose data did not all arrive.
        let staged = match staged {
            Some(s) => s,
            None => producing.await.map_err(refused)?,
        };
        let mut reader = Reader::new(
            resp.bytes_stream(),
            timeout + IDLE_TIMEOUT,
            timeout + super::runtime::COLLECT_BUDGET,
        )
        .with_min_rate(MIN_BYTES_PER_SEC, RATE_GRACE + timeout);
        let head: ResponseHeader = reader.json().await.map_err(|_| unavailable())?;
        reader.set_idle(IDLE_TIMEOUT);
        if head.v != WIRE_V2 {
            return Err(unavailable());
        }
        let rejected = dropped(&head.dropped);
        let too_many_entries = head.too_many_entries;
        match head.status {
            RunStatus::Ok => {
                let emitted = receive(&mut reader, &head, &call).await?;
                Ok(MountedResult {
                    result: PythonRunResult {
                        output: head.output,
                        stdout: head.stdout,
                    },
                    staged,
                    emitted,
                    rejected,
                    too_many_entries,
                })
            }
            RunStatus::PythonError => Err(MountedError::Run(PythonRunError::Python(
                head.message.unwrap_or_default(),
            ))),
            RunStatus::Timeout => Err(MountedError::Run(PythonRunError::Timeout)),
            RunStatus::Internal => Err(MountedError::Run(PythonRunError::Internal(
                "PythonExecutorError: the isolated Python executor failed".to_string(),
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::stage::StageLimits;
    use super::super::testkit::*;
    use super::super::verify::verify_prepared;
    use super::*;
    use std::sync::atomic::Ordering::SeqCst;
    use std::sync::Arc;
    use std::time::Duration;

    /// Runs `produce` to the end against an unbounded reader of the channel and
    /// returns the bytes it sent and how it ended.
    async fn sent(
        p: &Prepared,
        tables: &[usize],
        limits: StageLimits,
    ) -> (Vec<u8>, Result<Staged, RunRefusal>, bool) {
        let plan = verify_prepared(&*p.registry, &*p.storage, SOURCE)
            .await
            .unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(2);
        let call = MountedCall {
            storage: &*p.storage,
            plan: &plan,
            tables,
            limits,
            out_mb: 4,
            sink: None,
        };
        let collect = async {
            let (mut all, mut errored) = (vec![], false);
            while let Some(item) = rx.recv().await {
                match item {
                    Ok(b) => all.extend_from_slice(&b),
                    Err(_) => errored = true,
                }
            }
            (all, errored)
        };
        let (made, (all, errored)) = tokio::join!(produce(tx, b"{}".to_vec(), &call), collect);
        (all, made, errored)
    }

    /// Only canonical paths go out, never a key; the declared sizes match what
    /// follows; the body ends with the empty frame.
    #[tokio::test]
    async fn the_request_body_names_files_by_canonical_path_and_never_a_key() {
        let p = prepared(&[("sales", 2)], 6).await;
        let (all, made, errored) = sent(&p, &[0], StageLimits::default()).await;
        assert!(!errored);
        let staged = made.unwrap();
        assert_eq!((staged.parts, staged.tables.clone()), (2, vec![0]));
        for needle in [SOURCE, ROOT, "chat-attachments", "gs://", "http"] {
            assert!(
                !String::from_utf8_lossy(&all).contains(needle),
                "{needle} on the wire"
            );
        }
        let chunks: Vec<Result<Bytes, ()>> = vec![Ok(Bytes::from(all))];
        let mut r = Reader::new(
            futures::stream::iter(chunks),
            Duration::from_secs(1),
            Duration::from_secs(5),
        );
        let _: serde_json::Value = r.json().await.unwrap();
        let mut seen = vec![];
        while let Some(raw) = r.frame().await.unwrap() {
            let e: FileEntry = serde_json::from_slice(&raw).unwrap();
            let mut body = vec![];
            r.copy_exact(e.size, &mut body).await.unwrap();
            seen.push((e.path, body.len()));
        }
        r.expect_end().await.unwrap();
        assert_eq!(
            seen[1..],
            [
                ("t0/part-00000.parquet".to_string(), 6),
                ("t0/part-00001.parquet".to_string(), 6)
            ]
        );
        assert_eq!(seen[0].0, "manifest.json");
    }

    /// A storage that sends other than it declared aborts the body: the server
    /// would see an error, never a short file it takes for the real one.
    #[tokio::test]
    async fn a_storage_that_lies_about_a_size_aborts_the_body() {
        for (total, declared) in [(10u64, 20u64), (20, 10)] {
            let p = prepared(&[("sales", 1)], 4).await;
            let plan = verify_prepared(&*p.registry, &*p.storage, SOURCE)
                .await
                .unwrap();
            let live = Arc::new(Live::default());
            let l = live.clone();
            p.storage.serve(&plan.part_key(0, 0).unwrap(), move || {
                generated(total, 5, declared, l.clone(), None)
            });
            let (_, made, errored) = sent(&p, &[0], StageLimits::default()).await;
            let expected = match total < declared {
                true => RunRefusal::Storage,
                false => RunRefusal::Invalid(Invalid::Parts),
            };
            assert_eq!(made.unwrap_err(), expected, "{total}/{declared}");
            assert!(errored, "the body was aborted");
            assert!(live.produced.load(SeqCst) as u64 <= declared.max(total));
        }
    }

    #[tokio::test]
    async fn a_part_or_call_over_its_limit_is_refused_before_its_bytes_are_read() {
        let p = prepared(&[("sales", 2)], 100).await;
        let manifest_len = p.manifest.to_json().unwrap().len() as u64;
        let part = StageLimits {
            total_bytes: 1 << 30,
            part_bytes: 99,
            ..StageLimits::default()
        };
        let (_, made, errored) = sent(&p, &[0], part).await;
        assert_eq!(
            made.unwrap_err(),
            RunRefusal::OverBudget(Budget::Part { limit_bytes: 99 })
        );
        assert!(errored);
        let total = StageLimits {
            total_bytes: manifest_len + 150,
            part_bytes: 100,
            ..StageLimits::default()
        };
        let (_, made, _) = sent(&p, &[0], total).await;
        assert_eq!(
            made.unwrap_err(),
            RunRefusal::OverBudget(Budget::Table {
                limit_bytes: manifest_len + 150
            })
        );
        let (_, made, _) = sent(
            &p,
            &[0],
            StageLimits {
                total_bytes: manifest_len + 200,
                part_bytes: 100,
                ..StageLimits::default()
            },
        )
        .await;
        assert!(made.is_ok(), "exactly at the limit");
    }

    #[tokio::test]
    async fn a_storage_failure_while_sending_hides_the_adapters_text() {
        let p = prepared(&[("sales", 1)], 4).await;
        let plan = verify_prepared(&*p.registry, &*p.storage, SOURCE)
            .await
            .unwrap();
        p.storage
            .broken
            .lock()
            .unwrap()
            .push(plan.part_key(0, 0).unwrap());
        let (_, made, errored) = sent(&p, &[0], StageLimits::default()).await;
        let err = made.unwrap_err();
        assert_eq!(err, RunRefusal::Storage);
        assert!(!err.message().contains("secret"));
        let _ = errored;
    }

    #[test]
    fn the_servers_refusals_become_the_existing_typed_refusals() {
        let body = |r: &str| {
            Some(Refusal {
                refusal: r.into(),
                reason: None,
            })
        };
        let refusal = |e: MountedError| match e {
            MountedError::Refused(r) => r,
            other => panic!("{other:?}"),
        };
        assert_eq!(
            refusal(refusal_of(503, true, body("busy"))),
            RunRefusal::OverBudget(Budget::Volumes)
        );
        assert_eq!(
            refusal(refusal_of(503, true, None)),
            RunRefusal::OverBudget(Budget::Volumes)
        );
        assert_eq!(
            refusal(refusal_of(503, false, body("mounts_disabled"))),
            RunRefusal::Unavailable(Unavailable::MountsDisabled)
        );
        assert_eq!(
            refusal(refusal_of(501, false, body("no_staging_root"))),
            RunRefusal::Unavailable(Unavailable::NoStagingRoot)
        );
        assert_eq!(
            refusal(refusal_of(404, false, None)),
            RunRefusal::Unavailable(Unavailable::Unsupported)
        );
        assert!(matches!(
            refusal(refusal_of(413, false, body("too_large"))),
            RunRefusal::OverBudget(Budget::Data { .. })
        ));
        assert_eq!(
            refusal(refusal_of(502, false, None)),
            RunRefusal::Unavailable(Unavailable::Executor)
        );
        assert_eq!(
            refusal(refusal_of(503, false, None)),
            RunRefusal::Unavailable(Unavailable::Executor)
        );
        assert!(
            matches!(refusal_of(401, false, None), MountedError::Run(PythonRunError::Internal(t)) if t.contains("rejected"))
        );
    }

    #[test]
    fn a_dropped_name_is_shown_only_if_it_passes_the_local_charset() {
        let list = vec![
            Dropped {
                name: Some("big.csv".into()),
                reason: "TooLarge".into(),
            },
            Dropped {
                name: Some("bad name.csv".into()),
                reason: "TooLarge".into(),
            },
            Dropped {
                name: None,
                reason: "Weird".into(),
            },
        ];
        let got = dropped(&list);
        assert_eq!(
            got[0],
            Rejection {
                name: Some("big.csv".into()),
                reason: RejectReason::TooLarge
            }
        );
        assert_eq!(got[1].name, None);
        assert_eq!(got[2].reason, RejectReason::Unreadable);
    }

    /// Records what reaches it.
    #[derive(Default)]
    struct Recording(std::sync::Mutex<Vec<String>>);
    #[async_trait]
    impl super::super::mounted::OutputSink for Recording {
        async fn accept(&self, file: OutFile) -> Result<(), RunRefusal> {
            self.0.lock().unwrap().push(file.name);
            Ok(())
        }
    }

    /// A server's response of `files` (name, declared size) with `body` after the
    /// header, run through `receive`; returns its answer and what reached the sink.
    async fn hostile(
        files: &[(&str, u64)],
        body: &[u8],
    ) -> (Result<Vec<String>, MountedError>, Vec<String>) {
        let p = prepared(&[("sales", 1)], 4).await;
        let plan = verify_prepared(&*p.registry, &*p.storage, SOURCE)
            .await
            .unwrap();
        let sink = Recording::default();
        let call = MountedCall {
            storage: &*p.storage,
            plan: &plan,
            tables: &[0],
            limits: StageLimits::default(),
            out_mb: 4,
            sink: Some(&sink),
        };
        let head = ResponseHeader {
            v: WIRE_V2,
            status: RunStatus::Ok,
            message: None,
            output: None,
            stdout: String::new(),
            files: files
                .iter()
                .map(|(n, s)| super::super::wire::OutEntry {
                    name: n.to_string(),
                    size: *s,
                })
                .collect(),
            dropped: vec![],
            too_many_entries: false,
        };
        let chunks: Vec<Result<Bytes, ()>> = vec![Ok(Bytes::copy_from_slice(body))];
        let mut reader = Reader::new(
            futures::stream::iter(chunks),
            Duration::from_millis(300),
            Duration::from_secs(5),
        );
        let got = receive(&mut reader, &head, &call).await;
        let seen = sink.0.lock().unwrap().clone();
        (got, seen)
    }

    /// A hostile or broken server cannot get anything into the sink: a repeated
    /// name (any case), an odd name, a size over the caps, too many files, bytes
    /// after the end, a body that ends early. A good response gives all of it,
    /// and only after it ended.
    #[tokio::test]
    async fn a_hostile_response_gives_the_sink_nothing() {
        let unavailable = || Err(super::unavailable());
        for (label, files, body) in [
            (
                "duplicate",
                vec![("a.csv", 1u64), ("a.csv", 1)],
                b"xx".to_vec(),
            ),
            (
                "duplicate in another case",
                vec![("a.csv", 1), ("A.csv", 1)],
                b"xx".to_vec(),
            ),
            ("path name", vec![("../x.csv", 1)], b"x".to_vec()),
            ("odd extension", vec![("x.sh", 1)], b"x".to_vec()),
            (
                "over the file cap",
                vec![("a.csv", 64 * 1024 * 1024 + 1)],
                vec![],
            ),
            (
                "over the total",
                vec![
                    ("a.csv", 60 << 20),
                    ("b.csv", 60 << 20),
                    ("c.csv", 60 << 20),
                ],
                vec![],
            ),
            (
                "extra bytes after the end",
                vec![("a.csv", 1)],
                b"xEXTRA".to_vec(),
            ),
            ("ends early", vec![("a.csv", 10)], b"short".to_vec()),
        ] {
            let (got, seen) = hostile(&files, &body).await;
            assert_eq!(
                got.map_err(|e| format!("{e:?}")),
                unavailable().map_err(|e: MountedError| format!("{e:?}")),
                "{label}"
            );
            assert!(seen.is_empty(), "{label}: the sink saw {seen:?}");
        }
        let many: Vec<(String, u64)> = (0..9).map(|i| (format!("f{i}.csv"), 1)).collect();
        let many: Vec<(&str, u64)> = many.iter().map(|(n, s)| (n.as_str(), *s)).collect();
        let (got, seen) = hostile(&many, &[b'x'; 9]).await;
        assert!(got.is_err() && seen.is_empty(), "too many files");
        let (got, seen) = hostile(&[("a.csv", 1), ("b.parquet", 2)], b"xyz").await;
        assert_eq!(got.unwrap(), ["a.csv", "b.parquet"]);
        assert_eq!(seen, ["a.csv", "b.parquet"]);
    }

    // ---- a hostile or broken SERVER, against the real client, without privileges ----

    use crate::dag_engine::infrastructure::python_exec::config::{RemoteAuthConfig, RemoteConfig};
    use axum::body::Body;
    use axum::http::StatusCode;
    use axum::response::IntoResponse;
    use axum::routing::{get, post};
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A server that is ready and takes credentials, and answers `/v2/run` as
    /// `on_run` says (given the number of the request, from 0). Returns the client
    /// and how many `/v2/run` requests it saw.
    async fn server_that(
        on_run: impl Fn(usize) -> axum::response::Response + Clone + Send + Sync + 'static,
    ) -> (RemoteExecutor, Arc<AtomicUsize>) {
        let seen = Arc::new(AtomicUsize::new(0));
        let counter = seen.clone();
        let app = axum::Router::new()
            .route("/readyz", get(|| async { StatusCode::OK }))
            .route("/v1/run", post(|| async { StatusCode::BAD_REQUEST }))
            .route(
                "/v2/run",
                post(move || {
                    let n = counter.fetch_add(1, Ordering::SeqCst);
                    let on_run = on_run.clone();
                    async move { on_run(n) }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await });
        let cfg = RemoteConfig {
            url: url.parse().unwrap(),
            auth: RemoteAuthConfig::None,
            max_request_bytes: 32 << 20,
            max_response_bytes: 1 << 20,
            max_wire_bytes: None,
        };
        (
            RemoteExecutor::new(cfg, Duration::from_secs(60)).unwrap(),
            seen,
        )
    }

    fn endless(status: StatusCode) -> axum::response::Response {
        let body = Body::from_stream(futures::stream::unfold((), |_| async {
            Some((
                Ok::<_, std::io::Error>(Bytes::from(vec![b'x'; 64 * 1024])),
                (),
            ))
        }));
        (status, body).into_response()
    }

    async fn call_against(
        client: &RemoteExecutor,
        code: &str,
    ) -> (Result<MountedResult, MountedError>, std::time::Duration) {
        let p = prepared(&[("sales", 1)], 4).await;
        let plan = verify_prepared(&*p.registry, &*p.storage, SOURCE)
            .await
            .unwrap();
        let call = MountedCall {
            storage: &*p.storage,
            plan: &plan,
            tables: &[0],
            limits: StageLimits::default(),
            out_mb: 4,
            sink: None,
        };
        let req = PythonRunRequest {
            code: code.into(),
            mode: "restricted".into(),
            timeout: Some(Duration::from_secs(5)),
            inputs: Default::default(),
        };
        let started = std::time::Instant::now();
        (client.run_with_mounts(req, call).await, started.elapsed())
    }

    fn is_unavailable(r: &Result<MountedResult, MountedError>) -> bool {
        matches!(
            r,
            Err(MountedError::Refused(RunRefusal::Unavailable(
                Unavailable::Executor
            )))
        )
    }

    /// A refusal with a body that never ends is read up to its cap and no more:
    /// the call ends at once, as "unavailable", whichever answer carried it.
    #[tokio::test]
    async fn an_endless_refusal_body_is_cut_at_its_cap_on_the_probe_and_on_the_call() {
        let (client, seen) = server_that(|_| endless(StatusCode::SERVICE_UNAVAILABLE)).await;
        let (got, took) = call_against(&client, "output = 1").await;
        assert!(is_unavailable(&got), "{got:?}");
        assert!(took < Duration::from_secs(10), "{took:?}");
        assert_eq!(seen.load(Ordering::SeqCst), 1, "only the probe was sent");
        // The probe passes (204) and the call itself is answered with an endless 503.
        let (client, _) = server_that(|n| match n {
            0 => StatusCode::NO_CONTENT.into_response(),
            _ => endless(StatusCode::SERVICE_UNAVAILABLE),
        })
        .await;
        let (got, took) = call_against(&client, "output = 1").await;
        assert!(is_unavailable(&got), "{got:?}");
        assert!(took < Duration::from_secs(10), "{took:?}");
    }

    /// A redirect is not followed: it is an answer that is not 204.
    #[tokio::test]
    async fn a_redirect_is_not_followed() {
        let (client, seen) = server_that(|_| {
            (
                StatusCode::FOUND,
                [("location", "http://127.0.0.1:1/elsewhere")],
            )
                .into_response()
        })
        .await;
        let (got, _) = call_against(&client, "output = 1").await;
        assert!(is_unavailable(&got), "{got:?}");
        assert_eq!(seen.load(Ordering::SeqCst), 1);
    }

    /// A response header frame that claims more than a frame may hold is refused
    /// without reading it.
    #[tokio::test]
    async fn an_oversized_response_header_frame_is_refused() {
        let (client, _) = server_that(|n| match n {
            0 => StatusCode::NO_CONTENT.into_response(),
            _ => {
                let mut body = ((super::super::wire::HEADER_MAX as u32) + 1)
                    .to_be_bytes()
                    .to_vec();
                body.extend(vec![b' '; 4096]);
                (StatusCode::OK, body).into_response()
            }
        })
        .await;
        let (got, took) = call_against(&client, "output = 1").await;
        assert!(is_unavailable(&got), "{got:?}");
        assert!(took < Duration::from_secs(10));
    }

    /// Code too large for a frame is the model's to shorten: a Python-kind error,
    /// before any request is made, and never "unavailable".
    #[tokio::test]
    async fn code_too_large_for_a_frame_is_a_typed_error_before_any_request() {
        let (client, seen) = server_that(|_| StatusCode::NO_CONTENT.into_response()).await;
        let (got, _) =
            call_against(&client, &"#".repeat(super::super::wire::HEADER_MAX + 10)).await;
        match got {
            Err(MountedError::Run(PythonRunError::Python(text))) => {
                assert!(text.contains("too large"), "{text}")
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(seen.load(Ordering::SeqCst), 0, "nothing was sent");
    }
}
