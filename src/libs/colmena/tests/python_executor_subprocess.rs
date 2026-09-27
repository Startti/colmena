#![cfg(target_os = "linux")]
//! The subprocess executor with the real `python_executor` binary. Each call
//! runs in the process jail, which needs root and CAP_SYS_ADMIN, so the tests
//! run only with `COLMENA_PYEXEC_JAIL_TESTS=1`; they then also need pandas,
//! numpy and scipy, which the warm template imports before it serves a call.

use colmena::dag_engine::domain::python_executor::{
    PythonExecutor, PythonRunError, PythonRunRequest,
};
use colmena::dag_engine::infrastructure::python_exec::config::SubprocessConfig;
use colmena::dag_engine::infrastructure::python_exec::inprocess::InProcessExecutor;
use colmena::dag_engine::infrastructure::python_exec::protocol::result_too_large_message;
use colmena::dag_engine::infrastructure::python_exec::subprocess::SubprocessExecutor;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn jail_tests_enabled() -> bool {
    std::env::var("COLMENA_PYEXEC_JAIL_TESTS").as_deref() == Ok("1")
}

fn executor_with(slots: usize, max_response_bytes: usize) -> Option<SubprocessExecutor> {
    if !jail_tests_enabled() {
        eprintln!("skipped: set COLMENA_PYEXEC_JAIL_TESTS=1 (Linux, root, CAP_SYS_ADMIN)");
        return None;
    }
    pyo3::Python::initialize();
    let stack = pyo3::Python::attach(|py| {
        ["pandas", "numpy", "scipy.stats"]
            .iter()
            .all(|m| py.import(*m).is_ok())
    });
    assert!(
        stack,
        "the jail tests need pandas, numpy and scipy importable"
    );
    let mut cfg = SubprocessConfig::from_lookup(&|_: &str| None::<String>).unwrap();
    cfg.bin = PathBuf::from(env!("CARGO_BIN_EXE_python_executor"));
    cfg.slots = slots;
    // Uids of their own: the tests run side by side, each with its executor.
    static NEXT: AtomicU32 = AtomicU32::new(0);
    cfg.uid_base = 20000 + 100 * NEXT.fetch_add(1, Ordering::Relaxed);
    cfg.max_response_bytes = max_response_bytes;
    Some(SubprocessExecutor::new(cfg, Duration::from_secs(60)).unwrap())
}

fn executor(slots: usize) -> Option<SubprocessExecutor> {
    executor_with(slots, 1 << 20)
}

fn req(code: &str, secs: u64) -> PythonRunRequest {
    PythonRunRequest {
        code: code.into(),
        mode: "none".into(),
        timeout: Some(Duration::from_secs(secs)),
        inputs: Default::default(),
    }
}

/// Live processes (zombies aside) whose real uid is `uid`.
fn processes_of(uid: u32) -> usize {
    let uid = uid.to_string();
    let statuses = std::fs::read_dir("/proc")
        .unwrap()
        .filter_map(|e| std::fs::read_to_string(e.ok()?.path().join("status")).ok());
    statuses
        .filter(|s| {
            let field = |k: &str| s.lines().find_map(|l| l.strip_prefix(k)).unwrap_or("");
            field("Uid:").split_whitespace().next() == Some(uid.as_str())
                && !field("State:").trim_start().starts_with('Z')
        })
        .count()
}

