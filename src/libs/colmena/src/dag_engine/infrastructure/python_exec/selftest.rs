//! Proves each layer of the process jail by its effect, in a throwaway child,
//! before the template accepts work. A layer that does not hold stops the
//! template: there is no partially isolated mode. Each check is reported as
//! fields only: a layer name, whether it held and a fixed reason code.

use super::child::CallHeader;
use super::jail::{self, JailSpec, CHANNEL_FD, DEFAULT_HIDDEN, NOFILE, NPROC};
use super::staging::{unescape_mount_point, StagedCall, OUT_MAX_INODES};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::os::unix::fs::{FileTypeExt, MetadataExt};
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
    mounts: None,
};
/// The layers a staging root adds to a complete report, checked from inside a
/// call that asks for mounts; a report without a staging root has none of them.
pub const MOUNT_LAYERS: &[&str] = &[
    "mount_data_readonly",
    "mount_out_bounded",
    "staging_root_hidden",
];
/// The output volume of the probe's call, in MiB, and the files its data
/// directory holds: one world-writable file and one world-writable directory,
/// so that only the mount, not a file mode, can stop a write.
const SELF_TEST_OUT_MB: u64 = 1;
const CANARY: &str = "canary";
const CANARY_CONTENT: &str = "canary";
/// A probe still running after this counts as failed.
const PROBE_TIMEOUT: Duration = Duration::from_secs(30);
const CONNECT_TIMEOUT: Duration = Duration::from_millis(500);
/// Every layer a complete report carries, the template's own check last. A
/// report with any other set of layers fails.
pub const LAYERS: &[&str] = &[
    "descriptors",
    "identity",
    "no_new_privs",
    "parent_death_signal",
    "limit_memory",
    "limit_cpu",
    "limit_file_size",
    "limit_open_files",
    "limit_processes",
    "limit_core",
    "namespace_network",
    "namespace_mount",
    "namespace_ipc",
    "namespace_uts",
    "proc_processes",
    "proc_entries",
    "hidden_paths",
    "private_tmp",
    "network_dns",
    "network_loopback",
    "network_link_local",
    "network_public",
    "network_interfaces",
    "syscall_filter_sockets",
    "syscall_filter_processes",
    "host_mounts",
];
/// What the probe may map after the template measures itself and before the
/// jail measures the probe.
const MEMORY_SLACK: u64 = 16 << 20;
/// Each namespace the jail enters, by its entry in `/proc/self/ns`.
const NAMESPACES: [(&str, &str); 4] = [
    ("namespace_network", "net"),
    ("namespace_mount", "mnt"),
    ("namespace_ipc", "ipc"),
    ("namespace_uts", "uts"),
];
/// Tunnel devices a kernel with their modules loaded creates, down, in every
/// network namespace.
const FALLBACK_TUNNELS: &[&str] = &[
    "tunl0", "gre0", "gretap0", "erspan0", "ip_vti0", "sit0", "ip6tnl0", "ip6gre0", "ip6_vti0",
];

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

/// The layers a complete report carries for this jail: the fixed ones, and the
/// mount ones only when a staging root is configured.
fn expected_layers(spec: &JailSpec) -> BTreeSet<&'static str> {
    let mounts = spec.staging_root.is_some().then_some(MOUNT_LAYERS);
    LAYERS
        .iter()
        .chain(mounts.into_iter().flatten())
        .copied()
        .collect()
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

fn failure(layer: &str, reason: &str, errno: Option<i32>) -> LayerCheck {
    LayerCheck {
        layer: layer.into(),
        ok: false,
        reason: reason.into(),
        errno,
    }
}

/// What the template sees before the fork, for the probe to compare with.
struct Before {
    /// A listener in the template's network namespace.
    loopback_port: u16,
    tmp_dev: u64,
    /// The hidden paths to check, each directory with its device.
    hidden: Vec<(PathBuf, Option<u64>)>,
    /// The inode of each of [`NAMESPACES`].
    namespaces: Vec<u64>,
    /// The lines of the mount table.
    mounts: usize,
    /// What the template maps, which the jail's memory budget goes on top of.
    vm_size: u64,
}

fn namespace_inode(ns: &str) -> io::Result<u64> {
    Ok(std::fs::metadata(format!("/proc/self/ns/{ns}"))?.ino())
}

