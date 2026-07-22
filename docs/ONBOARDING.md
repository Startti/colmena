# Onboarding to Colmena

Your starting point depends on what you are trying to do. Pick a path:

| Goal | Path | Estimated time |
|---|---|---|
| Try Colmena once | [Quick start](#path-1--quick-start) | 5 min |
| Use Colmena from Python or Rust | [Library user](#path-2--library-user) | 30 min |
| Author graphs for an existing deployment | [Graph author](#path-3--graph-author) | 1–2 hours |
| Contribute to the engine | [Contributor](#path-4--contributor) | 4–6 hours |
| Understand a specific feature | [Jump-to table](#path-5--jump-to-a-specific-feature) | varies |

---

## Path 1 — Quick start

No API keys required. Runs a minimal two-node graph (webhook trigger + log) using only built-in node types.

```bash
# Build (first run only — subsequent runs use the cache)
cargo build --bin dag_engine

# Run a graph
cargo run --bin dag_engine -- run tests/graphs/basic/trigger.json
```

Expected output (abbreviated):

```
data: {"type":"node-end","node_id":"my_webhook","node_type":"trigger_webhook","output":{"message":"Hello from Simulator!"}}
[LogNode]: {
  "message": "Hello from Simulator!"
}
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

## Path 2 — Library user

You want to USE Colmena from your Python or Rust app, not modify it.

1. **[`docs/developer_guide/00_architecture_overview.md`](./developer_guide/00_architecture_overview.md)** — 10 min
   What Colmena is, how the modules fit together, and cycle of a graph run. Start here.

2. **[`docs/examples/python_usage.md`](./examples/python_usage.md)** — scan key sections (~15 min)
   How to call Colmena from Python via the PyO3 bindings. 900+ lines; focus on the sections relevant to your use case.

3. **[`docs/developer_guide/14_llm_deep_dive.md`](./developer_guide/14_llm_deep_dive.md)** — 15 min
   LLM provider config, streaming, parameters, and tool calling setup. ~580 lines; scan the provider config and streaming sections.

4. **[`docs/developer_guide/15_memory_guide.md`](./developer_guide/15_memory_guide.md)** — 10 min
   Memory persistence options: SQLite (local) and PostgreSQL (production).

---

## Path 3 — Graph author

You write JSON graphs that orchestrate LLMs, tools, SQL, HTTP, etc. You do not modify the engine.

1. **[`docs/developer_guide/00_architecture_overview.md`](./developer_guide/00_architecture_overview.md)** — orientation, 10 min
   Understand what a graph is, what nodes exist, and how execution flows.

2. **[`docs/developer_guide/12_dag_engine_guide.md`](./developer_guide/12_dag_engine_guide.md)** — scan key sections (~30 min)
   Core graph concepts: node wiring, variable resolution, execution lifecycle. 1300+ lines; scan the concepts you need.

3. **[`docs/node_configurations.json`](./node_configurations.json)** — reference as needed
   Canonical config schema for every node type. The ground truth when a field's type or default is unclear.

4. **[`docs/developer_guide/16_data_flow_guide.md`](./developer_guide/16_data_flow_guide.md)** — 20 min
   How data flows between nodes: `$ref`, `$DYNAMIC`, outputs, and transforms.

5. **[`docs/developer_guide/09_tool_calling.md`](./developer_guide/09_tool_calling.md)** — 15 min
   `tool_configurations`, `node_schema`, `fixed_config`, and `enabled_tools`.

6. **[`docs/developer_guide/22_tool_execution_flow.md`](./developer_guide/22_tool_execution_flow.md)** — 15 min
   Lifecycle of a tool call: `node_schema` → merge → execution → response back to LLM.

7. **Pick the node guides for the nodes you will use:**

   | Node | Guide |
   |---|---|
   | LLM | [`docs/developer_guide/14_llm_deep_dive.md`](./developer_guide/14_llm_deep_dive.md) |
   | SQL | [`docs/developer_guide/23_sql_node.md`](./developer_guide/23_sql_node.md) |
   | HTTP / web | [`docs/developer_guide/25_web_nodes.md`](./developer_guide/25_web_nodes.md) |
   | Python script | [`docs/developer_guide/26_python_node.md`](./developer_guide/26_python_node.md) |
   | Documents (Word/Excel) | [`docs/developer_guide/27_documents_library.md`](./developer_guide/27_documents_library.md) |
   | Socket.IO | [`docs/developer_guide/21_socketio_node.md`](./developer_guide/21_socketio_node.md) |
   | Multimedia (images / TTS) | [`docs/developer_guide/32_multimedia_generation.md`](./developer_guide/32_multimedia_generation.md) |

8. **If you need skills or lazy tool loading:**
   - [`docs/developer_guide/24_skills.md`](./developer_guide/24_skills.md) — skills, layered tool context, and scoped skills
   - [`docs/developer_guide/29_lazy_tool_loading.md`](./developer_guide/29_lazy_tool_loading.md) — progressive `describe_tool` reveal

9. **If you need agent-session continuity (memory, HITL, secrets):**
   - [`docs/developer_guide/15_memory_guide.md`](./developer_guide/15_memory_guide.md) — conversational memory across sessions
   - [`docs/developer_guide/13_security_strategy.md`](./developer_guide/13_security_strategy.md) — `secure_suspend` for collecting secrets; pgcrypto (`pgp_sym_encrypt`) at rest

10. **Read 3–4 test graphs** that match your use case from [`tests/graphs/`](../tests/graphs/):
    - `tests/graphs/basic/` — simple nodes (trigger, math, log, suspend)
    - `tests/graphs/agents/` — LLM call, tool calling, streaming, extraction
    - `tests/graphs/advanced/` — orchestrators, planners, multi-step agents
    - `tests/graphs/memory/` — SQLite and PostgreSQL persistence
    - `tests/graphs/external/` — HTTP requests

---

## Path 4 — Contributor

You want to modify the engine, add a node type, or fix a bug.

Complete Paths 1–3 first — otherwise the architecture references below will not make sense.

1. **[`docs/developer_guide/01_architecture.md`](./developer_guide/01_architecture.md)** — 15 min
   Hexagonal layers in depth: domain, application, infrastructure, and the rules between them.

2. **[`docs/developer_guide/03_coding_conventions.md`](./developer_guide/03_coding_conventions.md)** — 10 min
   Naming, error handling, async patterns, and doc comment standards.

3. **[`docs/developer_guide/05_testing.md`](./developer_guide/05_testing.md)** — 15 min
   Testing strategy, mocking with `mockall`, `#[ignore]` conventions, and CI vs local commands.

4. **[`docs/developer_guide/02_environment_setup.md`](./developer_guide/02_environment_setup.md)** — 10 min
   Local dev environment: toolchain, feature flags, Python/Node bindings setup.

5. **[`docs/dds/ARQUITECTURA_HEXAGONAL_GUIA.md`](./dds/ARQUITECTURA_HEXAGONAL_GUIA.md)** — 20 min
   The original design intent behind the hexagonal architecture and port/adapter boundaries.

6. **Pick the design document for your area:**

   | Area | Design doc |
   |---|---|
   | LLM module | [`docs/developer_guide/14_llm_deep_dive.md`](./developer_guide/14_llm_deep_dive.md) |
   | DAG engine | [`docs/dds/DAG_ENGINE_DISEÑO.md`](./dds/DAG_ENGINE_DISEÑO.md) |
   | Agents and tools | [`docs/developer_guide/09_tool_calling.md`](./developer_guide/09_tool_calling.md) |
   | Security / secrets | [`docs/dds/SECURE_VALUES_DISEÑO.md`](./dds/SECURE_VALUES_DISEÑO.md) |

7. **[`docs/CHANGELOG_2026-05.md`](./CHANGELOG_2026-05.md)** — scan recent entries
   What has been changing recently: features shipped, breaking changes, and migration notes.

8. **[`CLAUDE.md`](../CLAUDE.md)** — skim the conventions sections
   Repo conventions used for AI-assisted contributions; also a useful "current style" reference for humans. Pay attention to the architecture rules, testing conventions, and the `node_schema+fixed` vs `fixed_config` table.

9. **[`docs/CODEBASE_TOUR.md`](./CODEBASE_TOUR.md)** — module-by-module walkthrough (~30 min)
   Hand-rail of the repo's directory structure, key types per module, and how modules connect. Use it after the architecture overview if you want a guided tour before diving into a specific subsystem.

---

## Path 5 — Jump to a specific feature

| Topic | Start here |
|---|---|
| Skills / `load_skill` | [`docs/developer_guide/24_skills.md`](./developer_guide/24_skills.md) |
| Layered tool context (policy + guide + scoped skills) | [`docs/developer_guide/24_skills.md`](./developer_guide/24_skills.md) § Visual reference |
| Lazy tool loading / `describe_tool` | [`docs/developer_guide/29_lazy_tool_loading.md`](./developer_guide/29_lazy_tool_loading.md) |
| SQL node | [`docs/developer_guide/23_sql_node.md`](./developer_guide/23_sql_node.md) |
| Multimedia (image generation, TTS) | [`docs/developer_guide/32_multimedia_generation.md`](./developer_guide/32_multimedia_generation.md) |
| Attachments / `load_attachment` | [`docs/developer_guide/31_load_attachment.md`](./developer_guide/31_load_attachment.md) |
| Attachment GC | [`docs/developer_guide/36_attachment_gc.md`](./developer_guide/36_attachment_gc.md) |
| Orchestrator / multi-phase agents | [`docs/developer_guide/20_orchestrator_architecture.md`](./developer_guide/20_orchestrator_architecture.md) |
| Subgraphs / HITL | [`docs/developer_guide/19_nested_agents_and_subgraphs.md`](./developer_guide/19_nested_agents_and_subgraphs.md) |
| Memory + Postgres schema | [`docs/developer_guide/15_memory_guide.md`](./developer_guide/15_memory_guide.md) + [`docs/developer_guide/30_database_schema.md`](./developer_guide/30_database_schema.md) |
| Security / secrets | [`docs/developer_guide/13_security_strategy.md`](./developer_guide/13_security_strategy.md) |
| Temporal & geographic context | [`docs/developer_guide/35_temporal_geographic_context.md`](./developer_guide/35_temporal_geographic_context.md) |
| SSE events | [`docs/sse_events_reference.md`](./sse_events_reference.md) |
| Troubleshooting | [`docs/developer_guide/18_troubleshooting.md`](./developer_guide/18_troubleshooting.md) |

---

## Keeping this doc current

When a new feature ships, add it to:
- The relevant path above (Library user / Graph author / Contributor), with a one-line description and an honest time estimate.
- The Path 5 jump-to table.

For the full feature changelog see [`docs/CHANGELOG_2026-05.md`](./CHANGELOG_2026-05.md) and `docs/superpowers/{specs,plans}/`.
