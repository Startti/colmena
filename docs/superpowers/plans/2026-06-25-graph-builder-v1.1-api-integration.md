# Graph Builder v1.1 (default-ports + Claude-like + API integration) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Upgrade the graph-builder agent so every graph it emits uses only bare default-port edges, it behaves like a thorough agent (scope → followups → confirm → always test → show the real result), and it can connect a generated agent to any HTTP API from its docs URL — demonstrated end-to-end against the real HubSpot API with secure token handling.

**Architecture:** Same single conversational `llm_call` (Option A). We (1) add tools to the builder (web fetch via `tavily_client`/`http_request`, `api_explorer`, and `secure_suspend_allowed`), (2) rewrite the `system_message` for the bare-edge invariant + Claude-like transparent testing + API/secure guidance, (3) rewrite all skill examples to bare edges and add a new `capability-api-integration` skill. The builder collects the API token via `secure_suspend` (handle only; never in LLM), tests the draft via `probar_grafo` (secure scope propagates through the subgraph), and delivers a graph whose auth references `${HUBSPOT_PRIVATE_APP_TOKEN}`.

**Tech Stack:** Colmena DAG engine (`serve`/`run`), JSON graphs, Markdown SKILL.md, builder = `google/gemini-2.5-flash`, generated API agents = `openai/gpt-4o`, Postgres (`DATABASE_URL`), `SECURE_VALUES_KEY`. No Rust changes expected.

**Spec:** [docs/superpowers/specs/2026-06-25-graph-builder-v1.1-api-integration-design.md](../specs/2026-06-25-graph-builder-v1.1-api-integration-design.md)
**Builds on:** the v1 implementation already on branch `feat/graph-builder-agent`.

**Conventions (CLAUDE.md / memories):** Conventional Commits only. `set -a; source .env; set +a` before provider/API runs. Save E2E SSE to `/tmp/colmena_e2e/<name>.sse` + friendly report. Never print secret values. Pass a stable `x-agent-session-id` header on every serve turn (memory + secure-value scope depend on it). Use real nodes/tools, never `log` as a mock.

---

## Bare-edge invariant (applies to EVERY graph any task emits or edits)

- Edges are ALWAYS `{"from":"A","to":"B"}` — never `{"from":"A.field","to":"B.field"}`.
- Field selection happens in `config`:
  - `llm_call`: `{{templates}}` referencing immediate inputs — e.g. `"prompt": "{{message}}"`, `"system_message": "...: {{task}}"`.
  - Nodes needing dynamic data (`http_request`, `sql_query`): use them **as a tool** in `tool_configurations` (no edge), OR feed them from a `python_script` adapter node whose output object the engine auto-flattens over a bare edge.
- Canonical agent shape: `trigger_webhook → llm_call → output` (3 bare edges); all complexity lives inside the `llm_call` config.

## API-tool auth pattern (used by the api-integration skill and E2E)

An API operation is exposed as an `http_request` tool. Auth uses the `bearer_token` field (the node auto-prefixes `Bearer `). The ONLY difference between the test graph and the delivered graph is that field's fixed value:

```json
// DELIVERED graph (others run it): resolves from env at egress
"node_schema": {
  "base_url":     { "type": "string", "fixed": "https://api.hubapi.com" },
  "endpoint":     { "type": "string", "fixed": "/crm/v3/objects/contacts" },
  "method":       { "type": "string", "fixed": "GET" },
  "bearer_token": { "type": "string", "fixed": "${HUBSPOT_PRIVATE_APP_TOKEN}" },
  "query_params": { "type": "object", "properties": {
      "limit": { "type": "string", "required": false, "description": "Cuántos traer (máx 100)" }
  }}
}
// TEST graph (builder, in-process): bearer_token fixed = the secure handle returned by ask_secret,
// e.g. "<sv_hubspot_private_app_token_a3f2bc7d>"  (resolves from encrypted DB via agent_session_id)
```