/// The mounts of this namespace, not counting those under `except`: with a
/// staging root, calls in flight mount and unmount their output volumes there
/// while a template starts (a restart, a second slot warming), and those are not
/// mounts the probe left behind.
fn mount_count(except: Option<&Path>) -> io::Result<usize> {
    let table = std::fs::read_to_string("/proc/self/mountinfo")?;
    Ok(mounts_outside(&table, except))
}

/// The lines of a `mountinfo` table whose mount point is not at or below `except`
/// (a path test, not a prefix test: `/srv/stx` is not below `/srv/st`).
fn mounts_outside(table: &str, except: Option<&Path>) -> usize {
    let under = |line: &str| {
        let point = line.split(' ').nth(4).map(unescape_mount_point);
        matches!((except, point), (Some(root), Some(p)) if Path::new(&p).starts_with(root))
    };
    table.lines().filter(|l| !under(l)).count()
}

/// Whether a `mountinfo` table shows no mount added since `before` were counted:
/// the mounts at or below `except` (the staging root, where calls in flight mount
/// and unmount their volumes at any moment) are not the probe's and not counted.
/// A pure function of the table, so the rule is proven without any timing.
fn mounts_unchanged(table: &str, except: Option<&Path>, before: usize) -> bool {
    mounts_outside(table, except) <= before
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

/// Hard limits, which the process cannot raise, at most what the jail sets:
/// memory is the call's budget on top of what the template maps.
fn limits(spec: &JailSpec, template_vm: u64) -> Vec<LayerCheck> {
    let file = spec.tmp_mb.saturating_mul(1 << 20);
    let budget = HEADER.memory_mb.saturating_mul(1 << 20);
    let memory = template_vm
        .saturating_add(budget)
        .saturating_add(MEMORY_SLACK);
    let bounds: [(&str, libc::__rlimit_resource_t, u64); 6] = [
        ("limit_memory", libc::RLIMIT_AS, memory),
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

/// Each namespace the jail enters is the probe's own, not the template's.
fn namespaces(before: &[u64]) -> impl Iterator<Item = LayerCheck> + '_ {
    NAMESPACES.iter().zip(before).map(|((layer, ns), ino)| {
        let own = namespace_inode(ns).is_ok_and(|i| i != *ino);
        check(layer, own, "own", "shared")
    })
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

/// Whether `p` sits under one of `covers` other than itself: a path there is
/// hidden by that cover already, so the self-test does not check it on its
/// own account. Lexical (a prefix test, no filesystem access), which is
/// enough because a configured path cannot carry a `..` component.
fn nested_under<'a>(p: &Path, mut covers: impl Iterator<Item = &'a Path>) -> bool {
    covers.any(|c| c != p && p.starts_with(c))
}

/// Every hidden path: the defaults, then the configured ones.
fn all_hidden(spec: &JailSpec) -> impl Iterator<Item = PathBuf> + '_ {
    let defaults = DEFAULT_HIDDEN.iter().map(PathBuf::from);
    let staging = spec.staging_root.iter().cloned();
    defaults
        .chain(spec.hide_paths.iter().cloned())
        .chain(staging)
}

/// An entry of the fresh `/proc`, checked as `proc_entries`.
fn in_proc(path: &Path) -> bool {
    path.starts_with("/proc") && path != Path::new("/proc")
}

/// The hidden paths the template sees, each directory with its device, which
/// its cover replaces. Not those under `/tmp` or under another hidden path:
/// that cover hides them already.
fn hidden_before(spec: &JailSpec) -> Vec<(PathBuf, Option<u64>)> {
    let all: Vec<PathBuf> = all_hidden(spec).filter(|p| !in_proc(p)).collect();
    let tmp = Path::new("/tmp");
    let covers = || all.iter().map(PathBuf::as_path).chain([tmp]);
    let nested = |p: &Path| nested_under(p, covers());
    let seen = |p: &PathBuf| {
        let meta = std::fs::metadata(p).ok()?;
        Some((p.clone(), meta.is_dir().then(|| meta.dev())))
    };
    let checked = all.iter().filter(|p| p.as_path() != tmp && !nested(p));
    checked.filter_map(seen).collect()
}

/// Nothing of a hidden path shows: a directory is another filesystem, its
/// cover; anything else is the null device, which does not open there. A
/// path gone since the template looked is covered from above.
fn hidden_paths(hidden: &[(PathBuf, Option<u64>)]) -> LayerCheck {
    let shown = |(path, dev): &(PathBuf, Option<u64>)| {
        let meta = match std::fs::metadata(path) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => return None,
            Err(_) => return Some("unverified"),
            Ok(meta) => meta,
        };
        let covered = match dev {
            Some(dev) => meta.dev() != *dev,
            None => {
                meta.file_type().is_char_device()
                    && meta.rdev() == libc::makedev(1, 3)
                    && std::fs::File::open(path).is_err()
            }
        };
        (!covered).then_some("uncovered")
    };
    let reason = hidden.iter().find_map(shown).unwrap_or("covered");
    outcome("hidden_paths", reason, "covered")
}

