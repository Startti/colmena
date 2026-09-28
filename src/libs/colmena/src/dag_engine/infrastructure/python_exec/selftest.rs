//! Proves each layer of the process jail by its effect, in a throwaway child,
//! before the template accepts work. A layer that does not hold stops the
//! template: there is no partially isolated mode. Each check is reported as
//! fields only: a layer name, whether it held and a fixed reason code.

use super::child::CallHeader;
use super::jail::{self, JailSpec, CHANNEL_FD, DEFAULT_HIDDEN, NOFILE, NPROC};
use serde::{Deserialize, Serialize};
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::os::unix::fs::MetadataExt;
use std::os::unix::io::{FromRawFd, IntoRawFd};
use std::os::unix::net::UnixStream;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The slot the probe runs as, above any slot an executor configures.
pub const SELF_TEST_SLOT: u32 = 9999;
const HEADER: CallHeader = CallHeader {
    slot: SELF_TEST_SLOT,
    memory_mb: 256,
    cpu_secs: 5,
    max_request_bytes: 1024,
};
/// A probe still running after this counts as failed.
const PROBE_TIMEOUT: Duration = Duration::from_secs(30);
const CONNECT_TIMEOUT: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayerCheck {
    pub layer: String,
    pub ok: bool,
    /// A fixed code for what was observed, never text from the system.
    pub reason: String,
    /// The OS error of a layer that could not be applied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub errno: Option<i32>,
}

fn outcome(layer: &str, reason: &str, held: &str) -> LayerCheck {
    LayerCheck {
        layer: layer.into(),
        ok: reason == held,
        reason: reason.into(),
        errno: None,
    }
}

fn check(layer: &str, ok: bool, held: &str, failed: &str) -> LayerCheck {
    outcome(layer, if ok { held } else { failed }, held)
}

fn failure(layer: &str, reason: &str, e: &io::Error) -> LayerCheck {
    LayerCheck {
        errno: e.raw_os_error(),
        ..outcome(layer, reason, "")
    }
}

/// What the template sees before the fork, for the probe to compare with.
struct Before {
    /// A listener in the template's network namespace.
    loopback_port: u16,
    tmp_dev: u64,
}

/// Besides the channel only 0, 1 and 2 are open, and they are the null device.
fn descriptors() -> LayerCheck {
    let null = |fd| {
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        let found = unsafe { libc::fstat(fd, &mut st) } == 0;
        found && st.st_mode & libc::S_IFMT == libc::S_IFCHR && st.st_rdev == libc::makedev(1, 3)
    };
    // The listing's own descriptor is closed once it has been read.
    let listed: Option<Vec<i32>> = std::fs::read_dir("/proc/self/fd").ok().map(|d| {
        d.filter_map(|e| e.ok()?.file_name().to_str()?.parse().ok())
            .collect()
    });
    let open = |fd: &i32| unsafe { libc::fcntl(*fd, libc::F_GETFD) } != -1;
    let others = listed.is_none_or(|fds| fds.iter().any(|fd| *fd > CHANNEL_FD && open(fd)));
    let ok = (0..=2).all(null) && !others;
    check("descriptors", ok, "channel_only", "other_open")
}

/// Real, effective and saved ids are the slot's, never root's, with no
/// supplementary groups.
fn identity(id: u32) -> LayerCheck {
    let (mut ru, mut eu, mut su, mut rg, mut eg, mut sg) = (0, 0, 0, 0, 0, 0);
    let read = unsafe {
        libc::getresuid(&mut ru, &mut eu, &mut su) == 0
            && libc::getresgid(&mut rg, &mut eg, &mut sg) == 0
            && libc::getgroups(0, std::ptr::null_mut()) == 0
    };
    let ok = read && id != 0 && [ru, eu, su, rg, eg, sg].iter().all(|&x| x == id);
    check("identity", ok, "slot_uid", "not_slot_uid")
}

fn privileges() -> [LayerCheck; 2] {
    let zero: libc::c_ulong = 0;
    let no_new = unsafe { libc::prctl(libc::PR_GET_NO_NEW_PRIVS, zero, zero, zero, zero) } == 1;
    let mut signal: libc::c_int = 0;
    let at = &mut signal as *mut libc::c_int as libc::c_ulong;
    let read = unsafe { libc::prctl(libc::PR_GET_PDEATHSIG, at, zero, zero, zero) } == 0;
    let dies = read && signal == libc::SIGKILL;
    [
        check("no_new_privs", no_new, "set", "not_set"),
        check("parent_death_signal", dies, "set", "not_set"),
    ]
}

