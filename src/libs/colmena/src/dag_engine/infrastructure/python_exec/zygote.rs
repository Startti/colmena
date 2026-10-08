//! Warm template: one thread, Python initialized, heavy modules imported once.
//! Each accepted connection gets a fresh process (`os.fork()` from Python so
//! CPython's after-fork hooks run); the template itself never runs user code.

use super::child::{self, JailSpec, EXIT_NOT_READY};
use super::selftest;
use pyo3::exceptions::PyImportError;
use pyo3::prelude::*;
use serde_json::{json, Value};
use std::ffi::CStr;
use std::io::{self, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::io::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The only environment the template (and every child) starts with.
///
/// `JE_ARROW_MALLOC_CONF` is the setting string of the jemalloc that pyarrow's
/// x86_64 wheels bundle, which otherwise starts a `jemalloc_bg_thd` thread the
/// moment `import pyarrow` runs (pandas 1.5.3 imports it when installed): the
/// template would then have two threads and refuse to start, for every call.
/// Without that thread jemalloc purges unused pages from the allocating thread
/// instead of a timer; nothing else changes. The aarch64 wheels have no such
/// thread, and the variable is ignored there and when pyarrow is absent.
pub const TEMPLATE_ENV: &[(&str, &str)] = &[
    ("PATH", "/usr/local/bin:/usr/bin:/bin"),
    ("OPENBLAS_NUM_THREADS", "1"),
    ("OMP_NUM_THREADS", "1"),
    ("MKL_NUM_THREADS", "1"),
    ("JE_ARROW_MALLOC_CONF", "background_thread:false"),
];

/// Added to the template's environment, and so to every call's, only when the
/// executor stages prepared data: pyarrow's default allocator reserves about a
/// gibibyte of address space under `RLIMIT_AS`, so its system pool is required,
/// and one I/O thread keeps the template able to fork.
pub const ARROW_ENV: &[(&str, &str)] = &[
    ("ARROW_DEFAULT_MEMORY_POOL", "system"),
    ("ARROW_IO_THREADS", "1"),
];

const WARM_IMPORTS: &CStr = c"import pandas, numpy, scipy.stats, json, math, re, datetime, collections, itertools, functools, string, decimal, statistics, hmac, hashlib, base64, secrets, io, csv, ast\n";

pub struct ZygoteArgs {
    pub socket: PathBuf,
    pub jail: JailSpec,
}

pub(crate) fn log(event: Value) {
    // stderr is read by the executor and forwarded to tracing.
    let _ = writeln!(io::stderr(), "{event}");
}

/// A Python exception's type name, never its message.
fn error_type(py: Python<'_>, e: &PyErr) -> Option<String> {
    e.get_type(py).name().ok().map(|n| n.to_string())
}

fn warm_imports() -> Result<(), Value> {
    Python::attach(|py| {
        py.run(WARM_IMPORTS, None, None).map_err(|e| {
            let module = e
                .is_instance_of::<PyImportError>(py)
                .then(|| e.value(py).getattr("name").ok()?.extract::<String>().ok())
                .flatten();
            json!({"event": "warm_imports_failed", "error_type": error_type(py, &e), "module": module})
        })
    })
}

fn python_fork() -> Result<i32, Value> {
    Python::attach(|py| {
        let forked = py.import("os").and_then(|os| os.call_method0("fork"));
        forked.and_then(|pid| pid.extract()).map_err(|e| {
            let errno = e.value(py).getattr("errno").ok();
            let errno = errno.and_then(|n| n.extract::<i32>().ok());
            json!({"event": "fork_failed", "error_type": error_type(py, &e), "errno": errno})
        })
    })
}

/// Asks the kernel to kill the template when its parent goes. The signal
/// follows the parent THREAD that started this process, not the parent
/// process: the host must start the template from a dedicated thread that
/// lives as long as the template should, never from a pooled or short-lived one.
fn die_with_parent() -> Result<(), Value> {
    let parent = unsafe { libc::getppid() };
    // `prctl` is variadic and reads its arguments as `unsigned long`, so they
    // are passed at that width.
    let rc = unsafe {
        libc::prctl(
            libc::PR_SET_PDEATHSIG,
            libc::SIGKILL as libc::c_ulong,
            0 as libc::c_ulong,
            0 as libc::c_ulong,
            0 as libc::c_ulong,
        )
    };
    if rc != 0 {
        let errno = io::Error::last_os_error().raw_os_error();
        return Err(json!({"event": "pdeathsig_failed", "errno": errno}));
    }
    // A parent that exited before `prctl` took effect sends no signal: this
    // process has already been re-parented.
    if unsafe { libc::getppid() } != parent {
        return Err(json!({"event": "parent_exited"}));
    }
    Ok(())
}

/// Makes the template the parent of whatever a child leaves behind when it
/// ends, so `reap_children` collects those processes too.
fn adopt_orphans() -> Result<(), Value> {
    let (on, zero): (libc::c_ulong, libc::c_ulong) = (1, 0);
    if unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, on, zero, zero, zero) } != 0 {
        let errno = io::Error::last_os_error().raw_os_error();
        return Err(json!({"event": "subreaper_failed", "errno": errno}));
    }
    Ok(())
}

