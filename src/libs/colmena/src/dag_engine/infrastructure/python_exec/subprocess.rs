//! Supervises the warm template process (`python_executor zygote`): starts it
//! with the environment `TEMPLATE_ENV` plus the host's `LANG`, `LC_ALL` and
//! `LC_CTYPE`, replaces it once it has exited and forwards its known stderr
//! events. The template is started from a thread this executor owns, because
//! it is killed when the thread that started it exits; that thread lives as
//! long as the executor. Each call runs in a fresh child the template forks
//! for it (see [`SubprocessExecutor::run_raw`]).

use super::child::{CallHeader, CallMounts, EXIT_NOT_READY, MAX_HEADER_BYTES};
use super::config::{ExecutorConfigError, SubprocessConfig};
use super::frame;
use super::jail::{self, JailSpec};
use super::protocol::{
    input_too_large_message, result_too_large_message, WireRequest, WireResponse, CRASHED_MESSAGE,
    MALFORMED_MESSAGE, REFUSED_MESSAGE,
};
use super::staging::{
    StageError, StagedCall, StagingBudget, StagingLock, STAGED_OUT_MIB_MAX, STAGED_VOLUMES_MAX,
};
use super::zygote::{ARROW_ENV, TEMPLATE_ENV};
use crate::dag_engine::domain::python_executor::{
    ExecutorKind, PythonExecutor, PythonRunError, PythonRunRequest, PythonRunResult,
};
use crate::dag_engine::log_policy::T_PYTHON_EXEC;
use async_trait::async_trait;
use serde_json::{Map, Value};
use std::ffi::OsString;
use std::io::{self, BufRead, BufReader, Read};
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::io::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::process::ExitStatusExt;
use std::path::PathBuf;
use std::process::{Child, ChildStderr, Command, Stdio};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
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
/// What every call gets once no slot is left (see [`Slot`]).
const NO_SLOT_LEFT: &str =
    "PythonExecutorError: no usable Python slot is left; the process must be restarted";
/// The pause before a slot's processes, not stopped, are stopped once more.
const STOP_RETRY_PAUSE: Duration = Duration::from_millis(50);

/// The events the template writes on stderr and the keys they may carry.
const EVENTS: &[&str] = &[
    "child_exit",
    "warm_imports_failed",
    "fork_failed",
    "pdeathsig_failed",
    "subreaper_failed",
    "parent_exited",
    "template_not_single_threaded",
    "thread_count_failed",
    "bind_failed",
    "socket_permissions_failed",
    "accept_failed",
    "self_test_failed",
    "mounts_disabled",
    "arrow_pool_not_system",
    "arrow_pool_unreadable",
];
const NUMBER_KEYS: &[&str] = &[
    "pid",
    "exit_code",
    "signal",
    "max_rss_kb",
    "errno",
    "threads",
];
const NAME_KEYS: &[&str] = &["error_type", "module", "layer", "reason", "backend"];
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
    /// The result contains a literal of the output policy.
    Refused,
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
            RawFailure::Refused => PythonRunError::Python(REFUSED_MESSAGE.to_string()),
            RawFailure::Unavailable(m) => PythonRunError::Internal(m),
        }
    }
}

struct Template {
    child: Child,
    socket: PathBuf,
    dir: PathBuf,
    /// Why this template cannot offer run mounts, from its own startup checks.
    mounts_disabled: Option<String>,
}

