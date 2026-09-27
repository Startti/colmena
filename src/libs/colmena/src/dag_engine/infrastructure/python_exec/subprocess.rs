//! Supervises the warm template process (`python_executor zygote`): starts it
//! with the environment `TEMPLATE_ENV`, replaces it once it has exited and
//! forwards its known stderr events. The template is started from a thread
//! this executor owns, because it is killed when the thread that started it
//! exits; that thread lives as long as the executor.

use super::child::EXIT_NOT_READY;
use super::config::{ExecutorConfigError, SubprocessConfig};
use super::zygote::TEMPLATE_ENV;
use crate::dag_engine::log_policy::T_PYTHON_EXEC;
use serde_json::{Map, Value};
use std::io::{BufRead, BufReader, Read};
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::process::ExitStatusExt;
use std::path::PathBuf;
use std::process::{Child, ChildStderr, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

const READY_TIMEOUT: Duration = Duration::from_secs(120);
const RESPAWN_BACKOFF: Duration = Duration::from_secs(5);
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

type Job = Box<dyn FnOnce() + Send>;

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
    state: tokio::sync::Mutex<State>,
    /// Runs every template start on the thread the template's life is tied to.
    spawner: mpsc::Sender<Job>,
    /// Template stderr lines that were not a known event; never logged.
    stderr_dropped: Arc<AtomicU64>,
}

impl SubprocessExecutor {
    pub fn new(cfg: SubprocessConfig) -> Result<Self, ExecutorConfigError> {
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
            cfg,
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
    cmd.env_clear().envs(TEMPLATE_ENV.iter().copied());
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
        (dir, SubprocessExecutor::new(cfg).unwrap())
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
    async fn a_template_that_exits_after_ready_is_replaced_at_once() {
        let (dir, ex) = fake("echo READY");
        ex.warm().await.unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        ex.warm().await.unwrap();
        assert_eq!(starts(&dir), 2);
    }
}
