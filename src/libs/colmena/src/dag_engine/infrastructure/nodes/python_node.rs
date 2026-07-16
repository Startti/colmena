use crate::dag_engine::domain::node::{ExecutableNode, NodeInputs};
use async_trait::async_trait;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyString};
use pythonize::{depythonize, pythonize};
use serde_json::{json, Value};
use std::error::Error as StdError;
use std::sync::Arc;

pub struct PythonNode;

const SANDBOX_VALIDATOR: &str = r#"
import ast as _ast

_ALLOWED_IMPORTS = {
    'math', 'json', 're', 'datetime', 'collections',
    'itertools', 'functools', 'string', 'decimal', 'statistics',
    # crdt_doc_run_python additions (subsistema C, 2026-06):
    'pandas', 'numpy', 'scipy',
    # Request-signing primitives. APIs such as NetSuite TBA, AWS SigV4 or
    # Shopify webhooks require an HMAC signature computed per request, which is
    # impossible from a static header. These modules are pure computation plus,
    # for `secrets`, an OS entropy read — none of them opens a socket or a file.
    # Sandboxed code still cannot perform the call itself: network modules
    # (`urllib`, `socket`, `requests`, ...) stay banned, so the signed request
    # must go out through the `http_request` node, where it remains auditable.
    'hmac', 'hashlib', 'base64', 'secrets',
}
_BANNED_BUILTINS = {'open', 'exec', 'eval', 'compile', '__import__'}

_violation = None

try:
    _tree = _ast.parse(_code_to_validate)
    for _node in _ast.walk(_tree):
        if _violation:
            break
        if isinstance(_node, _ast.Import):
            for _alias in _node.names:
                _name = _alias.name.split('.')[0]
                if _name not in _ALLOWED_IMPORTS:
                    _allowed = ', '.join(sorted(_ALLOWED_IMPORTS))
                    _violation = f"SandboxViolation: import '{_name}' is not allowed. Allowed imports: {_allowed}"
                    break
        elif isinstance(_node, _ast.ImportFrom):
            if _node.module:
                _name = _node.module.split('.')[0]
                if _name not in _ALLOWED_IMPORTS:
                    _allowed = ', '.join(sorted(_ALLOWED_IMPORTS))
                    _violation = f"SandboxViolation: import '{_name}' is not allowed. Allowed imports: {_allowed}"
        elif isinstance(_node, _ast.Call):
            _func = _node.func
            if isinstance(_func, _ast.Name) and _func.id in _BANNED_BUILTINS:
                _violation = f"SandboxViolation: '{_func.id}' is not allowed in sandbox mode"
except SyntaxError as _e:
    _violation = f"SyntaxError: {_e}"
"#;

fn validate_sandbox(py: Python<'_>, code: &str) -> Result<Option<String>, String> {
    let locals = PyDict::new(py);
    locals
        .set_item("_code_to_validate", PyString::new(py, code))
        .map_err(|e| format!("Validator setup error: {e}"))?;
    let validator_c = std::ffi::CString::new(SANDBOX_VALIDATOR)
        .map_err(|e| format!("Validator setup error: {e}"))?;
    py.run(validator_c.as_c_str(), Some(&locals), Some(&locals))
        .map_err(|e| format!("Validator execution error: {e}"))?;
    match locals.get_item("_violation") {
        Ok(Some(v)) if !v.is_none() => {
            let msg: String = v
                .extract()
                .map_err(|e| format!("Validator result error: {e}"))?;
            Ok(Some(msg))
        }
        _ => Ok(None),
    }
}

/// Result of running a Python code string via the sandboxed helper.
#[derive(Debug)]
pub struct PythonRunResult {
    /// The serialized value of the `output` variable in the user's namespace,
    /// or `None` if the user did not assign `output`.
    pub output: Option<Value>,
    /// Captured stdout (best-effort — Python `print()` calls).
    pub stdout: String,
}

