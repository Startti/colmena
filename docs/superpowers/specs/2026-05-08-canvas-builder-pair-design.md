# Diseño: par de grafos canvas-builder (controlado vs autónomo)

**Estado:** propuesta aprobada
**Fecha:** 2026-05-08
**Autor:** Daniel García (Startti)

## Contexto

Tenemos un seed `tests/graphs/external/socketio_canvas_builder.json` que orquesta el canvas de ADP vía Socket.IO con 9 tools básicos (CRUD nodos/aristas + API keys). Después de cerrar la plataforma (`secure_suspend`, Gap 1, Gap 2, Spec 4), todas las piezas están listas para entregar la **demo end-to-end del canvas-builder**: un agente que recibe una intención del usuario ("quiero un agente que actualice deals en HubSpot"), busca docs en internet, lee el spec OpenAPI, recolecta credenciales con `secure_suspend`, construye los nodos en el canvas, prueba el resultado vía `/chat/run`, e itera hasta que funcione.

El brainstorm decidió entregar **dos JSONs** que comparten 99% del cuerpo y difieren solo en personalidad (system_message + parámetros + qué subset de tools de gestión-de-grupo expone). Razón: el control real está en el prompt + tools expuestos, no en cableado distinto.

## Objetivo

Producir dos grafos en `tests/graphs/external/`:

- `canvas_builder_autonomous.json` — el meta-agente puede crear el group desde cero, itera hasta cerrar el ticket.
- `canvas_builder_controlled.json` — el meta-agente solo edita un group existente, confirma cada mutación con el usuario, no puede borrar nodos.

## Tools comunes (ambos sabores)

| Tool | Backing | Origen |
|---|---|---|
| `load_canvas` | `socketio_request` | seed |
| `create_canvas_node` | `socketio_request` | seed |
| `update_canvas_node` | `socketio_request` | seed |
| `create_edge` | `socketio_request` | seed |
| `update_edge` | `socketio_request` | seed |
| `list_api_keys` | `http_request` GET `/api-keys` | seed |
| `create_api_key` | `http_request` POST `/api-keys` | seed |
| `list_groups` | `http_request` GET `/agents/groups?environmentId=…` | **NUEVO** |
| `web` | `tavily_client` (`expose_sub_tools: all`) | **NUEVO** |
| `apis` | `api_explorer` (`expose_sub_tools: all`) | **NUEVO** |
| `ask_secret` | `secure_suspend` | **NUEVO** |
| `test_agent` | `http_request` POST `/chat/run` | **NUEVO** |

## Diferencias entre sabores

| Aspecto | Controlado | Autónomo |
|---|---|---|
| `temperature` | 0.0 | 0.3 |
| `reasoning_effort` (Gemini) | `low` | `high` |
| `system_message` | "Confirma cada mutación con el usuario antes de ejecutarla. Trabaja sobre un group existente — pídelo si no lo conoces." | "Construye el agente solicitado de un tirón, prueba con `test_agent`, ajusta y vuelve a probar. Pregunta solo cuando necesites credenciales." |
| Tools de mutación de grupo | NO incluye `create_group`, `update_group`, `delete_group` | Incluye los 3: `create_group`, `update_group`, `delete_group` |
| Tools de borrado de nodos/aristas | Incluye `delete_canvas_node` y `delete_edge` (con regla "confirmar antes" en system_message) | Incluye `delete_canvas_node` y `delete_edge` |
| Loop test | Después de cada `create_canvas_node`/`create_edge`, llamar `test_agent` para verificar | Test al menos 1 vez después de armar el grupo mínimo (chatInput + llmCall + chatOutput) |

## Configuración por tool nuevo

### `list_groups` (HTTP GET)

