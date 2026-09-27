//! Supervises the warm template process (`python_executor zygote`): starts it
//! with the environment `TEMPLATE_ENV` plus the host's `LANG`, `LC_ALL` and
//! `LC_CTYPE`, replaces it once it has exited and forwards its known stderr
//! events. The template is started from a thread this executor owns, because
//! it is killed when the thread that started it exits; that thread lives as
//! long as the executor. Each call runs in a fresh child the template forks
//! for it (see [`SubprocessExecutor::run_raw`]).

use super::child::{CallHeader, EXIT_NOT_READY, MAX_HEADER_BYTES};
use super::config::{ExecutorConfigError, SubprocessConfig};
use super::frame;
use super::protocol::{
    input_too_large_message, result_too_large_message, WireRequest, WireResponse, CRASHED_MESSAGE,
    MALFORMED_MESSAGE,
};
use super::zygote::TEMPLATE_ENV;
use crate::dag_engine::domain::python_executor::{
    ExecutorKind, PythonExecutor, PythonRunError, PythonRunRequest, PythonRunResult,
};
use crate::dag_engine::log_policy::T_PYTHON_EXEC;
use async_trait::async_trait;
use serde_json::{Map, Value};
use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read};
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::process::ExitStatusExt;
use std::path::PathBuf;
use std::process::{Child, ChildStderr, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::AsyncReadExt;
use tokio::net::UnixStream;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

const READY_TIMEOUT: Duration = Duration::from_secs(120);
const RESPAWN_BACKOFF: Duration = Duration::from_secs(5);
const PID_TIMEOUT: Duration = Duration::from_secs(10);
const EXIT_GRACE: Duration = Duration::from_secs(2);
/// Longest template stderr line that is parsed; a longer one is dropped.
const MAX_STDERR_LINE: u64 = 1024;
const SUPERVISOR_GONE: &str = "PythonExecutorError: the Python template supervisor stopped";

/// The events the template writes on stderr and the keys they may carry.
const EVENTS: &[&str] = &[
    "child_exit",
    "warm_imports_failed",
    "fork_failed",
    "pdeathsig_failed",
    "parent_exited",
    "template_not_single_threaded",
    "thread_count_failed",
    "bind_failed",
    "socket_permissions_failed",
    "accept_failed",
];
const NUMBER_KEYS: &[&str] = &[
    "pid",
    "exit_code",
    "signal",
    "max_rss_kb",
    "errno",
    "threads",
];
const NAME_KEYS: &[&str] = &["error_type", "module"];
/// Host variables the template gets on top of `TEMPLATE_ENV`, so text
/// encodings match the in-process interpreter. CPython takes only `LC_CTYPE`
/// from the environment at startup, which `LC_ALL` overrides and `LANG` backs.
const LOCALE_VARS: &[&str] = &["LANG", "LC_ALL", "LC_CTYPE"];

type Job = Box<dyn FnOnce() + Send>;

#[derive(Debug)]
pub enum RawFailure {
    Timeout,
    Crashed,
    RequestTooLarge,
    ResponseTooLarge,
    Unavailable(String),
}

impl RawFailure {
    pub fn into_run_error(self, cfg: &SubprocessConfig) -> PythonRunError {
        match self {
            RawFailure::Timeout => PythonRunError::Timeout,
            RawFailure::Crashed => PythonRunError::Python(CRASHED_MESSAGE.to_string()),
            RawFailure::RequestTooLarge => {
                PythonRunError::Python(input_too_large_message(cfg.max_request_bytes))
            }
            RawFailure::ResponseTooLarge => {
                PythonRunError::Python(result_too_large_message(cfg.max_response_bytes))
            }
            RawFailure::Unavailable(m) => PythonRunError::Internal(m),
        }
    }
}

struct Template {
    child: Child,
    socket: PathBuf,
    dir: PathBuf,
}

impl Drop for Template {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

enum State {
    NotStarted,
    Running(Template),
    Failed { at: Instant, reason: String },
}

pub struct SubprocessExecutor {
    cfg: SubprocessConfig,
    max_timeout: Duration,
    state: tokio::sync::Mutex<State>,
    permits: Arc<Semaphore>,
    free_slots: Arc<Mutex<Vec<u32>>>,
    /// Runs every template start on the thread the template's life is tied to.
    spawner: mpsc::Sender<Job>,
    /// Template stderr lines that were not a known event; never logged.
    stderr_dropped: Arc<AtomicU64>,
}

/// Returns its index to the pool when dropped. A call holds it until its
/// child is gone (see [`CallChild`]).
struct Slot {
    index: u32,
    pool: Arc<Mutex<Vec<u32>>>,
    _permit: OwnedSemaphorePermit,
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.pool.lock().unwrap().push(self.index);
    }
}

/// A call's child once its pid is known, with the call's connection and slot.
/// Unless the call saw the child close its end, dropping this hands all three
/// to [`reap`]: when the child is still there `EXIT_GRACE` after the call, or
/// when the call is abandoned midway (its future dropped).
struct CallChild {
    pid: libc::pid_t,
    live: Option<(UnixStream, Slot)>,
}

impl Drop for CallChild {
    fn drop(&mut self) {
        if let Some((conn, slot)) = self.live.take() {
            reap(self.pid, conn, slot);
        }
    }
}

fn kill(pid: libc::pid_t) {
    // SAFETY: a signal to one process; `run_raw` refuses a pid <= 1.
    unsafe { libc::kill(pid, libc::SIGKILL) };
}

/// Reads until the child closes its end (EOF) or the read fails.
async fn drain(conn: &mut UnixStream) {
    let mut sink = [0u8; 64];
    while conn.read(&mut sink).await.is_ok_and(|n| n > 0) {}
}

/// Kills a child its call no longer waits for, and returns the slot only once
/// the child has closed its end or `EXIT_GRACE` has passed since the kill.
/// Exception: if the runtime is shutting down, the waiting task is dropped and
/// the slot comes back right after the kill.
fn reap(pid: libc::pid_t, mut conn: UnixStream, slot: Slot) {
    kill(pid);
    let Ok(rt) = tokio::runtime::Handle::try_current() else {
        // Dropped outside a runtime: a thread waits out the grace period.
        let wait = move || {
            std::thread::sleep(EXIT_GRACE);
            drop(slot);
        };
        let _ = std::thread::Builder::new().spawn(wait);
        return;
    };
    rt.spawn(async move {
        let _ = tokio::time::timeout(EXIT_GRACE, drain(&mut conn)).await;
        drop(slot);
    });
}

impl SubprocessExecutor {
    pub fn new(cfg: SubprocessConfig, max_timeout: Duration) -> Result<Self, ExecutorConfigError> {
        if !cfg.bin.is_file() {
            return Err(ExecutorConfigError(format!(
                "the Python executor binary was not found at {}",
                cfg.bin.display()
            )));
        }
        let (spawner, jobs) = mpsc::channel::<Job>();
        // Ends when the executor drops `spawner`; the template goes with it.
        std::thread::Builder::new()
            .name("python-template".into())
            .spawn(move || jobs.into_iter().for_each(|job| job()))
            .map_err(|e| {
                ExecutorConfigError(format!("cannot start the Python template thread: {e}"))
            })?;
        Ok(Self {
            permits: Arc::new(Semaphore::new(cfg.slots)),
            free_slots: Arc::new(Mutex::new((0..cfg.slots as u32).rev().collect())),
            cfg,
            max_timeout,
            state: tokio::sync::Mutex::new(State::NotStarted),
            spawner,
            stderr_dropped: Arc::default(),
        })
    }

    /// Start the template now so the first call does not pay for the imports.
    pub async fn warm(&self) -> Result<(), String> {
        self.socket_path().await.map(|_| ())
    }

    /// The socket of a running template, starting one when there is none. A
    /// template that exited is replaced at once; after a failed start the next
    /// attempt waits `RESPAWN_BACKOFF`.
    async fn socket_path(&self) -> Result<PathBuf, String> {
        let mut st = self.state.lock().await;
        match &mut *st {
            State::Running(t) => {
                let status = t.child.try_wait();
                if let Ok(None) = status {
                    return Ok(t.socket.clone());
                }
                let status = status.ok().flatten();
                tracing::warn!(
                    target: T_PYTHON_EXEC,
                    exit_code = status.and_then(|s| s.code()),
                    signal = status.and_then(|s| s.signal()),
                    "python template exited"
                );
            }
            State::Failed { at, reason } if at.elapsed() < RESPAWN_BACKOFF => {
                return Err(reason.clone())
            }
            _ => {}
        }
        match self.start().await {
            Ok(t) => {
                let socket = t.socket.clone();
                *st = State::Running(t);
                Ok(socket)
            }
            Err(reason) => {
                tracing::warn!(target: T_PYTHON_EXEC, reason = %reason, "python template did not start");
                *st = State::Failed {
                    at: Instant::now(),
                    reason: reason.clone(),
                };
                Err(reason)
            }
        }
    }

    async fn start(&self) -> Result<Template, String> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let (cfg, dropped) = (self.cfg.clone(), self.stderr_dropped.clone());
        let job: Job = Box::new(move || {
            // A start whose caller stopped waiting still runs until READY (or
            // `READY_TIMEOUT`); a template it returns is then dropped here,
            // which stops it.
            let _ = tx.send(start_template(&cfg, dropped));
        });
        self.spawner
            .send(job)
            .map_err(|_| SUPERVISOR_GONE.to_string())?;
        rx.await.map_err(|_| SUPERVISOR_GONE.to_string())?
    }

    async fn take_slot(&self) -> Result<Slot, RawFailure> {
        let permit =
            self.permits.clone().acquire_owned().await.map_err(|_| {
                RawFailure::Unavailable("PythonExecutorError: executor closed".into())
            })?;
        let index = self.free_slots.lock().unwrap().pop();
        Ok(Slot {
            index: index.expect("a permit always has a free slot"),
            pool: self.free_slots.clone(),
            _permit: permit,
        })
    }

    /// One call with an already-encoded request. A request over the limit is
    /// refused here: the child could only answer it to a host whose write of
    /// the whole request succeeded.
    pub async fn run_raw(&self, timeout: Duration, request: &[u8]) -> Result<Vec<u8>, RawFailure> {
        if request.len() > self.cfg.max_request_bytes {
            return Err(RawFailure::RequestTooLarge);
        }
        let slot = self.take_slot().await?;
        let socket = self.socket_path().await.map_err(RawFailure::Unavailable)?;
        let mut conn = UnixStream::connect(&socket).await.map_err(|e| {
            RawFailure::Unavailable(format!(
                "PythonExecutorError: cannot reach the Python template process: {e}"
            ))
        })?;
        let mut pid = [0u8; 4];
        let read = tokio::time::timeout(PID_TIMEOUT, conn.read_exact(&mut pid)).await;
        // A pid of 0 or -1 would signal a whole group of processes.
        let pid = libc::pid_t::try_from(u32::from_be_bytes(pid)).unwrap_or(0);
        if !matches!(read, Ok(Ok(_))) || pid <= 1 {
            return Err(RawFailure::Unavailable(
                "PythonExecutorError: the Python process did not start".into(),
            ));
        }
        let header = CallHeader {
            slot: slot.index,
            memory_mb: self.cfg.memory_mb,
            cpu_secs: timeout.as_secs().saturating_add(1),
            max_request_bytes: self.cfg.max_request_bytes,
        };
        let header = serde_json::to_vec(&header).expect("header serializes");
        debug_assert!(header.len() <= MAX_HEADER_BYTES);
        let mut child = CallChild {
            pid,
            live: Some((conn, slot)),
        };
        let (conn, _) = child.live.as_mut().expect("the call holds its connection");
        let max_response = self.cfg.max_response_bytes;
        let exchange = async {
            frame::write_frame_async(conn, &header).await?;
            frame::write_frame_async(conn, request).await?;
            frame::read_frame_async(conn, max_response).await
        };
        let result = match tokio::time::timeout(timeout, exchange).await {
            Ok(Ok(bytes)) => Ok(bytes),
            Err(_) => Err(RawFailure::Timeout),
            Ok(Err(e)) if frame::is_frame_too_large(&e) => Err(RawFailure::ResponseTooLarge),
            Ok(Err(_)) => Err(RawFailure::Crashed),
        };
        if result.is_err() {
            kill(pid);
        }
        // The slot is free again once the child has closed its end; a child
        // that has not by `EXIT_GRACE` is left to `reap` when `child` drops.
        if tokio::time::timeout(EXIT_GRACE, drain(conn)).await.is_ok() {
            child.live = None;
        }
        result
    }
}