/// Run a Python code string with the same semantics as the `python_script`
/// DAG node. Used directly by other modules (e.g. `crdt_doc_run_python` tool)
/// that need fine-grained control of the namespace and result extraction.
///
/// * `sandbox_mode`: `"none"` (full Python) or `"restricted"` (AST validation
///   + import whitelist + banned-builtin enforcement).
/// * `_timeout_secs`: reserved for the caller. This helper does NOT enforce a
///   timeout — wrap the call in `tokio::task::spawn_blocking` +
///   `tokio::time::timeout` if you need it.
/// * `inputs`: a map of variable_name → JSON value to inject as Python globals
///   before executing the code.
///
/// Errors: returns `Err(String)` on sandbox violation, syntax error, or
/// runtime exception (with message + Python traceback when available).
///
/// Note: stdout is captured via a `sys.stdout` redirect orchestrated from
/// Rust (not from user code), so the redirect does NOT require the user
/// code to import `sys` — it remains compatible with `restricted` mode.
pub fn execute_sandboxed_helper(
    code: &str,
    sandbox_mode: &str,
    _timeout_secs: u64,
    inputs: &serde_json::Map<String, Value>,
) -> Result<PythonRunResult, String> {
    Python::attach(|py| -> Result<PythonRunResult, String> {
        // 1. Sandbox AST validation (only in restricted mode).
        if sandbox_mode == "restricted" {
            if let Some(violation) = validate_sandbox(py, code)? {
                return Err(violation);
            }
        }

        // 2. Build a single namespace dict used as both globals and locals,
        // matching PythonNode::execute semantics exactly.
        let locals = PyDict::new(py);
        for (key, value) in inputs.iter() {
            let py_val = pythonize(py, value)
                .map_err(|e| format!("Failed to convert input '{}' to Python: {}", key, e))?;
            locals
                .set_item(key, py_val)
                .map_err(|e| format!("Failed to set input '{}': {}", key, e))?;
        }

        // 3. Set up stdout capture via sys.stdout = io.StringIO().
        // Orchestrated from Rust so it works even in restricted mode (no
        // `import sys` in user code required).
        let sys_module = py.import("sys").map_err(|e| format!("import sys: {e}"))?;
        let io_module = py.import("io").map_err(|e| format!("import io: {e}"))?;
        let stdout_capture = io_module
            .call_method0("StringIO")
            .map_err(|e| format!("create StringIO: {e}"))?;
        let original_stdout = sys_module
            .getattr("stdout")
            .map_err(|e| format!("get sys.stdout: {e}"))?;
        sys_module
            .setattr("stdout", &stdout_capture)
            .map_err(|e| format!("redirect stdout: {e}"))?;

        // 4. Execute user code.
        let code_c = std::ffi::CString::new(code)
            .map_err(|e| format!("Python execution error: invalid code string: {e}"))?;
        let exec_result = py.run(code_c.as_c_str(), Some(&locals), Some(&locals));

        // 5. Always restore stdout BEFORE returning, regardless of exec result.
        let _ = sys_module.setattr("stdout", original_stdout);

        // 6. Capture stdout text (best-effort).
        let stdout = stdout_capture
            .call_method0("getvalue")
            .and_then(|v| v.extract::<String>())
            .unwrap_or_default();

        // 7. Surface execution errors AFTER restoring stdout.
        if let Err(e) = exec_result {
            return Err(format!("Python execution error: {}", e));
        }

        // 8. Extract `output` if defined. See PythonNode::execute for the
        // Python ↔ JSON boundary rationale (no auto-coercion).
        let output = match locals.get_item("output") {
            Ok(Some(output_obj)) => {
                let val: Value = depythonize(&output_obj)
                    .map_err(|e| format!("Failed to convert Python 'output' to JSON: {}", e))?;
                Some(val)
            }
            _ => None,
        };

        Ok(PythonRunResult { output, stdout })
    })
}