---

## File Structure

Under `tests/graphs/agents/graph_builder/` (existing dir from v1):
- `graph_builder.json` — MODIFY: add builder tools + `secure_suspend_allowed`; replace `system_message`.
- `skills/building-graphs-core/SKILL.md` — MODIFY: bare-edge rules + `python_script` adapter pattern + secure/two-placeholder note.
- `skills/capability-*/SKILL.md` (the 7 from v1) — MODIFY: convert every example to bare edges.
- `skills/capability-api-integration/SKILL.md` — CREATE.
- `README.md` — MODIFY: API flow + `secure_suspend`/`/resume` usage.

---

## Task 1: Add builder tools + secure_suspend to the meta-graph

Give the builder the ability to read docs, explore specs, and collect secrets. Config-only change; verify it still serves and greets.

**Files:** Modify `tests/graphs/agents/graph_builder/graph_builder.json` (the `agent` node `config`).

- [ ] **Step 1: Read current config**

Read `tests/graphs/agents/graph_builder/graph_builder.json` to see the existing `agent.config` (it has `provider/model/api_key/session_id/connection_url/system_message/skills/tool_configurations` with the `probar_grafo` subgraph tool).

- [ ] **Step 2: Add `secure_suspend_allowed`, `enabled_tools`, and a web-fetch + tavily tool**

In `agent.config`, add `"secure_suspend_allowed": true`, add `"enabled_tools": ["api_explorer"]`, and add to `tool_configurations` (keep the existing `probar_grafo`) a tavily fetch tool and a generic GET tool:

```json
"leer_web": {
  "name": "leer_web",
  "node_type": "tavily_client",
  "description": "Descarga y extrae el contenido de una URL (documentación de una API, una página web). Usalo para leer la doc que te pasa el usuario.",
  "node_config": { "api_key": "${TAVILY_API_KEY}" }
},
"http_get": {
  "name": "http_get",
  "node_type": "http_request",
  "description": "Hace un GET a una URL y devuelve el cuerpo. Úsalo para traer specs OpenAPI (.json/.yaml) o endpoints de documentación.",
  "node_schema": {
    "method":  { "type": "string", "fixed": "GET" },
    "base_url": { "type": "string", "required": true, "description": "Origen, ej. https://api.hubapi.com" },
    "endpoint": { "type": "string", "required": true, "description": "Ruta, ej. /crm/v3/objects/contacts" }
  }
}
```

(`api_explorer` is flag-only via `enabled_tools`. `secure_suspend_allowed: true` auto-injects the `ask_secret` tool. `tavily_client` needs `TAVILY_API_KEY` in `.env`; if absent, the `http_get` tool still covers fetching.)

- [ ] **Step 3: Verify it parses & greets**

```bash
mkdir -p /tmp/colmena_e2e
set -a; source .env; set +a
python3 -c "import json; json.load(open('tests/graphs/agents/graph_builder/graph_builder.json')); print('JSON OK')"
cargo run --bin dag_engine -- run tests/graphs/agents/graph_builder/graph_builder.json --agent-session-id gb11_smoke_001 2>&1 | tee /tmp/colmena_e2e/gb11_tools_smoke.sse
```
Expected: `JSON OK`; a friendly Spanish greeting; no validation error about unknown tool node_types (confirms `tavily_client`/`http_request`/`secure_suspend`/`api_explorer` are accepted).

- [ ] **Step 4: Commit**

```bash
git add tests/graphs/agents/graph_builder/graph_builder.json
git commit -m "feat(graph_builder): add doc-reading, api_explorer and secure_suspend tools to builder"
```

---

## Task 2: Rewrite the system_message (bare edges + Claude-like + API + secure)

**Files:** Modify `tests/graphs/agents/graph_builder/graph_builder.json` (`agent.config.system_message`).

- [ ] **Step 1: Replace `system_message` with the full text below**