/// With pyarrow LOADED (pandas loads it in the warm imports when it is
/// installed), its default memory pool must be the system one: a template that
/// would let calls reserve a gibibyte of address space each cannot offer mounts.
/// pyarrow not loaded is fine. Never imports it.
pub(crate) fn check_arrow(py: Python<'_>) -> Result<(), Value> {
    // Only a pyarrow that is already loaded (pandas imports it when it is
    // installed): importing it here could start a thread or crash the template.
    let loaded = py.import("sys").and_then(|sys| sys.getattr("modules"));
    let Some(arrow) = loaded.ok().and_then(|m| m.get_item("pyarrow").ok()) else {
        return Ok(());
    };
    let backend = arrow
        .call_method0("default_memory_pool")
        .and_then(|pool| pool.getattr("backend_name"))
        .and_then(|name| name.extract::<String>());
    match backend.as_deref() {
        Ok("system") => Ok(()),
        Ok(other) => Err(json!({"event": "arrow_pool_not_system", "backend": other})),
        Err(_) => Err(json!({"event": "arrow_pool_unreadable"})),
    }
}

/// `os.fork()` copies only the calling thread, so it is safe only while the
/// template has one: a second thread (a BLAS pool, say) could hold a lock the
/// children inherit held. Checked after the imports, which could start one.
fn check_single_threaded() -> Result<(), Value> {
    match std::fs::read_dir("/proc/self/task").map(Iterator::count) {
        Ok(1) => Ok(()),
        Ok(n) => Err(json!({"event": "template_not_single_threaded", "threads": n})),
        Err(e) => Err(json!({"event": "thread_count_failed", "errno": e.raw_os_error()})),
    }
}

/// Creates the socket owner-only. `umask` is process-wide, which is safe here
/// because the template is single-threaded; the previous mask is restored so
/// the children inherit the one the template started with.
fn bind_private(socket: &Path) -> Result<UnixListener, Value> {
    let previous = unsafe { libc::umask(0o077) };
    let bound = UnixListener::bind(socket);
    unsafe { libc::umask(previous) };
    let listener = bound.map_err(|e| json!({"event": "bind_failed", "errno": e.raw_os_error()}))?;
    std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| json!({"event": "socket_permissions_failed", "errno": e.raw_os_error()}))?;
    Ok(listener)
}

fn reap_children() {
    loop {
        let mut status = 0;
        let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
        let pid = unsafe { libc::wait4(-1, &mut status, libc::WNOHANG, &mut usage) };
        if pid <= 0 {
            return;
        }
        let (exit_code, signal) = if libc::WIFEXITED(status) {
            (Some(libc::WEXITSTATUS(status)), None)
        } else if libc::WIFSIGNALED(status) {
            (None, Some(libc::WTERMSIG(status)))
        } else {
            (None, None)
        };
        log(json!({
            "event": "child_exit",
            "pid": pid,
            "exit_code": exit_code,
            "signal": signal,
            "max_rss_kb": usage.ru_maxrss,
        }));
    }
}