#[async_trait]
impl ExecutableNode for PythonNode {
    async fn execute(
        &self,
        inputs: &NodeInputs,
        config: &Value,
        _state: &mut Value,
        _observer: Option<Arc<dyn crate::dag_engine::domain::observer::ExecutionObserver>>,
    ) -> Result<Value, Box<dyn StdError + Send + Sync>> {
        // 1. Extract code — input port takes priority over config
        let code = if let Some(input_code) = inputs.get("code").and_then(|v| v.as_str()) {
            input_code.to_string()
        } else {
            config
                .get("code")
                .and_then(|v| v.as_str())
                .ok_or("PythonNode error: 'code' field is missing in inputs or config")?
                .to_string()
        };

        // Strip markdown code blocks (LLMs often wrap code in ```python ... ```)
        let code = code.trim();
        let code = if code.starts_with("```") {
            let start = code.find('\n').map(|i| i + 1).unwrap_or(0);
            let end = code.rfind("```").unwrap_or(code.len());
            code[start..end].trim()
        } else {
            code
        };

        println!("[PythonNode] Executing code:\n{}", code);

        // 2. Extract sandbox config
        let sandbox_mode = inputs
            .get("sandbox_mode")
            .or_else(|| config.get("sandbox_mode"))
            .and_then(|v| v.as_str())
            .unwrap_or("none")
            .to_string();

        let timeout_secs = inputs
            .get("sandbox_timeout_secs")
            .or_else(|| config.get("sandbox_timeout_secs"))
            .and_then(|v| v.as_u64())
            .unwrap_or(10);

        // 3. Prepare inputs for the helper — skip sandbox config keys.
        // Intrinsic Python ↔ JSON limitation for the `output` value: see
        // execute_sandboxed_helper docs and docs/developer_guide/26_python_node.md
        // → "The Python ↔ JSON Boundary" for the full list and recommended
        // coercions. We do NOT auto-coerce.
        let sandbox_keys = ["sandbox_mode", "sandbox_timeout_secs", "code"];
        let helper_inputs: serde_json::Map<String, Value> = inputs
            .iter()
            .filter(|(k, _)| !sandbox_keys.contains(&k.as_str()))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        let code = code.to_string();
        let sandbox_mode_clone = sandbox_mode.clone();

        // 4. Schedule blocking execution (CPython is not async-safe).
        let blocking_task = tokio::task::spawn_blocking(move || -> Result<Value, String> {
            let result =
                execute_sandboxed_helper(&code, &sandbox_mode_clone, timeout_secs, &helper_inputs)?;
            Ok(result.output.unwrap_or(Value::Null))
        });

        // 5. Apply timeout in restricted mode; plain await otherwise.
        let output_json = if sandbox_mode == "restricted" {
            tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), blocking_task)
                .await
                .map_err(|_| {
                    format!(
                        "SandboxTimeout: execution exceeded {} seconds",
                        timeout_secs
                    )
                })?
                .map_err(|e| format!("Task join error: {e}"))?
                .map_err(|e| -> Box<dyn StdError + Send + Sync> { e.into() })?
        } else {
            blocking_task.await??
        };

        Ok(output_json)
    }

    fn default_output(&self) -> Option<&str> {
        // The node returns the raw value of the Python `output` variable, NOT a
        // wrapper like { "output": <value> }. Returning None here tells the edge
        // resolver to pass the raw value through instead of trying to extract a
        // non-existent "output" field — otherwise scalar outputs (number, string,
        // bool) get silently dropped to null on implicit edges.
        None
    }

    fn schema(&self) -> Value {
        json!({
            "name": "python_script",
            "description": "Executes Python code. Code can be provided via input 'code' or config 'code' (input takes priority). Non-reserved inputs are injected as Python variables. The script must assign its result to a variable named 'output'.",
            "config": {
                "code": {
                    "type": "string",
                    "description": "Python script to execute (fallback if 'code' is not present in inputs)."
                },
                "sandbox_mode": {
                    "type": "string",
                    "description": "'none' (default) or 'restricted'. In 'restricted' mode the code is validated via AST: only whitelisted imports are allowed (math, json, re, datetime, collections, itertools, functools, string, decimal, statistics) and banned builtins (open, exec, eval, compile, __import__) are blocked. A timeout is also enforced."
                },
                "sandbox_timeout_secs": {
                    "type": "number",
                    "description": "Max execution seconds in 'restricted' mode. Default 10. Ignored when sandbox_mode is 'none'."
                }
            },
            "inputs": {
                "code": {
                    "type": "string",
                    "description": "Reserved key. Python script to execute (overrides config.code). NOT injected as a Python variable."
                },
                "sandbox_mode": {
                    "type": "string",
                    "description": "Reserved key. Overrides config.sandbox_mode for this execution. NOT injected as a Python variable."
                },
                "sandbox_timeout_secs": {
                    "type": "number",
                    "description": "Reserved key. Overrides config.sandbox_timeout_secs for this execution. NOT injected as a Python variable."
                },
                "<any_key>": {
                    "type": "any",
                    "description": "Any input key OTHER than the reserved keys ('code', 'sandbox_mode', 'sandbox_timeout_secs') is injected into the script as a global Python variable. JSON objects/arrays are converted to Python dicts/lists."
                }
            },
            "outputs": {
                "<raw>": "The node emits the RAW value of the Python 'output' variable (number, string, bool, list, dict, or null). NOT wrapped in { 'output': ... }. default_output is None so implicit edges pass the value through unchanged."
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[tokio::test]
    async fn test_python_math_logic() {
        pyo3::Python::initialize();
        let node = PythonNode;
        let mut inputs = HashMap::new();
        inputs.insert("x".to_string(), json!(10));
        inputs.insert("y".to_string(), json!(5));
        let config = json!({ "code": "output = x * y + 2" });
        let mut state = json!({});
        let result = node
            .execute(&inputs, &config, &mut state, None)
            .await
            .unwrap();
        assert_eq!(result, 52);
    }

    #[tokio::test]
    async fn test_python_imports() {
        pyo3::Python::initialize();
        let node = PythonNode;
        let inputs = HashMap::new();
        let config = json!({ "code": "import math\noutput = math.sqrt(16)" });
        let mut state = json!({});
        let result = node
            .execute(&inputs, &config, &mut state, None)
            .await
            .unwrap();
        assert_eq!(result, 4.0);
    }

    #[tokio::test]
    async fn test_sandbox_allows_clean_code() {
        pyo3::Python::initialize();
        let node = PythonNode;
        let inputs = HashMap::new();
        let config = json!({ "code": "output = 2 + 2", "sandbox_mode": "restricted" });
        let mut state = json!({});
        let result = node
            .execute(&inputs, &config, &mut state, None)
            .await
            .unwrap();
        assert_eq!(result, 4);
    }

    #[tokio::test]
    async fn test_sandbox_allows_whitelisted_import() {
        pyo3::Python::initialize();
        let node = PythonNode;
        let inputs = HashMap::new();
        let config = json!({
            "code": "import math\noutput = math.sqrt(16)",
            "sandbox_mode": "restricted"
        });
        let mut state = json!({});
        let result = node
            .execute(&inputs, &config, &mut state, None)
            .await
            .unwrap();
        assert_eq!(result, 4.0);
    }

    #[tokio::test]
    async fn test_sandbox_blocks_os_import() {
        pyo3::Python::initialize();
        let node = PythonNode;
        let inputs = HashMap::new();
        let config = json!({
            "code": "import os\noutput = os.getcwd()",
            "sandbox_mode": "restricted"
        });
        let mut state = json!({});
        let err = node
            .execute(&inputs, &config, &mut state, None)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("SandboxViolation"), "got: {err}");
        assert!(err.to_string().contains("'os'"), "got: {err}");
        assert!(err.to_string().contains("Allowed imports:"), "got: {err}");
    }

    #[tokio::test]
    async fn test_sandbox_blocks_open_builtin() {
        pyo3::Python::initialize();
        let node = PythonNode;
        let inputs = HashMap::new();
        let config = json!({
            "code": "f = open('/etc/passwd')\noutput = f.read()",
            "sandbox_mode": "restricted"
        });
        let mut state = json!({});
        let err = node
            .execute(&inputs, &config, &mut state, None)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("SandboxViolation"), "got: {err}");
        assert!(err.to_string().contains("'open'"), "got: {err}");
    }

    #[tokio::test]
    async fn test_sandbox_blocks_eval() {
        pyo3::Python::initialize();
        let node = PythonNode;
        let inputs = HashMap::new();
        let config = json!({
            "code": "output = eval('1 + 1')",
            "sandbox_mode": "restricted"
        });
        let mut state = json!({});
        let err = node
            .execute(&inputs, &config, &mut state, None)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("SandboxViolation"), "got: {err}");
        assert!(err.to_string().contains("'eval'"), "got: {err}");
    }

    #[tokio::test]
    async fn test_sandbox_none_mode_allows_os() {
        pyo3::Python::initialize();
        let node = PythonNode;
        let inputs = HashMap::new();
        let config = json!({
            "code": "import os\noutput = type(os).__name__",
            "sandbox_mode": "none"
        });
        let mut state = json!({});
        let result = node
            .execute(&inputs, &config, &mut state, None)
            .await
            .unwrap();
        assert_eq!(result, "module");
    }

    #[tokio::test]
    async fn test_sandbox_default_mode_allows_os() {
        pyo3::Python::initialize();
        let node = PythonNode;
        let inputs = HashMap::new();
        let config = json!({
            "code": "import os\noutput = type(os).__name__"
        });
        let mut state = json!({});
        let result = node
            .execute(&inputs, &config, &mut state, None)
            .await
            .unwrap();
        assert_eq!(result, "module");
    }

    // NOTE: A direct end-to-end test of `sandbox_timeout_secs` would deadlock
    // the test harness. `tokio::time::timeout` correctly returns Err(Elapsed)
    // after N seconds, but the underlying spawn_blocking thread (running an
    // infinite Python loop holding the GIL) cannot be cancelled. Tokio's
    // runtime drop waits for blocking tasks to finish, so the test process
    // hangs even though the timeout fired. The timeout primitive itself is
    // upstream-tested by tokio. We verify wiring via the e2e graph in
    // tests/graphs/agents/python_sandbox_tool_test.json.

    #[test]
    fn restricted_mode_allows_pandas_import() {
        pyo3::Python::initialize();
        pyo3::Python::attach(|py| {
            let result = validate_sandbox(py, "import pandas as pd\noutput = 1");
            match result {
                Ok(None) => {} // OK — no violation
                Ok(Some(v)) => panic!("expected pandas to pass, got violation: {v}"),
                Err(e) => panic!("validator errored: {e}"),
            }
        });
    }

    #[test]
    fn restricted_mode_allows_numpy_import() {
        pyo3::Python::initialize();
        pyo3::Python::attach(|py| {
            let result = validate_sandbox(py, "import numpy as np\noutput = 1");
            match result {
                Ok(None) => {}
                other => panic!("expected numpy to pass: {other:?}"),
            }
        });
    }

    #[test]
    fn restricted_mode_allows_scipy_stats_import() {
        pyo3::Python::initialize();
        pyo3::Python::attach(|py| {
            let result = validate_sandbox(py, "from scipy import stats\noutput = 1");
            match result {
                Ok(None) => {}
                other => panic!("expected `from scipy import stats` to pass: {other:?}"),
            }
        });
    }

    #[test]
    fn restricted_mode_still_rejects_requests_import() {
        pyo3::Python::initialize();
        pyo3::Python::attach(|py| {
            let result = validate_sandbox(py, "import requests\noutput = 1");
            match result {
                Ok(Some(_)) => {} // OK — violation reported
                other => panic!("requests should be rejected: {other:?}"),
            }
        });
    }

    #[tokio::test]
    async fn test_sandbox_skips_reserved_keys_as_python_vars() {
        pyo3::Python::initialize();
        let node = PythonNode;
        let mut inputs = HashMap::new();
        inputs.insert("x".to_string(), json!(7));
        // sandbox_mode in inputs — should be read as config, NOT injected as Python var
        inputs.insert("sandbox_mode".to_string(), json!("restricted"));
        let config = json!({ "code": "output = x * 2" });
        let mut state = json!({});
        let result = node
            .execute(&inputs, &config, &mut state, None)
            .await
            .unwrap();
        assert_eq!(result, 14);
    }

    /// Signing primitives must be importable in `restricted` mode: APIs like
    /// NetSuite TBA or AWS SigV4 require an HMAC signature per request, which
    /// cannot be expressed as a static header.
    #[test]
    fn restricted_allows_signing_primitives() {
        pyo3::Python::initialize();
        let code = "import hmac, hashlib, base64, secrets\n\
                    sig = base64.b64encode(hmac.new(b'k', b'msg', hashlib.sha256).digest()).decode()\n\
                    output = {'sig': sig, 'nonce_len': len(secrets.token_hex(16))}";
        let res = execute_sandboxed_helper(code, "restricted", 30, &serde_json::Map::new())
            .expect("signing primitives must be allowed in restricted mode");
        let out = res.output.expect("output");
        assert_eq!(out["nonce_len"], 32);
        assert!(out["sig"].as_str().unwrap().len() > 20);
    }

    /// The sandbox must keep granting signing but never network egress: the
    /// signed request has to leave through `http_request`, not from Python.
    #[test]
    fn restricted_still_blocks_network_and_process_modules() {
        pyo3::Python::initialize();
        for module in [
            "urllib",
            "socket",
            "requests",
            "os",
            "subprocess",
            "sys",
            "shutil",
        ] {
            let code = format!("import {module}\noutput = 1");
            let err = execute_sandboxed_helper(&code, "restricted", 30, &serde_json::Map::new())
                .expect_err(&format!("'{module}' must stay banned in restricted mode"));
            assert!(
                err.contains("SandboxViolation"),
                "expected a sandbox violation for '{module}', got: {err}"
            );
        }
    }

    /// `urllib.parse` must not slip through: the validator matches on the root
    /// module, so allowing it would also unlock `urllib.request` (network).
    #[test]
    fn restricted_blocks_urllib_submodules() {
        pyo3::Python::initialize();
        let err = execute_sandboxed_helper(
            "from urllib.parse import quote\noutput = quote('a b')",
            "restricted",
            30,
            &serde_json::Map::new(),
        )
        .expect_err("urllib.parse must stay banned (root module match)");
        assert!(err.contains("SandboxViolation"), "got: {err}");
    }
}
