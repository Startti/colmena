# Graph Builder Agent — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a Colmena graph whose conversational `llm_call` agent helps non-technical people design and generate *other* Colmena graphs, talking only in plain-language capabilities (never node names), and which executes the draft graph for real to self-judge before delivering it.

**Architecture:** A single `llm_call` node (Option A from the spec) running in `serve` mode with conversational memory (`session_id` + `connection_url`). Knowledge is hybrid: a lean `system_message` plus a folder of `SKILL.md` files loaded on-demand via `skills.paths` + the `load_skill` tool. The agent tests its draft by calling a `probar_grafo` tool — a `subgraph` node receiving the draft as a **dynamic** `child_graph_inline` object — and judges the real result.

**Tech Stack:** Colmena DAG engine (`cargo run --bin dag_engine`), JSON graph files, Markdown SKILL.md files, `google/gemini-2.5-flash`, Postgres (`DATABASE_URL`) for memory. No Rust changes expected (gated by Task 1).

**Spec:** [docs/superpowers/specs/2026-06-25-graph-builder-agent-design.md](../specs/2026-06-25-graph-builder-agent-design.md)

**Conventions reminders (from CLAUDE.md / memories):**
- Conventional Commits only (`feat`/`fix`/`docs`/`test`/`chore`/…). Never `plan`/`spec`.
- Source `.env` before any graph run that hits providers: `set -a; source .env; set +a`.
- Save every E2E run's SSE to `/tmp/colmena_e2e/<name>.sse` and present a friendly report.
- Use real nodes/tools, never `log` as a mock backing for a tool.
- Default stack: `provider:"google"`, `model:"gemini-2.5-flash"`, `api_key:"${GEMINI_API_KEY}"`.

---

## File Structure

All new files live under `tests/graphs/agents/graph_builder/`:

- `graph_builder.json` — the meta-graph (trigger_webhook → llm_call → output).
- `skills/building-graphs-core/SKILL.md` — graph anatomy, edges/ports, patterns, gotchas.
- `skills/capability-ai-text/SKILL.md` — `llm_call`.
- `skills/capability-web-and-apis/SKILL.md` — `tavily_client`, `http_request`, `api_explorer`.
- `skills/capability-ask-user/SKILL.md` — `suspend`.
- `skills/capability-multimedia/SKILL.md` — `image_generation`, `image_edit`, `tts`.
- `skills/capability-docs-and-sheets/SKILL.md` — `gsheets_*`, `gdocs_*`.
- `skills/capability-data-sql/SKILL.md` — `sql_query`.
- `skills/capability-code-and-logic/SKILL.md` — `python_script`, `router`.
- `README.md` — how to serve it and talk to it.

Verification-only (temporary) file:
- `tests/graphs/agents/graph_builder/_feasibility_subgraph_inline.json` — Task 1 probe.

---

## Task 1: Verify dynamic `child_graph_inline` works as a tool (FEASIBILITY GATE)

The spec's central risk: a CLAUDE.md note says subgraph-as-tool needs `child_graph_path`/`inline` as a **fixed** field, but code reads `inputs.get("child_graph_inline")`. This task proves an LLM-supplied (non-fixed) inline graph executes. **Everything downstream depends on this.**

**Files:**
- Create: `tests/graphs/agents/graph_builder/_feasibility_subgraph_inline.json`

- [ ] **Step 1: Write the probe graph**

A single `llm_call` whose only tool is `probar_grafo` (a `subgraph` with a **non-fixed** `child_graph_inline` object field). The system message forces the model to call the tool with a trivial graph that echoes a constant, so success is unambiguous.

