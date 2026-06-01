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
  - `developer_guide/` — 27 guides (numbered up to 36; key entries highlighted below — see `DEVELOPER_GUIDE.md` for the full index):
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
    - `31_load_attachment.md` — On-demand attachment loading inside the LLM loop; auto-summary via cheap-tier provider
    - `32_multimedia_generation.md` — `image_generation` / `image_edit` / `tts` nodes; artifact storage with 3 adapters (LocalCache/LocalHttp/HttpCallback); `COLMENA_LOCAL` env guard; `$attachment:<key>` placeholder; binary scrubber. **Live in dev as of 2026-05-22** — Vertex uses ADC (no key file needed on Cloud Run); Gemini TTS auto-wraps L16 PCM to playable WAV; Vertex `google_project_id`/`google_location` fall back to `GOOGLE_CLOUD_PROJECT`/`GOOGLE_CLOUD_LOCATION` env vars (best practice: omit from graph JSON). Canvas-side configuration reference (per-tool fields, accepted values per model, env-var resolution chain) lives in the ADP repo at `docs/MULTIMEDIA_TOOLS_CANVAS_CONFIG.md`.
    - `35_temporal_geographic_context.md` — Auto-injected date/time/location/locale block in every llm_call system message
  - `dds/` — Design documents:
    - `ARQUITECTURA_HEXAGONAL_GUIA.md`, `DAG_ENGINE_DISEÑO.md`, `DISEÑO_AGENTES_Y_TOOLS.md`
    - `MODULO_LLM_DISEÑO.md`, `RAG_DISEÑO.md`
    - `SECURE_VALUES_DISEÑO.md` — Security design
    - `VARIABLE_RESOLUTION_DISEÑO.md` — Variable resolution ($ref, $DYNAMIC, secure_values)
  - `examples/` — `USAGE_EXAMPLES.md`, `amadeus_test.md`, `python_usage.md`
  - `testing/` — `critic_feedback_test_plan.md`
  - `history/` — Past implementation notes (superseded, for historical context only)

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
- **Attachment GC (cleanup)**: `cargo run --bin attachment_gc -- --dry-run` (or without --dry-run to actually delete)
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
cargo run --bin dag_engine -- run <file> [--session-id <id>] [--agent-session-id <id>] [--answer <text>] [--include-extra-info]
```

### Regla — Usar `--agent-session-id` en todas las pruebas de grafos
Para cualquier flujo con estado entre runs (suspend/resume, multi-turn conversacional, secure_values, agentes con memoria), **siempre pasar `--agent-session-id <id_estable>`** en lugar de depender de `--session-id`.

**Por qué:** los tres subsistemas que persisten estado entre runs keyan primero por `agent_session_id` (estable) con fallback a `session_id` (ephemeral, rotates per CLI invocation):
- Memoria conversacional (`llm_node_history`)
- DAG state para resume chains (`dag_runs.find_resume_entry`)
- Secure values (`secure_value_mappings`)

Cada `cargo run` genera un `session_id` ephemeral nuevo. Sin `--agent-session-id` el resume/memoria no funciona entre invocaciones distintas, y la prueba no representa cómo ADP ejecuta agentes en producción.

**Patrón canónico:**
```bash
# Run 1 — suspend
cargo run --bin dag_engine -- run graph.json --agent-session-id agent_demo_001

# Run 2 — resume (mismo agent, session_id ephemeral nuevo automático)
cargo run --bin dag_engine -- run graph.json --agent-session-id agent_demo_001 \
  --answer "Q[<id>]: <pregunta>\nA[<id>]: <respuesta>"
