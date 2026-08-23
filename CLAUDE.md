# Colmena - AI Agent Orchestration Library

## Project Identity
- Rust-native AI agent orchestration library with Python (PyO3) and TypeScript (napi-rs) bindings
- **Hexagonal Architecture** (Ports & Adapters): domain / application / infrastructure layers
- Version: 0.4.0 (alpha) — Phases 1-6 and 9 complete, Phase 7 (testing) and 8 (docs) pending
- Repository: https://github.com/Startti/colmena

## MANDATORY — Documentation ships WITH the code
Every push and every PR carries its documentation update **in the same change**.
Never a follow-up, never "lo actualizo después", never a separate ticket. A change
that ships without its docs is not done.

Documentation that lags the code is worse than none — people trust it and act on
stale rules. This is enforced, not suggested: a `PreToolUse` hook
([`.claude/hooks/require-docs-with-code.sh`](.claude/hooks/require-docs-with-code.sh))
blocks `git push` and `gh pr create` when the outgoing diff touches repo files and
no documentation.

Run this audit **before `review start`**, not merely before pushing:

1. **Update docs in the same commit range as the code.**
2. **Grep for stale references OUTSIDE the area you touched.** This is the step
   that gets skipped. When removing or renaming anything public — a constant, an
   env var, an event type, a config field, a limit — grep the whole `docs/` tree
   for the old name AND for prose describing the old behavior (e.g. "máximo 5
   niveles"), not just the files already open. The canonical references
   (`docs/node_as_tools_reference.json`, `docs/node_configurations.json`,
   `docs/agent_context/`, the developer guides) are read as ground truth by both
   agents and humans, and must never lag.
3. **Audit against the code on disk, never against memory of what you wrote.**
   `git log -S "<string>"` is the cheap check for "did this actually ship?".
4. **Cross-verify claims against real behavior.** For anything observable (SSE
   frames, CLI output, API responses), run it and check the doc's claims against
   the captured output. A doc that reads well can still be false.
5. **The PR body and commit message are documentation too.** They land in
   permanent history and are read more than the docs. `develop` is shared, so a
   wrong claim there cannot be rewritten — only corrected by a PR comment.
6. **Check links, anchors, and JSON validity.** Run the guard:

   ```bash
   python3 scripts/check_doc_links.py docs
   ```

   It fails on two things: a relative markdown link under `docs/` whose target
   does not exist, and a living doc naming a `tests/graphs/**.json` graph that
   was never committed (graph paths are usually inline code spans, so a plain
   link check misses them). `docs/superpowers/`, `docs/history/` and
   `docs/archive/` are exempt from the graph check — a plan there may name a
   graph that was proposed and never built. CI runs this on every PR to
   `develop`.

**Ordering matters as much as content.** A review receipt is bound to the exact
bytes of the candidate tree, and the review contract exposes no transition that
refreshes that snapshot (`immutable_snapshot` is a mandatory feature, not a
toggle). Documentation authored AFTER `review start` invalidates the receipt just
as surely as a `cargo fmt` would, and the only legal recovery is a brand-new
review: new lineage, new lens fan-out, new correction budget. In August 2026 that
pattern burned 53 review lenses on roughly six changes, with four base commits
each carrying three separate review starts. Follow this order:

```
implement → fix → docs → tests → cargo fmt → tests green → review start → finalize → commit
```

Never run `review start` mid-flight, with docs still pending.

**Size the candidate before you freeze it.** Documentation counts toward the
review tier exactly like code does, and docs run roughly 25-30% of a change in
this repo — so a 300-line code change routinely lands at 450+ total and silently
buys the 4-lens tier. Check the number first:

```bash
python3 scripts/review_size.py
```

It prints code/docs/total, the tier and lens count you will get, and the
correction budget with its margin. Above 400 total lines a change that touches
code jumps to 4 lenses (a pure-documentation change stays at one), and the
correction budget saturates at 200 — so the larger the candidate,
the *smaller* its proportional room to absorb findings before it escalates, and
escalation has no reentry.

**The reported tier is a floor, not a ceiling.** Size is only one input: gentle-ai
also forces `high` on non-size signals such as a file gaining mode `100755`
(`executable_mode`) or a file that spawns processes (`process_boundary`) — this
very script trips both, and is `high` at 283 lines. The tool reports the signals
it can detect and labels them, but it cannot see every rule the binary applies.
Slicing does not lower a signal-forced tier. Use `--base-ref origin/develop` (after a `git fetch`) to size a whole branch — a stale local `develop` silently measures merged PRs too. When
it reports `high`, slice the change with the `chained-pr` skill instead of paying
for four lenses.

If a change legitimately needs no docs (a revert, a CI-only fix, a security
patch), retry the push with the `DOCS_EXEMPT=1` prefix. Deliberate and visible,
rather than silently working around the gate.

## MANDATORY — Colmena work is done with Colmena nodes
When ANYTHING is requested in or for Colmena (a feature, an agent, a solution, a
test), it MUST be built and verified **exclusively as a real Colmena graph made of
registered nodes** (`python_script`, `sql_query`, `llm_call`, `http_request`,
`for_each`, `subgraph`, etc.) and run through the DAG engine
(`cargo run --bin dag_engine -- run <graph.json>`). It is NOT done as a standalone
Python/Rust script tested in isolation.

- Plain code is acceptable ONLY as the **body destined for a `python_script` node**,
  and it must then be **embedded in a real graph and exercised E2E through the DAG
  engine** before the task counts as done.
- Never back a tool/step with a placeholder/mock instead of a real registered node
  (see also the "Grafos JSON con tools reales" rule below).
- Verifying a standalone library with unit tests is NOT the same as delivering a
  Colmena solution. The deliverable is a working graph exercised by the engine.

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
- `src/libs/colmena/text/` — **LLM-facing text registry**. Every prompt,
  description, and summary the LLM reads lives here as YAML or Markdown.
  Edit a file in `text/prompts/` or `text/tools/` to change what the
  model sees — no Rust changes needed. See
  [docs/developer_guide/41_builtin_tools_index.md](docs/developer_guide/41_builtin_tools_index.md)
  for the user-facing index.
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
    - `13_security_strategy.md` — Secure Values, pgcrypto pgp_sym_encrypt secrets
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
    - `ARQUITECTURA_HEXAGONAL_GUIA.md`, `DAG_ENGINE_DISEÑO.md`
    - `SECURE_VALUES_DISEÑO.md` — Security design
    - `VARIABLE_RESOLUTION_DISEÑO.md` — Variable resolution ($ref, $DYNAMIC, secure_values)
    - `DISEÑO_AGENTES_Y_TOOLS.md`, `MODULO_LLM_DISEÑO.md` and `RAG_DISEÑO.md` were
      archived as superseded (PR #83) — they now live in `docs/archive/proposals/`
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

## Impact / Blast-Radius — Before Changing Any Rust File
Before assessing which files a change touches (exploration, spec, or design phase), consult the **auto-generated module dependency map** FIRST, then open only the files it points to — do not scan the whole repo:

- **`docs/agent_context/module_dependency_map.md`** — for every `.rs` module: **Used by** (the files that break if you change its public surface = its blast radius) and **Depends on** (what it needs). Opens with a blast-radius ranking of the riskiest modules to touch (e.g. `llm::domain` has 76 importers).
- It is **derived from `use crate::...` imports — never hand-edit it.** Regenerate after adding/removing files or imports: `python3 scripts/gen_module_map.py`.
- Workflow: look up the target file → read its **Used by** list → open only those callers + the target to judge impact. This replaces reading 4+ files to guess coupling.

When delegating exploration (e.g. `sdd-explore`), forward this map as the starting point for identifying affected areas and coupling.

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

> **Flag-only toolkits.** `api_explorer`, `gsheets`, `gdocs`, and `gdocsread` support activation by flag alone — credentials come from process-level env (the new OAuth user-scoped flow's `COLMENA_GOOGLE_OAUTH_*` vars for gsheets/gdocs since 2026-06-10; spec itself for api_explorer), not from per-node config, so listing the alias in `enabled_tools` is enough. All four also honor `!sub_tool` exclusions inside the same array (e.g. `["gsheets", "!gsheets_export_xlsx"]` → 9 tools). Other toolkits (`tavily_client`, future `browser`) still require an explicit `tool_configurations` entry because they need per-instance config (`api_key`, defaults, etc.).
>
> **Google auth migration (2026-06-10):** Service Account JSON path is gone from production. Production now uses OAuth user-scoped via `agents@startti.co`. Required env vars: `COLMENA_GOOGLE_OAUTH_CLIENT_ID`, `..._CLIENT_SECRET`, `..._REFRESH_TOKEN`, `COLMENA_GOOGLE_SHARE_EMAIL`. ADP deploy_gcp.sh must be updated to mount these from Secret Manager + remove the old `GOOGLE_APPLICATION_CREDENTIALS` mount. Full guide: [`docs/developer_guide/47_google_oauth.md`](docs/developer_guide/47_google_oauth.md).

`tavily_client` still needs a `tool_configurations` entry to pass `api_key`.

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
- **`for_each` deterministic list execution shipped 2026-07-20** — new node `node_type: "for_each"` runs an embedded `target` tool ({ node_type, node_schema? }) once per row of a list, deterministically (iteration happens in Rust via `ListToolExecutor`/`run_list`, not by the LLM re-calling the same tool N times in its own loop). One node, two usage forms: graph node (static `config`) or LLM tool (`tool_configurations` + `node_schema`, `target`/policy fields `fixed`, `items` LLM-visible). List source: `items` (inline array) → `items_from` (`source: "sheet"` in v1, with `column`/`as` selection — `source: "attachment"` deferred to v1.1, a plain `ExecutableNode` can't resolve `document_id → bytes` yet) → default input edge (graph-node path only). Policies `on_error` (continue/abort), `concurrency`, `max_items`. Per-row required-field validation before dispatch (row-level failure, not batch-abort, unless `on_error: "abort"`). Result: `{ output: { total, ok, err, results: [{index, input, status, output|error}] } }`. Two SSE events: `batch-progress` (aggregate snapshot) and `batch-item-finished` (per row) — when `target: subgraph`, child sub-agent events propagate with nested `level`/`path` (reuses the nested-visibility infra). HITL fail-closed (a suspend inside a row becomes that row's error) + self-target guard (`for_each` can't target itself). Config-first/inputs-fallback via `cfg_or_input` (same pattern as `suspend`) lets the same node code serve both usage paths unchanged. E2E-verified live (Gemini 2.5 Flash): graph-node `target: add` (2/2 ok, no LLM), tool `target: http_request` (3/3 ok), tool `target: subgraph` (3/3 ok, per-row sub-agent isolation confirmed). Purely additive — new node, no changes to existing node signatures or public API → ADP unaffected. See [`docs/developer_guide/49_for_each.md`](docs/developer_guide/49_for_each.md), [`docs/superpowers/specs/2026-07-20-deterministic-list-tool-execution-design.md`](docs/superpowers/specs/2026-07-20-deterministic-list-tool-execution-design.md), and CHANGELOG §5.
- **Nested-execution + SSE remediation, unbounded nesting — 2026-08-21** —
  Seven defects in how nested runs report themselves, plus the removal of the
  subgraph nesting limit. **Nesting is now unbounded by default**
  (`MAX_SUBGRAPH_TOOL_DEPTH` const deleted); an opt-in ceiling lives behind
  `COLMENA_MAX_SUBGRAPH_DEPTH=<n>` (unset/`0` = no limit) and its error leads
  with the stable code `SUBGRAPH_DEPTH_EXCEEDED:`. Fixes: (1) a `subgraph`
  dispatched as a tool now emits stream boundary frames — its `__node_id`
  fallback was dead code, since only the graph loop sets that key, so the whole
  branch streamed with no delimiter; new ambient key `__colmena_tool_name`;
  (2) an orchestrator's internal `planner`/`critic`/`phase_reactor` no longer
  split across two nesting levels — their thinking tokens are now wrapped like
  their node-start frames, and a wrapped `ThinkingToken` maps to `thinking-delta`
  instead of `subgraph-text-delta` (which had been rendering internal reasoning
  as the agent's answer); (3) an `llm_call` dispatched as a tool is now a real
  nesting level instead of borrowing its caller's identity; (4) `text_block_ids`
  in `SseMapper` keyed by lineage `path` instead of `node_id`, closing a
  cross-branch block collision; (6) subgraph depth handed to a child in memory
  (`DagRunUseCase::with_seed_state`) instead of riding a Postgres round-trip;
  (7) child events nest UNDER their boundary in `path` rather than beside it —
  synthetic boundaries only, the edge-based path is deliberately excluded; and
  (8, found by the E2E) every `for_each` row now runs under its own
  `<node_id>#<index>` lineage — all rows previously shared one identical `path`,
  which made per-row attribution impossible AND defeated fix (4) outright. New
  shared infra: `DagExecutionEvent::from_node_event` / `wrap_as_child_of` and
  `ChildScopeObserver`. **Verified live**, not just by unit tests:
  [`tests/graphs/advanced/nested_sse_remediation_e2e.json`](tests/graphs/advanced/nested_sse_remediation_e2e.json)
  exercises all of it in one run and
  [`scripts/verify_nested_sse_e2e.py`](scripts/verify_nested_sse_e2e.py) asserts
  each fix against the captured SSE (also in `--ceiling` mode). Wire-visible for
  ADP → one migration note per affected change in
  [`docs/adp_migration/`](docs/adp_migration/README.md).
- **Subgraph/LLM as tools (agents-as-tools) shipped 2026-06-19** — `node_type: "subgraph"` is now valid in `tool_configurations`, so an `llm_call` can expose an existing child graph (`child_graph_path`) or an inline `llm_call` (`child_graph_inline`) as a single tool the LLM chooses to call in its loop. Default input is one `task` string (injected as `{{task}}`); structured via `node_schema`. **Stateless per call** (ephemeral path qualifier derived deterministically from `tool_call_id` — also what lets HITL resume reconstruct the child's scope). **Transparent streaming** (`subgraph-*` child events propagate to the parent stream via threaded observer). **Full HITL**: the sub-agent can suspend; `SUSPENDED` bubbles up through the parent tool loop and resume re-enters the child in the same tool call. Recursion guard `MAX_SUBGRAPH_TOOL_DEPTH = 5` (**removed 2026-08-21** — nesting is now unbounded; see the 2026-08-21 entry above). Bonus: the `suspend` node now resolves `id`/`question`/`options` from `inputs` (helper `cfg_or_input`), enabling `suspend`-as-a-tool. Gotchas confirmed via E2E: with `node_schema` present, `child_graph_path`/`inline` must be a `fixed` field inside `node_schema` (the executor ignores `fixed_config` when `node_schema` is set); a structured child needs an explicit `prompt` templating its vars (the implicit-prompt fallback is the `task` input). Purely additive (new `with_observer`/`with_subgraph_depth` builders; `ExecutableNode::execute` signature unchanged) → ADP unaffected (frontend already renders `subgraph-*`). See [`docs/superpowers/specs/2026-06-18-subgraph-as-tool-design.md`](docs/superpowers/specs/2026-06-18-subgraph-as-tool-design.md), [`docs/developer_guide/19_nested_agents_and_subgraphs.md`](docs/developer_guide/19_nested_agents_and_subgraphs.md), and CHANGELOG §38.
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
- **Sheets write safety shipped 2026-06-07** — `gsheets_run_python` and
  `crdt_doc_run_python` gain a collision policy (`on_existing_sheet`, default
  `fail` → returns structured `SheetExists` error with `current_state` +
  `advice` + `valid_next_moves`; opt back into legacy `auto_suffix` or
  destructive `overwrite` via `fixed_config`) and a new `update_in_place`
  mode in `output_sheets` that diff-writes only changed cells (one
  `batchUpdate` for gsheets, per-cell ops for crdt). New shared modules
  `sheet_collision.rs` (policy + envelope) and `diff_writer.rs` (pure
  records-diff with strict duplicate-key + column-mismatch validations).
  **BREAKING:** legacy `write_to_sheet` arg + `output_sheet` (singular)
  Python global removed from `crdt_doc_run_python` (ADP confirmed clean);
  3 in-repo test graphs migrated. Default collision behavior changed from
  silent `auto_suffix` to `fail` — existing graphs that depended on it must
  set `on_existing_sheet: "auto_suffix"` explicitly. 1388 unit tests pass;
  P1 (collision fail) + P2 (update_in_place dispatch + zero-change safety
  guard) verified live against real Google Sheets. See
  [`docs/superpowers/specs/2026-06-06-sheets-write-safety-design.md`](docs/superpowers/specs/2026-06-06-sheets-write-safety-design.md)
  and CHANGELOG §11.
- **Google Docs integration shipped 2026-06-08 (Subsystem G)** —
  22 synthetic tools (`gdocs_*`) with content-addressed surgical edits,
  multi-tab, markdown import/export with loss detection, co-edit safety
  via Drive Revisions + postgres revision tracking
  (`gdocs_session_state` keyed on `agent_session_id` + `document_id`).
  Toolkit aliases `gdocs` (full 22) and `gdocsread` (read-only 6).
  Auth: SA JSON + ADC; requires
  `COLMENA_GDOCS_DEFAULT_PARENT_FOLDER_ID` (or per-call
  `parent_folder_id`) for creation. ~92 unit tests, 11 integration tests
  (`#[ignore]`-gated). See
  [`docs/developer_guide/45_gdocs.md`](docs/developer_guide/45_gdocs.md)
  and the spec at
  [`docs/superpowers/specs/2026-06-08-google-docs-design.md`](docs/superpowers/specs/2026-06-08-google-docs-design.md).
  v1.1 shipped: surgical table-cell edits (§46, 2026-06-21 — 6 table tools,
  gdocs 22→35), image insertion (§32/§43), and attachment plumbing for
  `gdocs_create_from_docx` / `gdocs_export` (Bundle 1). Still open in v1.1:
  `mode: "suggest"` and markdown tables in inserts (see BACKLOG
  "Subsystem G v1.1"). Purely additive — no breaking changes; ADP
  unaffected unless it opts in via `enabled_tools`.
- **SQL node multi-statement shipped 2026-06-09** — `execute_query` refactored
  to Política C: iterate AST statements one-by-one in a single atomic transaction.
  Fixes 'cannot insert multiple commands into a prepared statement' error
  when LLM writes natural multi-INSERT queries separated by `;\n`. Last statement
  shapes the output (SELECT → rows + LIMIT, mutation → rows_affected sum,
  CREATE TABLE/FUNCTION → created marker). Intermediate SELECTs execute but
  rows discarded. Atomic rollback on any failure. New LLM-facing docs:
  description_supplement always-on with visual anti-patterns + new built-in
  skill `sql-query-best-practices` (opt-in) with 6 references. Bonus: UTF-8
  panic fix in `sql.rs:396`. See CHANGELOG §18 and dev guide §"Multi-statement
  queries".
- **Subsystem G v1.1 paragraph diff shipped 2026-06-09** — co-edit guard
  now returns paragraph-level `before_text`/`after_text` per
  `HumanChange`, partitioned by scope overlap. Cambios fuera del scope
  pasan como `soft_warnings` (no bloquean); cambios dentro del scope
  bloquean con la lista poblada. Adds `last_snapshot_json` + 
  `last_snapshot_size_bytes` to `gdocs_session_state` via migration
  `20260609000000_gdocs_session_state_snapshot.sql`. 1 MB cap configurable
  via `COLMENA_GDOCS_MAX_SNAPSHOT_BYTES`. Instancias sin migración
  detectan ausencia de columnas vía `information_schema` y degradan a v1
  behavior con warn al boot. Diff vía Myers (crate `similar`). ADP debe
  agregar 2 columnas al schema Prisma — ver
  [`ADP_PRISMA_PENDING_TABLES.md`](ADP_PRISMA_PENDING_TABLES.md) §5.
  Spec en
  [`docs/superpowers/specs/2026-06-09-gdocs-paragraph-diff-design.md`](docs/superpowers/specs/2026-06-09-gdocs-paragraph-diff-design.md).
- **Tool-result structured digest (v1.1) shipped 2026-06-19** — resultados de
  tools estructurados (JSON object / array-of-objects / scalar array) ahora se
  compactan como un digest determinista (esquema + N filas + muestra + min/max)
  en vez de prosa NL, vía `llm/application/tool_digest.rs`. Sin LLM, sin cache,
  sin migración, sin cambio de API pública (solo el wire-format del bloque de
  resumen que ve el modelo) → ADP no afectado. Resultados de tools en NL caen al
  resumen semántico v1. Recall sigue lossless. Spec:
  [`docs/superpowers/specs/2026-06-18-conversation-semantic-summary-design.md`](docs/superpowers/specs/2026-06-18-conversation-semantic-summary-design.md);
  plan: [`docs/superpowers/plans/2026-06-19-tool-result-structured-digest-v1-1.md`](docs/superpowers/plans/2026-06-19-tool-result-structured-digest-v1-1.md).
- **`data_run_python` soft-deprecation shipped 2026-07-02** — `data_run_python`
  es ahora el tool tabular **primario**; `gsheets_run_python` y
  `attachment_run_python` quedan **deprecados** (siguen funcionando, se
  mantienen registrados por compatibilidad con grafos persistidos). Cambio
  **aditivo y reversible**: sus descripciones llevan prefijo `DEPRECATED`, las
  11 skills de gsheets ahora instruyen llamar `data_run_python`, y el alias
  `gsheets` lo incluye (`enabled_tools: ["gsheets"]` lo expone; `gsheets_run_python`
  sigue en el alias durante el bridge). Vía alias auto-detecta la capacidad
  gsheets; SQL sigue requiriendo `fixed_config.sql`. El **borrado real** del
  código de los dos tools viejos queda **diferido a una Fase 2 gated**
  (telemetría + verificación de grafos persistidos en ADP). ADP no afectado (sin
  cambio de API pública). Plan:
  [`docs/superpowers/plans/2026-07-02-data-run-python-soft-deprecation.md`](docs/superpowers/plans/2026-07-02-data-run-python-soft-deprecation.md);
  guía: [`docs/developer_guide/48_data_run_python.md`](docs/developer_guide/48_data_run_python.md).
