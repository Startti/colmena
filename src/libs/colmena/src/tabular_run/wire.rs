//! The `/v2/run` wire: one request and one response, each a stream, on any HTTP
//! version (the framing is ours, so it works over HTTP/1.1 chunked transfer as
//! well as HTTP/2). Nothing here knows which transport carries it.
//!
//! Request body:  `frame(CallHeader)`, then for each file `frame(FileEntry)`
//! followed by exactly `size` raw bytes, then an empty frame (`u32` 0). Nothing
//! may follow it.
//! Response body: `frame(ResponseHeader)` followed by the raw bytes of each file
//! the header lists, in order, exactly `size` bytes each.
//!
//! A frame is a big-endian `u32` length and that many bytes of JSON. Sizes are
//! declared per file when the file starts, so a sender never has to know the
//! sizes of everything before it sends the first byte, and a receiver checks
//! each file's bytes against what was declared AND the running total against
//! its cap. No key, URL or path of the host is ever on the wire: files are named
//! by the canonical part path (`t<n>/part-NNNNN.parquet`, `manifest.json`).

use bytes::{Bytes, BytesMut};
use futures::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::time::Duration;
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio::time::Instant;

pub const WIRE_V2: u32 = 2;
/// Largest JSON frame (1 MiB): room for the code, the inputs the prelude is given
/// (a wide table's columns) and a response header with its capped stdout and result.
pub const HEADER_MAX: usize = 1024 * 1024;

/// Most of a call's stdout the response header carries, and of its result's JSON.
pub const STDOUT_WIRE_MAX: usize = 64 * 1024;
pub const OUTPUT_WIRE_MAX: usize = 512 * 1024;

/// Slowest an upload or a download may go once its grace has passed: a peer that
/// trickles bytes under the idle limit holds a volume for the whole transfer limit.
pub const MIN_BYTES_PER_SEC: u64 = 256 * 1024;
pub const RATE_GRACE: Duration = Duration::from_secs(15);
/// Most files one call may send (the parts of the chosen tables and the manifest).
pub const MAX_FILES_IN: usize = 20_000;
/// The first frame must arrive within this.
pub const HEADER_TIMEOUT: Duration = Duration::from_secs(10);
/// No chunk for this long ends the transfer: a stalled peer holds nothing.
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(30);
/// The whole upload, or the whole download, may take at most this long.
pub const TRANSFER_MAX: Duration = Duration::from_secs(240);

/// The first frame of a request: the code and the call's limits. The same
/// fields as the `/v1/run` wire request, plus the size of the output volume.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallHeader {
    pub v: u32,
    pub code: String,
    pub mode: String,
    pub timeout_ms: u64,
    pub inputs: Map<String, Value>,
    pub out_mb: u64,
    /// Ask only whether a call like this would be taken now (the route, the
    /// credentials, mounts, a free volume): the answer is 204 and nothing runs.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub probe: bool,
}

/// One file of the request: its relative path and the bytes that follow.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntry {
    pub path: String,
    pub size: u64,
}

/// How the code's run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Ok,
    PythonError,
    Timeout,
    Internal,
}

/// One output the server kept: its name and the bytes that follow.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutEntry {
    pub name: String,
    pub size: u64,
}

/// An output the server did not keep: why, and its name only when it passed the
/// charset check (so it is safe to show).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dropped {
    pub name: Option<String>,
    pub reason: String,
}

/// The first frame of a response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseHeader {
    pub v: u32,
    pub status: RunStatus,
    /// The code's error text, or the executor's, when the status is not `ok`.
    pub message: Option<String>,
    pub output: Option<Value>,
    pub stdout: String,
    pub files: Vec<OutEntry>,
    pub dropped: Vec<Dropped>,
    pub too_many_entries: bool,
}

/// Why a call was refused before any code ran, as the body of a non-200 answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Refusal {
    /// `busy`, `mounts_disabled`, `no_staging_root`, `too_large`, `bad_request`.
    pub refusal: String,
    /// For `mounts_disabled`: the template's reason (letters, digits, `_`).
    pub reason: Option<String>,
}

