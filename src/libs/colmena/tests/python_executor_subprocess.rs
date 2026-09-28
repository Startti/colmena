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
use colmena::dag_engine::infrastructure::python_exec::selftest;
use colmena::dag_engine::infrastructure::python_exec::subprocess::SubprocessExecutor;
use std::collections::BTreeSet;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn jail_tests_enabled() -> bool {
    std::env::var("COLMENA_PYEXEC_JAIL_TESTS").as_deref() == Ok("1")
}

fn executor_with(
    slots: usize,
    edit: impl FnOnce(&mut SubprocessConfig),
) -> Option<SubprocessExecutor> {
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
    cfg.max_response_bytes = 1 << 20;
    edit(&mut cfg);
    Some(SubprocessExecutor::new(cfg, Duration::from_secs(60)).unwrap())
}

fn executor(slots: usize) -> Option<SubprocessExecutor> {
    executor_with(slots, |_| {})
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

/// A process that does not run as root cannot start the executor: each child
/// switches to an unprivileged user of its own, which only root may do. Needs
/// no jail, so it is not gated; as root there is nothing to check.
#[test]
fn without_root_the_executor_does_not_start() {
    if unsafe { libc::geteuid() } == 0 {
        eprintln!("skipped: runs as root");
        return;
    }
    let mut cfg = SubprocessConfig::from_lookup(&|_: &str| None::<String>).unwrap();
    cfg.bin = PathBuf::from(env!("CARGO_BIN_EXE_python_executor"));
    let e = SubprocessExecutor::new(cfg, Duration::from_secs(60)).err();
    assert_eq!(
        e.map(|e| e.0).as_deref(),
        Some("the subprocess executor must start as root inside its container: children switch to unprivileged users")
    );
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

/// The call's code cannot start a process: `os.fork()` and `subprocess` each
/// raise an error the code can catch, the call returns normally with it, and
/// nothing runs as the slot's uid afterwards.
#[tokio::test]
async fn a_call_cannot_start_a_process() {
    let Some(ex) = executor(1) else { return };
    let uid = slot_uid(&ex).await;
    let code = "import os, subprocess\n\
         def attempt(start):\n\
         \x20   try:\n\
         \x20       start()\n\
         \x20       return 'started'\n\
         \x20   except OSError as e:\n\
         \x20       return type(e).__name__\n\
         def fork():\n\
         \x20   if os.fork() == 0:\n\
         \x20       os._exit(0)\n\
         output = [attempt(fork), attempt(lambda: subprocess.run(['/bin/true']))]";
    let r = ex.run(req(code, 20)).await.unwrap();
    assert_eq!(
        r.output,
        Some(serde_json::json!(["PermissionError", "PermissionError"]))
    );
    assert!(
        none_left(uid).await,
        "a process still runs as the slot's uid"
    );
}

/// Whatever still runs as the slot's uid when a call ends is stopped then,
/// whoever started it. A call's code cannot start a process, so a stand-in
/// started here as that uid takes its place; it is this test's child, so its
/// exit status shows what ended it.
#[tokio::test]
async fn what_runs_as_the_slots_uid_is_stopped_after_a_call() {
    let Some(ex) = executor(1) else { return };
    let uid = slot_uid(&ex).await;
    assert!(none_left(uid).await, "the first call's child still runs");
    let mut stand_in = std::process::Command::new("/bin/sleep")
        .arg("300")
        .uid(uid)
        .gid(uid)
        .spawn()
        .unwrap();
    assert_eq!(processes_of(uid), 1, "the stand-in runs as the slot's uid");
    ex.run(req("output = 1", 10)).await.unwrap();
    let t0 = Instant::now();
    let mut status = stand_in.try_wait().unwrap();
    while status.is_none() && t0.elapsed() < Duration::from_secs(5) {
        tokio::time::sleep(Duration::from_millis(50)).await;
        status = stand_in.try_wait().unwrap();
    }
    if status.is_none() {
        // Still running: ended here, so the test leaves nothing behind.
        stand_in.kill().unwrap();
        stand_in.wait().unwrap();
    }
    let signal = status.and_then(|s| s.signal());
    assert_eq!(signal, Some(libc::SIGKILL), "{status:?}");
    assert!(
        none_left(uid).await,
        "a process still runs as the slot's uid"
    );
}

/// Threads still start inside the child: only new processes are refused.
/// `clone3` answers ENOSYS, so a libc that tries it first falls back to
/// `clone`. It is also asked directly: a libc that never tries it would not
/// show a wrong answer.
#[tokio::test]
async fn threads_still_work_inside_the_child() {
    let Some(ex) = executor(1) else { return };
    let code = "import ctypes, errno, threading\n\
         res = []\n\
         t = threading.Thread(target=lambda: res.append(7))\n\
         t.start()\n\
         t.join()\n\
         SYS_clone3 = 435\n\
         libc = ctypes.CDLL(None, use_errno=True)\n\
         rc = libc.syscall(SYS_clone3, None, 0)\n\
         output = [res, rc, errno.errorcode.get(ctypes.get_errno())]";
    let r = ex.run(req(code, 10)).await.unwrap();
    assert_eq!(r.output, Some(serde_json::json!([[7], -1, "ENOSYS"])));
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
    let Some(ex) = executor_with(1, |c| c.max_response_bytes = 1024) else {
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
    // With the syscall filter, `socket()` is refused before any route is
    // tried; the namespace itself is proven by the self-test's
    // `namespace_network` and `network_interfaces`.
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

/// A configured path that is a file is covered like a directory: a call can
/// read the file only while it is not configured.
#[tokio::test]
async fn a_hidden_file_cannot_be_read() {
    let Some(plain) = executor(1) else { return };
    // Outside /tmp, which every child replaces with its own. The name is
    // unique, and the file goes when `temp` drops, also on failure.
    let temp = tempfile::NamedTempFile::new_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let file = temp.path().to_path_buf();
    std::fs::write(&file, "seen").unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
    let code = format!(
        "try:\n    output = open({:?}).read()\nexcept OSError:\n    output = 'unreadable'",
        file.display().to_string()
    );
    let hiding = executor_with(1, |c| c.hide_paths = vec![file.clone()]).unwrap();
    let seen = plain.run(req(&code, 10)).await.unwrap().output;
    let hidden = hiding.run(req(&code, 10)).await.unwrap().output;
    assert_eq!(
        (seen, hidden),
        (
            Some(serde_json::json!("seen")),
            Some(serde_json::json!("unreadable"))
        )
    );
}

/// The child's /proc lists its own processes only: not the template it was
/// forked from, nor this test.
#[tokio::test]
async fn the_child_sees_only_its_own_processes() {
    let Some(ex) = executor(1) else { return };
    let code = format!(
        "import os\n\
         pids = [p for p in os.listdir('/proc') if p.isdigit()]\n\
         output = [pids == [str(os.getpid())], os.path.exists(f'/proc/{{os.getppid()}}'), os.path.exists('/proc/{}')]",
        std::process::id()
    );
    let r = ex.run(req(&code, 10)).await.unwrap();
    assert_eq!(r.output, Some(serde_json::json!([true, false, false])));
}

/// The entries of /proc that describe the machine or let it be driven are
/// covered in the child's fresh /proc, and pandas, numpy and scipy still work
/// there.
#[tokio::test]
async fn the_kernel_entries_of_proc_are_covered() {
    let Some(ex) = executor(1) else { return };
    let code = "import os, stat\n\
         import numpy as np, pandas as pd, scipy.stats\n\
         def shown(p):\n\
         \x20   if not os.path.lexists(p):\n\
         \x20       return False\n\
         \x20   if os.path.isdir(p):\n\
         \x20       return len(os.listdir(p)) > 0\n\
         \x20   return not stat.S_ISCHR(os.stat(p).st_mode)\n\
         names = ['acpi', 'asound', 'bus', 'fs', 'irq', 'kcore', 'keys', 'latency_stats', 'sched_debug', 'scsi', 'sys', 'sysrq-trigger', 'timer_list', 'timer_stats']\n\
         frame = pd.DataFrame({'k': [1, 1, 2], 'v': np.array([1.0, 2.0, 3.0])})\n\
         output = [[n for n in names if shown('/proc/' + n)], frame.groupby('k')['v'].sum().tolist(), float(scipy.stats.norm.cdf(0))]";
    let r = ex.run(req(code, 20)).await.unwrap();
    assert_eq!(r.output, Some(serde_json::json!([[], [3.0, 3.0], 0.5])));
}

/// A path under /dev/mqueue reaches the message queues of the IPC namespace
/// that mounted it, not the child's own: it is covered, so a call cannot
/// create a queue there.
#[tokio::test]
async fn a_call_cannot_create_a_message_queue_outside_its_namespace() {
    let Some(ex) = executor(1) else { return };
    // Precondition: without a real /dev/mqueue here, the child's inability to
    // create an entry there would prove nothing — assert this test's own
    // process can, so a runner missing it can't pass vacuously.
    let mqueue = Path::new("/dev/mqueue");
    assert!(
        mqueue.is_dir(),
        "/dev/mqueue must exist as a directory in the test environment"
    );
    let probe = mqueue.join(format!(
        "colmena-mqueue-precondition-{}",
        std::process::id()
    ));
    std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(&probe)
        .unwrap_or_else(|e| panic!("cannot create an entry under /dev/mqueue here: {e}"));
    std::fs::remove_file(&probe).unwrap();

    let path = format!("/dev/mqueue/colmena-test-{}", std::process::id());
    let code = format!(
        "import os\n\
         try:\n\
         \x20   os.close(os.open({path:?}, os.O_CREAT | os.O_RDWR, 0o600))\n\
         \x20   output = 'created'\n\
         except OSError:\n\
         \x20   output = 'refused'"
    );
    let r = ex.run(req(&code, 10)).await.unwrap().output;
    // Removed from here as well, so a queue the call did create does not stay.
    let left = std::fs::remove_file(&path).is_ok();
    assert_eq!((r, left), (Some(serde_json::json!("refused")), false));
}

/// Runs `python_executor self-test` with `args`: its exit code and checks.
fn self_test(args: &[&str]) -> (Option<i32>, Vec<serde_json::Value>) {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_python_executor"))
        .arg("self-test")
        .args(args)
        .output()
        .unwrap();
    let stdout = String::from_utf8(out.stdout).unwrap();
    let checks = stdout.lines().map(|l| serde_json::from_str(l).unwrap());
    (out.status.code(), checks.collect())
}

/// Every layer reports and holds, a configured file among the hidden paths.
#[test]
fn the_self_test_proves_every_layer() {
    if !jail_tests_enabled() {
        return;
    }
    // Under no directory the jail covers, whose cover would hide the file
    // before its own. The name is unique, and the file goes when `temp` drops.
    let temp = tempfile::Builder::new()
        .prefix("colmena-selftest-")
        .tempfile_in("/opt")
        .unwrap();
    std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o644)).unwrap();
    let file = temp.path().to_str().unwrap();
    let (code, checks) = self_test(&["--uid-base", "50000", "--hide", file]);
    let layers: BTreeSet<&str> = checks.iter().filter_map(|c| c["layer"].as_str()).collect();
    assert_eq!(layers, selftest::LAYERS.iter().copied().collect());
    assert!(checks.iter().all(|c| c["ok"] == true), "{checks:#?}");
    assert_eq!(code, Some(0));
}

/// A layer that cannot be applied fails the self-test: here a slot uid
/// that does not fit, which is refused before anything else changes. With
/// only that one layer reported, the set is short of every other layer the
/// probe would have added, so the report is also incomplete.
#[test]
fn the_self_test_fails_when_a_layer_cannot_be_applied() {
    let base = u32::MAX - selftest::SELF_TEST_SLOT;
    let (code, checks) = self_test(&["--uid-base", &base.to_string()]);
    let refused = serde_json::json!({"layer": "identity", "ok": false, "reason": "not_applied"});
    let incomplete =
        serde_json::json!({"layer": "self_test", "ok": false, "reason": "incomplete_report"});
    assert!(checks.contains(&refused), "{checks:#?}");
    assert!(checks.contains(&incomplete), "{checks:#?}");
    assert_eq!(code, Some(3));
}