/// Waits up to two seconds for no process to run as `uid`.
async fn none_left(uid: u32) -> bool {
    let t0 = Instant::now();
    while processes_of(uid) > 0 && t0.elapsed() < Duration::from_secs(2) {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    processes_of(uid) == 0
}

/// The uid the executor's only slot runs as.
async fn slot_uid(ex: &SubprocessExecutor) -> u32 {
    let r = ex.run(req("import os\noutput = os.getuid()", 10)).await;
    let uid = r.unwrap().output.and_then(|v| v.as_u64()).unwrap();
    assert_ne!(uid, 0);
    u32::try_from(uid).unwrap()
}

#[tokio::test]
async fn runs_pandas_in_a_child() {
    let Some(ex) = executor(2) else { return };
    let code = "import pandas as pd\noutput = int(pd.Series([1, 2, 3]).sum())";
    let r = ex.run(req(code, 30)).await.unwrap();
    assert_eq!(r.output, Some(serde_json::json!(6)));
}

#[tokio::test]
async fn a_deadline_kills_the_child() {
    let Some(ex) = executor(1) else { return };
    ex.warm().await.unwrap();
    let t0 = Instant::now();
    let e = ex.run(req("while True:\n    pass", 2)).await.unwrap_err();
    assert_eq!(e, PythonRunError::Timeout);
    // The deadline ends it, not the CPU limit a second later.
    assert!(
        t0.elapsed() < Duration::from_millis(2500),
        "{:?}",
        t0.elapsed()
    );
    // The slot is usable again.
    assert!(ex.run(req("output = 1", 10)).await.is_ok());
}

/// A call whose caller stops waiting (its future dropped) kills its child,
/// and the slot is usable again once the child is gone.
#[tokio::test]
async fn an_abandoned_call_kills_its_child() {
    let Some(ex) = executor(1) else { return };
    let uid = slot_uid(&ex).await;
    let mut call = Box::pin(ex.run(req("while True:\n    pass", 30)));
    let early = tokio::time::timeout(Duration::from_secs(1), &mut call).await;
    assert!(early.is_err(), "the call ended on its own");
    // Its child is the one process running as the slot's uid.
    assert_eq!(processes_of(uid), 1);
    drop(call);
    assert!(
        none_left(uid).await,
        "the abandoned call's child still runs"
    );
    let next = tokio::time::timeout(Duration::from_secs(10), ex.run(req("output = 1", 10)));
    assert!(next.await.expect("the slot came back").is_ok());
}

#[tokio::test]
async fn one_slot_runs_one_call_at_a_time() {
    let Some(ex) = executor(1) else { return };
    ex.warm().await.unwrap();
    let nap = "import time\ntime.sleep(0.5)\noutput = 1";
    let t0 = Instant::now();
    let (a, b) = tokio::join!(ex.run(req(nap, 10)), ex.run(req(nap, 10)));
    assert!(a.is_ok() && b.is_ok());
    assert!(t0.elapsed() >= Duration::from_secs(1), "{:?}", t0.elapsed());
}

#[tokio::test]
async fn module_state_does_not_leak_between_calls() {
    let Some(ex) = executor(1) else { return };
    ex.run(req("import json\njson.colmena_marker = 1\noutput = 1", 10))
        .await
        .unwrap();
    let code = "import json\noutput = hasattr(json, 'colmena_marker')";
    let r = ex.run(req(code, 10)).await.unwrap();
    assert_eq!(r.output, Some(serde_json::json!(false)));
}

#[tokio::test]
async fn random_numbers_differ_between_calls() {
    let Some(ex) = executor(1) else { return };
    let code =
        "import numpy as np\nimport random\noutput = [float(np.random.rand()), random.random()]";
    let a = ex.run(req(code, 10)).await.unwrap().output;
    let b = ex.run(req(code, 10)).await.unwrap().output;
    assert_ne!(a, b);
}

#[tokio::test]
async fn concurrent_calls_keep_their_own_stdout() {
    let Some(ex) = executor(2) else { return };
    let (a, b) = tokio::join!(
        ex.run(req("for _ in range(2000):\n    print('A')\noutput = 1", 30)),
        ex.run(req("for _ in range(2000):\n    print('B')\noutput = 2", 30)),
    );
    assert!(!a.unwrap().stdout.contains('B'));
    assert!(!b.unwrap().stdout.contains('A'));
}

#[tokio::test]
async fn a_dead_child_is_reported_not_hung() {
    let Some(ex) = executor(1) else { return };
    let e = ex.run(req("import os\nos._exit(9)", 10)).await.unwrap_err();
    assert!(
        matches!(e, PythonRunError::Python(ref m) if m.contains("ended without returning a result")),
        "{e:?}"
    );
}

#[tokio::test]
async fn a_result_over_the_limit_is_refused() {
    let Some(ex) = executor_with(1, 1024) else {
        return;
    };
    let e = ex.run(req("output = 'x' * 4096", 10)).await.unwrap_err();
    assert_eq!(e, PythonRunError::Python(result_too_large_message(1024)));
    assert!(ex.run(req("output = 1", 10)).await.is_ok());
}

/// The template dies with the thread that started it, so a call made from a
/// thread that then exits must not be the one that starts it.
#[tokio::test]
async fn the_template_outlives_the_thread_of_the_first_call() {
    let Some(ex) = executor(1) else { return };
    let ex = Arc::new(ex);
    let parent = "import os\noutput = os.getppid()";
    let first = {
        let ex = ex.clone();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(ex.run(req(parent, 30))).unwrap().output
        })
        .join()
        .unwrap()
    };
    tokio::time::sleep(Duration::from_millis(500)).await;
    let second = ex.run(req(parent, 30)).await.unwrap().output;
    assert_eq!(second, first, "the template was started again");
}

#[tokio::test]
async fn text_encodings_match_the_in_process_interpreter() {
    let Some(ex) = executor(1) else { return };
    let code = "import locale, sys\noutput = [locale.getpreferredencoding(False), sys.getfilesystemencoding()]";
    let here = InProcessExecutor.run(req(code, 10)).await.unwrap().output;
    assert_eq!(ex.run(req(code, 10)).await.unwrap().output, here);
}

#[tokio::test]
async fn the_child_network_namespace_has_no_route_anywhere() {
    let Some(ex) = executor(1) else { return };
    // A listener in the test's own namespace: the child must not reach it either.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let code = format!(
        "import socket\n\
         def tcp(host, port):\n\
         \x20   try:\n\
         \x20       socket.create_connection((host, port), timeout=2).close()\n\
         \x20       return 'open'\n\
         \x20   except OSError:\n\
         \x20       return 'refused'\n\
         try:\n\
         \x20   socket.getaddrinfo('example.com', 443)\n\
         \x20   dns = 'resolved'\n\
         except OSError:\n\
         \x20   dns = 'unresolved'\n\
         output = {{'dns': dns, 'link_local': tcp('169.254.169.254', 80), 'public': tcp('1.1.1.1', 443), 'loopback': tcp('127.0.0.1', {port})}}"
    );
    let r = ex.run(req(&code, 20)).await.unwrap();
    assert_eq!(
        r.output,
        Some(
            serde_json::json!({"dns": "unresolved", "link_local": "refused", "public": "refused", "loopback": "refused"})
        )
    );
}

/// Inside the jail the call's connection is the only descriptor besides the
/// standard three, which lead nowhere.
#[tokio::test]
async fn the_child_keeps_only_its_connection_and_null_streams() {
    let Some(ex) = executor(1) else { return };
    let code = "import os\n\
         fds = os.listdir('/proc/self/fd')\n\
         output = [len(fds)] + [os.readlink(f'/proc/self/fd/{n}') for n in (0, 1, 2)]";
    let r = ex.run(req(code, 10)).await.unwrap();
    // 0, 1, 2, the connection and the listing's own descriptor.
    let devnull = "/dev/null";
    assert_eq!(
        r.output,
        Some(serde_json::json!([5, devnull, devnull, devnull]))
    );
}
