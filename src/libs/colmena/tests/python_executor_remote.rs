#![cfg(target_os = "linux")]
//! The remote executor against `python_executor serve`'s router on a loopback
//! port, with a token and the real binary's template. Each call runs in the
//! process jail, which needs root and CAP_SYS_ADMIN, so the test runs only
//! with `COLMENA_PYEXEC_JAIL_TESTS=1`. CI also runs the golden bench through
//! a `serve` process.

use colmena::dag_engine::domain::python_executor::{PythonExecutor, PythonRunRequest};
use colmena::dag_engine::infrastructure::python_exec::config::SubprocessConfig;
use colmena::dag_engine::infrastructure::python_exec::config::{RemoteAuthConfig, RemoteConfig};
use colmena::dag_engine::infrastructure::python_exec::inprocess::InProcessExecutor;
use colmena::dag_engine::infrastructure::python_exec::remote::RemoteExecutor;
use colmena::dag_engine::infrastructure::python_exec::server::{router, AppState};
use colmena::dag_engine::infrastructure::python_exec::subprocess::SubprocessExecutor;
use std::{path::PathBuf, sync::Arc, time::Duration};

const TOKEN: &str = "remote-test-token-0123456789abcdef";

fn req(code: &str, mode: &str) -> PythonRunRequest {
    let inputs = serde_json::json!({ "x": 20 }).as_object().cloned().unwrap();
    let (code, mode, timeout) = (code.into(), mode.into(), Some(Duration::from_secs(30)));
    PythonRunRequest {
        code,
        mode,
        timeout,
        inputs,
    }
}

#[tokio::test]
async fn calls_through_the_server_match_the_in_process_results() {
    if std::env::var("COLMENA_PYEXEC_JAIL_TESTS").as_deref() != Ok("1") {
        eprintln!("skipped: set COLMENA_PYEXEC_JAIL_TESTS=1 (Linux, root, CAP_SYS_ADMIN)");
        return;
    }
    pyo3::Python::initialize();
    let mut sub = SubprocessConfig::from_lookup(&|_: &str| None::<String>).unwrap();
    sub.bin = PathBuf::from(env!("CARGO_BIN_EXE_python_executor"));
    (sub.slots, sub.uid_base) = (2, 30400);
    let exec = Arc::new(SubprocessExecutor::new(sub.clone(), Duration::from_secs(60)).unwrap());
    let (token, ready) = (Some(Arc::new(TOKEN.into())), Arc::new(true.into()));
    let max_timeout = Duration::from_secs(60);
    let state = AppState {
        exec,
        token,
        ready,
        max_timeout,
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, router(state)).await });
    let dir = tempfile::tempdir().unwrap();
    let client = |token: &str| {
        let file = dir.path().join(token);
        std::fs::write(&file, token).unwrap();
        let (url, auth) = (url.parse().unwrap(), RemoteAuthConfig::BearerFile(file));
        let (max_request_bytes, max_response_bytes) =
            (sub.max_request_bytes, sub.max_response_bytes);
        let max_wire_bytes = Some(1 << 20);
        let cfg = RemoteConfig {
            url,
            auth,
            max_request_bytes,
            max_response_bytes,
            max_wire_bytes,
        };
        RemoteExecutor::new(cfg, Duration::from_secs(60)).unwrap()
    };
    let remote = client(TOKEN);
    assert_eq!(remote.warm().await, Ok(()));
    let sum = remote.run(req("output = x + 1", "none")).await.unwrap();
    assert_eq!(sum.output, Some(serde_json::json!(21)));
    for (code, mode) in [
        ("output = x + 1", "none"),
        ("y = 1", "restricted"),
        ("print('ñandú 😀')\noutput = None", "restricted"),
        ("import os", "restricted"),
        ("output = {}['missing']", "restricted"),
        ("output = (", "restricted"),
        ("output = 2**70", "restricted"),
    ] {
        let expected = InProcessExecutor.run(req(code, mode)).await;
        assert_eq!(remote.run(req(code, mode)).await, expected, "{code}");
    }
    let refused = client("a-token-this-server-does-not-accept");
    let e = refused.run(req("output = 1", "none")).await.unwrap_err();
    assert!(
        e.to_string().contains("rejected this caller's credentials"),
        "{e}"
    );
}
