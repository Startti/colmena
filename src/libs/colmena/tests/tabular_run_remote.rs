#![cfg(target_os = "linux")]
//! The server side of a mounts call (`POST /v2/run`) on `serve`'s router over a
//! loopback port, with a token and the real jail. Needs root and CAP_SYS_ADMIN:
//! runs only with `COLMENA_PYEXEC_JAIL_TESTS=1`, one test at a time (they share a
//! staging root and mount tables): `-- --test-threads=1`.

use bytes::Bytes;
use colmena::dag_engine::infrastructure::python_exec::config::SubprocessConfig;
use colmena::dag_engine::infrastructure::python_exec::server::{router, AppState};
use colmena::dag_engine::infrastructure::python_exec::subprocess::SubprocessExecutor;
use colmena::tabular_run::wire::{
    end_frame, frame, CallHeader, FileEntry, Reader, Refusal, ResponseHeader, RunStatus, WIRE_V2,
};
use futures::{stream, StreamExt};
use serde_json::{json, Map};
use std::os::unix::fs::DirBuilderExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

const TOKEN: &str = "remote-test-token-0123456789abcdef";

fn enabled() -> bool {
    if std::env::var("COLMENA_PYEXEC_JAIL_TESTS").as_deref() == Ok("1") {
        return true;
    }
    eprintln!("skipped: set COLMENA_PYEXEC_JAIL_TESTS=1 (Linux, root, CAP_SYS_ADMIN)");
    false
}

fn staging_root() -> PathBuf {
    let path = PathBuf::from("/var/lib/colmena-tabular-remote-test");
    let made = std::fs::DirBuilder::new().mode(0o700).create(&path);
    assert!(made.is_ok() || path.is_dir(), "{made:?}");
    path
}

struct Server {
    url: String,
    exec: Arc<SubprocessExecutor>,
    root: Option<PathBuf>,
}