Embed as a single valid JSON string (escape newlines/quotes). Preserve content exactly:

```text
Sos un asistente experto que ayuda a personas SIN conocimientos de programación a crear "grafos" de Colmena (flujos automatizados). Hablás español, claro y amable.

REGLA DE ORO: nunca uses jerga técnica con la persona. Nunca nombres nodos, puertos, edges, JSON ni node_types. Hablás SIEMPRE en términos de CAPACIDADES y de lo que la persona quiere lograr. No narres tu mecánica interna (no digas "cargo skills" ni "armo el grafo"): mostrá resultados, no maquinaria.

QUÉ PODÉS CONSTRUIR (capacidades):
- Que una IA responda, escriba, resuma o transforme texto.
- Buscar información en internet.
- Conectar con un servicio o API externa (incluso una que el usuario te indique con su documentación).
- Pausar para pedirle a la persona un dato o una decisión.
- Crear o editar una imagen / generar audio o voz.
- Trabajar con hojas de cálculo o documentos.
- Consultar o guardar datos en una base de datos.
- Hacer un cálculo o transformación de datos a medida.
- Decidir un camino distinto según el caso.

VOCABULARIO COLOQUIAL → capacidad (entendé la jerga, no la corrijas):
- "Excel", "planilla", "hoja", "tabla" → hoja de cálculo. "Word", "documento" → documento.
- "base de datos", "guardar registros" → base de datos. "mandar a un sistema", "conectar con tal app/API" → API externa.
- "buscar en Google/internet" → buscar en internet. "chatbot", "que conteste/redacte/resuma" → IA de texto.
- "foto/imagen/logo" → crear imagen. "audio/voz/que lo lea/podcast" → generar voz.

MÉTODO DE TRABAJO (como un buen agente):
1. ALCANCE: entendé primero el OBJETIVO (qué problema resolver), no la solución técnica.
2. FOLLOWUPS dirigidos: una pregunta a la vez, concreta, con ejemplos cuando ayude. Si un término es ambiguo, desambiguá en lenguaje simple (ej. "Excel": ¿editable en línea o archivo para descargar?).
3. CONFIRMÁ EL PLAN EN PALABRAS antes de construir: "Entonces: entra X → pasa Y → te devuelve Z. ¿Correcto?".
4. CONSTRUÍ el grafo.
5. PROBALO SIEMPRE de verdad con la herramienta probar_grafo. Horneá valores de prueba en el punto de entrada del grafo.
6. MOSTRÁ LA PRUEBA AL USUARIO: contá qué probaste (el input), qué devolvió de verdad (un resumen del resultado real), y un veredicto claro: "✅ Funciona" o "⚠️ Encontré esto y lo voy a corregir". Iterá hasta que dé verde.
7. ENTREGÁ recién cuando funciona: el grafo en un bloque de código + un resumen simple de qué hace y cómo usarlo.

REGLA TÉCNICA INVARIANTE (nunca la rompas al armar grafos):
- Las conexiones entre pasos son SIEMPRE de bloque a bloque completo, nunca de un campo puntual. Para elegir un dato, usá plantillas {{campo}} dentro de la configuración del bloque de IA (ej. el mensaje de la IA = "{{message}}"), o usá un bloque que necesite datos (como una API o una base de datos) COMO HERRAMIENTA de la IA, o prepará los datos con un bloque de cálculo cuyo resultado se pasa entero al siguiente. Cuando dudes, cargá la skill building-graphs-core.

CONECTAR CON UNA API EXTERNA (cuando el usuario te pasa una doc/URL):
- Leé la documentación con tus herramientas (leer_web / http_get; si hay un spec OpenAPI usá api_explorer). Sacá: la dirección base, cómo se autentica, y los endpoints que sirven para lo que el usuario quiere.
- Armá un agente (IA con provider OpenAI, modelo gpt-4o) que tenga esa API como herramienta(s): una herramienta por operación, con la dirección y la autenticación fijas, y los parámetros que completa la IA.
- AUTENTICACIÓN: nunca pongas la clave en el grafo ni se la pidas por chat. Para PROBAR, pedila con la herramienta ask_secret (te devuelve un identificador seguro, nunca ves la clave real) y usá ese identificador como token al probar. En el grafo ENTREGADO, la autenticación referencia la variable de entorno correspondiente (ej. ${HUBSPOT_PRIVATE_APP_TOKEN}); avisale al usuario que debe setear esa variable.
- SEGURIDAD DE EFECTOS: probá solo operaciones de LECTURA por defecto. Antes de probar algo que escribe/borra/envía (crear o editar registros, mandar mensajes), AVISÁ y pedí confirmación; si seguís, usá datos de prueba inocuos.

CÓMO TRABAJÁS POR DENTRO (no se lo cuentes a la persona):
- Antes de armar, cargá building-graphs-core y la skill de la capacidad que aplique (capability-ai-text, -web-and-apis, -ask-user, -multimedia, -docs-and-sheets, -data-sql, -code-and-logic, -api-integration) con load_skill.
- Stack por defecto del grafo: Gemini, salvo APIs externas donde el agente generado usa OpenAI gpt-4o.
```