```json
{
  "nodes": {
    "trigger": {
      "type": "trigger_webhook",
      "config": { "path": "/chat", "test_payload": { "message": "Probá un grafo que devuelva exactamente el texto OK_123. Usá la herramienta probar_grafo." } }
    },
    "agent": {
      "type": "llm_call",
      "config": {
        "provider": "google",
        "model": "gemini-2.5-flash",
        "api_key": "${GEMINI_API_KEY}",
        "system_message": "Cuando el usuario te lo pida, construí un grafo Colmena mínimo y ejecutalo con la herramienta probar_grafo. Para devolver un texto constante usá un nodo input con data y un nodo output. Estructura: {\"nodes\":{\"in\":{\"type\":\"input\",\"config\":{\"data\":{\"output\":\"OK_123\"}}},\"out\":{\"type\":\"output\"}},\"edges\":[{\"from\":\"in\",\"to\":\"out\"}]}. Pasá ese objeto completo en el argumento child_graph_inline. Después contá qué devolvió.",
        "tool_configurations": {
          "probar_grafo": {
            "name": "probar_grafo",
            "node_type": "subgraph",
            "description": "Ejecuta un grafo Colmena completo y devuelve su resultado real. Pasá el grafo entero (objeto con nodes y edges) en child_graph_inline.",
            "node_schema": {
              "child_graph_inline": {
                "type": "object",
                "required": true,
                "description": "El grafo Colmena completo a ejecutar: un objeto con las claves nodes y edges."
              }
            }
          }
        }
      }
    },
    "out": { "type": "output" }
  },
  "edges": [
    { "from": "trigger.message", "to": "agent.prompt" },
    { "from": "agent", "to": "out" }
  ]
}
```

- [ ] **Step 2: Run it for real**

```bash
mkdir -p /tmp/colmena_e2e
set -a; source .env; set +a
cargo run --bin dag_engine -- run tests/graphs/agents/graph_builder/_feasibility_subgraph_inline.json \
  --agent-session-id gb_feas_001 --include-extra-info 2>&1 | tee /tmp/colmena_e2e/gb_feasibility.sse
```

Expected: the run completes; the tool-call trace shows `probar_grafo` executed; the subgraph returned `OK_123`; the final agent message references `OK_123`.

- [ ] **Step 3: Decision point**

- **If `OK_123` flows back through the tool result:** dynamic inline works. Delete the probe file, mark Task 1 done, proceed.
  ```bash
  rm tests/graphs/agents/graph_builder/_feasibility_subgraph_inline.json
  ```
- **If the subgraph errors with "requires child_graph_inline or child_graph_path"** (i.e. the non-fixed field did NOT reach `inputs`): STOP. Record the exact error in the run log. Two fallbacks, in order of preference:
  1. Read `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs` around the `node_schema` merge (the research cited lines ~1682–1851) and confirm whether a top-level non-fixed object field is placed into `inputs`. If a one-line fix makes it flow, write it under the `superpowers:rust_dev` skill with a unit test, then re-run Step 2.
  2. If a Rust fix is out of scope, escalate to the user with the captured error before continuing — the whole "test it for real" feature depends on this.

- [ ] **Step 4: Commit the decision record (no probe file)**

If feasible, nothing to commit yet (probe deleted). If a Rust fix was needed, commit it:
```bash
git add -A && git commit -m "fix(subgraph): allow dynamic child_graph_inline as a non-fixed tool field"
```

---

## Task 2: Scaffold the meta-graph skeleton (serves + replies)

Stand up the conversational shell with a placeholder system message and the `probar_grafo` tool, no skills yet. Prove it serves and holds a conversation with memory.

**Files:**
- Create: `tests/graphs/agents/graph_builder/graph_builder.json`

- [ ] **Step 1: Read the canonical memory + serve example to copy exact wiring**

Read `tests/graphs/memory/memory_postgres_example.json` to copy the exact `session_id` + `connection_url` shape and the trigger→llm edge wiring verbatim.

- [ ] **Step 2: Write the skeleton graph**

```json
{
  "timezone": "America/Bogota",
  "locale": "es-CO",
  "nodes": {
    "trigger": {
      "type": "trigger_webhook",
      "config": { "path": "/chat", "test_payload": { "message": "hola" } }
    },
    "agent": {
      "type": "llm_call",
      "config": {
        "provider": "google",
        "model": "gemini-2.5-flash",
        "api_key": "${GEMINI_API_KEY}",
        "session_id": "graph_builder_session_001",
        "connection_url": "${DATABASE_URL}",
        "system_message": "PLACEHOLDER — reemplazado en Task 11. Por ahora: sos un asistente que ayuda a crear grafos. Saludá y preguntá qué quiere lograr la persona.",
        "skills": { "paths": [] },
        "tool_configurations": {
          "probar_grafo": {
            "name": "probar_grafo",
            "node_type": "subgraph",
            "description": "Ejecuta un grafo Colmena completo y devuelve su resultado real para que puedas juzgar si funciona. Pasá el grafo entero (objeto con nodes y edges) en child_graph_inline.",
            "node_schema": {
              "child_graph_inline": {
                "type": "object",
                "required": true,
                "description": "El grafo Colmena completo a ejecutar: un objeto con las claves nodes y edges."
              }
            }
          }
        }
      }
    },
    "out": { "type": "output" }
  },
  "edges": [
    { "from": "trigger.message", "to": "agent.prompt" },
    { "from": "agent", "to": "out" }
  ]
}
```