async fn serve(root: Option<PathBuf>) -> Server {
    pyo3::Python::initialize();
    let mut cfg = SubprocessConfig::from_lookup(&|_: &str| None::<String>).unwrap();
    cfg.bin = PathBuf::from(env!("CARGO_BIN_EXE_python_executor"));
    cfg.slots = 2;
    static NEXT: AtomicU32 = AtomicU32::new(0);
    cfg.uid_base = 40000 + 100 * NEXT.fetch_add(1, Ordering::Relaxed);
    cfg.max_response_bytes = 1 << 20;
    cfg.staging_root = root.clone();
    let exec = Arc::new(SubprocessExecutor::new(cfg, Duration::from_secs(60)).unwrap());
    let state = AppState {
        exec: exec.clone(),
        token: Some(Arc::new(TOKEN.into())),
        ready: Arc::new(true.into()),
        max_timeout: Duration::from_secs(60),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v2/run", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, router(state)).await });
    Server { url, exec, root }
}

fn header(code: &str) -> Vec<u8> {
    let h = CallHeader {
        v: WIRE_V2,
        code: code.into(),
        mode: "none".into(),
        timeout_ms: 30_000,
        inputs: Map::new(),
        out_mb: 4,
    };
    frame(&serde_json::to_vec(&h).unwrap()).to_vec()
}

fn entry(path: &str, size: u64) -> Vec<u8> {
    let e = FileEntry {
        path: path.into(),
        size,
    };
    frame(&serde_json::to_vec(&e).unwrap()).to_vec()
}

fn request(code: &str, files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut out = header(code);
    for (path, bytes) in files {
        out.extend(entry(path, bytes.len() as u64));
        out.extend_from_slice(bytes);
    }
    out.extend_from_slice(&end_frame());
    out
}

fn post(url: &str, token: Option<&str>, body: reqwest::Body) -> reqwest::RequestBuilder {
    let rb = reqwest::Client::new().post(url).body(body);
    match token {
        Some(t) => rb.bearer_auth(t),
        None => rb,
    }
}

/// A body that sends `bytes` and then neither ends nor sends more.
fn stalling(bytes: Vec<u8>) -> reqwest::Body {
    let head = stream::iter(vec![Ok::<_, std::io::Error>(Bytes::from(bytes))]);
    reqwest::Body::wrap_stream(head.chain(stream::pending()))
}

async fn released(s: &Server) {
    for _ in 0..100 {
        let gone = s
            .root
            .as_ref()
            .is_none_or(|r| std::fs::read_dir(r).unwrap().count() == 0);
        if s.exec.staged_in_flight() == (0, 0) && gone {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!(
        "the volume was not released: {:?}",
        s.exec.staged_in_flight()
    );
}

const CODE: &str = r#"
import os
sizes = [os.path.getsize('/data/t0/' + p) for p in sorted(os.listdir('/data/t0'))]
open('/out/result.csv', 'w').write('n\n' + str(sum(sizes)) + '\n')
output = sizes
"#;

#[tokio::test]
async fn a_multi_part_call_runs_and_streams_its_output_back() {
    if !enabled() {
        return;
    }
    let s = serve(Some(staging_root())).await;
    let body = request(
        CODE,
        &[
            ("manifest.json", b"{}"),
            ("t0/part-00000.parquet", &[b'a'; 1000]),
            ("t0/part-00001.parquet", &[b'b'; 500]),
        ],
    );
    let resp = post(&s.url, Some(TOKEN), body.into()).send().await.unwrap();
    assert_eq!(resp.status(), 200);
    let mut reader = Reader::new(
        resp.bytes_stream(),
        Duration::from_secs(5),
        Duration::from_secs(30),
    );
    let head: ResponseHeader = reader.json().await.unwrap();
    assert_eq!(head.status, RunStatus::Ok, "{head:?}");
    assert_eq!(head.output, Some(json!([1000, 500])));
    assert_eq!(head.files.len(), 1);
    assert_eq!(head.files[0].name, "result.csv");
    let mut got = vec![];
    reader
        .copy_exact(head.files[0].size, &mut got)
        .await
        .unwrap();
    assert_eq!(got, b"n\n1500\n");
    reader.expect_end().await.unwrap();
    released(&s).await;
}

#[tokio::test]
async fn the_gate_is_the_v1_gate_a_missing_or_wrong_token_is_401() {
    if !enabled() {
        return;
    }
    let s = serve(Some(staging_root())).await;
    for token in [None, Some("a-token-this-server-does-not-accept")] {
        let r = post(&s.url, token, request("output = 1", &[]).into())
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 401, "{token:?}");
    }
    assert_eq!(s.exec.staged_in_flight(), (0, 0));
}

/// A file declaring more than arrives, bytes after the end, a path that is not
/// a canonical part, a duplicate and a file over its cap: each is refused, and
/// the volume is back every time.
#[tokio::test]
async fn a_malformed_or_oversized_upload_is_refused_and_leaves_nothing() {
    if !enabled() {
        return;
    }
    let s = serve(Some(staging_root())).await;
    let post_bytes = |b: Vec<u8>| post(&s.url, Some(TOKEN), b.into()).send();
    // Declared 100, sent 5, then the stream ends.
    let mut early = header("output = 1");
    early.extend(entry("manifest.json", 100));
    early.extend_from_slice(b"short");
    assert_eq!(post_bytes(early).await.unwrap().status(), 400);
    // Bytes after the terminating frame.
    let mut extra = request("output = 1", &[("manifest.json", b"{}")]);
    extra.extend_from_slice(b"EXTRA");
    assert_eq!(post_bytes(extra).await.unwrap().status(), 400);
    // Bytes beyond the declared size run into the next frame.
    let mut over = header("output = 1");
    over.extend(entry("manifest.json", 2));
    over.extend_from_slice(b"{}ZZZZZZZZ");
    assert_eq!(post_bytes(over).await.unwrap().status(), 400);
    // Paths that are not the manifest or a canonical part, and a duplicate.
    for path in ["../escape", "/etc/passwd", "t0/../x", "t0/part-1.parquet"] {
        assert_eq!(
            post_bytes(request("output = 1", &[(path, b"x")]))
                .await
                .unwrap()
                .status(),
            400,
            "{path}"
        );
    }
    let dup = request(
        "output = 1",
        &[("manifest.json", b"1"), ("manifest.json", b"2")],
    );
    assert_eq!(post_bytes(dup).await.unwrap().status(), 400);
    // A part declaring over its cap is refused before its bytes are read: the
    // body below never sends them and never ends.
    let mut huge = header("output = 1");
    huge.extend(entry("t0/part-00000.parquet", 200 * 1024 * 1024));
    let r = post(&s.url, Some(TOKEN), stalling(huge))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 413);
    assert_eq!(r.json::<Refusal>().await.unwrap().refusal, "too_large");
    // A bad header: wrong version, a mode that is not one, an output size of 0.
    for (v, mode, out_mb) in [(1, "none", 4), (WIRE_V2, "root", 4), (WIRE_V2, "none", 0)] {
        let h = CallHeader {
            v,
            code: "x".into(),
            mode: mode.into(),
            timeout_ms: 1000,
            inputs: Map::new(),
            out_mb,
        };
        let mut b = frame(&serde_json::to_vec(&h).unwrap()).to_vec();
        b.extend_from_slice(&end_frame());
        assert_eq!(
            post_bytes(b).await.unwrap().status(),
            400,
            "{v} {mode} {out_mb}"
        );
    }
    released(&s).await;
}

/// A stream that stops sending is cut off by the idle limit (30 s) and gives
/// its volume back. Slow by construction.
#[tokio::test]
async fn an_upload_that_stalls_is_cut_off_and_releases_its_volume() {
    if !enabled() {
        return;
    }
    let s = serve(Some(staging_root())).await;
    let mut partial = header("output = 1");
    partial.extend(entry("t0/part-00000.parquet", 1000));
    partial.extend_from_slice(&[b'x'; 10]);
    let r = post(&s.url, Some(TOKEN), stalling(partial))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 408);
    released(&s).await;
}

