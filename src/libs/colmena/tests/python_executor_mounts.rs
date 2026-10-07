#![cfg(target_os = "linux")]
//! The jail's run mounts, seen from outside and from inside the sandbox. Same
//! gate as the other jail suites: root and CAP_SYS_ADMIN, enabled with
//! `COLMENA_PYEXEC_JAIL_TESTS=1`.

use colmena::dag_engine::domain::python_executor::{
    PythonExecutor, PythonRunError, PythonRunRequest,
};
use colmena::dag_engine::infrastructure::python_exec::child::CallMounts;
use colmena::dag_engine::infrastructure::python_exec::config::SubprocessConfig;
use colmena::dag_engine::infrastructure::python_exec::protocol::CRASHED_MESSAGE;
use colmena::dag_engine::infrastructure::python_exec::staging::{
    check_out_volume, open_call_dirs, StagedCall, OUT_MAX_INODES,
};
use colmena::dag_engine::infrastructure::python_exec::subprocess::SubprocessExecutor;
use serde_json::{json, Value};
use std::fs::Permissions;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
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

async fn run_plain(ex: &SubprocessExecutor, code: &str) -> Result<Value, PythonRunError> {
    ex.run(req(code))
        .await
        .map(|r| r.output.unwrap_or_default())
}

async fn run_staged(
    ex: &SubprocessExecutor,
    mounts: CallMounts,
    code: &str,
) -> Result<Value, PythonRunError> {
    let result = ex.run_staged(req(code), mounts).await;
    result.map(|r| r.output.unwrap_or_default())
}

/// Files laid out by the trusted side, which runs as root: a world-writable
/// file and a world-writable directory, so that only the mount can stop a write.
fn stage_data(staged: &StagedCall) {
    let data = staged.data_dir();
    std::fs::write(data.join("a.txt"), "hello").unwrap();
    std::fs::write(data.join("world.bin"), "orig").unwrap();
    std::fs::set_permissions(data.join("world.bin"), Permissions::from_mode(0o666)).unwrap();
    std::fs::create_dir(data.join("sub")).unwrap();
    std::fs::set_permissions(data.join("sub"), Permissions::from_mode(0o777)).unwrap();
}

#[tokio::test]
async fn the_staged_data_is_readable_and_is_all_there_is() {
    let Some(root) = Root::new() else { return };
    let ex = executor(Some(&root));
    let staged = StagedCall::create(&root.path, 1).unwrap();
    stage_data(&staged);
    let code = "import os\n\
        output = {'ls': sorted(os.listdir('/data')), 'a': open('/data/a.txt').read()}";
    let out = run_staged(&ex, staged.mounts(), code).await.unwrap();
    assert_eq!(
        out,
        json!({"ls": ["a.txt", "sub", "world.bin"], "a": "hello"})
    );
}

/// Every way to change `/data` fails with EROFS, the error of a read-only
/// MOUNT: the file and the directory are world-writable, so a file mode would
/// have let the slot's user through. The source is unchanged afterwards.
#[tokio::test]
async fn nothing_can_be_changed_under_data() {
    let Some(root) = Root::new() else { return };
    let ex = executor(Some(&root));
    let staged = StagedCall::create(&root.path, 1).unwrap();
    stage_data(&staged);
    let code = r#"
import os, errno
def attempt(f):
    try:
        f()
        return 'done'
    except OSError as e:
        return errno.errorcode.get(e.errno, e.errno)
w = '/data/world.bin'
output = {
  'write_existing': attempt(lambda: open(w, 'r+').write('x')),
  'append': attempt(lambda: open(w, 'ab').write(b'x')),
  'create_in_sub': attempt(lambda: open('/data/sub/new', 'w').write('x')),
  'mkdir_in_sub': attempt(lambda: os.mkdir('/data/sub/d')),
  'remove': attempt(lambda: os.remove(w)),
  'rename': attempt(lambda: os.rename(w, '/data/sub/moved')),
  'symlink': attempt(lambda: os.symlink('/etc', '/data/sub/l')),
  'link': attempt(lambda: os.link(w, '/data/sub/h')),
  'chmod': attempt(lambda: os.chmod(w, 0o600)),
  'utime': attempt(lambda: os.utime(w, (0, 0))),
  'truncate': attempt(lambda: os.truncate(w, 0)),
}
"#;
    let out = run_staged(&ex, staged.mounts(), code).await.unwrap();
    let attempts = [
        "write_existing",
        "append",
        "create_in_sub",
        "mkdir_in_sub",
        "remove",
        "rename",
        "symlink",
        "link",
        "chmod",
        "utime",
        "truncate",
    ];
    let expected: serde_json::Map<String, Value> = attempts
        .iter()
        .map(|k| (k.to_string(), json!("EROFS")))
        .collect();
    assert_eq!(out, Value::Object(expected));
    let source = staged.data_dir();
    assert_eq!(
        std::fs::read_to_string(source.join("world.bin")).unwrap(),
        "orig"
    );
    assert_eq!(std::fs::read_dir(source.join("sub")).unwrap().count(), 0);
}