fn accept_with_timeout(l: &UnixListener, ms: i32) -> io::Result<Option<UnixStream>> {
    let mut pfd = libc::pollfd {
        fd: l.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    let n = unsafe { libc::poll(&mut pfd, 1, ms) };
    if n < 0 {
        let e = io::Error::last_os_error();
        return if e.kind() == io::ErrorKind::Interrupted {
            Ok(None)
        } else {
            Err(e)
        };
    }
    if n == 0 {
        return Ok(None);
    }
    let (s, _) = l.accept()?;
    s.set_nonblocking(false)?;
    Ok(Some(s))
}

/// Proves the jail in a throwaway child before any call is accepted. Every
/// layer that did not hold is logged, the last one as the returned event.
fn prove_jail(jail: &JailSpec) -> Result<(), Value> {
    let Err(checks) = selftest::run(jail) else {
        return Ok(());
    };
    let event = |c: &selftest::LayerCheck| json!({"event": "self_test_failed", "layer": c.layer, "reason": c.reason, "errno": c.errno});
    let mut failed: Vec<Value> = checks.iter().filter(|c| !c.ok).map(event).collect();
    let last = failed.pop();
    failed.into_iter().for_each(log);
    Err(last.unwrap_or_else(|| json!({"event": "self_test_failed"})))
}

fn start(args: &ZygoteArgs) -> Result<(UnixListener, Option<&'static str>), Value> {
    die_with_parent()?;
    adopt_orphans()?;
    pyo3::Python::initialize();
    warm_imports()?;
    check_single_threaded()?;
    prove_jail(&args.jail)?;
    let mounts_off = prove_mounts(&args.jail);
    let listener = bind_private(&args.socket)?;
    Ok((listener, mounts_off))
}

/// Whether this template can offer run mounts, for a template with a staging
/// root: `None` when it can, else a fixed reason code. A failure here disables
/// that capability, loudly, and nothing else: the executor keeps serving plain
/// calls. (A broken jail, by contrast, stops the template: see [`prove_jail`].)
fn prove_mounts(jail: &JailSpec) -> Option<&'static str> {
    jail.staging_root.as_ref()?;
    if let Err(event) = Python::attach(check_arrow) {
        let reason = match event["event"].as_str() {
            Some("arrow_pool_unreadable") => "arrow_pool_unreadable",
            _ => "arrow_pool_not_system",
        };
        log(event);
        return Some(reason);
    }
    let Err(checks) = selftest::run_mounts(jail) else {
        return None;
    };
    let failed: Vec<&selftest::LayerCheck> = checks.iter().filter(|c| !c.ok).collect();
    for c in &failed {
        log(
            json!({"event": "self_test_failed", "layer": c.layer, "reason": c.reason, "errno": c.errno}),
        );
    }
    let staging = failed
        .iter()
        .any(|c| c.layer == "self_test" && c.reason == "staging_failed");
    Some(if staging {
        "staging_unusable"
    } else {
        "mount_layer_failed"
    })
}