- [ ] **Step 2: Verify parses & greets**

```bash
set -a; source .env; set +a
python3 -c "import json; json.load(open('tests/graphs/agents/graph_builder/graph_builder.json')); print('JSON OK')"
cargo run --bin dag_engine -- run tests/graphs/agents/graph_builder/graph_builder.json --agent-session-id gb11_sys_001 2>&1 | tee /tmp/colmena_e2e/gb11_sys.sse
```
Expected: `JSON OK`; friendly greeting; no jargon.

- [ ] **Step 3: Commit**

```bash
git add tests/graphs/agents/graph_builder/graph_builder.json
git commit -m "feat(graph_builder): rewrite system_message for bare-edges, transparent testing and API integration"
```

---

## Task 3: Rewrite `building-graphs-core` for bare edges + adapter pattern

**Files:** Modify `tests/graphs/agents/graph_builder/skills/building-graphs-core/SKILL.md` (keep frontmatter unchanged).

- [ ] **Step 1: Update the edges/ports section to the bare-edge invariant**

Replace any example or rule that uses dotted edges. The skill MUST now state and show:
- Edges are ALWAYS bare `{"from":"A","to":"B"}`. NO `nodo.campo` anywhere.
- Field selection in config via `{{templates}}` (llm_call). Show the canonical `trigger_webhook → llm_call → output` with `"prompt": "{{message}}"` (copy the proven shape from `/tmp/colmena_e2e/mem_min.json` pattern: bare edges + `{{message}}`).
- The **python_script adapter pattern**: when a node needs dynamic inputs (e.g. `http_request`), put a `python_script` before it that builds the input object; a bare edge auto-flattens it. Include a runnable example:
```json
{
  "nodes": {
    "trigger": { "type": "trigger_webhook", "config": { "path": "/run", "test_payload": { "ciudad": "Bogota" } } },
    "preparar": { "type": "python_script", "config": { "code": "output = { 'base_url': 'https://api.exemplo.com', 'endpoint': '/clima/' + ciudad, 'method': 'GET' }" } },
    "llamar": { "type": "http_request", "config": {} },
    "out": { "type": "output", "config": {} }
  },
  "edges": [
    { "from": "trigger", "to": "preparar" },
    { "from": "preparar", "to": "llamar" },
    { "from": "llamar", "to": "out" }
  ]
}
```
- Note the alternative: expose `http_request`/`sql_query` as a **tool** of an `llm_call` (no edges at all) — preferred for agents.

- [ ] **Step 2: Add a short "secrets & APIs" subsection**

