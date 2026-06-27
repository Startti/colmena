# Diseño — Graph Builder v1.1: default-ports, comportamiento tipo-Claude e integración de APIs

**Fecha:** 2026-06-25
**Estado:** Diseño aprobado, pendiente de plan de implementación
**Autor:** daniel + Claude
**Antecede:** [v1 design](2026-06-25-graph-builder-agent-design.md) (ya implementado en rama `feat/graph-builder-agent`)

## 1. Resumen

Segunda iteración del agente creador de grafos. Agrega tres cosas:

1. **Regla dura — edges siempre default node→node** (nunca `nodo.campo`). La selección de
   datos vive en el `config`, no en los edges.
2. **Comportamiento "como Claude"** — define alcance, hace followups, confirma el plan,
   **siempre prueba el grafo de verdad y le muestra el resultado al usuario** antes de entregar.
3. **Nueva capacidad: conectar a cualquier API desde su documentación (URL)** — el usuario
   dice "conectate a esta API, acá está la doc <URL>"; el builder la lee, arma un agente que
   la consume, y lo prueba contra la API real de forma segura. Ejemplo E2E: **HubSpot**.

Sigue siendo un único `llm_call` conversacional (Opción A de v1), con más herramientas y
una skill nueva. Standalone en Colmena (modo `serve`). ADP queda fuera de alcance.

## 2. Bloque A — Reglas de comportamiento (afectan todo el builder)

### A1. Edges siempre default node→node (restricción dura)
El builder **nunca** emite un edge con `nodo.campo`. Todos los edges son bare
`{"from":"A","to":"B"}`. La selección de datos se hace en `config`:
- **`llm_call`**: `{{templates}}` (ej. `prompt: "{{message}}"`, `system_message` con `{{task}}`).
  El template referencia los inputs inmediatos del nodo (resolución `resolve_template_vars`).
- **Nodos que necesitan datos dinámicos** (`http_request`, `sql_query`): se usan **como tool
  del `llm_call`** (config en `tool_configurations`, sin edges) **o** se alimentan desde un
  nodo `python_script` adaptador cuyo objeto de salida el motor **auto-aplana** al nodo
  destino por un edge bare.
- **Fundamento técnico** (confirmado en `run_use_case.rs` edge-resolution, ~líneas 900-1009):
  un edge bare usa `default_output` de A y `default_input` de B; si B no tiene
  `default_input`, se auto-aplanan todas las claves del objeto de A en los inputs de B.
  `http_request` **no** soporta `{{templates}}` en config (solo `${ENV}`), por eso se usa como
  tool o vía adaptador `python_script`.

**Impacto:** regenerar los ejemplos de las 8 skills v1 **y** de `building-graphs-core` a edges
bare. Hoy `building-graphs-core` muestra `trigger.message → agent.prompt`; pasa a
`trigger → agent` + `prompt: "{{message}}"`. Mecánico pero global. El `system_message` agrega
la regla como invariante.

### A2. Comportamiento "como Claude" + prueba transparente
Upgrade del `system_message`:
1. **Definir el alcance** y hacer **followups dirigidos** (no genéricos).
2. **Confirmar el plan en palabras** antes de construir.
3. **Construir** el grafo.
4. **SIEMPRE correr una prueba real** con `probar_grafo`.
5. **Mostrarle al usuario la prueba**: qué input usó, la respuesta real (muestra, con secretos
   enmascarados), y un **veredicto explícito** (✅ funciona / ⚠️ problema + qué va a hacer).
6. **Iterar** hasta que dé verde; recién entonces **entregar**.

## 3. Bloque B — Capacidad: conectar a cualquier API desde su doc (URL)

Nueva skill `capability-api-integration` + nuevas herramientas en el builder.

