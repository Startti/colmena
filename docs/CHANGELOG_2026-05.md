# Cambios recientes — auditoría 2026-05-03 al 2026-05-20

> **Generado:** 2026-05-17 · **Actualizado:** 2026-05-20 (multimedia generation pipeline + artifacts unification)
> **Alcance:** Commits sobre `develop` desde 2026-05-11 (los anteriores a esta ventana fueron auditados previamente).
> **Total:** ~90 commits committed + 31 modified files + 20 new files pending commit (feature 8) agrupados en 12 features.

## Cómo leer este documento

Una sección por feature. Cada sección contiene:

- **Qué cambió** — 1-2 frases describiendo el efecto observable.
- **Documentación de referencia** — links a spec, plan, dev guide y schema del nodo.
- **Commits** — el rango o lista (los nombres en `feat(...)` siguen Conventional Commits).
- **Estado** — done / spec-only / partial.

Al final hay una **matriz de completitud** por feature y una sección de **gaps / acciones recomendadas**.

---

## 1. ProviderKind rename: `gemini` → `google` (ok)

**Qué cambió.** El identificador del provider de Google cambió de `gemini` a `google` en todo el stack — `ProviderKind`, parser de configs JSON, factories de adapters, test graphs y developer guides. Tu graph JSON ahora debe usar `"provider": "google"` (el string `"gemini"` ya no se reconoce).

**Documentación:**
- Plan: [docs/superpowers/plans/2026-05-11-rename-provider-gemini-to-google.md](superpowers/plans/2026-05-11-rename-provider-gemini-to-google.md) — implementación tarea por tarea.
- Schema: [docs/node_configurations.json](node_configurations.json) → lista canónica de providers contiene `"google"`.
- Dev guides: developer_guides actualizados para usar `"google"` consistentemente.

**Commits:** `5204933`, `bfd93b1`, `dc8897a`, `8099bb2`, `0d11dd9`, `d13a597`, `c4be767`, `796a2d8`, `4674e14` (2026-05-11).

**Estado:** ✅ Done. Backwards-incompatible si tenés graphs viejos hardcodeando `"gemini"` — buscalo y reemplazalo.

---

## 2. `secure_suspend_allowed` flag en `llm_call`

**Qué cambió.** Nuevo flag boolean `secure_suspend_allowed: true` en `llm_call.config` que auto-registra una tool sintética `ask_secret` (backed por `secure_suspend`) con descripción y `node_schema` canónicos — el LLM puede pedir credenciales al usuario sin que el graph author tenga que escribir un `tool_configurations` entry. Si la entrada explícita existe, gana sobre el flag (no-op).

**Documentación:**
- Plan: [docs/superpowers/plans/2026-05-11-secure-suspend-allowed-flag.md](superpowers/plans/2026-05-11-secure-suspend-allowed-flag.md)
- Dev guide: [docs/developer_guide/13_security_strategy.md](developer_guide/13_security_strategy.md) → sección "Mode B — `secure_suspend_allowed: true` (recomendado)" (línea 396).
- Schema: [docs/node_configurations.json](node_configurations.json) → campo `secure_suspend_allowed` en `llm_call.config_fields`.

**Commits:** `0d749ed`, `61ad78c`, `fd96098`, `aa3605e`, `1d56c1c`, `0d2dd34` (2026-05-11).

**Estado:** ✅ Done.

---

## 3. Secure values: sliding TTL + outbound masking + leakage prevention

**Qué cambió.** Tres cambios coordinados al stack de secure-values:

1. **Sliding TTL.** El postgres repo `decrypt` ahora hace `UPDATE … RETURNING` que extiende atómicamente `expires_at` en cada uso (24h sliding). El cleanup periódico borra solo filas expiradas (`cleanup_expired_for_run`) en lugar del barrido total al final del run.
2. **Handle hardening.** `persist_secret` agrega un sufijo random de 8 hex chars, y `secure_suspend` rechaza valores con menos de 4 chars.
3. **Outbound masking.** `SecureValueService::inject_secrets` retorna el mapeo `decrypted → handle` para que `DagToolExecutor::execute_inner` enmascare cada respuesta de tool (Ok y Err) antes de que llegue al agente — los secretos no se filtran al LLM via tool outputs.