- [ ] **Step 3: Validate the graph parses (no run)**

```bash
cargo run --bin dag_engine -- run tests/graphs/agents/graph_builder/graph_builder.json --agent-session-id gb_smoke_001 2>&1 | tee /tmp/colmena_e2e/gb_skeleton.sse
```
Expected: the agent produces a Spanish greeting asking what the person wants to build. No parse/validation error.

- [ ] **Step 4: Commit**

```bash
git add tests/graphs/agents/graph_builder/graph_builder.json
git commit -m "feat(graph_builder): scaffold conversational meta-graph skeleton"
```

---

## Task 3: Author `building-graphs-core` skill

The foundational reference: exact graph anatomy, edges/ports, common patterns, gotchas. The agent loads this whenever it starts assembling a graph.

**Files:**
- Create: `tests/graphs/agents/graph_builder/skills/building-graphs-core/SKILL.md`

- [ ] **Step 1: Write the SKILL.md (frontmatter verbatim, body complete)**

Frontmatter MUST be exactly:
```markdown
---
name: building-graphs-core
description: Use when assembling ANY Colmena graph JSON — the structural rules. Covers the nodes/edges anatomy, default vs explicit ports, ${ENV} for keys, entry (trigger_webhook/input) and output nodes, and the most common wiring gotchas.
---
```

Body MUST document, with at least one verbatim example each (copy real shapes — confirmed field names):
1. **Top-level structure**: `{ "nodes": {<id>: {"type": ..., "config": {...}}}, "edges": [{"from","to"}], "timezone"?, "locale"? }`. `config` required even if `{}`.
2. **Edges & ports**: `from`/`to` can be a bare node id (default ports) or `nodo.campo` (explicit port). Show `{"from":"trigger.message","to":"agent.prompt"}` and `{"from":"agent","to":"out"}`.
3. **Entry nodes**: `trigger_webhook` (`config.path`, `config.test_payload`) for served/event graphs; `input` (`config.data`) for static values. Every graph needs one entry and should end in an `output` node.
4. **Secrets**: never inline keys — use `${OPENAI_API_KEY}`, `${GEMINI_API_KEY}`, `${ANTHROPIC_API_KEY}`, `${DATABASE_URL}`.
5. **Default LLM stack**: `provider:"google"`, `model:"gemini-2.5-flash"`, `api_key:"${GEMINI_API_KEY}"`.
6. **Three canonical skeletons** (copy verbatim, runnable): (a) single `llm_call` Q&A, (b) `llm_call` with a `tool_configurations` tool, (c) `trigger_webhook → llm_call → output`.
7. **Gotchas**: `config` must exist; edge endpoints must reference existing node ids; a tool's `node_type` must be a real registered type; fields meant to be hidden from the LLM go in `node_schema` with `"fixed"`.

- [ ] **Step 2: Commit**

```bash
git add tests/graphs/agents/graph_builder/skills/building-graphs-core/
git commit -m "docs(graph_builder): add building-graphs-core skill"
```

---

## Task 4: Author `capability-ai-text` skill

**Files:**
- Create: `tests/graphs/agents/graph_builder/skills/capability-ai-text/SKILL.md`

- [ ] **Step 1: Write the SKILL.md**

Frontmatter verbatim:
```markdown
---
name: capability-ai-text
description: Use when the user wants an AI to answer, write, summarize, classify or transform text. Covers the llm_call node — provider/model/api_key, system_message, and giving it tools.
---
```
Body MUST document the `llm_call` config fields (`provider`, `api_key`, `model`, `system_message`) and how to attach tools via `tool_configurations`. Include a verbatim runnable example of a single-`llm_call` "responde preguntas" graph using the default Gemini stack. State the default stack. Reference `building-graphs-core` with `[[building-graphs-core]]` for the wiring.

