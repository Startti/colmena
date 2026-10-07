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
    check_out_volume, open_call_dirs, sweep_staging_root, StageError, StagedCall, OUT_MAX_INODES,
    OUT_MB_MAX, STAGED_VOLUMES_MAX,
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

/// A directory name no other test or earlier run used.
fn unique(prefix: &str) -> String {
    format!("{prefix}-{}", uuid::Uuid::new_v4().simple())
}

/// Removes its directory when the test ends, pass or fail.
struct Scrap(PathBuf);
impl Drop for Scrap {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn mounts_under(root: &Path) -> usize {
    std::fs::read_to_string("/proc/self/mountinfo")
        .unwrap()
        .lines()
        .filter(|l| {
            l.split(' ')
                .nth(4)
                .is_some_and(|p| Path::new(p).starts_with(root))
        })
        .count()
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
    let real = root.join(unique("real"));
    let _scrap = Scrap(real.clone());
    std::fs::create_dir(&real).unwrap();
    let link = root.join(unique("link"));
    let _scrap_link = Scrap(link.clone());
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
    let plain_id = unique("plain");
    let plain = root.join(&plain_id);
    let _scrap = Scrap(plain.clone());
    for d in ["", "data", "out"] {
        std::fs::create_dir_all(plain.join(d)).unwrap();
    }
    std::fs::set_permissions(&plain, std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
    let plain_dirs = open_call_dirs(&root, &plain_id).unwrap();
    assert!(check_out_volume(&plain_dirs, 1024).is_err());
}

/// Small enough to pass the size test, but it is the volume that holds the call
/// directory: refused for that reason alone.
#[test]
fn an_output_on_the_volume_of_the_call_directory_is_refused() {
    // Mounts a tmpfs over its own directory, which the other tests' templates
    // would count: it runs alone.
    let Some(_alone) = Root::exclusive() else {
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
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

/// The staging root every test of this suite shares, outside `/tmp` (the jail's
/// own `/tmp` would hide it anyway) and outside the default hidden paths, so that
/// hiding it is the jail's doing. One root for all, because the self-test of every
/// template ignores the mounts under ITS root and counts the rest: tests running
/// side by side in different roots would read as leaks to each other.
static SHARED_ROOT: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// The mount table is global to the suite: a self-test without a staging root
/// counts every mount, so the tests that run it hold this exclusively while the
/// tests that mount hold it shared.
static MOUNT_TABLE: std::sync::RwLock<()> = std::sync::RwLock::new(());

struct Root {
    path: PathBuf,
    exclusive: bool,
    _lock: Box<dyn std::any::Any>,
}

impl Root {
    fn new() -> Option<Self> {
        Self::with(false)
    }

    /// For the tests that read the whole mount table.
    fn exclusive() -> Option<Self> {
        Self::with(true)
    }

    fn with(exclusive: bool) -> Option<Self> {
        if !enabled() {
            eprintln!("skipped: set COLMENA_PYEXEC_JAIL_TESTS=1 (Linux, root, CAP_SYS_ADMIN)");
            return None;
        }
        let lock: Box<dyn std::any::Any> = match exclusive {
            true => Box::new(MOUNT_TABLE.write().unwrap_or_else(|e| e.into_inner())),
            false => Box::new(MOUNT_TABLE.read().unwrap_or_else(|e| e.into_inner())),
        };
        let path = SHARED_ROOT.get_or_init(|| {
            let path = PathBuf::from("/var/lib/colmena-mounts-test");
            let made = std::fs::DirBuilder::new().mode(0o700).create(&path);
            assert!(made.is_ok() || path.is_dir(), "{made:?}");
            path
        });
        Some(Root {
            path: path.clone(),
            exclusive,
            _lock: lock,
        })
    }
}

/// An executor with no staging root. Its template counts EVERY mount of the host
/// while it starts, so only a test that holds the mount table alone may have one:
/// any other test mounting an output volume at that moment would read as a leak.
fn executor_without_root(alone: &Root) -> SubprocessExecutor {
    assert!(
        alone.exclusive,
        "an unrooted executor needs Root::exclusive()"
    );
    build_executor(None)
}

fn executor(root: Option<&Root>) -> SubprocessExecutor {
    build_executor(root.map(|r| r.path.clone()))
}

/// A range of slot uids no other executor or self-test of this process uses: one
/// counter for all of them, a hundred uids apiece.
fn next_uid_base() -> u32 {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    30000 + 100 * NEXT.fetch_add(1, Ordering::Relaxed)
}

fn build_executor(staging_root: Option<PathBuf>) -> SubprocessExecutor {
    pyo3::Python::initialize();
    let mut cfg = SubprocessConfig::from_lookup(&|_: &str| None::<String>).unwrap();
    cfg.bin = PathBuf::from(env!("CARGO_BIN_EXE_python_executor"));
    cfg.slots = 1;
    cfg.uid_base = next_uid_base();
    cfg.max_response_bytes = 1 << 20;
    cfg.staging_root = staging_root;
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

/// The shared staging root, with the suite's lock held for the test.
fn root() -> Option<(Root, PathBuf)> {
    let root = Root::new()?;
    let path = root.path.clone();
    Some((root, path))
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
def err(call):
    # errno is only meaningful after a failure, and a stale one must not be read as
    # a result: clear it first; a call that returns a descriptor (fsopen,
    # open_tree) succeeded, and the descriptor is closed.
    ctypes.set_errno(0)
    rc = call()
    if rc >= 0:
        if rc > 2:
            try:
                os.close(rc)
            except OSError:
                pass
        return 'SUCCEEDED'
    return errno.errorcode.get(ctypes.get_errno(), ctypes.get_errno())
MS_REMOUNT, MS_BIND = 32, 4096
CLONE_NEWNS, CLONE_NEWUSER = 0x20000, 0x10000000
def sc(nr, *args):
    return libc.syscall(nr, *[ctypes.c_long(a) if isinstance(a, int) else a for a in args])
res = {
  'remount_rw': err(lambda: libc.mount(b'', b'/data', None, MS_REMOUNT | MS_BIND, None)),
  'umount': err(lambda: libc.umount2(b'/data', 0)),
  'umount_lazy': err(lambda: libc.umount2(b'/data', 2)),
  'tmpfs_over': err(lambda: libc.mount(b'tmpfs', b'/data', b'tmpfs', 0, None)),
  'bind_over': err(lambda: libc.mount(b'/tmp', b'/data', None, MS_BIND, None)),
  'unshare_ns': err(lambda: libc.unshare(CLONE_NEWNS)),
  'unshare_user': err(lambda: libc.unshare(CLONE_NEWUSER)),
  'chroot': err(lambda: libc.chroot(b'/tmp')),
  'fsopen': err(lambda: sc(430, b'tmpfs', 0)),
  'open_tree': err(lambda: sc(428, -100, b'/data', 1)),
  'mount_setattr': err(lambda: sc(442, -100, b'/data', 0, ctypes.byref((ctypes.c_uint64 * 4)(0, 1, 0, 0)), 32)),
  'move_mount': err(lambda: sc(429, -100, b'/data', -100, b'/tmp', 0)),
  'fsconfig': err(lambda: sc(431, 0, 0, 0, 0, 0)),
  'fsmount': err(lambda: sc(432, 0, 0, 0)),
  'fspick': err(lambda: sc(433, -100, b'/data', 0)),
  'open_tree_attr': err(lambda: sc(467, -100, b'/data', 0, 0, 0)),
}
status = dict(l.split(':', 1) for l in open('/proc/self/status').read().splitlines() if ':' in l)
res['caps'] = [status[k].strip() for k in ('CapInh', 'CapPrm', 'CapEff', 'CapBnd', 'CapAmb')]
res['still_read_only'] = os.statvfs('/data').f_flag & os.ST_RDONLY != 0
output = res
"#;
    let out = run_staged(&ex, staged.mounts(), code).await.unwrap();
    // All five capability sets are empty, the bounding set included.
    let zero = json!("0000000000000000");
    let caps = out["caps"].as_array().unwrap();
    assert_eq!(caps.iter().collect::<Vec<_>>(), [&zero; 5]);
    assert_eq!(out["still_read_only"], json!(true));
    // Every mount call is refused with EPERM, ENOSYS never accepted: for the
    // eight new-API calls the FILTER answers (EPERM) even on a kernel that lacks
    // the call (see `seccomp::tests` for the proof with the capability present).
    for (name, result) in out.as_object().unwrap() {
        if name == "caps" || name == "still_read_only" {
            continue;
        }
        assert_eq!(result.as_str().unwrap(), "EPERM", "{name}");
    }
}

/// With a staging root the bounding set is empty inside the jail, for a call
/// with mounts and a call without; without one the jail is today's, with the
/// bounding set untouched.
#[tokio::test]
async fn the_bounding_set_is_empty_inside_a_staged_jail_and_untouched_otherwise() {
    let Some(root) = Root::exclusive() else {
        return;
    };
    let code = "status = dict(l.split(':', 1) for l in open('/proc/self/status').read().splitlines() if ':' in l)\n\
        output = {'bnd': status['CapBnd'].strip(), 'nnp': status['NoNewPrivs'].strip()}";
    let staged_ex = executor(Some(&root));
    let staged = StagedCall::create(&root.path, 1).unwrap();
    let empty = json!({"bnd": "0000000000000000", "nnp": "1"});
    assert_eq!(
        run_staged(&staged_ex, staged.mounts(), code).await.unwrap(),
        empty
    );
    assert_eq!(run_plain(&staged_ex, code).await.unwrap(), empty);
    let today = run_plain(&executor_without_root(&root), code)
        .await
        .unwrap();
    assert_ne!(
        today["bnd"],
        json!("0000000000000000"),
        "today's jail never cleared it"
    );
    assert_eq!(today["nnp"], json!("1"));
}

/// What the program is, has and can do is the same with or without mounts: user,
/// groups, capabilities, `no_new_privs`, seccomp mode, environment, limits,
/// network and working directory. Only `/data` and `/out` are added.
#[tokio::test]
async fn a_call_with_mounts_is_otherwise_the_same_sandbox() {
    let Some(root) = Root::new() else { return };
    let ex = executor(Some(&root));
    // A larger volume than the default 64 MiB file limit, so the limit is compared.
    let staged = StagedCall::create(&root.path, 100).unwrap();
    let code = r#"
import os, resource, socket, errno
status = dict(l.split(':', 1) for l in open('/proc/self/status').read().splitlines() if ':' in l)
keep = ('Uid', 'Gid', 'Groups', 'CapInh', 'CapPrm', 'CapEff', 'CapBnd', 'CapAmb', 'NoNewPrivs', 'Seccomp')
limits = {n: resource.getrlimit(getattr(resource, n)) for n in dir(resource) if n.startswith('RLIMIT_') and n != 'RLIMIT_FSIZE'}
fsize = resource.getrlimit(resource.RLIMIT_FSIZE)[1] >> 20
try:
    socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    net = 'socket'
except OSError as e:
    net = errno.errorcode[e.errno]
output = {
  'status': {k: status[k].strip() for k in keep},
  'env': dict(os.environ),
  'limits': limits,
  'fsize_mb': fsize,
  'net': net,
  'ifaces': sorted(l.split(':')[0].strip() for l in open('/proc/net/dev').read().splitlines()[2:]),
  'cwd': os.getcwd(),
  'tmp': os.statvfs('/tmp').f_blocks,
  'root': [d for d in sorted(os.listdir('/')) if d not in ('data', 'out')],
}
"#;
    let mut plain = run_plain(&ex, code).await.unwrap();
    let mut staged_out = run_staged(&ex, staged.mounts(), code).await.unwrap();
    assert_eq!(plain["net"], json!("EPERM"));
    assert_eq!(plain["status"]["Seccomp"], json!("2"));
    // The one thing that differs: the largest file follows the volume.
    assert_eq!(plain["fsize_mb"], json!(64));
    assert_eq!(staged_out["fsize_mb"], json!(100));
    plain.as_object_mut().unwrap().remove("fsize_mb");
    staged_out.as_object_mut().unwrap().remove("fsize_mb");
    assert_eq!(plain, staged_out);
}

/// A call asking for mounts without a staging root never starts a child.
#[tokio::test]
async fn mounts_without_a_staging_root_are_refused() {
    let Some(root) = Root::exclusive() else {
        return;
    };
    let ex = executor_without_root(&root);
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
/// Only this call's directory is reachable: its bind. The staging root is
/// covered, the call directory and another call's files are nowhere, and `..`
/// of `/data` is the jail's root, not the directory the data came from.
#[tokio::test]
async fn nothing_but_this_calls_data_is_reachable() {
    let Some(root) = Root::new() else { return };
    let ex = executor(Some(&root));
    let mine = StagedCall::create(&root.path, 1).unwrap();
    let other = StagedCall::create(&root.path, 1).unwrap();
    std::fs::write(mine.data_dir().join("mine.txt"), "m").unwrap();
    std::fs::write(other.data_dir().join("secret-other.txt"), "s").unwrap();
    let code = format!(
        r#"
import os
root, mine, other = {root:?}, {mine:?}, {other:?}
found = []
for d, dirs, files in os.walk('/', onerror=lambda e: None):
    dirs[:] = [x for x in dirs if os.path.join(d, x) not in ('/proc', '/sys', '/dev')]
    if 'secret-other.txt' in files:
        found.append(d)
output = {{
  'root_listing': os.listdir(root),
  'my_dir_exists': os.path.exists(root + '/' + mine),
  'other_dir_exists': os.path.exists(root + '/' + other),
  'found_other_file': found,
  'dotdot_is_root': sorted(os.listdir('/data/..')) == sorted(os.listdir('/')),
}}
"#,
        root = root.path.to_str().unwrap(),
        mine = mine.id(),
        other = other.id()
    );
    let out = run_staged(&ex, mine.mounts(), &code).await.unwrap();
    assert_eq!(
        out,
        json!({"root_listing": [], "my_dir_exists": false, "other_dir_exists": false,
               "found_other_file": [], "dotdot_is_root": true})
    );
}

/// With a staging root configured, even a call without mounts cannot see it.
#[tokio::test]
async fn the_staging_root_is_hidden_from_a_call_without_mounts() {
    let Some(root) = Root::new() else { return };
    let ex = executor(Some(&root));
    let staged = StagedCall::create(&root.path, 1).unwrap();
    std::fs::write(staged.data_dir().join("a.txt"), "hello").unwrap();
    let code = format!(
        "import os\nroot = {:?}\noutput = [os.listdir(root), os.path.exists(root + '/' + {:?}), os.listdir('/data') if os.path.exists('/data') else [], os.listdir('/out') if os.path.exists('/out') else []]",
        root.path.to_str().unwrap(),
        staged.id()
    );
    let out = run_plain(&ex, &code).await.unwrap();
    assert_eq!(out, json!([[], false, [], []]));
}

// ---------------------------------------------------------------------------
// The output volume, from inside.
// ---------------------------------------------------------------------------

/// `/out` takes files, the trusted side finds them where it staged them, and the
/// next call starts with an empty one.
#[tokio::test]
async fn out_is_writable_and_private_to_each_call() {
    let Some(root) = Root::new() else { return };
    let ex = executor(Some(&root));
    let first = StagedCall::create(&root.path, 2).unwrap();
    let write = "import os\nopen('/out/result.csv', 'w').write('a,b\\n1,2\\n')\noutput = os.listdir('/out')";
    let out = run_staged(&ex, first.mounts(), write).await.unwrap();
    assert_eq!(out, json!(["result.csv"]));
    let seen = std::fs::read_to_string(first.out_dir().join("result.csv")).unwrap();
    assert_eq!(seen, "a,b\n1,2\n");
    let second = StagedCall::create(&root.path, 2).unwrap();
    let list = "import os\noutput = os.listdir('/out')";
    assert_eq!(
        run_staged(&ex, second.mounts(), list).await.unwrap(),
        json!([])
    );
}

/// The volume is the one the trusted side sized: its flags, its size and the
/// limits on one file and on the number of files, as the program sees them.
#[tokio::test]
async fn out_is_bounded_and_mounted_nosuid_nodev_noexec() {
    let Some(root) = Root::new() else { return };
    let ex = executor(Some(&root));
    let staged = StagedCall::create(&root.path, 2).unwrap();
    let code = r#"
import os, errno
st = os.statvfs('/out')
def attempt(f):
    try:
        f()
        return 'done'
    except OSError as e:
        return errno.errorcode.get(e.errno, e.errno)
def big():
    with open('/out/big', 'wb') as f:
        f.write(b'x' * (3 << 20))
def many():
    for i in range(2000):
        open('/out/f%d' % i, 'w').close()
output = {
  'flags': [bool(st.f_flag & x) for x in (os.ST_RDONLY, os.ST_NOSUID, os.ST_NODEV, os.ST_NOEXEC)],
  'size': st.f_blocks * st.f_frsize,
  'big': attempt(big),
  'many': attempt(many),
  'mknod': attempt(lambda: os.mknod('/out/dev', 0o600 | 0o020000, os.makedev(1, 3))),
  'hardlink_from_data': attempt(lambda: os.link('/data/a.txt', '/out/h')),
}
"#;
    std::fs::write(staged.data_dir().join("a.txt"), "hello").unwrap();
    let out = run_staged(&ex, staged.mounts(), code).await.unwrap();
    assert_eq!(
        out,
        json!({"flags": [false, true, true, true], "size": 2 << 20, "big": "ENOSPC",
               "many": "ENOSPC", "mknod": "EPERM", "hardlink_from_data": "EXDEV"})
    );
}

/// Nothing the program wrote is followed on the trusted side: a link it left
/// pointing at a directory of the trusted side leaves that directory alone when
/// the call is released.
#[tokio::test]
async fn a_link_left_in_out_is_never_followed_by_the_cleanup() {
    let Some(root) = Root::new() else { return };
    let ex = executor(Some(&root));
    let trusted = root.path.join(unique("trusted"));
    let _scrap = Scrap(trusted.clone());
    std::fs::create_dir(&trusted).unwrap();
    std::fs::write(trusted.join("canary"), "keep").unwrap();
    let staged = StagedCall::create(&root.path, 1).unwrap();
    let code = format!(
        "import os\nos.symlink({:?}, '/out/to_trusted')\nos.symlink('/data', '/out/to_data')\noutput = sorted(os.listdir('/out'))",
        trusted.to_str().unwrap()
    );
    let out = run_staged(&ex, staged.mounts(), &code).await.unwrap();
    assert_eq!(out, json!(["to_data", "to_trusted"]));
    drop(staged);
    assert_eq!(
        std::fs::read_to_string(trusted.join("canary")).unwrap(),
        "keep"
    );
}

/// `/out` cannot be remounted with fewer restrictions or unmounted either.
#[tokio::test]
async fn out_cannot_be_remounted_or_unmounted() {
    let Some(root) = Root::new() else { return };
    let ex = executor(Some(&root));
    let staged = StagedCall::create(&root.path, 1).unwrap();
    let code = r#"
import ctypes, os, errno
libc = ctypes.CDLL(None, use_errno=True)
def err(call):
    ctypes.set_errno(0)
    rc = call()
    return 'SUCCEEDED' if rc >= 0 else errno.errorcode.get(ctypes.get_errno(), ctypes.get_errno())
MS_REMOUNT, MS_BIND = 32, 4096
output = {
  'remount_exec': err(lambda: libc.mount(b'', b'/out', None, MS_REMOUNT | MS_BIND, None)),
  'umount': err(lambda: libc.umount2(b'/out', 0)),
  'tmpfs_over': err(lambda: libc.mount(b'tmpfs', b'/out', b'tmpfs', 0, None)),
  'noexec_still': bool(os.statvfs('/out').f_flag & os.ST_NOEXEC),
}
"#;
    let out = run_staged(&ex, staged.mounts(), code).await.unwrap();
    assert_eq!(
        out,
        json!({"remount_exec": "EPERM", "umount": "EPERM", "tmpfs_over": "EPERM",
               "noexec_still": true})
    );
}

/// An output the jail cannot vouch for ends the call before any code runs: a
/// bound smaller than the volume really is, and an output that is only a
/// directory on the staging volume.
#[tokio::test]
async fn an_output_that_is_not_a_bound_volume_ends_the_call() {
    let Some(root) = Root::new() else { return };
    let ex = executor(Some(&root));
    let staged = StagedCall::create(&root.path, 2).unwrap();
    let mut too_small = staged.mounts();
    too_small.out_mb = 1;
    let err = run_staged(&ex, too_small, "output = 1").await.unwrap_err();
    assert_eq!(err.to_string(), CRASHED_MESSAGE);
    // A plain directory where the volume should be.
    let plain_id = unique("plain");
    let plain = root.path.join(&plain_id);
    let _scrap = Scrap(plain.clone());
    for d in ["", "data", "out"] {
        std::fs::create_dir_all(plain.join(d)).unwrap();
    }
    std::fs::set_permissions(&plain, Permissions::from_mode(0o700)).unwrap();
    let mounts = CallMounts {
        stage_id: plain_id.clone(),
        out_mb: 1,
    };
    let err = run_staged(&ex, mounts, "output = 1").await.unwrap_err();
    assert_eq!(err.to_string(), CRASHED_MESSAGE);
}

/// The file size limit follows the output volume for a call with mounts and
/// nothing else changes: `/tmp` stays a 64 MiB volume, and a call without mounts
/// keeps 64 MiB.
#[tokio::test]
async fn only_the_file_size_limit_follows_the_output_volume() {
    let Some(root) = Root::new() else { return };
    let ex = executor(Some(&root));
    let staged = StagedCall::create(&root.path, 100).unwrap();
    let code = r#"
import os, resource, errno
def attempt(path, mb):
    try:
        with open(path, 'wb') as f:
            f.write(b'x' * (mb << 20))
        return 'done'
    except OSError as e:
        return errno.errorcode.get(e.errno, e.errno)
output = {
  'fsize_mb': resource.getrlimit(resource.RLIMIT_FSIZE)[1] >> 20,
  'tmp_65': attempt('/tmp/big', 65),
  'out_80': attempt('/out/big', 80) if os.path.exists('/out') else 'no out',
}
"#;
    let with = run_staged(&ex, staged.mounts(), code).await.unwrap();
    assert_eq!(
        with,
        json!({"fsize_mb": 100, "tmp_65": "ENOSPC", "out_80": "done"})
    );
    let limit = "import resource\noutput = resource.getrlimit(resource.RLIMIT_FSIZE)[1] >> 20";
    assert_eq!(run_plain(&ex, limit).await.unwrap(), json!(64));
}

// ---------------------------------------------------------------------------
// The startup self-test proves the run mounts too.
// ---------------------------------------------------------------------------

/// The self-test with no staging root counts every mount of the host: alone only.
fn self_test_without_root(alone: &Root) -> (bool, Vec<Value>) {
    assert!(
        alone.exclusive,
        "an unrooted self-test needs Root::exclusive()"
    );
    self_test(None)
}

fn self_test(root: Option<&Root>) -> (bool, Vec<Value>) {
    self_test_at(root.map(|r| r.path.as_path()))
}

fn self_test_at(root: Option<&Path>) -> (bool, Vec<Value>) {
    let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_python_executor"));
    cmd.arg("self-test").arg("--uid-base");
    cmd.arg(next_uid_base().to_string());
    if let Some(root) = root {
        cmd.arg("--staging-root").arg(root);
    }
    let out = cmd.output().unwrap();
    let lines = String::from_utf8(out.stdout).unwrap();
    let checks = lines
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    (out.status.success(), checks)
}

/// With a staging root the report has three more layers, all held; without one
/// it is the 26 layers it always was, none of them about mounts.
#[test]
fn the_self_test_reports_the_mount_layers_only_with_a_staging_root() {
    let Some(root) = Root::exclusive() else {
        return;
    };
    let (ok, plain) = self_test_without_root(&root);
    assert!(ok && plain.len() == 26, "{plain:?}");
    let (ok, staged) = self_test(Some(&root));
    assert!(ok, "{staged:?}");
    let names: Vec<&str> = staged
        .iter()
        .map(|c| c["layer"].as_str().unwrap())
        .collect();
    for layer in [
        "mount_data_readonly",
        "mount_out_bounded",
        "staging_root_hidden",
    ] {
        let found = staged.iter().find(|c| c["layer"] == layer);
        assert!(
            found.is_some_and(|c| c["ok"] == true),
            "{layer} in {names:?}"
        );
    }
    assert_eq!(staged.len(), 30);
    assert_eq!(mounts_under(&root.path), 0, "the probe left a mount behind");
}

/// Calls in flight mount and unmount their output volumes under the staging root
/// while a template starts (a restart, another slot warming). The self-test's
/// "no mount added to the template's namespace" check must not read those as
/// mounts the probe left behind, or a template cannot start while any call runs.
#[test]
fn the_self_test_ignores_mounts_of_calls_in_flight() {
    let Some(root) = Root::new() else { return };
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    // Several threads, so that a mount is nearly always in flight.
    let churn: Vec<_> = (0..3)
        .map(|_| {
            let (stop, path) = (stop.clone(), root.path.clone());
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    drop(StagedCall::create(&path, 1).unwrap());
                }
            })
        })
        .collect();
    for _ in 0..25 {
        let (ok, checks) = self_test(Some(&root));
        assert!(ok, "{checks:?}");
    }
    stop.store(true, Ordering::Relaxed);
    for t in churn {
        t.join().unwrap();
    }
}

// ---------------------------------------------------------------------------
// The template's Arrow settings.
// ---------------------------------------------------------------------------

/// A staged executor's calls see the two Arrow variables on top of the fixed
/// environment, and nothing else changes in it.
#[tokio::test]
async fn a_staged_executor_adds_exactly_the_arrow_variables() {
    let Some(root) = Root::new() else { return };
    let ex = executor(Some(&root));
    let out = run_plain(&ex, "import os\noutput = dict(os.environ)").await;
    let mut expected = json!({
        "PATH": "/usr/local/bin:/usr/bin:/bin",
        "OPENBLAS_NUM_THREADS": "1",
        "OMP_NUM_THREADS": "1",
        "MKL_NUM_THREADS": "1",
        "ARROW_DEFAULT_MEMORY_POOL": "system",
        "ARROW_IO_THREADS": "1",
    });
    for k in ["LANG", "LC_ALL", "LC_CTYPE"] {
        if let Ok(v) = std::env::var(k) {
            expected[k] = v.into();
        }
    }
    assert_eq!(out.unwrap(), expected);
}

/// Where pyarrow is installed for the system Python: pandas reads a Parquet
/// part from `/data` inside the jail, user code never imports pyarrow, the
/// template has one thread and its pool is the system one (its start would
/// have been refused otherwise). Skipped, loudly, where pyarrow is absent.
#[tokio::test]
async fn pandas_reads_a_parquet_part_through_data_when_pyarrow_is_installed() {
    let Some(root) = Root::new() else { return };
    let make = "import pandas as pd, sys\n\
        pd.DataFrame({'k': ['a', 'b', 'a'], 'v': [1, 2, 3]}).to_parquet(sys.argv[1] + '/p.parquet')";
    let ex = executor(Some(&root));
    let staged = StagedCall::create(&root.path, 1).unwrap();
    let made = std::process::Command::new("python3")
        .args(["-c", make])
        .arg(staged.data_dir())
        .output()
        .unwrap();
    if !made.status.success() {
        // CI's job does not install pyarrow (a decision for the sandbox owner), so
        // by default this is a visible note, not a failure. Where pyarrow is
        // expected (the Docker run sets the opt-in) the skip line the isolated
        // job's step fails on is printed, and the test fails.
        if std::env::var("COLMENA_PYEXEC_EXPECT_PYARROW").as_deref() == Ok("1") {
            eprintln!("skipped: set COLMENA_PYEXEC_JAIL_TESTS=1 (Linux, root, CAP_SYS_ADMIN) and install pyarrow: it is expected here and python3 cannot import it");
            panic!(
                "pyarrow is expected here (COLMENA_PYEXEC_EXPECT_PYARROW=1) but is not installed"
            );
        }
        eprintln!("NOTE: pandas_reads_a_parquet_part_through_data did not run: pyarrow is not installed for python3");
        return;
    }
    let code = "import pandas as pd\n\
        df = pd.read_parquet('/data/p.parquet', columns=['k', 'v'], use_threads=False)\n\
        output = df.groupby('k')['v'].sum().to_dict()";
    let out = run_staged(&ex, staged.mounts(), code).await.unwrap();
    assert_eq!(out, json!({"a": 4, "b": 2}));
}

// ---------------------------------------------------------------------------
// The output size: validated, budgeted, and the file limit follows the volume.
// ---------------------------------------------------------------------------

/// A size of zero (an unlimited tmpfs) or over the ceiling is refused before any
/// directory or mount exists.
#[test]
fn an_invalid_output_size_makes_nothing() {
    let Some(_t) = Root::new() else { return };
    // A root of its own, so that other tests' calls cannot be mistaken for leftovers.
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    for bad in [0, OUT_MB_MAX + 1, u64::MAX] {
        let e = StagedCall::create(&root, bad).unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::InvalidInput, "{bad}");
    }
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
    assert_eq!(mounts_under(&root), 0);
}

/// The executor stages calls under a budget of volumes and mebibytes in flight:
/// over it, a typed error and nothing mounted; a released call frees its share.
#[test]
fn the_executor_refuses_staged_calls_over_its_budget() {
    // Counts the mounts under the shared root, and has an executor without one:
    // nothing else may mount meanwhile.
    let Some(root) = Root::exclusive() else {
        return;
    };
    let ex = executor(Some(&root));
    // Small volumes, so that it is the COUNT that bites; the MiB total is
    // covered by the budget's unit test.
    let mut held = Vec::new();
    for _ in 0..STAGED_VOLUMES_MAX {
        held.push(ex.stage_call(10).unwrap());
    }
    let over = ex.stage_call(1).unwrap_err();
    assert!(matches!(over, StageError::OverBudget { .. }), "{over:?}");
    let before = mounts_under(&root.path);
    assert!(ex.stage_call(1).is_err());
    assert_eq!(mounts_under(&root.path), before, "a refusal mounts nothing");
    drop(held.pop());
    held.push(ex.stage_call(1).unwrap());
    let none = executor_without_root(&root);
    assert!(matches!(
        none.stage_call(1).unwrap_err(),
        StageError::NoStagingRoot
    ));
}

/// The header can claim any size; what the call may write follows the volume the
/// jail verified, not the claim.
#[tokio::test]
async fn the_file_limit_follows_the_verified_volume_not_the_header() {
    let Some(root) = Root::new() else { return };
    let ex = executor(Some(&root));
    let staged = StagedCall::create(&root.path, 2).unwrap();
    let mut claim = staged.mounts();
    claim.out_mb = 100;
    let limit = "import resource\noutput = resource.getrlimit(resource.RLIMIT_FSIZE)[1] >> 20";
    assert_eq!(run_staged(&ex, claim, limit).await.unwrap(), json!(64));
}

/// An out size the jail will not accept ends the call before any mount or code.
#[tokio::test]
async fn a_header_with_an_invalid_output_size_ends_the_call() {
    let Some(root) = Root::new() else { return };
    let ex = executor(Some(&root));
    let staged = StagedCall::create(&root.path, 2).unwrap();
    for bad in [0, OUT_MB_MAX + 1, u64::MAX] {
        let mut claim = staged.mounts();
        claim.out_mb = bad;
        let err = run_staged(&ex, claim, "output = 1").await.unwrap_err();
        assert_eq!(err.to_string(), CRASHED_MESSAGE, "{bad}");
    }
}

/// Real mounts, one fact at a time: a tmpfs without an inode bound, a plain
/// directory that is not a mount root, and a different filesystem type.
#[test]
fn real_volumes_that_miss_one_fact_are_refused() {
    let Some(_alone) = Root::exclusive() else {
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let flags = libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC;
    let mount_at = |dir: &Path, fstype: &str, data: &str| {
        let (t, d) = (
            std::ffi::CString::new(fstype).unwrap(),
            std::ffi::CString::new(data).unwrap(),
        );
        let target = std::ffi::CString::new(dir.to_str().unwrap()).unwrap();
        let rc = unsafe {
            libc::mount(
                t.as_ptr(),
                target.as_ptr(),
                t.as_ptr(),
                flags,
                d.as_ptr() as *const libc::c_void,
            )
        };
        assert_eq!(rc, 0, "{fstype}: {}", std::io::Error::last_os_error());
    };
    let call = |name: &str| {
        let dir = root.join(name);
        for d in ["", "data", "out"] {
            std::fs::create_dir_all(dir.join(d)).unwrap();
        }
        std::fs::set_permissions(&dir, Permissions::from_mode(0o700)).unwrap();
        dir
    };
    let mut mounted = Vec::new();
    // tmpfs of the right size, no inode bound.
    let no_inodes = call("no-inodes");
    mount_at(&no_inodes.join("out"), "tmpfs", "size=2m");
    mounted.push(no_inodes.join("out"));
    let dirs = open_call_dirs(&root, "no-inodes").unwrap();
    let e = check_out_volume(&dirs, 2).unwrap_err();
    assert!(e.to_string().contains("inode"), "{e}");
    // The same tmpfs with the bound is accepted.
    let bounded = call("bounded");
    mount_at(
        &bounded.join("out"),
        "tmpfs",
        &format!("size=2m,nr_inodes={OUT_MAX_INODES}"),
    );
    mounted.push(bounded.join("out"));
    let dirs = open_call_dirs(&root, "bounded").unwrap();
    assert_eq!(check_out_volume(&dirs, 2).unwrap(), 2 << 20);
    // Another filesystem type (ramfs has no size at all).
    let other = call("ramfs");
    mount_at(&other.join("out"), "ramfs", "");
    mounted.push(other.join("out"));
    let dirs = open_call_dirs(&root, "ramfs").unwrap();
    assert!(check_out_volume(&dirs, 2).is_err());
    for m in mounted.iter().rev() {
        let c = std::ffi::CString::new(m.to_str().unwrap()).unwrap();
        unsafe { libc::umount2(c.as_ptr(), libc::MNT_DETACH) };
    }
}

// ---------------------------------------------------------------------------
// Other calls' volumes are not in this call's namespace.
// ---------------------------------------------------------------------------

/// Every call starts with a copy of the whole mount table, other calls' output
/// volumes included. The jail detaches them: from inside, no mount of another
/// call (or of this call's own staging path) is listed, with or without mounts.
#[tokio::test]
async fn a_call_does_not_see_other_calls_mounts() {
    let Some(root) = Root::new() else { return };
    let ex = executor(Some(&root));
    let (mine, other) = (
        StagedCall::create(&root.path, 1).unwrap(),
        StagedCall::create(&root.path, 1).unwrap(),
    );
    // The id of the call itself may appear: the bind of its own `/data` names
    // the directory it came from. Another call's id may not, anywhere.
    let others = format!(
        "table = open('/proc/self/mountinfo').read().splitlines()\n\
         output = [l for l in table if {:?} in l]",
        other.id()
    );
    assert_eq!(
        run_staged(&ex, mine.mounts(), &others).await.unwrap(),
        json!([])
    );
    assert_eq!(run_plain(&ex, &others).await.unwrap(), json!([]));
    let own_mounts_under_root = format!(
        "table = open('/proc/self/mountinfo').read().splitlines()\n\
         output = [l.split()[4] for l in table if l.split()[4].startswith({:?} + '/')]",
        root.path.to_str().unwrap()
    );
    let under = run_staged(&ex, mine.mounts(), &own_mounts_under_root).await;
    assert_eq!(
        under.unwrap(),
        json!([]),
        "no mount is left under the staging root"
    );
    // The call's own /out is still there, and is not listed under the staging path.
    let own = "output = [l.split()[4] for l in open('/proc/self/mountinfo').read().splitlines() if l.split()[4] in ('/data', '/out')]";
    let mut got = run_staged(&ex, mine.mounts(), own).await.unwrap();
    got.as_array_mut()
        .unwrap()
        .sort_by_key(|v| v.as_str().unwrap().to_string());
    assert_eq!(got, json!(["/data", "/out"]));
}

fn shmem_kib() -> u64 {
    let info = std::fs::read_to_string("/proc/meminfo").unwrap();
    let line = info.lines().find(|l| l.starts_with("Shmem:")).unwrap();
    line.split_whitespace().nth(1).unwrap().parse().unwrap()
}

/// The memory of a released output volume is freed while a call that started
/// before the release is still running. In the Docker container this was
/// written in the volume is freed even without the detach (the host's unmount
/// propagates to the call's copy), so this guards the property; it is
/// `a_call_does_not_see_other_calls_mounts` that fails without the detach.
#[tokio::test]
async fn a_released_volume_is_freed_while_an_overlapping_call_runs() {
    let Some(root) = Root::exclusive() else {
        return;
    };
    let ex = executor(Some(&root));
    let released = StagedCall::create(&root.path, 128).unwrap();
    std::fs::write(released.out_dir().join("fill"), vec![1u8; 96 << 20]).unwrap();
    let running = StagedCall::create(&root.path, 4).unwrap();
    let hold = r#"
import os, time
open('/out/ready', 'w').close()
for _ in range(400):
    if os.path.exists('/out/go'):
        break
    time.sleep(0.05)
output = os.path.exists('/out/go')
"#;
    let call = run_staged(&ex, running.mounts(), hold);
    let control = async {
        let ready = running.out_dir().join("ready");
        for _ in 0..400 {
            if ready.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(ready.exists(), "the overlapping call never started");
        let before = shmem_kib();
        drop(released);
        let after = shmem_kib();
        std::fs::write(running.out_dir().join("go"), "").unwrap();
        (before, after)
    };
    let (answer, (before, after)) = tokio::join!(call, control);
    assert_eq!(answer.unwrap(), json!(true));
    let freed_mib = before.saturating_sub(after) / 1024;
    assert!(
        freed_mib >= 64,
        "only {freed_mib} MiB freed (before {before} KiB, after {after} KiB)"
    );
}

/// Calls come and go while others start: a mount that vanishes between the jail
/// reading the table and detaching it is not an error.
#[tokio::test]
async fn calls_start_correctly_while_other_calls_come_and_go() {
    let Some(root) = Root::new() else { return };
    let ex = executor(Some(&root));
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let churn = {
        let (stop, path) = (stop.clone(), root.path.clone());
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                drop(StagedCall::create(&path, 1).unwrap());
            }
        })
    };
    for _ in 0..25 {
        let staged = StagedCall::create(&root.path, 1).unwrap();
        let out = run_staged(&ex, staged.mounts(), "output = 1").await;
        assert_eq!(out.unwrap(), json!(1));
        assert_eq!(run_plain(&ex, "output = 2").await.unwrap(), json!(2));
    }
    stop.store(true, Ordering::Relaxed);
    churn.join().unwrap();
}

// ---------------------------------------------------------------------------
// A broken staging setup disables the mounts capability, not the executor.
// ---------------------------------------------------------------------------

/// A staging root on a read-only filesystem: the self-test's staged call cannot
/// be made. Plain calls keep working; calls that ask for mounts are refused with
/// a typed error naming the reason.
#[tokio::test]
async fn a_broken_staging_root_disables_mounts_and_not_plain_calls() {
    let Some(good) = Root::exclusive() else {
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let broken = tmp.path().canonicalize().unwrap();
    let rc = unsafe {
        libc::mount(
            c"tmpfs".as_ptr(),
            std::ffi::CString::new(broken.to_str().unwrap())
                .unwrap()
                .as_ptr(),
            c"tmpfs".as_ptr(),
            libc::MS_RDONLY,
            c"size=1m".as_ptr() as *const libc::c_void,
        )
    };
    assert_eq!(rc, 0, "{}", std::io::Error::last_os_error());
    struct Unmount(PathBuf);
    impl Drop for Unmount {
        fn drop(&mut self) {
            let c = std::ffi::CString::new(self.0.to_str().unwrap()).unwrap();
            unsafe { libc::umount2(c.as_ptr(), libc::MNT_DETACH) };
        }
    }
    let _guard = Unmount(broken.clone());
    let mut cfg = SubprocessConfig::from_lookup(&|_: &str| None::<String>).unwrap();
    cfg.bin = PathBuf::from(env!("CARGO_BIN_EXE_python_executor"));
    cfg.slots = 1;
    cfg.uid_base = 64000;
    cfg.staging_root = Some(broken.clone());
    let ex = SubprocessExecutor::new(cfg, Duration::from_secs(60)).unwrap();
    assert_eq!(run_plain(&ex, "output = 1").await.unwrap(), json!(1));
    // A perfectly good staged call, made elsewhere: the executor still refuses it.
    let staged = StagedCall::create(&good.path, 1).unwrap();
    let err = run_staged(&ex, staged.mounts(), "output = 2")
        .await
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("disabled") && err.contains("staging_unusable"),
        "{err}"
    );
    assert_eq!(run_plain(&ex, "output = 3").await.unwrap(), json!(3));
}

// ---------------------------------------------------------------------------
// Cleanup: failures are logged with the call id; leftovers are swept at start.
// ---------------------------------------------------------------------------

#[derive(Clone, Default)]
struct Captured(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);
impl std::io::Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Captured {
    type Writer = Captured;
    fn make_writer(&'a self) -> Captured {
        self.clone()
    }
}

/// A cleanup that fails (here: a mount left over `data`, which cannot be
/// removed) is not swallowed: it is logged with the call id, and the directories
/// stay for the sweep.
#[test]
fn a_failed_cleanup_is_logged_with_the_call_id() {
    let Some(_alone) = Root::exclusive() else {
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let log = Captured::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(log.clone())
        .with_ansi(false)
        .finish();
    let id = tracing::subscriber::with_default(subscriber, || {
        let staged = StagedCall::create(&root, 1).unwrap();
        let data = staged.data_dir();
        let c = std::ffi::CString::new(data.to_str().unwrap()).unwrap();
        let rc = unsafe {
            libc::mount(
                c"tmpfs".as_ptr(),
                c.as_ptr(),
                c"tmpfs".as_ptr(),
                0,
                c"size=1m".as_ptr() as *const libc::c_void,
            )
        };
        assert_eq!(rc, 0);
        let id = staged.id().to_string();
        drop(staged);
        unsafe { libc::umount2(c.as_ptr(), libc::MNT_DETACH) };
        id
    });
    let text = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
    assert!(text.contains(&id) && text.contains("cleanup"), "{text}");
    assert!(
        root.join(&id).exists(),
        "what could not be removed is left for the sweep"
    );
    let report = sweep_staging_root(&root).unwrap();
    assert_eq!(report.removed, 1, "{report:?}");
    assert!(!root.join(&id).exists());
}

/// After a crash nothing reclaims the call directories (prepared customer data)
/// or the volumes: the sweep unmounts and removes them, never follows anything
/// inside, and leaves what is not a call directory alone.
#[test]
fn the_sweep_reclaims_leftover_calls_and_follows_nothing() {
    let Some(_alone) = Root::exclusive() else {
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let canary_dir = root.join("canary-dir");
    std::fs::create_dir(&canary_dir).unwrap();
    std::fs::write(canary_dir.join("canary"), "keep").unwrap();
    // A crashed executor: two calls left mounted and filled.
    let mut left = Vec::new();
    for _ in 0..2 {
        let staged = StagedCall::create(&root, 1).unwrap();
        std::fs::write(staged.data_dir().join("part.parquet"), "customer").unwrap();
        std::fs::write(staged.out_dir().join("result.csv"), "result").unwrap();
        // The program's link, and a link in what the trusted side staged.
        std::os::unix::fs::symlink(&canary_dir, staged.out_dir().join("escape")).unwrap();
        std::os::unix::fs::symlink(&canary_dir, staged.data_dir().join("escape")).unwrap();
        left.push(staged.id().to_string());
        std::mem::forget(staged);
    }
    // A call-id-shaped link to the canary, and things that are not calls at all
    // (the canary's own directory is one: only generated ids are swept).
    let linked = "a".repeat(32);
    std::os::unix::fs::symlink(&canary_dir, root.join(&linked)).unwrap();
    std::fs::write(root.join("notes.txt"), "mine").unwrap();
    std::fs::create_dir(root.join("not-a-call id")).unwrap();
    let report = sweep_staging_root(&root).unwrap();
    assert_eq!(report.removed, 2, "{report:?}");
    assert_eq!(report.skipped, 4, "{report:?}");
    for id in &left {
        assert!(!root.join(id).exists(), "{id} left behind");
    }
    assert_eq!(mounts_under(&root), 0);
    assert_eq!(
        std::fs::read_to_string(canary_dir.join("canary")).unwrap(),
        "keep"
    );
    assert!(root
        .join(&linked)
        .symlink_metadata()
        .unwrap()
        .file_type()
        .is_symlink());
    assert!(root.join("notes.txt").exists() && root.join("not-a-call id").exists());
}

/// The executor that is about to serve sweeps its staging root first (the host
/// and `python_executor serve` both build it this way); a plain `new` does not,
/// because a sweep cannot tell a leftover from a call in flight.
#[test]
fn an_executor_about_to_serve_sweeps_its_root() {
    let Some(root) = Root::exclusive() else {
        return;
    };
    let build = |serving: bool| {
        let mut cfg = SubprocessConfig::from_lookup(&|_: &str| None::<String>).unwrap();
        cfg.bin = PathBuf::from(env!("CARGO_BIN_EXE_python_executor"));
        cfg.staging_root = Some(root.path.clone());
        cfg.uid_base = 65000;
        match serving {
            true => SubprocessExecutor::new_for_serving(cfg, Duration::from_secs(60)),
            false => SubprocessExecutor::new(cfg, Duration::from_secs(60)),
        }
        .unwrap()
    };
    let staged = StagedCall::create(&root.path, 1).unwrap();
    let leftover = root.path.join(staged.id());
    std::mem::forget(staged);
    let _plain = build(false);
    assert!(leftover.exists(), "a plain new must not sweep");
    let _serving = build(true);
    assert!(!leftover.exists(), "a leftover call survived the start");
    assert_eq!(mounts_under(&root.path), 0);
}

// ---------------------------------------------------------------------------
// A staging root below a path the jail covers.
// ---------------------------------------------------------------------------

/// A Cloud Run volume normally mounts under `/mnt`, which the jail covers, and a
/// test root under `/tmp` is below the jail's own fresh `/tmp`: inside the jail
/// such a root does not exist, and that is hidden. Mounts must still work and the
/// self-test must pass with all 30 layers, not report "unreadable".
#[tokio::test]
async fn a_staging_root_below_a_covered_path_enables_mounts() {
    let Some(_alone) = Root::exclusive() else {
        return;
    };
    let under_mnt = PathBuf::from(format!(
        "/mnt/colmena-nested-{}",
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&under_mnt)
        .unwrap();
    let _scrap = Scrap(under_mnt.clone());
    let tmp = tempfile::tempdir().unwrap();
    let under_tmp = tmp.path().canonicalize().unwrap();
    for root in [&under_mnt, &under_tmp] {
        let (ok, checks) = self_test_at(Some(root));
        assert!(ok, "{root:?}: {checks:?}");
        assert_eq!(checks.len(), 30, "{root:?}");
        let hidden = checks
            .iter()
            .find(|c| c["layer"] == "staging_root_hidden")
            .unwrap();
        assert_eq!(hidden["ok"], true, "{root:?}: {hidden}");
        // And a real call with mounts is served, not refused as disabled.
        let mut cfg = SubprocessConfig::from_lookup(&|_: &str| None::<String>).unwrap();
        cfg.bin = PathBuf::from(env!("CARGO_BIN_EXE_python_executor"));
        cfg.slots = 1;
        cfg.uid_base = 66000;
        cfg.staging_root = Some(root.clone());
        let ex = SubprocessExecutor::new(cfg, Duration::from_secs(60)).unwrap();
        let staged = ex.stage_call(1).unwrap();
        std::fs::write(staged.data_dir().join("a.txt"), "hi").unwrap();
        let code = "import os\noutput = open('/data/a.txt').read()";
        assert_eq!(
            run_staged(&ex, staged.mounts(), code).await.unwrap(),
            json!("hi"),
            "{root:?}"
        );
    }
}

/// The budget share goes back when the volume was unmounted even though the rest
/// of the cleanup failed (here: a mount left over `data`).
#[test]
fn a_share_goes_back_when_the_volume_was_unmounted() {
    let Some(root) = Root::exclusive() else {
        return;
    };
    let ex = executor(Some(&root));
    let staged = ex.stage_call(1).unwrap();
    assert_eq!(ex.staged_in_flight(), (1, 1));
    let data = std::ffi::CString::new(staged.data_dir().to_str().unwrap()).unwrap();
    let rc = unsafe {
        libc::mount(
            c"tmpfs".as_ptr(),
            data.as_ptr(),
            c"tmpfs".as_ptr(),
            0,
            c"size=1m".as_ptr() as *const libc::c_void,
        )
    };
    assert_eq!(rc, 0);
    let id = staged.id().to_string();
    drop(staged);
    unsafe { libc::umount2(data.as_ptr(), libc::MNT_DETACH) };
    assert_eq!(ex.staged_in_flight(), (0, 0));
    let _ = sweep_staging_root(&root.path);
    assert!(!root.path.join(id).exists());
}

/// An `out` the sweep cannot open for a reason other than ENOENT (here a file
/// where the directory should be: ENOTDIR) is not known to be unmounted: the call
/// directory, and what is in it, stays.
#[test]
fn the_sweep_leaves_a_call_whose_out_it_cannot_open() {
    let Some(_alone) = Root::exclusive() else {
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let id = uuid::Uuid::new_v4().simple().to_string();
    let call = root.join(&id);
    std::fs::create_dir_all(call.join("data")).unwrap();
    std::fs::write(call.join("data").join("part.parquet"), "customer").unwrap();
    std::fs::write(call.join("out"), "not a directory").unwrap();
    let report = sweep_staging_root(&root).unwrap();
    assert_eq!((report.removed, report.failed), (0, 1), "{report:?}");
    assert_eq!(
        std::fs::read_to_string(call.join("data").join("part.parquet")).unwrap(),
        "customer"
    );
    // A call with no `out` at all (ENOENT) is reclaimed.
    std::fs::remove_file(call.join("out")).unwrap();
    let report = sweep_staging_root(&root).unwrap();
    assert_eq!(report.removed, 1, "{report:?}");
}
