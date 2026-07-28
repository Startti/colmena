---
name: test_graph
description: Protocol for running and testing Colmena DAG graph JSON files. Use when the user wants to test, run, validate, or debug a JSON graph file in tests/graphs/ or any .json DAG definition. Triggers on phrases like "test this graph", "run this json", "check this graph", "does this graph work", "execute this dag".
---

> **MANDATORY — Colmena work uses Colmena nodes.** Any task in/for Colmena MUST be
> built and verified as a real Colmena graph of registered nodes (`python_script`,
> `sql_query`, `llm_call`, `http_request`, `for_each`, `subgraph`, …) run through the
> DAG engine (`cargo run --bin dag_engine -- run <graph.json>`) — never as a
> standalone script tested in isolation. Plain code is only the body of a
> `python_script` node, and must be embedded in a graph and exercised E2E before the
> task counts as done.

# Test Graph Skill

## When to Use
- Testing or running any `.json` DAG graph file
- Validating that a graph executes correctly
- Debugging a graph that isn't working
- Checking that a newly written graph produces expected output

## Graph File Locations

```
tests/graphs/
├── basic/        — Simple single-node graphs
├── agents/       — LLM agent graphs (tool calling, ReAct)
├── advanced/     — Multi-step, branching graphs
├── memory/       — Memory/RAG graphs
├── media/        — Vision/media graphs
└── external/     — HTTP, external API graphs
```

## How to Run a Graph

The DAG engine CLI uses the `run` subcommand:

```bash
# Syntax (run from the repo root)
cargo run --bin dag_engine -- run <path/to/graph.json>

# Options
cargo run --bin dag_engine -- run <file> [--session-id <id>] [--answer <text>] [--include-extra-info]

# Serve mode (starts HTTP server)
cargo run --bin dag_engine -- serve <path/to/graph.json>
```

### Ejemplos concretos

```bash
# Basic
cargo run --bin dag_engine -- run tests/graphs/basic/trigger.json
cargo run --bin dag_engine -- run tests/graphs/basic/power.json

# Agents & LLM
cargo run --bin dag_engine -- run tests/graphs/agents/llm_call.json
cargo run --bin dag_engine -- run tests/graphs/agents/agent_with_tools.json
cargo run --bin dag_engine -- run tests/graphs/agents/http_tool_dynamic_placeholder_test.json
cargo run --bin dag_engine -- run tests/graphs/agents/extraction_example.json
cargo run --bin dag_engine -- run tests/graphs/agents/planner_test.json

# Memory
cargo run --bin dag_engine -- run tests/graphs/memory/memory_sqlite_example.json
cargo run --bin dag_engine -- run tests/graphs/memory/memory_postgres_example.json

# External HTTP
cargo run --bin dag_engine -- run tests/graphs/external/http_request.json
cargo run --bin dag_engine -- run tests/graphs/external/dynamic_http.json
```

Set required env vars before running if the graph uses external APIs:

```bash
export OPENAI_API_KEY=sk-...
export ANTHROPIC_API_KEY=sk-ant-...
cargo run --bin dag_engine -- run tests/graphs/agents/agent_with_tools.json
```

## Validation Protocol

Before running a graph, validate its structure:

### 1. JSON Structure Check
Verify the graph has:
- `nodes` object with at least one node
- `edges` array defining connections
- Each node has `type` and optionally `config`

### 2. Node Types
Check that all `type` values are valid. Reference: `docs/agent_context/nodes_documentation.md`

Common node types:
- `trigger_webhook` — entry point for HTTP-triggered graphs
- `llm_call` — LLM inference (OpenAI, Anthropic, Gemini)
- `http_request` — outbound HTTP calls
- `log` — output/debug logging
- `python_script` — embedded Python execution
- `math_operation` — arithmetic operations
- `text_transform` — string manipulation

### 3. Edge Validation
- Every non-trigger node must be reachable via edges
- No cycles (DAG = Directed Acyclic Graph)
- `from` and `to` values must match node keys in `nodes`

### 4. Config Validation
- LLM nodes: check `provider`, `model`, and `api_key` (use `${ENV_VAR}` pattern)
- HTTP nodes: check `base_url`, `endpoint`, `method`
- Tool configurations: verify `enabled_tools` match keys in `tool_configurations`

## Running & Interpreting Output

After running, look for:
- `✓` or `OK` — node executed successfully
- Node output under `{ "output": ... }` convention
- Error messages with the failing node name

## Writing Integration Tests

If the graph should have a permanent Rust test, add it to `tests/rust/`. There is **no** `run_graph_from_file` helper — Rust integration tests in this project test the Colmena API directly (not JSON files). Look at existing tests for the real pattern:

- `tests/rust/dynamic_placeholder_integration_test.rs` — tests `DagToolExecutor` and `ToolConfiguration` directly
- `tests/rust/openai_tool_test.rs` / `tests/rust/gemini_tool_test.rs` — tests against LLM providers

For end-to-end JSON graph tests, use the CLI:

```bash
cargo run --bin dag_engine -- run tests/graphs/<category>/<graph>.json
```

This is the preferred way to validate a JSON graph — no Rust boilerplate required.

## Common Issues

| Problem | Likely Cause | Fix |
|---|---|---|
| `Unknown node type` | Typo or unregistered node | Check `nodes_documentation.md` |
| `Missing env var` | API key not set | Export the required `${VAR}` |
| `Edge references unknown node` | Typo in `from`/`to` | Match exact node keys |
| `Tool not found` | `enabled_tools` mismatch | Keys must match `tool_configurations` |
| Build error | Rust code changed | Run `cargo build` first |