#[async_trait]
impl PythonExecutor for SubprocessExecutor {
    fn kind(&self) -> ExecutorKind {
        ExecutorKind::Subprocess
    }

    async fn run(&self, req: PythonRunRequest) -> Result<PythonRunResult, PythonRunError> {
        let timeout = req.timeout.unwrap_or(self.max_timeout);
        let bytes = serde_json::to_vec(&WireRequest::new(req, timeout)).map_err(|e| {
            PythonRunError::Internal(format!(
                "PythonExecutorError: cannot encode the request: {e}"
            ))
        })?;
        match self.run_raw(timeout, &bytes).await {
            Ok(resp) => serde_json::from_slice::<WireResponse>(&resp)
                .map_err(|_| PythonRunError::Python(MALFORMED_MESSAGE.to_string()))?
                .into_result(),
            Err(f) => Err(f.into_run_error(&self.cfg)),
        }
    }
}

/// Blocking. Runs on the executor's own thread (see [`SubprocessExecutor::new`]).
fn start_template(cfg: &SubprocessConfig, dropped: Arc<AtomicU64>) -> Result<Template, String> {
    let root = unsafe { libc::geteuid() } == 0;
    let base = if root {
        PathBuf::from("/run")
    } else {
        std::env::temp_dir()
    };
    let dir = base.join(format!("colmena-python-executor-{}", uuid::Uuid::new_v4()));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&dir)
        .map_err(|e| format!("PythonExecutorError: cannot create the socket directory: {e}"))?;
    let socket = dir.join("zygote.sock");
    let mut cmd = Command::new(&cfg.bin);
    cmd.arg("zygote").arg("--socket").arg(&socket);
    cmd.arg("--uid-base").arg(cfg.uid_base.to_string());
    cmd.arg("--tmp-mb").arg(cfg.tmp_mb.to_string());
    for p in &cfg.hide_paths {
        cmd.arg("--hide").arg(p);
    }
    cmd.env_clear().envs(template_env(std::env::vars_os()));
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = cmd.spawn().map_err(|e| {
        let _ = std::fs::remove_dir_all(&dir);
        format!("PythonExecutorError: cannot start the Python template process: {e}")
    })?;
    let mut t = Template { child, socket, dir };
    let stderr = t.child.stderr.take().expect("stderr is piped");
    let stdout = t.child.stdout.take().expect("stdout is piped");
    let (ready_tx, ready) = mpsc::channel();
    let named = |name: &str| std::thread::Builder::new().name(name.into());
    named("python-template-stderr")
        .spawn(move || forward_stderr(stderr, &dropped))
        .and_then(|_| {
            named("python-template-stdout").spawn(move || {
                let mut out = BufReader::new(stdout);
                let mut first = String::new();
                let read = out.by_ref().take(64).read_line(&mut first);
                let _ = ready_tx.send(read.is_ok() && first.trim_end() == "READY");
                // Children inherit this pipe: keep it drained so none blocks on it.
                let _ = std::io::copy(&mut out, &mut std::io::sink());
            })
        })
        .map_err(|e| {
            format!("PythonExecutorError: cannot read the Python template process: {e}")
        })?;
    if ready.recv_timeout(READY_TIMEOUT).unwrap_or(false) {
        tracing::info!(target: T_PYTHON_EXEC, pid = t.child.id(), "python template ready");
        return Ok(t);
    }
    let _ = t.child.kill();
    Err(match t.child.wait().ok().and_then(|s| s.code()) {
        // Its stderr event says which startup check failed.
        Some(EXIT_NOT_READY) => "PythonExecutorError: the Python template process could not start",
        _ => "PythonExecutorError: the Python template process did not become ready",
    }
    .to_string())
}