```

`--session-id` sigue siendo válido para tests one-shot sin estado.

### Regla — Formato canónico Q/A para `--answer`

Todos los nodos de pausa (`suspend`, `secure_suspend`) consumen `--answer` en formato ID-keyed:

```
Q[<id>]: <pregunta echo>
A[<id>]: <respuesta>
Q[<id2>]: <pregunta echo>
A[<id2>]: <respuesta>
```

- `<id>` proviene de `config.id` (suspend clásico, **obligatorio**) o **`secrets[i].name`** (secure_suspend — `name` ES el id; `config.id` y `__node_id` no influyen).
- Orden-independiente — el parser hace bind por id.
- Multilínea preservada entre `A[<id>]:` y el siguiente prefijo o EOF.
- `options` en choice questions es solo sugerencia UX — cualquier texto es válido.
- Spec completo: [docs/superpowers/specs/2026-05-08-suspend-qa-response-format-design.md](docs/superpowers/specs/2026-05-08-suspend-qa-response-format-design.md).

### Levantar como servidor HTTP (serve)
```bash
cargo run --bin dag_engine -- serve tests/graphs/agents/llm_call.json
# Servidor disponible en http://localhost:3000
```

### Regla — Grafos JSON con tools reales (no mocks)
Cuando crees un grafo JSON para probar funcionalidad (LLM tool calling, lazy loading, agents, etc.):
- **Siempre** usa `node_type` de nodos registrados reales (`current_time`, `add`, `multiply`, `http_request`, `tavily_client`, `sql_query`, `python_script`, etc.) o referencia tools existentes en `enabled_tools`.
- **Nunca** uses `node_type: "log"` (u otro placeholder) como backing de un tool en `tool_configurations` solo para llenar la lista — eso convierte la prueba en un mock y oculta fallos reales (validación de schema, ejecución, parseo de outputs).
- Si el tool que necesitas no existe, créalo como un `ExecutableNode` real y regístralo en `registry.rs` antes de usarlo en el grafo.
- Verifica que cada `node_type` esté en `src/libs/colmena/src/dag_engine/infrastructure/registry.rs` antes de ejecutar el grafo.

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
- **CI vs local**: CI runs `cargo test --verbose` (unit + integration + doctests). `cargo test --lib` only runs unit tests — use `--verbose` before pushing to catch doctest failures.
- **`#[ignore]` convention**: tests that read required env vars (e.g. `DATABASE_URL`, `TAVILY_API_KEY`) MUST be marked `#[ignore = "requires X — run with \`cargo test -- --ignored\`"]`. Otherwise they panic in CI where `.env` is not available. Run them locally with `source .env && cargo test -- --ignored`.
- **Rust toolchain**: pinned to `1.95.0` via `rust-toolchain.toml` at repo root. CI workflows use `actions-rust-lang/setup-rust-toolchain@v1` which reads the toml automatically — local and CI are always aligned.
- **Deny-warnings**: `Cargo.toml` has `[lints.rust] warnings = "deny"`. Any rustc warning fails the build (unused import, dead code, deprecated API). For tests that intentionally exercise a `#[deprecated]` API (backward-compat coverage), put `#[allow(deprecated)]` on the test module — never on production code.
- Full guide: [docs/developer_guide/05_testing.md](docs/developer_guide/05_testing.md)

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

> **Flag-only toolkits.** Today only `api_explorer` supports activation by flag alone (toolkit-prefix match + dispatch fallback synthesises a default `ToolConfiguration`). Other toolkits (`tavily_client`, future `browser`) still require an explicit `tool_configurations` entry because they need per-instance config (`api_key`, defaults, etc.).

Because `api_explorer` has no required per-instance configuration (auth comes from the spec itself, not from node config), it's the only toolkit with flag-only activation today; other toolkits like `tavily_client` still need a `tool_configurations` entry to pass `api_key`.

```json
// RECOMMENDED — flag-only activation for api_explorer (auto-exposes 5 sub-tools:
// load_spec, list_endpoints, search_endpoint, get_endpoint_details, build_http_request)
"enabled_tools": ["api_explorer"]

// Wildcard — expose everything including built-ins
"enabled_tools": "*"

// Enable a built-in alongside tool_configurations
"enabled_tools": ["tavily_web"]

// CORRECT — omit enabled_tools when all tools are in tool_configurations
"tool_configurations": { "run_python": { ... }, "search_products": { ... } }
```