/// Four network routes, each checked on its own. The loopback listener is the
/// template's: only a network namespace of the probe's own keeps it
/// unreachable. With the syscall filter, `socket()` is refused before any
/// route is tried; the namespace itself is proven by `namespace_network` and
/// `network_interfaces`.
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

/// Interface names in `/proc/net/dev`'s content: two header lines, then one
/// `name: counters` line per interface. Pure so CI can exercise it on fixed
/// text without a real `/proc`.
fn interface_names(dev: &str) -> Vec<&str> {
    dev.lines()
        .skip(2)
        .map(|l| l.split(':').next().unwrap_or("").trim())
        .collect()
}

/// Only loopback, and the kernel's own per-namespace fallback tunnel devices.
fn only_loopback(names: &[&str]) -> bool {
    let allowed = |n: &&str| *n == "lo" || FALLBACK_TUNNELS.contains(n);
    names.contains(&"lo") && names.iter().all(allowed)
}

/// The probe's network namespace has loopback and no interface besides the
/// tunnel devices a kernel may create in any namespace. Read from `/proc`,
/// this needs no socket.
fn interfaces() -> LayerCheck {
    let Ok(dev) = std::fs::read_to_string("/proc/net/dev") else {
        return outcome("network_interfaces", "unlisted", "loopback_only");
    };
    let reason = if only_loopback(&interface_names(&dev)) {
        "loopback_only"
    } else {
        "other_interface"
    };
    outcome("network_interfaces", reason, "loopback_only")
}

/// The syscall filter refuses a socket of any kind and a new process, each
/// with EPERM. The probe is single-threaded, so it may fork; a process that
/// does start exits at once and is reaped before the report.
fn syscall_filter() -> [LayerCheck; 2] {
    let refused = |rc: libc::c_long| {
        rc == -1 && io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    };
    let fd = unsafe { libc::socket(libc::AF_INET, libc::SOCK_DGRAM, 0) };
    let sockets = refused(fd.into());
    if fd >= 0 {
        unsafe { libc::close(fd) };
    }
    // x86_64 kernels may also take the same call by its x32 number. The
    // filter answers it with EPERM; a kernel without x32 would answer ENOSYS
    // itself, which does not count.
    #[cfg(target_arch = "x86_64")]
    let sockets = {
        let x32 = libc::SYS_socket | 0x4000_0000;
        let domain = libc::c_long::from(libc::AF_INET);
        let kind = libc::c_long::from(libc::SOCK_DGRAM);
        let zero: libc::c_long = 0;
        let fd = unsafe { libc::syscall(x32, domain, kind, zero) };
        let held = refused(fd);
        if fd >= 0 {
            unsafe { libc::close(fd as libc::c_int) };
        }
        sockets && held
    };
    let pid = unsafe { libc::fork() };
    if pid == 0 {
        unsafe { libc::_exit(0) };
    }
    let processes = refused(pid.into());
    if pid > 0 {
        wait(pid);
    }
    [
        check("syscall_filter_sockets", sockets, "refused", "allowed"),
        check("syscall_filter_processes", processes, "refused", "allowed"),
    ]
}

fn flags_of(path: &Path) -> Option<u64> {
    let c = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).ok()?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    (unsafe { libc::statvfs(c.as_ptr(), &mut st) } == 0).then_some(st.f_flag)
}

fn refused_as(result: io::Result<()>, errno: i32) -> bool {
    result.is_err_and(|e| e.raw_os_error() == Some(errno))
}

