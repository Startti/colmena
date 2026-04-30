# 26. Python Script Node (`python_script`)

## Overview

The `python_script` node executes arbitrary Python code inside the DAG. It is the most flexible escape hatch in Colmena: any data transformation, calculation, filtering, parsing, or formatting that does not have a dedicated node can be expressed as a short Python snippet.

It serves two main use cases:

1. **Static helper inside the DAG** — the code is fixed at design time and operates on values arriving from upstream edges (e.g. compute a metric, reshape a JSON payload).
2. **LLM tool** — exposed via `tool_configurations` so an `llm_call` node can run computations on data the LLM has already seen. When the LLM is allowed to write the code itself, the optional `restricted` sandbox validates the script via an AST check and enforces a timeout.

**Source:** `src/libs/colmena/src/dag_engine/infrastructure/nodes/python_node.rs`
**Registered as:** `"python_script"` in the node registry
**Feature flag:** requires the `python` cargo feature (PyO3 bindings)

---

## Architecture

The node is intentionally small: there is no separate use case or domain port. Execution lives entirely inside `python_node.rs`.

```
┌────────────────────────────────────────────────────────────┐
│                    PythonNode (node)                        │
│           infrastructure/nodes/python_node.rs               │
│                                                             │
│   1. Resolve `code`     (input port > config field)         │
│   2. Strip ```python ... ``` markdown wrappers              │
│   3. Resolve `sandbox_mode` and `sandbox_timeout_secs`      │
│   4. Filter reserved keys from the input map                │
│   5. spawn_blocking → Python::with_gil:                     │
│        • If restricted: AST validation                      │
│        • Inject inputs as global Python variables           │
│        • py.run_bound(code)                                 │
│        • Extract `output` variable → JSON                   │
│   6. If restricted: wrap in tokio::time::timeout            │
└────────────────────────────────────────────────────────────┘
```

CPython's GIL is not async-safe, so the entire Python execution runs inside `tokio::task::spawn_blocking`. This isolates the GIL on a dedicated blocking-pool thread and keeps the async runtime responsive.

---

## Configuration Reference

| Field | Type | Required | Default | Description |
|---|---|---|---|---|
| `code` | string | One of `code` (config) or `code` input port must be present | — | The Python source to execute. Single expression or multi-line script. |
| `sandbox_mode` | string | No | `"none"` | `"none"` runs with full Python access. `"restricted"` enables AST validation + timeout. |
| `sandbox_timeout_secs` | number | No | `10` | Max execution seconds. Only enforced when `sandbox_mode` is `"restricted"`. |

### `code`

Plain Python source. The script must assign its final result to a variable named `output`. Any JSON-serializable value is supported (numbers, strings, booleans, lists, dicts, nested combinations, `None`). If `output` is not defined after execution, the node returns `null`.

When the `code` input port is present (e.g. an LLM emits the code into the edge), it overrides `config.code`. Markdown wrappers like ` ```python ... ``` ` are stripped automatically, so it is safe to feed raw LLM output directly into the node.

### `sandbox_mode`

| Value | Behavior |
|---|---|
| `"none"` | Full Python access. No AST check, no timeout. Use for code authored by you. |
| `"restricted"` | AST validation runs before execution. Only whitelisted imports are allowed; banned builtins are blocked. Execution is wrapped in `tokio::time::timeout`. |

**Allowed imports (restricted mode):** `math`, `json`, `re`, `datetime`, `collections`, `itertools`, `functools`, `string`, `decimal`, `statistics`.

**Banned builtins (restricted mode):** `open`, `exec`, `eval`, `compile`, `__import__`.

On a sandbox violation the node returns a `SandboxViolation: ...` error string the LLM can read and retry from. Syntax errors are returned as `SyntaxError: ...`.

### `sandbox_timeout_secs`

Wall-clock seconds budget for the Python script when `sandbox_mode` is `"restricted"`. On timeout the caller receives `SandboxTimeout: execution exceeded N seconds`.

