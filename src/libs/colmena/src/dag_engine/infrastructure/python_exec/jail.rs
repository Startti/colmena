//! Isolation applied by each per-call child before it reads its request.
//! Order matters: descriptors, namespaces, mounts, identity, privileges,
//! limits. There is no syscall filter yet.

use super::child::CallHeader;
use serde::{Deserialize, Serialize};
use std::ffi::CString;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::io::{FromRawFd, IntoRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::ptr;

/// The call's connection inside the jail. Every other descriptor above the
/// standard three is closed, and those three point at `/dev/null`.
pub const CHANNEL_FD: RawFd = 3;
/// Paths every child finds covered: host directories, and the entries of
/// its fresh `/proc` that describe the machine or let it be driven, which a
/// container runtime usually masks in the host's own `/proc`.
pub const DEFAULT_HIDDEN: &[&str] = &[
    "/app",
    "/root",
    "/home",
    "/run",
    "/var/tmp",
    "/dev/shm",
    // The message queues of the IPC namespace that mounted it, not the child's.
    "/dev/mqueue",
    "/srv",
    "/mnt",
    "/proc/acpi",
    "/proc/asound",
    "/proc/bus",
    "/proc/fs",
    "/proc/irq",
    "/proc/kcore",
    "/proc/keys",
    "/proc/latency_stats",
    "/proc/sched_debug",
    "/proc/scsi",
    "/proc/sys",
    "/proc/sysrq-trigger",
    "/proc/timer_list",
    "/proc/timer_stats",
];
const MIB: u64 = 1024 * 1024;
pub(crate) const NOFILE: u64 = 256;
pub(crate) const NPROC: u64 = 64;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JailSpec {
    /// Slot `n` runs as uid and gid `uid_base + n`.
    pub uid_base: u32,
    /// Size of each child's private `/tmp`, and the largest file it may write.
    pub tmp_mb: u64,
    /// Absolute paths hidden on top of [`DEFAULT_HIDDEN`], files included.
    pub hide_paths: Vec<PathBuf>,
}

/// The layer that could not be applied, and why.
#[derive(Debug)]
pub struct JailError {
    pub layer: &'static str,
    pub source: io::Error,
}

fn at(layer: &'static str) -> impl Fn(io::Error) -> JailError {
    move |source| JailError { layer, source }
}

fn check(rc: libc::c_int) -> io::Result<libc::c_int> {
    if rc < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(rc)
    }
}

fn cstr(s: &[u8]) -> io::Result<CString> {
    CString::new(s).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))
}

fn mount(
    src: Option<&str>,
    target: &Path,
    fstype: Option<&str>,
    flags: libc::c_ulong,
    data: Option<&str>,
) -> io::Result<()> {
    let src = src.map(|s| cstr(s.as_bytes())).transpose()?;
    let fstype = fstype.map(|s| cstr(s.as_bytes())).transpose()?;
    let data = data.map(|s| cstr(s.as_bytes())).transpose()?;
    let target = cstr(target.as_os_str().as_bytes())?;
    check(unsafe {
        libc::mount(
            src.as_ref().map_or(ptr::null(), |s| s.as_ptr()),
            target.as_ptr(),
            fstype.as_ref().map_or(ptr::null(), |s| s.as_ptr()),
            flags,
            data.as_ref()
                .map_or(ptr::null(), |s| s.as_ptr() as *const libc::c_void),
        )
    })
    .map(|_| ())
}

/// `prctl` is variadic and reads its arguments as `unsigned long`, so they are
/// passed at that width.
fn prctl(option: libc::c_int, arg: libc::c_ulong) -> io::Result<()> {
    let zero: libc::c_ulong = 0;
    check(unsafe { libc::prctl(option, arg, zero, zero, zero) }).map(|_| ())
}

fn set_limit(resource: libc::__rlimit_resource_t, soft: u64, hard: u64) -> io::Result<()> {
    let lim = libc::rlimit {
        rlim_cur: soft,
        rlim_max: hard,
    };
    check(unsafe { libc::setrlimit(resource, &lim) }).map(|_| ())
}

fn vm_size_bytes() -> io::Result<u64> {
    let status = std::fs::read_to_string("/proc/self/status")?;
    status
        .lines()
        .find_map(|l| l.strip_prefix("VmSize:"))
        .and_then(|v| v.trim().trim_end_matches("kB").trim().parse::<u64>().ok())
        .map(|kb| kb * 1024)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "VmSize not found"))
}

