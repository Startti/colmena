# Cambios recientes — 2026-06

> **Generado:** 2026-06-03 (subsystem B landed)
> **Alcance:** Commits sobre `feature/docs` desde el cierre de V2 (commit `88b3bc7`) hasta el merge eventual a `develop`.

## Cómo leer este documento

Una sección por feature. Cada sección contiene:
- **Qué cambió** — efecto observable.
- **Documentación de referencia** — spec, plan, dev guide, schema.
- **Commits** — rango o lista.
- **Estado** — done / partial.

---

## 1. CRDT Documents — Recent changes awareness + artifact discovery (subsistema B)

**Qué cambió.** Cada `llm_call` con `crdt_documents` config auto-recibe un bloque corto en el `system_message` describiendo qué cambiaron otros peers desde su último turn (filtrando mutaciones propias vía `origin = agent:{session_id}`). Tool `crdt_doc_get_recent_changes` extendido con filtros (`sheet_id?`, `limit?`). Dos tools nuevos: `crdt_doc_list_my_artifacts` (lista artifacts de la sesión) y `crdt_doc_create_artifact` (crea uno nuevo dentro del turn). Toda la auditoría queda en SQL: 3 tablas nuevas (`crdt_doc_events`, `crdt_doc_session_cursors`, `crdt_doc_session_artifacts`). Backend abstraction (`CrdtBackend` trait con `DirectBackend` + `RestBackend`) — el agente en WS-peer mode no toca el DB del server directamente, va via REST. WS upgrade ahora emite `?peer_type=agent&session_id=X` para que el server atribuya origin correctamente.

**Por qué importa.** Antes de B, el agente no sabía qué editaba el humano entre sus turnos (a menos que llamara explícitamente al tool). Ahora la información llega como contexto persistente, gratis. Además el agente puede descubrir/crear workbooks desde adentro de su sesión, lo que abre el camino para subsistema F (compare two excels) y futuros agentes orquestadores.

**Documentación de referencia.**
- Spec: [`docs/superpowers/specs/2026-06-03-crdt-recent-changes-design.md`](superpowers/specs/2026-06-03-crdt-recent-changes-design.md)
- Plan: [`docs/superpowers/plans/2026-06-03-crdt-recent-changes.md`](superpowers/plans/2026-06-03-crdt-recent-changes.md)
- Dev guide §5.5: [`docs/developer_guide/38_crdt_documents.md`](developer_guide/38_crdt_documents.md)
- Items deferidos: [`docs/BACKLOG.md`](BACKLOG.md) (per-cell attribution para peer:browser, paginación, TTL events)

**Commits (B-T1 a B-T18).** Ver `git log feature/docs --grep="B-T"`.

**Estado.** done.

**Limitaciones conocidas v1.**
- Eventos de `peer:browser` tienen `sheet_id: NULL` (server no infiere semántica del Yjs update). Aparecen como "Workbook (sheet unknown)" en el auto-summary. Mejora: BACKLOG "Per-cell attribution para peer:browser events".
- `list_my_artifacts` cap 50 sin paginación. Mejora: BACKLOG.
- TTL de la tabla `crdt_doc_events` no implementado. Mejora: BACKLOG.
- Bug latente descubierto + fixeado durante B-T14: SQLite `INSERT ... ; SELECT last_insert_rowid()` devolvía id=0 bajo pool multi-conexión. Reemplazado por `INSERT ... RETURNING id` que es soportado desde SQLite 3.35.

---

## 2. CRDT Documents — Pandas/Python integration (subsistema C)

**Qué cambió.** Nuevo tool `crdt_doc_run_python(sheet_ids, code, write_to_sheet?)` que ejecuta código Python sandboxed contra workbook data. El agente envía código que usa pandas/numpy/scipy.stats; el runtime carga las sheets pedidas como DataFrames server-side, ejecuta el código, y devuelve `output` (cualquier JSON) al LLM. Si `write_to_sheet` está set, opcionalmente persiste `output_sheet` (un DataFrame) como una nueva sheet en el workbook con auto-suffix de name collision.

**Por qué importa.** Para Excel grandes (>1000 filas), pasar todo al LLM en context es prohibitivo en tokens (~125k tokens para 10k filas). Esta pattern (read sample → generate code → execute server-side) ahorra 10x-1000x tokens. Es el approach standard para data analysis con LLMs (OpenAI Code Interpreter, LangChain pandas agent, etc.).

**Documentación de referencia.**
- Spec: [`docs/superpowers/specs/2026-06-03-crdt-pandas-integration-design.md`](superpowers/specs/2026-06-03-crdt-pandas-integration-design.md)
- Plan: [`docs/superpowers/plans/2026-06-03-crdt-pandas-integration.md`](superpowers/plans/2026-06-03-crdt-pandas-integration.md)
- Dev guide §5.6: [`docs/developer_guide/38_crdt_documents.md`](developer_guide/38_crdt_documents.md)
- Item v1.1 deferido: [`docs/BACKLOG.md`](BACKLOG.md) — "Configurable limits para `crdt_doc_run_python`".

**Commits (C-T1 a C-T9).** Ver `git log feature/docs --grep="C-T"`.

**Estado.** done.

**Requisitos de runtime.** El Python embebido por PyO3 del worker debe tener `pandas`, `numpy`, `scipy` instalados. Local dev: `.venv/bin/pip install pandas numpy scipy`. Producción ADP: incluir en el container del worker.

**Limitaciones conocidas v1.**
- Límites hardcoded (100MB load, 30s timeout, 10KB output, 100K rows write). Mejora: BACKLOG.
- Write-back solo a nueva sheet (no overwrite/append a sheet existente). Mejora: v1.1 cuando UX feedback lo amerite.
- No multi-artifact en un solo call (cross-workbook joins son subsistema F).
- Scipy whitelist es por top-level module (`scipy` completo, no solo `scipy.stats`) por cómo el AST validator hace split en `.`. En la práctica el agente solo usa `scipy.stats` para v1 use cases.