> **Known limitation.** A tight CPU loop holds the GIL and cannot be cancelled by `tokio::time::timeout`. The error is still surfaced to the caller, but the underlying blocking-pool thread remains busy until the process restarts. In long-running `serve` mode this can starve the blocking pool over time. For LLM-generated code this is acceptable in practice — tight loops are rare and recoverable. Avoid using the python_script node as a true multi-tenant sandbox for fully untrusted code.

---

## Input Ports

| Port | Reserved? | Description |
|---|---|---|
| `code` | **Yes** | Overrides `config.code`. NOT injected as a variable. |
| `sandbox_mode` | **Yes** | Overrides `config.sandbox_mode`. NOT injected as a variable. |
| `sandbox_timeout_secs` | **Yes** | Overrides `config.sandbox_timeout_secs`. NOT injected as a variable. |
| `<any_other_key>` | No | Injected into the script as a global Python variable with the same name. |

**Reserved-key rule.** The keys `code`, `sandbox_mode`, and `sandbox_timeout_secs` are consumed as configuration. They are filtered out of the input map before injection so they never appear as Python variables. Every other key becomes a global variable accessible by name. JSON objects and arrays become Python `dict` and `list` respectively (via the `pythonize` crate).

```json
// Edges
{ "from": "user", "to": "py.name" },
{ "from": "items", "to": "py.rows" }
```

```python
# Inside the script
greeting = f"Hello {name}, you have {len(rows)} items"
output = {"greeting": greeting}
```

---

## Output Port

| Port | Type | Description |
|---|---|---|
| `output` | any (JSON-serializable) | The value of the Python `output` variable after execution. `null` if `output` was never assigned. |

The default output port is `output`, so a downstream edge can be written implicitly: `{ "from": "py", "to": "next" }`.

---

## Markdown Stripping

LLMs often wrap Python in code fences. The node detects this and strips the wrapper before execution:

```
```python
output = sum(rows)
```
```
becomes:
```
output = sum(rows)
```

The stripping is conservative: only leading/trailing triple-backtick fences are removed, the rest of the script is left untouched.

---

## Threading & GIL Safety

- The Python interpreter is initialized once via `pyo3::prepare_freethreaded_python()` (called eagerly from the engine boot path).
- Every execution runs inside `tokio::task::spawn_blocking` so it cannot block the async runtime.
- Inside the blocking task, `Python::with_gil` acquires the GIL, runs the validator (if restricted), executes the user code, and releases the GIL on return.
- `tokio::time::timeout` only wraps the `JoinHandle` — it cannot interrupt a Python frame already holding the GIL. See the limitation note above.

---

## Use as a Plain DAG Node

Minimal example: receive two numbers from upstream and return a calculation.

```json
{
  "nodes": {
    "start":  { "type": "mock_input", "config": { "x": 10, "y": 5 } },
    "calc":   { "type": "python_script", "config": { "code": "output = x * y + 2" } },
    "log":    { "type": "log" }
  },
  "edges": [
    { "from": "start", "to": "calc" },
    { "from": "calc",  "to": "log"  }
  ]
}
```

A more realistic example: reshape an HTTP response.

```json
{
  "nodes": {
    "fetch": {
      "type": "http_request",
      "config": {
        "base_url": "https://dummyjson.com",
        "endpoint": "/products?limit=5",
        "method": "GET"
      }
    },
    "summarize": {
      "type": "python_script",
      "config": {
        "code": "items = body['products']\noutput = {\n  'count': len(items),\n  'avg_price': sum(p['price'] for p in items) / max(len(items), 1)\n}"
      }
    }
  },
  "edges": [
    { "from": "fetch", "to": "summarize" }
  ]
}
```

The HTTP node emits `{ status, body }`; both keys flow into the script, so `body` is available as a Python `dict`.

---

## Use as an LLM Tool

There are two canonical patterns. In both cases place behavioral fields (`sandbox_mode`, `sandbox_timeout_secs`) inside `node_schema` with `fixed`, never inside `fixed_config` — this is the project-wide rule documented in `CLAUDE.md`.

### Pattern A — Fixed code, LLM provides input variables