pub fn run(args: ZygoteArgs) -> i32 {
    let (listener, mounts_off) = match start(&args) {
        Ok(started) => started,
        Err(event) => {
            log(event);
            return EXIT_NOT_READY;
        }
    };
    if let Some(reason) = mounts_off {
        log(json!({"event": "mounts_disabled", "reason": reason}));
        // Read by the executor before READY; a fixed code, never free text.
        println!("MOUNTS_DISABLED {reason}");
    }
    println!("READY");
    let _ = io::stdout().flush();
    loop {
        reap_children();
        match accept_with_timeout(&listener, 200) {
            Ok(Some(conn)) => match python_fork() {
                Ok(0) => {
                    drop(listener);
                    child::serve_forked(conn, &args.jail)
                }
                Ok(_) => drop(conn),
                Err(event) => log(event),
            },
            Ok(None) => {}
            Err(e) => {
                log(json!({"event": "accept_failed", "errno": e.raw_os_error()}));
                // Pause so a lasting `poll`/`accept` failure does not spin.
                std::thread::sleep(Duration::from_millis(200));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stand-in `pyarrow` under the name the check looks up, so no real one is needed.
    fn with_fake_pyarrow(backend: Option<&str>, body: impl FnOnce(Python<'_>)) {
        pyo3::Python::initialize();
        Python::attach(|py| {
            let sys_modules = py.import("sys").unwrap().getattr("modules").unwrap();
            let saved = sys_modules.get_item("pyarrow").ok();
            if let Some(backend) = backend {
                let code = format!(
                    "import types\nm = types.ModuleType('pyarrow')\nm.default_memory_pool = lambda: types.SimpleNamespace(backend_name={backend:?})\n"
                );
                let ns = pyo3::types::PyDict::new(py);
                py.run(&std::ffi::CString::new(code).unwrap(), Some(&ns), None)
                    .unwrap();
                sys_modules
                    .set_item("pyarrow", ns.get_item("m").unwrap())
                    .unwrap();
            }
            body(py);
            match saved {
                Some(m) => sys_modules.set_item("pyarrow", m).unwrap(),
                None => {
                    let _ = sys_modules.del_item("pyarrow");
                }
            }
        });
    }

    /// The template refuses to start when pyarrow is loaded and its default
    /// memory pool is anything but the system one (the default pool reserves
    /// about a gibibyte of address space under `RLIMIT_AS`).
    #[test]
    fn a_loaded_pyarrow_must_use_the_system_pool() {
        with_fake_pyarrow(Some("system"), |py| assert!(check_arrow(py).is_ok()));
        for backend in ["mimalloc", "jemalloc", ""] {
            with_fake_pyarrow(Some(backend), |py| {
                let e = check_arrow(py).unwrap_err();
                assert_eq!(e["event"], "arrow_pool_not_system", "{backend}");
                assert_eq!(e["backend"], backend);
            });
        }
    }

    #[test]
    fn pyarrow_is_never_imported_just_to_be_checked() {
        pyo3::Python::initialize();
        Python::attach(|py| {
            let sys = py.import("sys").unwrap();
            let modules = sys.getattr("modules").unwrap();
            let saved = modules.get_item("pyarrow").ok();
            let _ = modules.del_item("pyarrow");
            let ns = pyo3::types::PyDict::new(py);
            let hook = c"class Hook:\n    asked = []\n    def find_spec(self, name, path=None, target=None):\n        if name == 'pyarrow':\n            Hook.asked.append(name)\n        return None\nhook = Hook()\nimport sys\nsys.meta_path.insert(0, hook)\n";
            py.run(hook, Some(&ns), None).unwrap();
            let result = check_arrow(py);
            py.run(c"import sys\nsys.meta_path.remove(hook)\n", Some(&ns), None)
                .unwrap();
            let asked: usize = py
                .eval(c"len(Hook.asked)", Some(&ns), None)
                .unwrap()
                .extract()
                .unwrap();
            if let Some(m) = saved {
                modules.set_item("pyarrow", m).unwrap();
            }
            assert!(result.is_ok());
            assert_eq!(asked, 0, "pyarrow was imported to be checked");
        });
    }

    /// What the template's warm imports do to its thread count, in a fresh Python
    /// with exactly the template's environment (plus the staged executor's Arrow
    /// variables, the larger of the two): pyarrow's x86_64 wheels bundle a jemalloc
    /// that starts `jemalloc_bg_thd` during `import pyarrow` unless
    /// `TEMPLATE_ENV` turns it off, and a second thread makes the template refuse
    /// to start. This runs where the failure shows (x86_64 with pyarrow installed);
    /// where pyarrow is missing it is skipped unless the environment says it is
    /// expected (`COLMENA_PYEXEC_EXPECT_PYARROW=1`).
    #[test]
    #[cfg(target_os = "linux")]
    fn importing_pyarrow_leaves_the_template_environment_single_threaded() {
        let code = "import sys\n\
            def threads():\n\
            \treturn int([l for l in open('/proc/self/status') if l.startswith('Threads:')][0].split()[1])\n\
            try:\n\
            \timport pyarrow\n\
            except ImportError:\n\
            \tprint('absent'); sys.exit(0)\n\
            import pandas, numpy, scipy.stats\n\
            print(threads())";
        let out = std::process::Command::new("python3")
            .env_clear()
            .envs(TEMPLATE_ENV.iter().chain(ARROW_ENV).copied())
            .args(["-c", code])
            .output()
            .expect("python3 runs");
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        if text.trim() == "absent" {
            let expected = std::env::var("COLMENA_PYEXEC_EXPECT_PYARROW").as_deref() == Ok("1");
            assert!(!expected, "pyarrow is expected here but is not installed");
            eprintln!("skipped: pyarrow is not installed");
            return;
        }
        assert_eq!(text.trim(), "1", "the warm imports started a thread");
    }

    /// The setting that keeps that thread away is part of the fixed environment
    /// every template and call gets, with or without a staging root.
    #[test]
    fn the_fixed_environment_turns_off_the_arrow_jemalloc_thread() {
        assert!(TEMPLATE_ENV.contains(&("JE_ARROW_MALLOC_CONF", "background_thread:false")));
        assert!(!ARROW_ENV.iter().any(|(k, _)| *k == "JE_ARROW_MALLOC_CONF"));
    }

    /// Only a forked child is single-threaded under the multi-threaded test
    /// harness: there one thread passes the check and a second is reported.
    #[test]
    fn the_check_counts_the_threads_of_the_process() {
        match unsafe { libc::fork() } {
            0 => {
                unsafe { libc::alarm(10) }; // a hang after fork fails instead of blocking waitpid
                let ok = std::panic::catch_unwind(|| {
                    let one = check_single_threaded().is_ok();
                    let (_keep, wait) = std::sync::mpsc::channel::<()>();
                    let _waiter = std::thread::spawn(move || wait.recv());
                    one && check_single_threaded().is_err_and(|e| e["threads"] == 2)
                });
                unsafe { libc::_exit(i32::from(!ok.unwrap_or(false))) }
            }
            pid => {
                let mut status = 0;
                assert_eq!(unsafe { libc::waitpid(pid, &mut status, 0) }, pid);
                assert!(libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0);
            }
        }
    }
}
