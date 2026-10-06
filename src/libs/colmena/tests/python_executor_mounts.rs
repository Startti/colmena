#![cfg(target_os = "linux")]
//! The jail's run mounts, seen from outside and from inside the sandbox. Same
//! gate as the other jail suites: root and CAP_SYS_ADMIN, enabled with
//! `COLMENA_PYEXEC_JAIL_TESTS=1`.

use colmena::dag_engine::domain::python_executor::{PythonRunError, PythonRunRequest};
use colmena::dag_engine::infrastructure::python_exec::child::CallMounts;
use colmena::dag_engine::infrastructure::python_exec::config::SubprocessConfig;
use colmena::dag_engine::infrastructure::python_exec::protocol::CRASHED_MESSAGE;
use colmena::dag_engine::infrastructure::python_exec::staging::{
    check_out_volume, open_call_dirs, StagedCall, OUT_MAX_INODES,
};
use colmena::dag_engine::infrastructure::python_exec::subprocess::SubprocessExecutor;
use serde_json::Value;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

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

// ---------------------------------------------------------------------------
// From inside the sandbox: a call that carries prepared data, run in `none`
// mode (full Python) by the subprocess executor.
// ---------------------------------------------------------------------------

/// A staging root outside `/tmp` (the jail's own `/tmp` would hide it anyway)
/// and outside the default hidden paths, so that hiding it is the jail's doing.
struct Root {
    path: PathBuf,
}

impl Root {
    fn new() -> Option<Self> {
        if !enabled() {
            eprintln!("skipped: set COLMENA_PYEXEC_JAIL_TESTS=1 (Linux, root, CAP_SYS_ADMIN)");
            return None;
        }
        let path = PathBuf::from(format!(
            "/var/lib/colmena-mounts-test-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&path)
            .unwrap();
        Some(Root { path })
    }
}

impl Drop for Root {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn executor(root: Option<&Root>) -> SubprocessExecutor {
    pyo3::Python::initialize();
    let mut cfg = SubprocessConfig::from_lookup(&|_: &str| None::<String>).unwrap();
    cfg.bin = PathBuf::from(env!("CARGO_BIN_EXE_python_executor"));
    cfg.slots = 1;
    static NEXT: AtomicU32 = AtomicU32::new(0);
    cfg.uid_base = 60000 + 100 * NEXT.fetch_add(1, Ordering::Relaxed);
    cfg.max_response_bytes = 1 << 20;
    cfg.staging_root = root.map(|r| r.path.clone());
    SubprocessExecutor::new(cfg, Duration::from_secs(60)).unwrap()
}

fn req(code: &str) -> PythonRunRequest {
    PythonRunRequest {
        code: code.into(),
        mode: "none".into(),
        timeout: Some(Duration::from_secs(30)),
        inputs: Default::default(),
    }
}

async fn run_staged(
    ex: &SubprocessExecutor,
    mounts: CallMounts,
    code: &str,
) -> Result<Value, PythonRunError> {
    let result = ex.run_staged(req(code), mounts).await;
    result.map(|r| r.output.unwrap_or_default())
}

#[tokio::test]
async fn mounts_without_a_staging_root_are_refused() {
    let Some(_root) = Root::new() else { return };
    let ex = executor(None);
    let mounts = CallMounts {
        stage_id: "a".repeat(32),
        out_mb: 1,
    };
    let err = run_staged(&ex, mounts, "output = 1").await.unwrap_err();
    assert!(err.to_string().contains("staging"), "{err}");
}

/// Until the jail binds the prepared data, a call that asks for it ends before
/// any code runs: it never runs without what it asked for.
#[tokio::test]
async fn a_call_asking_for_mounts_ends_before_any_code_runs() {
    let Some(root) = Root::new() else { return };
    let ex = executor(Some(&root));
    let staged = StagedCall::create(&root.path, 1).unwrap();
    let err = run_staged(&ex, staged.mounts(), "output = 1")
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), CRASHED_MESSAGE);
}
