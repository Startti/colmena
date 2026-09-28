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

### The subprocess executor (Linux, not selectable yet)

`python_exec::subprocess` supervises the warm template: it starts it from a dedicated long-lived thread (the template
dies with that thread) with only `TEMPLATE_ENV`, replaces it when it exits, and retries a failed start after a pause.
Its stderr is read as JSON events with known fields only; any other line is dropped and counted. Settings:

| Variable | Default |
|---|---|
| `COLMENA_PYTHON_EXECUTOR_BIN` | `python_executor` |
| `COLMENA_PYTHON_EXECUTOR_SLOTS` | CPU cores, at most 8 (1-64) |
| `COLMENA_PYTHON_EXECUTOR_MEMORY_MB` | 2048 (256-65536) |
| `COLMENA_PYTHON_EXECUTOR_HIDE_PATHS` | none (`:`-separated absolute paths, no `..`) |
| `COLMENA_PYTHON_EXECUTOR_MAX_REQUEST_MB`, `…_MAX_RESPONSE_MB` | 256 (1-4095) |

Each call takes a slot, checks the request size first, forks a child through the template and reads one capped
response; a passed deadline kills the child (`Timeout`), and an abandoned call's child is killed and its slot freed
once it is gone. The template gets only the host's `LANG`, `LC_ALL` and `LC_CTYPE` (same text encodings as in process),
no `PYTHONPATH`/`PYTHONHOME`. Memory and CPU limits are sent but not enforced yet. The host must ignore `SIGPIPE`.

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
before any code runs. There is no syscall filter yet. The template needs root and `CAP_SYS_ADMIN`; without them every
call ends with the crashed text. The uid range `uid_base..uid_base+slots` must belong to one executor per PID
namespace and to no real user.

Processes a call starts end with the call: before a slot is reused, everything running as its uid is stopped (and
the template reaps the orphans). If that cannot be done even on a second try, the slot is retired; once none is left,
calls fail with `PythonExecutorError: no usable Python slot is left…` until the process restarts. The host signals a
call's child through a pidfd (the pid when pidfds are unavailable).

### Startup self-test (Linux)

Before it binds its socket, the template forks a throwaway child that enters the jail as slot 9999 (uid
`uid_base + 9999`, reserved) and checks 24 layers by their effect: descriptors, uid/gid, `no_new_privs`, the
parent-death signal, each limit (the address space within the template's size plus the call's budget), each namespace
by its `/proc/self/ns` inode (network, mount, IPC and UTS differ from the template's), the private `/proc` (the
template's pid hidden, masked entries covered), the covered paths (a directory is on another device; a file is the
read-only `/dev/null` and cannot be opened), a private `/tmp`, no network (DNS, loopback, link-local and a public
address unreachable; only `lo` and the kernel's per-namespace fallback tunnel devices listed) and, from the template
once the probe is done, no mount added to its namespace. Each result is a JSON line `{layer, ok, reason, errno?}`
with fixed reason codes. A report that does not carry exactly these layers fails as `incomplete_report`; a template
that cannot read its own namespaces, mounts or size fails as `namespace_unreadable`, `mounts_unreadable` or
`vm_size_unreadable`. A configured path the slot uid
cannot reach (a file under a directory it cannot enter) fails as `unverified`: cover that directory instead. Any
failure logs `self_test_failed` and the template exits 3; success is implied by `READY`.
`python_executor self-test [--uid-base N] [--tmp-mb N] [--hide /abs]` runs the same checks (same values as the
executor settings; root and `CAP_SYS_ADMIN` needed): exit 0 when every layer holds, 3 when one fails, 2 for bad
arguments.

## About `restricted`

`restricted` validates imports and a few builtins before running. It helps
authors stay within the supported module set; it is not an isolation boundary.
No isolated executor is available in this build yet.

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