/// Hard limits, which the process cannot raise, at most what the jail sets.
fn limits(spec: &JailSpec) -> Vec<LayerCheck> {
    let file = spec.tmp_mb.saturating_mul(1 << 20);
    let bounds: [(&str, libc::__rlimit_resource_t, u64); 6] = [
        ("limit_memory", libc::RLIMIT_AS, libc::RLIM_INFINITY - 1),
        ("limit_cpu", libc::RLIMIT_CPU, HEADER.cpu_secs + 1),
        ("limit_file_size", libc::RLIMIT_FSIZE, file),
        ("limit_open_files", libc::RLIMIT_NOFILE, NOFILE),
        ("limit_processes", libc::RLIMIT_NPROC, NPROC),
        ("limit_core", libc::RLIMIT_CORE, 0),
    ];
    let bounded = |(layer, resource, max): (&str, _, u64)| {
        let mut lim = libc::rlimit {
            rlim_cur: libc::RLIM_INFINITY,
            rlim_max: libc::RLIM_INFINITY,
        };
        let read = unsafe { libc::getrlimit(resource, &mut lim) } == 0;
        check(layer, read && lim.rlim_max <= max, "bounded", "above_bound")
    };
    bounds.into_iter().map(bounded).collect()
}

/// The fresh `/proc` lists this process and no other: not the template.
fn own_processes_only() -> LayerCheck {
    let own = std::process::id().to_string();
    let template = unsafe { libc::getppid() }.to_string();
    let pids: Option<Vec<String>> = std::fs::read_dir("/proc").ok().map(|d| {
        d.filter_map(|e| e.ok()?.file_name().into_string().ok())
            .filter(|n| n.bytes().all(|b| b.is_ascii_digit()))
            .collect()
    });
    let reason = match pids {
        None => "unlisted",
        Some(p) if p.contains(&template) || Path::new("/proc").join(&template).exists() => {
            "template_listed"
        }
        Some(p) if p != [own.as_str()] => "other_listed",
        Some(_) => "own_only",
    };
    outcome("proc_processes", reason, "own_only")
}

/// Nothing of `path` shows: it is absent, an empty directory or a file that
/// does not open.
fn covered(path: &Path) -> bool {
    match std::fs::metadata(path) {
        Err(_) => true,
        Ok(m) if m.is_dir() => !std::fs::read_dir(path).is_ok_and(|mut d| d.next().is_some()),
        Ok(_) => std::fs::File::open(path).is_err(),
    }
}

/// Four network routes, each checked on its own. The loopback listener is the
/// template's: only a network namespace of the probe's own keeps it
/// unreachable.
fn network(loopback_port: u16) -> Vec<LayerCheck> {
    let resolved = ("example.com", 443).to_socket_addrs().is_ok();
    let mut out = vec![check("network_dns", !resolved, "unresolved", "resolved")];
    let routes = [
        ("network_loopback", [127, 0, 0, 1], loopback_port),
        ("network_link_local", [169, 254, 169, 254], 80),
        ("network_public", [1, 1, 1, 1], 443),
    ];
    for (layer, ip, port) in routes {
        let addr = SocketAddr::from((ip, port));
        let reached = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT).is_ok();
        out.push(check(layer, !reached, "unreachable", "connected"));
    }
    out
}

fn probe(spec: &JailSpec, before: &Before) -> Vec<LayerCheck> {
    // First, while nothing else the probe opens is open.
    let mut out = vec![descriptors(), identity(jail::uid_for(spec, SELF_TEST_SLOT))];
    out.extend(privileges());
    out.extend(limits(spec));
    out.push(own_processes_only());
    let (in_proc, others): (Vec<&str>, Vec<&str>) = DEFAULT_HIDDEN
        .iter()
        .copied()
        .partition(|p| p.starts_with("/proc/"));
    let proc_covered = in_proc.iter().all(|p| covered(Path::new(p)));
    out.push(check("proc_entries", proc_covered, "covered", "readable"));
    let mut hidden = others
        .into_iter()
        .map(PathBuf::from)
        .chain(spec.hide_paths.iter().cloned());
    let hidden_covered = hidden.all(|p| covered(&p));
    out.push(check("hidden_paths", hidden_covered, "covered", "readable"));
    let in_tmp = std::env::current_dir().is_ok_and(|d| d == Path::new("/tmp"));
    let fresh = std::fs::metadata("/tmp").is_ok_and(|m| m.dev() != before.tmp_dev);
    out.push(check("private_tmp", in_tmp && fresh, "private", "shared"));
    out.extend(network(before.loopback_port));
    out
}