Add the two-placeholder rule (test = secure handle `<sv_...>` in `bearer_token`; delivered = `${ENV_VAR}` in `bearer_token`), one sentence each, and a pointer `[[capability-api-integration]]`.

- [ ] **Step 3: Verify no dotted edges remain**

```bash
grep -nE '"(from|to)"\s*:\s*"[a-zA-Z0-9_]+\.' tests/graphs/agents/graph_builder/skills/building-graphs-core/SKILL.md || echo "NO DOTTED EDGES — OK"
```
Expected: `NO DOTTED EDGES — OK`.

- [ ] **Step 4: Commit**

```bash
git add tests/graphs/agents/graph_builder/skills/building-graphs-core/
git commit -m "docs(graph_builder): bare-edge rules + python_script adapter in building-graphs-core"
```

---

## Task 4: Convert the 7 capability skills to bare edges

**Files:** Modify each `tests/graphs/agents/graph_builder/skills/capability-{ai-text,web-and-apis,ask-user,multimedia,docs-and-sheets,data-sql,code-and-logic}/SKILL.md` (frontmatter unchanged).

- [ ] **Step 1: Find every dotted edge across the 7 skills**

```bash
grep -rnE '"(from|to)"\s*:\s*"[a-zA-Z0-9_]+\.' tests/graphs/agents/graph_builder/skills/capability-*/SKILL.md
```

- [ ] **Step 2: Convert each occurrence**

For every runnable example, rewrite edges to bare node→node and move the field selection into config:
- If the downstream is an `llm_call` reading a field, set its `prompt`/`system_message` to use `{{field}}` and make the edge bare.
- If the downstream needs structured dynamic data (http/sql as a plain node), insert a `python_script` adapter (per `building-graphs-core`) or convert it to a tool. Keep examples runnable and accurate.
- Do NOT change frontmatter or factual config field names.

- [ ] **Step 3: Verify no dotted edges remain in any capability skill**

```bash
grep -rnE '"(from|to)"\s*:\s*"[a-zA-Z0-9_]+\.' tests/graphs/agents/graph_builder/skills/capability-*/SKILL.md && echo "STILL HAS DOTTED — FIX" || echo "ALL BARE — OK"
```
Expected: `ALL BARE — OK`.

- [ ] **Step 4: Sanity-check one converted example actually runs**

Pick the converted `capability-ai-text` example, save it to `/tmp/colmena_e2e/gb11_ai_text_check.json`, and run:
```bash
set -a; source .env; set +a
cargo run --bin dag_engine -- run /tmp/colmena_e2e/gb11_ai_text_check.json --agent-session-id gb11_ai_001 2>&1 | tee /tmp/colmena_e2e/gb11_ai_text_check.sse
```
Expected: runs and produces an LLM answer (proves bare-edge + `{{template}}` example is valid).

- [ ] **Step 5: Commit**

```bash
git add tests/graphs/agents/graph_builder/skills/
git commit -m "docs(graph_builder): convert all capability skill examples to bare edges"
```

---

## Task 5: Author `capability-api-integration` skill

**Files:** Create `tests/graphs/agents/graph_builder/skills/capability-api-integration/SKILL.md`.

- [ ] **Step 1: Write the SKILL.md**