- [ ] **Step 2: Commit**

```bash
git add tests/graphs/agents/graph_builder/skills/capability-ai-text/
git commit -m "docs(graph_builder): add capability-ai-text skill"
```

---

## Task 5: Author `capability-web-and-apis` skill

**Files:**
- Create: `tests/graphs/agents/graph_builder/skills/capability-web-and-apis/SKILL.md`

- [ ] **Step 1: Write the SKILL.md**

Frontmatter verbatim:
```markdown
---
name: capability-web-and-apis
description: Use when the user wants to search the internet or pull data from an external service/API. Covers tavily_client (web search), http_request (calling any API), and api_explorer (discovering endpoints of a known API).
---
```
Body MUST:
- Document `tavily_client` as an LLM tool (sub-tools `search`, `fetch`) — copy the `tool_configurations` shape including the required `api_key` from `docs/developer_guide/25_web_nodes.md` or a real graph under `tests/graphs/external/`.
- Document `http_request` as both a node and a tool. Copy the exact `node_schema` tool example **verbatim** from `tests/graphs/agents/http_tool_node_schema_test.json` (the `create_blog_post` block) showing `base_url`/`endpoint`/`method` as `"fixed"` and `title`/`content` as `required`.
- Document `api_explorer` flag-only activation (`"enabled_tools": ["api_explorer"]`) and its 5 sub-tools, per `docs/developer_guide/25_web_nodes.md`.
- **Side-effect warning**: any `method` of POST/PUT/PATCH/DELETE mutates remote state → the agent must warn/confirm before test-running (cross-reference the system_message rule).

- [ ] **Step 2: Commit**

```bash
git add tests/graphs/agents/graph_builder/skills/capability-web-and-apis/
git commit -m "docs(graph_builder): add capability-web-and-apis skill"
```

---

## Task 6: Author `capability-ask-user` skill

**Files:**
- Create: `tests/graphs/agents/graph_builder/skills/capability-ask-user/SKILL.md`

- [ ] **Step 1: Write the SKILL.md**

Frontmatter verbatim:
```markdown
---
name: capability-ask-user
description: Use when the built graph needs to pause and ask the end-user for a value or a decision before continuing (human-in-the-loop). Covers the suspend node.
---
```
Body MUST document the `suspend` node config (`id` required 1–64 chars, `question`, `question_type` `"open"`/`"choice"`, `options`) and its default I/O (`question` → `answer_received`), copied from `docs/developer_guide/44_suspend_node.md`. Include one verbatim runnable example of a graph that asks a choice question and continues. Note that `options` is only a UX hint.

- [ ] **Step 2: Commit**

```bash
git add tests/graphs/agents/graph_builder/skills/capability-ask-user/
git commit -m "docs(graph_builder): add capability-ask-user skill"
```

---

## Task 7: Author `capability-multimedia` skill

**Files:**
- Create: `tests/graphs/agents/graph_builder/skills/capability-multimedia/SKILL.md`

- [ ] **Step 1: Write the SKILL.md**

Frontmatter verbatim:
```markdown
---
name: capability-multimedia
description: Use when the user wants to create or edit an image, or turn text into spoken audio/voice. Covers image_generation, image_edit and tts.
---
```
Body MUST document, copying exact config fields from `docs/developer_guide/32_multimedia_generation.md` and `docs/node_configurations.json`:
- `image_generation` (`provider`, `api_key`, `model`, `prompt`) — output `{ images: [{document_id, ...}] }`.
- `image_edit` (`provider:"openai"`, `api_key`, `source_url` incl. `$attachment:<id>`, `prompt`, optional `mask_url`).
- `tts` (`provider`, `api_key`, `model`, `text`, `voice`) — output `{ audio: {document_id, ...} }`.
Include at least one verbatim runnable example (e.g. text → image). Note these nodes only register when a storage adapter is present (`COLMENA_LOCAL`), so the agent should mention the deployment caveat when relevant.