/// Fails on any entry it cannot read: a descriptor left unlisted would stay open.
fn close_fds_above(keep: RawFd) -> io::Result<()> {
    let mut fds = Vec::new();
    for entry in std::fs::read_dir("/proc/self/fd")? {
        let name = entry?.file_name();
        let fd: RawFd = name
            .to_str()
            .and_then(|n| n.parse().ok())
            .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidData))?;
        if fd > keep {
            fds.push(fd);
        }
    }
    for fd in fds {
        unsafe { libc::close(fd) };
    }
    Ok(())
}

/// Covers `path` so that nothing of it shows: an empty read-only tmpfs over a
/// directory, the null device, which cannot be opened there, over anything
/// else. A path that does not exist is left alone.
fn hide(path: &Path) -> io::Result<()> {
    use io::ErrorKind::{NotADirectory, NotFound};
    let meta = match std::fs::metadata(path) {
        Ok(meta) => meta,
        Err(e) if matches!(e.kind(), NotFound | NotADirectory) => return Ok(()),
        Err(e) => return Err(e),
    };
    let flags = libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC | libc::MS_RDONLY;
    if meta.is_dir() {
        return mount(
            Some("tmpfs"),
            path,
            Some("tmpfs"),
            flags,
            Some("size=16k,mode=0555"),
        );
    }
    // A bind mount takes its flags from a remount.
    mount(Some("/dev/null"), path, None, libc::MS_BIND, None)?;
    mount(
        None,
        path,
        None,
        libc::MS_REMOUNT | libc::MS_BIND | flags,
        None,
    )
}

/// The uid and gid of `slot`: `u32::MAX`, which [`enter`] refuses, when the
/// sum does not fit.
pub fn uid_for(spec: &JailSpec, slot: u32) -> u32 {
    spec.uid_base.saturating_add(slot)
}

/// The id `slot` runs as, refused when it is 0, which is root, or
/// `u32::MAX`, which `setresuid` reads as "leave this id unchanged".
fn slot_id(spec: &JailSpec, slot: u32) -> io::Result<u32> {
    match uid_for(spec, slot) {
        0 | u32::MAX => Err(io::Error::from(io::ErrorKind::InvalidInput)),
        id => Ok(id),
    }
}

