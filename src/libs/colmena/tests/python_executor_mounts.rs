#![cfg(target_os = "linux")]
//! The jail's run mounts, seen from outside and from inside the sandbox. Same
//! gate as the other jail suites: root and CAP_SYS_ADMIN, enabled with
//! `COLMENA_PYEXEC_JAIL_TESTS=1`.

use colmena::dag_engine::infrastructure::python_exec::staging::{
    check_out_volume, open_call_dirs, StagedCall, OUT_MAX_INODES,
};
use std::path::{Path, PathBuf};

fn enabled() -> bool {
    std::env::var("COLMENA_PYEXEC_JAIL_TESTS").as_deref() == Ok("1")
}

/// A canonical staging root of its own.
fn root() -> Option<(tempfile::TempDir, PathBuf)> {
    if !enabled() {
        eprintln!("skipped: set COLMENA_PYEXEC_JAIL_TESTS=1 (Linux, root, CAP_SYS_ADMIN)");
        return None;
    }
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    Some((tmp, root))
}

fn mounted(path: &Path) -> bool {
    let want = path.to_str().unwrap();
    std::fs::read_to_string("/proc/self/mountinfo")
        .unwrap()
        .lines()
        .any(|l| l.split(' ').nth(4) == Some(want))
}

fn statvfs(path: &Path) -> libc::statvfs {
    let c = std::ffi::CString::new(path.to_str().unwrap()).unwrap();
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    assert_eq!(unsafe { libc::statvfs(c.as_ptr(), &mut s) }, 0);
    s
}

#[test]
fn the_output_is_a_bound_tmpfs_of_its_own_and_data_is_not_a_mount() {
    let Some((_t, root)) = root() else { return };
    let staged = StagedCall::create(&root, 4).unwrap();
    assert!(mounted(&staged.out_dir()));
    assert!(!mounted(&staged.data_dir()));
    let s = statvfs(&staged.out_dir());
    assert_eq!(s.f_blocks * s.f_frsize, 4 << 20);
    let flags = s.f_flag;
    for f in [libc::ST_NOSUID, libc::ST_NODEV, libc::ST_NOEXEC] {
        assert_ne!(flags & f, 0, "flag {f}");
    }
    assert_eq!(flags & libc::ST_RDONLY, 0);
}

#[test]
fn the_output_stops_at_its_size_and_at_its_inode_count() {
    let Some((_t, root)) = root() else { return };
    let staged = StagedCall::create(&root, 1).unwrap();
    let big = staged.out_dir().join("big");
    let err = std::fs::write(&big, vec![0u8; 2 << 20]).unwrap_err();
    assert_eq!(err.raw_os_error(), Some(libc::ENOSPC));
    std::fs::remove_file(&big).unwrap();
    let mut made = 0u64;
    let failure = loop {
        match std::fs::File::create(staged.out_dir().join(format!("f{made}"))) {
            Ok(_) => made += 1,
            Err(e) => break e,
        }
        assert!(made <= OUT_MAX_INODES + 1, "the inode bound did not hold");
    };
    assert_eq!(failure.raw_os_error(), Some(libc::ENOSPC));
}

/// Dropping the call unmounts the output and removes every directory: the next
/// call starts empty and nothing is left on the staging volume.
#[test]
fn dropping_the_call_unmounts_and_removes_everything() {
    let Some((_t, root)) = root() else { return };
    let staged = StagedCall::create(&root, 1).unwrap();
    let (call_dir, out) = (root.join(staged.id()), staged.out_dir());
    std::fs::write(staged.data_dir().join("part-00000.parquet"), b"x").unwrap();
    std::fs::write(out.join("result.csv"), b"y").unwrap();
    drop(staged);
    assert!(!mounted(&out));
    assert!(!call_dir.exists(), "{call_dir:?} was left behind");
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
}

#[test]
fn release_twice_is_fine_and_each_call_has_its_own_directories() {
    let Some((_t, root)) = root() else { return };
    let mut a = StagedCall::create(&root, 1).unwrap();
    let b = StagedCall::create(&root, 1).unwrap();
    assert_ne!(a.id(), b.id());
    a.release().unwrap();
    a.release().unwrap();
    assert!(b.data_dir().is_dir());
}

#[test]
fn a_root_that_is_a_link_is_refused() {
    let Some((_t, root)) = root() else { return };
    let real = root.join("real");
    std::fs::create_dir(&real).unwrap();
    let link = root.join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    assert!(StagedCall::create(&link, 1).is_err());
    assert_eq!(std::fs::read_dir(&real).unwrap().count(), 0);
}

/// The jail binds only an output that is a volume of its own, no bigger than
/// its declared bound.
#[test]
fn an_output_that_is_not_a_bound_volume_is_refused() {
    let Some((_t, root)) = root() else { return };
    let staged = StagedCall::create(&root, 2).unwrap();
    let dirs = open_call_dirs(&root, staged.id()).unwrap();
    check_out_volume(&dirs, 2).unwrap();
    // Declared smaller than the volume really is.
    assert!(check_out_volume(&dirs, 1).is_err());
    // A plain directory on the staging volume is not a bound volume.
    let plain = root.join("plain");
    for d in ["", "data", "out"] {
        std::fs::create_dir_all(plain.join(d)).unwrap();
    }
    std::fs::set_permissions(&plain, std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
    let plain_dirs = open_call_dirs(&root, "plain").unwrap();
    assert!(check_out_volume(&plain_dirs, 1024).is_err());
}

/// Small enough to pass the size test, but it is the volume that holds the call
/// directory: refused for that reason alone.
#[test]
fn an_output_on_the_volume_of_the_call_directory_is_refused() {
    let Some((_t, root)) = root() else { return };
    struct Unmount(PathBuf);
    impl Drop for Unmount {
        fn drop(&mut self) {
            let c = std::ffi::CString::new(self.0.to_str().unwrap()).unwrap();
            unsafe { libc::umount2(c.as_ptr(), libc::MNT_DETACH) };
        }
    }
    let target = std::ffi::CString::new(root.to_str().unwrap()).unwrap();
    let rc = unsafe {
        libc::mount(
            c"tmpfs".as_ptr(),
            target.as_ptr(),
            c"tmpfs".as_ptr(),
            0,
            c"size=1m,mode=0755".as_ptr() as *const libc::c_void,
        )
    };
    assert_eq!(rc, 0, "{}", std::io::Error::last_os_error());
    let _guard = Unmount(root.clone());
    for d in ["c", "c/data", "c/out"] {
        std::fs::create_dir(root.join(d)).unwrap();
    }
    std::fs::set_permissions(
        root.join("c"),
        std::os::unix::fs::PermissionsExt::from_mode(0o700),
    )
    .unwrap();
    let dirs = open_call_dirs(&root, "c").unwrap();
    assert!(check_out_volume(&dirs, 1).is_err());
    assert!(check_out_volume(&dirs, 1024).is_err());
}