- [ ] **Step 2: Commit**

```bash
git add tests/graphs/agents/graph_builder/skills/capability-multimedia/
git commit -m "docs(graph_builder): add capability-multimedia skill"
```

---

## Task 8: Author `capability-docs-and-sheets` skill

**Files:**
- Create: `tests/graphs/agents/graph_builder/skills/capability-docs-and-sheets/SKILL.md`

- [ ] **Step 1: Write the SKILL.md**

Frontmatter verbatim:
```markdown
---
name: capability-docs-and-sheets
description: Use when the user talks about spreadsheets or documents — including lay terms like "Excel", "planilla", "hoja", "tabla", "Word", "documento". Covers the gsheets and gdocs toolkits and the online-vs-downloadable disambiguation.
---
```
Body MUST:
- Explain that these are **toolkit flags** activated via `enabled_tools` (`["gsheets"]`, `["gdocs"]`, `["gdocsread"]`) — no per-node config needed (copy the rule from `docs/developer_guide/40_toolkit_packages.md` / CLAUDE.md). Show `!sub_tool` exclusion syntax (e.g. `["gsheets","!gsheets_delete_sheet"]`).
- List the key `gsheets_*` tools (create, read, set_cell/range, format_range, share, run_python, create_from_xlsx, export_xlsx) and key `gdocs_*` tools (create, create_from_markdown, read_as_markdown, replace_text, insert_after_text, append_markdown, table tools), referencing `docs/developer_guide/41_builtin_tools_index.md`.
- **The "Excel" disambiguation** (the user's explicit requirement): "Excel/planilla/hoja" can mean an *online editable Google Sheet* (default gsheets) OR a *downloadable .xlsx file* (`gsheets_create_from_xlsx` / `gsheets_export_xlsx`). Instruct the agent to ask in plain language ("¿editable en línea o un archivo para descargar?") and map accordingly. Same for "Word/documento" → gdocs.
- Note OAuth/env dependency (`COLMENA_GOOGLE_OAUTH_*`) per CLAUDE.md, so the agent mentions setup when relevant.

- [ ] **Step 2: Commit**

```bash
git add tests/graphs/agents/graph_builder/skills/capability-docs-and-sheets/
git commit -m "docs(graph_builder): add capability-docs-and-sheets skill"
```

---

## Task 9: Author `capability-data-sql` skill

**Files:**
- Create: `tests/graphs/agents/graph_builder/skills/capability-data-sql/SKILL.md`

- [ ] **Step 1: Write the SKILL.md**

Frontmatter verbatim:
```markdown
---
name: capability-data-sql
description: Use when the user wants to read from or save into a database (lay terms like "base de datos", "guardar registros", "consultar datos"). Covers the sql_query node, permission presets and the side-effects of writes.
---
```
Body MUST document `sql_query` config (`query`, `preset` e.g. `select`/`read_write_delete`, optional `critic`) and as-a-tool usage, copying exact shapes from `docs/developer_guide/23_sql_node.md`. Include one verbatim read-only example. **Side-effect warning**: INSERT/UPDATE/DELETE mutate data → warn/confirm before test-running; prefer read-only or a disposable test schema. Cross-reference `building-graphs-core`.

- [ ] **Step 2: Commit**

```bash
git add tests/graphs/agents/graph_builder/skills/capability-data-sql/
git commit -m "docs(graph_builder): add capability-data-sql skill"
```

---

## Task 10: Author `capability-code-and-logic` skill

**Files:**
- Create: `tests/graphs/agents/graph_builder/skills/capability-code-and-logic/SKILL.md`

- [ ] **Step 1: Write the SKILL.md**

Frontmatter verbatim:
```markdown
---
name: capability-code-and-logic
description: Use when the user needs a custom calculation/data transformation, or wants the flow to branch down different paths depending on the case. Covers python_script and router.
---
```
Body MUST:
- Document `python_script` (`code`; injected inputs become Python variables; the `output` variable is the result; sandbox `restricted` default whitelist), copying field facts from `docs/developer_guide/26_python_node.md`. Include one verbatim example (e.g. transform a number).
- Document `router` (`mode:"llm_direct"` vs `"extract_and_route"`, `branches`, schema for mode B) from `docs/developer_guide/37_router_and_output_parser.md`. Include one verbatim branching example.

- [ ] **Step 2: Commit**

```bash
git add tests/graphs/agents/graph_builder/skills/capability-code-and-logic/
git commit -m "docs(graph_builder): add capability-code-and-logic skill"
```

---

## Task 11: Write the system_message and wire skills + paths into the meta-graph

This is the brain. Replace the placeholder `system_message` and populate `skills.paths` with all eight skill directories.

**Files:**
- Modify: `tests/graphs/agents/graph_builder/graph_builder.json` (the `agent` node's `config.system_message` and `config.skills.paths`)

- [ ] **Step 1: Set `skills.paths` to all eight skill dirs**

```json
"skills": {
  "paths": [
    "tests/graphs/agents/graph_builder/skills/building-graphs-core",
    "tests/graphs/agents/graph_builder/skills/capability-ai-text",
    "tests/graphs/agents/graph_builder/skills/capability-web-and-apis",
    "tests/graphs/agents/graph_builder/skills/capability-ask-user",
    "tests/graphs/agents/graph_builder/skills/capability-multimedia",
    "tests/graphs/agents/graph_builder/skills/capability-docs-and-sheets",
    "tests/graphs/agents/graph_builder/skills/capability-data-sql",
    "tests/graphs/agents/graph_builder/skills/capability-code-and-logic"
  ]
}
```

- [ ] **Step 2: Replace `system_message` with the full text below (verbatim)**

Embed this as the `system_message` JSON string (escape newlines as needed, or keep as one JSON string). Content:

```text
Sos un asistente experto que ayuda a personas SIN conocimientos de programación a crear "grafos" de Colmena (flujos automatizados). Hablás español, claro y amable.

REGLA DE ORO: nunca uses jerga técnica con la persona. Nunca nombres nodos, puertos, edges, JSON ni node_types. Hablás SIEMPRE en términos de CAPACIDADES y de lo que la persona quiere lograr.

QUÉ PODÉS CONSTRUIR (capacidades disponibles):
- Que una IA responda, escriba, resuma o transforme texto.
- Buscar información en internet.
- Traer datos de un servicio o aplicación externa (una API).
- Pausar para pedirle a la persona un dato o una decisión.
- Crear o editar una imagen.
- Generar audio o voz a partir de texto.
- Trabajar con hojas de cálculo o documentos.
- Consultar o guardar datos en una base de datos.
- Hacer un cálculo o transformación de datos a medida.
- Decidir un camino distinto según el caso.

VOCABULARIO COLOQUIAL → capacidad (entendé la jerga, no la corrijas):
- "Excel", "planilla", "hoja", "tabla", "spreadsheet" → hoja de cálculo.
- "Word", "documento", "doc" → documento.
- "base de datos", "guardar registros", "consultar datos", "un sistema donde guardar" → base de datos.
- "mandar a un sistema", "conectar con tal app", "que llame a tal servicio" → API externa.
- "buscar en Google", "buscar en internet", "investigar" → buscar en internet.
- "chatbot", "que conteste", "que redacte", "que resuma" → IA de texto.
- "foto", "imagen", "dibujo", "logo" → crear imagen.
- "audio", "voz", "que lo lea en voz alta", "podcast" → generar voz.

MÉTODO DE ENTREVISTA:
1. Entendé primero el OBJETIVO (qué problema querés resolver), no la solución técnica. Preguntá por el para qué.
2. Una sola pregunta a la vez. Lenguaje simple, con ejemplos concretos cuando ayude.
3. Mapeá la jerga a una capacidad. Si un término es ambiguo, desambiguá en lenguaje simple. Ejemplo clave: si dicen "Excel", preguntá "¿la necesitás editable en línea, o como un archivo para descargar?". Si dicen "documento", aclarará si es para editar en línea o exportar.
4. Cuando ya entendiste, PROPONÉ EL FLUJO EN PALABRAS y pedí confirmación antes de armar nada. Ejemplo: "Entonces: entra tu pregunta → la IA busca en internet → te devuelve un resumen. ¿Es eso lo que querés?".
5. Recién con el OK, armá el grafo, probálo en silencio y corregilo hasta que funcione. No muestres el detalle técnico durante este proceso.
6. Nunca le pidas a la persona detalles técnicos. Si falta algo técnico, inferí un valor sensato o usá los valores por defecto.

CÓMO TRABAJÁS POR DENTRO (no se lo cuentes a la persona):
- Cuando vayas a armar un grafo, primero cargá la skill "building-graphs-core" y la skill de la capacidad que aplique (capability-ai-text, capability-web-and-apis, capability-ask-user, capability-multimedia, capability-docs-and-sheets, capability-data-sql, capability-code-and-logic). Usá load_skill.
- Construí el grafo siguiendo esas skills. Usá el stack por defecto (Gemini) salvo que haga falta otra cosa.
- PROBALO DE VERDAD con la herramienta probar_grafo: pasale el grafo completo y mirá el resultado real. Si falla, leé el error, corregí y volvé a probar. No entregues un grafo que no probaste exitosamente.
- Para probar, horneá valores de prueba en el punto de entrada del grafo (no dependas de inputs externos).

SEGURIDAD AL PROBAR (importante):
- Si el grafo tiene efectos reales (mandar datos a una API con POST/PUT/PATCH/DELETE, escribir/borrar en una base de datos, enviar mensajes), AVISÁ a la persona antes de ejecutarlo y, si seguís, usá datos de prueba inocuos. Los grafos que solo leen o solo usan IA podés probarlos sin pedir permiso.

ENTREGA FINAL:
- Cuando el grafo funciona, entregalo en el chat dentro de un bloque de código, y agregá un resumen en lenguaje simple de QUÉ HACE y CÓMO USARLO (cómo correrlo). Nada de jerga en el resumen.
```

- [ ] **Step 3: Re-run the skeleton smoke test (still parses, greets)**

```bash
set -a; source .env; set +a
cargo run --bin dag_engine -- run tests/graphs/agents/graph_builder/graph_builder.json --agent-session-id gb_sys_001 2>&1 | tee /tmp/colmena_e2e/gb_systemmsg.sse
```
Expected: a friendly Spanish greeting that asks what the person wants to achieve — no node names, no JSON.

- [ ] **Step 4: Commit**

```bash
git add tests/graphs/agents/graph_builder/graph_builder.json
git commit -m "feat(graph_builder): add interview system_message and wire on-demand skills"
```

---

## Task 12: E2E — full conversation, simple AI bot

Prove the end-to-end loop: interview → propose-in-words → build → test → deliver, in `serve` mode.

- [ ] **Step 1: Serve the graph**

```bash
set -a; source .env; set +a
cargo run --bin dag_engine -- serve tests/graphs/agents/graph_builder/graph_builder.json
```

- [ ] **Step 2: Hold the conversation (separate terminal)**

```bash
curl -s -X POST http://localhost:3000/chat -H "Content-Type: application/json" \
  -d '{"message":"quiero algo que conteste preguntas de mis clientes"}' | tee -a /tmp/colmena_e2e/gb_convo_bot.sse
# follow up based on its question, e.g.:
curl -s -X POST http://localhost:3000/chat -H "Content-Type: application/json" \
  -d '{"message":"si, que conteste en español y sea amable"}' | tee -a /tmp/colmena_e2e/gb_convo_bot.sse
curl -s -X POST http://localhost:3000/chat -H "Content-Type: application/json" \
  -d '{"message":"si, dale, armámelo"}' | tee -a /tmp/colmena_e2e/gb_convo_bot.sse
```

Expected behavior to verify in the SSE:
- The agent asks about the goal in plain language (no node names).
- It proposes the flow in words and asks for confirmation.
- It internally calls `load_skill` (building-graphs-core + capability-ai-text) and `probar_grafo`.
- The `probar_grafo` run returns a real LLM answer.
- The final message contains a graph JSON in a code block + a plain-language summary.

- [ ] **Step 3: Validate the delivered graph actually runs**

Save the delivered JSON to `/tmp/colmena_e2e/gb_delivered_bot.json` and run it:
```bash
cargo run --bin dag_engine -- run /tmp/colmena_e2e/gb_delivered_bot.json --agent-session-id gb_delivered_001 2>&1 | tee /tmp/colmena_e2e/gb_delivered_bot.sse
```
Expected: it runs without validation/parse errors and produces an answer.

- [ ] **Step 4: Present the friendly E2E report** (input, what it asked, skills loaded, test result, delivered graph, tokens). Do not paste the whole SSE.

- [ ] **Step 5: Commit any prompt/skill fixes found during E2E**

```bash
git add -A && git commit -m "fix(graph_builder): refine prompt/skills from bot E2E"
```

---

## Task 13: E2E — "Excel" disambiguation scenario

Prove the vocabulary mapping + disambiguation requirement the user called out explicitly.

- [ ] **Step 1: Converse (graph still served from Task 12, or re-serve)**

```bash
curl -s -X POST http://localhost:3000/chat -H "Content-Type: application/json" \
  -d '{"message":"necesito pasar unos datos a un Excel automáticamente"}' | tee -a /tmp/colmena_e2e/gb_convo_excel.sse
```
Expected: the agent recognizes "Excel" as a spreadsheet capability and **asks the online-vs-downloadable disambiguation in plain language** (no mention of gsheets/xlsx tool names). Continue the conversation and verify it builds with the correct toolkit (`gsheets` vs `gsheets_create_from_xlsx`/`export_xlsx`) based on the answer.

- [ ] **Step 2: Present the friendly report and commit any fixes**

```bash
git add -A && git commit -m "test(graph_builder): verify Excel disambiguation E2E"
```

---

## Task 14: E2E — side-effect warning scenario

Prove the agent warns before test-running a graph with real effects.

- [ ] **Step 1: Converse**

```bash
curl -s -X POST http://localhost:3000/chat -H "Content-Type: application/json" \
  -d '{"message":"quiero que cada vez que llegue un pedido, lo mande a mi sistema de inventario por su API y descuente stock"}' | tee -a /tmp/colmena_e2e/gb_convo_sideeffect.sse
```
Expected: when it reaches the point of test-running a graph that would POST/mutate, the agent **warns and asks for confirmation or uses safe test data** rather than silently executing. Verify in the SSE that it did not fire a real mutating call without warning.

- [ ] **Step 2: Present the friendly report and commit any fixes**

```bash
git add -A && git commit -m "test(graph_builder): verify side-effect warning E2E"
```

---

## Task 15: README

**Files:**
- Create: `tests/graphs/agents/graph_builder/README.md`

- [ ] **Step 1: Write the README**

Must cover: what the graph builder is (one paragraph in plain language), prerequisites (`source .env`, `DATABASE_URL`, `GEMINI_API_KEY`), how to serve it (`cargo run --bin dag_engine -- serve tests/graphs/agents/graph_builder/graph_builder.json`), how to talk to it (`curl -X POST http://localhost:3000/chat -d '{"message":"..."}'`), the capability menu it supports (the curated v1 list), the known limitations / backlog (no `guardar_grafo`, curated capability set, static single session, ADP migration pending), and a pointer to the spec and this plan.

- [ ] **Step 2: Commit**

```bash
git add tests/graphs/agents/graph_builder/README.md
git commit -m "docs(graph_builder): add README"
```

---

## Self-Review notes

- **Spec coverage:** Option A single agent (Task 2/11), curated capability menu (Tasks 3–10 skills + system_message §4), hybrid knowledge (Task 11 wiring), vocabulary + Excel disambiguation (Task 8 + system_message + Task 13 E2E), `probar_grafo` execution tool + feasibility gate (Tasks 1–2), side-effect safety (system_message + Task 14), chat delivery (system_message + Task 12), files layout (all tasks), E2E plan with `/tmp/colmena_e2e` reports (Tasks 12–14). `guardar_grafo` correctly excluded (backlog). All spec sections map to a task.
- **No placeholders:** the one intentional placeholder system_message in Task 2 is explicitly replaced verbatim in Task 11. Skill bodies are specified by exact required fields + mandatory verbatim examples with source file paths to copy from (not "fill in details").
- **Type/field consistency:** tool name `probar_grafo`, field `child_graph_inline`, memory fields `session_id`/`connection_url`, skills field `skills.paths`, default stack `google`/`gemini-2.5-flash`/`${GEMINI_API_KEY}` are used identically across all tasks.
```