/// The mount's own flags, as the program reads them: read-only, no setuid, no
/// device nodes, no execution.
#[tokio::test]
async fn data_is_mounted_nosuid_nodev_noexec_and_read_only() {
    let Some(root) = Root::new() else { return };
    let ex = executor(Some(&root));
    let staged = StagedCall::create(&root.path, 1).unwrap();
    let code = "import os\nf = os.statvfs('/data').f_flag\n\
        output = [bool(f & x) for x in (os.ST_RDONLY, os.ST_NOSUID, os.ST_NODEV, os.ST_NOEXEC)]";
    let out = run_staged(&ex, staged.mounts(), code).await.unwrap();
    assert_eq!(out, json!([true, true, true, true]));
}

/// From inside, the mount cannot be undone or replaced: remount, unmount, a new
/// mount on top, a new mount or user namespace (which would grant capabilities),
/// the new mount API and a chroot all fail, and the program holds no capability.
#[tokio::test]
async fn data_cannot_be_remounted_unmounted_or_replaced() {
    let Some(root) = Root::new() else { return };
    let ex = executor(Some(&root));
    let staged = StagedCall::create(&root.path, 1).unwrap();
    stage_data(&staged);
    let code = r#"
import ctypes, os, errno
libc = ctypes.CDLL(None, use_errno=True)
def err(rc):
    return 'ok' if rc == 0 else errno.errorcode.get(ctypes.get_errno(), ctypes.get_errno())
MS_REMOUNT, MS_BIND = 32, 4096
CLONE_NEWNS, CLONE_NEWUSER = 0x20000, 0x10000000
def sc(nr, *args):
    return libc.syscall(nr, *[ctypes.c_long(a) if isinstance(a, int) else a for a in args])
res = {
  'remount_rw': err(libc.mount(b'', b'/data', None, MS_REMOUNT | MS_BIND, None)),
  'umount': err(libc.umount2(b'/data', 0)),
  'umount_lazy': err(libc.umount2(b'/data', 2)),
  'tmpfs_over': err(libc.mount(b'tmpfs', b'/data', b'tmpfs', 0, None)),
  'bind_over': err(libc.mount(b'/tmp', b'/data', None, MS_BIND, None)),
  'unshare_ns': err(libc.unshare(CLONE_NEWNS)),
  'unshare_user': err(libc.unshare(CLONE_NEWUSER)),
  'chroot': err(libc.chroot(b'/tmp')),
  'fsopen': err(sc(430, b'tmpfs', 0)),
  'open_tree': err(sc(428, -100, b'/data', 1)),
  'mount_setattr': err(sc(442, -100, b'/data', 0, ctypes.byref((ctypes.c_uint64 * 4)(0, 1, 0, 0)), 32)),
  'move_mount': err(sc(429, -100, b'/data', -100, b'/tmp', 0)),
}
status = dict(l.split(':', 1) for l in open('/proc/self/status').read().splitlines() if ':' in l)
res['caps'] = [status[k].strip() for k in ('CapInh', 'CapPrm', 'CapEff', 'CapBnd', 'CapAmb')]
res['still_read_only'] = os.statvfs('/data').f_flag & os.ST_RDONLY != 0
output = res
"#;
    let out = run_staged(&ex, staged.mounts(), code).await.unwrap();
    // Inheritable, permitted, effective and ambient are empty. The bounding
    // set (index 3) is not touched by the jail, today or with mounts.
    let zero = json!("0000000000000000");
    let caps = out["caps"].as_array().unwrap();
    assert_eq!([&caps[0], &caps[1], &caps[2], &caps[4]], [&zero; 4]);
    assert_eq!(out["still_read_only"], json!(true));
    for (name, result) in out.as_object().unwrap() {
        if name == "caps" || name == "still_read_only" {
            continue;
        }
        // ENOSYS: a kernel without the new mount API, where the call is unavailable.
        let code = result.as_str().unwrap();
        assert!(["EPERM", "ENOSYS"].contains(&code), "{name}: {code}");
    }
}