/// The forked child: enters the jail, probes it and reports on fd 3. That
/// stays the channel whichever layer fails, so `enter` gets a copy of it.
fn probe_in_child(spec: &JailSpec, before: &Before, channel: UnixStream) -> bool {
    let fd = channel.into_raw_fd();
    if fd != CHANNEL_FD
        && (unsafe { libc::dup2(fd, CHANNEL_FD) } < 0 || unsafe { libc::close(fd) } < 0)
    {
        return false;
    }
    let copy = unsafe { libc::fcntl(CHANNEL_FD, libc::F_DUPFD, CHANNEL_FD + 1) };
    if copy < 0 {
        return false;
    }
    let checks = match jail::enter(spec, &HEADER, unsafe { UnixStream::from_raw_fd(copy) }) {
        // What it returns is fd 3, written through below.
        Ok(channel) => {
            let _ = channel.into_raw_fd();
            probe(spec, before)
        }
        Err(e) => vec![failure(e.layer, "not_applied", &e.source)],
    };
    let mut channel = unsafe { UnixStream::from_raw_fd(CHANNEL_FD) };
    serde_json::to_vec(&checks).is_ok_and(|report| channel.write_all(&report).is_ok())
}

fn wait(pid: libc::pid_t) -> Option<libc::c_int> {
    let mut status = 0;
    loop {
        if unsafe { libc::waitpid(pid, &mut status, 0) } == pid {
            return Some(status);
        }
        if io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
            return None;
        }
    }
}

/// Forks a child that enters the jail as [`SELF_TEST_SLOT`] and checks each
/// layer; `Ok` only when every check held. Only for a single-threaded
/// process: the child allocates after the fork.
pub fn run(spec: &JailSpec) -> Result<Vec<LayerCheck>, Vec<LayerCheck>> {
    let setup = || -> io::Result<_> {
        let (ours, theirs) = UnixStream::pair()?;
        // Open until the probe ends and out of its reach. Opened after the
        // pair, so never at fd 3: it is also a descriptor the jail must close.
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let before = Before {
            loopback_port: listener.local_addr()?.port(),
            tmp_dev: std::fs::metadata("/tmp")?.dev(),
        };
        Ok((listener, before, ours, theirs))
    };
    let (listener, before, mut ours, theirs) =
        setup().map_err(|e| vec![failure("self_test", "setup_failed", &e)])?;
    let pid = match unsafe { libc::fork() } {
        -1 => {
            let e = io::Error::last_os_error();
            return Err(vec![failure("self_test", "fork_failed", &e)]);
        }
        0 => {
            drop(ours);
            let sent = catch_unwind(AssertUnwindSafe(|| probe_in_child(spec, &before, theirs)));
            let code = i32::from(!matches!(sent, Ok(true)));
            // Dropping a panic's payload could panic again.
            std::mem::forget(sent);
            unsafe { libc::_exit(code) }
        }
        pid => pid,
    };
    drop(theirs);
    let mut report = Vec::new();
    let read = ours
        .set_read_timeout(Some(PROBE_TIMEOUT))
        .and_then(|()| ours.read_to_end(&mut report));
    if read.is_err() {
        unsafe { libc::kill(pid, libc::SIGKILL) };
    }
    let exited = wait(pid).is_some_and(|s| libc::WIFEXITED(s) && libc::WEXITSTATUS(s) == 0);
    drop(listener);
    let mut checks: Vec<LayerCheck> = serde_json::from_slice(&report).unwrap_or_default();
    if checks.is_empty() || !exited {
        checks.push(check("self_test", false, "reported", "no_report"));
    }
    // The child's mounts stayed in its own namespace.
    let kept = std::fs::metadata("/tmp").is_ok_and(|m| m.dev() == before.tmp_dev);
    checks.push(check("mount_namespace", kept, "private", "propagated"));
    if checks.iter().all(|c| c.ok) {
        Ok(checks)
    } else {
        Err(checks)
    }
}
