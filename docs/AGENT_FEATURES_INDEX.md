# Agent Features — Índice Rápido

Punteros a la documentación específica de las capacidades más usadas al construir agentes en Colmena. Cada sección lista el archivo "vivo" (developer guide) y la fuente de verdad técnica (spec / código).

---

## Lazy tool loading

Carga progresiva del schema de tools en `llm_call` vía el tool sintético `describe_tool` — el modelo recibe un catálogo ligero `name + summary` y revela schemas completos on-demand.

- **Guía:** [`docs/developer_guide/29_lazy_tool_loading.md`](developer_guide/29_lazy_tool_loading.md)
- **Spec:** [`docs/superpowers/specs/2026-05-03-lazy-tool-loading-design.md`](superpowers/specs/2026-05-03-lazy-tool-loading-design.md)
- **Configuración por tool**: `summary` y `eager` en `tool_configurations`. SSE: `tool-described` events; final summary: `tools_discovered`.

---

## Skills

Paquetes de conocimiento en markdown cargados bajo demanda por `llm_call` vía el tool sintético `load_skill`.

### Modos de definir skills

| Modo | Cómo se declara | Doc |
|---|---|---|
| **Built-in** (compiladas con `include_dir!`) | Nada en el grafo — siempre disponibles | [`24_skills.md`](developer_guide/24_skills.md) §"Skills integradas" |
| **Filesystem path** (declared) | `skills.paths: ["/abs/path/to/skill_dir"]` | [`24_skills.md`](developer_guide/24_skills.md) §"Skills del usuario (paths)" |
| **Signed URL** (preprocesado por ADP worker) | `skills.declared` con `url` apuntando a un signed URL → ADP descarga, escribe en cache local, reescribe a `paths` antes de invocar Colmena | [`docs/superpowers/plans/2026-05-03-user-skills-via-signed-urls.md`](superpowers/plans/2026-05-03-user-skills-via-signed-urls.md) |
| **Inline content** (preprocesado por ADP worker) | `skills.declared` con `content` literal → ADP escribe a archivos, reescribe a `paths` | mismo plan ↑ |

**Seguridad:** `paths` se validan con `canonicalize()` y el allowed-dirs whitelist; sin escape via `../` o symlinks. Catálogo + observabilidad en `24_skills.md` §"Observabilidad".

---

## Nodo `python_script`

Ejecuta código Python arbitrario vía PyO3 dentro del DAG.

- **Guía:** [`docs/developer_guide/26_python_node.md`](developer_guide/26_python_node.md)

### Dos modos de uso

| Modo | Cómo | Cuándo |
|---|---|---|
| **Como nodo DAG top-level** | `node_type: "python_script"`, `code` literal en config | Transformaciones determinísticas, glue entre nodos |
| **Como LLM tool** | `tool_configurations.<name>` con `node_type: "python_script"`. El `code` puede ser fijo o que el LLM lo genere. Activar `sandbox_mode: "restricted"` cuando el LLM escribe código | Cálculos sobre datos que el LLM ya tiene |

### Sandbox

- `sandbox_mode: "none"` — Python completo. Default.
- `sandbox_mode: "restricted"` — AST whitelist + builtins prohibidos + timeout. **Obligatorio cuando el LLM genera el código.**

Convención de output: el script asigna su resultado a una variable `output`. Reserved keys, threading model y troubleshooting en la guía.

---

## Nodo `api_explorer`

Descubrimiento de specs OpenAPI/Swagger + builder de payloads para `http_request`. **No ejecuta llamadas** — solo describe.

- **Guía:** [`docs/developer_guide/25_web_nodes.md`](developer_guide/25_web_nodes.md) §"api_explorer"
- **Spec:** [`docs/superpowers/specs/2026-04-23-web-nodes-c-api-explorer-design.md`](superpowers/specs/2026-04-23-web-nodes-c-api-explorer-design.md)
- **5 sub-tools** expuestos vía `expose_sub_tools: "all"`: load_spec, list_endpoints, describe_endpoint, search_endpoints, build_http_request.
- Caché de specs por `conversation_id` con eviction al cerrar la conversación.
- Conversión automática Swagger 2.0 → OpenAPI 3.0; resolución inline de `$ref`.

