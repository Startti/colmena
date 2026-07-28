# Gaps de la API pública `/v1` de ADP para autonomía de agentes

- **Fecha:** 2026-07-25
- **Autor:** daniel@startti.co (con Claude Code)
- **Para:** equipo de ADP (plataforma / API pública)
- **Contexto:** estamos construyendo un asistente de creación de agentes (Claude Code, skill `adp-prod-api`) que crea, corre y compone agentes de Colmena **exclusivamente vía la API pública `/v1`** (`Authorization: Bearer adp_...`). Este documento lista lo que **hoy no se puede hacer por `/v1`** y que hace falta para que un agente pueda operar el ciclo de vida completo de forma autónoma. Cada gap incluye el estado actual, la evidencia en código, el workaround interino (handoff al usuario) y una propuesta de endpoint.
- **Fuente de verdad:** código ADP en `apps/api/src/public-api/*`, `apps/api/src/agents/*`, `packages/shared/*`. Verificado el 2026-07-25.

---

## Resumen y prioridad

| # | Capacidad faltante | Gap en `/v1` | Prioridad | Propuesta |
|---|---|---|---|---|
| 1 | Editar un agente existente in-place | No hay `PATCH/PUT /v1/agents/:id` | **P0** | `PATCH /v1/agents/:id` |
| 2 | Leer el grafo/package de un agente | `GET /v1/agents` solo da `id/name/publishStatus` | **P0** | `GET /v1/agents/:id/package` |
| 3 | Publicar un agente como asset reutilizable | Solo canvas (sesión de usuario) | **P1** | `POST /v1/assets/publish` + `GET /v1/assets` |
| 4 | Referenciar un asset por `assetVersionId` desde un package | El import de `/v1` no lo resuelve | **P1** | Resolver `assetVersionId` en import (rebind) |
| 5 | Borrar un agente | No hay `DELETE /v1/agents/:id` | **P2** | `DELETE /v1/agents/:id` |
| 6 | CRUD de modalidades (webhook/schedule/trigger) | No expuesto para autoría autónoma | **P2** | `.../v1/agents/:id/{webhooks,schedules,triggers}` |
| 7 | Listar las sesiones de un agente | `GET /v1/sessions/:id` es de a una | **P2** | `GET /v1/agents/:id/sessions` |
| 8 | Crear/revocar platform keys | Solo sesión de usuario (backoffice) | **P3** | (posiblemente intencional — ver nota) |

**Lectura rápida:** los dos P0 (leer + editar) son los que más duelen — sin ellos el asistente **no puede iterar sobre un agente ya creado**; solo puede crear agentes nuevos. Los P1 habilitan la **reutilización/composición real** (hoy limitada a inline). Los P2 son ciclo de vida. El P3 puede ser intencional por seguridad.

---

## Detalle por gap

### 1. Editar un agente existente in-place — **P0**
- **Estado:** `V1Controller` no expone `PATCH`/`PUT`. El único camino de escritura es `POST /v1/run` en modo package, que por idempotencia (`hash = sha256(package)`) **reusa** el agente si el hash coincide o **crea uno nuevo** (nuevo `agentId`) si cambia un byte. No existe edición in-place: cambiar el prompt de un agente publicado genera otro agente.
- **Impacto:** el asistente no puede "ajustar el system_message del agente X"; solo puede ofrecer crear una versión nueva (nuevo id) o pedir al usuario que edite en el canvas.
- **Evidencia:** `apps/api/src/public-api/infrastructure/http/v1.controller.ts` (rutas: `run`, `agents` GET, `triggers`, `cancel`, `sessions/:id`, `attachments`); idempotencia en `apps/api/src/agents/groups/application/group-dag-write.service.ts` (`importColmenaPackage`).
- **Workaround interino:** handoff — el usuario edita en el canvas; o el asistente re-crea vía package (asumiendo nuevo id).
- **Propuesta:** `PATCH /v1/agents/:id` que acepte un package parcial o updates de campos (`system_message`, `model`, `provider_key_id`, `enabled_tools`, …), preservando `agentId` y sesiones.

### 2. Leer el grafo/package de un agente — **P0**
- **Estado:** `GET /v1/agents` devuelve `{ id, name, publishStatus }`. No hay forma de recuperar el `colmena { nodes, edges }` ni el package de un agente existente.
- **Impacto:** el asistente no puede introspeccionar ni razonar sobre un agente que no creó él mismo en esta sesión; no puede "ver cómo está armado el agente X para modificarlo".
- **Evidencia:** `apps/api/src/public-api/infrastructure/http/v1.controller.ts` (handler de `GET /v1/agents`).
- **Workaround interino:** el usuario exporta el JSON del agente desde el canvas y lo pega.
- **Propuesta:** `GET /v1/agents/:id/package` → el package v3 (o el `colmena` pelado) del agente, respetando el scope/allowlist de la key.

