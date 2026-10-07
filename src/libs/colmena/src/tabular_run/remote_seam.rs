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
    end_frame, frame, CallHeader, Dropped, FileEntry, Reader, Refusal, ResponseHeader, RunStatus,
    IDLE_TIMEOUT, TRANSFER_MAX, WIRE_V2,
};
use crate::dag_engine::domain::python_executor::{
    PythonRunError, PythonRunRequest, PythonRunResult,
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
    if !send(frame(&header)).await {
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
    if !send(frame(&entry)).await || !send(Bytes::from(manifest)).await {
        return Err(gone());
    }
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
                return Err(RunRefusal::OverBudget(Budget::Data {
                    limit_bytes: limits.total_bytes,
                }));
            }
            let entry = serde_json::to_vec(&FileEntry {
                path: rel,
                size: declared,
            })
            .map_err(|_| gone())?;
            if !send(frame(&entry)).await {
                return Err(gone());
            }
            let mut sent = 0u64;
            loop {
                let next = tokio::time::timeout(IDLE_TIMEOUT, stream.stream.next()).await;
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
            if sent != declared {
                abort(&tx).await;
                return Err(RunRefusal::Invalid(Invalid::Parts));
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
/// them to the sink. Returns the names handed on and what was dropped.
async fn receive<S>(
    reader: &mut Reader<S>,
    head: &ResponseHeader,
    call: &MountedCall<'_>,
) -> Result<Vec<String>, MountedError>
where
    S: futures::Stream<Item = Result<Bytes, reqwest::Error>> + Unpin,
{
    let limits = CollectLimits::default();
    if head.files.len() > limits.max_files {
        return Err(unavailable());
    }
    let mut total = 0u64;
    let mut emitted = vec![];
    for entry in &head.files {
        // A server that sends more than the collector would keep is not followed.
        let named = checked_name(entry.name.as_bytes());
        total = total.saturating_add(entry.size);
        let Some((name, format)) =
            named.filter(|_| entry.size <= limits.file_bytes && total <= limits.total_bytes)
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
        if let Some(sink) = call.sink {
            let out = OutFile::spooled(name.clone(), format, entry.size, std_file);
            sink.accept(out).await.map_err(refused)?;
            emitted.push(name);
        }
    }
    reader.expect_end().await.map_err(|_| unavailable())?;
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

#[async_trait]
impl MountedExecutor for RemoteExecutor {
    async fn run_with_mounts(
        &self,
        req: PythonRunRequest,
        call: MountedCall<'_>,
    ) -> Result<MountedResult, MountedError> {
        let (client, base, max_timeout) = self.transport();
        let timeout = req.timeout.unwrap_or(max_timeout).min(max_timeout);
        let header = serde_json::to_vec(&CallHeader {
            v: WIRE_V2,
            code: req.code,
            mode: req.mode,
            timeout_ms: u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX),
            inputs: req.inputs,
            out_mb: call.out_mb,
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
            let body = resp
                .bytes()
                .await
                .ok()
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
            timeout + TRANSFER_MAX,
        );
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
            assert_eq!(
                made.unwrap_err(),
                RunRefusal::Invalid(Invalid::Parts),
                "{total}/{declared}"
            );
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
        };
        let (_, made, _) = sent(&p, &[0], total).await;
        assert_eq!(
            made.unwrap_err(),
            RunRefusal::OverBudget(Budget::Data {
                limit_bytes: manifest_len + 150
            })
        );
        let (_, made, _) = sent(
            &p,
            &[0],
            StageLimits {
                total_bytes: manifest_len + 200,
                part_bytes: 100,
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
}
