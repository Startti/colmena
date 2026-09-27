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
| `COLMENA_PYTHON_EXECUTOR_HIDE_PATHS` | none (`:`-separated paths) |
| `COLMENA_PYTHON_EXECUTOR_MAX_REQUEST_MB`, `…_MAX_RESPONSE_MB` | 256 (1-4095) |

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