/// What the program is, has and can do is the same with or without mounts: user,
/// groups, capabilities, `no_new_privs`, seccomp mode, environment, limits,
/// network and working directory. Only `/data` is added.
#[tokio::test]
async fn a_call_with_mounts_is_otherwise_the_same_sandbox() {
    let Some(root) = Root::new() else { return };
    let ex = executor(Some(&root));
    let staged = StagedCall::create(&root.path, 1).unwrap();
    let code = r#"
import os, resource, socket, errno
status = dict(l.split(':', 1) for l in open('/proc/self/status').read().splitlines() if ':' in l)
keep = ('Uid', 'Gid', 'Groups', 'CapInh', 'CapPrm', 'CapEff', 'CapBnd', 'CapAmb', 'NoNewPrivs', 'Seccomp')
limits = {n: resource.getrlimit(getattr(resource, n)) for n in dir(resource) if n.startswith('RLIMIT_')}
try:
    socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    net = 'socket'
except OSError as e:
    net = errno.errorcode[e.errno]
output = {
  'status': {k: status[k].strip() for k in keep},
  'env': dict(os.environ),
  'limits': limits,
  'net': net,
  'ifaces': sorted(l.split(':')[0].strip() for l in open('/proc/net/dev').read().splitlines()[2:]),
  'cwd': os.getcwd(),
  'tmp': os.statvfs('/tmp').f_blocks,
  'root': [d for d in sorted(os.listdir('/')) if d != 'data'],
}
"#;
    let plain = run_plain(&ex, code).await.unwrap();
    let staged_out = run_staged(&ex, staged.mounts(), code).await.unwrap();
    assert_eq!(plain["net"], json!("EPERM"));
    assert_eq!(plain["status"]["Seccomp"], json!("2"));
    assert_eq!(plain, staged_out);
}

/// A call asking for mounts without a staging root never starts a child.
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

/// A call the jail cannot trust ends the call before any code runs: the code
/// would answer `1`, the call answers with the crash text.
#[tokio::test]
async fn a_call_the_jail_cannot_trust_ends_before_any_code_runs() {
    let Some(root) = Root::new() else { return };
    let ex = executor(Some(&root));
    let staged = StagedCall::create(&root.path, 1).unwrap();
    // A `data` directory swapped for a link after the trusted side made it.
    let swapped = StagedCall::create(&root.path, 1).unwrap();
    std::fs::remove_dir(swapped.data_dir()).unwrap();
    std::os::unix::fs::symlink("/etc", swapped.data_dir()).unwrap();
    // A `data` directory anyone can write in.
    let open = StagedCall::create(&root.path, 1).unwrap();
    std::fs::set_permissions(open.data_dir(), Permissions::from_mode(0o777)).unwrap();
    let ids = [
        "../etc".to_string(),
        "nonexistent".to_string(),
        staged.id().to_string() + "/..",
        swapped.id().to_string(),
        open.id().to_string(),
    ];
    for stage_id in ids {
        let mounts = CallMounts {
            stage_id: stage_id.clone(),
            out_mb: 1,
        };
        let err = run_staged(&ex, mounts, "output = 1").await.unwrap_err();
        assert_eq!(err.to_string(), CRASHED_MESSAGE, "{stage_id}");
    }
    // The same executor still serves a valid call afterwards.
    let ok = run_staged(&ex, staged.mounts(), "output = 1").await;
    assert_eq!(ok.unwrap(), json!(1));
}