The LLM never sees the script. It only fills in semantic arguments (`text`, `query`, etc.) which become Python variables.

```json
"tool_configurations": {
  "word_count": {
    "name": "word_count",
    "description": "Count words in a text.",
    "node_type": "python_script",
    "node_schema": {
      "sandbox_mode": { "type": "string", "fixed": "restricted" },
      "code":         { "type": "string", "fixed": "output = {'count': len(text.split())}" },
      "text":         { "type": "string", "required": true, "description": "Text to analyze." }
    }
  }
}
```

### Pattern B — LLM writes the code AND passes the data

The LLM authors the script itself and also passes the raw data as a tool argument (because in the current architecture upstream node outputs do not auto-flow into a tool call — the LLM has already seen the data in a previous turn and re-supplies it).

```json
"tool_configurations": {
  "run_python": {
    "name": "run_python",
    "description": "Run sandboxed Python over a list. Pass 'rows' (the list) and 'code' (the script). Assign result to 'output' as a dict.",
    "node_type": "python_script",
    "node_schema": {
      "sandbox_mode":         { "type": "string", "fixed": "restricted" },
      "sandbox_timeout_secs": { "type": "number", "fixed": 10 },
      "code": {
        "type": "string",
        "required": true,
        "description": "Python code. Allowed imports: math, json, re, datetime, collections, itertools, functools, string, decimal, statistics. Forbidden: os, sys, open, exec, eval. Assign result to 'output' as a dict."
      },
      "rows": {
        "type": "array",
        "items": { "type": "object" },
        "required": true,
        "description": "Objects to process. Pass exactly the list you received from a previous tool call."
      }
    }
  }
}
```

The system message of the agent should instruct it to always call the tool for any computation (instead of doing math mentally) and to pass the raw list back through the `rows` argument. See `tests/graphs/agents/python_sandbox_tool_test.json` for a working end-to-end example with `gpt-4o-mini` and `tests/graphs/agents/python_sandbox_tool_thinking_test.json` for the `o4-mini` variant with `thinking_budget`.

> **Trade-off.** The LLM has to re-emit the data into its `rows` argument, costing tokens. In return you get a minimal, dependency-free sandbox feature without changes to the tool execution architecture. Token-efficient tool-to-tool piping (where the LLM never re-sees the raw data) is tracked as a future enhancement.

---

## Common Patterns

### Reshape an LLM result into a typed object

```json
{
  "nodes": {
    "extract": { "type": "llm_call", "config": { ... } },
    "shape": {
      "type": "python_script",
      "config": {
        "code": "import json\nparsed = json.loads(result)\noutput = {\n  'name': parsed.get('name'),\n  'age': int(parsed.get('age', 0))\n}"
      }
    }
  },
  "edges": [
    { "from": "extract.result", "to": "shape.result" }
  ]
}
```

### LLM emits raw code, downstream Python executes it

```json
{
  "nodes": {
    "llm_gen": { "type": "llm_call", "config": { ... } },
    "python_exec": {
      "type": "python_script",
      "config": { "code": "output = 'fallback if LLM produces nothing'" }
    }
  },
  "edges": [
    { "from": "llm_gen.result", "to": "python_exec.code" }
  ]
}
```

The `python_exec.code` input port wins over the config fallback. Markdown fences from the LLM are stripped automatically.

### Conditional output

The script can return `None` (Python) → `null` (JSON) and downstream nodes can branch on that:

```python
output = item if item.get('active') else None
```

---

## Reserved Keys & Pitfalls

- Sending `code`, `sandbox_mode`, or `sandbox_timeout_secs` through an edge always reconfigures the node — they are never seen as variables. If you genuinely want a variable named `code` in the script, rename the edge target (e.g. `code_template`).
- The `output` variable convention is mandatory. Returning a value via `return` or printing to stdout has no effect — the node looks for the literal name `output` in the locals dict.
- Inputs are injected as **globals** of the script, not as function arguments. Subsequent assignments in the script can overwrite them.
- The script runs in a fresh `dict` per execution. State does not persist between runs of the same node. For persistence, use `task_memory_writer` or the LLM memory layer.

