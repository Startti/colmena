# Python Script Sandbox Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `sandbox_mode: "restricted"` to the `python_script` node so LLM-generated code is validated via AST check (import whitelist + banned builtins) and executed with a configurable timeout.

**Architecture:** Modify `python_node.rs` to extract `sandbox_mode` and `sandbox_timeout_secs` from config/inputs. When `restricted`, run a Python AST validator inside `spawn_blocking` (using Python's stdlib `ast` module via PyO3) before executing, then wrap the task with `tokio::time::timeout`. Error messages are returned as tool results so the LLM can retry with corrected code.

**Tech Stack:** Rust, PyO3, tokio (spawn_blocking + time::timeout), Python `ast` stdlib module (no extra deps).

---

## File Map

| File | Action | Responsibility |
|------|--------|----------------|
| `src/libs/colmena/src/dag_engine/infrastructure/nodes/python_node.rs` | Modify | Add sandbox_mode extraction, validate_sandbox fn, timeout wrapper |
| `docs/node_configurations.json` | Modify | Add `sandbox_mode` and `sandbox_timeout_secs` to python_script config_fields |
| `docs/node_as_tools_reference.json` | Modify | Add LLM-generated code example to python_script section |
| `tests/graphs/agents/python_sandbox_tool_test.json` | Create | End-to-end test graph |

---

## Task 1: AST Validator — tests first

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/python_node.rs`

- [ ] **Step 1.1: Write failing tests for validate_sandbox**

Add at the bottom of the `#[cfg(test)]` module in `python_node.rs`:

```rust
#[tokio::test]
async fn test_sandbox_allows_clean_code() {
    pyo3::prepare_freethreaded_python();
    let node = PythonNode;
    let inputs = std::collections::HashMap::new();
    let config = serde_json::json!({
        "code": "output = 2 + 2",
        "sandbox_mode": "restricted"
    });
    let mut state = serde_json::json!({});
    let result = node.execute(&inputs, &config, &mut state, None).await.unwrap();
    assert_eq!(result, 4);
}

#[tokio::test]
async fn test_sandbox_allows_whitelisted_import() {
    pyo3::prepare_freethreaded_python();
    let node = PythonNode;
    let inputs = std::collections::HashMap::new();
    let config = serde_json::json!({
        "code": "import math\noutput = math.sqrt(16)",
        "sandbox_mode": "restricted"
    });
    let mut state = serde_json::json!({});
    let result = node.execute(&inputs, &config, &mut state, None).await.unwrap();
    assert_eq!(result, 4.0);
}

#[tokio::test]
async fn test_sandbox_blocks_os_import() {
    pyo3::prepare_freethreaded_python();
    let node = PythonNode;
    let inputs = std::collections::HashMap::new();
    let config = serde_json::json!({
        "code": "import os\noutput = os.getcwd()",
        "sandbox_mode": "restricted"
    });
    let mut state = serde_json::json!({});
    let err = node.execute(&inputs, &config, &mut state, None).await.unwrap_err();
    assert!(err.to_string().contains("SandboxViolation"));
    assert!(err.to_string().contains("'os'"));
    assert!(err.to_string().contains("Allowed imports:"));
}

#[tokio::test]
async fn test_sandbox_blocks_open_builtin() {
    pyo3::prepare_freethreaded_python();
    let node = PythonNode;
    let inputs = std::collections::HashMap::new();
    let config = serde_json::json!({
        "code": "f = open('/etc/passwd')\noutput = f.read()",
        "sandbox_mode": "restricted"
    });
    let mut state = serde_json::json!({});
    let err = node.execute(&inputs, &config, &mut state, None).await.unwrap_err();
    assert!(err.to_string().contains("SandboxViolation"));
    assert!(err.to_string().contains("'open'"));
}

#[tokio::test]
async fn test_sandbox_blocks_eval() {
    pyo3::prepare_freethreaded_python();
    let node = PythonNode;
    let inputs = std::collections::HashMap::new();
    let config = serde_json::json!({
        "code": "output = eval('1 + 1')",
        "sandbox_mode": "restricted"
    });
    let mut state = serde_json::json!({});
    let err = node.execute(&inputs, &config, &mut state, None).await.unwrap_err();
    assert!(err.to_string().contains("SandboxViolation"));
    assert!(err.to_string().contains("'eval'"));
}

#[tokio::test]
async fn test_sandbox_none_mode_allows_os() {
    pyo3::prepare_freethreaded_python();
    let node = PythonNode;
    let inputs = std::collections::HashMap::new();
    let config = serde_json::json!({
        "code": "import os\noutput = type(os).__name__",
        "sandbox_mode": "none"
    });
    let mut state = serde_json::json!({});
    let result = node.execute(&inputs, &config, &mut state, None).await.unwrap();
    assert_eq!(result, "module");
}

#[tokio::test]
async fn test_sandbox_default_mode_allows_os() {
    // sandbox_mode defaults to "none" — no breaking change
    pyo3::prepare_freethreaded_python();
    let node = PythonNode;
    let inputs = std::collections::HashMap::new();
    let config = serde_json::json!({
        "code": "import os\noutput = type(os).__name__"
        // no sandbox_mode field
    });
    let mut state = serde_json::json!({});
    let result = node.execute(&inputs, &config, &mut state, None).await.unwrap();
    assert_eq!(result, "module");
}

#[tokio::test]
async fn test_sandbox_timeout() {
    pyo3::prepare_freethreaded_python();
    let node = PythonNode;
    let inputs = std::collections::HashMap::new();
    let config = serde_json::json!({
        "code": "while True: pass",
        "sandbox_mode": "restricted",
        "sandbox_timeout_secs": 2
    });
    let mut state = serde_json::json!({});
    let err = node.execute(&inputs, &config, &mut state, None).await.unwrap_err();
    assert!(err.to_string().contains("SandboxTimeout"));
    assert!(err.to_string().contains("2 seconds"));
}
```

- [ ] **Step 1.2: Run tests to verify they all fail (function not yet implemented)**

```bash
cargo test --lib python_node -p colmena_dag_engine -- --nocapture 2>&1 | tail -20
```

Expected: compilation error or test failures — `sandbox_mode` not handled yet.

- [ ] **Step 1.3: Add `validate_sandbox` function and sandbox logic to `python_node.rs`**

Replace the full content of `python_node.rs` with:

```rust
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
                    if let Some(violation) =
                        validate_sandbox(py, &code).map_err(|e| e)?
                    {
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
            "description": "Executes Python code. Code can be provided via input 'code' or config 'code'. Inputs are injected as variables. Assign result to variable 'output'.",
            "config": {
                "code": {
                    "type": "string",
                    "description": "Python script to execute (fallback if not in inputs)"
                },
                "sandbox_mode": {
                    "type": "string",
                    "description": "'none' (default) or 'restricted'. Restricted mode validates imports and builtins via AST check and enforces a timeout."
                },
                "sandbox_timeout_secs": {
                    "type": "number",
                    "description": "Max execution seconds in restricted mode. Default 10."
                }
            },
            "inputs": {
                "code": {
                    "type": "string",
                    "description": "Python script to execute (optional, overrides config)"
                },
                "description": "Key-value pairs injected as global variables (sandbox_mode and sandbox_timeout_secs are reserved)"
            },
            "outputs": {
                "output": "The value of the 'output' variable from the script"
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

    #[tokio::test]
    async fn test_sandbox_timeout() {
        pyo3::prepare_freethreaded_python();
        let node = PythonNode;
        let inputs = HashMap::new();
        let config = json!({
            "code": "while True: pass",
            "sandbox_mode": "restricted",
            "sandbox_timeout_secs": 2
        });
        let mut state = json!({});
        let err = node.execute(&inputs, &config, &mut state, None).await.unwrap_err();
        assert!(err.to_string().contains("SandboxTimeout"), "got: {err}");
        assert!(err.to_string().contains("2 seconds"), "got: {err}");
    }

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
```

- [ ] **Step 1.4: Run tests**

```bash
cargo test --lib python_node -p colmena_dag_engine -- --nocapture 2>&1 | tail -30
```

Expected: all tests pass except `test_sandbox_timeout` (may be slow — see note below).

> **Note on timeout test:** `test_sandbox_timeout` runs an infinite loop and waits 2 seconds. It will pass but takes 2s. If it hangs beyond 5s something is wrong with the timeout wiring.

- [ ] **Step 1.5: Run clippy and fix warnings**

```bash
cargo clippy -p colmena_dag_engine --lib -- -D warnings 2>&1 | grep -E "error|warning" | head -20
```

- [ ] **Step 1.6: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/python_node.rs
git commit -m "feat(python-node): add sandbox_mode with AST validator and timeout"
```

---

## Task 2: Update `node_configurations.json`

**Files:**
- Modify: `docs/node_configurations.json` lines ~938-946 (python_script config_fields)

- [ ] **Step 2.1: Add `sandbox_mode` and `sandbox_timeout_secs` to config_fields**

Find the `python_script` → `config_fields` section (currently only has `"code"`). Replace it with:

```json
"config_fields": {
  "code": {
    "type": "string",
    "required": false,
    "default": null,
    "description": "The Python code to execute. Can be a single expression (e.g., 'output = x * y + 2') or a multi-line script with imports, functions, loops, etc. The code runs in an isolated namespace where all input values are available as variables. You MUST assign the result to a variable called 'output' — this is the convention the node uses to extract the return value. If 'output' is not defined after execution, the node returns null. The code input port takes priority over this config field, allowing LLMs or upstream nodes to dynamically provide code.",
    "example": "import math\noutput = math.sqrt(x ** 2 + y ** 2)"
  },
  "sandbox_mode": {
    "type": "string",
    "required": false,
    "default": "none",
    "description": "Execution sandbox level. 'none' (default) runs code with full Python access — safe for trusted code. 'restricted' validates the code via AST before execution: only whitelisted imports are allowed (math, json, re, datetime, collections, itertools, functools, string, decimal, statistics) and banned builtins (open, exec, eval, compile, __import__) are blocked. Use 'restricted' whenever the code is generated by an LLM or provided by an untrusted source.",
    "valid_values": ["none", "restricted"],
    "example": "restricted"
  },
  "sandbox_timeout_secs": {
    "type": "number",
    "required": false,
    "default": 10,
    "description": "Maximum execution time in seconds. Only applies when sandbox_mode is 'restricted'. If the script exceeds this limit, the node returns a SandboxTimeout error. Minimum recommended value is 2. Increase for scripts that process large datasets.",
    "example": 10
  }
},
```

- [ ] **Step 2.2: Verify JSON is valid**

```bash
python3 -c "import json; json.load(open('docs/node_configurations.json')); print('OK')"
```

Expected: `OK`

- [ ] **Step 2.3: Commit**

```bash
git add docs/node_configurations.json
git commit -m "docs: add sandbox_mode and sandbox_timeout_secs to python_script config schema"
```

---

## Task 3: Update `node_as_tools_reference.json` — LLM-generated code example

**Files:**
- Modify: `docs/node_as_tools_reference.json` — `node_types_as_tools.python_script` section

- [ ] **Step 3.1: Find the python_script section**

```bash
grep -n '"python_script"' docs/node_as_tools_reference.json
```

- [ ] **Step 3.2: Add LLM-generated code example**

Find the `python_script` entry under `node_types_as_tools`. The current `special_behaviors` says *"The 'script' field should be fixed — the LLM never writes code."* Replace the full `python_script` section with:

```json
"python_script": {
  "summary": "Run a Python snippet as a tool. Two patterns: (1) Fixed code + LLM provides input variables. (2) LLM generates the code itself — use sandbox_mode: 'restricted' to validate it safely.",
  "special_behaviors": [
    "All inputs except 'code', 'sandbox_mode', and 'sandbox_timeout_secs' are injected as Python variables.",
    "The script must assign its result to 'output'. Prefer dict output: output = {'count': 5, 'ids': [1,2,3]}.",
    "When sandbox_mode is 'restricted': imports are validated against a whitelist; open/exec/eval/compile are blocked; execution is capped at sandbox_timeout_secs.",
    "On sandbox violation the node returns an error string the LLM can read and retry with corrected code."
  ],
  "examples": {
    "fixed_code_llm_provides_variables": {
      "description": "LLM provides a string; the script (fixed) transforms it. LLM never sees the code.",
      "tool_configurations_entry": {
        "word_count": {
          "name": "word_count",
          "description": "Contar las palabras en un texto.",
          "node_type": "python_script",
          "node_schema": {
            "sandbox_mode": { "fixed": "restricted" },
            "code":         { "fixed": "output = {'count': len(text.split())}" },
            "text":         { "type": "string", "required": true, "description": "Texto a analizar." }
          }
        }
      }
    },
    "llm_generates_code": {
      "description": "LLM writes Python code to process upstream data (e.g. HTTP response). sandbox_mode is fixed to 'restricted' so the LLM cannot change it. Variables available in the script come from the 'context' field.",
      "tool_configurations_entry": {
        "run_python": {
          "name": "run_python",
          "description": "Ejecuta código Python para procesar datos. Variable disponible: 'rows' (lista de objetos del HTTP response). Asigna el resultado a 'output'. Prefiere formato diccionario: output = {'count': 5, 'ids': [1,2,3]}.",
          "node_type": "python_script",
          "node_schema": {
            "sandbox_mode":         { "fixed": "restricted" },
            "sandbox_timeout_secs": { "fixed": 10 },
            "code": {
              "type": "string",
              "required": true,
              "description": "Código Python a ejecutar. Imports permitidos: math, json, re, datetime, collections, itertools. Asigna el resultado a 'output' como diccionario. Ejemplo: output = {'active': len([r for r in rows if r['stock'] > 0])}"
            }
          },
          "context": {
            "rows": "${http_node.body.products}"
          }
        }
      },
      "note": "The 'context' field maps upstream node outputs to Python variables. The key becomes the variable name in the script. The LLM must describe its code in terms of those variable names."
    }
  }
}
```

- [ ] **Step 3.3: Verify JSON is valid**

```bash
python3 -c "import json; json.load(open('docs/node_as_tools_reference.json')); print('OK')"
```

Expected: `OK`

- [ ] **Step 3.4: Commit**

```bash
git add docs/node_as_tools_reference.json
git commit -m "docs: add LLM-generated code pattern to python_script tool reference"
```

---

## Task 4: Create end-to-end test graph

**Files:**
- Create: `tests/graphs/agents/python_sandbox_tool_test.json`

- [ ] **Step 4.1: Create the test graph**

```json
{
  "nodes": {
    "start": {
      "type": "mock_input",
      "config": {
        "prompt": "Usando la API de productos de dummyjson, dime cuántos productos tienen un rating mayor a 4.5"
      }
    },
    "fetch_products": {
      "type": "http_request",
      "config": {
        "base_url": "https://dummyjson.com",
        "endpoint": "/products?limit=100",
        "method": "GET"
      }
    },
    "agent": {
      "type": "llm_call",
      "config": {
        "provider": "openai",
        "model": "gpt-4o-mini",
        "api_key": "${OPENAI_API_KEY}",
        "system_message": "Eres un asistente de análisis de datos. Cuando necesites procesar listas de datos usa la tool run_python. Siempre retorna resultados como diccionario en 'output'.",
        "tool_configurations": {
          "run_python": {
            "name": "run_python",
            "description": "Ejecuta código Python para procesar datos. Variable disponible: 'products' (lista de productos de dummyjson, cada uno con campos: id, title, price, rating, stock, category). Asigna el resultado a 'output' como diccionario.",
            "node_type": "python_script",
            "node_schema": {
              "sandbox_mode":         { "fixed": "restricted" },
              "sandbox_timeout_secs": { "fixed": 10 },
              "code": {
                "type": "string",
                "required": true,
                "description": "Código Python. Imports permitidos: math, json, re, datetime, collections, itertools. Asigna resultado a 'output' como diccionario. Ejemplo: output = {'count': len([p for p in products if p['rating'] > 4.0])}"
              }
            },
            "context": {
              "products": "${fetch_products.body.products}"
            }
          }
        }
      }
    },
    "output": {
      "type": "output"
    }
  },
  "edges": [
    { "from": "start",          "to": "agent" },
    { "from": "fetch_products", "to": "agent" },
    { "from": "agent",          "to": "output" }
  ]
}
```

- [ ] **Step 4.2: Run the graph and verify the LLM uses the tool**

```bash
set -a; source .env; set +a
cargo run --bin dag_engine -- run tests/graphs/agents/python_sandbox_tool_test.json 2>&1 | tail -30
```

Expected output contains:
- `[PythonNode] Executing code:` — confirms the tool was called
- A number as the final answer (how many products have rating > 4.5)
- No `SandboxViolation` errors

- [ ] **Step 4.3: Commit**

```bash
git add tests/graphs/agents/python_sandbox_tool_test.json
git commit -m "test(graph): add python sandbox tool end-to-end test graph"
```

---

## Self-Review

**Spec coverage check:**

| Spec requirement | Task |
|-----------------|------|
| `sandbox_mode: "none" \| "restricted"` config field | Task 1 |
| `sandbox_timeout_secs` config field, default 10 | Task 1 |
| AST validator via Python `ast` stdlib | Task 1 — `SANDBOX_VALIDATOR` constant |
| Import whitelist (math, json, re, datetime, collections, itertools, functools, string, decimal, statistics) | Task 1 |
| Banned builtins (open, exec, eval, compile, __import__) | Task 1 |
| Timeout via tokio::time::timeout wrapping spawn_blocking | Task 1 |
| Error: `SandboxViolation: import 'X' is not allowed. Allowed imports: ...` | Task 1 |
| Error: `SandboxViolation: 'open' is not allowed in sandbox mode` | Task 1 |
| Error: `SandboxTimeout: execution exceeded N seconds` | Task 1 |
| Backward compatible — default `"none"` keeps existing behavior | Task 1 — `test_sandbox_default_mode_allows_os` |
| `sandbox_mode` in `node_schema+fixed` (not `fixed_config`) | Task 3 examples |
| `node_configurations.json` updated | Task 2 |
| `node_as_tools_reference.json` updated with both patterns | Task 3 |
| End-to-end test graph | Task 4 |
| Reserved keys not injected as Python vars | Task 1 — inputs_clone filtering + `test_sandbox_skips_reserved_keys_as_python_vars` |
