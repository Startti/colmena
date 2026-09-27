//! Warm template: one thread, Python initialized, heavy modules imported once.
//! Each accepted connection gets a fresh process (`os.fork()` from Python so
//! CPython's after-fork hooks run); the template itself never runs user code.

use super::child::{self, JailSpec};
use pyo3::prelude::*;
use serde_json::json;
use std::ffi::CString;
use std::io::{self, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::io::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;

/// The only environment the template (and every child) starts with.
pub const TEMPLATE_ENV: &[(&str, &str)] = &[
    ("PATH", "/usr/local/bin:/usr/bin:/bin"),
    ("OPENBLAS_NUM_THREADS", "1"),
    ("OMP_NUM_THREADS", "1"),
    ("MKL_NUM_THREADS", "1"),
];

const WARM_IMPORTS: &str = "import pandas, numpy, scipy.stats, json, math, re, datetime, collections, itertools, functools, string, decimal, statistics, hmac, hashlib, base64, secrets, io, csv, ast\n";

pub struct ZygoteArgs {
    pub socket: PathBuf,
    pub jail: JailSpec,
}

pub(crate) fn log(event: serde_json::Value) {
    // stderr is read by the executor and forwarded to tracing.
    let _ = writeln!(io::stderr(), "{event}");
}

fn warm_imports() -> Result<(), String> {
    let code = CString::new(WARM_IMPORTS).map_err(|e| e.to_string())?;
    Python::attach(|py| {
        py.run(code.as_c_str(), None, None)
            .map_err(|e| e.to_string())
    })
}

fn python_fork() -> Result<i32, String> {
    Python::attach(|py| -> PyResult<i32> { py.import("os")?.call_method0("fork")?.extract() })
        .map_err(|e| e.to_string())
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

pub fn run(args: ZygoteArgs) -> i32 {
    // Dies with the process that started it. `prctl` is variadic and reads its
    // arguments as `unsigned long`, so they are passed at that width.
    unsafe {
        libc::prctl(
            libc::PR_SET_PDEATHSIG,
            libc::SIGKILL as libc::c_ulong,
            0 as libc::c_ulong,
            0 as libc::c_ulong,
            0 as libc::c_ulong,
        )
    };
    pyo3::Python::initialize();
    if let Err(e) = warm_imports() {
        log(json!({"event": "warm_imports_failed", "error": e}));
        return 3;
    }
    let listener = match UnixListener::bind(&args.socket) {
        Ok(l) => l,
        Err(e) => {
            log(json!({"event": "bind_failed", "error": e.to_string()}));
            return 3;
        }
    };
    let _ = std::fs::set_permissions(&args.socket, std::fs::Permissions::from_mode(0o600));
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
                Err(e) => log(json!({"event": "fork_failed", "error": e})),
            },
            Ok(None) => {}
            Err(e) => log(json!({"event": "accept_failed", "error": e.to_string()})),
        }
    }
}
