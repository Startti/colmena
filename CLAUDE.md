# Colmena - AI Agent Orchestration Library

## Project Identity
- Rust-native AI agent orchestration library with Python (PyO3) and TypeScript (napi-rs) bindings
- **Hexagonal Architecture** (Ports & Adapters): domain / application / infrastructure layers
- Version: 0.3.0 (alpha) — Phases 1-6 and 9 complete, Phase 7 (testing) and 8 (docs) pending
- Repository: https://github.com/Startti/colmena

## Key Directories
- `src/libs/colmena/src/` — All Rust source code
  - `llm/` — LLM module: multi-provider abstraction (OpenAI, Anthropic, Gemini)
    - `domain/` — Traits (`LlmRepository`), value objects, errors
    - `application/` — Use cases (`LlmCallUseCase`, `AgentService`)
    - `infrastructure/` — Provider adapters, persistence
  - `dag_engine/` — DAG execution engine (25+ node types)
    - `domain/` — Graph structures, `ExecutableNode` trait
    - `application/` — DAG run orchestration
    - `infrastructure/` — Node implementations in `nodes/`, CLI, REST server
  - `python_bindings/` — PyO3 bindings (`#[pyclass]`, `#[pymethods]`)
  - `node_bindings/` — napi-rs bindings (`#[napi]`, `#[napi(object)]`)
  - `shared/` — Config resolver, service container
- `python/tests/` — Python test scripts
- `tests/` — Rust integration tests
- `tests/graphs/` — JSON DAG test graphs (basic/, agents/, advanced/, memory/, media/)
- `docs/` — Project documentation (start here before searching the repo)
  - `DEVELOPER_GUIDE.md` — **Main index** of all developer guides (22 sections)
  - `node_configurations.json` — **Canonical config schema** for every node type (fields, types, defaults)
  - `node_as_tools_reference.json` — **How to use nodes as LLM tools** (tool_configurations schema, node_schema, expose_sub_tools, examples per node type)
  - `agent_context/node_ports_reference.md` — Ports & outputs per node type
  - `developer_guide/` — 20 guides:
    - `01_architecture.md` — Hexagonal architecture, layers, data flow
    - `05_testing.md` — Test strategy, mocking, commands
    - `09_tool_calling.md` — Tool calling setup and usage in DAG
    - `12_dag_engine_guide.md` — DAG engine technical details
    - `13_security_strategy.md` — Secure Values, AES-256-GCM secrets
    - `14_llm_deep_dive.md` — LLM node advanced parameters
    - `15_memory_guide.md` — SQLite/PostgreSQL persistence
    - `16_data_flow_guide.md` — Data passing and transforms between nodes
    - `17_technical_reference.md` — JSON schemas and data types
    - `18_troubleshooting.md` — Common errors and fixes
    - `19_nested_agents_and_subgraphs.md` — Subgraph node, HITL propagation
    - `20_orchestrator_architecture.md` — Orchestrator: phases, bridge tasks, HITL, critic loop
    - `21_socketio_node.md` — Socket.IO node: config, ack/wait-event modes, LLM tool examples
    - `22_tool_execution_flow.md` — End-to-end tool call lifecycle: node_schema → merge → execution
    - `23_sql_node.md` — SQL node: permissions, validation pipeline, RLS, sandbox, LLM tool examples
    - `24_skills.md` — Skills feature: built-in + user-provided markdown packages loaded on-demand via `load_skill` tool
    - `29_lazy_tool_loading.md` — Lazy tool loading: progressive `describe_tool` reveal, `summary`/`eager` per tool, `tools_discovered` summary
  - `dds/` — Design documents:
    - `ARQUITECTURA_HEXAGONAL_GUIA.md`, `DAG_ENGINE_DISEÑO.md`, `DISEÑO_AGENTES_Y_TOOLS.md`
    - `MODULO_LLM_DISEÑO.md`, `RAG_DISEÑO.md`
    - `SECURE_VALUES_DISEÑO.md` — Security design
    - `VARIABLE_RESOLUTION_DISEÑO.md` — Variable resolution ($ref, $DYNAMIC, secure_values)
  - `examples/` — `USAGE_EXAMPLES.md`, `amadeus_test.md`, `python_usage.md`
  - `testing/` — `critic_feedback_test_plan.md`
  - `history/` — Past implementation notes (superseded, for historical context only)
  - `AUDIT_ENGINEERING_REPORT.md` — Audit findings and gaps

