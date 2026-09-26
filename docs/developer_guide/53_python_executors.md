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
