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

## Wire protocol (isolated executors)

Isolated executors (not available in this build yet) talk to the process that
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
host runs as root (children switch to unprivileged users). The template is warmed in the background at startup; while it
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
reboot calls return `EPERM`; `clone3` returns `ENOSYS`, so threads are created through `clone`; on x86_64 every x32
syscall number returns `EPERM`; other architectures have no filter and the jail does not start. Threads work. Code that
uses `multiprocessing`, `subprocess`, an `asyncio` event loop (its self-pipe is a socket pair) or a library that probes
the system with a subprocess gets a `PermissionError` or `OSError`. The template needs root and `CAP_SYS_ADMIN`; without them every
call ends with the crashed text. The uid range `uid_base..uid_base+slots` must belong to one executor per PID
namespace and to no real user.

A call cannot start a process. As a second line, before a slot is reused, everything running as its uid is stopped
(and the template reaps the orphans). If that cannot be done even on a second try, the slot is retired; once none is left,
calls fail with `PythonExecutorError: no usable Python slot is left…` until the process restarts. The host signals a
call's child through a pidfd (the pid when pidfds are unavailable).

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
once the probe is done, no mount added to its namespace. Each result is a JSON line `{layer, ok, reason, errno?}`
with fixed reason codes. A report that does not carry exactly these layers fails as `incomplete_report`; a template
that cannot read its own namespaces, mounts or size fails as `namespace_unreadable`, `mounts_unreadable` or
`vm_size_unreadable`. A configured path the slot uid
cannot reach (a file under a directory it cannot enter) fails as `unverified`: cover that directory instead. Any
failure logs `self_test_failed` and the template exits 3; success is implied by `READY`.
`python_executor self-test [--uid-base N] [--tmp-mb N] [--hide /abs]` runs the same checks (same values as the
executor settings; root and `CAP_SYS_ADMIN` needed): exit 0 when every layer holds, 3 when one fails, 2 for bad
arguments.

## Running code you did not write

For model-written code or anything else you did not write, use `COLMENA_PYTHON_EXECUTOR=subprocess` with
`COLMENA_PYTHON_EXECUTOR_MODES=all`, on Linux, in a container where the host starts as root with `CAP_SYS_ADMIN`; run
`python_executor self-test` there first. In process, `restricted` is an aid to authors, not an isolation boundary.

## Remote service (`python_executor serve`)

`python_executor serve` puts the subprocess executor behind HTTP, for callers in another process or container (Linux;
root and `CAP_SYS_ADMIN`, as for `subprocess`). It serves:

- `POST /v1/run`: the body is a wire request as JSON, the answer a wire response. Each call takes a slot and runs in
  the same jail, under the same limits and output policy, as a call made by a host. A body over `…_MAX_REQUEST_MB` is
  a 413, a body that is not a wire request of this version a 400, and the deadline a request asks for is capped at
  `…_MAX_TIMEOUT_SECS`. While the template is not running a call is a 503; it never runs anywhere else.
- `GET /healthz`: 200 while the process runs.
- `GET /readyz`: 200 while the warm template runs, else 503; checked every minute, every 5 s while not ready.

The probes need no token. `--listen` defaults to `127.0.0.1:8080`. With `--token-file PATH`, every `POST /v1/run` must
carry `Authorization: Bearer <token>`, the token being the file's content, trimmed: 32 or more visible ASCII bytes. It
is checked before the body is read; a missing or wrong token is a 401 without a body. Without `--token-file` the server
starts only on a loopback address. The token file joins the hidden paths by its canonical path (through any symlink),
so the code a call runs cannot read it, and the startup self-test proves that path is covered. In a container:
`python_executor serve --listen 0.0.0.0:8080 --token-file /secrets/token`.

The executor settings are the `COLMENA_PYTHON_EXECUTOR_*` variables a host reads (`…_BIN`, `…_SLOTS`, `…_MEMORY_MB`,
`…_HIDE_PATHS`, `…_MAX_REQUEST_MB`, `…_MAX_RESPONSE_MB`, `…_REFUSE_OUTPUT`, `…_MAX_TIMEOUT_SECS`), with the same
defaults and ranges; there are no flags for them. Before it listens, the server runs the jail self-test in its own
process. Exit codes: 0 after SIGTERM or Ctrl-C (a graceful stop); 1 when serving fails (the address cannot be bound,
for example); 2 for a bad configuration (an invalid variable, named in the error; no token file on a non-loopback
address; a token file that cannot be read or holds a short token); 3 when the self-test fails, each failed layer logged.

Logs (target `colmena::python_exec`, level from `RUST_LOG`, `info` by default) are fields only: one `python serve run`
event per call with `outcome`, `in_bytes`, `out_bytes` and `duration_ms`. They never carry code, inputs, outputs,
stdout, tokens or headers.

## Equivalence with the in-process executor

`tests/python_executor_golden.rs` runs 17 cases (plain scripts, `restricted` refusals, syntax and runtime errors, a
value that cannot become JSON, and the four tabular tools' wrappers over pandas) both in process and through the
executor the environment selects, and requires the same result, stdout included. Each case also states what it must
produce (a value, or an error of a given kind), so two identical failures do not count as a match. With the default
`inprocess` there is nothing to compare and the test says so; run it with `COLMENA_PYTHON_EXECUTOR=subprocess`
(`COLMENA_PYTHON_EXECUTOR_MODES=all` to route every case) in a root container with `CAP_SYS_ADMIN`.

Concurrent calls: through `subprocess` each call has its own process, so each gets only its own stdout. In process,
the executor swaps the interpreter's `sys.stdout` for each call, so two calls running at the same time can mix what
they print; `concurrent_in_process_calls_keep_their_own_stdout` records that and is ignored by default.

## Testing

The jail suites (`tests/python_executor_subprocess.rs`, `tests/python_executor_isolation.rs`) and the equivalence bench
need Linux, root and `CAP_SYS_ADMIN`; they run only with `COLMENA_PYEXEC_JAIL_TESTS=1` and otherwise print a skip line.
Locally, run them in a container:
`docker run --rm --cap-add SYS_ADMIN --security-opt seccomp=unconfined --security-opt apparmor=unconfined
-e COLMENA_PYEXEC_JAIL_TESTS=1 -v "$PWD":/work -w /work <image with Rust, python3-dev and pandas> cargo test --test
python_executor_subprocess --test python_executor_isolation`.

CI runs them in the `python-executor` job of `ci-develop.yml` (Debian bookworm container with those options, pandas,
numpy and scipy from Debian): the executor's unit tests, both jail suites (the step fails on any skip line),
`python_executor self-test`, the Python node and tool suites and the equivalence bench under `subprocess` with
`COLMENA_PYTHON_EXECUTOR_MODES=all`, the bench again with the default modes, and the smoke graph under `subprocess`
(four `python run` events with `outcome="ok"`). Four `gsheets_run_python` tests are skipped there by name: with pandas
installed they fail the same way under every executor, and no job runs their assertions today.

## About `restricted`

`restricted` validates imports and a few builtins before running. It helps
authors stay within the supported module set; it is not an isolation boundary.
Isolation comes from the `subprocess` executor (Linux).

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