## Node Documentation — Where to Look
When you need to understand or modify any node (HTTP, LLM, orchestrator, etc.):
1. **Config fields**: `docs/node_configurations.json` — start here, canonical schema for all nodes
2. **Ports & outputs**: `docs/agent_context/node_ports_reference.md`
3. **Developer guide**: `docs/DEVELOPER_GUIDE.md` → pick the relevant section
4. **Design intent**: `docs/dds/` for architecture decisions
5. **Rust source**: `src/libs/colmena/src/dag_engine/infrastructure/nodes/` — one file per node
6. **Test graphs**: `tests/graphs/` organized by category (external/ for HTTP, agents/ for tool calling)

**Do NOT search the whole repo** — the answer is almost always in `docs/`.

## Build Commands
- **Rust**: `cargo check`, `cargo build`, `cargo test`, `cargo clippy`, `cargo fmt`
- **Python**: `maturin develop` (builds PyO3 bindings into `.venv`)
- **TypeScript**: `npm run build` (napi build with `--features node`)
- **DAG Engine CLI (run)**: `cargo run --bin dag_engine -- run <path/to/graph.json>`
- **DAG Engine CLI (serve)**: `cargo run --bin dag_engine -- serve <path/to/graph.json>`
- **Docs**: `cargo doc --no-deps --open`

**IMPORTANT — Python virtual environment:** Always use the project's `.venv` for Python commands:
```bash
.venv/bin/pip install ...           # Install packages
.venv/bin/pytest python/ -v         # Run Python tests
```
Or activate it first: `source .venv/bin/activate`