**Documentación:**
- Spec: [docs/superpowers/specs/2026-05-11-secure-values-sliding-ttl-design.md](superpowers/specs/2026-05-11-secure-values-sliding-ttl-design.md) — diseño completo (status: Design).
- Plan: [docs/superpowers/plans/2026-05-11-secure-values-sliding-ttl.md](superpowers/plans/2026-05-11-secure-values-sliding-ttl.md) — implementación tarea por tarea.
- Dev guide: [docs/developer_guide/13_security_strategy.md](developer_guide/13_security_strategy.md) cubre Secure Values en general; **falta sección específica sobre sliding TTL + outbound masking** (ver gap #1 abajo).

**Commits:** `9974631` (refactor inject_secrets), `4e27df7` (mask_outbound helper), `e6d6970` (mask en DagToolExecutor), `1f87a6a` (integration test), `58a7992` (cross-pool count assertion), `204346f` (secret survives cleanup), `60d31b1` (clippy fix) (2026-05-11).

**Estado:** ✅ Done en código y tests. ⚠️ Doc usuario incompleta — el dev guide menciona Secure Values pero no detalla las garantías nuevas (sliding TTL, masking, min-length).

---

## 4. LLM temporal & geographic context injection

**Qué cambió.** Inyección automática al inicio del `system_message` de cada `llm_call` de un bloque con fecha/hora local (**ISO 8601** + echo human-readable), timezone IANA, location free-text, y locale BCP 47. Tres fields opcionales al root del graph JSON (`timezone`, `location`, `locale`) propagan vía `__colmena_*` a todos los nodos; el `llm_call` los lee y llama al helper `format_temporal_context_block` que renderiza el bloque con `chrono` + `chrono-tz`. Defaults: `America/Bogota` / `Bogotá, Colombia` / `es-CO`. IANA inválido cae a Bogotá silenciosamente y reescribe el label visible para que `(timezone, offset)` queden coherentes.

Ejemplo del bloque renderizado:

```
## Temporal & Geographic Context
Current date and time: 2026-05-18T10:34:00-05:00 (Sunday, May 18, 2026, 10:34 AM)
Timezone: America/Bogota (UTC-5)
Location: Bogotá, Colombia
Locale: es-CO
```

**Standards seguidos** (revisión hecha el 2026-05-18 antes de implementar):
- ISO 8601 / RFC 3339 — mismo formato que Anthropic Claude inyecta en sus system prompts.
- IANA TZDB para timezones (universal).
- BCP 47 (RFC 5646) para locale (estándar formal de "idioma+región", usado por iOS/Android/CLDR).

**Documentación:**
- Spec: [docs/superpowers/specs/2026-05-12-llm-temporal-geographic-context-design.md](superpowers/specs/2026-05-12-llm-temporal-geographic-context-design.md) — status: approved (revisado 2026-05-18 para alinear a ISO 8601 + BCP 47).
- Plan: [docs/superpowers/plans/2026-05-18-llm-temporal-geographic-context.md](superpowers/plans/2026-05-18-llm-temporal-geographic-context.md) — 7 tasks TDD ejecutadas.
- **Dev guide: [docs/developer_guide/35_temporal_geographic_context.md](developer_guide/35_temporal_geographic_context.md)** — incluye diagrama detallado del pipeline end-to-end (JSON → struct → inputs → helper → bloque → system message → request al provider).
- Index: [docs/DEVELOPER_GUIDE.md](DEVELOPER_GUIDE.md) entrada #35.
- Schema: [docs/node_configurations.json](node_configurations.json) → nueva sección `graph_root_fields` con los 3 campos.
- Test graph: `tests/graphs/agents/llm_temporal_context_test.json` — smoke con Gemini Flash + Bogotá + es-CO, verificado e2e (modelo respondió en español citando fecha actual y Bogotá).

**Commits:** `4d0cca5` (spec + plan revision aligning to ISO 8601/BCP 47), `9a3bc0b` (chrono-tz dep), `6cd8e79` (Graph struct), `72e8304` (helper TDD), `be99117` (dead_code fix interino), `bc4c513` (engine injection), `6bbfba8` (wire en llm.rs + dead_code removal), `1581a6e` (smoke graph), `1a6bafb` (docs/node_configurations.json), `<this commit>` (dev guide 35 + diagrama + actualización de este changelog) (2026-05-18).

**Estado:** ✅ Done + verificado e2e contra Gemini Flash real.

---

## 5. `load_attachment` — feature base

**Qué cambió.** Nuevo synthetic tool `load_attachment` que permite al LLM cargar documentos previamente subidos en un turno anterior de la misma `agent_session_id`, sin re-adjuntarlos en cada call. Cuatro componentes nuevos:

1. **Domain layer** — `AttachmentRegistry` trait, `ConversationAttachment`, `AttachmentSource` enum (SignedUrl/Path/Inline), `AttachmentError`, `generate_attachment_id` (SHA-256 stable id).
2. **Persistencia** — tabla `conversation_attachments` (Postgres + SQLite), con migraciones idempotentes y RLS-ready (keyed por `agent_session_id`).
3. **DAG tool executor** — intercepta tool calls a `load_attachment` y emite un sentinel `LOAD_ATTACHMENT` que `AgentService` traduce en un mensaje `user` sintético con el archivo persistido en la historia.
4. **Auto-registración + recuperación** — `llm_call` auto-registra los `files[]` que recibe; cuando el `provider_file_id` expira (Gemini 48h), re-sube silenciosamente desde el `source` original.

Más un flag `attachments_enabled` (default `true`) para opt-out por nodo, y `ATTACHMENTS_SYSTEM_PRELUDE` auto-inyectado al system_message cuando hay attachments en el catálogo.

**Documentación:**
- Spec: [docs/superpowers/specs/2026-05-13-load-attachment-design.md](superpowers/specs/2026-05-13-load-attachment-design.md) — status: Approved for planning.
- Plan: [docs/superpowers/plans/2026-05-13-load-attachment.md](superpowers/plans/2026-05-13-load-attachment.md)
- Dev guide: [docs/developer_guide/31_load_attachment.md](developer_guide/31_load_attachment.md) — sección principal del documento.
- Index: [docs/DEVELOPER_GUIDE.md](DEVELOPER_GUIDE.md) entrada #33.
- Schema: [docs/node_configurations.json](node_configurations.json) → campo `attachments_enabled` en `llm_call`.
- Test graphs: `tests/graphs/agents/load_attachment_{basic,subgraph,opt_out}.json`.

**Commits:** `8483261` → `2b097d0` (2026-05-13). Highlights: `4f73c49` (wire registry), `e4e6056` (tool definition), `b443ee0` (executor intercept), `df1b696` (ReAct loop handler), `aa75ad0` (system prelude), `bc048da` (silent re-upload), `2b097d0` (sqlite config key fix + real re-upload wiring).

**Estado:** ✅ Done.

---

## 6. Attachment auto-summary

**Qué cambió.** Cuando `files[]` registra un archivo **sin `description`**, el motor genera una descripción de 1 línea con el cheap-tier del provider (Gemini Flash / GPT-4o-mini / Claude Haiku) **en paralelo** con el answer call del turno, vía `tokio::join!` + `tokio::task::JoinSet`. La descripción se persiste en `conversation_attachments.description` y aparece en el catálogo del tool `load_attachment` desde el siguiente turno. Best-effort: failures (extracción, LLM error, timeout) dejan `description = null` y caen a `filename` como fallback.

Stack:
- Extracción local de texto (`pdf-extract` para PDFs, UTF-8 decode para `text/*`).
- Truncado a 5000 chars (config `summary_max_chars`).
- Imágenes van como vision input directo, sin extracción.
- Cheap-tier por provider mapeado en `provider_cheap_tier()`.
- Per-call timeout via `tokio::time::timeout(config.timeout, …)`.
- `JoinSet` aborta tasks pendientes si la batch excede el timeout o el caller cancela.

**Documentación:**
- Spec: [docs/superpowers/specs/2026-05-14-attachment-auto-summary-design.md](superpowers/specs/2026-05-14-attachment-auto-summary-design.md) — status: Approved for planning, sección de concurrency refleja JoinSet + two-layer timeout.
- Plan: [docs/superpowers/plans/2026-05-14-attachment-auto-summary.md](superpowers/plans/2026-05-14-attachment-auto-summary.md) — 12 tasks TDD.
- Dev guide: [docs/developer_guide/31_load_attachment.md](developer_guide/31_load_attachment.md) → sección "Auto-generated descriptions (auto-summary)" (línea 100+) con pipeline, per-MIME table, config table, failure matrix, cost analysis y limitaciones.
- Index: [docs/DEVELOPER_GUIDE.md](DEVELOPER_GUIDE.md) entrada #33 menciona auto-summary explícitamente con los 5 config fields.
- Schema: [docs/node_configurations.json](node_configurations.json) → 5 campos nuevos:
  - `summary_enabled` (bool, default `true`)
  - `summary_max_chars` (int, default 5000)
  - `summary_model` (string, default cheap-tier del provider)
  - `summary_timeout_secs` (int, default 15)
  - `summary_max_output_chars` (int, default 200)
- Test graph: `tests/graphs/agents/load_attachment_auto_summary.json`.

**Commits:** `3cdb244` (spec), `c3196b4` (plan), `d0c1c2b` (pdf-extract dep), `db61a84` → `68de91d` (12 tareas + fix + docs) (2026-05-14). Highlights: `f75147e` + `f243620` (text extraction + MIME-param strip), `abfb250` (generator adapter), `4540cac` (tokio::join! wiring), `9ea5d02` (JoinSet + per-call timeout + skip-inline fix), `68de91d` (full reference docs).

**Estado:** ✅ Done.

---

## 7. `path:` y `data:` registration fix (auto-summary path coverage)

**Qué cambió.** Bugfix descubierto durante el integration testing del two-agent flow: el bloque de upload-al-provider en `llm.rs` estaba gateado por `FileSource::SignedUrl`, así que archivos con `path:` o `data:` (que `parse_file_entries` mapea a `FileSource::InlineBytes`) **nunca pasaban por la auto-registración** — el LLM en el turno actual igual los veía, pero no aparecían en `conversation_attachments` para turnos siguientes ni para auto-summary. El fix:

1. Extiende el gate en `llm.rs:779` para disparar también con `InlineBytes`.
2. Agrega un brazo `InlineBytes` en `LlmCallUseCase::resolve_one` (cache path) que stream-uploadea los bytes al provider via `upload_streaming` y dedupea intra-request por `document_id`.
3. Agrega el brazo paralelo en el no-cache fallback de `llm.rs`.

Después del fix, `path:` y `data:` se vuelven `Uploaded` antes de la registración, así que el resto del pipeline los trata como cualquier otra fuente. Verificado end-to-end con Postgres real: registración correcta, `source_kind = path`, auto-summary populated, reader del segundo turno llama `load_attachment` y describe el doc.

**Documentación:**
- Spec / issue: [docs/superpowers/specs/2026-05-16-load-attachment-path-data-registration-issue.md](superpowers/specs/2026-05-16-load-attachment-path-data-registration-issue.md) — status: **Fixed in plan**.
- Plan: [docs/superpowers/plans/2026-05-16-load-attachment-path-data-fix.md](superpowers/plans/2026-05-16-load-attachment-path-data-fix.md) — 6 tasks TDD.
- Dev guide: [docs/developer_guide/31_load_attachment.md](developer_guide/31_load_attachment.md) → limitación #0 (path/data no registra) removida; item #1 reformulado para clarificar que `path:` ya funciona y solo queda `data:` sin auto-summary.
- Test graphs nuevos: `tests/graphs/agents/load_attachment_two_agents_step{1_upload,2_read,3_isolated}.json` — tres pasos secuenciales con el mismo `--agent-session-id`.

**Commits:** `0d17246` (issue spec + test graphs), `89bb6e3` (plan), `3ade96d` (failing tests), `34cd7c2`, `7bcb54b`, `806e862` (fix), `f438e8a` (docs), `a6c77e7` (e2e revert) (2026-05-16).

**Estado:** ✅ Done + verificado e2e contra Postgres real (`agent_path_fix_001`).

---

## Inline data: auto-summary (v2)

**Date.** 2026-05-18.
**Status.** Done.
**Why.** Closes BACKLOG entry #1.

When a file enters via `data:` (base64 inline), the bytes used to be consumed by the upload pipeline so the auto-summary path skipped them. Now `resolve_one` clones the bytes a second time into a `retained_inline_bytes` field on `FileData`, and the auto-register loop passes them to `acquire_bytes` for the inline source. End-to-end verified against Gemini Flash: `source_kind = inline` rows now carry a non-null description.

Commits: aeea269 (RED), cc924a3 (domain field + resolve_one), a3053cd (GREEN), 50a021d (E2E graph).
Test graph: `tests/graphs/agents/load_attachment_inline_summary.json`.

---

## Lifecycle simplification — TTL passive + process death

**Date.** 2026-05-18.
**Status.** Done.
**Why.** Closes Plan-C TODOs; removes dead code.

The `ConversationLifecycleBus`, `ConversationLifecycleSubscriber` trait, `with_conversation_lifecycle` builder, `subscribe_lifecycle` plumbing, and two TODOs in `run_use_case.rs` were never wired to an external signal. ADP's worker (long-lived axum service holding `Arc<ColmenaEngine>`) processes Redis jobs one at a time and has no notion of "conversation closed". Replaced with passive TTL sweeping inside `SessionRegistry` (sweep period 60s, eviction at 15min idle / 1h max), already implemented and tested but never enabled in production. The sweeper now starts automatically inside `ApiExplorerNode::new()` when constructed within a tokio runtime.

Commit: 8a6a17a. Net delta: −174 LOC.

---

## api_explorer flag-only activation

**Date.** 2026-05-19.
**Status.** Done.
**Why.** Frontend UX: single boolean toggle replaces the verbose `tool_configurations` block.

Three coordinated changes let graphs activate the `api_explorer` toolkit with just `enabled_tools: ["api_explorer"]`:

1. `available_tools()` (catalog) auto-expands the 5 sub-tools when no `tool_configurations` entry exists.
2. The `enabled_tools` filter in `LlmNode` matches entries as toolkit prefixes (exact OR `{alias}__*`).
3. The dispatch path in `DagToolExecutor::execute_inner` synthesises a default `ToolConfiguration` (empty `node_config`, `expose_sub_tools: All`) when no explicit entry exists for `api_explorer`.

Other toolkits (`tavily_client`, future `browser`) are unaffected and still require explicit `tool_configurations` because they need per-instance config (e.g., `api_key`).

E2E verified against OpenAI gpt-4o-mini + real petstore3.swagger.io spec. Test graph: `tests/graphs/web/api_explorer_petstore_flag_only.json`.

**References.** Canonical reference rewritten on 2026-05-19: [`docs/developer_guide/25_web_nodes.md` → Api Explorer](developer_guide/25_web_nodes.md) now contains the dispatch-flow ASCII diagram, the data-injection table (what gets injected vs not under the toolkit path), the 5 sub-tool full schema, and the lazy-vs-eager clarification. `09_tool_calling.md` and `CLAUDE.md` now present the flag-only pattern as recommended; `node_as_tools_reference.json` adds `recommended_activation`, `data_injection`, `is_lazy_tool`, `dispatch_flow` fields (and fixes wrong sub-tool param names: `load_spec` takes `url` not `spec_url`; `get_endpoint_details` / `build_http_request` take `operation_id` not `path`+`method`).

Commits: 131c540, 3f1a9f3, dd47a54.

---

## 8. Multimedia generation pipeline + artifacts unification (2026-05-19/20)

**Qué cambió.** Tres nodos nuevos para generar media (`image_generation`, `image_edit`, `tts`), un sistema de storage abstraído por trait con 3 adapters (in-memory para CI, HTTP-callback para prod via host application → GCS, HTTP local con server `axum` para dev), y la unificación de outputs generados con el `AttachmentRegistry` existente — el agente puede ABRIR sus propias generaciones via `load_attachment`, ENCADENAR ediciones por `attachment_id`, y ENVIAR los bytes a endpoints externos via `$attachment:<key>` placeholder en `http_request`, todo sin que el LLM vea nunca bytes binarios crudos.

El principio arquitectónico que se locked-in: **el contexto del LLM nunca contiene bytes binarios**. Tres mecanismos lo enforcen:
- **Output direction** (tool → LLM): URLs cortas en `read_url` (handles, signed GCS, o `http://127.0.0.1` según adapter).
- **Input direction** (LLM → tool): el LLM pasa `$attachment:<storage_key>` y el engine resuelve a `data:` URI antes de salir.
- **Echo direction** (external endpoint → tool result → LLM): scrubber universal en `DagToolExecutor` reemplaza `data:*;base64,*` por `[binary elided: mime=X, encoded_size=N bytes]` y trunca strings > 50 KB (configurable via `max_tool_result_bytes` en `llm_call.config`).

Para dev/prod symmetry agregamos `COLMENA_LOCAL=true|false` como guard rail explícito: `true` fuerza `LocalHttpStorageAdapter` con defaults sanos (`/tmp/colmena-out`, port 8765); `false` fuerza `HttpCallbackStorageAdapter` y **panica loud** si faltan callback URL/secret (previene "deploy a prod sin el callback configurado y silenciosamente cae a in-memory"). Cada modo emite `tracing::info!(target: "colmena::engine", "storage_mode_selected …")` al startup.

**Nodos** (todos siguen el patrón inputs-over-config para campos LLM-controllable y registran sus outputs en `AttachmentRegistry` con `provider: Generated`):

| Nodo | Provider(s) | Endpoint | Notas |
|---|---|---|---|
| `image_generation` | OpenAI (gpt-image-1, dall-e-3), Google Vertex Imagen 4 | OpenAI `/v1/images/generations` o Vertex `:predict` | Vertex auth via `yup-oauth2` (service-account JWT exchange, token cacheado ~50min) |
| `image_edit` | OpenAI gpt-image-1 / dall-e-2 | `/v1/images/edits` multipart | `source_url` acepta `data:`, `http(s)://`, `local://<key>` (resuelve via storage) |
| `tts` | OpenAI tts-1/gpt-4o-mini-tts, ElevenLabs eleven_multilingual_v2, Google gemini-2.5-flash-preview-tts | varía | Factory dispatch via trait `TtsRepository` |

**Side effects de calidad** durante este feature también arreglamos dos bugs heredados:

- `tool_configurations` parse failure ahora **fail-hard con mensaje pedagógico** (antes: silent log + tools array vacío — modelo "improvisaba" tool calls como JSON inline en texto, hard to debug)
- `NodeSchemaField.type` ahora es opcional cuando `fixed` está presente (antes: error críptico `missing field 'type'` para schemas que tenían `{"fixed": "openai"}` sin `"type": "string"`)

**Documentación:**
- Spec/plan: [`docs/superpowers/plans/2026-05-19-multimedia-generation-nodes.md`](superpowers/plans/2026-05-19-multimedia-generation-nodes.md) — diseño original + status banner + delta de shipped-vs-planned.
- Dev guide: [`docs/developer_guide/32_multimedia_generation.md`](developer_guide/32_multimedia_generation.md) — onboarding completo con architectural invariant, URL strategy table, tool configurations, $attachment placeholder, cross-provider lazy upload flow, COLMENA_LOCAL setup, troubleshooting.
- Schema: [`docs/node_configurations.json`](node_configurations.json) → entradas nuevas para `image_generation`, `image_edit`, `tts` + categoría `media` + nota sobre `$attachment:<key>` en `http_request`.
- Ports: [`docs/agent_context/node_ports_reference.md`](agent_context/node_ports_reference.md) → 3 nodos nuevos en la tabla.
- Sample graphs: `tests/graphs/media/{image_generation_basic,tts_basic,image_edit_basic,image_gen_then_edit}.json` y `tests/graphs/agents/{multimedia_agent,multimedia_agent_with_load}.json`.

**Tests:** +26 unit tests entre storage, scrubber, image_generation, image_edit, tts, http $attachment, env guard rail, y cross-prov lazy upload. Total suite: **852 pass / 0 fail**. Smoke E2E verificado contra OpenAI gpt-image-1 real (LocalHttpStorageAdapter mode + scrubber active): gen → http_post → finish sin rate limits.

**Pendiente — bloqueante para producción.** Wiring del lado de la host application (downstream private repo que consume esta librería): endpoint `POST /internal/gcs/sign-put` + guard de service-to-service auth + env vars del worker (`COLMENA_LOCAL=false` + `COLMENA_STORAGE_CALLBACK_URL` + `COLMENA_STORAGE_CALLBACK_SECRET`) + parsing del tool output en el handler que persiste mensajes → rows de attachment table + schema migration con enum `source` (user / image_gen / tts / image_edit). El lado colmena ya implementa el client contract; el lado server es responsabilidad del downstream host.

**Commits.** Pendientes de commit en `develop` — branch tiene 31 archivos modificados + 20 archivos nuevos sin commitear al momento de escribir esta entrada.

---

## 9. `sql_query` — auto-creación de `allowed_schemas` faltantes (2026-05-28)

**Qué cambió.** El nodo `sql_query` ahora **provisiona los schemas listados en `permissions.allowed_schemas`** durante la inicialización: revisa uno por uno y crea (`CREATE SCHEMA IF NOT EXISTS`, identificador quoteado) los que no existen. Es **operator-driven** — los nombres vienen de la config fija del nodo, no del LLM —, así que **no relaja el bloqueo de `CREATE SCHEMA` emitido por el LLM** en un query (sigue bloqueado por el static validator). Controlado por el nuevo flag `permissions.create_schemas_if_missing`:

- **Default `true`**: si el flag está ausente en el graph JSON, se asume `true` y se crean los schemas faltantes.
- **`false`**: comportamiento legacy — `allowed_schemas` es solo allowlist de validación.
- **Check-then-create**: los schemas que ya existen nunca se re-crean, así un agente read-only apuntando a schemas existentes no requiere privilegio `CREATE`.
- **Hard-fail en init**: si un schema faltante no se puede crear (p. ej. el rol de BD no tiene `CREATE`), la inicialización del nodo falla con `Failed to create schema '<name>': ...` y el nodo no arranca.
- `information_schema` y `pg_catalog` nunca se consideran faltantes ni se crean.

**Documentación:**
- Plan: [docs/superpowers/plans/2026-05-28-sql-node-auto-create-allowed-schemas.md](superpowers/plans/2026-05-28-sql-node-auto-create-allowed-schemas.md)
- Dev guide: [docs/developer_guide/23_sql_node.md](developer_guide/23_sql_node.md) → tabla "Permissions Object" + subsección "Operator-Driven Schema Provisioning".
- Schema: [docs/node_configurations.json](node_configurations.json) → `sql_query.permissions.create_schemas_if_missing` (default `true`).
- Tool reference: [docs/node_as_tools_reference.json](node_as_tools_reference.json) → nota de provisioning en `sql_query`.
- Test graph: `tests/graphs/agents/sql_provision_schema_tool.json` (LLM tool, schema fresco).

**Archivos tocados:** `domain/sql_permissions.rs` (field + accesores `create_schemas_if_missing()` / `allowed_schemas_iter()`), `domain/sql_ports.rs` (`missing_schemas` + `create_schema` en `SqlConnectionPort`), `infrastructure/sql_pool_adapter.rs` (impl + tests `#[ignore]` con `TEST_DATABASE_URL`), `infrastructure/nodes/sql.rs` (paso de provisioning en `do_initialize_inner`).

**Estado:** ✅ Done. Verificado end-to-end contra Postgres real (dev/GCP): schema inexistente → grafo ejecutado con colmena → schema presente en la BD. Flag ausente y `false` también verificados.

> **Sweep ADP:** agrega dos métodos al trait interno `SqlConnectionPort`. No hay impls externos (el único impl es `PgPoolAdapter`), así que no rompe el worker de ADP. No cambia `EngineConfig`/`ColmenaEngine`.

---

## 10. Layered tool context — policy + node-type guide + tool-scoped skills (2026-05-29)

**Qué cambió.** Cada nodo usado como tool LLM ahora recibe, de forma
automática, un **bloque de contexto** compuesto por: (1) su description,
(2) política derivada de su fixed config (vía un hook nuevo en
`ExecutableNode::tool_description_supplement`), (3) la guía de
best-practices del node-type (una SKILL.md con `node_type: <name>` en el
frontmatter — una por node-type), y (4) un anuncio de las "skills
específicas" scoped a esa tool (`tool_configurations.<name>.skills`),
que el modelo puede cargar con `load_skill` solo después de hacer
`describe_tool` (visibility-gating sobre el `discovered_set`). En modo
eager o sin lazy, todo el bloque va en la `description` desde el turno
1 y las skills scoped quedan disponibles también desde turno 1.

Reusa la infra de Skills (`include_dir!`, frontmatter, 64 KB) como
único repositorio de markdown. Una skill con `node_type` nunca entra al
catálogo de `load_skill` (es auto-folded). El primer nodo con guía es
`sql_query`: la política sale de `SqlPermissions` y la guía vive en
`skills/sql_query-guide/SKILL.md`.

**Documentación:**
- Spec: [docs/superpowers/specs/2026-05-29-layered-tool-context-design.md](superpowers/specs/2026-05-29-layered-tool-context-design.md)
- Plan: [docs/superpowers/plans/2026-05-29-layered-tool-context.md](superpowers/plans/2026-05-29-layered-tool-context.md)
- Dev guides: [29_lazy_tool_loading.md](developer_guide/29_lazy_tool_loading.md) ("Tool context block"); [24_skills.md](developer_guide/24_skills.md) ("Layered routing").
- Schema: [node_configurations.json](node_configurations.json) → `tool_configuration_schema.skills`.

**Estado:** ✅ Done. Verificado E2E contra Gemini Flash + Postgres.

> **UX wart resolved (2026-05-29):** the previous requirement to list
> layer-2 skill names in BOTH `tool_configurations.<X>.skills` AND
> `llm_call.skills.builtin` has been eliminated. The engine now derives
> the load list automatically via `augment_builtin_names`: names in
> `tool.skills` are auto-registered from the builtin pool, node-type
> guides auto-load when any configured tool matches their `node_type`
> frontmatter, and path-based skills auto-discover every `SKILL.md`
> under declared directories. Operators only need to declare scoped
> skills in `tool_configurations.<name>.skills` — one place, no
> duplication. See [24_skills.md](developer_guide/24_skills.md)
> ("How skills auto-load") for the full behavior and a before/after
> example.

> **Sweep ADP:** añade un método default-None a `ExecutableNode` y un
> campo opcional a `ToolConfiguration` (con `#[serde(default)]`).
> Cambios additivos — no rompe el worker de ADP.

---

## Misc

### `.gitignore` para `.DS_Store` y otros artifacts de OS

**Qué cambió.** Agregados patrones para `.DS_Store` (macOS), `Thumbs.db` (Windows), swap files de editores (`*.swp`, `*~`) y carpetas de IDE (`.vscode/`, `.idea/`). `git status` ya no muestra los `.DS_Store` huérfanos.

**Commit:** `90d5a3b` (2026-05-16).

### Test graph basic actualizado

**Qué cambió.** `tests/graphs/agents/load_attachment_basic.json` migrado del placeholder de "Q3 Financial Report" al smoke scenario "what is in the screenshot?" que usamos en la validación inicial del feature.

**Commit:** `b115185` (2026-05-16).

### Real-API smoke graph para auto-summary

**Qué cambió.** Nuevo graph `tests/graphs/agents/load_attachment_auto_summary.json` para correr la generación de auto-summary contra Gemini Flash + Postgres reales (ignored por defecto, requiere `.env`).

**Commit:** `830db90` (2026-05-15).

---

## Matriz de completitud de la documentación

| Feature | Spec | Plan | Dev guide | Schema (`node_configurations.json`) | Test graph | Notas |
|---|---|---|---|---|---|---|
| 1. ProviderKind rename | — | ✅ | ✅ | ✅ | ✅ (renamed) | Tarea de refactor, no de feature; no necesita spec |
| 2. `secure_suspend_allowed` flag | — | ✅ | ✅ | ✅ | ✅ | OK |
| 3. Secure values sliding TTL + masking | ✅ | ✅ | ✅ | (n/a, no son fields del schema) | ✅ | Gap #1 cerrado el 2026-05-18 — sección dedicada en `13_security_strategy.md` |
| 4. LLM temporal/geographic context | ✅ | ✅ | ✅ | ✅ (3 fields en `graph_root_fields`) | ✅ | Gap #2 cerrado el 2026-05-18 — implementado + dev guide con diagrama |
| 5. `load_attachment` base | ✅ | ✅ | ✅ | ✅ | ✅ | OK |
| 6. Attachment auto-summary | ✅ | ✅ | ✅ | ✅ (5 fields) | ✅ | OK |
| 7. path/data registration fix | ✅ (issue) | ✅ | ✅ | (n/a) | ✅ (two-agent) | OK |
| 8. Multimedia generation + artifacts unification | — | ✅ | ✅ | ✅ (3 nodos + categoría `media`) | ✅ (4 media + 2 agents) | Host application wiring pendiente (downstream private repo). Spec absorbida en el plan. |
| 9. `sql_query` auto-crea `allowed_schemas` | — | ✅ | ✅ | ✅ (`create_schemas_if_missing`, default `true`) | ✅ (1 tool + 1 standalone) | Verificado e2e contra Postgres real. Plan absorbe la spec. |
| 10. Layered tool context | ✅ | ✅ | ✅ | ✅ (`tool_configuration_schema.skills`) | ✅ (1 E2E) | sql_query es el nodo de referencia. Guías por nodo (http_request, socketio, etc.) quedan como follow-ups. |

**Leyenda:** ✅ presente y completo · ⚠️ parcial · ❌ ausente · — no aplica · (n/a) no requiere

---

## Gaps y acciones recomendadas

> **Update 2026-05-18:** los tres gaps abajo se cerraron en una pasada paralela. Esta sección queda como histórico — para gaps activos, ver [docs/BACKLOG.md](BACKLOG.md).

### Gap #1 — Secure values sliding TTL no tiene sección en el dev guide ✅ CERRADO 2026-05-18

Estaba: doc gap. Implementación ya existía (2026-05-11) pero el dev guide no la reflejaba.

**Acción aplicada:** sección nueva "## Sliding TTL y outbound masking (desde 2026-05-11)" agregada a `13_security_strategy.md` (commit `b5b65d7`), cubriendo las 4 garantías con snippets SQL/Rust y cross-refs al spec. Footer del guide bumped a v1.3.

### Gap #2 — LLM temporal/geographic context: spec aprobado sin implementación ✅ CERRADO 2026-05-18

Estaba: spec-only desde 2026-05-12.

**Acción aplicada:**
1. Spec **revisada** el 2026-05-18 para alinear a estándares (ISO 8601 + BCP 47 + locale field nuevo). Sin esta revisión, el formato human-only del datetime hubiera divergido de Anthropic y la industria. Commit `4d0cca5`.
2. Plan TDD escrito (`2026-05-18-llm-temporal-geographic-context.md`, 7 tasks).
3. Plan **ejecutado end-to-end** via subagent-driven-development (commits `9a3bc0b` a `1a6bafb`, 8 commits).
4. Smoke graph verificado contra Gemini Flash real — modelo respondió en español citando fecha actual y Bogotá.
5. Dev guide nueva: [`docs/developer_guide/35_temporal_geographic_context.md`](developer_guide/35_temporal_geographic_context.md) con **diagrama end-to-end del pipeline** (JSON → struct → inputs → helper → bloque → system message → provider). Index `DEVELOPER_GUIDE.md` actualizado con entrada #35.

### Gap #3 — `data:` (base64 inline) sigue sin auto-summary ✅ PARQUEADO 2026-05-18

Estaba: limitación conocida del feature de auto-summary, documentada pero sin owner ni trigger explícito.

**Acción aplicada:** parqueado formalmente en [docs/BACKLOG.md](BACKLOG.md) entrada #1 con template completo (origen, problema, workaround actual, por qué parqueado, fix v2 propuesto con tee de upload stream, acceptance criteria, estimación ~80-120 LOC, trigger explícito "cuando ADP empiece a usar uploads inline"). Commit `947bc06`.

---

## Cómo verificar este changelog

```bash
# Lista cronológica de commits en la ventana
git log --since="2026-05-11" --pretty=format:"%h %ad %s" --date=short

# Specs nuevos / modificados
ls docs/superpowers/specs/2026-05-1*.md

# Plans nuevos
ls docs/superpowers/plans/2026-05-1*.md

# Verificar que los campos del schema están documentados
grep -E "\"(attachments_enabled|secure_suspend_allowed|summary_(enabled|max_chars|model|timeout_secs|max_output_chars))\"" docs/node_configurations.json
```

---

## Próxima revisión

Cuando se cierre la próxima ventana (2026-05-18 a 2026-05-31), continuar este changelog en `docs/CHANGELOG_2026-05-second-half.md` o consolidar en un `docs/CHANGELOG_2026.md` por mes.

---

## 2026-05-31 — Revert layered-tool-context

**Breaking changes:**

- Skill frontmatter no longer supports `node_type:` (rejected with a migration error at load time pointing to the new model).
- `tool_configuration.skills` field removed from graph schema (the tools no longer carry their own skill references).
- `ExecutableNode::tool_description_supplement` trait method removed (auto-injected policy text no longer reaches the LLM; runtime validators still enforce).
- Built-in skill `sql_query-guide` removed (was layer-1).

**Migration:**

- Skills with `node_type:` → remove the frontmatter field; reference the skill explicitly from `llm_call.skills` instead.
- `tool_configuration.skills` → move the skill names to `llm_call.skills` on the LLM node.
- SQL permissions visibility to the LLM → author a skill markdown describing the policy and reference it from `llm_call.skills`. The runtime validator still rejects out-of-policy queries.

**New features:**

- **Recursive references**: `references/<name>.md` files can declare their own `references:` frontmatter. The LLM navigates with `load_reference("skill", "path/to/sub")`. Max depth 5, cycles rejected at load time.
- **`skills_path` / `skills_paths` on `llm_call`**: load all skills under a directory without enumerating by name. Coexists with the existing `skills: [...]` array (union, dedup).

See:
- Spec: [docs/superpowers/specs/2026-05-31-revert-layered-tool-context-design.md](superpowers/specs/2026-05-31-revert-layered-tool-context-design.md)
- Plan: [docs/superpowers/plans/2026-05-31-revert-layered-tool-context.md](superpowers/plans/2026-05-31-revert-layered-tool-context.md)
- Updated guide: [docs/developer_guide/24_skills.md](developer_guide/24_skills.md)


---

## 2026-05-31 — Router & Output Parser nodes

**Qué cambió.** Dos nodos nuevos shipped en `develop`:

- **`output_parser`** — wrapper liviano de `information_extraction` con UX para encadenar después de un `llm_call` o agente. Single `input` port (default), schema inline-required (`{ field: { type, required, description } }`), hard-error en input vacío (null / `""` / `[]` / `{}`).
- **`router`** — bifurcación declarativa entre N ramas nombradas. Dos modos:
  - `llm_direct`: el LLM elige una rama por nombre desde las descripciones.
  - `extract_and_route`: el LLM extrae JSON contra un schema; DSL `when` declarativo (operadores `equals`, `not_equals`, `in`, `contains`, `gt`/`lt`/`gte`/`lte`, `matches`, `exists`; combinadores `all`/`any`/`not`; dotted paths) elige la rama (primera que matchea gana).
  - Cada rama puede declarar opcionalmente un `subgraph` inline (path o inline) que se ejecuta antes de emitir.
  - Sin rama default — fail-fast embebe el `extracted` JSON en el error.
  - Always-on `__decision` output port con `{ selected_branch, reason?, extracted? }` para audit/logging.

**Reuso interno.** Se extrajeron dos helpers compartidos a `nodes/util/`: `inline_schema.rs` (converter inline→standard JSON Schema + validador) y `extract_with_schema.rs` (LLM call + parse + fence-stripping + schema validation). `extraction.rs` se refactorizó para delegar a `extract_with_schema` sin cambio de comportamiento observable (-79 líneas netas).

**Wiring.** `RouterNode` y `SubGraphNode` comparten el mismo `Arc<OnceLock<SubGraphExecutorPort>>` en el registry, así una sola llamada a `set_subgraph_executor()` los wirea a ambos.

**Documentación:**
- Spec: [docs/superpowers/specs/2026-05-31-router-and-output-parser-nodes-design.md](superpowers/specs/2026-05-31-router-and-output-parser-nodes-design.md)
- Plan: [docs/superpowers/plans/2026-05-31-router-and-output-parser-nodes.md](superpowers/plans/2026-05-31-router-and-output-parser-nodes.md)
- Dev guide: [docs/developer_guide/37_router_and_output_parser.md](developer_guide/37_router_and_output_parser.md)
- Schema: [docs/node_configurations.json](node_configurations.json) → entries `output_parser` y `router`
- Tests: 5 graphs en `tests/graphs/control_flow/` (`output_parser_basic.json`, `router_llm_direct.json`, `router_extract_rules.json`, `router_with_subgraph.json`, `router_chained.json`) — gated on `GEMINI_API_KEY`.

**Tests:** ~55 unit tests nuevos (12 `inline_schema` + 6 `extract_with_schema` + 5 `output_parser` + 11 router `config` + 16 router `when_dsl` + 5 router `node`) + 2 registry tests + smoke tests con Gemini real (mode A confirmado: `selected_branch: "sales"` para query de compra).

**Estado:** ✅ Done. Aditivo — sin breaking changes en la API pública de Colmena ni en el worker de ADP.
