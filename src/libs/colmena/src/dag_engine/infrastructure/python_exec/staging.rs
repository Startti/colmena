//! Per-call staging directories for runs that carry prepared data (dark behind
//! `COLMENA_LARGE_TABULAR`).
//!
//! Layout, created and cleaned by the trusted side only:
//!
//! ```text
//! <staging root>/<call id>/data   read-only for the call; the trusted side fills it
//! <staging root>/<call id>/out    a tmpfs of its own, size-bound, where the call writes
//! ```
//!
//! The jail never receives a path. It receives the call id, which it checks
//! against a fixed charset, and derives both directories itself, walking every
//! component with `O_NOFOLLOW` and keeping the descriptors it ends up with:
//! what gets bound is what those descriptors name, whatever the paths turn
//! into afterwards. Nothing the sandboxed program writes is ever used to
//! build or to follow a path on the trusted side.

use std::ffi::CString;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::path::{Component, Path};

/// Directory of a call holding the prepared tables, mounted read-only.
pub const DATA_NAME: &str = "data";
/// Directory of a call the program writes to, a size-bound tmpfs.
pub const OUT_NAME: &str = "out";
/// Longest call id the jail accepts.
pub const STAGE_ID_MAX: usize = 64;
/// The largest output volume of a call, in MiB: the design's output cap (the
/// 1 GiB upload cap). A ceiling, not a recommendation: the final value waits for
/// the instance memory measurement (spike item 5).
pub const OUT_MB_MAX: u64 = 1024;
/// Staged volumes in flight on one executor: the design's heavy-call
/// concurrency (`PYEXEC_SLOTS` 2, one heavy slot) rounded up to the slots, and
/// their total, the design's `V = D_max + OUT_MAX` of 2,048 MiB. Estimates from
/// the design (D11), NOT measured: the final values need spike item 5.
pub const STAGED_VOLUMES_MAX: usize = 2;
pub const STAGED_OUT_MIB_MAX: u64 = 2048;

/// A volume size the trusted side may create and the jail may bind: not zero (a
/// tmpfs with no size is unlimited) and not above [`OUT_MB_MAX`].
pub fn valid_out_mb(mb: u64) -> bool {
    (1..=OUT_MB_MAX).contains(&mb)
}

/// Why a staged call was not made. The text is what a caller shows.
#[derive(Debug)]
pub enum StageError {
    InvalidSize(u64),
    OverBudget {
        volumes: usize,
        mib: u64,
        max_volumes: usize,
        max_mib: u64,
    },
    NoStagingRoot,
    Io(io::Error),
}

impl std::fmt::Display for StageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StageError::InvalidSize(mb) => write!(
                f,
                "PythonExecutorError: an output volume of {mb} MiB is not allowed (1 to {OUT_MB_MAX} MiB)"
            ),
            StageError::OverBudget { volumes, mib, max_volumes, max_mib } => write!(
                f,
                "PythonExecutorError: too many staged volumes in flight ({volumes} of {max_volumes} volumes, {mib} of {max_mib} MiB); retry later"
            ),
            StageError::NoStagingRoot => {
                write!(f, "PythonExecutorError: this executor has no staging directory configured")
            }
            StageError::Io(e) => write!(f, "PythonExecutorError: cannot stage the call: {e}"),
        }
    }
}

impl std::error::Error for StageError {}

impl From<io::Error> for StageError {
    fn from(e: io::Error) -> Self {
        StageError::Io(e)
    }
}

/// How many staged volumes, and how many MiB of them, may be in flight at once.
#[derive(Debug)]
pub struct StagingBudget {
    max_volumes: usize,
    max_mib: u64,
    used: std::sync::Mutex<(usize, u64)>,
}

/// One call's share of a [`StagingBudget`], given back on drop.
#[derive(Debug)]
pub struct Reservation {
    budget: std::sync::Arc<StagingBudget>,
    mib: u64,
}