/// The data directory is read-only AT THE MOUNT: the world-writable file and
/// directory cannot be written, the flags say so, and the content is readable.
fn data_mount(dir: &Path) -> LayerCheck {
    let layer = "mount_data_readonly";
    if std::fs::read_to_string(dir.join(CANARY)).ok().as_deref() != Some(CANARY_CONTENT) {
        return outcome(layer, "unreadable", "read_only");
    }
    let open = std::fs::OpenOptions::new()
        .write(true)
        .open(dir.join(CANARY));
    let create = std::fs::File::create(dir.join("sub").join("probe")).map(|_| ());
    let blocked = refused_as(open.map(|_| ()), libc::EROFS) && refused_as(create, libc::EROFS);
    let _ = std::fs::remove_file(dir.join("sub").join("probe"));
    if !blocked {
        return outcome(layer, "writable", "read_only");
    }
    let wanted = libc::ST_RDONLY | libc::ST_NOSUID | libc::ST_NODEV | libc::ST_NOEXEC;
    let flags = flags_of(dir).unwrap_or(0);
    outcome(
        layer,
        if flags & wanted == wanted {
            "read_only"
        } else {
            "flags_missing"
        },
        "read_only",
    )
}

/// The output directory takes a file, is the size the trusted side gave it, stops
/// a write past it, and is `nosuid`, `nodev` and `noexec`.
fn out_mount(dir: &Path) -> LayerCheck {
    let layer = "mount_out_bounded";
    let file = dir.join("probe");
    if std::fs::write(&file, b"x").is_err() {
        return outcome(layer, "unwritable", "bounded");
    }
    let past = std::fs::write(&file, vec![0u8; (SELF_TEST_OUT_MB as usize + 1) << 20]);
    let _ = std::fs::remove_file(&file);
    let mut fs: libc::statvfs = unsafe { std::mem::zeroed() };
    let c = std::ffi::CString::new(dir.as_os_str().as_encoded_bytes()).unwrap_or_default();
    let read = unsafe { libc::statvfs(c.as_ptr(), &mut fs) } == 0;
    let size = (fs.f_blocks as u64).saturating_mul(fs.f_frsize as u64);
    if !read || size != SELF_TEST_OUT_MB << 20 || !refused_as(past, libc::ENOSPC) {
        return outcome(layer, "unbounded", "bounded");
    }
    if !inodes_bounded(dir) {
        return outcome(layer, "unbounded", "bounded");
    }
    let wanted = libc::ST_NOSUID | libc::ST_NODEV | libc::ST_NOEXEC;
    let flags = fs.f_flag;
    let held = flags & wanted == wanted && flags & libc::ST_RDONLY == 0;
    outcome(
        layer,
        if held { "bounded" } else { "flags_missing" },
        "bounded",
    )
}

/// Empty files stop at the inode bound: creating more than [`OUT_MAX_INODES`]
/// fails with `ENOSPC` no later than that, and what was made is removed.
fn inodes_bounded(dir: &Path) -> bool {
    let probe = dir.join("inodes");
    if std::fs::create_dir(&probe).is_err() {
        return false;
    }
    let mut made = 0u64;
    let failure = loop {
        match std::fs::File::create(probe.join(made.to_string())) {
            Ok(_) => made += 1,
            Err(e) => break Some(e),
        }
        if made > OUT_MAX_INODES + 8 {
            break None;
        }
    };
    let _ = std::fs::remove_dir_all(&probe);
    failure.is_some_and(|e| e.raw_os_error() == Some(libc::ENOSPC)) && made <= OUT_MAX_INODES
}

/// The staging root shows nothing: absent, or an empty cover.
fn staging_hidden(root: &Path) -> LayerCheck {
    check("staging_root_hidden", covered(root), "covered", "visible")
}