/// `TEMPLATE_ENV` plus the host's `LOCALE_VARS`; nothing else of `host`.
fn template_env(host: impl Iterator<Item = (OsString, OsString)>) -> Vec<(OsString, OsString)> {
    let fixed = TEMPLATE_ENV.iter().map(|&(k, v)| (k.into(), v.into()));
    let locale = host.filter(|(k, _)| LOCALE_VARS.iter().any(|l| k.as_os_str() == *l));
    fixed.chain(locale).collect()
}

/// Logs each known template event from stderr. The children share that stream
/// for now, so any other line is only counted, never logged.
fn forward_stderr(stderr: ChildStderr, dropped: &AtomicU64) {
    let mut r = BufReader::new(stderr);
    let mut line = Vec::new();
    loop {
        line.clear();
        match r
            .by_ref()
            .take(MAX_STDERR_LINE)
            .read_until(b'\n', &mut line)
        {
            Ok(0) | Err(_) => return,
            Ok(n) if n as u64 == MAX_STDERR_LINE && !line.ends_with(b"\n") => {
                let _ = r.skip_until(b'\n');
            }
            Ok(_) => {}
        }
        let Some(event) = template_event(&line) else {
            let n = dropped.fetch_add(1, Ordering::Relaxed) + 1;
            if n.is_power_of_two() {
                tracing::warn!(target: T_PYTHON_EXEC, stderr_dropped = n, "python template stderr lines dropped");
            }
            continue;
        };
        let child_exit = event["event"] == "child_exit";
        let fields = Value::Object(event);
        if child_exit {
            tracing::debug!(target: T_PYTHON_EXEC, fields = %fields, "python template event");
        } else {
            tracing::warn!(target: T_PYTHON_EXEC, fields = %fields, "python template event");
        }
    }
}