```jsonc
{
  "name": "list_groups",
  "node_type": "http_request",
  "description": "List all agent groups in the current ADP environment. Use this BEFORE deciding whether to create a new group: if the user already has a group they want to edit, find it here. Returns an array of {id, name, label, publishStatus, ...}. The id is what you pass as groupId to test_agent.",
  "node_schema": {
    "base_url":  { "type": "string",  "fixed": "${ADP_API_URL}" },
    "endpoint":  { "type": "string",  "fixed": "/agents/groups" },
    "method":    { "type": "string",  "fixed": "GET" },
    "headers":   { "type": "object",  "fixed": {
      "Cookie": "__Secure-better-auth.session_token=${ADP_SESSION_TOKEN}"
    }},
    "query_params": { "type": "object", "fixed": {
      "environmentId": "${ADP_ENVIRONMENT_ID}"
    }}
  }
}
```

### `web` (tavily_client expose_sub_tools all)

Pattern existente — copy-paste from `api_explorer_hubspot_conversation.json`:

```jsonc
{
  "name": "web",
  "description": "Web search and fetch via Tavily. Use to discover API documentation, OpenAPI spec URLs, and reference material before building API integrations.",
  "node_type": "tavily_client",
  "node_config": { "api_key": "${TAVILY_API_KEY}" },
  "expose_sub_tools": "all"
}
```

### `apis` (api_explorer expose_sub_tools all)

```jsonc
{
  "name": "apis",
  "description": "OpenAPI / Swagger 2.0 discovery: load specs, list endpoints, describe parameters, and build apiCall payloads. Use after `web` finds a spec URL. NEVER skip api_explorer when wiring an apiCall — the LLM-generated config without spec inspection has too high error rate.",
  "node_type": "api_explorer",
  "node_config": {},
  "expose_sub_tools": "all"
}
```

### `ask_secret` (secure_suspend)

```jsonc
{
  "name": "ask_secret",
  "node_type": "secure_suspend",
  "description": "Ask the user for ONE OR MORE secrets in a SINGLE prompt cycle. Pass `secrets: [{question, name}, ...]` — typically batch all credentials needed for the same external service in one call. Returns `{handles: {<name>: <sv_<name>>}}`. Paste each handle as a complete string value into bearerToken / a key inside object-form body / etc. NEVER ask for secrets via plain chat. NEVER embed a handle inside a longer string. The OAuth two-call pattern (client_id + client_secret) is fully supported — see references/apiCall.md §Authentication patterns."
}
```

### `test_agent` (HTTP POST `/chat/run`)

```jsonc
{
  "name": "test_agent",
  "node_type": "http_request",
  "description": "Run the agent currently being built and get back its full response (text, tool_calls, errorText, suspended state). Use this AFTER each set of canvas mutations to verify behavior. Pass groupId (the id of the group you've been editing) and prompt (a representative user input). Errors come back as HTTP 200 with `errorText` populated — branch on body.",
  "node_schema": {
    "base_url": { "type": "string", "fixed": "${ADP_API_URL}" },
    "endpoint": { "type": "string", "fixed": "/chat/run" },
    "method":   { "type": "string", "fixed": "POST" },
    "headers":  { "type": "object", "fixed": {
      "Cookie": "__Secure-better-auth.session_token=${ADP_SESSION_TOKEN}",
      "Content-Type": "application/json"
    }},
    "body": {
      "type": "object",
      "required": true,
      "properties": {
        "groupId":   { "type": "string", "required": true,  "description": "The id of the group to test (from list_groups or create_group)." },
        "prompt":    { "type": "string", "required": true,  "description": "User input to feed into the agent." },
        "sessionId": { "type": "string", "required": false, "description": "Resume an existing test session." },
        "persist":   { "type": "boolean", "fixed": false }
      }
    }
  }
}
```

### Solo en autónomo: `create_group`, `update_group`, `delete_group` (Socket.IO)

`delete_canvas_node` y `delete_edge` se incluyen en AMBOS sabores. La diferencia está en el workflow del system_message del controlado: "ALWAYS confirm with the user before calling this — destructive". El meta-agente puede borrar para corregir errores, pero solo después de confirmación explícita.

