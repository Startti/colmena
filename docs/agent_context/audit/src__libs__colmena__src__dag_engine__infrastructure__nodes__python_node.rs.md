# src/libs/colmena/src/dag_engine/infrastructure/nodes/python_node.rs

**Layer:** infrastructure  **Purpose:** ExecutableNode implementation for sandboxed Python script execution with import whitelisting, banned builtin blocking, stdout capture, and JSON I/O.

## Symbols

- `PythonNode` (struct, pub) — marker struct implementing ExecutableNode trait for python_script DAG node type
- `SANDBOX_VALIDATOR` (const, pub) — embedded Python AST validator script that checks imports against whitelist and blocks banned builtins
- `validate_sandbox` (fn, private) — runs SANDBOX_VALIDATOR on user code via PyO3 and returns violation message if found
- `PythonRunResult` (struct, pub) — holds serialized JSON output variable and captured stdout from sandboxed execution
- `execute_sandboxed_helper` (fn, pub) — core helper: executes Python code with optional sandbox AST validation, input injection, stdout capture, and error reporting; used by python_script node and other modules (e.g., crdt_doc_run_python)
- `ExecutableNode::execute` (impl async method) — DAG node execute: extracts code from inputs/config, strips markdown wrappers, validates sandbox if restricted, injects non-reserved inputs as Python globals, spawns blocking task with optional timeout, returns raw Python output value
- `ExecutableNode::default_output` (impl method) — returns None to pass through raw output value instead of extracting an "output" wrapper field
- `ExecutableNode::schema` (impl method) — returns comprehensive JSON schema documenting code/sandbox_mode/sandbox_timeout_secs config, reserved/custom inputs, and raw output semantics
- `test_python_math_logic` (test fn) — verifies arithmetic with variable injection (x=10, y=5 → output=52)
- `test_python_imports` (test fn) — verifies math module import in unrestricted mode
- `test_sandbox_allows_clean_code` (test fn) — verifies simple arithmetic passes restricted sandbox
- `test_sandbox_allows_whitelisted_import` (test fn) — verifies whitelisted math import in restricted mode
- `test_sandbox_blocks_os_import` (test fn) — verifies os module rejected in restricted mode with violation message
- `test_sandbox_blocks_open_builtin` (test fn) — verifies open() builtin blocked in restricted mode
- `test_sandbox_blocks_eval` (test fn) — verifies eval() builtin blocked in restricted mode
- `test_sandbox_none_mode_allows_os` (test fn) — verifies unrestricted mode permits os import
- `test_sandbox_default_mode_allows_os` (test fn) — verifies default mode (none) permits os import
- `restricted_mode_allows_pandas_import` (test fn) — verifies pandas allowed in restricted mode (subsystem C, 2026-06)
- `restricted_mode_allows_numpy_import` (test fn) — verifies numpy allowed in restricted mode
- `restricted_mode_allows_scipy_stats_import` (test fn) — verifies scipy.stats allowed in restricted mode
- `restricted_mode_still_rejects_requests_import` (test fn) — verifies requests module blocked despite being in Python stdlib
- `test_sandbox_skips_reserved_keys_as_python_vars` (test fn) — verifies reserved keys (sandbox_mode, code, etc.) are not injected as Python variables
- `restricted_allows_signing_primitives` (test fn) — verifies HMAC/hashlib/base64/secrets allowed in restricted mode for request signing without granting network access
- `restricted_still_blocks_network_and_process_modules` (test fn) — verifies network (urllib, socket, requests) and process (os, subprocess, sys, shutil) modules remain banned
- `restricted_blocks_urllib_submodules` (test fn) — verifies root-module matching blocks urllib.parse to prevent network egress via submodule import

## File-level notes

- **Sandbox strategy:** AST-based (not execution-based) validation of import statements + Name-node calls to banned builtins. Whitelist includes math, json, re, datetime, collections, itertools, functools, string, decimal, statistics (standard analysis), plus pandas, numpy, scipy (subsystem C, 2026-06), plus HMAC/hashlib/base64/secrets for request signing without network egress (documented in lines 23-26).
- **Timeout asymmetry:** Timeout enforcement (via `tokio::time::timeout`) only applies in restricted sandbox mode; unrestricted mode ("none") has no timeout and can hang indefinitely.
- **Output semantics:** `execute_sandboxed_helper` returns `PythonRunResult { output: Option<Value>, stdout }`, but the DAG node's execute() unwraps output to null, losing the distinction between "user didn't assign output" and "user assigned null". Documented in schema as required assignment.
- **I/O boundary:** Code/sandbox_mode/sandbox_timeout_secs are reserved (not injected as Python variables). All other inputs converted JSON→Python via `pythonize` crate.
- **Stdout capture:** Orchestrated from Rust via `sys.stdout = io.StringIO()` before execution, restored after. Enables capture in restricted mode without requiring user to `import sys`.
- **Markdown handling:** Heuristic stripping of triple-backtick wrappers (lines 203-209); edge case: backticks inside strings not escaped.
- **Note on timeout testing:** Direct e2e test of `sandbox_timeout_secs` would deadlock the test harness (underlying blocking task holds GIL and cannot be cancelled even after timeout fires). Timeout wiring verified via `tests/graphs/agents/python_sandbox_tool_test.json` instead (line 474-481).
- **Security consideration:** Line 211 includes a `println!()` that logs user code to stdout during execution. This may leak secrets/PII in code strings and is not gated by a feature flag.