/// `line` when it is a known event: a JSON object with a known `event` whose
/// other keys are all known and carry numbers, nulls or short names only.
fn template_event(line: &[u8]) -> Option<Map<String, Value>> {
    let Ok(Value::Object(event)) = serde_json::from_slice(line) else {
        return None;
    };
    let name = |v: &Value| {
        v.as_str().is_some_and(|s| {
            (1..=64).contains(&s.len())
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.')
        })
    };
    let known = event.iter().all(|(k, v)| match k.as_str() {
        "event" => v.as_str().is_some_and(|e| EVENTS.contains(&e)),
        k if NUMBER_KEYS.contains(&k) => v.is_null() || v.is_i64() || v.is_u64(),
        k if NAME_KEYS.contains(&k) => v.is_null() || name(v),
        _ => false,
    });
    (known && event.contains_key("event")).then_some(event)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_known_events_with_known_fields_pass() {
        let exit = br#"{"event":"child_exit","pid":7,"exit_code":null,"signal":9,"max_rss_kb":1}"#;
        assert!(template_event(exit).is_some());
        let import =
            br#"{"event":"warm_imports_failed","error_type":"ImportError","module":"scipy.stats"}"#;
        assert!(template_event(import).is_some());
        for line in [
            &b"Traceback (most recent call last):"[..],
            br#"{"event":"child_exit","pid":7,"note":"x"}"#,
            br#"{"event":"something_else"}"#,
            br#"{"event":"child_exit","pid":"7"}"#,
            br#"{"event":"fork_failed","error_type":"two words"}"#,
            br#"{"pid":7}"#,
            br#"["child_exit"]"#,
        ] {
            assert!(
                template_event(line).is_none(),
                "{}",
                String::from_utf8_lossy(line)
            );
        }
    }

    /// A shell script in place of the binary that counts its own starts.
    fn fake(body: &str) -> (tempfile::TempDir, SubprocessExecutor) {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("template");
        let starts = dir.path().join("starts").display().to_string();
        std::fs::write(&bin, format!("#!/bin/sh\necho >> {starts}\n{body}\n")).unwrap();
        std::fs::set_permissions(&bin, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        let mut cfg = SubprocessConfig::from_lookup(&|_: &str| None::<String>).unwrap();
        cfg.bin = bin;
        (
            dir,
            SubprocessExecutor::new(cfg, Duration::from_secs(60)).unwrap(),
        )
    }

    fn starts(dir: &tempfile::TempDir) -> usize {
        let log = std::fs::read_to_string(dir.path().join("starts"));
        log.map(|s| s.lines().count()).unwrap_or(0)
    }

    #[tokio::test]
    async fn a_template_that_cannot_start_is_not_retried_before_the_backoff() {
        let event =
            r#"{"event":"warm_imports_failed","error_type":"ImportError","module":"pandas"}"#;
        let (dir, ex) = fake(&format!("echo '{event}' >&2\necho 'free text' >&2\nexit 3"));
        let e = ex.warm().await.unwrap_err();
        assert!(
            e.starts_with("PythonExecutorError:") && e.contains("could not start"),
            "{e}"
        );
        assert_eq!(ex.warm().await.unwrap_err(), e);
        assert_eq!(starts(&dir), 1);
        // The template has exited, so its two lines are read by now; only the
        // free-text one is dropped.
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(ex.stderr_dropped.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn a_request_over_the_limit_is_refused_before_the_template_starts() {
        let (dir, mut ex) = fake("exit 1");
        ex.cfg.max_request_bytes = 1024;
        let mut inputs = Map::new();
        inputs.insert("x".into(), "a".repeat(2048).into());
        let req = PythonRunRequest {
            code: "output = 1".into(),
            mode: "none".into(),
            timeout: None,
            inputs,
        };
        let e = ex.run(req).await.unwrap_err();
        assert_eq!(e, PythonRunError::Python(input_too_large_message(1024)));
        assert_eq!(starts(&dir), 0);
    }

    #[test]
    fn the_template_gets_only_the_fixed_environment_and_the_locale() {
        let host = "LC_SOMETHING=x LANG=C.UTF-8 HOME=/ PATH=/opt LC_ALL=C LC_CTYPE=C";
        let host = host.split(' ').map(|kv| kv.split_once('=').unwrap());
        let env = template_env(host.map(|(k, v)| (k.into(), v.into())));
        let env: Vec<_> = env
            .iter()
            .map(|(k, v)| format!("{}={}", k.display(), v.display()))
            .collect();
        let fixed = TEMPLATE_ENV.iter().map(|(k, v)| format!("{k}={v}"));
        let locale = ["LANG=C.UTF-8", "LC_ALL=C", "LC_CTYPE=C"].map(String::from);
        assert_eq!(env, fixed.chain(locale).collect::<Vec<_>>());
    }

    /// Dropping a call's child kills it; the slot stays taken until the
    /// child's end closes (in a runtime) or the grace period passes.
    #[test]
    fn an_abandoned_call_keeps_its_slot_until_the_child_is_gone() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        for (in_runtime, wait) in [(true, Duration::from_millis(300)), (false, EXIT_GRACE)] {
            let pool = Arc::new(Mutex::new(Vec::new()));
            let free = || !pool.lock().unwrap().is_empty();
            let mut stand_in = Command::new("sleep").arg("30").spawn().unwrap();
            let (conn, peer) = rt.block_on(async { UnixStream::pair() }).unwrap();
            let permit = rt.block_on(Arc::new(Semaphore::new(1)).acquire_owned());
            let slot = Slot {
                index: 7,
                pool: pool.clone(),
                _permit: permit.unwrap(),
            };
            let _in = in_runtime.then(|| rt.enter());
            drop(CallChild {
                pid: stand_in.id() as libc::pid_t,
                live: Some((conn, slot)),
            });
            assert_eq!(stand_in.wait().unwrap().signal(), Some(libc::SIGKILL));
            std::thread::sleep(Duration::from_millis(300));
            assert!(!free(), "released while the child's end is open");
            drop(peer);
            std::thread::sleep(wait);
            assert!(free(), "not released (in_runtime = {in_runtime})");
        }
    }

    #[tokio::test]
    async fn a_template_that_exits_after_ready_is_replaced_at_once() {
        let (dir, ex) = fake("echo READY");
        ex.warm().await.unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        ex.warm().await.unwrap();
        assert_eq!(starts(&dir), 2);
    }
}