`create_group` ejemplo:

```jsonc
{
  "name": "create_group",
  "node_type": "socketio_request",
  "description": "Create a new agent group in the current environment. Use this when the user wants to build a new agent from scratch (no list_groups match). Returns the created group object — use its id with subsequent create_canvas_node calls and with test_agent.",
  "node_schema": {
    "url":       { "type": "string", "fixed": "${ADP_API_URL}" },
    "namespace": { "type": "string", "fixed": "/canvas" },
    "event":     { "type": "string", "fixed": "create_group" },
    "cookies":   { "type": "string", "fixed": "__Secure-better-auth.session_token=${ADP_SESSION_TOKEN}" },
    "timeout_ms":{ "type": "integer", "fixed": 15000 },
    "payload": {
      "type": "object",
      "properties": {
        "environmentId": { "type": "string", "fixed": "${ADP_ENVIRONMENT_ID}" },
        "name":          { "type": "string", "required": true, "description": "Human-readable group name." },
        "label":         { "type": "string", "description": "Optional label." }
      }
    }
  }
}
```

`update_group` y `delete_group` siguen el mismo molde.

## System messages (texto canónico)

### Controlado

```
You are a CONTROLLED canvas-builder agent. Your job is to help the user EDIT an existing agent group on the ADP canvas.

WORKFLOW:
1. Always call `list_groups` first. Ask the user which group they want to edit if their intent is ambiguous.
2. Call `load_canvas` to see the existing nodes/edges/groups.
3. Before EVERY mutation (`create_canvas_node`, `update_canvas_node`, `create_edge`, `update_edge`), state in plain text what you're about to do and WAIT for the user's confirmation. Do NOT batch mutations.
4. Before creating any AI node, call `list_api_keys` and present the available keys. ASK the user which to use — never fabricate.
5. When credentials are needed for an external API: ALWAYS use `ask_secret` (one call, all secrets needed). Paste handles into bearerToken / object-form body — see references/apiCall.md §Authentication patterns.
6. After each set of mutations, call `test_agent` to verify the agent works. Read `errorText` and `text` carefully. If errors appear, propose a fix and wait for user confirmation before applying.

CONSTRAINTS:
- You CANNOT delete nodes, edges, or groups in this mode.
- You CANNOT create new groups — only edit the one the user selected.
- Always prefer reusing existing nodes over creating duplicates.

LAYOUT: each node is ~250×50px. Vertical flow: Δy=200 between rows. Tools attached to AI nodes: Δx=400 to the side, Δy=120 between stacked tools. Read load_canvas output before placing — don't overlap.

When the user describes an integration ("connect HubSpot", "fetch from API X"): use `web` to find the API docs, then `apis` to load and inspect the spec, then build the apiCall config from there. Never guess endpoint paths or request shapes.
```

### Autónomo

```
You are an AUTONOMOUS canvas-builder agent. Your job is to deliver a working agent group from a single user intent statement.

WORKFLOW:
1. Call `list_groups` to see what already exists. If the user's intent matches an existing group, edit it. Otherwise call `create_group` with a name derived from the intent.
2. Build the minimum viable group: chatInput → llmCall (or agent) → chatOutput. Configure llmCall with `list_api_keys` first; pick a sensible default key (Gemini if available).
3. Run `test_agent` ONCE to confirm the skeleton works. Then start adding tools per the user's intent.
4. For external APIs: use `web` to find docs, `apis` to load the OpenAPI spec, build the apiCall node from the spec — never guess. When credentials are needed: `ask_secret` with all needed names in one call, paste handles per the rules in references/apiCall.md.
5. After meaningful additions (a new tool, a new edge), call `test_agent` to verify. Read `errorText` and `text`. Iterate: fix → test → fix → test. You're done when test_agent returns a coherent response that matches the user's intent.
6. Tell the user briefly what you built and the groupId. Do NOT enumerate every tool — the user can see the canvas.

CONSTRAINTS:
- ASK the user only for credentials (via `ask_secret`) and for ambiguous intent. Otherwise pick reasonable defaults and proceed.
- You CAN delete nodes / edges / groups when refactoring or recovering from a wrong build.

LAYOUT: same as controlled mode — see load_canvas output before placing.

Be efficient: the user will not see your reasoning. Show your final build via test_agent's response and a brief one-line summary.
```