Frontmatter verbatim:
```markdown
---
name: capability-api-integration
description: Use when the user wants an agent to connect to an external HTTP API and points you at its documentation (a URL). Covers reading the docs, building an agent whose tools call the API, secure token handling, and testing against the real API.
---
```
Body (Spanish) MUST cover:
1. **Leer la doc (híbrido):** use `leer_web` (tavily fetch) / `http_get` to download docs; if an OpenAPI spec exists, use `api_explorer` (`enabled_tools: ["api_explorer"]`, sub-tools load_spec/list_endpoints/search_endpoint/get_endpoint_details/build_http_request). Extract base_url, auth scheme, relevant endpoints.
2. **Armar el agente generado:** an `llm_call` with `provider:"openai"`, `model:"gpt-4o"`, `api_key:"${OPENAI_API_KEY}"`, edges `trigger → llm → output` (bare), and one `http_request` tool per operation in `tool_configurations`. Include the VERBATIM HubSpot "listar contactos" tool block from the "API-tool auth pattern" section at the top of the plan (the DELIVERED variant with `bearer_token` fixed = `${HUBSPOT_PRIVATE_APP_TOKEN}`).
3. **Auth segura (las dos variantes):** TEST graph uses `bearer_token` fixed = the secure handle returned by `ask_secret` (e.g. `<sv_hubspot_private_app_token_a3f2bc7d>`); DELIVERED graph uses `bearer_token` fixed = `${HUBSPOT_PRIVATE_APP_TOKEN}`. Explain the handle is session+TTL scoped so it can't be shipped, and the delivered graph reads the env var at egress. The agent must tell the user to set that env var.
4. **Probar seguro:** collect token with `ask_secret`, bake the handle into the test graph, run via `probar_grafo` (needs stable `agent_session_id` so the secret scope propagates into the subgraph). Read-only by default; warn before writes.
5. Reference `[[building-graphs-core]]` (bare edges) and `[[capability-web-and-apis]]` (http_request/tavily/api_explorer detail).

- [ ] **Step 2: Add the skill dir to the builder's `skills.paths`**

In `tests/graphs/agents/graph_builder/graph_builder.json`, append `"tests/graphs/agents/graph_builder/skills/capability-api-integration"` to `agent.config.skills.paths`.

- [ ] **Step 3: Verify**

```bash
grep -m1 '^name:' tests/graphs/agents/graph_builder/skills/capability-api-integration/SKILL.md
python3 -c "import json; d=json.load(open('tests/graphs/agents/graph_builder/graph_builder.json')); assert any('api-integration' in p for p in d['nodes']['agent']['config']['skills']['paths']); print('PATH WIRED OK')"
```
Expected: prints the name line and `PATH WIRED OK`.

- [ ] **Step 4: Commit**

```bash
git add tests/graphs/agents/graph_builder/skills/capability-api-integration/ tests/graphs/agents/graph_builder/graph_builder.json
git commit -m "docs(graph_builder): add capability-api-integration skill and wire it"
```

---

## Task 6: E2E — bare-edge regression (the v1 bot still works)

**Files:** none (verification).

- [ ] **Step 1: Serve and converse with a stable handle**

```bash
set -a; source .env; set +a
cargo run --bin dag_engine -- serve tests/graphs/agents/graph_builder/graph_builder.json > /tmp/colmena_e2e/gb11_serve_bot.log 2>&1 &
SERVE_PID=$!
# wait until up (poll log / curl), then:
curl -s -X POST http://localhost:3000/chat -H "Content-Type: application/json" -H "x-agent-session-id: gb11_bot_001" -d '{"message":"quiero algo que conteste preguntas de mis clientes, es una cafetería"}' | tee -a /tmp/colmena_e2e/gb11_bot.sse
# answer its followups realistically; confirm "dale armámelo"; up to ~6 turns
kill $SERVE_PID
```

- [ ] **Step 2: Verify the delivered graph uses ONLY bare edges and self-runs**

Extract the delivered JSON to `/tmp/colmena_e2e/gb11_delivered_bot.json`, then:
```bash
grep -nE '"(from|to)"\s*:\s*"[a-zA-Z0-9_]+\.' /tmp/colmena_e2e/gb11_delivered_bot.json && echo "HAS DOTTED — FAIL" || echo "ALL BARE — OK"
cargo run --bin dag_engine -- run /tmp/colmena_e2e/gb11_delivered_bot.json --agent-session-id gb11_bot_deliv_001 2>&1 | tee /tmp/colmena_e2e/gb11_delivered_bot.sse
```
Expected: `ALL BARE — OK`; the graph runs and produces an answer (LLM node executes, not just the trigger).

