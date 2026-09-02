# Resumen de gaps doc-vs-código — priorizado por severidad

Consolidado de los 37 archivos de [`docs/qa/nodes/`](README.md). Reúne las
secciones **1) config documentada NO soportada** y **2) código NO documentado**
de cada nodo, ordenadas por impacto para un operador/QA.

Criterio de severidad:
- **Alta** — el operador obtiene comportamiento incorrecto o silencioso, o una
  feature real que no puede descubrir sin leer el código fuente.
- **Media** — contrato incompleto: defaults, side-effects o errores silenciosos
  que un test descubre pero la doc no anticipa.
- **Baja** — documentación incompleta sin sorpresa de runtime (falta una entrada,
  terminología imprecisa, detalle interno).

Marca **[✓código]** = verificado contra el código fuente en esta auditoría;
el resto proviene del análisis del subagente por nodo (ver el `.md` del nodo
para `archivo:línea`).

---

## Severidad ALTA

| # | Nodo | Gap | Evidencia |
|---|------|-----|-----------|
| ~~A1~~ | `sql_query` | ✅ **RESUELTO.** El campo fantasma `guardrail_enabled` se eliminó del `schema()` del nodo y de toda la doc canónica. La validación estática es incondicional por diseño (es lo que bloquea `DROP`/`TRUNCATE`/`DELETE` sin `WHERE`), así que se quitó el flag en vez de cablearlo. Los grafos que aún lo pasan siguen funcionando: la clave sobrante se ignora. | Ver [sql_query.md](sql_query.md) |
| ~~A2~~ | `loop_controller` | ✅ **RESUELTO.** Se descartó el fail-closed estricto: el enum estaba incompleto (`orchestrator` emite `FINISHED_PHASE`) y rechazarlo habría roto el orquestador. En su lugar el nodo **coacciona** todo valor no reconocido a `FINISHED` con un `warn`, y se añadió el techo `COLMENA_MAX_GRAPH_TURNS` (default 50) al loop de `api.rs` — que ataca la causa raíz, porque los contadores `max_total_calls` se reconstruyen en cada turno y no podían acotar el bucle exterior. | Ver [loop_controller.md](loop_controller.md) |
| A3 | familia LLM | Campos de config reales **invisibles en `node_configurations.json`**: `thinking_budget`, `streaming`, `temperature` (fija/oculta). No se pueden descubrir sin leer el código. | Afecta `llm_call`, `planner`, `critic`, `reactor`, `orchestrator`, `router`. Ver cada `.md` |
| A4 | `input` | El doc-comment afirma resolver `{{key.nested}}` pero la implementación hace **lookup plano** (`state.get("key.nested")` literal, sin traversal). Un template anidado se reemplaza por vacío en silencio. | **[✓código]** `input.rs:8` (comentario) vs `input.rs:22-23` (lookup plano). Ver [input.md](input.md) |
| A5 | `http_request` | `bearer_token` **sí** resuelve `${ENV_VAR}` pese a que la doc declara `supports_env_vars: false` — contradicción que confunde y puede exponer el manejo de secretos. | Ver [http_request.md](http_request.md) |

## Severidad MEDIA