For overrides (custom alias, `expose_sub_tools` filtering, `cache_ttl_seconds`, etc.), declare an explicit `tool_configurations` entry with `node_type: "api_explorer"` — see `docs/developer_guide/25_web_nodes.md`.

## Skills
- `/rust_dev` — Rust development protocol (architecture-aware, includes documentation)
- `/python_dev` — Python development protocol (PyO3/maturin-aware, includes documentation)
- `/typescript_dev` — TypeScript/Node.js development protocol (napi-rs-aware, includes documentation)
- `/ideation` — Expert critic and ideation partner for brainstorming and planning
- `/test_graph` — Run, validate, and debug JSON DAG graph files via the DAG engine CLI

## Current Status
- **Active development on `develop`**. See `docs/CHANGELOG_*.md` for the rolling change log; `docs/BACKLOG.md` for parked items.
- **Gemini scalar tool response fix shipped 2026-06-01** — `gemini_adapter.rs` now wraps any non-object `LlmMessage::Tool` content (scalars, arrays, null, JSON strings) in `{ "result": <value> }` before injecting into `functionResponse.response`. Fixes silent agent death (`completion_tokens: 0`, empty `result`) on Gemini agents whose tool returns a non-dict — e.g. a `python_script` that assigns `output = 5040`. Previously the adapter only wrapped on parse-failure; now it wraps any non-object too. OpenAI/Anthropic adapters audited clean (they pass tool content as opaque strings). ADP unaffected — wire-format change between Colmena and the Gemini REST API only; never crosses the SSE boundary. See [`docs/superpowers/plans/2026-06-01-gemini-scalar-tool-response-fix.md`](docs/superpowers/plans/2026-06-01-gemini-scalar-tool-response-fix.md).
- **Multimedia generation pipeline shipped 2026-05-22** — 3 nodes (image_generation, image_edit, tts) live in dev (`api.dev.startti.ai`). Deployed colmena commits on develop: `b6eaeb9`, `f2cc36d`, `058762e`, `4f50db2`, `a0c9e98`, `004489d`. The downstream ADP repo (`/Users/danielgarcia/startti/adp`) consumes colmena develop directly via its platform service (`apps/service/ia/platform/{api,worker}/`). Pending: cost tracking, frontend rendering (badge + audio player), prod deploy. See [`docs/superpowers/plans/2026-05-19-multimedia-generation-nodes.md`](docs/superpowers/plans/2026-05-19-multimedia-generation-nodes.md) for the original plan.
- **Breaking-change discipline**: anything that changes colmena's public API (`EngineConfig`, `ColmenaEngine`, exported trait signatures) must be swept against the ADP worker (`apps/service/ia/platform/{worker,api}/src/` in the adp repo) BEFORE pushing to colmena develop — that worker pulls colmena develop directly via Cargo and a breaking change fails its next Cloud Build.
- **HTTP multipart streaming shipped 2026-05-24** — `http_request` node now supports `Content-Type: multipart/form-data` with URL-sourced and `$attachment:<key>`-sourced parts, both streamed end-to-end (no in-memory buffering). `OutputStorageRepository` extended with additive `read_stream` method. See [`docs/developer_guide/25_web_nodes.md`](docs/developer_guide/25_web_nodes.md) and the spec at [`docs/superpowers/specs/2026-05-24-http-multipart-streaming-design.md`](docs/superpowers/specs/2026-05-24-http-multipart-streaming-design.md).
- **SQL node parser hardened 2026-05-26** — all regex/substring heuristics replaced by `sqlparser` AST analysis (`infrastructure/sql_ast.rs`). Fixes a false positive where decimal literals like `1.81` were misread as schema references. Multi-statement queries are now validated per-statement (closes a hole where `SELECT 1; DROP TABLE x;` slipped through). New DDL kinds (`CREATE SCHEMA`/`INDEX`/`VIEW`) are explicitly blocked with clear messages.
- **SQL node auto-creates `allowed_schemas` 2026-05-28** — at init the `sql_query` node provisions any schema in `permissions.allowed_schemas` that doesn't exist (operator-driven `CREATE SCHEMA IF NOT EXISTS`, quoted). Gated by `permissions.create_schemas_if_missing` (**default `true`** — absent flag = on). Check-then-create (existing schemas never re-created), hard-fails init if a missing schema can't be created, skips `information_schema`/`pg_catalog`. Does NOT relax the LLM `CREATE SCHEMA` block. Adds `missing_schemas`/`create_schema` to the internal `SqlConnectionPort` trait (no external impls → no ADP break). See [`docs/superpowers/plans/2026-05-28-sql-node-auto-create-allowed-schemas.md`](docs/superpowers/plans/2026-05-28-sql-node-auto-create-allowed-schemas.md) and [`docs/developer_guide/23_sql_node.md`](docs/developer_guide/23_sql_node.md).
- **Layered tool context shipped 2026-05-29** — every node used as an LLM
  tool now receives an auto-assembled context block: description +
  config-derived policy (via `ExecutableNode::tool_description_supplement`)
  + node-type best-practices guide (a `SKILL.md` with
  `node_type: <name>` frontmatter, auto-folded) + announcement of
  tool-scoped layer-2 skills (`tool_configurations.<name>.skills`).
  Layer-2 skills are gated by visibility on the lazy `discovered_set`
  (visible after `describe_tool`; from turn 1 in non-lazy). Reuses the
  Skills infra (`include_dir!`, frontmatter, 64 KB). First node with a
  guide: `sql_query`. **Skill load list is fully auto-derived** —
  operators only declare scoped skills in `tool_configurations.<name>.skills`
  (one place); the engine auto-loads builtin skills referenced there,
  auto-folds node-type guides when the node_type matches, and
  auto-discovers all `SKILL.md` files under `llm_call.skills.paths`.
  See [`docs/superpowers/specs/2026-05-29-layered-tool-context-design.md`](docs/superpowers/specs/2026-05-29-layered-tool-context-design.md)
  and [`docs/developer_guide/24_skills.md`](docs/developer_guide/24_skills.md)
  ("How skills auto-load"; also canonical reference for skill authoring —
  single-skill dir vs folder-of-skills, error matrix, references workflow).