/// Isolates the calling process for one call and returns its connection, now
/// at [`CHANNEL_FD`]. Only for a single-threaded process that exits after the
/// call: nothing here can be undone.
pub fn enter(spec: &JailSpec, hdr: &CallHeader, conn: UnixStream) -> Result<UnixStream, JailError> {
    // First, before anything in the process changes.
    let id = slot_id(spec, hdr.slot).map_err(at("identity"))?;
    let template = unsafe { libc::getppid() };
    let template_vm = vm_size_bytes().map_err(at("limits"))?;

    // 1. Descriptors: the channel becomes fd 3, everything else above it
    //    closes, and 0/1/2 point at /dev/null so stray writes reach no log.
    let raw = conn.into_raw_fd();
    if raw != CHANNEL_FD {
        check(unsafe { libc::dup2(raw, CHANNEL_FD) }).map_err(at("descriptors"))?;
        unsafe { libc::close(raw) };
    }
    close_fds_above(CHANNEL_FD).map_err(at("descriptors"))?;
    let null = check(unsafe { libc::open(c"/dev/null".as_ptr(), libc::O_RDWR) })
        .map_err(at("descriptors"))?;
    for fd in 0..=2 {
        check(unsafe { libc::dup2(null, fd) }).map_err(at("descriptors"))?;
    }
    if null > 2 {
        unsafe { libc::close(null) };
    }

    // 2. Namespaces: an empty network namespace, private mounts, IPC and UTS.
    check(unsafe {
        libc::unshare(
            libc::CLONE_NEWNS | libc::CLONE_NEWNET | libc::CLONE_NEWIPC | libc::CLONE_NEWUTS,
        )
    })
    .map_err(at("namespaces"))?;

    // 3. Mounts: private root, a fresh /tmp as the working directory, a /proc
    //    that lists only this uid's processes, and host paths covered.
    mount(
        None,
        Path::new("/"),
        None,
        libc::MS_REC | libc::MS_PRIVATE,
        None,
    )
    .map_err(at("mounts"))?;
    mount(
        Some("tmpfs"),
        Path::new("/tmp"),
        Some("tmpfs"),
        libc::MS_NOSUID | libc::MS_NODEV,
        Some(&format!("size={}m,mode=1777", spec.tmp_mb)),
    )
    .map_err(at("mounts"))?;
    // What kernels do with `hidepid=invisible` differs. From 5.8 each proc
    // mount keeps its own options and this one applies it. Before 4.8, and
    // from 5.1 to 5.7, the value is not understood and the mount fails. From
    // 4.8 to 5.0 the mount reuses the proc superblock the PID namespace
    // already has without reading any option, so it succeeds and every
    // process stays listed; the check after the uid change fails the jail.
    mount(
        Some("proc"),
        Path::new("/proc"),
        Some("proc"),
        libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC,
        Some("hidepid=invisible"),
    )
    .map_err(at("mounts"))?;
    let hidden = DEFAULT_HIDDEN
        .iter()
        .map(PathBuf::from)
        .chain(spec.hide_paths.iter().cloned());
    for p in hidden.filter(|p| p != Path::new("/tmp")) {
        hide(&p).map_err(at("mounts"))?;
    }
    std::env::set_current_dir("/tmp").map_err(at("mounts"))?;

    // 4. Identity: one unprivileged uid/gid per slot.
    check(unsafe { libc::setgroups(0, ptr::null()) }).map_err(at("identity"))?;
    check(unsafe { libc::setresgid(id, id, id) }).map_err(at("identity"))?;
    check(unsafe { libc::setresuid(id, id, id) }).map_err(at("identity"))?;

    // 5. Privileges. The uid change clears the parent-death signal, so it is
    //    set after it; a template that exited before then sends none.
    prctl(libc::PR_SET_NO_NEW_PRIVS, 1).map_err(at("privileges"))?;
    prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL as libc::c_ulong).map_err(at("privileges"))?;
    if unsafe { libc::getppid() } != template {
        return Err(at("privileges")(io::Error::other("the template exited")));
    }
    // Only now, without root's view, does /proc show what this uid sees.
    match Path::new(&format!("/proc/{template}")).try_exists() {
        Ok(false) => {}
        Ok(true) => return Err(at("mounts")(io::Error::other("the template is listed"))),
        Err(e) => return Err(at("mounts")(e)),
    }

    // 6. Limits. Memory is a budget on top of what the template already maps.
    let as_limit = template_vm.saturating_add(hdr.memory_mb.saturating_mul(MIB));
    let file_limit = spec.tmp_mb.saturating_mul(MIB);
    set_limit(libc::RLIMIT_AS, as_limit, as_limit).map_err(at("limits"))?;
    set_limit(
        libc::RLIMIT_CPU,
        hdr.cpu_secs,
        hdr.cpu_secs.saturating_add(1),
    )
    .map_err(at("limits"))?;
    set_limit(libc::RLIMIT_FSIZE, file_limit, file_limit).map_err(at("limits"))?;
    set_limit(libc::RLIMIT_NOFILE, NOFILE, NOFILE).map_err(at("limits"))?;
    set_limit(libc::RLIMIT_NPROC, NPROC, NPROC).map_err(at("limits"))?;
    set_limit(libc::RLIMIT_CORE, 0, 0).map_err(at("limits"))?;

    Ok(unsafe { UnixStream::from_raw_fd(CHANNEL_FD) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_and_an_overflowing_slot_uid_are_refused() {
        let spec = |uid_base| JailSpec {
            uid_base,
            tmp_mb: 1,
            hide_paths: vec![],
        };
        assert_eq!(uid_for(&spec(u32::MAX - 1), 2), u32::MAX);
        for (uid_base, slot) in [(0, 0), (u32::MAX - 1, 1), (u32::MAX - 1, 2)] {
            let id = slot_id(&spec(uid_base), slot);
            assert!(id.is_err(), "{uid_base} + {slot}: {id:?}");
        }
        assert_eq!(slot_id(&spec(0), 1).unwrap(), 1);
        assert_eq!(slot_id(&spec(20000), 3).unwrap(), 20003);
        assert_eq!(slot_id(&spec(u32::MAX - 2), 1).unwrap(), u32::MAX - 1);
    }
}