impl Drop for Template {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl Drop for SubprocessExecutor {
    fn drop(&mut self) {
        let mut steps = Vec::new();
        self.teardown(&mut steps);
    }
}

enum State {
    NotStarted,
    Running(Template),
    Failed { at: Instant, reason: String },
}

pub struct SubprocessExecutor {
    cfg: SubprocessConfig,
    /// The jail the template applies; here for the uid of each slot.
    jail: JailSpec,
    max_timeout: Duration,
    state: tokio::sync::Mutex<State>,
    permits: Arc<Semaphore>,
    free_slots: Arc<Mutex<Vec<u32>>>,
    /// Slots not retired (see [`Slot`]).
    usable_slots: Arc<AtomicUsize>,
    /// Runs every template start on the thread the template's life is tied to.
    spawner: mpsc::Sender<Job>,
    /// Template stderr lines that were not a known event; never logged.
    stderr_dropped: Arc<AtomicU64>,
    /// Staged volumes in flight (dark behind `COLMENA_LARGE_TABULAR`).
    staging_budget: Arc<StagingBudget>,
    /// Why this executor cannot offer run mounts although a staging root is set,
    /// decided at start (the root's lock could not be taken for a reason other than
    /// "held"); `None` when it can.
    staging_unavailable: Option<&'static str>,
    /// The lock that makes this the only executor of its staging root, held for
    /// this executor's life (see [`Self::new_for_serving`]). The LAST field, and
    /// released by [`Self::teardown`] after everything else: a successor must not
    /// take the root while a call of this executor is still alive.
    _staging_lock: Option<StagingLock>,
}

/// Returns its index to the pool when dropped, once every process still
/// running as its uid is stopped. When they cannot be stopped, even on a
/// second try after [`STOP_RETRY_PAUSE`], the slot is retired instead: its
/// index and its permit are gone for good. Retiring the last one closes the
/// pool, and calls then fail at once with [`NO_SLOT_LEFT`] rather than wait.
/// A call holds it until its child is gone (see [`CallChild`]).
struct Slot {
    index: u32,
    uid: u32,
    pool: Arc<Mutex<Vec<u32>>>,
    usable: Arc<AtomicUsize>,
    permit: Option<OwnedSemaphorePermit>,
    /// [`kill_uid`] outside tests.
    stop: fn(u32) -> Result<(), NotStopped>,
}

impl Drop for Slot {
    fn drop(&mut self) {
        let stopped = (self.stop)(self.uid).or_else(|_| {
            std::thread::sleep(STOP_RETRY_PAUSE);
            (self.stop)(self.uid)
        });
        let Err(e) = stopped else {
            self.pool.lock().unwrap().push(self.index);
            return;
        };
        tracing::error!(
            target: T_PYTHON_EXEC,
            slot = self.index,
            errno = e.errno,
            status = e.status,
            "python slot retired"
        );
        if let Some(permit) = self.permit.take() {
            if self.usable.fetch_sub(1, Ordering::AcqRel) == 1 {
                tracing::warn!(target: T_PYTHON_EXEC, slot = self.index, "no usable python slot left");
                permit.semaphore().close();
            }
            permit.forget();
        }
    }
}

/// Why the processes of a uid may still run: the errno of a failed `fork` or
/// `waitpid`, or the wait status of a helper that did not succeed.
#[derive(Debug, Default)]
struct NotStopped {
    errno: Option<i32>,
    status: Option<i32>,
}

/// A call's child once its pid is known, with the call's connection and slot.
/// Unless the call saw the child close its end, dropping this kills the child
/// and hands the connection and slot to [`reap`]: when the child is still
/// there `EXIT_GRACE` after the call, or when the call is abandoned midway
/// (its future dropped).
struct CallChild {
    pid: libc::pid_t,
    /// Reaches this very process even once its pid is reused; `None` on a
    /// kernel without pidfds, where the pid is signalled instead.
    pidfd: Option<OwnedFd>,
    live: Option<(UnixStream, Slot)>,
}

impl Drop for CallChild {
    fn drop(&mut self) {
        if let Some((conn, slot)) = self.live.take() {
            kill(self.pid, self.pidfd.as_ref());
            reap(conn, slot);
        }
    }
}

/// A pidfd call failed because pidfds are not available: a kernel without
/// them, or a seccomp profile that refuses the call.
fn no_pidfds(e: &io::Error) -> bool {
    matches!(e.raw_os_error(), Some(libc::ENOSYS | libc::EPERM))
}

/// SIGKILLs a call's child through its pidfd, or by pid where there is none.
fn kill(pid: libc::pid_t, pidfd: Option<&OwnedFd>) {
    if let Some(fd) = pidfd {
        let (fd, sig) = (
            libc::c_long::from(fd.as_raw_fd()),
            libc::c_long::from(libc::SIGKILL),
        );
        let zero: libc::c_long = 0;
        // SAFETY: a signal through a pidfd the call owns.
        let rc = unsafe { libc::syscall(libc::SYS_pidfd_send_signal, fd, sig, zero, zero) };
        if rc == 0 || !no_pidfds(&io::Error::last_os_error()) {
            return;
        }
    }
    // SAFETY: a signal to one process; `run_raw` refuses a pid <= 1.
    unsafe { libc::kill(pid, libc::SIGKILL) };
}

/// A pidfd for `pid`, or `None` where pidfds are not available.
fn open_pidfd(pid: libc::pid_t) -> io::Result<Option<OwnedFd>> {
    let zero: libc::c_long = 0;
    // SAFETY: returns a new descriptor, or -1.
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, libc::c_long::from(pid), zero) };
    if fd >= 0 {
        // SAFETY: `fd` was just opened and nothing else owns it.
        return Ok(Some(unsafe { OwnedFd::from_raw_fd(fd as RawFd) }));
    }
    let e = io::Error::last_os_error();
    if no_pidfds(&e) {
        return Ok(None);
    }
    Err(e)
}

/// SIGKILLs every process running as `uid`: a call's child and whatever it
/// started, even once detached from it. A helper forked with that uid calls
/// `kill(-1)`, which reaches exactly the processes it may signal, in one step
/// a concurrent fork cannot outrun. It signals nothing unless it has lost
/// the right to signal the host, and succeeds only when it took the uid and
/// `kill(-1)` signalled or found nothing to signal.
fn kill_uid(uid: u32) -> Result<(), NotStopped> {
    if uid == 0 {
        return Err(NotStopped::default());
    }
    let id = libc::c_long::from(uid);
    // SAFETY: the host is multi-threaded, so the helper makes raw system
    // calls only and exits.
    let pid = unsafe { libc::fork() };
    if pid == 0 {
        unsafe {
            let dropped = libc::syscall(libc::SYS_setgroups, 0 as libc::c_long, 0 as libc::c_long)
                == 0
                && libc::syscall(libc::SYS_setresgid, id, id, id) == 0
                && libc::syscall(libc::SYS_setresuid, id, id, id) == 0;
            let killed = dropped
                && libc::kill(libc::getppid(), 0) != 0
                && (libc::kill(-1, libc::SIGKILL) == 0
                    || io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH));
            libc::_exit(if killed { 0 } else { 1 })
        }
    }
    let failed = || NotStopped {
        errno: io::Error::last_os_error().raw_os_error(),
        status: None,
    };
    if pid < 0 {
        return Err(failed());
    }
    let mut status = 0;
    while unsafe { libc::waitpid(pid, &mut status, 0) } < 0 {
        if io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
            return Err(failed());
        }
    }
    if libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0 {
        return Ok(());
    }
    Err(NotStopped {
        errno: None,
        status: Some(status),
    })
}

/// Longest [`confirm_uid_gone`] waits for the processes to leave `/proc`.
const CONFIRM_WAIT: Duration = Duration::from_secs(2);

/// Whether a process other than a zombie runs as `uid`. Reads the host's `/proc`
/// (the executor runs outside the jail); a `/proc` that cannot be read says "yes":
/// when it cannot be known, it is not confirmed.
fn uid_has_processes(uid: u32) -> bool {
    let Ok(dir) = std::fs::read_dir("/proc") else {
        return true;
    };
    for entry in dir.flatten() {
        let name = entry.file_name();
        if !name.to_string_lossy().bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let Ok(status) = std::fs::read_to_string(entry.path().join("status")) else {
            // Gone between the listing and the read, which is what is hoped for.
            continue;
        };
        let zombie = status
            .lines()
            .any(|l| l.starts_with("State:") && l.contains('Z'));
        let ours = status
            .lines()
            .find_map(|l| l.strip_prefix("Uid:"))
            .is_some_and(|rest| {
                rest.split_whitespace()
                    .next()
                    .and_then(|f| f.parse::<u32>().ok())
                    == Some(uid)
            });
        if ours && !zombie {
            return true;
        }
    }
    false
}