- [ ] **Step 3: Friendly report + commit any prompt/skill fixes**

```bash
git add -A && git commit -m "test(graph_builder): verify bare-edge bot E2E" || echo "nothing to commit"
```

---

## Task 7: E2E — HubSpot, full secure API flow (the headline scenario)

Prerequisites: `.env` has `OPENAI_API_KEY`, `GEMINI_API_KEY`, `DATABASE_URL`, `SECURE_VALUES_KEY`, and `HUBSPOT_PRIVATE_APP_TOKEN` (real token). Never print the token.

**Files:** none (verification).

- [ ] **Step 1: Serve**

```bash
set -a; source .env; set +a
test -n "$HUBSPOT_PRIVATE_APP_TOKEN" && echo "token present" || echo "MISSING TOKEN"
test -n "$SECURE_VALUES_KEY" && echo "key present" || echo "MISSING KEY"
cargo run --bin dag_engine -- serve tests/graphs/agents/graph_builder/graph_builder.json > /tmp/colmena_e2e/gb11_serve_hs.log 2>&1 &
SERVE_PID=$!
# wait until up
```

- [ ] **Step 2: Ask for a HubSpot agent and let it read the docs**

```bash
curl -s -X POST http://localhost:3000/chat -H "Content-Type: application/json" -H "x-agent-session-id: gb11_hs_001" \
  -d '{"message":"quiero un agente que me consulte los contactos de mi cuenta de HubSpot. Acá está la doc: https://developers.hubspot.com/docs/api-reference/latest/overview"}' | tee -a /tmp/colmena_e2e/gb11_hs.sse
# continue answering followups (e.g. "solo leer contactos, traer nombre y email") and confirm the plan
```
Expected: the builder uses `leer_web`/`http_get`/`api_explorer` (visible as tool calls), and proposes a flow naming the HubSpot base URL, Bearer auth, and a contacts read endpoint.

- [ ] **Step 3: Secure token collection via ask_secret + /resume**

When the builder calls `ask_secret`, the serve response will be a SUSPEND carrying a session id and the secret question. Capture that session id, then resume with the real token from the env (do NOT echo it):
```bash
# SID extracted from the suspend response of the previous turn:
curl -s -X POST http://localhost:3000/resume -H "Content-Type: application/json" -H "x-agent-session-id: gb11_hs_001" \
  -d "$(python3 -c "import json,os; print(json.dumps({'session_id': os.environ['GB_SID'], 'answer': 'Q[hubspot_private_app_token]: token\nA[hubspot_private_app_token]: '+os.environ['HUBSPOT_PRIVATE_APP_TOKEN']}))")" \
  | tee -a /tmp/colmena_e2e/gb11_hs_resume.sse
```
(`GB_SID` = the suspended run's session id from Step 2's response.) Expected: the builder receives a handle and proceeds. **Verify the token string does NOT appear anywhere in `/tmp/colmena_e2e/gb11_hs*.sse`.**

```bash
grep -c "$HUBSPOT_PRIVATE_APP_TOKEN" /tmp/colmena_e2e/gb11_hs*.sse && echo "LEAK — FAIL" || echo "NO LEAK — OK"
```
Expected: `NO LEAK — OK`.

- [ ] **Step 4: Verify the real test ran and the result was shown**

In the SSE, confirm: a `probar_grafo` tool call whose test graph's `bearer_token` is the `<sv_...>` handle; a real HubSpot response (contacts payload, masked); and the builder's message to the user showing the test input + a sample of the real result + a ✅/⚠️ verdict.

- [ ] **Step 5: Verify the delivered graph uses env var and runs standalone**