### Leer la documentación (híbrido)
El builder recibe una **URL de docs** y la entiende:
- **Fetch genérico (universal):** descarga la doc con un fetch web (`tavily_client` sub-tool
  `fetch`, o `http_request` GET) y razona sobre el HTML/markdown para extraer **base_url**,
  **esquema de auth**, y los **endpoints relevantes**. Puede navegar varias páginas.
- **`api_explorer` (cuando hay spec):** si detecta un OpenAPI/Swagger disponible, lo carga con
  `api_explorer` para precisión estructurada.

### Construir el agente generado
- Un `llm_call` con **provider OpenAI** (ej. `gpt-4o`) — el builder en sí sigue en
  `google/gemini-2.5-flash`.
- Las operaciones de la API se exponen como **`http_request` en `tool_configurations`**: una
  tool por operación elegida, con `base_url` fijo, header de auth fijo
  (`Authorization: Bearer ${HUBSPOT_PRIVATE_APP_TOKEN}` en el grafo entregado), y los
  parámetros que llena el LLM (`node_schema` con `fixed` para lo estático y campos `required`
  para lo dinámico).
- **Edges del agente generado:** `trigger → llm → output`, todos bare (la complejidad vive en
  `tool_configurations`, no en edges).
- Opcional (caso "explorar toda la API"): `enabled_tools: ["api_explorer"]` en el agente
  generado. v1.1 prioriza `http_request` tools por operación.

### Provider del agente generado
Generado = OpenAI (`gpt-4o`); builder = Gemini. (Ajustable; default sensato.)

## 4. Bloque C — Flujo seguro de prueba

### Colecta del secreto (nunca toca el LLM)
- El builder tiene `secure_suspend_allowed: true` → la tool `ask_secret`.
- Al ir a probar, llama `ask_secret` pidiendo `hubspot_private_app_token`. En serve esto
  **suspende** la corrida y devuelve la pregunta; el operador manda el token por
  **`POST /resume`** (canal Q/A, mismo `x-agent-session-id`) — **nunca como mensaje de chat**.
- El builder recibe de vuelta solo un **handle** `<sv_hubspot_private_app_token_8hex>`.

### Prueba real
- El builder hornea **el handle** en la auth del **grafo de prueba**
  (`Authorization: Bearer <sv_...>`) y lo corre con `probar_grafo`.
- Como el `agent_session_id` es estable, el **scope del secreto se propaga al `subgraph`**
  (confirmado: `subgraph.rs` propaga `agent_session_id`; `inject_secrets` resuelve el handle
  por `(agent_session_id, handle)`). El valor real se inyecta en el **egress** hacia la API,
  la llamada real (lectura) se ejecuta, y la respuesta vuelve **enmascarada**.
- El builder le muestra al usuario el resultado real (status + muestra de datos), sin el token.

### Entrega (dos placeholders distintos, a propósito)
| | Grafo de PRUEBA (builder, en proceso) | Grafo ENTREGADO (otros lo corren) |
|---|---|---|
| Auth | `Bearer <sv_..._8hex>` (handle) | `Bearer ${HUBSPOT_PRIVATE_APP_TOKEN}` (env var) |
| Valor real desde | DB encriptada, por `agent_session_id` del builder | Variable de entorno, en el egress |
| Usa la DB de secretos | Sí | **No** |

El builder **swappea** la auth al entregar. **No se comparte la DB de secretos** entre builder
y agente creado: el handle es scoped a sesión + TTL 24h y no se puede shippear; el agente
creado lee el token de `HUBSPOT_PRIVATE_APP_TOKEN` en el env de su host. La DB encriptada se
usa solo transitoriamente durante la prueba del builder.

### Seguridad de efectos
Prueba **solo lectura** por defecto (ej. listar contactos). **Avisa y pide confirmación**
antes de cualquier escritura (crear/editar registros).

### Requisitos
`SECURE_VALUES_KEY`, `DATABASE_URL`, y `x-agent-session-id` estable en cada turno (mismo
requisito que la memoria conversacional — ver [v1 README](../../../tests/graphs/agents/graph_builder/README.md)).