/// Blocking. Stops every process of `uid` (again) and waits, up to `wait`, until
/// none is left. `false` when the stop fails or something still runs.
fn confirm_uid_gone(stop: fn(u32) -> Result<(), NotStopped>, uid: u32, wait: Duration) -> bool {
    if stop(uid).is_err() {
        return false;
    }
    let end = Instant::now() + wait;
    loop {
        if !uid_has_processes(uid) {
            return true;
        }
        if Instant::now() >= end {
            return false;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Reads until the child closes its end (EOF) or the read fails.
async fn drain(conn: &mut UnixStream) {
    let mut sink = [0u8; 64];
    while conn.read(&mut sink).await.is_ok_and(|n| n > 0) {}
}

/// A slot kept until `until`. Dropped with the slot still in it (outside a
/// runtime, or when a shutting-down runtime cancels [`reap`]'s task), it
/// leaves the rest of the wait to a thread.
struct Held {
    slot: Option<Slot>,
    until: Instant,
}

impl Drop for Held {
    fn drop(&mut self) {
        if let Some(slot) = self.slot.take() {
            let rest = self.until.saturating_duration_since(Instant::now());
            let wait = move || {
                std::thread::sleep(rest);
                drop(slot);
            };
            let _ = std::thread::Builder::new().spawn(wait);
        }
    }
}

/// Stops what a killed child's call left running, and returns the slot only
/// once the child has closed its end or `EXIT_GRACE` has passed since then.
fn reap(mut conn: UnixStream, slot: Slot) {
    // Early; the slot stops them again, and checks, when it is freed.
    let _ = (slot.stop)(slot.uid);
    let mut held = Held {
        slot: Some(slot),
        until: Instant::now() + EXIT_GRACE,
    };
    if let Ok(rt) = tokio::runtime::Handle::try_current() {
        rt.spawn(async move {
            let _ = tokio::time::timeout(EXIT_GRACE, drain(&mut conn)).await;
            drop(held.slot.take());
        });
    }
}

impl SubprocessExecutor {
    /// Volumes and MiB of staged calls in flight on this executor.
    pub fn staged_in_flight(&self) -> (usize, u64) {
        self.staging_budget.in_flight()
    }

    /// Stops this executor's processes in order, recording each step: the template
    /// (which takes its children with it), then whatever still runs as one of the
    /// slots' users (a call's process that outlived its call), and only then releases
    /// the staging lock (when there is one). Runs from `Drop`.
    fn teardown(&mut self, steps: &mut Vec<&'static str>) {
        if let Ok(mut state) = self.state.try_lock() {
            // Dropping the template kills it and waits for it.
            *state = State::NotStarted;
        }
        steps.push("template_stopped");
        if self._staging_lock.is_some() {
            for index in 0..self.cfg.slots as u32 {
                let uid = jail::uid_for(&self.jail, index);
                for _ in 0..3 {
                    if kill_uid(uid).is_ok() {
                        break;
                    }
                    std::thread::sleep(STOP_RETRY_PAUSE);
                }
            }
        }
        steps.push("slot_processes_stopped");
        if self._staging_lock.take().is_some() {
            steps.push("staging_lock_released");
        }
    }

    /// Stages one call (dark behind `COLMENA_LARGE_TABULAR`): takes a share of
    /// the executor's budget of volumes in flight, then makes the directories
    /// and the output volume. The share goes back when the call is dropped.
    pub fn stage_call(&self, out_mb: u64) -> Result<StagedCall, StageError> {
        let root = self
            .cfg
            .staging_root
            .as_ref()
            .ok_or(StageError::NoStagingRoot)?;
        if let Some(reason) = self.staging_unavailable {
            return Err(StageError::MountsDisabled(reason));
        }
        let share = StagingBudget::reserve(&self.staging_budget, out_mb)?;
        Ok(StagedCall::create_with(root, out_mb, Some(share))?)
    }

    /// One call that carries prepared data (dark behind
    /// `COLMENA_LARGE_TABULAR`): the same sandbox as [`PythonExecutor::run`],
    /// plus the call's staged directories. The seam the protocol work builds on.
    pub async fn run_staged(
        &self,
        req: PythonRunRequest,
        mounts: CallMounts,
    ) -> Result<PythonRunResult, PythonRunError> {
        self.run_with(req, Some(mounts), false).await
    }

    /// [`Self::run_staged`] for a caller that is about to read the call's output
    /// volume as root: a result is returned only once EVERY process of the call's
    /// uid is confirmed gone (`/proc` lists none, zombies aside), after a second
    /// kill of the uid. If that cannot be confirmed the call fails with an
    /// executor error and the result is discarded: nothing may still be writing to
    /// the volume when it is read.
    pub async fn run_staged_confirmed(
        &self,
        req: PythonRunRequest,
        mounts: CallMounts,
    ) -> Result<PythonRunResult, PythonRunError> {
        self.run_with(req, Some(mounts), true).await
    }

    async fn run_with(
        &self,
        req: PythonRunRequest,
        mounts: Option<CallMounts>,
        confirm: bool,
    ) -> Result<PythonRunResult, PythonRunError> {
        let timeout = req.timeout.unwrap_or(self.max_timeout);
        let bytes = serde_json::to_vec(&WireRequest::new(req, timeout)).map_err(|e| {
            PythonRunError::Internal(format!(
                "PythonExecutorError: cannot encode the request: {e}"
            ))
        })?;
        match self.run_raw_with(timeout, &bytes, mounts, confirm).await {
            Ok(resp) => serde_json::from_slice::<WireResponse>(&resp)
                .map_err(|_| PythonRunError::Python(MALFORMED_MESSAGE.to_string()))?
                .into_result(),
            Err(f) => Err(f.into_run_error(&self.cfg)),
        }
    }

    /// Fails unless the binary exists and this process runs as root: each
    /// child switches to an unprivileged user of its own, which only root may
    /// do. The template starts on the first call, or on [`Self::warm`].
    pub fn new(cfg: SubprocessConfig, max_timeout: Duration) -> Result<Self, ExecutorConfigError> {
        if !cfg.bin.is_file() {
            return Err(ExecutorConfigError(format!(
                "the Python executor binary was not found at {}",
                cfg.bin.display()
            )));
        }
        if unsafe { libc::geteuid() } != 0 {
            return Err(ExecutorConfigError(
                "the subprocess executor must start as root inside its container: children switch to unprivileged users".into(),
            ));
        }
        Self::unchecked(cfg, max_timeout)
    }

    /// [`Self::new`] for an executor that is about to serve: first reclaims what a
    /// killed predecessor left in the staging root (prepared customer data and
    /// mounted volumes, which nothing else removes). Not part of `new` because a
    /// sweep cannot tell a leftover from a call in flight: it is for the one
    /// executor that owns its root, at the one moment it starts.
    pub fn new_for_serving(
        cfg: SubprocessConfig,
        max_timeout: Duration,
    ) -> Result<Self, ExecutorConfigError> {
        Self::new_for_serving_with(cfg, max_timeout, StagingLock::acquire)
    }

    /// [`Self::new_for_serving`] with the lock acquisition supplied, so that a test
    /// can make it fail. "Held" (another executor owns the root) is a hard startup
    /// error and sweeps nothing. A lock that cannot be taken for any OTHER reason
    /// (ENOLCK, ENOSYS, EOPNOTSUPP on some network or FUSE volumes) does not take the
    /// executor down: nothing is swept (the root is not owned), run mounts are
    /// disabled with the reason `staging_lock_unavailable`, loudly logged, and plain
    /// calls are served.
    pub fn new_for_serving_with(
        cfg: SubprocessConfig,
        max_timeout: Duration,
        acquire: impl FnOnce(&std::path::Path) -> Result<StagingLock, super::staging::LockError>,
    ) -> Result<Self, ExecutorConfigError> {
        let mut executor = Self::new(cfg, max_timeout)?;
        if let Some(root) = executor.cfg.staging_root.clone() {
            // Before the sweep, which would unmount a live executor's volumes: the
            // root is taken first, or nothing is swept.
            match acquire(&root) {
                Ok(lock) => {
                    executor._staging_lock = Some(lock);
                    if let Err(e) = super::staging::sweep_staging_root(&root) {
                        tracing::error!(target: T_PYTHON_EXEC, error = %e, "cannot sweep the staging root");
                    }
                }
                Err(super::staging::LockError::Io(e)) => {
                    tracing::error!(
                        target: T_PYTHON_EXEC,
                        error = %e,
                        reason = "staging_lock_unavailable",
                        "the staging root cannot be locked: run mounts are DISABLED, nothing is swept"
                    );
                    executor.staging_unavailable = Some("staging_lock_unavailable");
                }
                Err(held) => return Err(ExecutorConfigError(held.to_string())),
            }
        }
        Ok(executor)
    }

    /// [`Self::new`] without its checks, for the unit tests' stand-in
    /// templates, which run without root.
    pub(super) fn unchecked(
        cfg: SubprocessConfig,
        max_timeout: Duration,
    ) -> Result<Self, ExecutorConfigError> {
        let (spawner, jobs) = mpsc::channel::<Job>();
        // Ends when the executor drops `spawner`; the template goes with it.
        std::thread::Builder::new()
            .name("python-template".into())
            .spawn(move || jobs.into_iter().for_each(|job| job()))
            .map_err(|e| {
                ExecutorConfigError(format!("cannot start the Python template thread: {e}"))
            })?;
        Ok(Self {
            jail: JailSpec {
                uid_base: cfg.uid_base,
                tmp_mb: cfg.tmp_mb,
                hide_paths: cfg.hide_paths.clone(),
                staging_root: cfg.staging_root.clone(),
            },
            permits: Arc::new(Semaphore::new(cfg.slots)),
            free_slots: Arc::new(Mutex::new((0..cfg.slots as u32).rev().collect())),
            usable_slots: Arc::new(AtomicUsize::new(cfg.slots)),
            cfg,
            max_timeout,
            state: tokio::sync::Mutex::new(State::NotStarted),
            spawner,
            stderr_dropped: Arc::default(),
            staging_budget: Arc::new(StagingBudget::new(STAGED_VOLUMES_MAX, STAGED_OUT_MIB_MAX)),
            staging_unavailable: None,
            _staging_lock: None,
        })
    }

    /// Whether a call can still get a slot: false once every slot is retired
    /// (see [`Slot`]), when the process must be restarted.
    pub fn has_usable_slot(&self) -> bool {
        self.usable_slots.load(Ordering::Acquire) > 0
    }

    pub fn config(&self) -> &SubprocessConfig {
        &self.cfg
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

    /// Why this executor cannot take a call with run mounts, if it cannot: no
    /// staging root, or the reason its template turned the mounts off. Starts
    /// the template when it is not running, so a server can refuse a mounts
    /// call before it reads the call's data.
    pub async fn mounts_unavailable(&self) -> Option<String> {
        if self.cfg.staging_root.is_none() {
            return Some("no_staging_root".to_string());
        }
        if self.warm().await.is_err() {
            return Some("template_not_ready".to_string());
        }
        self.mounts_disabled().await
    }

    /// Why the running template cannot offer run mounts, if it cannot.
    async fn mounts_disabled(&self) -> Option<String> {
        match &*self.state.lock().await {
            State::Running(t) => t.mounts_disabled.clone(),
            _ => None,
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
        // The pool is closed only once its last slot is retired.
        let closed = |_| RawFailure::Unavailable(NO_SLOT_LEFT.into());
        let permit = self.permits.clone().acquire_owned().await.map_err(closed)?;
        let index = self.free_slots.lock().unwrap().pop();
        let index = index.expect("a permit always has a free slot");
        Ok(Slot {
            index,
            uid: jail::uid_for(&self.jail, index),
            pool: self.free_slots.clone(),
            usable: self.usable_slots.clone(),
            permit: Some(permit),
            stop: kill_uid,
        })
    }

    /// One call with an already-encoded request. A request over the limit is
    /// refused here: the child could only answer it to a host whose write of
    /// the whole request succeeded.
    pub async fn run_raw(&self, timeout: Duration, request: &[u8]) -> Result<Vec<u8>, RawFailure> {
        self.run_raw_with(timeout, request, None, false).await
    }

    /// [`Self::run_raw`] for a call that carries prepared data: the header
    /// asks the jail for the call's staged directories. Without a staging
    /// root no child is started.
    async fn run_raw_with(
        &self,
        timeout: Duration,
        request: &[u8],
        mounts: Option<CallMounts>,
        confirm: bool,
    ) -> Result<Vec<u8>, RawFailure> {
        if let (Some(_), Some(reason)) = (&mounts, self.staging_unavailable) {
            return Err(RawFailure::Unavailable(
                StageError::MountsDisabled(reason).to_string(),
            ));
        }
        if mounts.is_some() && self.cfg.staging_root.is_none() {
            return Err(RawFailure::Unavailable(
                "PythonExecutorError: this executor has no staging directory configured".into(),
            ));
        }
        if request.len() > self.cfg.max_request_bytes {
            return Err(RawFailure::RequestTooLarge);
        }
        let slot = self.take_slot().await?;
        let socket = self.socket_path().await.map_err(RawFailure::Unavailable)?;
        if mounts.is_some() {
            if let Some(reason) = self.mounts_disabled().await {
                return Err(RawFailure::Unavailable(format!(
                    "PythonExecutorError: run mounts are disabled on this executor ({reason}); see its startup log"
                )));
            }
        }
        let mut conn = UnixStream::connect(&socket).await.map_err(|e| {
            RawFailure::Unavailable(format!(
                "PythonExecutorError: cannot reach the Python template process: {e}"
            ))
        })?;
        let mut pid = [0u8; 4];
        let read = tokio::time::timeout(PID_TIMEOUT, conn.read_exact(&mut pid)).await;
        // A pid of 0 or -1 would signal a whole group of processes.
        let pid = libc::pid_t::try_from(u32::from_be_bytes(pid)).unwrap_or(0);
        // The child waits for its header, so its pid is not reused before this.
        let pidfd = (matches!(read, Ok(Ok(_))) && pid > 1).then(|| open_pidfd(pid));
        let Some(Ok(pidfd)) = pidfd else {
            return Err(RawFailure::Unavailable(
                "PythonExecutorError: the Python process did not start".into(),
            ));
        };
        let header = CallHeader {
            slot: slot.index,
            memory_mb: self.cfg.memory_mb,
            cpu_secs: timeout.as_secs().saturating_add(1),
            max_request_bytes: self.cfg.max_request_bytes,
            mounts,
        };
        let header = serde_json::to_vec(&header).expect("header serializes");
        debug_assert!(header.len() <= MAX_HEADER_BYTES);
        let (slot_stop, slot_uid) = (slot.stop, slot.uid);
        let mut child = CallChild {
            pid,
            pidfd,
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
            kill(pid, child.pidfd.as_ref());
        }
        // A caller that will read the output volume wants every process of the
        // call's uid GONE first, not signalled: the stop that frees the slot only
        // logs when it fails.
        if confirm && result.is_ok() {
            let (stop, uid) = (slot_stop, slot_uid);
            let gone =
                tokio::task::spawn_blocking(move || confirm_uid_gone(stop, uid, CONFIRM_WAIT))
                    .await
                    .unwrap_or(false);
            if !gone {
                tracing::error!(target: T_PYTHON_EXEC, "could not confirm every process of a call exited");
                return Err(RawFailure::Unavailable(
                    "PythonExecutorError: could not confirm that the call's processes ended".into(),
                ));
            }
        }
        // The slot is free again once the child has closed its end; a child
        // that has not by `EXIT_GRACE` is left to `reap` when `child` drops.
        if tokio::time::timeout(EXIT_GRACE, drain(conn)).await.is_ok() {
            child.live = None;
        }
        // Output, stdout and error text travel in this one body.
        match result {
            Ok(bytes)
                if self
                    .cfg
                    .refuse_output
                    .iter()
                    .any(|p| contains(&bytes, p.as_bytes())) =>
            {
                tracing::warn!(target: T_PYTHON_EXEC, "python result refused by the output policy");
                Err(RawFailure::Refused)
            }
            other => other,
        }
    }
}

/// Whether `needle` occurs in `haystack`; never for an empty `needle`.
fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && haystack.windows(needle.len()).any(|w| w == needle)
}

#[async_trait]
impl PythonExecutor for SubprocessExecutor {
    fn kind(&self) -> ExecutorKind {
        ExecutorKind::Subprocess
    }

    async fn run(&self, req: PythonRunRequest) -> Result<PythonRunResult, PythonRunError> {
        self.run_with(req, None, false).await
    }

    async fn warm(&self) -> Result<(), String> {
        SubprocessExecutor::warm(self).await
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
    if let Some(root) = &cfg.staging_root {
        cmd.arg("--staging-root").arg(root);
    }
    let staged = cfg.staging_root.is_some();
    cmd.env_clear()
        .envs(template_env_with(std::env::vars_os(), staged));
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = cmd.spawn().map_err(|e| {
        let _ = std::fs::remove_dir_all(&dir);
        format!("PythonExecutorError: cannot start the Python template process: {e}")
    })?;
    let mut t = Template {
        child,
        socket,
        dir,
        mounts_disabled: None,
    };
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
                let mut read = out.by_ref().take(96).read_line(&mut first);
                // An optional line before READY: run mounts are unavailable.
                let off = mounts_disabled_reason(&first);
                if off.is_some() {
                    first.clear();
                    read = out.by_ref().take(64).read_line(&mut first);
                }
                let ok = read.is_ok() && first.trim_end() == "READY";
                let _ = ready_tx.send(ok.then_some(off));
                // Children inherit this pipe: keep it drained so none blocks on it.
                let _ = std::io::copy(&mut out, &mut std::io::sink());
            })
        })
        .map_err(|e| {
            format!("PythonExecutorError: cannot read the Python template process: {e}")
        })?;
    if let Ok(Some(off)) = ready.recv_timeout(READY_TIMEOUT) {
        if let Some(reason) = &off {
            tracing::error!(target: T_PYTHON_EXEC, reason = %reason, "python template ready WITHOUT run mounts");
        }
        t.mounts_disabled = off;
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

/// The reason on a `MOUNTS_DISABLED <reason>` line: 1 to 64 letters, digits and
/// underscores, nothing else.
fn mounts_disabled_reason(line: &str) -> Option<String> {
    let reason = line.trim_end().strip_prefix("MOUNTS_DISABLED ")?;
    let plain = !reason.is_empty()
        && reason.len() <= 64
        && reason
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_');
    plain.then(|| reason.to_string())
}

/// `TEMPLATE_ENV` plus the host's `LOCALE_VARS`; nothing else of `host`.
#[cfg(test)]
fn template_env(host: impl Iterator<Item = (OsString, OsString)>) -> Vec<(OsString, OsString)> {
    template_env_with(host, false)
}

/// [`template_env`], plus [`ARROW_ENV`] after the fixed variables when the
/// executor stages prepared data (a staging root is configured).
fn template_env_with(
    host: impl Iterator<Item = (OsString, OsString)>,
    arrow: bool,
) -> Vec<(OsString, OsString)> {
    let fixed = TEMPLATE_ENV.iter().map(|&(k, v)| (k.into(), v.into()));
    let arrow = ARROW_ENV
        .iter()
        .filter(|_| arrow)
        .map(|&(k, v)| (k.into(), v.into()));
    let locale = host.filter(|(k, _)| LOCALE_VARS.iter().any(|l| k.as_os_str() == *l));
    fixed.chain(arrow).chain(locale).collect()
}

/// Logs each known template event from stderr. Any other line is only counted,
/// never logged.
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
    use super::super::{config::ModesPolicy, inprocess::InProcessExecutor, Dispatcher};
    use super::*;

    #[test]
    fn only_known_events_with_known_fields_pass() {
        let exit = br#"{"event":"child_exit","pid":7,"exit_code":null,"signal":9,"max_rss_kb":1}"#;
        assert!(template_event(exit).is_some());
        let import =
            br#"{"event":"warm_imports_failed","error_type":"ImportError","module":"scipy.stats"}"#;
        assert!(template_event(import).is_some());
        let layer = br#"{"event":"self_test_failed","layer":"network_dns","reason":"resolved","errno":null}"#;
        assert!(template_event(layer).is_some());
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
        write_executable(&bin, &format!("#!/bin/sh\necho >> {starts}\n{body}\n"));
        let mut cfg = SubprocessConfig::from_lookup(&|_: &str| None::<String>).unwrap();
        cfg.bin = bin;
        (
            dir,
            SubprocessExecutor::unchecked(cfg, Duration::from_secs(60)).unwrap(),
        )
    }

    /// Writes `script` to `path` from a child shell and makes it executable,
    /// so that this process never holds a writable descriptor on a file it
    /// later executes. Such a descriptor, even `O_CLOEXEC`, is copied into any
    /// child another test thread forks while it is open, and stays there until
    /// that child execs or exits; executing the file inside that window fails
    /// with ETXTBSY ("Text file busy"). Renaming would not help: the copy
    /// holds the inode, whatever its name. The shell's `printf` is a builtin,
    /// so the file is closed once the shell has exited and `status()` returns.
    fn write_executable(path: &std::path::Path, script: &str) {
        let written = Command::new("/bin/sh")
            .args(["-c", r#"printf '%s' "$1" > "$0""#])
            .arg(path)
            .arg(script)
            .status()
            .unwrap();
        assert!(written.success(), "{written}");
        std::fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
    }

    /// Other tests fork while `fake` writes its script, so every script it
    /// writes must run at once however busy the process is forking. Written
    /// with `std::fs::write` instead, about one run in ten fails with ETXTBSY.
    #[test]
    fn a_written_script_runs_while_other_threads_fork() {
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let forkers: Vec<_> = (0..4)
            .map(|_| {
                let stop = stop.clone();
                std::thread::spawn(move || {
                    while !stop.load(Ordering::Relaxed) {
                        let _ = Command::new("/bin/true").status();
                    }
                })
            })
            .collect();
        let dir = tempfile::tempdir().unwrap();
        let runs: Vec<_> = (0..200)
            .map(|i| {
                let bin = dir.path().join(i.to_string());
                write_executable(&bin, "#!/bin/sh\nexit 0\n");
                Command::new(&bin).status()
            })
            .collect();
        stop.store(true, Ordering::Relaxed);
        forkers.into_iter().for_each(|f| f.join().unwrap());
        for (i, run) in runs.into_iter().enumerate() {
            assert!(run.as_ref().is_ok_and(|s| s.success()), "run {i}: {run:?}");
        }
    }

    /// Teardown order: the template is stopped, then whatever still runs as a slot's
    /// user, and only then is the staging lock released, so that a successor cannot
    /// take the root (and sweep it) while a call of this executor is still alive.
    #[tokio::test]
    async fn teardown_stops_the_template_and_the_slot_processes_before_the_lock_is_released() {
        use std::os::unix::process::CommandExt;
        let (_dir, mut ex) = fake("echo READY\nexec sleep 30");
        let root = tempfile::tempdir().unwrap();
        let root_path = root.path().canonicalize().unwrap();
        ex._staging_lock = Some(StagingLock::acquire(&root_path).unwrap());
        ex.warm().await.unwrap();
        let template = match &*ex.state.lock().await {
            State::Running(t) => t.child.id() as libc::pid_t,
            _ => panic!("the template did not start"),
        };
        // A process of a call left running as slot 0's user (needs root to be made).
        let stray = (unsafe { libc::geteuid() } == 0).then(|| {
            let uid = jail::uid_for(&ex.jail, 0);
            let mut cmd = std::process::Command::new("sleep");
            cmd.arg("60").uid(uid).gid(uid);
            cmd.spawn().unwrap()
        });
        let mut steps = Vec::new();
        ex.teardown(&mut steps);
        assert_eq!(
            steps,
            [
                "template_stopped",
                "slot_processes_stopped",
                "staging_lock_released"
            ]
        );
        assert_ne!(
            unsafe { libc::kill(template, 0) },
            0,
            "the template is still there"
        );
        if let Some(mut stray) = stray {
            let status = stray.wait().unwrap();
            assert!(
                !status.success(),
                "the call's process outlived the teardown"
            );
        }
        // Free now. A helper that another test of this process forked at this very
        // moment holds a copy of the descriptor for an instant: retry briefly.
        let freed = (0..100).any(|_| {
            let ok = StagingLock::acquire(&root_path).is_ok();
            if !ok {
                std::thread::sleep(Duration::from_millis(20));
            }
            ok
        });
        assert!(freed, "the lock was not released");
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

    /// With a staging root the template gets the Arrow variables on top, after
    /// the fixed ones; without one it gets exactly the environment above.
    #[test]
    fn the_arrow_variables_are_added_only_for_a_staged_executor() {
        let host = || std::iter::empty::<(OsString, OsString)>();
        assert_eq!(template_env_with(host(), false), template_env(host()));
        let arrow: Vec<_> = template_env_with(host(), true)
            .iter()
            .map(|(k, v)| format!("{}={}", k.display(), v.display()))
            .collect();
        let fixed = TEMPLATE_ENV.iter().map(|(k, v)| format!("{k}={v}"));
        let extra = ARROW_ENV.iter().map(|(k, v)| format!("{k}={v}"));
        assert_eq!(arrow, fixed.chain(extra).collect::<Vec<_>>());
        assert_eq!(
            ARROW_ENV,
            [
                ("ARROW_DEFAULT_MEMORY_POOL", "system"),
                ("ARROW_IO_THREADS", "1")
            ]
        );
    }

    /// Dropping a call's child kills it; the slot stays taken until the
    /// child's end closes (in a runtime) or the grace period passes, also when
    /// the runtime reaping it shuts down first.
    #[test]
    fn an_abandoned_call_keeps_its_slot_until_the_child_is_gone() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        for case in ["runtime", "no runtime", "runtime shut down"] {
            let pool = Arc::new(Mutex::new(Vec::new()));
            let free = || !pool.lock().unwrap().is_empty();
            let mut stand_in = Command::new("sleep").arg("30").spawn().unwrap();
            let temp = tokio::runtime::Runtime::new().unwrap();
            let reaper = if case == "runtime" { &rt } else { &temp };
            let (conn, peer) = reaper.block_on(async { UnixStream::pair() }).unwrap();
            let permit = rt.block_on(Arc::new(Semaphore::new(1)).acquire_owned());
            let slot = Slot {
                index: 7,
                uid: 20007,
                pool: pool.clone(),
                usable: Arc::new(AtomicUsize::new(1)),
                permit: permit.ok(),
                stop: |_| Ok(()),
            };
            let pid = stand_in.id() as libc::pid_t;
            let entered = (case != "no runtime").then(|| reaper.enter());
            drop(CallChild {
                pid,
                pidfd: open_pidfd(pid).unwrap(),
                live: Some((conn, slot)),
            });
            drop(entered);
            // In the last case this cancels the task that was reaping.
            drop(temp);
            assert_eq!(stand_in.wait().unwrap().signal(), Some(libc::SIGKILL));
            std::thread::sleep(Duration::from_millis(300));
            assert!(!free(), "released while the child's end is open ({case})");
            drop(peer);
            let wait = match case {
                "runtime" => Duration::from_millis(300),
                _ => EXIT_GRACE,
            };
            std::thread::sleep(wait);
            assert!(free(), "not released ({case})");
        }
    }

    fn not_stopped(_: u32) -> Result<(), NotStopped> {
        Err(NotStopped::default())
    }

    /// A slot is freed only once its processes are stopped, on the first try
    /// or the second, which comes after a pause; otherwise it is retired: no
    /// index back in the pool, one permit fewer, and the pool closed once no
    /// slot is left.
    #[test]
    fn a_slot_whose_processes_cannot_be_stopped_is_retired() {
        static TRIES: AtomicUsize = AtomicUsize::new(0);
        fn stopped_on_retry(_: u32) -> Result<(), NotStopped> {
            match TRIES.fetch_add(1, Ordering::Relaxed) {
                0 => Err(NotStopped::default()),
                _ => Ok(()),
            }
        }
        let permits = Arc::new(Semaphore::new(2));
        let pool = Arc::new(Mutex::new(vec![1, 0]));
        let usable = Arc::new(AtomicUsize::new(2));
        let take = |stop: fn(u32) -> Result<(), NotStopped>| Slot {
            index: pool.lock().unwrap().pop().unwrap(),
            uid: 20000,
            pool: pool.clone(),
            usable: usable.clone(),
            permit: permits.clone().try_acquire_owned().ok(),
            stop,
        };
        let state = || (permits.available_permits(), pool.lock().unwrap().clone());
        let t0 = Instant::now();
        drop(take(stopped_on_retry));
        assert!(t0.elapsed() >= STOP_RETRY_PAUSE, "tried again at once");
        assert_eq!(state(), (2, vec![1, 0]));
        drop(take(not_stopped));
        assert_eq!(state(), (1, vec![1]));
        assert!(!permits.is_closed());
        drop(take(not_stopped));
        assert_eq!(state(), (0, vec![]));
        assert!(permits.is_closed());
    }

    /// Fails unless the helper took the uid and signalled as it: never for
    /// root, nor for `u32::MAX`, which `setresuid` reads as "unchanged".
    #[test]
    fn stopping_a_uid_succeeds_only_as_that_uid() {
        assert!(kill_uid(0).is_err());
        assert!(kill_uid(u32::MAX).is_err());
        // Nothing runs as this uid; only root may take it.
        let root = unsafe { libc::geteuid() } == 0;
        assert_eq!(kill_uid(4_000_000_000).is_ok(), root);
    }

    /// Once every slot is retired, a call fails at once and says so.
    #[tokio::test]
    async fn a_call_fails_at_once_when_no_slot_is_left() {
        let (_dir, ex) = fake("exit 1");
        assert!(ex.has_usable_slot());
        let mut held = Vec::new();
        for _ in 0..ex.cfg.slots {
            let Ok(mut slot) = ex.take_slot().await else {
                panic!("a slot is free")
            };
            slot.stop = not_stopped;
            held.push(slot);
        }
        drop(held);
        assert!(!ex.has_usable_slot());
        let taken = tokio::time::timeout(Duration::from_secs(1), ex.take_slot()).await;
        let Ok(Err(RawFailure::Unavailable(m))) = taken else {
            panic!("the call did not fail at once")
        };
        assert_eq!(m, NO_SLOT_LEFT);
    }

    #[test]
    fn only_enosys_and_eperm_mean_no_pidfds() {
        for (errno, unavailable) in [
            (libc::ENOSYS, true),
            (libc::EPERM, true),
            (libc::ESRCH, false),
            (libc::EINVAL, false),
        ] {
            let e = io::Error::from_raw_os_error(errno);
            assert_eq!(no_pidfds(&e), unavailable, "{errno}");
        }
    }

    #[test]
    fn contains_finds_a_literal_anywhere_and_never_an_empty_one() {
        for (haystack, needle, found) in [
            ("abc-def", "abc", true),
            ("abc-def", "c-d", true),
            ("abc-def", "def", true),
            ("abc-def", "abc-def", true),
            ("abc-def", "abd", false),
            ("abc", "abcd", false),
            ("abc", "", false),
            ("", "a", false),
        ] {
            let seen = contains(haystack.as_bytes(), needle.as_bytes());
            assert_eq!(seen, found, "{needle:?} in {haystack:?}");
        }
    }

    #[test]
    fn a_refused_result_is_a_python_error_with_a_fixed_message() {
        let cfg = SubprocessConfig::from_lookup(&|_: &str| None::<String>).unwrap();
        let e = RawFailure::Refused.into_run_error(&cfg);
        assert_eq!(e, PythonRunError::Python(REFUSED_MESSAGE.to_string()));
        assert!(REFUSED_MESSAGE.starts_with("Python execution error:"));
    }

    /// A dispatcher over `ex` whose template start is under way and holds the
    /// state lock, as `Dispatcher::build` leaves it.
    async fn starting(ex: SubprocessExecutor) -> Dispatcher {
        let ex = Arc::new(ex);
        let background = ex.clone();
        tokio::spawn(async move { background.warm().await });
        tokio::task::yield_now().await;
        assert!(ex.state.try_lock().is_err(), "the start is not under way");
        let local = Arc::new(InProcessExecutor);
        Dispatcher::new(ModesPolicy::Restricted, Duration::from_secs(60), ex, local)
    }

    /// A template that reports its mounts capability disabled still serves plain
    /// calls (it printed READY); a call that asks for mounts is refused with a
    /// typed error naming the reason, and no child is started for it.
    #[tokio::test]
    async fn a_template_with_mounts_disabled_refuses_only_mounts_calls() {
        let (dir, ex) = fake("echo MOUNTS_DISABLED staging_unusable\necho READY\nexec sleep 30");
        let mut cfg = ex.config().clone();
        cfg.staging_root = Some(dir.path().to_path_buf());
        let ex = SubprocessExecutor::unchecked(cfg, Duration::from_secs(60)).unwrap();
        ex.warm().await.unwrap();
        let mounts = CallMounts {
            stage_id: "a".repeat(32),
            out_mb: 1,
        };
        let req = PythonRunRequest {
            code: "output = 1".into(),
            mode: "none".into(),
            timeout: None,
            inputs: Default::default(),
        };
        let err = ex.run_staged(req, mounts).await.unwrap_err().to_string();
        assert!(
            err.contains("disabled") && err.contains("staging_unusable"),
            "{err}"
        );
        assert_eq!(starts(&dir), 1, "no second template for the refusal");
    }

    #[test]
    fn only_a_plain_reason_is_read_from_the_template() {
        assert_eq!(
            mounts_disabled_reason("MOUNTS_DISABLED staging_unusable\n"),
            Some("staging_unusable".into())
        );
        for bad in [
            "READY",
            "MOUNTS_DISABLED",
            "MOUNTS_DISABLED a b",
            "MOUNTS_DISABLED ../x",
            "MOUNTS_DISABLED ",
            &format!("MOUNTS_DISABLED {}", "x".repeat(65)),
        ] {
            assert_eq!(mounts_disabled_reason(bad), None, "{bad}");
        }
    }

    /// Waiting joins the start under way: it returns once the template has
    /// printed READY, and starts no second template.
    #[tokio::test]
    async fn waiting_returns_once_the_template_is_ready() {
        // `${0%/*}` is the directory of the script.
        let (dir, ex) = fake("sleep 1\necho > \"${0%/*}/ready\"\necho READY\nexec sleep 30");
        let d = starting(ex).await;
        d.wait_until_ready().await.unwrap();
        assert!(dir.path().join("ready").exists(), "returned before READY");
        assert_eq!(starts(&dir), 1);
    }

    /// A start that fails is what the wait returns; it is not retried.
    #[tokio::test]
    async fn waiting_returns_the_error_of_a_failed_start() {
        let (dir, ex) = fake("sleep 1\nexit 3");
        let d = starting(ex).await;
        let e = d.wait_until_ready().await.unwrap_err();
        assert!(
            e.starts_with("PythonExecutorError:") && e.contains("could not start"),
            "{e}"
        );
        assert_eq!(starts(&dir), 1);
    }

    #[tokio::test]
    async fn a_template_that_exits_after_ready_is_replaced_at_once() {
        let (dir, ex) = fake("echo READY");
        ex.warm().await.unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        ex.warm().await.unwrap();
        assert_eq!(starts(&dir), 2);
    }

    /// The confirmation that lets a root reader trust the output volume: a stop that
    /// fails is "not confirmed", a uid nothing runs as is confirmed at once, and a
    /// uid something still runs as is not, however long it waits (a zombie aside).
    #[test]
    fn a_uid_is_confirmed_gone_only_when_the_stop_works_and_nothing_runs_as_it() {
        let ok: fn(u32) -> Result<(), NotStopped> = |_| Ok(());
        let fails: fn(u32) -> Result<(), NotStopped> = |_| Err(NotStopped::default());
        let nobody = 4_000_000_001u32;
        let me = unsafe { libc::geteuid() };
        assert!(!confirm_uid_gone(fails, nobody, Duration::from_millis(50)));
        assert!(confirm_uid_gone(ok, nobody, Duration::from_millis(50)));
        let started = Instant::now();
        assert!(
            !confirm_uid_gone(ok, me, Duration::from_millis(150)),
            "this very process runs as {me}"
        );
        assert!(started.elapsed() >= Duration::from_millis(150));
        assert!(uid_has_processes(me) && !uid_has_processes(nobody));
    }
}