**IMPORTANT — LLM API keys:** The project `.env` at repo root contains API keys for OpenAI, Anthropic, and Gemini (env vars: `OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, `GEMINI_API_KEY`). Source it (`source .env` or `set -a; source .env; set +a`) before running graphs that hit real providers, e.g. `tests/graphs/agents/llm_call.json`. Do NOT commit or print the key values.

**IMPORTANT — Cargo package name:** The crate is named `colmena_dag_engine` (NOT `colmena`). When running tests for a specific module use:
```bash
cargo test --lib <module_name>                    # Run all tests matching module
cargo test --lib tool_configuration               # Example: tool_configuration tests
cargo test -p colmena_dag_engine --lib <module>   # Explicit package (same result)
```
Do NOT use `cargo test -p colmena` — that package does not exist and will error.

## Running JSON DAG Graphs

All JSON test graphs live in `tests/graphs/` organized by category:

| Categoría | Ruta | Descripción |
|-----------|------|-------------|
| Basic | `tests/graphs/basic/` | Nodos simples: math, log, trigger, suspend |
| Agents | `tests/graphs/agents/` | llm_call, tool calling, streaming, extraction |
| Advanced | `tests/graphs/advanced/` | Orchestrators, planners, multi-step agents |
| Memory | `tests/graphs/memory/` | SQLite y PostgreSQL persistence |
| External | `tests/graphs/external/` | HTTP requests, Amadeus API |
| Media | `tests/graphs/media/` | Vision and document processing |

### Ejecutar un grafo en modo local (run)
```bash
# Sintaxis
cargo run --bin dag_engine -- run <path/to/graph.json>

# Ejemplos concretos
cargo run --bin dag_engine -- run tests/graphs/basic/trigger.json
cargo run --bin dag_engine -- run tests/graphs/agents/llm_call.json
cargo run --bin dag_engine -- run tests/graphs/agents/http_tool_dynamic_placeholder_test.json
cargo run --bin dag_engine -- run tests/graphs/memory/memory_sqlite_example.json
```

### Opciones adicionales del subcomando `run`
```bash
cargo run --bin dag_engine -- run <file> [--session-id <id>] [--answer <text>] [--include-extra-info]
```

### Levantar como servidor HTTP (serve)
```bash
cargo run --bin dag_engine -- serve tests/graphs/agents/llm_call.json
# Servidor disponible en http://localhost:3000
```

## Architecture Rules
- Domain layer has **ZERO** infrastructure dependencies
- All external integrations go through traits (ports) defined in domain
- Infrastructure layer implements adapters for those ports
- Use `thiserror` for domain errors, `anyhow` for infrastructure
- All DAG nodes implement the `ExecutableNode` trait
- Node outputs use `{ "output": ... }` convention

## Feature Flags
- `python` — enables PyO3 bindings
- `node` — enables napi-rs bindings
- Default: neither (pure Rust library)

## Testing
- Unit tests: inline `#[cfg(test)]` modules in source files
- Integration tests: `tests/` (Rust), `python/tests/` (Python)
- Mocking: `mockall` crate, `MockAdapter` for LLM tests without API calls
- Test graphs: `tests/graphs/` JSON files

## Conventions
- **Rust**: PascalCase types, snake_case functions, `///` doc comments on all public items
- **Python**: PEP 8, Google-style docstrings
- **Errors**: `Result<T, DomainError>` in domain, `?` propagation
- **Docs language**: Spanish in `docs/`, English in code comments and API docs

## Tool Config Standard — `node_schema+fixed` vs `fixed_config`

When configuring nodes as LLM tools, use this rule to decide where to put fields the LLM should NOT see:

| Situation | Use |
|-----------|-----|
| Field is a node behavioral parameter (`sandbox_mode`, `method`, `secure`) | `node_schema` with `fixed` |
| Field co-exists with LLM-visible fields in the same schema | `node_schema` with `fixed` |
| Field is purely static plumbing (`base_url` for a known endpoint, `api_key`) | `fixed_config` |
| Quick override with `$DYNAMIC` placeholders (all strings, flat, ≤5 fields) | `fixed_config` |

**When in doubt: `node_schema+fixed` is always correct.**

```json
// CORRECT — behavioral param alongside LLM-visible field
"node_schema": {
  "sandbox_mode": { "fixed": "restricted" },
  "code":         { "type": "string", "required": true, "description": "..." }
}

// CORRECT — static plumbing only
"fixed_config": { "base_url": "https://api.example.com", "method": "GET" }

// WRONG — mixing: sandbox_mode belongs in node_schema, not fixed_config
"node_schema": { "code": { "type": "string", "required": true } },
"fixed_config": { "sandbox_mode": "restricted" }
```

Full reference: `docs/node_as_tools_reference.json` → `parameter_strategies.node_schema_fixed_vs_fixed_config`

## Tool Config Standard — `enabled_tools`

Tools defined in `tool_configurations` are **auto-enabled** — you do NOT need `enabled_tools` to activate them.

```json
// CORRECT — omit enabled_tools when all tools are in tool_configurations
"tool_configurations": { "run_python": { ... }, "search_products": { ... } }

// ONLY needed for built-in toolkit tools (tavily_client, etc.) or wildcard
"enabled_tools": "*"            // expose everything including built-ins
"enabled_tools": ["tavily_web"] // enable a built-in alongside tool_configurations
```

## Skills
- `/rust_dev` — Rust development protocol (architecture-aware, includes documentation)
- `/python_dev` — Python development protocol (PyO3/maturin-aware, includes documentation)
- `/typescript_dev` — TypeScript/Node.js development protocol (napi-rs-aware, includes documentation)
- `/ideation` — Expert critic and ideation partner for brainstorming and planning
- `/test_graph` — Run, validate, and debug JSON DAG graph files via the DAG engine CLI

## Current Status
- **Audit v0.3.0**: Systematic audit of docs 01-12 completed.
- **Critical Finding**: Tool calling with Secure Values is currently broken (see report).
- **Engineering Report**: See [docs/AUDIT_ENGINEERING_REPORT.md](file:///home/daniel-garcia4/startti/colmena/docs/AUDIT_ENGINEERING_REPORT.md) for detailed gaps and next steps.
- **Tasks**: See `docs/PENDING_TASKS.md` for overall project tracking.