/// A frame: its length, then `json`; `Malformed` when `json` is longer than a
/// frame may be, so a sender learns it BEFORE it sends anything.
pub fn try_frame(json: &[u8]) -> Result<Bytes, WireError> {
    let len = u32::try_from(json.len())
        .ok()
        .filter(|_| json.len() <= HEADER_MAX)
        .ok_or(WireError::Malformed)?;
    let mut out = BytesMut::with_capacity(4 + json.len());
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(json);
    Ok(out.freeze())
}

/// [`try_frame`] for a frame built to be small (the tests, literals). Production
/// paths use [`try_frame`] and handle the error.
pub fn frame(json: &[u8]) -> Bytes {
    try_frame(json).expect("a frame within HEADER_MAX")
}

/// The empty frame that ends a request.
pub fn end_frame() -> Bytes {
    Bytes::from_static(&[0, 0, 0, 0])
}

/// Why reading a stream failed. None of them carries peer text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WireError {
    /// No chunk within [`IDLE_TIMEOUT`] (or the limit given).
    Stalled,
    /// The transfer took longer than its deadline.
    Deadline,
    /// The stream ended before the declared bytes.
    Early,
    /// A frame that is too long, not JSON, or bytes after the end.
    Malformed,
    /// The transport failed.
    Transport,
}

/// Reads frames and exact byte counts from a body stream, holding at most one
/// chunk. `stream` is whatever the transport gives (an axum body, a reqwest
/// response); a chunk is read only when the previous one has been consumed, so
/// the transport's own backpressure holds the sender.
pub struct Reader<S> {
    stream: S,
    buf: Bytes,
    idle: Duration,
    deadline: Instant,
    /// `(bytes per second, grace)`: past the grace, slower than this is a stall.
    min_rate: Option<(u64, Duration)>,
    started: Instant,
    seen: u64,
}