### Caveat de seguridad consciente
Con la env var, el token del agente creado se resuelve en el egress pero **no se enmascara**
en logs (solo los handles de secure values se enmascaran). Aceptable para un host propio;
documentado para decisión consciente.

## 5. Alcance de la iteración

Actualiza artefactos existentes y agrega nuevos bajo `tests/graphs/agents/graph_builder/`:
- **Modificar** `system_message` (en `graph_builder.json`): regla A1 (edges bare), comportamiento
  A2 (alcance/followups/plan/prueba transparente/iterar), y guía de cuándo leer docs/usar
  `secure_suspend`.
- **Agregar tools al builder** (en `graph_builder.json`): fetch web (`tavily_client` y/o
  `http_request`), `api_explorer`, y `secure_suspend_allowed: true`.
- **Regenerar a edges bare** los ejemplos de `building-graphs-core` y de las 7 capability skills.
- **Nueva skill** `capability-api-integration` (leer docs → armar agente con http tools → auth
  por env var → probar seguro).
- **Actualizar README** con el flujo de API + secure_suspend/`/resume`.

## 6. Plan de pruebas (E2E real, no mocks)

Memoria del proyecto: E2E real, SSE a `/tmp/colmena_e2e/<nombre>.sse`, reporte amistoso, token
nunca impreso. Pasar `x-agent-session-id` estable en cada turno.

1. **Regla A1 sigue verde:** reconstruir el bot v1 con edges bare + `{{message}}`; correr y
   entregar; el grafo entregado se auto-ejecuta. (Asegura que A1 no rompió lo de v1.)
2. **Lectura de docs:** "quiero un agente que consulte contactos de HubSpot, acá está la doc
   <URL>" → el builder descarga la doc y propone el flujo con base_url + auth + endpoint de
   lectura correctos.
3. **Colecta segura:** el builder llama `ask_secret` → la corrida suspende → `POST /resume` con
   el token real (en `HUBSPOT_PRIVATE_APP_TOKEN`) → el builder recibe el handle. Verificar que
   el token **no** aparece en el contexto del LLM ni en el SSE.
4. **Prueba real:** el builder corre el grafo de prueba (handle) vía `probar_grafo` → llamada
   real de lectura a HubSpot → respuesta enmascarada → muestra resultado al usuario.
5. **Entrega:** el grafo entregado usa `${HUBSPOT_PRIVATE_APP_TOKEN}`; correrlo standalone con
   la env var seteada devuelve datos reales de HubSpot.
6. **Efecto real:** pedir una operación de escritura → el builder avisa/pide confirmación antes
   de probar.

## 7. Riesgos y mitigaciones
- **`secure_suspend` como tool suspendiendo/reanudando dentro de serve** — verificar en vivo
  (paso 3). Mitigación: el subgraph soporta HITL (SUSPENDED bubbles up).
- **Propagación del scope del secreto al `subgraph` de `probar_grafo`** — confirmado en código;
  re-verificar en vivo (paso 4); depende del `agent_session_id` estable.
- **El fetch de docs de HubSpot puede no alcanzar** (HTML pesado, multipágina) — mitigación:
  híbrido con `api_explorer` si hay spec; permitir que el builder navegue/pregunte.
- **A1 rompe patrones que antes usaban edges punteados** — mitigación: paso 1 del E2E + el
  patrón adaptador `python_script` documentado en `building-graphs-core`.

## 8. Backlog (post-v1.1)
- Ingesta de docs por **PDF** (attachments en serve).
- **Agente creado que use `secure_suspend` en runtime** (multi-tenant, cada usuario trae su
  token, lee de la DB encriptada bajo su propia sesión) — la Opción B descartada por ahora.
- Enmascarado de env vars en logs (hoy solo se enmascaran secure values).
- `guardar_grafo` (escribir a archivo), capacidades avanzadas, migración a ADP (de v1).