**Patrón canónico**: el LLM usa api_explorer para entender la API, luego llama un `http_request` separado para ejecutar. Nunca encadenar como un solo nodo.

---

## Nodo `secure_suspend`

Pausa el DAG para recolectar uno o más secretos del usuario en una sola pausa, persiste cifrados y devuelve solo handles `<sv_<name>>`. El valor real nunca llega al LLM.

- **Catalog:** [`docs/node_configurations.json`](node_configurations.json) → `secure_suspend`
- **Ports:** [`docs/agent_context/node_ports_reference.md`](agent_context/node_ports_reference.md) §"secure_suspend"
- **Spec:** [`docs/superpowers/specs/2026-05-07-secure-suspend-node-design.md`](superpowers/specs/2026-05-07-secure-suspend-node-design.md)
- **Guía de seguridad:** [`docs/developer_guide/13_security_strategy.md`](developer_guide/13_security_strategy.md) §"Strategy 6"

### Dos modos

- **Top-level DAG node** — config con `secrets: [{question, name}, ...]` directamente.
- **LLM tool** — registrado en `tool_configurations` con `node_type: "secure_suspend"`. El LLM provee la lista de secrets en sus args.

### Reanudación

CLI: `--answer "<pregunta_1>\n<valor_1>\n<pregunta_2>\n<valor_2>"`. Parser ancla en el texto literal de cada pregunta (preserva multilinea internos).

---

## Cambios recientes (2026-05) — pre-requisitos para el flujo canvas-builder

Los siguientes specs cierran el ciclo end-to-end de `secure_suspend` desde el meta-agente hasta el agente generado, validado contra Postgres + httpbin reales:

| Cambio | Spec | Plan | Resumen |
|---|---|---|---|
| `secure_suspend` node | [spec](superpowers/specs/2026-05-07-secure-suspend-node-design.md) | [plan](superpowers/plans/2026-05-07-secure-suspend-node.md) | Recolección interactiva en batch, persistencia cifrada, handles de salida. |
| Inject covers `config` | [spec](superpowers/specs/2026-05-07-inject-secrets-in-config-design.md) | [plan](superpowers/plans/2026-05-07-inject-secrets-in-config.md) | `inject_secrets` corre sobre inputs **y** config antes de cada nodo. |
| `llm_call` propaga SUSPENDED | [spec](superpowers/specs/2026-05-08-llm-call-tool-suspend-design.md) | [plan](superpowers/plans/2026-05-08-llm-call-tool-suspend.md) | Cuando un tool retorna SUSPENDED, el agente corta el loop, pausa el DAG, y al resume replaya la conversación + re-ejecuta el tool con la respuesta. |
| `agent_session_id`-first lookup | [spec](superpowers/specs/2026-05-08-secure-values-agent-session-id-design.md) | [plan](superpowers/plans/2026-05-08-secure-values-agent-session-id.md) | `secure_value_mappings` keya por `agent_session_id` cuando está set; fallback a `session_id`. Mismo patrón que `llm_node_history` y `dag_runs`. Habilita cross-session. |

### Convención de testing

Todas las pruebas con estado entre runs (suspend/resume, multi-turn, secure_values) **deben usar `--agent-session-id <id_estable>`** — el `--session-id` ephemeral rota por invocación CLI. Detalle en [CLAUDE.md](../CLAUDE.md) §"Regla — Usar `--agent-session-id` en todas las pruebas de grafos".

### Schema BD (3 tablas afectadas)

`secure_value_mappings`, `llm_node_history`, `dag_runs` — ver [`docs/developer_guide/30_database_schema.md`](developer_guide/30_database_schema.md).

---

## Otros índices útiles

- [DEVELOPER_GUIDE.md](DEVELOPER_GUIDE.md) — índice general de las guías técnicas.
- [node_configurations.json](node_configurations.json) — schema canónico de cada `node_type`.
- [agent_context/node_ports_reference.md](agent_context/node_ports_reference.md) — ports y outputs por nodo.