## Reasoning budget / parámetros LLM

| Param | Controlado | Autónomo |
|---|---|---|
| `provider` | `gemini` | `gemini` |
| `model` | `gemini-2.5-flash` | `gemini-2.5-flash` |
| `temperature` | 0.0 | 0.3 |
| `reasoning_effort` | `low` | `high` |
| `stream` | false | false |

## Pre-requisitos del entorno

Variables de entorno requeridas (todos los grafos):

- `GEMINI_API_KEY` — para el meta-agente.
- `TAVILY_API_KEY` — para el tool `web`.
- `ADP_API_URL` — base URL del API de ADP.
- `ADP_SESSION_TOKEN` — cookie de sesión del usuario que ejecuta el meta-agente.
- `ADP_ENVIRONMENT_ID` — environment donde se editará/creará el group.
- `ADP_WORKSPACE_ID` — para `create_api_key`.
- `DATABASE_URL` — Postgres compartido (memoria conversacional + secure values).

Skills cargadas (igual que el seed): `["/app/skills/adp-node-catalog"]`.

## Plan de testing

### Smoke local

Con todas las env vars + DATABASE_URL apuntando a una DB que comparta el mapping con ADP:

```bash
cargo run --bin dag_engine -- run tests/graphs/external/canvas_builder_autonomous.json \
  --agent-session-id canvas_builder_smoke_001
```

El meta-agente debería:
1. Llamar `list_groups`.
2. Si no hay groups → llamar `create_group` con un nombre razonable.
3. Suspender via `ask_secret` cuando necesite credenciales.

### Demo HubSpot end-to-end (validación final manual)

Prompt al autónomo: "Crea un agente que pueda actualizar deals en HubSpot."

Esperado: el agente busca docs HubSpot, carga el OpenAPI spec, pide el HubSpot Private App token vía `ask_secret`, crea apiCall(s) con `bearerToken: <sv_hubspot_token>` y `secure: true`, llama `test_agent` con un prompt de prueba ("Update deal id 123 with stage 'closedwon'"), ajusta hasta que funcione, reporta el groupId.

## Pre-condiciones cumplidas

- `secure_suspend` (Spec 1) ✓
- `inject_secrets` cubre config (Gap 1) ✓
- `llm_call` propaga SUSPENDED (Gap 2) ✓
- `agent_session_id`-first lookup (Spec 4) ✓
- Patrón de auth segura documentado en `adp-node-catalog/references/apiCall.md` ✓

## Fuera de alcance

- Webhook / schedule triggers, runCode, fileSearch, websocketCall, condition, memory nodes — no se exponen al meta-agente porque la skill `adp-node-catalog` aún no los documenta. Cuando esa skill se expanda (Spec 3 diferido), añadir referencias a este grafo.
- Crítico LLM o validación automática de los grafos generados — el meta-agente confía en `test_agent` y en el feedback del usuario.

## Cambios concretos al repo

| Archivo | Acción |
|---|---|
| `tests/graphs/external/canvas_builder_autonomous.json` | NUEVO. ~800-900 líneas (seed + 4 tools nuevos + 3 group socketio tools + system_message). |
| `tests/graphs/external/canvas_builder_controlled.json` | NUEVO. ~700-800 líneas (idem sin tools de mutación destructiva, sin create_group, system_message distinto, params menos liberales). |

No tocan código Rust. Solo JSON.
