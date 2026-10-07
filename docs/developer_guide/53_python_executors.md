# Python executors

Every piece of Python that Colmena runs — the `python_script` node and the
`data_run_python`, `gsheets_run_python`, `attachment_run_python` and
`crdt_doc_run_python` tools — goes through one entry point,
`dag_engine::infrastructure::python_exec::run`. The executor decides where the
code runs. Data loading and writing (Sheets, SQL, attachments, documents) always
stay in the host process; Python only receives JSON inputs and returns JSON.

## Choosing an executor

| `COLMENA_PYTHON_EXECUTOR` | Where the code runs | Status |
|---|---|---|
| `inprocess` (default) | inside the host process, embedded interpreter | available |
| `subprocess` | a jailed child process per call, forked from a warm template (Linux; the host starts as root inside its container) | available |
| `remote` | a [`python_executor serve`](#remote-service-python_executor-serve) endpoint, through [the remote client](#the-remote-client) | available |

Any other value is a configuration error: the host fails at startup (when it
calls `install_from_env`, as `EngineConfig::from_env` does, and as the
`dag_engine` CLI does for every subcommand except `lint`, which never runs
Python) and every Python call would otherwise return `PythonExecutorError: …`.
An executor never falls back to `inprocess`. The same is true of
`COLMENA_PYTHON_EXECUTOR_MODES` and `COLMENA_PYTHON_EXECUTOR_MAX_TIMEOUT_SECS`
below: an invalid value stops startup the same way, named in the error.

| Variable | Values | Default |
|---|---|---|
| `COLMENA_PYTHON_EXECUTOR_MODES` | `restricted` (only `restricted` code goes to an isolated executor) or `all` | `restricted` |
| `COLMENA_PYTHON_EXECUTOR_MAX_TIMEOUT_SECS` | a positive whole number of seconds — the deadline applied to isolated requests that carry none of their own; has no effect while `COLMENA_PYTHON_EXECUTOR=inprocess` | `3600` |

## Installing at startup

`install_from_env` builds the process executor from the environment and is
idempotent — calling it more than once is safe, and only the first successful
call emits the `info` install event below. `EngineConfig::from_env` and the
`dag_engine` CLI already call it, for every subcommand except `lint` (skipped
so the one-time install event, which goes to stdout, doesn't pollute
`--format json`; `lint` never runs Python, so there is nothing to install for
it anyway). A host that builds `EngineConfig` by hand (without going through
`from_env`), or that never constructs an `EngineConfig` at all, should call
`install_from_env()` itself at startup. Otherwise a bad
`COLMENA_PYTHON_EXECUTOR` value surfaces only at the first Python call instead
of at boot, and the install event never fires.

A host that serves requests should then await
`python_exec::wait_until_ready()` before it starts listening. With
`subprocess`, `install_from_env` only begins the template's start (interpreter,
module imports, self-test) in the background; without the wait it runs beside
the first requests, and a platform that throttles CPU outside requests can make
it several times slower. On a 2-vCPU service with CPU only during requests it
took about 106 s on one instance and passed the 120 s start limit on another,
whose Python calls failed until a later start; the same image with full CPU did
the cold imports in 17.4 s. The wait joins the start under way (it never starts
a second template beside it) and returns `Ok(())` once the template is ready,
or the `PythonExecutorError: …` text of a failed start; `remote` waits for the
service ([The remote client](#the-remote-client)); `inprocess` returns
`Ok(())` at once. A failed start does not stop the host: log it and serve;
isolated calls fail until a later start succeeds. `EngineConfig::from_env` and
the `dag_engine` CLI do not wait.

```rust
python_exec::install_from_env()?;
if let Err(e) = python_exec::wait_until_ready().await {
    tracing::warn!("python executor not ready: {e}");
}
// ...then bind the port.
```

## Wire protocol (isolated executors)

Isolated executors (`subprocess`, and `python_executor serve` in front of it) talk to the process that
runs the code over a byte stream, one request and one response per call
(`python_exec::protocol`, `python_exec::frame`):

- **Frames.** A 4-byte big-endian length followed by that many bytes of JSON.
  The reader checks the length against a limit before it allocates or reads
  the body; a longer frame is refused (`frame::is_frame_too_large`), and a
  stream that ends mid-frame is an `UnexpectedEof`.
- **Request** (`WireRequest`): `v` (protocol version, `1`), `code`, `mode`,
  `timeout_ms` (the request's own deadline when it has one, else the
  executor's; saturates instead of wrapping) and `inputs`.
- **Response** (`WireResponse`): `v`, `status` (`ok`, `python_error`,
  `timeout`, `crashed`, `too_large`), `output_set` and `output` (so a code
  that assigned `output = None` is told apart from one that never assigned
  it), `stdout`, `message` and `exec_ms`.
- **Mapping back.** `ok` → the result; `python_error` → the message as is
  (the model sees the same text as in process); `timeout` → the caller's own
  timeout text; `crashed` → a fixed `Python execution error: …` text (a
  message sent by the process is ignored); `too_large` → its message. A
  missing message, or a response with another `v`, becomes
  `Python execution error: …` (malformed) or
  `PythonExecutorError: unsupported protocol version …`.

### The per-call body

`python_exec::child::handle_request` is what the process that runs the code
does for one call: read one request frame (capped at the size the host
allows), check its version, run the same helper the in-process executor
runs, and write one response frame.

- A request larger than the cap gets a `too_large` response, but only a host
  whose write of that frame succeeded sees it; a host should check the size
  before sending (it knows the cap), and the stream is not reused after it.
- A request of another protocol version gets a version error stamped with the
  child's own version, so the host's version check fires; the version is read
  before the rest of the request, so a request of a different shape still
  gets it.
- A truncated request or one that is not valid JSON writes nothing: the host
  sees the stream close. So does a panic in the helper. The process that hosts
  the body is expected to exit after either.

### The warm template (Linux)

`python_executor zygote --socket <path>` starts one Python with the heavy modules imported once, checks it is
single-threaded, binds a 0600 socket, prints `READY` and forks one child per connection (per-call numpy reseed; the
child always ends in `_exit`). It exits `3` if a startup check fails; a child exits `70` on a protocol error. The host
must start it from a long-lived thread: it is killed when that thread exits. Logs are JSON fields only.

### The subprocess executor (Linux)

`python_exec::subprocess` supervises the warm template: it starts it from a dedicated long-lived thread (the template
dies with that thread) with only `TEMPLATE_ENV`, replaces it when it exits, and retries a failed start after a pause.
Its stderr is read as JSON events with known fields only; any other line is dropped and counted. Settings:

| Variable | Default |
|---|---|
| `COLMENA_PYTHON_EXECUTOR_BIN` | the `python_executor` file in the host executable's directory (not a `PATH` lookup) |
| `COLMENA_PYTHON_EXECUTOR_SLOTS` | CPU cores, at most 8 (1-64) |
| `COLMENA_PYTHON_EXECUTOR_MEMORY_MB` | 2048 (256-65536) |
| `COLMENA_PYTHON_EXECUTOR_HIDE_PATHS` | none (`:`-separated absolute paths, no `..`) |
| `COLMENA_PYTHON_EXECUTOR_MAX_REQUEST_MB`, `…_MAX_RESPONSE_MB` | 256 (1-4095) |
| `COLMENA_PYTHON_EXECUTOR_REFUSE_OUTPUT` | none (`,`-separated literals, e.g. a credential prefix) |

Each call takes a slot, checks the request size first, forks a child through the template and reads one capped
response; a passed deadline kills the child (`Timeout`), and an abandoned call's child is killed and its slot freed
once it is gone. The template gets only the host's `LANG`, `LC_ALL` and `LC_CTYPE` (same text encodings as in process),
no `PYTHONPATH`/`PYTHONHOME`. The host must ignore `SIGPIPE`. `SubprocessExecutor::new` refuses to start unless the
host runs as root (children switch to unprivileged users). The template is warmed in the background at startup (a host
waits for it with `wait_until_ready`, see [Installing at startup](#installing-at-startup)); while it
cannot start (its self-test fails, the binary is missing) every call fails with a `PythonExecutorError`, never by running
the code in process. A result that contains any `…_REFUSE_OUTPUT` literal, byte for byte, does not leave: the call
fails with `Python execution error: the result was refused by the executor's output policy`. That is a second layer,
not a replacement for the process isolation: an encoded or transformed value does not match, and the in-process
executor does not apply it.

### Process isolation (Linux)

Each per-call child isolates itself after the header and before it reads the request, in this order: fds 0-2 to
`/dev/null` and every other descriptor but the call's closed; new mount, network, IPC and UTS namespaces (the network
namespace is empty: no DNS, loopback or outside route); a private `/tmp` (`tmpfs`, 64 MiB); a fresh `/proc` with `hidepid=invisible` (the jail
fails if the template is still listed after the uid change) whose runtime-masked entries are covered; the host
paths in `DEFAULT_HIDDEN` plus `…_HIDE_PATHS` (absolute; a directory gets an empty read-only `tmpfs`, a file a
read-only `nodev` `/dev/null`, a missing path is skipped) covered, `/dev/mqueue` among them (a new IPC namespace does
not cover a message-queue filesystem mounted there); one unprivileged uid/gid per slot (`uid_base + slot`, 20000 by
default); `no_new_privs` and death with the template; limits on address space (the template's size plus
`…_MEMORY_MB`), CPU seconds, file size, descriptors, processes and core files. A failed step ends the child (exit 71)
before any code runs. The last step is a syscall filter: creating sockets, starting processes or programs (`fork`,
`clone` without `CLONE_THREAD`, `execve`), tracing, mounts, namespaces, keyrings, BPF, `io_uring`, and module, swap and
reboot calls return `EPERM`; `clone3` returns `ENOSYS`, so threads are created through `clone`; on x86_64 every number of 512 or
more (the x32 ABI) returns `EPERM`; other architectures have no filter and the jail does not start. Threads work. Code that
uses `multiprocessing`, `subprocess`, an `asyncio` event loop (its self-pipe is a socket pair) or a library that probes
the system with a subprocess gets a `PermissionError` or `OSError`. The template needs root and `CAP_SYS_ADMIN`; without them every
call ends with the crashed text. The uid range `uid_base..uid_base+slots` must belong to one executor per PID
namespace and to no real user.

A call cannot start a process. As a second line, before a slot is reused, everything running as its uid is stopped
(and the template reaps the orphans). If that cannot be done even on a second try, the slot is retired; once none is left,
calls fail with `PythonExecutorError: no usable Python slot is left…` until the process restarts. The host signals a
call's child through a pidfd (the pid when pidfds are unavailable).

### Run staging directories (Linux, dark)

Dark behind `COLMENA_LARGE_TABULAR`: nothing below runs while the switch is off. A call that carries prepared data
is given two directories under a staging root: `<root>/<call id>/data` (read-only for the call) and
`<root>/<call id>/out` (a size-bound volume the call writes to). The trusted side makes and removes them with
`staging::StagedCall`: `out` is a `tmpfs` of its own (`size=<out_mb>m`, `nr_inodes=1024`, `nosuid,nodev,noexec`) and
`data` is left for the trusted side to fill. Dropping the `StagedCall` unmounts `out` first and only then removes the
directories, so nothing the program wrote is ever walked on the trusted side.

The jail never receives a path. It receives the call id, which must be 1-64 characters of `[A-Za-z0-9_-]` (so it
cannot be `.`/`..` or hold a separator), and derives the directories itself: every component, the root's included, is
opened with `O_NOFOLLOW` (`staging::open_call_dirs`), `call` and `data` must be owned by the executor and not writable
by group or others, and the descriptors it ends up with are what gets bound, so swapping a path after the check
changes nothing. `check_out_volume` refuses an `out` that is larger than the bound the header declares, has no size, or
shares the volume of the call directory.

The call header carries an optional `mounts` object `{stage_id, out_mb}` and nothing else: a call without prepared
data sends exactly the header it always sent, with no `mounts` key. The jail needs a staging root to honour it, set
with `COLMENA_PYTHON_EXECUTOR_STAGING_DIR` (an absolute, existing directory without `..`, canonicalised once at
startup) and read ONLY while `COLMENA_LARGE_TABULAR` is on; with the switch off the variable is not even looked at.
The subprocess executor passes it to the template as `--staging-root`, as `python_executor self-test` and `zygote`
accept it. `SubprocessExecutor::run_staged` is the call that sends `mounts`; without a staging root it starts no
child.

A call that asks for mounts gets its prepared data at `/data`, bound from the directory the jail itself opened (in the
call's new mount namespace: a descriptor from before `unshare` is refused by the kernel) and mounted read-only,
`nosuid`, `nodev` and `noexec` at the MOUNT, so a world-writable file in it still fails with `EROFS`; the flags are read
back with `statvfs` and a mount that did not take them ends the call. `/data` is created when the root filesystem
lacks it (a link or a file there is refused; calls starting together may race to create it, which is fine) and then
stays, empty and not writable by the program, for later calls; pre-create it in the image if the root filesystem is
read-only. The staging root is covered (an empty read-only `tmpfs`) for EVERY call once it is configured, with or without
mounts, so a call without mounts cannot list it either. Nothing else of the staging volume is visible: the call directory and every other call's files are not bound, and `..` of
`/data` is the jail's root. The jail detaches, in the call's own mount namespace, EVERY mount below the staging root, this call's own volume at its
staging path included (the copies of the executor's mounts the namespace starts with), deepest first, before it covers
the root; only the `/data` and `/out` binds, made before and from descriptors, survive. From inside,
`/proc/self/mountinfo` names no other call and lists nothing under the root. A mount whose path has vanished by the time
it is detached (ENOENT, or EINVAL) is ignored, and any other failure ends the call: that branch is reached in practice
(a full run of the suite hit ENOENT on stale copies of released volumes 56 times, counted once with a temporary probe),
but no test asserts it on purpose. A call id the jail cannot trust (a bad charset, a link in the path, a `data` directory
others can write in) ends the call before any code runs, with the crash text. Everything else the jail applies, the
user, capabilities, limits, seccomp filter, environment and the empty network namespace, is the same with or without
mounts (`tests/python_executor_mounts.rs` compares them from inside), except the largest file a call may write: it is
the larger of `…_TMP_MB`'s 64 MiB and the call's output size (`/tmp` itself stays a 64 MiB `tmpfs`).

The call's `out` volume appears at `/out`, writable, `nosuid`, `nodev`, `noexec`, and cannot be remounted or unmounted
from inside. Before binding it the jail checks that it is a volume of its own with a size, no bigger than the
`out_mb` the header declares, and not the volume of the call directory (`check_out_volume`); otherwise the call ends
before any code runs. Its size and inode count are what bound the output. What the program leaves there, links
included, is never followed on the trusted side: the volume is unmounted before anything is removed. `/out` is created
like `/data` when missing.

With a staging root configured the template's fixed environment gains two variables, `ARROW_DEFAULT_MEMORY_POOL=system`
and `ARROW_IO_THREADS=1`, and every call inherits them; without one the environment is exactly the five fixed variables of
`TEMPLATE_ENV`: `PATH`, three BLAS/OpenMP thread counts of 1 and `JE_ARROW_MALLOC_CONF=background_thread:false`. The last
stops the jemalloc bundled in pyarrow's x86_64 wheels from starting a `jemalloc_bg_thd` thread during `import pyarrow`
(pandas imports it when installed), which would give the template a second thread and fail its single-thread check for
every call; the check is unchanged, and only jemalloc's timer-driven purge of unused pages becomes purge-on-use.
The system pool is required under the address-space limit: with pyarrow's default allocator, versions 21 and later
reserve about 1 GiB of address space per process. pandas 1.5.3 imports pyarrow inside `import pandas` when it is
installed, so a template of such an image has it loaded. A template with a staging root checks, without importing
anything, that a pyarrow that is ALREADY loaded uses the system pool; an image without pyarrow loaded is not checked. A
wrong pool disables the mounts capability (see below); the thread-count check that follows is the one every template
already had, and a pyarrow older than 18.1 (a second thread at import) fails it for every call, as it would today. User code still cannot import pyarrow; pandas
reads Parquet for it (`pd.read_parquet(..., use_threads=False)`).

The output size is validated twice, before any mount: `out_mb` must be 1 to `OUT_MB_MAX` (1,024 MiB, the design's output
cap) both when the trusted side creates the volume (a size of 0 would mount a tmpfs with no limit) and when the jail
reads the header. The call's largest file is taken from the size of the volume the jail verified, never from the header's
claim. `SubprocessExecutor::stage_call` also keeps a budget of staged volumes in flight (only there: `StagedCall::create` and
`run_staged(CallMounts)` are public primitives without it, used by the tests; a host MUST obtain staged calls through
`stage_call`, which visibility does not enforce), `STAGED_VOLUMES_MAX` = 2 volumes
and `STAGED_OUT_MIB_MAX` = 2,048 MiB in total (the design's slots and `V = D_max + OUT_MAX`; estimates, the final values
wait for the instance memory measurement, spike item 5); a request over either gets `StageError::OverBudget` (a
`PythonExecutorError … retry later`) and mounts nothing.

Before binding `/out` the jail reads the facts of the output directory and `judge_out_volume` requires each of them: the
filesystem is a tmpfs, it has a size no bigger than the declared `out_mb`, its inode count is at most 1,024
(`OUT_MAX_INODES`, so the number of files is bounded too), it is the root of its mount (its parent is on another
device) and it is not the volume of the call directory. The `mount_out_bounded` self-test layer also probes the inode
limit (empty files stop with `ENOSPC` at or before 1,024).

Cleanup failures are logged, never swallowed: a `StagedCall` whose unmount or removal fails logs a warning with the call
id and leaves what it could not remove in place (it never deletes through a mount). Nothing else reclaims a killed
executor's leftovers, so `SubprocessExecutor::new_for_serving` (what the host and `python_executor serve` build) sweeps
the staging root before serving, `staging::sweep_staging_root`: for each entry that is a real directory named like a
generated id (32 lowercase hex digits; a link, a file or any other name is skipped and never opened) it detaches the
`out` volume and, once it is known not to be a mount, removes the directory without following links; a volume that cannot
be detached, or an `out` that cannot be opened for any reason but ENOENT (EMFILE, EACCES, EIO, a file in its place), is left
in place and logged. It cannot tell a leftover from a call in flight, so the staging root must belong
to ONE executor, and that is enforced: `new_for_serving` first takes an exclusive `flock` on `.executor.lock` inside the root
(`staging::StagingLock`, opened `O_NOFOLLOW`, held for the executor's life and released by a crash), BEFORE the sweep; a
second process on the same root gets `PythonExecutorError: the staging root is already owned by another executor` as its
startup error and sweeps nothing. A lock that cannot be taken for any OTHER reason (ENOLCK, ENOSYS, EOPNOTSUPP on some
network or FUSE volumes) does not stop the executor: nothing is swept, mounts are disabled with the reason
`staging_lock_unavailable` (logged; `stage_call` and calls asking for mounts get a typed error) and plain calls are served. The lock is released last: dropping a `SubprocessExecutor` stops the template first, then whatever still runs as a slot's
user (a call's process that outlived its call), and only then frees the root, so a successor cannot take it, and sweep it,
while a call of the old executor is alive. The sweep does not count the lock file (`.executor.lock`) as an entry it
skipped, and logs nothing when nothing was swept. If the staging volume is persistent, the prepared data of a call that was killed stays on it until the
next start of an executor with that root; a tmpfs staging volume vanishes with the instance.

The seccomp denylist of EVERY jail, staged or not, includes the new mount API, `open_tree`, `move_mount`, `fsopen`,
`fsconfig`, `fsmount`, `fspick`, `mount_setattr` and `open_tree_attr` (428-433, 442, 467), the two calls that read a
namespace's mount table, `statmount` (457) and `listmount` (458), and `kexec_file_load`, with the same action and errno
as `mount` (EPERM). The numbers are the same on x86_64 and aarch64, so none is skipped on either; the libc crate names
only some of them, the rest are listed by number. The filter matches numbers, not the kernel's table, so it loads on a
kernel that lacks a call. The one observable change for a jail that never staged anything: on a kernel lacking one of
them the call answers EPERM instead of ENOSYS (with every capability dropped it was already EPERM on a kernel that has
the call). On x86_64 the x32 guard refuses every number of 512 or more under the native arch value (the x32-only table,
512 to 547, includes an `execve` and an `execveat`, as well as the numbers with the x32 bit); the native table ends in
the 470s, so nothing the template or Python needs is affected.

A call that asks for mounts gets one more layer, so that the read-only guarantee of `/data` does not rest on the empty
effective and permitted sets alone: the capability BOUNDING set is cleared (`PR_CAPBSET_DROP` of every capability, read
back) and the INHERITABLE set is cleared (`capset`, effective and permitted left as they are, read back), while the
process is still root and before the uid change; `no_new_privs` was already set and is checked. It is tied to the call, not
to the jail, on purpose: dropping the bounding set needs `CAP_SETPCAP`, and a runtime that lacks it must lose the mounts
capability (the mount probe fails, reason `capability_drop_failed`, below) rather than the template. A call without mounts
keeps today's bounding set; a mounts call whose drop fails ends before any code runs.

### Calls with prepared tables (Linux, dark)

Dark behind `COLMENA_LARGE_TABULAR`. `tabular_run::mounted::MountedExecutor::run_with_mounts(req, call)` is the contract of
a call over prepared tables: the executor makes the verified tables readable at `/data`, runs the code and answers with the
result or a refusal. Nothing above the trait knows how the tables get there.

- *Subprocess executor* (`tabular_run::local`). `stage_call(out_mb)` takes the volume through the budgeted path (the
  unbudgeted `StagedCall::create` and `run_staged` are never used to take a call's volume), then `stage_tables` streams the
  manifest and parts into the call's `data` directory (see [54_tabular_prepare.md](./54_tabular_prepare.md)), then
  `run_staged` runs the code with `mounts` in its header. The staged call is dropped (output unmounted, directories removed)
  on a blocking thread and before `run_with_mounts` returns, so the budget share is back by then. `OUT_MIB` (256) is the size
  of the output volume asked for; an estimate until the instance is measured (spike item 5).
- *Refusals.* An executor without a staging root is `Unavailable(NoStagingRoot)`; a budget of volumes in flight that is full
  is `OverBudget(Volumes)` (retry later); any other failure to stage is `Unavailable(Executor)`; the executor's own text is
  logged and never shown. The code's own failure (a Python error, a timeout, a crash) comes back as the usual
  `PythonRunError`.
- *Remote executor: not built.* The design carries the tables on one streamed HTTP/2 request (`POST /v2/run`). Whether a
  Cloud Run service accepts a 60 MiB to 1 GiB body that way is spike item 3, which needs the deployed executor and could not
  be measured here, so `RemoteExecutor` implements the trait by refusing with `Unavailable(Unsupported)` rather than guessing
  at a wire. The in-process executor has no run mounts and does not implement the trait.

`python_exec::mounted_executor()` gives the process executor as a `MountedExecutor` (an additive accessor: `run`, the
dispatcher's routing and every existing signature are unchanged). The subprocess executor answers with itself, the remote one
with the refusing implementation above, the in-process one with `None`; a misconfigured process also gives `None`, and `run`
keeps reporting the misconfiguration. It reads the dispatcher the process already built: no executor, template or setting is
added, and with `COLMENA_LARGE_TABULAR` off nothing calls it.

The proofs are in `tests/tabular_run_mounts.rs` (Linux, root and `CAP_SYS_ADMIN`, enabled with
`COLMENA_PYEXEC_JAIL_TESTS=1` like the other jail suites; each test prints a skip line otherwise): the code lists and reads
the staged parts at `/data` and cannot write there; pandas reads a staged Parquet part in `restricted` mode with pyarrow
loaded by the template (the part is made with python3 and pyarrow, and the test skips without them); an executor without a
staging root refuses and reads nothing from storage; a call over the data limit is refused, its volume is given back and no
call directory is left; a full budget of volumes in flight is refused and stages nothing; the code's own failure comes back as
a Python error.

### Startup self-test (Linux)

Before it binds its socket, the template forks a throwaway child that enters the jail as slot 9999 (uid
`uid_base + 9999`, reserved) and checks 26 layers by their effect: descriptors, uid/gid, `no_new_privs`, the
parent-death signal, each limit (the address space within the template's size plus the call's budget), each namespace
by its `/proc/self/ns` inode (network, mount, IPC and UTS differ from the template's), the private `/proc` (the
template's pid hidden, masked entries covered), the covered paths (a directory is on another device; a file is the
read-only `/dev/null` and cannot be opened), a private `/tmp`, the syscall filter (a socket and a new process are
refused with `EPERM`; on x86_64 an x32-numbered socket call too), no network (DNS, loopback, link-local and a public
address unreachable, though with the filter `socket()` is refused before any route is tried; only `lo` and the
kernel's per-namespace fallback tunnel devices listed) and, from the template
once the probe is done, no mount added to its namespace (not counting mounts under the staging root: calls in flight
mount and unmount their output volumes there while a template starts, and a template must still start). Each result is a JSON line `{layer, ok, reason, errno?}`
with fixed reason codes. A report that does not carry exactly these layers fails as `incomplete_report`; a template
that cannot read its own namespaces, mounts or size fails as `namespace_unreadable`, `mounts_unreadable` or
`vm_size_unreadable`. A configured path the slot uid
cannot reach (a file under a directory it cannot enter) fails as `unverified`: cover that directory instead. A failure of any of these 26 logs `self_test_failed` and the template exits 3;
success is implied by `READY`.

With a staging root (`--staging-root`, see
[Run staging directories](#run-staging-directories-linux-dark)) a second probe, `selftest::run_mounts`, is a throwaway
staged call that asks for mounts and checks four more layers, 30 in all in `python_executor self-test`:
`mount_data_readonly` (a world-writable file and directory under `/data` cannot be written, it reads back, and its flags
are read-only, `nosuid`, `nodev`, `noexec`), `mount_out_bounded` (`/out` takes a file, is the size the trusted side gave
it, refuses a write past it, stops empty files at the inode bound and is `nosuid`, `nodev`, `noexec`) and
`capabilities_empty` (all five capability sets read zero, the bounding set included, `no_new_privs` is set, and each call of
the new mount API is refused with EPERM) and `staging_root_hidden` (the root IS the empty read-only tmpfs cover, listed successfully and found empty; a path that
cannot be listed fails the layer, because the probe runs as the slot user and the real directory is the executor's). A root
BELOW a path the jail covers (a volume mounted under `/mnt`, `/srv`, `/run`, `/home`, `/root`, `/var/tmp`, `/app`,
`/dev/shm`, a configured hidden path, or the jail's own `/tmp`) does not exist inside the jail, and that is hidden: the
layer passes only when it is absent AND the nearest covered ancestor is proven to be the cover (an empty read-only
tmpfs, or the fresh tmpfs `/tmp`); an absent root with no such ancestor is `unreadable`.
The probe's staged call is removed before the host's mount table is checked (which, as above, does not count mounts
under the staging root).

A failure of that second probe does NOT stop the template: it disables the mounts capability. So does a loaded pyarrow
whose pool is not the system one. The template logs the failing layers and a `mounts_disabled` event, prints
`MOUNTS_DISABLED <reason>` before `READY`, and keeps serving plain calls. Reasons: `staging_unusable` (the probe's
staged call could not be made: an unwritable or read-only staging root, a root filesystem that refuses `mkdir /data` or
`/out`), `capability_drop_failed` (the probe call could not clear its bounding or inheritable set: a runtime without `CAP_SETPCAP`),
`mount_layer_failed` (a mount layer did not hold), `arrow_pool_not_system`, `arrow_pool_unreadable`. A staging root that
cannot be locked for any reason but "held" (see below) disables mounts with `staging_lock_unavailable`. A call that
asks for mounts then gets `PythonExecutorError: run mounts are disabled on this executor (<reason>)` before any child
starts; the pool setting is therefore enforced for every mounts call. The `python_executor self-test` exit code covers
all 30.

`python_executor self-test [--uid-base N] [--tmp-mb N] [--hide /abs] [--staging-root /abs]` runs the same checks (same values as the
executor settings; root and `CAP_SYS_ADMIN` needed): exit 0 when every layer holds, 3 when one fails, 2 for bad
arguments.

## Running code you did not write

For model-written code or anything else you did not write, use `COLMENA_PYTHON_EXECUTOR=subprocess` with
`COLMENA_PYTHON_EXECUTOR_MODES=all`, on Linux, in a container where the host starts as root with `CAP_SYS_ADMIN`; run
`python_executor self-test` there first; or `COLMENA_PYTHON_EXECUTOR=remote`, also with `…_MODES=all`, against a
`python_executor serve` in such a container. In process, `restricted` is an aid to authors, not an isolation boundary.

## Remote service (`python_executor serve`)

`python_executor serve` puts the subprocess executor behind HTTP, for callers in another process or container (Linux;
root and `CAP_SYS_ADMIN`, as for `subprocess`). It serves:

- `POST /v1/run`: the body is a wire request as JSON, the answer a wire response. Each call takes a slot and runs in
  the same jail, under the same limits and output policy, as a call made by a host. A body over `…_MAX_REQUEST_MB` is
  a 413, a body that is not a wire request of this version a 400, and the deadline a request asks for is capped at
  `…_MAX_TIMEOUT_SECS`. While the template is not running a call is a 503; it never runs anywhere else.
- `GET /healthz`: 200 while the process runs, even once no usable slot is left.
- `GET /readyz`: 200 while the warm template runs and every `--require-closed-egress` target is proven closed
  (checked every minute, every 5 s while not ready) and a slot is usable (checked on each request); else 503. Once
  every slot is retired the process must be restarted, and only `/readyz` says so: act on readiness, not only on
  liveness.

The probes need no token. `--listen` defaults to `127.0.0.1:8080`. With `--token-file PATH`, every `POST /v1/run` must
carry `Authorization: Bearer <token>`, the token being the file's content, trimmed: 32 or more visible ASCII bytes. It
is checked before the body is read; a missing or wrong token is a 401 without a body. Without `--token-file` the server
starts only on a loopback address, unless `--allow-no-token` is given (for a platform that authenticates callers
itself); then startup logs the warning `python serve without a token on a non-loopback address`. The token file joins
the hidden paths by its canonical path (through any symlink), so the code a call runs cannot read it, and the startup
self-test proves that path is covered. The file is read once, at startup: a token rotated by swapping a symlink (as a
mounted secret volume does) is only picked up after a restart. In a container:
`python_executor serve --listen 0.0.0.0:8080 --token-file /secrets/token`.

`--require-closed-egress host:port[,host:port…]` (the flag may repeat) makes readiness require that every target is
proven closed: it resolves within 2 s and each of its addresses refuses the connection or lets it time out (2 s). Any
other result proves nothing and keeps the server not ready: a target that does not resolve, that accepts, or that
fails otherwise (no route, for example); a warning names the target and the error kind, if any. So a server on a
network with no route at all never becomes ready, and where DNS is blocked a host name never resolves: use IP literals
there, of hosts that would accept a connection if egress were open. A target that is not `host:port` (an IPv6 host in
brackets, a port from 1 to 65535) stops `serve` at flag parsing.

At most twice `…_SLOTS` requests are in flight, counted after the token and readiness checks and before the body is
read; one more gets a 503 with `Retry-After: 1`. Calls run `…_SLOTS` at a time and the rest upload and wait, so no
slot idles between calls and the bodies held in memory stay near `2 × slots × …_MAX_REQUEST_MB` (plus one decoded
copy per zstd request).

Bodies may be zstd-compressed: a request with `Content-Encoding: zstd` is decompressed up to `…_MAX_REQUEST_MB` (past
it a 413; a body that is not a zstd frame, a 400), and any other encoding than `identity` is a 415. An answer is
compressed when the request's `Accept-Encoding` lists `zstd`. The server speaks HTTP/1.1 and HTTP/2 without TLS (h2c,
prior knowledge), so a platform that forwards HTTP/2 to the container reaches it.

The executor settings are the `COLMENA_PYTHON_EXECUTOR_*` variables a host reads (`…_BIN`, `…_SLOTS`, `…_MEMORY_MB`,
`…_HIDE_PATHS`, `…_MAX_REQUEST_MB`, `…_MAX_RESPONSE_MB`, `…_REFUSE_OUTPUT`, `…_MAX_TIMEOUT_SECS`), with the same
defaults and ranges; there are no flags for them. Before it listens, the server runs the jail self-test in its own
process. Exit codes: 0 after SIGTERM or Ctrl-C (a graceful stop); 1 when serving fails (the address cannot be bound,
for example); 2 for a bad configuration (an invalid variable, named in the error; no token file on a non-loopback
address without `--allow-no-token`; a token file that cannot be read or holds a short token; a malformed egress
target); 3 when the self-test fails, each failed layer logged.

Logs (target `colmena::python_exec`, level from `RUST_LOG`, `info` by default) are fields only: one `python serve run`
event per call with `request_id`, `outcome`, `in_bytes`, `wire_in_bytes` (the body as sent), `out_bytes` and
`duration_ms`. `request_id` is the caller's `X-Colmena-Request-Id` header reduced to 64 characters of
`[A-Za-z0-9._:-]` (`-` without one). They never carry code, inputs, outputs, stdout, tokens or other headers.

## Remote executor settings

`COLMENA_PYTHON_EXECUTOR=remote` needs `COLMENA_PYTHON_EXECUTOR_URL`: without it startup stops with
`…=remote needs COLMENA_PYTHON_EXECUTOR_URL`. The settings are read whenever
`COLMENA_PYTHON_EXECUTOR_URL` is set, whatever the executor, so an invalid one stops startup like any other variable,
named in the error. The URL only ever comes from the process environment, never from graph data, and its error never
echoes the value.

| Variable | Values | Default |
|---|---|---|
| `COLMENA_PYTHON_EXECUTOR_URL` | the service's absolute URL: `https`, or `http` only for a loopback host (`127.0.0.0/8`, `::1`, `localhost`); no user, password, query or fragment | unset: no remote settings |
| `COLMENA_PYTHON_EXECUTOR_AUTH` | required with a URL: `none`, `bearer_file` or `gcp_id_token` (`https` only) | — |
| `COLMENA_PYTHON_EXECUTOR_TOKEN_FILE` | required with `bearer_file`: a file holding the same token as the service's `--token-file` | — |
| `COLMENA_PYTHON_EXECUTOR_AUDIENCE` | with `gcp_id_token`, the audience the identity token is bound to | the URL's origin (`https://svc.example`, with its port if any) |
| `COLMENA_PYTHON_EXECUTOR_MAX_WIRE_MB` | 1 to 4095: a cap on the compressed request body, for a transport in front of the service that limits request size (HTTP/1 fronts commonly cap at 32 MiB) | unset: no cap |

The request and response limits are `…_MAX_REQUEST_MB` and `…_MAX_RESPONSE_MB`, as for `subprocess`. For
`gcp_id_token`, `python_exec::id_token` asks the GCP metadata server of the host's own runtime identity for an
identity token bound to the audience (`…/service-accounts/default/identity`, `format=standard`, 5 s timeout). The
token is kept in memory only and fetched again once less than 5 minutes remain before its `exp` claim. Concurrent
callers wait for one fetch and share its token or its error, so a failing server is asked once, not once per waiting
call. If a refresh fails while the cached token has more than 30 s left, that token is used and the next call tries
again. The claim is read without checking the signature: the token is only forwarded, and the service checks it.
The token is never logged, and the error of a failed fetch names what failed (no answer, the HTTP status, an
unreadable answer or one that is not a JWT with `exp`), never the answer itself.

### The remote client

`python_exec::remote::RemoteExecutor` sends each call as `POST <URL>/v1/run`, keeping a path in
the URL, with an `X-Colmena-Request-Id` that the service logs. Nothing is sent over `…_MAX_REQUEST_MB` or, compressed,
over `…_MAX_WIRE_MB`, and an answer over `…_MAX_RESPONSE_MB` fails. Bodies are zstd-compressed; serve behind HTTP/2
end to end when inputs may exceed what an HTTP/1 front accepts, or set `COLMENA_PYTHON_EXECUTOR_MAX_WIRE_MB` so
oversized calls fail clearly. Proxy variables and redirects are ignored, and the token file is read per call.

A call ends within its timeout plus 30 s, credentials and waits included, on a connection of its own (none is kept for
reuse). Only a call that certainly did not run is sent again, with the same request id: after a connection error (a
connect timeout too) or a 503 without `Retry-After` (not ready, no usable slot), once, 250 ms later; after 429, or 503
with `Retry-After` (a full `serve`), when it says (at most 2 s), while the code would still get its whole timeout, so
waits fit in the 30 s. 502, 504, a lost or cut answer, late credentials or a wait that no longer fits is
`PythonExecutorError: … unavailable (…)`, never an in-process run or the code's timeout. 401/403 is `… rejected this
caller's credentials`, 413 `Python execution error: the input exceeds what the isolated Python executor accepts`,
another status `… answered HTTP <status>`. A call's first resend logs `python remote call retried` with `request_id` and
`reason` only. `warm` (what `wait_until_ready` awaits) waits up to 120 s in all, asking every second, for `GET
<URL>/readyz` to answer 200 and then for an empty `POST <URL>/v1/run` with the credentials to answer 400, taken as ready
(`serve` runs nothing without a body), so a wire-version mismatch or a front that answers 400 before checking
credentials passes it; 401, 403 or credentials it cannot obtain end the wait at once, so a wrong token shows at startup.

## Equivalence with the in-process executor

`tests/python_executor_golden.rs` runs 17 cases (plain scripts, `restricted` refusals, syntax and runtime errors, a
value that cannot become JSON, and the four tabular tools' wrappers over pandas) both in process and through the
executor the environment selects, and requires the same result, stdout included. Each case also states what it must
produce (a value, or an error of a given kind), so two identical failures do not count as a match. With the default
`inprocess` there is nothing to compare and the test says so; run it with `COLMENA_PYTHON_EXECUTOR=subprocess`
(`COLMENA_PYTHON_EXECUTOR_MODES=all` to route every case) in a root container with `CAP_SYS_ADMIN`, or with
`remote` against a `python_executor serve` running in one.

Concurrent calls: through `subprocess` each call has its own process, so each gets only its own stdout. In process,
the executor swaps the interpreter's `sys.stdout` for each call, so two calls running at the same time can mix what
they print; `concurrent_in_process_calls_keep_their_own_stdout` records that and is ignored by default.

## Testing

The jail suites (`tests/python_executor_subprocess.rs`, `tests/python_executor_isolation.rs`, `tests/python_executor_mounts.rs`) and the equivalence bench
need Linux, root and `CAP_SYS_ADMIN`; they run only with `COLMENA_PYEXEC_JAIL_TESTS=1` and otherwise print a skip line.
Locally, run them in a container:
`docker run --rm --cap-add SYS_ADMIN --security-opt seccomp=unconfined --security-opt apparmor=unconfined
-e COLMENA_PYEXEC_JAIL_TESTS=1 -v "$PWD":/work -w /work <image with Rust, python3-dev and pandas> cargo test --test
python_executor_subprocess --test python_executor_isolation`.

The one test that reads a Parquet part through `/data` needs pyarrow for the system Python, which the job's Debian
packages do not include (a decision for the sandbox owner): without it the test prints a `NOTE` and passes; with
`COLMENA_PYEXEC_EXPECT_PYARROW=1` (set it wherever pyarrow is installed) a missing pyarrow prints the skip line the
job's step fails on and fails the test.

CI runs them in the `python-executor` job of `ci-develop.yml` (Debian bookworm container with those options, pandas,
numpy and scipy from Debian): the executor's unit tests, both jail suites (the step fails on any skip line),
`python_executor self-test`, the Python node and tool suites and the equivalence bench under `subprocess` with
`COLMENA_PYTHON_EXECUTOR_MODES=all`, the bench again with the default modes, and the smoke graph under `subprocess`
(four `python run` events with `outcome="ok"`); then, against a local `python_executor serve` (loopback, no token, 2
slots), the same suites, the bench (its burst of 12 calls exceeds the 4 in flight) and the smoke graph under `remote`.
`tests/python_executor_remote.rs`, a jail suite, runs cases through `serve`'s router with a token and compares them
with in process. Four `gsheets_run_python` tests are skipped there by name: with pandas installed they fail the same
way under every executor, and no job runs their assertions today.

## About `restricted`

`restricted` validates imports and a few builtins before running. It helps
authors stay within the supported module set; it is not an isolation boundary.
Isolation comes from the `subprocess` executor (Linux), in the host or behind `remote`.

## Observability

Target `colmena::python_exec`:

```
RUST_LOG=colmena::python_exec=debug
```

An `info` event, `"python executor installed"`, fires once per process, on the
first successful `install_from_env` call, with `executor` and `modes`. A
`debug` event, `"python run"`, fires once per call to `run` that reaches the
installed executor, with `executor`, `mode`, `code_len`, `duration_ms` and
`outcome` (`ok`, `python_error`, `timeout`, `internal`). Neither event ever
carries code, inputs, outputs or error text.

Neither event fires when the configuration is invalid — there is no installed
executor to log from, only the `PythonExecutorError: …` returned to the
caller. The `debug` event also does not fire for a call made through `scope`
(the test-only override that swaps in a different executor for one async
call): `scope` skips the dispatcher entirely, so the request it wraps never
reaches the code that emits the event.