---

## Limitations & Caveats

| Concern | Status |
|---|---|
| GIL-holding tight loops can starve the blocking pool in `serve` mode | Known. Not a true multi-tenant sandbox. |
| No package manager / no third-party libs | Only the CPython stdlib is available. Whitelisted further in `restricted` mode. |
| No filesystem / network access | Not enforced in `none` mode. In `restricted` mode `open` is banned and the import whitelist excludes `socket`, `urllib`, `os`, etc. |
| No subprocess / fork | Not enforced in `none` mode. In `restricted` mode the import whitelist excludes `subprocess`, `os`. |
| Script size | No hard limit — keep snippets short for readability and token cost when used as a tool. |

If you need a stronger sandbox (process isolation, resource limits, separate kernel namespace) consider running the script as a subprocess in a container outside of Colmena and integrating via `http_request`.

---

## Troubleshooting

### `'output' variable not defined → node returns null`

The script ran but never assigned `output`. Check for:
- Misspelling (`out`, `result`, etc.).
- An exception silently raised (the node returns the exception text in this case, so check the error path).
- `output` assigned inside a function but never called.

### `SandboxViolation: import 'X' is not allowed`

`sandbox_mode` is `restricted` and your code imports a non-whitelisted module. Either:
- Switch to `sandbox_mode: "none"` if the code is trusted.
- Reformulate the script to use only the allowed imports.
- If the LLM is writing the code, update the tool description with the allowed imports list so it stays inside the whitelist.

### `SandboxViolation: 'open' is not allowed in sandbox mode`

Same as above for banned builtins. Trusted code paths should use `"none"`. LLM-generated code paths should keep `"restricted"` and avoid file I/O entirely (return data to the DAG and let downstream nodes persist it).

### `SandboxTimeout: execution exceeded N seconds`

The script took longer than `sandbox_timeout_secs`. Either:
- Raise the timeout (`"sandbox_timeout_secs": 30`).
- Optimize the script (slice the input, avoid quadratic loops, etc.).
- Move the heavy work to a dedicated service and call it via `http_request`.

### `Python execution error: NameError: name 'X' is not defined`

The script references `X` as a Python variable, but no edge sent it. Double-check the upstream edges and whether the source node actually emits a field named `X`. If the source emits a nested object, you may need to extract: `{ "from": "source.field", "to": "py.X" }`.

### Inputs arrive as `dict` when you expected an attribute

`pythonize` converts JSON objects to Python `dict`, not to objects. Use `payload['key']`, not `payload.key`.

### LLM-generated code wrapped in markdown is interpreted as Python

The node strips ` ```python ... ``` ` automatically, so this works. If you ever need to preserve a literal triple-backtick block (rare), pass it through a different field name.

### Unit-test runs but `cargo test` for `sandbox_timeout_secs` would hang

This is intentional. A direct end-to-end test would deadlock the harness because the blocking thread holding the GIL cannot be cancelled. The wiring is verified through the e2e graph `tests/graphs/agents/python_sandbox_tool_test.json`. The `tokio::time::timeout` primitive itself is upstream-tested by tokio.

---

## Related Documentation

- `docs/node_configurations.json` → `python_script` — full machine-readable config schema.
- `docs/node_as_tools_reference.json` → `python_script` — tool-mode examples (Pattern A and Pattern B).
- `docs/agent_context/node_ports_reference.md` — quick reference of default ports for every node.
- `docs/superpowers/plans/2026-04-27-python-sandbox.md` — original implementation plan for the sandbox feature.
- Test graphs:
  - `tests/graphs/basic/python_simple_graph.json` — minimal DAG usage.
  - `tests/graphs/agents/python_llm_graph.json` — LLM emits code into the `code` input port.
  - `tests/graphs/agents/python_sandbox_tool_test.json` — LLM tool with `restricted` sandbox.
  - `tests/graphs/agents/python_sandbox_tool_thinking_test.json` — same as above with a reasoning model.
