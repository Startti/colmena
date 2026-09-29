#![cfg(target_os = "linux")]
//! `python_executor serve`'s router over HTTP, with the real binary's
//! template. Each call runs in the process jail, which needs root and
//! CAP_SYS_ADMIN, so the tests run only with `COLMENA_PYEXEC_JAIL_TESTS=1`.
//! The checks made before a call are covered, without a jail, by the unit
//! tests.

use colmena::dag_engine::infrastructure::python_exec::config::SubprocessConfig;
use colmena::dag_engine::infrastructure::python_exec::protocol::WireResponse;
use colmena::dag_engine::infrastructure::python_exec::server::{router, AppState};
use colmena::dag_engine::infrastructure::python_exec::subprocess::SubprocessExecutor;
use reqwest::StatusCode;
use serde_json::json;
use std::{path::PathBuf, sync::Arc, time::Duration};

/// State for a router with a fresh executor, running as uids from `uid_base`.
fn state(uid_base: u32, ready: bool) -> Option<AppState> {
    if std::env::var("COLMENA_PYEXEC_JAIL_TESTS").as_deref() != Ok("1") {
        eprintln!("skipped: set COLMENA_PYEXEC_JAIL_TESTS=1 (Linux, root, CAP_SYS_ADMIN)");
        return None;
    }
    let mut cfg = SubprocessConfig::from_lookup(&|_: &str| None::<String>).unwrap();
    cfg.bin = PathBuf::from(env!("CARGO_BIN_EXE_python_executor"));
    (cfg.slots, cfg.uid_base) = (1, uid_base);
    let exec = Arc::new(SubprocessExecutor::new(cfg, Duration::from_secs(60)).unwrap());
    let (token, ready) = (Some(Arc::new("s3cret".into())), Arc::new(ready.into()));
    let max_timeout = Duration::from_secs(60);
    Some(AppState {
        exec,
        token,
        ready,
        max_timeout,
    })
}

/// Serves `state` on a loopback port; its base URL.
async fn listen(state: AppState) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, router(state)).await });
    url
}

fn wire(code: &str) -> Vec<u8> {
    let body = json!({"v": 1, "code": code, "mode": "none", "timeout_ms": 30000, "inputs": {}});
    serde_json::to_vec(&body).unwrap()
}

#[tokio::test]
async fn a_call_is_answered_compressed_and_over_h2c() {
    let Some(st) = state(30100, true) else {
        return;
    };
    let url = format!("{}/v1/run", listen(st).await);
    let packed = zstd::bulk::compress(&wire("output = 1 + 2"), 3).unwrap();
    let req = reqwest::Client::new().post(&url).bearer_auth("s3cret");
    let req = req
        .header("content-encoding", "zstd")
        .header("accept-encoding", "zstd");
    let resp = req.body(packed.clone()).send().await.unwrap();
    let encoding = resp.headers()["content-encoding"]
        .to_str()
        .ok()
        .map(str::to_string);
    assert_eq!(
        (resp.status(), encoding),
        (StatusCode::OK, Some("zstd".into()))
    );
    let body = zstd::bulk::decompress(&resp.bytes().await.unwrap(), 1 << 20).unwrap();
    let body: WireResponse = serde_json::from_slice(&body).unwrap();
    assert_eq!(body.output, Some(json!(3)));

    let h2c = reqwest::Client::builder()
        .http2_prior_knowledge()
        .build()
        .unwrap();
    let req = h2c
        .post(&url)
        .bearer_auth("s3cret")
        .header("content-encoding", "zstd");
    let resp = req.body(packed).send().await.unwrap();
    assert_eq!(
        (resp.status(), resp.version()),
        (StatusCode::OK, reqwest::Version::HTTP_2)
    );
    let body: WireResponse = serde_json::from_slice(&resp.bytes().await.unwrap()).unwrap();
    assert_eq!(body.output, Some(json!(3)));
}
