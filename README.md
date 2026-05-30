# Colmena — AI Agent Orchestration Library

[![Rust](https://img.shields.io/badge/rust-1.95.0-orange.svg)](https://www.rust-lang.org)
[![Python](https://img.shields.io/badge/python-3.8+-blue.svg)](https://www.python.org)
[![License](https://img.shields.io/badge/license-MIT-green.svg)](LICENSE)
[![Status](https://img.shields.io/badge/status-alpha-red.svg)](https://github.com/Startti/colmena)

## What is Colmena?

Colmena is a **Rust-native AI agent orchestration library**. Its core is a DAG execution engine that runs directed acyclic graphs of nodes — LLM calls, SQL queries, Python scripts, HTTP requests, Socket.IO events, document generation, and more than 25 additional node types — asynchronously over Tokio.

The library provides a **unified multi-provider LLM abstraction** (OpenAI, Anthropic, Gemini) with streaming, tool calling, ReAct agent loops, persistent conversation memory, and structured-output extraction. Agents can be composed modularly using subgraphs and orchestrated with a built-in planner + critic loop.

Colmena is designed to be embedded: it exposes native bindings for **Python** (PyO3) and **Node.js** (napi-rs), and can also be used as a **CLI** (`dag_engine run`) or **HTTP server** (`dag_engine serve`). The architecture follows the **Hexagonal (Ports & Adapters)** pattern — the domain has zero infrastructure dependencies, and every external integration lives behind a trait.

Crate: `colmena_dag_engine` v0.3.0 · Repository: <https://github.com/Startti/colmena>

---

## Features at a glance

### LLM module
- Multi-provider support: OpenAI, Anthropic, Gemini — unified `LlmRepository` trait
- Synchronous and streaming responses
- Persistent conversation memory: SQLite and PostgreSQL
- Structured-output extraction, vision/document inputs
- `AgentService` — ReAct loop with tool calling

### DAG engine
- 25+ node types: `llm_call`, `http_request`, `sql_query`, `python_script`, `socketio_request`, `subgraph`, `orchestrator`, `trigger_webhook`, `suspend`, `document_create/edit/read`, `image_generation`, `image_edit`, `tts`, and more
- Tool calling: any node can be exposed as an LLM tool via `tool_configurations`
- Orchestrator node with HITL (human-in-the-loop) suspend/resume and dynamic replanning
- Subgraphs: compose and reuse agent modules; session isolation, HITL propagation
- Temporal & geographic context auto-injected into every LLM system message

### Skills & layered tool context
- Built-in skills (compiled with `include_dir!`) and user-provided skills (filesystem paths)
- LLM loads skills on demand via `load_skill` — no bloated system prompts
- Lazy tool loading (`lazy_tool_loading: true`): expose a name+summary catalog; full schema revealed only when the LLM calls `describe_tool`
- Layered tool context: every tool gets an auto-assembled block of description + config-derived policy + node-type guide + scoped skills

### SQL node
- Permission presets (`read_only`, `read_write`, `analytics`) with per-query deny lists
- Sandbox schema for user-defined functions; multi-tenant Row-Level Security
- Auto-creates missing `allowed_schemas` at init (`create_schemas_if_missing: true` by default)
- Optional LLM critic loop for query validation
- AST-based SQL parser (sqlparser crate) — no regex heuristics

### Multimedia generation
- `image_generation`: OpenAI (`gpt-image-1`) and Google Vertex AI (Imagen 4)
- `image_edit`: OpenAI multipart image editing
- `tts`: Text-to-speech via OpenAI, ElevenLabs, or Google Gemini TTS
- Storage abstraction: `LocalCache` (tests), `LocalHttp` (local dev), `HttpCallback` (production GCS)
- `COLMENA_LOCAL=true` env guard for safe local iteration without GCS credentials

### Attachments
- `$attachment:<document_id>` placeholder — bytes streamed directly to HTTP endpoints without LLM ever seeing them
- `load_attachment` tool: LLM loads document content on demand (ephemeral, no context bloat)
- Auto-summary of uploaded files via cheap-tier provider (Flash / 4o-mini / Haiku)
- `attachment_gc` standalone binary for TTL-based cleanup (designed for Cloud Scheduler → Cloud Run Job)

### Security
- Secure values: AES-256-GCM encrypted secrets, never exposed to LLM outputs
- `secure_suspend` node: interactive secret collection via LLM tool or top-level pause
- Outbound response masking — auto-hashes secrets in HTTP responses

### Bindings
- **Python**: `import colmena` — `ColmenaLlm`, `run_dag`, `serve_dag`, `validate_graph` (built with `maturin develop`)
- **Node.js**: `require('./index.node')` — `ColmenaLlm`, `runDag`, `serveDag` (built with `npm run build`)

---

## Quick start

No API keys required. This example runs a minimal two-node graph (webhook trigger + log) using only built-in node types:

```bash
# Build (first run only — subsequent runs use the cache)
cargo build --bin dag_engine

# Run a graph
cargo run --bin dag_engine -- run tests/graphs/basic/trigger.json
```

Expected output (abbreviated):

```
data: {"type":"node-end","node_id":"my_webhook","node_type":"trigger_webhook","output":{"message":"Hello from Simulator!"}}
[LogNode]: { "message": "Hello from Simulator!" }
data: {"type":"finish","finishReason":"stop",...}
data: [DONE]
```

Exit code 0.

To run a graph that calls a real LLM (requires API keys in `.env`):

```bash
source .env
cargo run --bin dag_engine -- run tests/graphs/agents/llm_call.json
```

---

## Use cases

- **Conversational agents with tools** — LLM nodes connected to SQL, HTTP, Python, and web-search tools in a single graph
- **Multi-step document workflows** — ingest attachments, generate Word/Excel outputs, TTS narration, all chained via DAG
- **Orchestrated multi-agent systems** — planner + specialist subgraphs + critic feedback loop with human-in-the-loop pauses
- **Production API services** — `dag_engine serve` exposes any graph as an SSE-streaming HTTP endpoint
- **Platform integrations** — embed Colmena in Python or Node.js services via native bindings; session state persists in Postgres

---

## Architecture

Colmena follows **Hexagonal Architecture (Ports & Adapters)**. The domain layer has zero infrastructure dependencies. Every external integration — LLM providers, databases, storage backends, Python runtime — is encapsulated behind a trait defined in the domain.

The entry points (CLI binaries, HTTP server, Python module, Node.js module) all delegate to `ColmenaEngine` / `DagRunUseCase`, which parses a `Graph` from JSON, executes nodes in topological order via `execute_stream()`, and publishes `NodeEvent` to an observer (SSE stream or CLI log).

For a full system map, execution lifecycle diagram, and module table:

**[docs/developer_guide/00_architecture_overview.md](docs/developer_guide/00_architecture_overview.md)** — start here.

---

## Documentation

| Document | What it covers |
|----------|---------------|
| [docs/developer_guide/00_architecture_overview.md](docs/developer_guide/00_architecture_overview.md) | System tour: modules, entry points, execution lifecycle, "follow your interest" navigation table |
| [docs/DEVELOPER_GUIDE.md](docs/DEVELOPER_GUIDE.md) | Master index of all 37 developer guides |
| [docs/node_configurations.json](docs/node_configurations.json) | Canonical config schema for every node type (fields, types, defaults) |
| [docs/node_as_tools_reference.json](docs/node_as_tools_reference.json) | How to expose nodes as LLM tools (`tool_configurations`, `node_schema`, `fixed_config`) |
| [docs/agent_context/node_ports_reference.md](docs/agent_context/node_ports_reference.md) | Ports and outputs per node type |
| [docs/dds/](docs/dds/) | Original design documents (hexagonal architecture, DAG engine, LLM module, security, variable resolution) |
| [docs/superpowers/specs/](docs/superpowers/specs/) | Feature specs (attachment resolution, multimedia pipeline, SQL hardening, etc.) |
| [docs/superpowers/plans/](docs/superpowers/plans/) | Implementation plans |
| [CLAUDE.md](CLAUDE.md) | Repo conventions, build commands, "Current Status" feature timeline, and AI-assistant instructions |

---

## Project status

**Version**: 0.3.0 (alpha) — Phases 1–6 and 9 complete. Phase 7 (testing) and Phase 8 (docs) in progress.

**Active development on `develop`.** Recent shipped features (see `CLAUDE.md` "Current Status" for the full timeline):

- Multimedia generation pipeline: `image_generation`, `image_edit`, `tts` — validated end-to-end in dev (2026-05-22)
- HTTP multipart streaming — `$attachment:<key>` parts streamed without in-memory buffering (2026-05-24)
- Attachment GC binary (`attachment_gc`) for TTL-based cleanup (2026-05-25)
- SQL node AST hardening — all regex heuristics replaced by `sqlparser` AST analysis (2026-05-26)
- SQL node auto-creates missing `allowed_schemas` at init (2026-05-28)
- Layered tool context — every node used as an LLM tool gets an auto-assembled policy + guide + skills block (2026-05-29)

---

## Build & test

```bash
# Check and build (Rust, pinned to 1.95.0 via rust-toolchain.toml)
cargo check
cargo build

# Run all unit + integration tests
cargo test --verbose

# Run tests that require env vars (DATABASE_URL, TAVILY_API_KEY, etc.)
source .env && cargo test -- --ignored

# Build Python bindings (requires maturin in .venv)
maturin develop

# Run Python tests
.venv/bin/pytest python/ -v

# Build Node.js bindings
npm run build

# Run the DAG engine CLI
cargo run --bin dag_engine -- run tests/graphs/basic/trigger.json

# Run the attachment GC (dry-run mode)
cargo run --bin attachment_gc -- --dry-run
```

---

## License

[MIT](LICENSE)