Extract delivered JSON to `/tmp/colmena_e2e/gb11_delivered_hs.json`:
```bash
grep -nE '"(from|to)"\s*:\s*"[a-zA-Z0-9_]+\.' /tmp/colmena_e2e/gb11_delivered_hs.json && echo "HAS DOTTED — FAIL" || echo "ALL BARE — OK"
grep -q 'HUBSPOT_PRIVATE_APP_TOKEN' /tmp/colmena_e2e/gb11_delivered_hs.json && echo "ENV REF OK" || echo "MISSING ENV REF — FAIL"
grep -q '<sv_' /tmp/colmena_e2e/gb11_delivered_hs.json && echo "LEAKED HANDLE — FAIL" || echo "NO HANDLE — OK"
cargo run --bin dag_engine -- run /tmp/colmena_e2e/gb11_delivered_hs.json --agent-session-id gb11_hs_deliv_001 2>&1 | tee /tmp/colmena_e2e/gb11_delivered_hs.sse
kill $SERVE_PID
```
Expected: `ALL BARE — OK`, `ENV REF OK`, `NO HANDLE — OK`; the standalone run authenticates via the env var and returns real HubSpot contact data.

- [ ] **Step 6: Friendly report + commit fixes**

Present: the request, the docs it read, the proposed flow, proof the token never leaked, the real test result shown to the user, and the delivered graph. Don't paste whole SSE; never print the token.
```bash
git add -A && git commit -m "test(graph_builder): verify HubSpot secure API integration E2E" || echo "nothing to commit"
```

---

## Task 8: E2E — write-effect warning + README update

- [ ] **Step 1: Verify the write-effect warning**

Serve again; with `x-agent-session-id: gb11_hsw_001` ask: `"ahora quiero que el agente CREE un contacto nuevo en HubSpot cuando se lo pida"`. Expected: before test-running the create (POST), the builder warns and asks for confirmation / proposes safe test data, rather than firing a real write unprompted. Save to `/tmp/colmena_e2e/gb11_hs_write.sse`.

- [ ] **Step 2: Update README**

Modify `tests/graphs/agents/graph_builder/README.md`: add an "Conectar con una API (ej. HubSpot)" section covering — pass the docs URL in chat; the builder reads it; it asks for the token via `ask_secret` and you answer via `POST /resume` (same `x-agent-session-id`, never as a chat message); it tests read-only against the real API; the delivered graph needs `HUBSPOT_PRIVATE_APP_TOKEN` set in env; required env (`SECURE_VALUES_KEY`, `DATABASE_URL`, `OPENAI_API_KEY`, optionally `TAVILY_API_KEY`). Also add one line to the capability list: "Conectar con una API externa desde su documentación".

- [ ] **Step 3: Commit**

```bash
git add tests/graphs/agents/graph_builder/README.md
git commit -m "docs(graph_builder): document API integration + secure_suspend flow in README"
```

---

## Self-Review notes

- **Spec coverage:** A1 bare edges (Tasks 3,4 + system_message Task 2 + verified Tasks 6,7 via grep); A2 transparent testing (system_message Task 2 + verified Task 7 step 4); B API integration — hybrid doc read (Task 1 tools + Task 5 skill), generated agent OpenAI + http tools (Task 5 + Task 7), auth env var (Task 5,7); C secure flow — secure_suspend collect (Task 1 + Task 7 step 3), handle in test graph + probar_grafo scope (Task 7 step 4), two-placeholder delivery (Task 5 + Task 7 step 5), no-leak check (Task 7 step 3,5), read-only default + write warning (system_message + Task 8). README (Task 8). All spec sections map to tasks.
- **Placeholder scan:** system_message in Task 2 is full verbatim text, not a placeholder. Skill bodies specify exact required content + verbatim auth block referenced from the plan header. No "TBD/etc."
- **Consistency:** `probar_grafo`, `leer_web`, `http_get`, `ask_secret`, `secure_suspend_allowed`, `bearer_token`, `${HUBSPOT_PRIVATE_APP_TOKEN}`, `<sv_hubspot_private_app_token_...>`, stable `x-agent-session-id` used identically across tasks.
```
