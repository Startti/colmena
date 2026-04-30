use crate::dag_engine::domain::node::{ExecutableNode, NodeInputs};
use async_trait::async_trait;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyString};
use pythonize::{depythonize_bound, pythonize};
use serde_json::{json, Value};
use std::error::Error as StdError;
use std::sync::Arc;

pub struct PythonNode;

const SANDBOX_VALIDATOR: &str = r#"
import ast as _ast

_ALLOWED_IMPORTS = {
    'math', 'json', 're', 'datetime', 'collections',
    'itertools', 'functools', 'string', 'decimal', 'statistics'
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
    let locals = PyDict::new_bound(py);
    locals
        .set_item("_code_to_validate", PyString::new_bound(py, code))
        .map_err(|e| format!("Validator setup error: {e}"))?;
    py.run_bound(SANDBOX_VALIDATOR, Some(&locals), Some(&locals))
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

        // 3. Prepare inputs for the closure — skip sandbox config keys
        let sandbox_keys = ["sandbox_mode", "sandbox_timeout_secs", "code"];
        let inputs_clone: NodeInputs = inputs
            .iter()
            .filter(|(k, _)| !sandbox_keys.contains(&k.as_str()))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        let code = code.to_string();
        let sandbox_mode_clone = sandbox_mode.clone();

        // 4. Schedule blocking execution (CPython is not async-safe)
        let blocking_task = tokio::task::spawn_blocking(move || -> Result<Value, String> {
            Python::with_gil(|py| {
                // 4a. AST validation in restricted mode
                if sandbox_mode_clone == "restricted" {
                    if let Some(violation) = validate_sandbox(py, &code)? {
                        return Err(violation);
                    }
                }

                // 4b. Inject inputs as Python variables
                let locals = PyDict::new_bound(py);
                for (key, value) in &inputs_clone {
                    let py_val = pythonize(py, value).map_err(|e| {
                        format!("Failed to convert input '{}' to Python: {}", key, e)
                    })?;
                    locals
                        .set_item(key, py_val)
                        .map_err(|e| format!("Failed to set input '{}': {}", key, e))?;
                }

                // 4c. Execute user code
                py.run_bound(&code, Some(&locals), Some(&locals))
                    .map_err(|e| format!("Python execution error: {}", e))?;

                // 4d. Extract result from 'output' variable
                match locals.get_item("output") {
                    Ok(Some(output_obj)) => {
                        let json_output: Value =
                            depythonize_bound(output_obj).map_err(|e| {
                                format!("Failed to convert Python 'output' to JSON: {}", e)
                            })?;
                        Ok(json_output)
                    }
                    _ => Ok(Value::Null),
                }
            })
        });

        // 5. Apply timeout in restricted mode; plain await otherwise
        let output_json = if sandbox_mode == "restricted" {
            tokio::time::timeout(
                std::time::Duration::from_secs(timeout_secs),
                blocking_task,
            )
            .await
            .map_err(|_| {
                format!("SandboxTimeout: execution exceeded {} seconds", timeout_secs)
            })?
            .map_err(|e| format!("Task join error: {e}"))?
            .map_err(|e| -> Box<dyn StdError + Send + Sync> { e.into() })?
        } else {
            blocking_task.await??
        };

        Ok(output_json)
    }

    fn default_output(&self) -> Option<&str> {
        Some("output")
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
                "output": "The value of the Python 'output' variable after execution. Returns null if 'output' is not defined."
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
        pyo3::prepare_freethreaded_python();
        let node = PythonNode;
        let mut inputs = HashMap::new();
        inputs.insert("x".to_string(), json!(10));
        inputs.insert("y".to_string(), json!(5));
        let config = json!({ "code": "output = x * y + 2" });
        let mut state = json!({});
        let result = node.execute(&inputs, &config, &mut state, None).await.unwrap();
        assert_eq!(result, 52);
    }

    #[tokio::test]
    async fn test_python_imports() {
        pyo3::prepare_freethreaded_python();
        let node = PythonNode;
        let inputs = HashMap::new();
        let config = json!({ "code": "import math\noutput = math.sqrt(16)" });
        let mut state = json!({});
        let result = node.execute(&inputs, &config, &mut state, None).await.unwrap();
        assert_eq!(result, 4.0);
    }

    #[tokio::test]
    async fn test_sandbox_allows_clean_code() {
        pyo3::prepare_freethreaded_python();
        let node = PythonNode;
        let inputs = HashMap::new();
        let config = json!({ "code": "output = 2 + 2", "sandbox_mode": "restricted" });
        let mut state = json!({});
        let result = node.execute(&inputs, &config, &mut state, None).await.unwrap();
        assert_eq!(result, 4);
    }

    #[tokio::test]
    async fn test_sandbox_allows_whitelisted_import() {
        pyo3::prepare_freethreaded_python();
        let node = PythonNode;
        let inputs = HashMap::new();
        let config = json!({
            "code": "import math\noutput = math.sqrt(16)",
            "sandbox_mode": "restricted"
        });
        let mut state = json!({});
        let result = node.execute(&inputs, &config, &mut state, None).await.unwrap();
        assert_eq!(result, 4.0);
    }

    #[tokio::test]
    async fn test_sandbox_blocks_os_import() {
        pyo3::prepare_freethreaded_python();
        let node = PythonNode;
        let inputs = HashMap::new();
        let config = json!({
            "code": "import os\noutput = os.getcwd()",
            "sandbox_mode": "restricted"
        });
        let mut state = json!({});
        let err = node.execute(&inputs, &config, &mut state, None).await.unwrap_err();
        assert!(err.to_string().contains("SandboxViolation"), "got: {err}");
        assert!(err.to_string().contains("'os'"), "got: {err}");
        assert!(err.to_string().contains("Allowed imports:"), "got: {err}");
    }

    #[tokio::test]
    async fn test_sandbox_blocks_open_builtin() {
        pyo3::prepare_freethreaded_python();
        let node = PythonNode;
        let inputs = HashMap::new();
        let config = json!({
            "code": "f = open('/etc/passwd')\noutput = f.read()",
            "sandbox_mode": "restricted"
        });
        let mut state = json!({});
        let err = node.execute(&inputs, &config, &mut state, None).await.unwrap_err();
        assert!(err.to_string().contains("SandboxViolation"), "got: {err}");
        assert!(err.to_string().contains("'open'"), "got: {err}");
    }

    #[tokio::test]
    async fn test_sandbox_blocks_eval() {
        pyo3::prepare_freethreaded_python();
        let node = PythonNode;
        let inputs = HashMap::new();
        let config = json!({
            "code": "output = eval('1 + 1')",
            "sandbox_mode": "restricted"
        });
        let mut state = json!({});
        let err = node.execute(&inputs, &config, &mut state, None).await.unwrap_err();
        assert!(err.to_string().contains("SandboxViolation"), "got: {err}");
        assert!(err.to_string().contains("'eval'"), "got: {err}");
    }

    #[tokio::test]
    async fn test_sandbox_none_mode_allows_os() {
        pyo3::prepare_freethreaded_python();
        let node = PythonNode;
        let inputs = HashMap::new();
        let config = json!({
            "code": "import os\noutput = type(os).__name__",
            "sandbox_mode": "none"
        });
        let mut state = json!({});
        let result = node.execute(&inputs, &config, &mut state, None).await.unwrap();
        assert_eq!(result, "module");
    }

    #[tokio::test]
    async fn test_sandbox_default_mode_allows_os() {
        pyo3::prepare_freethreaded_python();
        let node = PythonNode;
        let inputs = HashMap::new();
        let config = json!({
            "code": "import os\noutput = type(os).__name__"
        });
        let mut state = json!({});
        let result = node.execute(&inputs, &config, &mut state, None).await.unwrap();
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

    #[tokio::test]
    async fn test_sandbox_skips_reserved_keys_as_python_vars() {
        pyo3::prepare_freethreaded_python();
        let node = PythonNode;
        let mut inputs = HashMap::new();
        inputs.insert("x".to_string(), json!(7));
        // sandbox_mode in inputs — should be read as config, NOT injected as Python var
        inputs.insert("sandbox_mode".to_string(), json!("restricted"));
        let config = json!({ "code": "output = x * 2" });
        let mut state = json!({});
        let result = node.execute(&inputs, &config, &mut state, None).await.unwrap();
        assert_eq!(result, 14);
    }
}
