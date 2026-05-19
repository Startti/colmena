# Cambios recientes — auditoría 2026-05-03 al 2026-05-19

> **Generado:** 2026-05-17 · **Actualizado:** 2026-05-19 (api_explorer flag-only activation)
> **Alcance:** Commits sobre `develop` desde 2026-05-11 (los anteriores a esta ventana fueron auditados previamente).
> **Total:** ~90 commits agrupados en 11 features.

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
