#![cfg(target_os = "linux")]
//! What code run by the subprocess executor can reach, seen from the code
//! itself, in `none` mode (full Python). Complements
//! `python_executor_subprocess.rs`, which covers the network, new processes,
//! hidden paths, `/proc` and the result size limit. Same gate: the tests run
//! only with `COLMENA_PYEXEC_JAIL_TESTS=1` (root, CAP_SYS_ADMIN) and need
//! pandas, numpy and scipy, which the warm template imports.

use colmena::dag_engine::domain::python_executor::{
    PythonExecutor, PythonRunError, PythonRunRequest,
};
use colmena::dag_engine::infrastructure::python_exec::config::SubprocessConfig;
use colmena::dag_engine::infrastructure::python_exec::protocol::{
    result_too_large_message, CRASHED_MESSAGE, MALFORMED_MESSAGE, REFUSED_MESSAGE,
};
use colmena::dag_engine::infrastructure::python_exec::subprocess::SubprocessExecutor;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

fn enabled() -> bool {
    std::env::var("COLMENA_PYEXEC_JAIL_TESTS").as_deref() == Ok("1")
}

/// One slot, a result limit of 1 MiB and uids of its own: the tests run side
/// by side, each with its executor, and apart from the other suite's uids.
fn config() -> SubprocessConfig {
    let mut cfg = SubprocessConfig::from_lookup(&|_: &str| None::<String>).unwrap();
    cfg.bin = PathBuf::from(env!("CARGO_BIN_EXE_python_executor"));
    cfg.slots = 1;
    static NEXT: AtomicU32 = AtomicU32::new(0);
    cfg.uid_base = 40000 + 100 * NEXT.fetch_add(1, Ordering::Relaxed);
    cfg.max_response_bytes = 1 << 20;
    cfg
}