fn probe(spec: &JailSpec, before: &Before) -> Vec<LayerCheck> {
    // First, while nothing else the probe opens is open.
    let mut out = vec![descriptors(), identity(jail::uid_for(spec, SELF_TEST_SLOT))];
    out.extend(privileges());
    out.extend(limits(spec, before.vm_size));
    out.extend(namespaces(&before.namespaces));
    out.push(own_processes_only());
    let proc_covered = all_hidden(spec).filter(|p| in_proc(p)).all(|p| covered(&p));
    out.push(check("proc_entries", proc_covered, "covered", "readable"));
    out.push(hidden_paths(&before.hidden));
    let in_tmp = std::env::current_dir().is_ok_and(|d| d == Path::new("/tmp"));
    let fresh = std::fs::metadata("/tmp").is_ok_and(|m| m.dev() != before.tmp_dev);
    out.push(check("private_tmp", in_tmp && fresh, "private", "shared"));
    out.extend(network(before.loopback_port));
    out.push(interfaces());
    out.extend(syscall_filter());
    if let Some(root) = &spec.staging_root {
        out.push(data_mount(Path::new("/data")));
        out.push(out_mount(Path::new("/out")));
        out.push(staging_hidden(root));
    }
    out
}

/// The forked child: enters the jail, probes it and reports on fd 3. That
/// stays the channel whichever layer fails, so `enter` gets a copy of it.
fn probe_in_child(
    spec: &JailSpec,
    before: &Before,
    header: &CallHeader,
    channel: UnixStream,
) -> bool {
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
    let checks = match jail::enter(spec, header, unsafe { UnixStream::from_raw_fd(copy) }) {
        // What it returns is fd 3, written through below.
        Ok(channel) => {
            let _ = channel.into_raw_fd();
            probe(spec, before)
        }
        Err(e) => vec![failure(e.layer, "not_applied", e.source.raw_os_error())],
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

fn setup_error(reason: &str, e: io::Error) -> Vec<LayerCheck> {
    vec![failure("self_test", reason, e.raw_os_error())]
}

/// What [`run`] reads before it forks. Three of its reads get a reason of
/// their own so their failure is distinguishable from the rest: a namespace
/// inode, the mount table and the template's own memory size. Anything else
/// that fails here (the channel pair, the loopback listener, its port, /tmp's
/// device) stays `setup_failed`.
fn setup(
    spec: &JailSpec,
) -> Result<(TcpListener, Before, UnixStream, UnixStream), Vec<LayerCheck>> {
    let (ours, theirs) = UnixStream::pair().map_err(|e| setup_error("setup_failed", e))?;
    // Open until the probe ends and out of its reach. Opened after the pair,
    // so never at fd 3: it is also a descriptor the jail must close.
    let listener =
        TcpListener::bind(("127.0.0.1", 0)).map_err(|e| setup_error("setup_failed", e))?;
    let loopback_port = listener
        .local_addr()
        .map_err(|e| setup_error("setup_failed", e))?
        .port();
    let tmp_dev = std::fs::metadata("/tmp")
        .map_err(|e| setup_error("setup_failed", e))?
        .dev();
    let namespaces = NAMESPACES
        .iter()
        .map(|(_, ns)| namespace_inode(ns))
        .collect::<io::Result<_>>()
        .map_err(|e| setup_error("namespace_unreadable", e))?;
    let mounts = mount_count(spec.staging_root.as_deref())
        .map_err(|e| setup_error("mounts_unreadable", e))?;
    let vm_size = jail::vm_size_bytes().map_err(|e| setup_error("vm_size_unreadable", e))?;
    let before = Before {
        loopback_port,
        tmp_dev,
        hidden: hidden_before(spec),
        namespaces,
        mounts,
        vm_size,
    };
    Ok((listener, before, ours, theirs))
}

/// A staged call for the probe: the files [`data_mount`] looks for.
fn stage_probe(root: &Path) -> io::Result<StagedCall> {
    use std::os::unix::fs::PermissionsExt;
    let staged = StagedCall::create(root, SELF_TEST_OUT_MB)?;
    let data = staged.data_dir();
    std::fs::write(data.join(CANARY), CANARY_CONTENT)?;
    std::fs::set_permissions(data.join(CANARY), std::fs::Permissions::from_mode(0o666))?;
    std::fs::create_dir(data.join("sub"))?;
    std::fs::set_permissions(data.join("sub"), std::fs::Permissions::from_mode(0o777))?;
    Ok(staged)
}

/// Forks a child that enters the jail as [`SELF_TEST_SLOT`] and checks each
/// layer; `Ok` only when every check held. Only for a single-threaded
/// process: the child allocates after the fork.
pub fn run(spec: &JailSpec) -> Result<Vec<LayerCheck>, Vec<LayerCheck>> {
    let (listener, before, mut ours, theirs) = setup(spec)?;
    // With a staging root the probe is a call that asks for mounts, staged like
    // any other and removed after the probe, before the host's mounts are read.
    let staged = match &spec.staging_root {
        Some(root) => Some(stage_probe(root).map_err(|e| setup_error("staging_failed", e))?),
        None => None,
    };
    let header = CallHeader {
        mounts: staged.as_ref().map(StagedCall::mounts),
        ..HEADER
    };
    let pid = match unsafe { libc::fork() } {
        -1 => {
            let e = io::Error::last_os_error();
            return Err(vec![failure("self_test", "fork_failed", e.raw_os_error())]);
        }
        0 => {
            drop(ours);
            let sent = catch_unwind(AssertUnwindSafe(|| {
                probe_in_child(spec, &before, &header, theirs)
            }));
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
    drop(staged);
    let mut checks: Vec<LayerCheck> = serde_json::from_slice(&report).unwrap_or_default();
    let reported = exited && !checks.is_empty();
    // The probe's mounts stayed where it made them: the template's /tmp and
    // mount table are as they were.
    let kept = std::fs::metadata("/tmp").is_ok_and(|m| m.dev() == before.tmp_dev)
        && std::fs::read_to_string("/proc/self/mountinfo")
            .is_ok_and(|t| mounts_unchanged(&t, spec.staging_root.as_deref(), before.mounts));
    checks.push(check("host_mounts", kept, "unchanged", "mounts_leaked"));
    let layers: BTreeSet<&str> = checks.iter().map(|c| c.layer.as_str()).collect();
    let complete = layers == expected_layers(spec);
    if !reported {
        checks.push(failure("self_test", "no_report", None));
    } else if !complete {
        // Either a layer is absent (a jail layer failed before the probe
        // could run at all) or the set carries one the template does not
        // know, which is just as much a report it cannot trust.
        checks.push(failure("self_test", "incomplete_report", None));
    }
    if checks.iter().all(|c| c.ok) {
        Ok(checks)
    } else {
        Err(checks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The probes are exercised in this process, as root, on real mounts: each
    /// must hold on a correct mount and name what is wrong on an incorrect one.
    fn enabled() -> bool {
        std::env::var("COLMENA_PYEXEC_JAIL_TESTS").as_deref() == Ok("1")
    }

    fn bind(src: &Path, dst: &Path, extra: libc::c_ulong) {
        jail::mount(Some(src.to_str().unwrap()), dst, None, libc::MS_BIND, None).unwrap();
        let flags = libc::MS_REMOUNT | libc::MS_BIND | extra;
        jail::mount(None, dst, None, flags, None).unwrap();
    }

    struct Unmount(PathBuf);
    impl Drop for Unmount {
        fn drop(&mut self) {
            let c = std::ffi::CString::new(self.0.to_str().unwrap()).unwrap();
            unsafe { libc::umount2(c.as_ptr(), libc::MNT_DETACH) };
        }
    }

    fn data_fixture(dir: &Path) {
        std::fs::write(dir.join(CANARY), CANARY_CONTENT).unwrap();
        std::fs::set_permissions(
            dir.join(CANARY),
            std::os::unix::fs::PermissionsExt::from_mode(0o666),
        )
        .unwrap();
        std::fs::create_dir(dir.join("sub")).unwrap();
        std::fs::set_permissions(
            dir.join("sub"),
            std::os::unix::fs::PermissionsExt::from_mode(0o777),
        )
        .unwrap();
    }

    #[test]
    fn the_data_probe_holds_on_a_read_only_mount_and_names_what_is_wrong_otherwise() {
        if !enabled() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let (src, dst) = (tmp.path().join("src"), tmp.path().join("dst"));
        std::fs::create_dir(&src).unwrap();
        std::fs::create_dir(&dst).unwrap();
        data_fixture(&src);
        let safe = libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC;
        let _u = Unmount(dst.clone());
        // Not a mount at all: world-writable files are writable.
        assert_eq!(data_mount(&src).reason, "writable");
        bind(&src, &dst, safe | libc::MS_RDONLY);
        let held = data_mount(&dst);
        assert!(held.ok, "{held:?}");
        // Read-only but missing a flag.
        bind(
            &src,
            &dst,
            libc::MS_RDONLY | libc::MS_NOSUID | libc::MS_NODEV,
        );
        assert_eq!(data_mount(&dst).reason, "flags_missing");
        // Every flag but read-only.
        bind(&src, &dst, safe);
        assert_eq!(data_mount(&dst).reason, "writable");
        assert_eq!(data_mount(&tmp.path().join("absent")).reason, "unreadable");
    }

    #[test]
    fn the_out_probe_holds_on_a_bound_volume_and_names_what_is_wrong_otherwise() {
        if !enabled() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let out = tmp.path().join("out");
        std::fs::create_dir(&out).unwrap();
        let safe = libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC;
        let size = format!("size={SELF_TEST_OUT_MB}m,nr_inodes={OUT_MAX_INODES},mode=1777");
        let no_inode_bound = format!("size={SELF_TEST_OUT_MB}m,mode=1777");
        let _u = Unmount(out.clone());
        // A directory on a big volume: writable and not bounded.
        assert_eq!(out_mount(&out).reason, "unbounded");
        // Right size and flags, but the number of files is not bounded.
        let unbounded = Some(no_inode_bound.as_str());
        jail::mount(Some("tmpfs"), &out, Some("tmpfs"), safe, unbounded).unwrap();
        assert_eq!(out_mount(&out).reason, "unbounded");
        let c = std::ffi::CString::new(out.to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::umount2(c.as_ptr(), libc::MNT_DETACH) }, 0);
        jail::mount(Some("tmpfs"), &out, Some("tmpfs"), safe, Some(&size)).unwrap();
        let held = out_mount(&out);
        assert!(held.ok, "{held:?}");
        let umount = || {
            let c = std::ffi::CString::new(out.to_str().unwrap()).unwrap();
            assert_eq!(unsafe { libc::umount2(c.as_ptr(), libc::MNT_DETACH) }, 0);
        };
        umount();
        // Right size, missing flags.
        jail::mount(Some("tmpfs"), &out, Some("tmpfs"), 0, Some(&size)).unwrap();
        assert_eq!(out_mount(&out).reason, "flags_missing");
        umount();
        // Read-only output: nothing can be written.
        let ro = safe | libc::MS_RDONLY;
        jail::mount(Some("tmpfs"), &out, Some("tmpfs"), ro, Some(&size)).unwrap();
        assert_eq!(out_mount(&out).reason, "unwritable");
    }

    #[test]
    fn the_staging_probe_holds_only_on_a_covered_directory() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(staging_hidden(&tmp.path().join("absent")).ok);
        assert!(staging_hidden(tmp.path()).ok);
        std::fs::write(tmp.path().join("visible"), "x").unwrap();
        assert_eq!(staging_hidden(tmp.path()).reason, "visible");
    }

    /// The layers a staging root adds are fixed: three, and the 26 of before are unchanged.
    #[test]
    fn the_expected_layers_grow_by_three_only_with_a_staging_root() {
        let spec = |staging_root| JailSpec {
            uid_base: 1,
            tmp_mb: 1,
            hide_paths: vec![],
            staging_root,
        };
        assert_eq!(expected_layers(&spec(None)).len(), 26);
        let with = expected_layers(&spec(Some("/x".into())));
        assert_eq!(with.len(), 29);
        assert!(MOUNT_LAYERS.iter().all(|l| with.contains(l)));
        assert!(LAYERS.iter().all(|l| with.contains(l)));
    }

    // A real `/proc/net/dev`'s two header lines, then one `name: counters`
    // line per interface — exercised here on fixed text so CI covers this
    // parsing without root or a jail container.
    const HEADER: &str = "Inter-|   Receive                                                |  Transmit\n face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed\n";

    fn dev(interfaces: &str) -> String {
        format!("{HEADER}{interfaces}")
    }

    #[test]
    fn loopback_alone_is_recognized() {
        let dev = dev("    lo: 100 1 0 0 0 0 0 0  100 1 0 0 0 0 0 0\n");
        let names = interface_names(&dev);
        assert_eq!(names, ["lo"]);
        assert!(only_loopback(&names));
    }

    #[test]
    fn a_fallback_tunnel_device_does_not_fail_the_check() {
        let dev = dev("    lo: 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0\n\
              tunl0: 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0\n\
            ip6tnl0: 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0\n");
        assert!(only_loopback(&interface_names(&dev)));
    }

    #[test]
    fn another_interface_fails_the_check() {
        let dev = dev(
            "    lo: 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0\n  eth0: 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0\n",
        );
        assert!(!only_loopback(&interface_names(&dev)));
    }

    #[test]
    fn no_loopback_at_all_fails_the_check() {
        let dev = dev("  eth0: 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0\n");
        assert!(!only_loopback(&interface_names(&dev)));
        assert!(!only_loopback(&[]));
    }

    #[test]
    fn mounts_under_the_staging_root_are_not_counted() {
        let table_lines = |except: Option<&Path>| mount_count(except).unwrap();
        assert!(table_lines(None) > 0);
        assert_eq!(table_lines(Some(Path::new("/"))), 0);
        // Two reads of a table other tests of this process may be changing: the
        // counts agree on some attempt (an exclusion of a root that holds nothing
        // changes nothing).
        let agree =
            (0..50).any(|_| table_lines(None) == table_lines(Some(Path::new("/nonexistent-root"))));
        assert!(agree);
        assert_eq!(unescape_mount_point("/a\\040b\\134c"), "/a b\\c");
    }

    /// The check of a template that started while calls were mounting: mounts that
    /// come and go under the staging root change nothing, one anywhere else is a
    /// leak. Deterministic: the "later" table is given, not raced.
    #[test]
    fn a_mount_under_the_staging_root_appearing_later_is_not_a_leak() {
        let line = |point: &str| format!("36 1 0:30 / {point} rw,nosuid - tmpfs tmpfs rw\n");
        let root = Path::new("/srv/st");
        let first: String = ["/", "/proc", "/tmp"].iter().map(|p| line(p)).collect();
        let before = mounts_outside(&first, Some(root));
        assert_eq!(before, 3);
        let with_call = format!(
            "{first}{}{}",
            line("/srv/st/ab12/out"),
            line("/srv/st/cd34/out")
        );
        assert!(mounts_unchanged(&with_call, Some(root), before));
        // The same table with no staging root configured counts them: a leak.
        assert!(!mounts_unchanged(&with_call, None, before));
        // A mount outside the root is a leak, whatever else comes and goes.
        let leaked = format!("{with_call}{}", line("/mnt/x"));
        assert!(!mounts_unchanged(&leaked, Some(root), before));
        // Calls that finished meanwhile are not a leak either.
        assert!(mounts_unchanged(&first, Some(root), before));
    }

    /// Only the mounts at or below the root are left out: a sibling sharing the
    /// prefix, the root's parent and an escaped path are told apart.
    #[test]
    fn the_exclusion_is_by_path_not_by_prefix() {
        let line = |point: &str| format!("36 1 0:30 / {point} rw,nosuid - tmpfs tmpfs rw\n");
        let table: String = [
            "/",
            "/srv",
            "/srv/st",
            "/srv/st/a/out",
            "/srv/stx/out",
            "/srv/st\\040y/out",
        ]
        .iter()
        .map(|p| line(p))
        .collect();
        let root = Path::new("/srv/st");
        // Outside: "/", "/srv", "/srv/stx/out", "/srv/st y/out" (a different dir).
        assert_eq!(mounts_outside(&table, Some(root)), 4);
        assert_eq!(mounts_outside(&table, None), 6);
    }

    #[test]
    fn a_path_under_a_cover_other_than_itself_is_nested() {
        let covers = [Path::new("/var/tmp"), Path::new("/tmp")];
        assert!(nested_under(Path::new("/var/tmp/x"), covers.into_iter()));
        assert!(nested_under(Path::new("/tmp/x/y"), covers.into_iter()));
    }

    #[test]
    fn a_cover_itself_and_an_unrelated_path_are_not_nested() {
        let covers = [Path::new("/var/tmp"), Path::new("/tmp")];
        assert!(!nested_under(Path::new("/var/tmp"), covers.into_iter()));
        assert!(!nested_under(Path::new("/etc"), covers.into_iter()));
        assert!(!nested_under(Path::new("/var/tmpfoo"), covers.into_iter()));
    }
}