/// A client that goes away mid-upload: the handler is dropped, and with it the
/// volume.
#[tokio::test]
async fn a_client_that_disconnects_mid_upload_releases_the_volume() {
    if !enabled() {
        return;
    }
    let s = serve(Some(staging_root())).await;
    let mut partial = header("output = 1");
    partial.extend(entry("t0/part-00000.parquet", 1000));
    partial.extend_from_slice(&[b'x'; 10]);
    let (url, body) = (s.url.clone(), stalling(partial));
    let call = tokio::spawn(async move { post(&url, Some(TOKEN), body).send().await });
    for _ in 0..50 {
        if s.exec.staged_in_flight().0 == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(
        s.exec.staged_in_flight().0,
        1,
        "the call was holding a volume"
    );
    call.abort();
    let _ = call.await;
    released(&s).await;
}

/// Two calls fit the executor's budget of volumes; a third is told to retry,
/// with `Retry-After`, and once one ends a new call is taken.
#[tokio::test]
async fn calls_within_the_budget_run_and_one_over_it_is_told_to_retry() {
    if !enabled() {
        return;
    }
    let s = serve(Some(staging_root())).await;
    let hold = |n: usize| {
        let mut b = header("output = 1");
        b.extend(entry("t0/part-00000.parquet", 1000));
        b.extend_from_slice(&[b'x'; 10]);
        let (url, body) = (s.url.clone(), stalling(b));
        let _ = n;
        tokio::spawn(async move { post(&url, Some(TOKEN), body).send().await })
    };
    let (a, b) = (hold(0), hold(1));
    for _ in 0..50 {
        if s.exec.staged_in_flight().0 == 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(s.exec.staged_in_flight().0, 2, "within the budget");
    let over = post(&s.url, Some(TOKEN), request("output = 1", &[]).into())
        .send()
        .await
        .unwrap();
    assert_eq!(over.status(), 503);
    assert!(over.headers().contains_key("retry-after"));
    assert_eq!(over.json::<Refusal>().await.unwrap().refusal, "busy");
    a.abort();
    b.abort();
    released(&s).await;
    let ok = post(&s.url, Some(TOKEN), request("output = 1", &[]).into())
        .send()
        .await
        .unwrap();
    assert_eq!(ok.status(), 200);
    released(&s).await;
}

/// Two calls at once, both within the budget, both complete with their own data.
#[tokio::test]
async fn two_concurrent_calls_each_see_only_their_own_data() {
    if !enabled() {
        return;
    }
    let s = serve(Some(staging_root())).await;
    let call = |fill: u8, n: usize| {
        let url = s.url.clone();
        tokio::spawn(async move {
            let body = request(
                CODE,
                &[
                    ("manifest.json", b"{}"),
                    ("t0/part-00000.parquet", &vec![fill; n]),
                ],
            );
            let r = post(&url, Some(TOKEN), body.into()).send().await.unwrap();
            let mut reader = Reader::new(
                r.bytes_stream(),
                Duration::from_secs(5),
                Duration::from_secs(30),
            );
            reader.json::<ResponseHeader>().await.unwrap().output
        })
    };
    let (a, b) = (call(b'a', 100), call(b'b', 200));
    assert_eq!(a.await.unwrap(), Some(json!([100])));
    assert_eq!(b.await.unwrap(), Some(json!([200])));
    released(&s).await;
}

#[tokio::test]
async fn a_server_without_a_staging_root_has_no_mounts_route() {
    if !enabled() {
        return;
    }
    let s = serve(None).await;
    let r = post(&s.url, Some(TOKEN), request("output = 1", &[]).into())
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404);
}

/// A staging root the template cannot use turns the mounts off, and the server
/// says so BEFORE it reads the call's data: the body below never ends.
#[tokio::test]
async fn a_server_whose_mounts_are_disabled_refuses_before_reading_the_data() {
    if !enabled() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let broken = tmp.path().canonicalize().unwrap();
    let target = std::ffi::CString::new(broken.to_str().unwrap()).unwrap();
    let rc = unsafe {
        libc::mount(
            c"tmpfs".as_ptr(),
            target.as_ptr(),
            c"tmpfs".as_ptr(),
            libc::MS_RDONLY,
            c"size=1m".as_ptr() as *const libc::c_void,
        )
    };
    assert_eq!(rc, 0, "{}", std::io::Error::last_os_error());
    struct Unmount(std::ffi::CString);
    impl Drop for Unmount {
        fn drop(&mut self) {
            unsafe { libc::umount2(self.0.as_ptr(), libc::MNT_DETACH) };
        }
    }
    let _guard = Unmount(target);
    let s = serve(Some(broken)).await;
    let r = post(&s.url, Some(TOKEN), stalling(header("output = 1")))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 503);
    let refusal = r.json::<Refusal>().await.unwrap();
    assert_eq!(refusal.refusal, "mounts_disabled");
    assert_eq!(refusal.reason.as_deref(), Some("staging_unusable"));
}