### 3. Publicar un agente como asset reutilizable — **P1**
- **Estado:** publicar un asset = `POST /agents/assets/publish` en `AssetsController`, gated por `SessionAuthGuard` (sesión de usuario, canvas/backoffice). **No está registrado bajo `PublicApiModule`** — no hay equivalente `/v1`, ni forma de obtener un `assetVersionId` con una key `adp_`.
- **Impacto:** la visión de "construir agentes simples, publicarlos y orquestarlos como assets reutilizables" **no es alcanzable por `/v1`**. Hoy la única composición vía `/v1` es inline (`child_graph_inline`), sin reuso por-referencia ni versionado en runtime.
- **Evidencia:** `apps/api/src/agents/assets/infrastructure/http/assets.controller.ts` (`@Controller('agents/assets')`, `SessionAuthGuard`, `POST publish`); `apps/api/src/public-api/public-api.module.ts` (controllers `/v1` — sin assets).
- **Workaround interino:** composición inline; o el usuario publica y compone en el canvas.
- **Propuesta:** `POST /v1/assets/publish` (desde un `agentId` o package) → `{ assetVersionId }`, y `GET /v1/assets` para descubrir los del workspace.

### 4. Referenciar un asset por `assetVersionId` desde un package — **P1**
- **Estado:** aunque se obtuviera un `assetVersionId`, el import de `/v1` **nunca lo resuelve**. `importColmenaPackage` solo re-bindea `external_refs` (api_keys/db/ws por nombre) y persiste el colmena **verbatim**; `validateColmenaShape` valida solo shape de nodos/aristas de nivel superior, no inspecciona `config`. La única resolución de `assetVersionId → child_graph_inline` es `GroupsService.buildAssetSubgraph`, alcanzable solo por `compileGraph` (ruta del canvas, sesión). Comentario explícito: *"compileGraph drops assetVersionId/paramValues (only child_graph_inline survives)"*.
- **Impacto:** un `subgraph` con `assetVersionId` mandado por `/v1` **pasa la validación en silencio y falla al correr** (el executor de Colmena solo entiende `child_graph_path`/`child_graph_inline`). Es una trampa: 200 en import, error en runtime.
- **Evidencia:** `apps/api/src/agents/groups/application/group-dag-write.service.ts` (`importColmenaPackage`, `validateColmenaShape`); `apps/api/src/agents/groups/application/groups.service.ts` (`buildAssetSubgraph`); `packages/shared/src/lib/dag/colmena-config-to-adp.ts` (comentario del drop).
- **Workaround interino:** usar `child_graph_inline` (embeber el grafo hijo verbatim) — sí funciona por `/v1`.
- **Propuesta:** que el import de `/v1` resuelva `assetVersionId` (rebind por workspace, como `external_refs`), materializándolo a `child_graph_inline` en el persist. Va de la mano del gap #3.

### 5. Borrar un agente — **P2**
- **Estado:** `V1Controller` no expone `DELETE`.
- **Workaround interino:** handoff al backoffice.
- **Propuesta:** `DELETE /v1/agents/:id` (respetando scope/allowlist; posible soft-delete).

### 6. CRUD de modalidades (webhook/schedule/trigger) — **P2**
- **Estado:** las modalidades no son nodos; se configuran vía endpoints scoped al agente que hoy no forman parte de la superficie de autoría autónoma de `/v1` (el `POST /v1/agents/:id/triggers/:name` **dispara** un trigger, no lo **crea**).
- **Workaround interino:** el usuario las crea en el backoffice; la skill las documenta como referencia.
- **Propuesta:** CRUD por `/v1` scoped al agente: `POST/GET/PATCH/DELETE /v1/agents/:id/{webhooks,schedules,triggers}`.

### 7. Listar las sesiones de un agente — **P2**
- **Estado:** `GET /v1/sessions/:id` devuelve una sesión puntual; no hay listado.
- **Workaround interino:** trackear `sessionId`/`sessionKey` del lado del cliente.
- **Propuesta:** `GET /v1/agents/:id/sessions` (paginado).

### 8. Crear/revocar platform keys — **P3**
- **Estado:** `PlatformKeyController` (`POST/GET/DELETE /workspace/:id/platform-keys`) es sesión de usuario, no la key `adp_`.
- **Nota:** probablemente **intencional** — que una API key no pueda mintear otras keys es una buena postura de seguridad. Se documenta como límite, no necesariamente como algo a cambiar.

---

## Nota de diseño: qué SÍ funciona hoy por `/v1`

Para que el equipo tenga el contraste completo, lo que hoy es plenamente autónomo por `/v1`:
- **Crear + correr** un agente desde package (`POST /v1/run` con `package`), incluyendo `child_graph_inline` para composición.
- **Correr** un agente existente por `agentId`, con streaming SSE, sesiones (`sessionId`/`sessionKey`) y suspend/resume.
- **Disparar** triggers ya definidos (`POST /v1/agents/:id/triggers/:name`).
- **Descubrir** agentes (`GET /v1/agents`), provider-keys y modelos (`GET /v1/provider-keys`, `/:id/models`).
- **Adjuntos** (`POST /v1/attachments`), **cancelar** (`POST /v1/cancel`), **ver** una sesión (`GET /v1/sessions/:id`).

El gap central es el **ciclo de vida de autoría** (leer → editar → versionar → componer por referencia → borrar), no la ejecución.