fn executor_with(edit: impl FnOnce(&mut SubprocessConfig)) -> Option<SubprocessExecutor> {
    if !enabled() {
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
    let mut cfg = config();
    edit(&mut cfg);
    Some(SubprocessExecutor::new(cfg, Duration::from_secs(60)).unwrap())
}

fn executor() -> Option<SubprocessExecutor> {
    executor_with(|_| {})
}

fn req(code: &str, secs: u64) -> PythonRunRequest {
    PythonRunRequest {
        code: code.into(),
        mode: "none".into(),
        timeout: Some(Duration::from_secs(secs)),
        inputs: Default::default(),
    }
}

/// The call's `output`, `null` when it assigned none.
async fn run(ex: &SubprocessExecutor, code: &str, secs: u64) -> Result<Value, PythonRunError> {
    ex.run(req(code, secs))
        .await
        .map(|r| r.output.unwrap_or_default())
}

/// The code sees the template's fixed variables and the host's locale, if
/// set, and nothing else of the host's environment, which here holds at
/// least the variable that enables these tests.
#[tokio::test]
async fn the_environment_holds_only_the_template_variables() {
    let Some(ex) = executor() else { return };
    let mut expected = json!({
        "PATH": "/usr/local/bin:/usr/bin:/bin",
        "OPENBLAS_NUM_THREADS": "1",
        "OMP_NUM_THREADS": "1",
        "MKL_NUM_THREADS": "1",
    });
    for k in ["LANG", "LC_ALL", "LC_CTYPE"] {
        if let Ok(v) = std::env::var(k) {
            expected[k] = v.into();
        }
    }
    let env = run(&ex, "import os\noutput = dict(os.environ)", 10).await;
    assert_eq!(env.unwrap(), expected);
}

/// Of these directories only `/tmp` takes a file: the ones any user may write
/// to on the host are covered, and the system ones are not writable by the
/// slot's user (the root filesystem itself is not remounted read-only).
#[tokio::test]
async fn of_these_directories_only_tmp_takes_a_file() {
    let Some(ex) = executor() else { return };
    for d in ["/var/tmp", "/dev/shm"] {
        assert!(std::path::Path::new(d).is_dir(), "{d} exists here");
    }
    let code = "def writes(path):\n\
         \x20   try:\n\
         \x20       with open(path, 'w') as f:\n\
         \x20           f.write('x')\n\
         \x20       return True\n\
         \x20   except OSError:\n\
         \x20       return False\n\
         dirs = ['/tmp', '/var/tmp', '/dev/shm', '/usr/local', '/etc', '/']\n\
         output = [d for d in dirs if writes(d.rstrip('/') + '/probe')]";
    assert_eq!(run(&ex, code, 10).await.unwrap(), json!(["/tmp"]));
}

/// Another process's environment cannot be read, though this test, outside
/// the jail, can read the same file.
#[tokio::test]
async fn another_process_environment_is_unreadable() {
    let Some(ex) = executor() else { return };
    assert!(std::fs::read("/proc/1/environ").is_ok(), "readable here");
    let code = "try:\n\
         \x20   open('/proc/1/environ', 'rb').read()\n\
         \x20   output = 'read'\n\
         except OSError:\n\
         \x20   output = 'refused'";
    assert_eq!(run(&ex, code, 10).await.unwrap(), json!("refused"));
}

/// A file a call leaves in `/tmp` is gone for the next call.
#[tokio::test]
async fn tmp_is_private_to_each_call() {
    let Some(ex) = executor() else { return };
    let write = "import os\nopen('/tmp/marker', 'w').write('x')\noutput = os.listdir('/tmp')";
    assert_eq!(run(&ex, write, 10).await.unwrap(), json!(["marker"]));
    let read = "import os\noutput = os.listdir('/tmp')";
    assert_eq!(run(&ex, read, 10).await.unwrap(), json!([]));
}

/// An allocation over the call's memory budget raises `MemoryError` in the
/// code, which comes back as a Python error; the slot then serves the next
/// call.
#[tokio::test]
async fn memory_exhaustion_is_a_python_error_and_the_next_call_works() {
    let Some(ex) = executor_with(|c| c.memory_mb = 256) else {
        return;
    };
    let e = run(&ex, "x = bytearray(1024**3)\noutput = 1", 20).await;
    let e = e.unwrap_err();
    assert!(
        matches!(&e, PythonRunError::Python(m) if m.contains("MemoryError")),
        "{e:?}"
    );
    assert_eq!(run(&ex, "output = 2", 10).await.unwrap(), json!(2));
}

/// Code that writes its own frame on the call's channel (fd 3) and exits gets
/// an error, whatever the frame, and the slot serves the next call.
#[tokio::test]
async fn a_forged_frame_is_an_error_and_the_next_call_works() {
    let Some(ex) = executor() else { return };
    let cases = [
        // A length over the result limit.
        ("b'\\xff\\xff\\xff\\xff'", result_too_large_message(1 << 20)),
        // A complete frame that is not a response.
        (
            "b'\\x00\\x00\\x00\\x05hello'",
            MALFORMED_MESSAGE.to_string(),
        ),
        // A frame shorter than its length.
        ("b'\\x00\\x00\\x00\\x10abc'", CRASHED_MESSAGE.to_string()),
    ];
    for (frame, message) in cases {
        let code = format!("import os\nos.write(3, {frame})\nos._exit(0)");
        let e = run(&ex, &code, 10).await.unwrap_err();
        assert_eq!(e, PythonRunError::Python(message), "{frame}");
        assert_eq!(run(&ex, "output = 3", 10).await.unwrap(), json!(3));
    }
}

/// With an output policy, a result containing one of its literals does not
/// leave: in `output`, in stdout, in an error's text, or in a frame the code
/// wrote itself. A result without the whole literal does.
#[tokio::test]
async fn a_result_with_a_refused_marker_does_not_leave() {
    let Some(ex) = executor_with(|c| c.refuse_output = vec!["MARK3R.".into()]) else {
        return;
    };
    for code in [
        "output = 'x MARK3R.abc'",
        "print('MARK3R.abc')\noutput = 1",
        "raise ValueError('MARK3R.abc')",
        "import os\nos.write(3, b'\\x00\\x00\\x00\\x07MARK3R.')\nos._exit(0)",
    ] {
        let e = run(&ex, code, 10).await.unwrap_err();
        assert_eq!(e, PythonRunError::Python(REFUSED_MESSAGE.into()), "{code}");
    }
    for clean in ["clean", "MARK3R"] {
        let code = format!("output = {clean:?}");
        assert_eq!(run(&ex, &code, 10).await.unwrap(), json!(clean));
    }
}