impl<S, E> Reader<S>
where
    S: Stream<Item = Result<Bytes, E>> + Unpin,
{
    pub fn new(stream: S, idle: Duration, total: Duration) -> Self {
        Self {
            stream,
            buf: Bytes::new(),
            idle,
            deadline: Instant::now() + total,
            min_rate: None,
            started: Instant::now(),
            seen: 0,
        }
    }

    /// Requires a minimum average rate once `grace` has passed.
    pub fn with_min_rate(mut self, bytes_per_sec: u64, grace: Duration) -> Self {
        self.min_rate = Some((bytes_per_sec, grace));
        self
    }

    fn too_slow(&self) -> bool {
        let Some((bps, grace)) = self.min_rate else {
            return false;
        };
        let elapsed = self.started.elapsed();
        elapsed > grace && (self.seen as f64) < elapsed.as_secs_f64() * bps as f64
    }

    /// Changes the idle limit: the first frame of a response waits for the code
    /// to run, the rest of it must flow.
    pub fn set_idle(&mut self, idle: Duration) {
        self.idle = idle;
    }

    /// Makes `buf` non-empty. `false` at the end of the stream.
    async fn fill(&mut self) -> Result<bool, WireError> {
        while self.buf.is_empty() {
            if self.too_slow() {
                return Err(WireError::Stalled);
            }
            let wait = self
                .idle
                .min(self.deadline.saturating_duration_since(Instant::now()));
            let next = tokio::time::timeout(wait, self.stream.next())
                .await
                .map_err(|_| {
                    if Instant::now() >= self.deadline {
                        WireError::Deadline
                    } else {
                        WireError::Stalled
                    }
                })?;
            match next {
                None => return Ok(false),
                Some(Err(_)) => return Err(WireError::Transport),
                Some(Ok(chunk)) => {
                    self.seen += chunk.len() as u64;
                    self.buf = chunk;
                }
            }
        }
        Ok(true)
    }

    /// Exactly `n` bytes into a vector (for frames, which are small).
    async fn take_vec(&mut self, n: usize) -> Result<Vec<u8>, WireError> {
        let mut out = Vec::with_capacity(n);
        while out.len() < n {
            if !self.fill().await? {
                return Err(WireError::Early);
            }
            let take = (n - out.len()).min(self.buf.len());
            out.extend_from_slice(&self.buf.split_to(take));
        }
        Ok(out)
    }

    /// The next frame's bytes: `None` for the empty frame that ends a request.
    pub async fn frame(&mut self) -> Result<Option<Vec<u8>>, WireError> {
        let len = self.take_vec(4).await?;
        let len = u32::from_be_bytes([len[0], len[1], len[2], len[3]]) as usize;
        if len > HEADER_MAX {
            return Err(WireError::Malformed);
        }
        if len == 0 {
            return Ok(None);
        }
        self.take_vec(len).await.map(Some)
    }

    /// A frame that must be there and parse as `T`.
    pub async fn json<T: serde::de::DeserializeOwned>(&mut self) -> Result<T, WireError> {
        let bytes = self.frame().await?.ok_or(WireError::Malformed)?;
        serde_json::from_slice(&bytes).map_err(|_| WireError::Malformed)
    }

    /// Exactly `size` bytes into `sink`, a chunk at a time.
    pub async fn copy_exact<W: AsyncWrite + Unpin>(
        &mut self,
        size: u64,
        sink: &mut W,
    ) -> Result<(), WireError> {
        let mut left = size;
        while left > 0 {
            if !self.fill().await? {
                return Err(WireError::Early);
            }
            let take = (left.min(self.buf.len() as u64)) as usize;
            let part = self.buf.split_to(take);
            sink.write_all(&part)
                .await
                .map_err(|_| WireError::Transport)?;
            left -= take as u64;
        }
        Ok(())
    }

    /// The stream must be over: a byte after the end is malformed.
    pub async fn expect_end(&mut self) -> Result<(), WireError> {
        match self.fill().await? {
            false => Ok(()),
            true => Err(WireError::Malformed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::stream;

    type Chunks = Vec<Result<Bytes, ()>>;

    fn body(parts: &[&[u8]]) -> impl Stream<Item = Result<Bytes, ()>> + Unpin {
        let chunks: Chunks = parts
            .iter()
            .map(|p| Ok(Bytes::copy_from_slice(p)))
            .collect();
        stream::iter(chunks)
    }

    fn reader(parts: &[&[u8]]) -> Reader<impl Stream<Item = Result<Bytes, ()>> + Unpin> {
        Reader::new(
            body(parts),
            Duration::from_millis(200),
            Duration::from_secs(5),
        )
    }

    fn request_bytes(files: &[(&str, &[u8])]) -> Vec<u8> {
        let header = CallHeader {
            v: WIRE_V2,
            code: "output = 1".into(),
            mode: "restricted".into(),
            timeout_ms: 1000,
            inputs: Map::new(),
            out_mb: 4,
            probe: false,
        };
        let mut out = frame(&serde_json::to_vec(&header).unwrap()).to_vec();
        for (path, bytes) in files {
            let entry = FileEntry {
                path: path.to_string(),
                size: bytes.len() as u64,
            };
            out.extend_from_slice(&frame(&serde_json::to_vec(&entry).unwrap()));
            out.extend_from_slice(bytes);
        }
        out.extend_from_slice(&end_frame());
        out
    }

    /// A request read byte by byte parses as one read whole: chunk boundaries
    /// fall anywhere, inside a length, a frame or a file.
    #[tokio::test]
    async fn a_request_parses_whatever_the_chunk_boundaries() {
        let bytes = request_bytes(&[
            ("manifest.json", b"{}"),
            ("t0/part-00000.parquet", b"abcdef"),
        ]);
        for step in [1usize, 3, 7, bytes.len()] {
            let parts: Vec<&[u8]> = bytes.chunks(step).collect();
            let mut r = reader(&parts);
            let header: CallHeader = r.json().await.unwrap();
            assert_eq!((header.v, header.out_mb), (WIRE_V2, 4));
            let mut seen = vec![];
            while let Some(raw) = r.frame().await.unwrap() {
                let entry: FileEntry = serde_json::from_slice(&raw).unwrap();
                let mut got = vec![];
                r.copy_exact(entry.size, &mut got).await.unwrap();
                seen.push((entry.path, got));
            }
            r.expect_end().await.unwrap();
            assert_eq!(
                seen,
                [
                    ("manifest.json".to_string(), b"{}".to_vec()),
                    ("t0/part-00000.parquet".to_string(), b"abcdef".to_vec())
                ],
                "step {step}"
            );
        }
    }

    #[tokio::test]
    async fn bytes_after_the_end_are_malformed() {
        let mut bytes = request_bytes(&[]);
        bytes.push(b'x');
        let mut r = reader(&[&bytes]);
        r.json::<CallHeader>().await.unwrap();
        assert_eq!(r.frame().await.unwrap(), None);
        assert_eq!(r.expect_end().await, Err(WireError::Malformed));
    }

    /// A file that declares more than arrives is an early end; one that declares
    /// less leaves its extra bytes where the next frame must start.
    #[tokio::test]
    async fn bytes_that_differ_from_the_declared_size_are_caught() {
        let mut bytes = frame(b"{}").to_vec();
        let entry = serde_json::to_vec(&FileEntry {
            path: "manifest.json".into(),
            size: 10,
        })
        .unwrap();
        bytes.extend_from_slice(&frame(&entry));
        bytes.extend_from_slice(b"short");
        let mut r = reader(&[&bytes]);
        r.frame().await.unwrap();
        r.frame().await.unwrap();
        let mut sink = vec![];
        assert_eq!(r.copy_exact(10, &mut sink).await, Err(WireError::Early));

        // Declared 2, sent 6: the next frame read is garbage, not a length.
        let mut bytes = frame(b"{}").to_vec();
        let entry = serde_json::to_vec(&FileEntry {
            path: "manifest.json".into(),
            size: 2,
        })
        .unwrap();
        bytes.extend_from_slice(&frame(&entry));
        bytes.extend_from_slice(b"abZZZZ");
        let mut r = reader(&[&bytes]);
        r.frame().await.unwrap();
        r.frame().await.unwrap();
        let mut sink = vec![];
        r.copy_exact(2, &mut sink).await.unwrap();
        assert_eq!(sink, b"ab");
        assert_eq!(r.frame().await, Err(WireError::Malformed));
    }

    #[tokio::test]
    async fn a_frame_over_the_limit_or_not_json_is_malformed() {
        let mut r = reader(&[&((HEADER_MAX as u32 + 1).to_be_bytes())]);
        assert_eq!(r.frame().await, Err(WireError::Malformed));
        let bytes = frame(b"not json");
        let mut r = reader(&[&bytes]);
        assert_eq!(
            r.json::<CallHeader>().await.unwrap_err(),
            WireError::Malformed
        );
        // An empty frame where a header is required.
        let mut r = reader(&[&end_frame()]);
        assert_eq!(
            r.json::<CallHeader>().await.unwrap_err(),
            WireError::Malformed
        );
    }

    #[tokio::test]
    async fn a_stream_that_stalls_ends_the_transfer_and_one_that_is_slow_overall_too() {
        let stalled =
            stream::iter(vec![Ok::<_, ()>(Bytes::from_static(b"\0\0"))]).chain(stream::pending());
        let mut r = Reader::new(
            Box::pin(stalled),
            Duration::from_millis(50),
            Duration::from_secs(5),
        );
        assert_eq!(r.frame().await, Err(WireError::Stalled));
        // Chunks keep coming, but the whole transfer is over its deadline.
        let slow = stream::unfold(0, |n| async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            Some((Ok::<_, ()>(Bytes::from_static(b"x")), n + 1))
        });
        let mut r = Reader::new(
            Box::pin(slow),
            Duration::from_millis(500),
            Duration::from_millis(100),
        );
        let mut sink = tokio::io::sink();
        assert_eq!(
            r.copy_exact(1_000_000, &mut sink).await,
            Err(WireError::Deadline)
        );
    }

    #[tokio::test]
    async fn a_transport_error_is_reported_without_its_text() {
        let failing = stream::iter(vec![Err::<Bytes, _>("secret transport detail")]);
        let mut r = Reader::new(failing, Duration::from_secs(1), Duration::from_secs(1));
        assert_eq!(r.frame().await, Err(WireError::Transport));
    }

    /// Memory is one chunk: 64 MiB streamed in 1 MiB chunks reaches the sink with
    /// no chunk held past its write.
    #[tokio::test]
    async fn a_large_file_is_never_held_whole() {
        struct Counting {
            written: u64,
            biggest: usize,
        }
        impl AsyncWrite for Counting {
            fn poll_write(
                mut self: std::pin::Pin<&mut Self>,
                _: &mut std::task::Context<'_>,
                buf: &[u8],
            ) -> std::task::Poll<std::io::Result<usize>> {
                self.written += buf.len() as u64;
                self.biggest = self.biggest.max(buf.len());
                std::task::Poll::Ready(Ok(buf.len()))
            }
            fn poll_flush(
                self: std::pin::Pin<&mut Self>,
                _: &mut std::task::Context<'_>,
            ) -> std::task::Poll<std::io::Result<()>> {
                std::task::Poll::Ready(Ok(()))
            }
            fn poll_shutdown(
                self: std::pin::Pin<&mut Self>,
                _: &mut std::task::Context<'_>,
            ) -> std::task::Poll<std::io::Result<()>> {
                std::task::Poll::Ready(Ok(()))
            }
        }
        let mib = 1024 * 1024;
        let chunks = (0..64).map(|_| Ok::<_, ()>(Bytes::from(vec![0u8; mib])));
        let mut r = Reader::new(
            stream::iter(chunks),
            Duration::from_secs(1),
            Duration::from_secs(30),
        );
        let mut sink = Counting {
            written: 0,
            biggest: 0,
        };
        r.copy_exact(64 * mib as u64, &mut sink).await.unwrap();
        assert_eq!(sink.written, 64 * mib as u64);
        assert!(sink.biggest <= mib);
        r.expect_end().await.unwrap();
    }

    #[test]
    fn the_response_header_round_trips_and_names_no_key() {
        let h = ResponseHeader {
            v: WIRE_V2,
            status: RunStatus::Ok,
            message: None,
            output: Some(serde_json::json!({"a": 1})),
            stdout: "hi".into(),
            files: vec![OutEntry {
                name: "a.csv".into(),
                size: 3,
            }],
            dropped: vec![Dropped {
                name: None,
                reason: "bad_name".into(),
            }],
            too_many_entries: false,
        };
        let text = serde_json::to_string(&h).unwrap();
        let back: ResponseHeader = serde_json::from_str(&text).unwrap();
        assert_eq!(back.files, h.files);
        assert_eq!(back.status, RunStatus::Ok);
        assert!(text.contains("\"status\":\"ok\""));
        let r = Refusal {
            refusal: "busy".into(),
            reason: None,
        };
        assert_eq!(
            serde_json::from_str::<Refusal>(&serde_json::to_string(&r).unwrap()).unwrap(),
            r
        );
    }

    /// A frame is refused before it is sent when it cannot be received.
    #[test]
    fn a_frame_over_the_limit_is_refused_on_the_sending_side() {
        assert!(try_frame(&vec![b' '; HEADER_MAX]).is_ok());
        assert_eq!(
            try_frame(&vec![b' '; HEADER_MAX + 1]).unwrap_err(),
            WireError::Malformed
        );
    }

    /// A peer that sends a chunk now and then, each inside the idle limit, but
    /// slower than the minimum rate, is cut off after the grace.
    #[tokio::test]
    async fn a_trickle_under_the_idle_limit_is_cut_off_by_the_minimum_rate() {
        let trickle = stream::unfold(0, |n| async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            Some((Ok::<_, ()>(Bytes::from_static(b"x")), n + 1))
        });
        let mut r = Reader::new(
            Box::pin(trickle),
            Duration::from_secs(1),
            Duration::from_secs(30),
        )
        .with_min_rate(1_000_000, Duration::from_millis(100));
        let mut sink = tokio::io::sink();
        assert_eq!(
            r.copy_exact(1_000_000, &mut sink).await,
            Err(WireError::Stalled)
        );
        // A fast peer is never cut off by it.
        let fast = stream::iter((0..50).map(|_| Ok::<_, ()>(Bytes::from(vec![0u8; 1 << 20]))));
        let mut r = Reader::new(fast, Duration::from_secs(1), Duration::from_secs(30))
            .with_min_rate(1_000_000, Duration::from_millis(1));
        r.copy_exact(50 << 20, &mut tokio::io::sink())
            .await
            .unwrap();
    }
}