- **Attachment uniform resolution Plan A shipped 2026-05-25** — any document (inline, signed URL, or generated artifact) can be forwarded via `$attachment:<document_id>` in `http_request` multipart. Catalog auto-injected in LLM system message. Bytes persist uniformly to `OutputStorageRepository` at registration, regardless of source. See [`docs/superpowers/specs/2026-05-25-attachment-uniform-resolution-design.md`](docs/superpowers/specs/2026-05-25-attachment-uniform-resolution-design.md).
- **Attachment uniform resolution Plan B shipped 2026-05-25** — LLM no longer auto-receives attached doc content; catalog-driven via system message. `load_attachment` results are ephemeral (marker in history, not content). `image_generation`/`image_edit`/`tts` tool results dropped legacy `attachment_id` and `url`; only `document_id` remains. BREAKING for ADP frontend (Rust services swept clean). See [`docs/superpowers/specs/2026-05-25-attachment-uniform-resolution-design.md`](docs/superpowers/specs/2026-05-25-attachment-uniform-resolution-design.md) and [`docs/superpowers/specs/2026-05-25-plan-b-adp-migration-notes.md`](docs/superpowers/specs/2026-05-25-plan-b-adp-migration-notes.md).
- **Attachment uniform resolution Plan C shipped 2026-05-25** — new `attachment_gc`
  binary deletes `conversation_attachments` rows + their backing blobs when
  `COALESCE(last_used_at, registered_at) < now() - COLMENA_ATTACHMENT_TTL_DAYS` (default 7).
  Designed to run as Cloud Scheduler → Cloud Run Job. Requires host application
  to expose `<base>/internal/gcs/delete` endpoint. See
  [`docs/developer_guide/36_attachment_gc.md`](docs/developer_guide/36_attachment_gc.md).