| Nodo | Gap |
|------|-----|
| `sql_query` | Multi-statement no está en el outputs-schema; auto-RLS post-`CREATE TABLE` es un side-effect no documentado; `${ENV_VAR}` en `tenant_user_id` falla en silencio; constantes `MAX_SCHEMA_TABLES/CHARS` no documentadas; el nodo nunca lanza (siempre retorna JSON con error). |
| `trigger_webhook` | `method` no se valida (acepta cualquier string) **[✓código `trigger.rs:55`]**; prioridad de payload implícita; error de serialización no documentado; auto-flatten como contrato downstream. |
| `loop_controller` | `suspend_flag: true` sobrescribe `loop_status` incondicionalmente; `question` solo se incluye en `SUSPENDED` y `all_tasks`→`final_result` solo en `FINISHED` (exclusión silenciosa); fallback inputs→config no documentado. |
| `information_extraction` | Inputs vacíos: ¿skip o null? no documentado; suspend no documentado; falta validación de la estructura del JSON Schema; merge system_message config+input. |
| `subgraph` | `memory_mode` documentado en la guía pero **no en `tool_configurations.schema`**; eventos SSE `subgraph-*` no documentados; nesting unbounded por defecto; claves internas `__colmena_*`. |
| `secure_suspend` | Discrepancia de formato de IDs (spec vs ports-ref); el parser es más estricto que el formato textual documentado; la descripción de `id` contradice el comportamiento (`secrets[].name` ES el id). |
| `suspend` | Patrón `cfg_or_input` (uso como tool) no documentado; "Troubleshooting" enlaza a un doc inexistente; nota de `config.id` fallback ambigua. |
| `task_memory_writer` | Sin validación en `add_tasks`; silencia errores en `delete_tasks`; output NO wrappeado en `{output:...}`; fallbacks silenciosos por `session_id` en `_state`. |
| `tavily_client` | `max_results` se ajusta fuera de rango sin aviso; `time_range` no documentado; `extract_format` cae a markdown sin advertencia; session id hardcoded a "default" para rate limiting. |
| `api_explorer` | `enable_cache` se ignora silenciosamente; clamp de `limit`/`max_results` [1,200] no documentado; método HTTP forzado a mayúsculas; `secure_values` es dead_code. |
| `router` | Temperatura fija 0.1; `reason` con conversión silenciosa a `""`; regex de validación de nombre de rama no documentado; conversión silenciosa de entrada a JSON. |
| `image_generation` / `image_edit` / `tts` | Inputs engine-injected `__colmena_session_id`/`__colmena_agent_session_id`; inyección de secure-values; auto-registro en AttachmentRegistry es fail-soft; token caching de Vertex (~50 min); mapping MIME→extensión; `${ENV_VAR}` en `api_key`; (`image_edit`) resolución de `$attachment:` en `source_url`. |
| `http_request` | Logging a stdout en modo multipart; OAuth no soportado en multipart (error en runtime); alias legacy `query_parameters`; `$attachment:` en multipart requiere `agent_session_id`. |
| `document_edit` | Falta fallback de `session_id`; array `conflicts` poco claro; `PatchSource::Agent` hardcoded, no configurable. |
| `orchestrator` | `temperature` (planner/critic/reactors), `thinking_budget` (final_reactor) y `allow_suspend` (agentes) no documentados; validación permisiva de `agents.description`. |

## Severidad BAJA

| Tema | Nodos afectados |
|------|-----------------|
| **Falta la entrada en `node_as_tools_reference.json`** (patrón dominante) | `current_time`, `log`, `divide`, `exponential`, `document_create`, `document_read`, `document_edit`, `python_script`, `suspend`, `secure_suspend`, `critic`, y varios de la familia LLM |
| Falta entrada en `node_ports_reference.md` | `python_script`, `divide` (oculta error de div/0) |
| Terminología imprecisa | `current_time` (dice "ISO-8601", emite RFC3339) |
| Errores no tipificados / genéricos en docs | `add` (`MathError::NotANumber` con nombre de campo), `multiply`, `subtract` (uso de `f64` e imprecisión decimal), `exponential` (error genérico en `get_f64`) |
| Campo de config faltante puntual | `output` (`supports_templates`), `document_edit` (defaults en `schema()`) |
| Detalles internos sin contrato observable | `for_each` (`ChildScopeObserver`, init de `item_state`), `mock_input` (comentario interno, sin validación de config) |

---

## Vista por archivo de documentación a tocar

Para planear el trabajo de doc-fixes, agrupado por el archivo canónico que hay que editar:

| Archivo de doc | Acción principal |
|----------------|------------------|
| `docs/node_configurations.json` | Añadir campos ocultos: `thinking_budget`, `streaming`, `temperature` (LLM-family); `supports_templates` (output); `memory_mode` en `subgraph`. (El `guardrail_enabled` de `sql_query` se eliminó en vez de documentarse — ver A1.) |
| `docs/node_as_tools_reference.json` | Crear las entradas faltantes (ver tabla BAJA — es el gap más numeroso). |
| `docs/agent_context/node_ports_reference.md` | Añadir `python_script`; corregir div/0 en `divide`; outputs de `socketio_request` (`exception`, transport-errors). |
| Guías `docs/developer_guide/` | Documentar `cfg_or_input` (suspend/secure_suspend), inputs `__colmena_*` (multimedia), fail-soft de auto-registro, side-effect auto-RLS (sql). |

> **Nota:** las severidades Alta están verificadas contra el código fuente en esta
> auditoría. Las Media/Baja provienen del análisis por nodo; antes de accionar cada
> fix, confirmar el `archivo:línea` citado en el `.md` correspondiente (las memorias
> y notas de agente son observaciones puntuales, no estado vivo).
