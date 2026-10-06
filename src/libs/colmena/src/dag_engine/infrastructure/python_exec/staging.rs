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
/// The output directory is a volume of its own with a size, no bigger than
/// `out_mb` MiB and not the volume that holds the call directory. This is what
/// bounds what a call can write: the jail refuses a call whose output is
/// anything else. [`StagedCall`] makes it a tmpfs.
#[cfg(target_os = "linux")]
pub fn check_out_volume(dirs: &CallDirs, out_mb: u64) -> io::Result<()> {
    let unbounded = |why| refused(io::ErrorKind::PermissionDenied, why);
    let mut fs: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstatfs(dirs.out.as_raw_fd(), &mut fs) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let bytes = (fs.f_blocks as u128) * (fs.f_bsize as u128);
    if bytes == 0 || bytes > u128::from(out_mb) << 20 {
        return Err(unbounded("out is larger than the declared bound"));
    }
    let dev = |fd: &OwnedFd| {
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        (unsafe { libc::fstat(fd.as_raw_fd(), &mut st) } == 0).then_some(st.st_dev as u64)
    };
    match (dev(&dirs.out), dev(&dirs.call)) {
        (Some(out), Some(call)) if out != call => Ok(()),
        _ => Err(unbounded("out shares the volume of the call directory")),
    }
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
}

#[cfg(target_os = "linux")]
impl StagedCall {
    /// Creates `<root>/<fresh id>/{data,out}` under a root that is a plain
    /// absolute path (no link anywhere in it) and mounts the output volume.
    pub fn create(root: &Path, out_mb: u64) -> io::Result<Self> {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
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
        let _ = self.release();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{symlink, DirBuilderExt, MetadataExt};

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

    /// Between the check and the bind a path can be swapped for a link. The
    /// descriptors opened before the swap keep naming the directories that
    /// were checked: the jail binds those, never the path again.
    #[test]
    fn the_descriptors_keep_the_checked_directories_after_a_swap() {
        let f = fixture();
        let dirs = open_call_dirs(&f.root, "c1").unwrap();
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        unsafe { libc::fstat(dirs.data.as_raw_fd(), &mut st) };
        let checked = (st.st_dev as u64, st.st_ino as u64);

        let data = f.root.join("c1").join("data");
        let evil = f.root.join("evil");
        std::fs::DirBuilder::new()
            .mode(0o755)
            .create(&evil)
            .unwrap();
        std::fs::rename(&data, f.root.join("c1").join("data-old")).unwrap();
        symlink(&evil, &data).unwrap();

        unsafe { libc::fstat(dirs.data.as_raw_fd(), &mut st) };
        assert_eq!((st.st_dev as u64, st.st_ino as u64), checked);
        let evil_ino = std::fs::metadata(&evil).unwrap().ino();
        assert_ne!(st.st_ino as u64, evil_ino);
    }
}