impl StagingBudget {
    pub fn new(max_volumes: usize, max_mib: u64) -> Self {
        StagingBudget {
            max_volumes,
            max_mib,
            used: std::sync::Mutex::new((0, 0)),
        }
    }

    /// Volumes and MiB in flight now.
    pub fn in_flight(&self) -> (usize, u64) {
        *self.used.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Takes a share for one volume of `mib`, or says why not; a refusal holds nothing.
    pub fn reserve(this: &std::sync::Arc<Self>, mib: u64) -> Result<Reservation, StageError> {
        if !valid_out_mb(mib) {
            return Err(StageError::InvalidSize(mib));
        }
        let mut used = this.used.lock().unwrap_or_else(|e| e.into_inner());
        let (volumes, total) = *used;
        if volumes + 1 > this.max_volumes || total.saturating_add(mib) > this.max_mib {
            return Err(StageError::OverBudget {
                volumes,
                mib: total,
                max_volumes: this.max_volumes,
                max_mib: this.max_mib,
            });
        }
        *used = (volumes + 1, total + mib);
        Ok(Reservation {
            budget: this.clone(),
            mib,
        })
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        let mut used = self.budget.used.lock().unwrap_or_else(|e| e.into_inner());
        *used = (used.0.saturating_sub(1), used.1.saturating_sub(self.mib));
    }
}

/// A call id is one path component of letters, digits, `_` and `-`: it cannot
/// be `.` or `..`, hold a separator, or start a hidden name.
pub fn valid_stage_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= STAGE_ID_MAX
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn refused(kind: io::ErrorKind, why: &'static str) -> io::Error {
    io::Error::new(kind, why)
}

/// Opens `name` below `parent` as a directory, never following a link in the
/// last component.
fn open_dir_at(parent: RawFd, name: &str) -> io::Result<OwnedFd> {
    let name = CString::new(name).map_err(|_| refused(io::ErrorKind::InvalidInput, "nul"))?;
    let flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;
    let fd = unsafe { libc::openat(parent, name.as_ptr(), flags) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

/// Opens an absolute path made of plain components one at a time, none of
/// them allowed to be a link: a configured root that has become, or runs
/// through, a link is refused, not followed.
fn open_root(root: &Path) -> io::Result<OwnedFd> {
    let mut parts = root.components();
    if parts.next() != Some(Component::RootDir) {
        return Err(refused(io::ErrorKind::InvalidInput, "root is not absolute"));
    }
    let mut dir = open_dir_at(libc::AT_FDCWD, "/")?;
    for part in parts {
        let Component::Normal(name) = part else {
            return Err(refused(io::ErrorKind::InvalidInput, "root is not plain"));
        };
        let name = name
            .to_str()
            .ok_or_else(|| refused(io::ErrorKind::InvalidInput, "root is not utf-8"))?;
        dir = open_dir_at(dir.as_raw_fd(), name)?;
    }
    Ok(dir)
}

/// A directory the trusted side made: owned by the process that opens it and
/// not writable by group or others, so nothing but that process could have put
/// something else in its place.
fn check_trusted(fd: RawFd) -> io::Result<()> {
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(fd, &mut st) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let own = st.st_uid == unsafe { libc::geteuid() };
    if !own || st.st_mode & 0o022 != 0 {
        return Err(refused(
            io::ErrorKind::PermissionDenied,
            "staged directory is not private to the executor",
        ));
    }
    Ok(())
}

/// The directories of one call, open. Holding them is what keeps a later swap
/// of a path from changing what gets bound.
#[derive(Debug)]
pub struct CallDirs {
    pub call: OwnedFd,
    pub data: OwnedFd,
    pub out: OwnedFd,
}

/// Opens `<root>/<id>/{data,out}` without following any link. `data` must be
/// private to the executor ([`check_trusted`]); `out` is the call's own
/// mount, whose checks belong to the jail.
pub fn open_call_dirs(root: &Path, id: &str) -> io::Result<CallDirs> {
    if !valid_stage_id(id) {
        return Err(refused(io::ErrorKind::InvalidInput, "invalid call id"));
    }
    let root = open_root(root)?;
    let call = open_dir_at(root.as_raw_fd(), id)?;
    check_trusted(call.as_raw_fd())?;
    let data = open_dir_at(call.as_raw_fd(), DATA_NAME)?;
    check_trusted(data.as_raw_fd())?;
    let out = open_dir_at(call.as_raw_fd(), OUT_NAME)?;
    Ok(CallDirs { call, data, out })
}

/// Most inodes the output volume of a call may hold: a program cannot fill the
/// executor's memory with empty files.
pub const OUT_MAX_INODES: u64 = 1024;
/// The octal escapes `mountinfo` uses for space, tab, newline and backslash.
pub fn unescape_mount_point(field: &str) -> String {
    [
        ("\\040", " "),
        ("\\011", "\t"),
        ("\\012", "\n"),
        ("\\134", "\\"),
    ]
    .iter()
    .fold(field.to_string(), |acc, (from, to)| acc.replace(from, to))
}

/// The mount points below `root` in a `mountinfo` table, never `root` itself,
/// children before their parents (deepest first), so that detaching them in
/// order never meets a busy parent.
pub fn mount_points_under(table: &str, root: &Path) -> Vec<std::path::PathBuf> {
    let mut points: Vec<std::path::PathBuf> = table
        .lines()
        .filter_map(|l| l.split(' ').nth(4))
        .map(|f| std::path::PathBuf::from(unescape_mount_point(f)))
        .filter(|p| p != root && p.starts_with(root))
        .collect();
    points.sort_by_key(|p| std::cmp::Reverse(p.components().count()));
    points
}

/// `f_type` of a tmpfs.
pub const TMPFS_MAGIC: i64 = 0x0102_1994;

/// What the jail reads about a call's output directory before it binds it.
#[derive(Debug, Clone, PartialEq)]
pub struct OutFacts {
    /// `f_type` of the filesystem.
    pub f_type: i64,
    /// Its size in bytes.
    pub bytes: u128,
    /// Its inode count (`f_files`).
    pub files: u64,
    /// Device of the output directory, of the call directory, and of the parent
    /// of the output directory (what `..` of the mount root reaches).
    pub dev: u64,
    pub call_dev: u64,
    pub parent_dev: u64,
}

/// The output directory must be a tmpfs of its own with a size no bigger than
/// `out_mb` MiB and an inode bound of at most [`OUT_MAX_INODES`], the root of its
/// mount (its parent is on another device) and not the volume of the call
/// directory. This is what bounds what a call can write: the jail refuses a
/// call whose output is anything else. Returns the verified size in bytes.
pub fn judge_out_volume(f: &OutFacts, out_mb: u64) -> io::Result<u64> {
    let unbounded = |why| refused(io::ErrorKind::PermissionDenied, why);
    if !valid_out_mb(out_mb) {
        return Err(refused(io::ErrorKind::InvalidInput, "invalid output size"));
    }
    if f.f_type != TMPFS_MAGIC {
        return Err(unbounded("out is not a tmpfs"));
    }
    if f.bytes == 0 {
        return Err(unbounded("out has no size"));
    }
    if f.bytes > u128::from(out_mb) << 20 {
        return Err(unbounded("out is larger than the declared bound"));
    }
    if f.files == 0 || f.files > OUT_MAX_INODES {
        return Err(unbounded("out has no inode bound within the limit"));
    }
    if f.dev == f.call_dev {
        return Err(unbounded("out shares the volume of the call directory"));
    }
    if f.dev == f.parent_dev {
        return Err(unbounded("out is not the root of its mount"));
    }
    Ok(f.bytes as u64)
}

/// Reads the facts of the call's output directory and judges them.
#[cfg(target_os = "linux")]
pub fn check_out_volume(dirs: &CallDirs, out_mb: u64) -> io::Result<u64> {
    let mut fs: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstatfs(dirs.out.as_raw_fd(), &mut fs) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let dev = |fd: RawFd| {
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        match unsafe { libc::fstat(fd, &mut st) } {
            0 => Ok(st.st_dev as u64),
            _ => Err(io::Error::last_os_error()),
        }
    };
    let parent = open_dir_at(dirs.out.as_raw_fd(), "..")?;
    let facts = OutFacts {
        f_type: fs.f_type as i64,
        bytes: (fs.f_blocks as u128) * (fs.f_bsize as u128),
        files: fs.f_files as u64,
        dev: dev(dirs.out.as_raw_fd())?,
        call_dev: dev(dirs.call.as_raw_fd())?,
        parent_dev: dev(parent.as_raw_fd())?,
    };
    judge_out_volume(&facts, out_mb)
}

/// One call's staging directories, made and removed by the trusted side. The
/// output directory is mounted as a tmpfs of `out_mb` MiB; `data` is left for
/// the trusted side to fill. Dropping it unmounts the output (its content goes
/// with the mount, without anything inside being walked) and removes the
/// directories.
#[cfg(target_os = "linux")]
#[derive(Debug)]
pub struct StagedCall {
    call: std::path::PathBuf,
    id: String,
    out_mb: u64,
    mounted: bool,
    released: bool,
    /// Given back after the volume is unmounted (fields drop after `Drop::drop`).
    _share: Option<Reservation>,
}

#[cfg(target_os = "linux")]
impl StagedCall {
    /// Creates `<root>/<fresh id>/{data,out}` under a root that is a plain
    /// absolute path (no link anywhere in it) and mounts the output volume.
    pub fn create(root: &Path, out_mb: u64) -> io::Result<Self> {
        Self::create_with(root, out_mb, None)
    }

    /// [`Self::create`] for a share already taken from a [`StagingBudget`].
    pub fn create_with(root: &Path, out_mb: u64, share: Option<Reservation>) -> io::Result<Self> {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        // Before anything is made: a size of 0 would mount a tmpfs with no limit.
        if !valid_out_mb(out_mb) {
            return Err(refused(io::ErrorKind::InvalidInput, "invalid output size"));
        }
        drop(open_root(root)?);
        let id = uuid::Uuid::new_v4().simple().to_string();
        let call = root.join(&id);
        let make = |p: &Path, mode: u32| -> io::Result<()> {
            std::fs::DirBuilder::new().mode(mode).create(p)?;
            // The umask must not decide who can read what the jail binds.
            std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode))
        };
        let mut staged = StagedCall {
            call: call.clone(),
            id,
            out_mb,
            mounted: false,
            released: false,
            _share: share,
        };
        make(&call, 0o700)?;
        make(&call.join(DATA_NAME), 0o755)?;
        make(&call.join(OUT_NAME), 0o755)?;
        let options = format!("size={out_mb}m,nr_inodes={OUT_MAX_INODES},mode=1777");
        let flags = libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC;
        super::jail::mount(
            Some("tmpfs"),
            &call.join(OUT_NAME),
            Some("tmpfs"),
            flags,
            Some(&options),
        )?;
        staged.mounted = true;
        Ok(staged)
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    /// What the header of the call that uses this staging carries.
    pub fn mounts(&self) -> super::child::CallMounts {
        super::child::CallMounts {
            stage_id: self.id.clone(),
            out_mb: self.out_mb,
        }
    }

    /// Where the trusted side puts what the call reads.
    pub fn data_dir(&self) -> std::path::PathBuf {
        self.call.join(DATA_NAME)
    }

    /// Where the call's output lives while the call runs and until release.
    pub fn out_dir(&self) -> std::path::PathBuf {
        self.call.join(OUT_NAME)
    }

    /// Unmounts the output volume and removes the directories. Nothing is
    /// removed while the volume is still mounted: deleting through a mount
    /// would walk what the program wrote. Safe to call again.
    pub fn release(&mut self) -> io::Result<()> {
        if self.released {
            return Ok(());
        }
        if self.mounted {
            let out = std::ffi::CString::new(self.out_dir().as_os_str().as_encoded_bytes())
                .map_err(|_| refused(io::ErrorKind::InvalidInput, "nul"))?;
            let busy = |rc: libc::c_int| {
                rc != 0 && io::Error::last_os_error().raw_os_error() == Some(libc::EBUSY)
            };
            let mut rc = unsafe { libc::umount2(out.as_ptr(), 0) };
            if busy(rc) {
                rc = unsafe { libc::umount2(out.as_ptr(), libc::MNT_DETACH) };
            }
            if rc != 0 {
                return Err(io::Error::last_os_error());
            }
            self.mounted = false;
        }
        let gone = |r: io::Result<()>| match r {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        };
        gone(std::fs::remove_dir(self.out_dir()))?;
        gone(std::fs::remove_dir_all(self.data_dir()))?;
        gone(std::fs::remove_dir(&self.call))?;
        self.released = true;
        Ok(())
    }
}

#[cfg(target_os = "linux")]
impl Drop for StagedCall {
    fn drop(&mut self) {
        if let Err(e) = self.release() {
            // Not swallowed: what could not be unmounted or removed stays where
            // it is (never deleted through a mount) until the startup sweep.
            tracing::warn!(
                target: crate::dag_engine::log_policy::T_PYTHON_EXEC,
                call = %self.id,
                error = %e,
                "staged call cleanup failed; left for the startup sweep"
            );
        }
    }
}

/// Whether the `out` directory of a leftover call is still a mount: it is on
/// another device than the call directory. An `out` that cannot be opened is
/// not (nothing to unmount); a call directory whose device cannot be read is
/// treated as still mounted, which leaves it alone.
#[cfg(target_os = "linux")]
fn out_is_still_mounted(out_dev: Option<u64>, call_dev: Option<u64>) -> bool {
    match (out_dev, call_dev) {
        (None, _) => false,
        (Some(_), None) => true,
        (Some(out), Some(call)) => out != call,
    }
}

/// What a sweep of the staging root did.
#[cfg(target_os = "linux")]
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SweepReport {
    /// Call directories unmounted and removed.
    pub removed: usize,
    /// Entries that are not call directories made by [`StagedCall::create`]: left alone.
    pub skipped: usize,
    /// Call directories that could not be reclaimed: logged, left in place.
    pub failed: usize,
}

/// The ids [`StagedCall::create`] generates: 32 lowercase hexadecimal digits.
#[cfg(target_os = "linux")]
fn is_generated_id(name: &str) -> bool {
    name.len() == 32
        && name
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Reclaims what an executor that was killed left in its staging root: for each
/// directory a call made (a real directory named like a generated id; a link, a
/// file or anything else is skipped and never opened), detaches its output volume
/// and, once it is no longer a mount, removes the directory (removal does not
/// follow links). Never walks a volume that is still mounted. For the root of
/// one executor only: it cannot tell a leftover from a call in flight.
#[cfg(target_os = "linux")]
pub fn sweep_staging_root(root: &Path) -> io::Result<SweepReport> {
    let rootfd = open_root(root)?;
    use crate::dag_engine::log_policy::T_PYTHON_EXEC;
    let mut report = SweepReport::default();
    for entry in std::fs::read_dir(root)? {
        let name = match entry {
            Ok(e) => e.file_name(),
            Err(_) => {
                report.failed += 1;
                continue;
            }
        };
        let Some(id) = name.to_str().filter(|n| is_generated_id(n)) else {
            report.skipped += 1;
            continue;
        };
        // A real directory only: a link by that name is not ours to follow.
        let Ok(call) = open_dir_at(rootfd.as_raw_fd(), id) else {
            tracing::warn!(target: T_PYTHON_EXEC, call = %id, "staging entry is not a directory; skipped");
            report.skipped += 1;
            continue;
        };
        let path = root.join(id);
        let out = path.join(OUT_NAME);
        if let Ok(c) = CString::new(out.as_os_str().as_encoded_bytes()) {
            // Not mounted (EINVAL) or already gone (ENOENT) is fine.
            unsafe { libc::umount2(c.as_ptr(), libc::MNT_DETACH) };
        }
        let dev = |fd: RawFd| {
            let mut st: libc::stat = unsafe { std::mem::zeroed() };
            (unsafe { libc::fstat(fd, &mut st) } == 0).then_some(st.st_dev as u64)
        };
        let out_dev = open_dir_at(call.as_raw_fd(), OUT_NAME)
            .ok()
            .and_then(|f| dev(f.as_raw_fd()));
        let still_mounted = out_is_still_mounted(out_dev, dev(call.as_raw_fd()));
        if still_mounted {
            tracing::warn!(target: T_PYTHON_EXEC, call = %id, "staged volume could not be unmounted; left in place");
            report.failed += 1;
            continue;
        }
        match std::fs::remove_dir_all(&path) {
            Ok(()) => report.removed += 1,
            Err(e) => {
                tracing::warn!(target: T_PYTHON_EXEC, call = %id, error = %e, "leftover staged call could not be removed");
                report.failed += 1;
            }
        }
    }
    if report != SweepReport::default() {
        tracing::info!(target: T_PYTHON_EXEC, removed = report.removed, skipped = report.skipped, failed = report.failed, "staging root swept");
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{symlink, DirBuilderExt};

    /// A staging root and one call directory under it, as the trusted side
    /// lays them out. The root is canonical: macOS temp dirs run through a link.
    struct Fixture {
        _tmp: tempfile::TempDir,
        root: std::path::PathBuf,
    }

    fn fixture() -> Fixture {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let call = root.join("c1");
        for p in [&call, &call.join("data"), &call.join("out")] {
            std::fs::DirBuilder::new().mode(0o755).create(p).unwrap();
        }
        Fixture { _tmp: tmp, root }
    }

    fn good_volume() -> OutFacts {
        OutFacts {
            f_type: TMPFS_MAGIC,
            bytes: 2 << 20,
            files: OUT_MAX_INODES,
            dev: 7,
            call_dev: 3,
            parent_dev: 3,
        }
    }

    /// Each fact the jail reads about the output volume is checked on its own:
    /// every variation below breaks exactly one and is refused for that.
    #[test]
    fn the_output_volume_is_judged_fact_by_fact() {
        assert_eq!(judge_out_volume(&good_volume(), 2).unwrap(), 2 << 20);
        let refused_with = |edit: &dyn Fn(&mut OutFacts), why: &str| {
            let mut f = good_volume();
            edit(&mut f);
            let e = judge_out_volume(&f, 2).unwrap_err();
            assert!(e.to_string().contains(why), "{why}: {e}");
        };
        refused_with(&|f| f.f_type = 0x858458f6, "not a tmpfs"); // ramfs
        refused_with(&|f| f.f_type = 0xEF53, "not a tmpfs"); // ext4
        refused_with(&|f| f.bytes = 0, "no size");
        refused_with(&|f| f.bytes = (2 << 20) + 1, "larger than");
        refused_with(&|f| f.files = 0, "inode");
        refused_with(&|f| f.files = OUT_MAX_INODES + 1, "inode");
        refused_with(&|f| f.files = u64::MAX, "inode");
        refused_with(&|f| f.dev = 3, "call directory");
        refused_with(&|f| f.parent_dev = 7, "root of its mount");
        let e = judge_out_volume(&good_volume(), 0).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidInput);
    }

    /// The mount points below a root, deepest first, from a mount table: not the
    /// root itself, not a sibling that merely shares its prefix, escapes decoded.
    #[test]
    fn the_mounts_under_a_root_come_deepest_first() {
        let line = |point: &str| format!("36 1 0:30 / {point} rw,nosuid - tmpfs tmpfs rw\n");
        let table: String = [
            "/",
            "/srv/st",
            "/srv/st/a/out",
            "/srv/st/b\\040c/out",
            "/srv/stx/out",
            "/srv/st/a",
            "/srv",
        ]
        .iter()
        .map(|p| line(p))
        .collect();
        let got = mount_points_under(&table, Path::new("/srv/st"));
        let want: Vec<std::path::PathBuf> = ["/srv/st/b c/out", "/srv/st/a/out", "/srv/st/a"]
            .map(Into::into)
            .into();
        assert_eq!(got.len(), 3, "{got:?}");
        assert_eq!(
            &got[..2]
                .iter()
                .cloned()
                .collect::<std::collections::BTreeSet<_>>(),
            &want[..2].iter().cloned().collect()
        );
        assert_eq!(got[2], want[2], "a parent comes after its children");
        assert_eq!(unescape_mount_point("/a\\040b\\134c"), "/a b\\c");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_leftover_out_is_mounted_when_it_is_on_another_device() {
        assert!(out_is_still_mounted(Some(7), Some(3)));
        assert!(!out_is_still_mounted(Some(3), Some(3)));
        assert!(!out_is_still_mounted(None, Some(3)));
        assert!(
            out_is_still_mounted(Some(7), None),
            "unknown: leave it alone"
        );
    }

    #[test]
    fn an_output_size_is_between_one_mib_and_the_ceiling() {
        assert!(!valid_out_mb(0));
        assert!(valid_out_mb(1));
        assert!(valid_out_mb(OUT_MB_MAX));
        assert!(!valid_out_mb(OUT_MB_MAX + 1));
        assert!(!valid_out_mb(u64::MAX));
    }

    /// Volumes in flight are counted and summed; a request over either bound is
    /// refused with a typed error and holds nothing; a drop gives it back.
    #[test]
    fn the_budget_counts_volumes_and_mebibytes_and_gives_them_back() {
        // The count bites on its own when there is plenty of room in MiB.
        let roomy = std::sync::Arc::new(StagingBudget::new(2, 1000));
        let (x, y) = (
            StagingBudget::reserve(&roomy, 1).unwrap(),
            StagingBudget::reserve(&roomy, 1).unwrap(),
        );
        assert!(matches!(
            StagingBudget::reserve(&roomy, 1).unwrap_err(),
            StageError::OverBudget {
                volumes: 2,
                mib: 2,
                ..
            }
        ));
        drop((x, y));
        let budget = std::sync::Arc::new(StagingBudget::new(2, 100));
        let a = StagingBudget::reserve(&budget, 60).unwrap();
        let b = StagingBudget::reserve(&budget, 40).unwrap();
        assert_eq!(budget.in_flight(), (2, 100));
        let third = StagingBudget::reserve(&budget, 1).unwrap_err();
        assert!(
            matches!(
                third,
                StageError::OverBudget {
                    volumes: 2,
                    mib: 100,
                    ..
                }
            ),
            "{third:?}"
        );
        drop(a);
        assert_eq!(budget.in_flight(), (1, 40));
        let too_big = StagingBudget::reserve(&budget, 61).unwrap_err();
        assert!(matches!(too_big, StageError::OverBudget { .. }));
        assert_eq!(budget.in_flight(), (1, 40), "a refusal holds nothing");
        let c = StagingBudget::reserve(&budget, 60).unwrap();
        drop((b, c));
        assert_eq!(budget.in_flight(), (0, 0));
        assert!(matches!(
            StagingBudget::reserve(&budget, 0).unwrap_err(),
            StageError::InvalidSize(0)
        ));
        let text = StageError::OverBudget {
            volumes: 2,
            mib: 100,
            max_volumes: 2,
            max_mib: 100,
        }
        .to_string();
        assert!(
            text.starts_with("PythonExecutorError:") && text.contains("retry"),
            "{text}"
        );
    }

    #[test]
    fn a_call_id_is_one_plain_component() {
        for ok in ["a", "A1_b-2", &"x".repeat(STAGE_ID_MAX)] {
            assert!(valid_stage_id(ok), "{ok}");
        }
        let long = "x".repeat(STAGE_ID_MAX + 1);
        for bad in [
            "", ".", "..", "a/b", "../c1", "a.b", ".hidden", "a b", "a\0b", "é", &long,
        ] {
            assert!(!valid_stage_id(bad), "{bad:?}");
        }
    }

    #[test]
    fn the_three_directories_open() {
        let f = fixture();
        let dirs = open_call_dirs(&f.root, "c1").unwrap();
        for fd in [&dirs.call, &dirs.data, &dirs.out] {
            let mut st: libc::stat = unsafe { std::mem::zeroed() };
            assert_eq!(unsafe { libc::fstat(fd.as_raw_fd(), &mut st) }, 0);
        }
    }

    #[test]
    fn an_invalid_id_is_refused_before_any_path_is_touched() {
        let f = fixture();
        for bad in ["../c1", "", "c1/data", "."] {
            let e = open_call_dirs(&f.root, bad).unwrap_err();
            assert_eq!(e.kind(), io::ErrorKind::InvalidInput, "{bad:?}");
        }
    }

    /// A link in place of the call directory, of `data` or of `out`, pointing at
    /// a real directory, is refused: it is not followed.
    #[test]
    fn a_link_in_any_component_is_refused() {
        for name in ["call", "data", "out"] {
            let f = fixture();
            let elsewhere = f.root.join("elsewhere");
            std::fs::DirBuilder::new()
                .mode(0o755)
                .create(&elsewhere)
                .unwrap();
            let path = match name {
                "call" => f.root.join("c1"),
                other => f.root.join("c1").join(other),
            };
            if name == "call" {
                std::fs::rename(&path, f.root.join("moved")).unwrap();
            } else {
                std::fs::remove_dir(&path).unwrap();
            }
            symlink(&elsewhere, &path).unwrap();
            assert!(
                open_call_dirs(&f.root, "c1").is_err(),
                "{name} was followed"
            );
        }
    }

    #[test]
    fn a_root_that_runs_through_a_link_is_refused() {
        let f = fixture();
        let alias = f.root.join("alias");
        symlink(f.root.join("c1"), &alias).unwrap();
        assert!(open_root(&alias).is_err());
        assert!(open_root(&alias.join("data")).is_err());
    }

    #[test]
    fn a_relative_or_dotted_root_is_refused() {
        let f = fixture();
        assert!(open_call_dirs(Path::new("relative/root"), "c1").is_err());
        let dotted = f.root.join("c1").join("..").join("c1");
        assert!(open_call_dirs(&dotted, "c1").is_err());
    }

    #[test]
    fn a_regular_file_where_data_should_be_is_refused() {
        let f = fixture();
        let data = f.root.join("c1").join("data");
        std::fs::remove_dir(&data).unwrap();
        std::fs::write(&data, b"x").unwrap();
        assert!(open_call_dirs(&f.root, "c1").is_err());
    }

    /// Anything a group or others could write in is not a directory the
    /// executor made: refused, for the call directory and for `data`.
    #[test]
    fn a_directory_others_can_write_is_refused() {
        for name in ["c1", "c1/data"] {
            let f = fixture();
            let path = f.root.join(name);
            for mode in [0o775, 0o757] {
                std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(mode))
                    .unwrap();
                let e = open_call_dirs(&f.root, "c1").unwrap_err();
                assert_eq!(e.kind(), io::ErrorKind::PermissionDenied, "{name} {mode:o}");
            }
        }
    }
}
