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

---

## 3. CRDT Documents — C smoke debt cleanup (post-subsistema C)

**Qué cambió.** Cinco fixes destapados durante el browser smoke de C con xlsx 1000-row:

1. **`crdt_doc_run_python` ahora retorna `loaded_sheet_columns` en cada error response.** El dispatcher ya tiene `records_by_sheet` en mano antes de invocar el sandbox, lo expone como `{sheet_id: [col1, col2, …]}` en errores. El agente puede self-corregir KeyError en 1 turn en vez del loop de 8 retries que observamos.
2. **Nueva builtin skill `crdt-doc-run-python`** (`src/libs/colmena/skills/crdt-doc-run-python/SKILL.md`) que documenta el contrato de shape (Y.Doc row 1 → pandas columns, manejo de title rows en xlsx importado, type quirks, debug workflow). Se activa con `config.skills.builtin: ["crdt-doc-run-python"]`. El grafo de smoke C ahora la usa.
3. **Frontend `static/index.html`:** botón "Import .xlsx…" abre file picker + POST a `/documents/:id/import` (estaba hardcoded a fetch `/spike.xlsx`, confundía a usuarios nuevos). Botón legacy "spike fixture" preservado para smoke graphs viejos. Grid `rowCount` bumpeado 100→50000 para que imports >100 filas sean visibles en el canvas (Univer es virtualizado → memoria ~0).
4. **`scripts/check_python_env.sh`** valida match de ABI entre `.venv/bin/python` y el binario `dag_engine` (vía `otool -L`). Catch del numpy ImportError class de errores en <2s, con instrucciones de recovery para ambos paths (rebuild venv vs rebuild binary con `PYO3_PYTHON`).
5. **`crdt-yws` / `crdt-yws-graph` startup logs** ahora imprimen path absoluto + count de artifacts loaded + warning si `--dump-dir` es relativo. Resuelve el "mystery artifact wipe" que pasamos investigando 20 min.

**Por qué importa.** El smoke C consumió ~1.5 horas debuggeando issues que ninguno era del feature core de C — todas eran fricciones operacionales (ABI mismatch, hardcoded fixture, off-by-one en mental model, path relativo silente). Estos fixes los previenen permanentemente, especialmente importante porque F va a usar `run_python` masivamente y heredar los mismos riesgos.

**Verificación end-to-end.** Re-corrida del smoke C atómico con todos los fixes: agente invoca `load_skill('crdt-doc-run-python')` antes del primer pandas call, ejecuta los 2 `run_python` (138 items < $10 + 15 cantidades únicas) **en 0 retries**, total 24,801 tokens (vs 14,239 sin skill — +10K por el skill, pero -8 iteraciones por el loop evitado).

**Documentación de referencia.**
- Skill: [`src/libs/colmena/skills/crdt-doc-run-python/SKILL.md`](../src/libs/colmena/skills/crdt-doc-run-python/SKILL.md)
- Dev guide §7 actualizado con notas de persistencia: [`docs/developer_guide/38_crdt_documents.md`](developer_guide/38_crdt_documents.md)
- Items deferidos en [`docs/BACKLOG.md`](BACKLOG.md): dynamic grid sizing desde Y.Doc max-row, auto-detect title row en `df_records`.

**Commits.** `bde1d08`, `615d988`, `c5a8f70`, `f0d4aeb`, `7666653` (5 commits separados por fix).

**Estado.** done.

---

## 4. CRDT Documents — Cross-sheet & cross-artifact analysis (subsistema F)

**Qué cambió.** El agente puede comparar, unir, enriquecer o transformar datos entre sheets — ya sea dentro del mismo artifact o trayéndolas desde otros artifacts. Dos tools nuevos: `crdt_doc_list_sheets_of({artifact_id})` (peek cross-artifact sin clonar) y `crdt_doc_import_sheet({source_artifact_id, source_sheet_id, new_name?})` (clonado snapshot al artifact actual). Una skill builtin nueva: `crdt-doc-cross-sheet-analysis` con 6 patrones pandas canónicos (cell-diff, row-diff por key, schema-diff, statistical, join/enrich, conditional transform). Extensión backward-compatible a `crdt_doc_get_recent_changes` con `artifact_id?` opcional para auditar otros artifacts. Cero cambios a `crdt_doc_run_python` (subsistema C) — la sheet clonada vive en el mismo artifact que el principal, multi-sheet ya funcionaba.

**Por qué importa.** Los workflows reales con xlsx exigen cruzar varias hojas / varios workbooks (versionado Q3 vs Q4, enrichment con catálogo, reglas externas). Sin F la única forma era pasar todo el contenido vía prompt o usar set_range manualmente — ambos prohibitivos en tokens y propensos a error. F unifica ambos casos en un solo flujo (`list_sheets_of → import_sheet → run_python`) reusando 100% de la infra de B (audit) y C (pandas).

**Documentación de referencia.**
- Spec: [`docs/superpowers/specs/2026-06-04-crdt-cross-sheet-analysis-design.md`](superpowers/specs/2026-06-04-crdt-cross-sheet-analysis-design.md)
- Plan: [`docs/superpowers/plans/2026-06-04-crdt-cross-sheet-analysis.md`](superpowers/plans/2026-06-04-crdt-cross-sheet-analysis.md)
- Dev guide §5.7: [`docs/developer_guide/38_crdt_documents.md`](developer_guide/38_crdt_documents.md)
- Items deferidos: [`docs/BACKLOG.md`](BACKLOG.md) — multi-session workspace (bloqueante para A), live linking, delete sheet, consolidate parse_a1, reuse projection in list_sheets_of.

**Commits (F-T1 a F-T10).** Ver `git log feature/docs --grep="F-T"`.

**Estado.** done.

**Limitaciones conocidas v1.**
- Snapshot only — sin live linking (BACKLOG v1.1).
- Sin tool de delete_sheet — el cap de 100 sheets/artifact protege pero no limpia (BACKLOG).
- Discovery sigue session-scoped (`list_my_artifacts`); cross-session via workspace es v1.1 bloqueante para subsistema A.
- Sin permission model — cualquier agente con `artifact_id` puede leer/importar.

**Forward compatibility.** Los tools de F no enforcean session ownership a nivel de tool — cuando workspace concept aterrize en v1.1, solo el discovery cambia; los tools de F siguen funcionando sin modificación.

---

## 5. Token optimization — Plan GAMMA (F-T14)

**Qué cambió.** Tres optimizaciones acumulativas para reducir tokens enviados al LLM en cada iteración del ReAct loop. Todas miden con el smoke `f_cross_artifact_smoke.json` contra el baseline previo (gemini-2.5-flash).

| Step | Cambio | Per-iter avg (T) | vs baseline |
|---|---|---:|---:|
| Baseline | — | 7,925 | — |
| **A1** | Comprimir descriptions de tools crdt_doc_* + load_skill + system PRELUDE + tool-use rules | 7,344 | -7% |
| **A1+A2** | + skill-out-of-history (compactar load_skill tool results >N=3 turnos atrás) | 6,120 | -23% |
| **A1+A2+A3** | + lazy_tool_loading extendido a synthetic crdt_doc_* | **4,670** | **-41%** |

**A1 (commit `e293001`)** — string compression. crdt_doc_run_python description: 458T → 110T. crdt_doc_import_sheet: 361T → 60T. Etcétera. PRELUDE auto-injected (`CRDT_SPREADSHEET_PROTOCOL_PRELUDE`) reescrito de 6 rules con ejemplos a 6 rules en 1 línea cada una. "Tool Use Instructions" del system message reducido de 4 bullets verbose a 1 línea. Total: -870T per iter fijo.

**A2 (commit `2ead7b1`)** — skill-out-of-history. Nueva función `compact_old_load_skill_in_history()` en `agent_service.rs`: para cada Tool message con `tool_call_id` que el matching Assistant emitió como `load_skill` y está más de 8 mensajes atrás (≈3 ReAct turns), reemplaza el body por un marker `"[skill X loaded earlier (N chars). Call load_skill again to re-read.]"`. La conversación persistida en `conversation_repository` NO se altera — solo el `LlmRequest` enviado al provider. Provider-agnostic: funciona en OpenAI, Anthropic, Gemini sin cambios al adapter. 6 unit tests nuevos cubren noop, happy path con/sin reference, preservación de tool calls recientes, no-load_skill tools intactos, idempotencia.

**A3 (commit `60ca29a`)** — lazy_tool_loading para synthetic tools. Cuando ambos `lazy_tool_loading: true` y `crdt_documents` config están seteados, los crdt_doc_* tools se registran como CatalogEntry (con summary auto-derivado del description) en vez de inyectarse directo al tools[] array. La infraestructura existente de lazy (`describe_tool` + `tools_provider` closure) los expone sólo después que el agente los descubre. `load_skill` queda always-eager (es el entry point de skill discovery). Tools[] arranca con 2 tools (~826T), crece hasta 6 (~1411T) en lugar del fijo 12 tools (~2453T).

**Por qué importa.** ReAct loops re-envían toda la conversación + tools cada iteración. Por design las APIs son stateless. Para un smoke típico de F (10-17 iter con 4 outputs), bajar de 100K → 75K total tokens es:
- ~25% menos $$$ por run (~$0.043 → $0.032 en Gemini Flash)
- Latencia menor (menos bytes que procesar per iter)
- Headroom para que el agente "piense" más sin agotar context window

A escala (ADP con miles de runs/día) el ahorro acumulado es relevante.

**Bonus instrumentación.** En `agent_service.rs` quedó formalizada una diagnóstica gated por env vars:
- `COLMENA_DUMP_PROMPT_SIZES=1` → 1 line per iter con sizes de msgs + tools
- `COLMENA_DUMP_PROMPT_FULL=1` → full breakdown de cada msg + cada tool

Ambas con costo CERO cuando están off. Útil para medir el impacto de cualquier optimización futura sin agregar lógica nueva.

**Limitaciones / forward-compat.**
- A3 sigue la convención existente: `load_skill` siempre eager. Si en el futuro hay 100 skills, el catálogo de load_skill crece — habría que considerar un segundo nivel de lazy para skills. v1.1.
- Prompt caching nativo de los providers (Anthropic con cache_control, Gemini Cached Content API) NO se implementó en este PR. El adapter de OpenAI ya tiene caching automático; Anthropic adapter lee `cache_read_input_tokens` pero no SETEA markers; Gemini adapter no tiene nada. Item BACKLOG para v1.1.
- Mock 3 (análisis estático) predijo ~6K para A3 pero el run real dio ~10K — la diferencia es que A3 también previene el "growth" de tools cuando hay errores o re-pruebas (el agente ya descubrió la tool, no necesita re-descubrir).

**Commits.** `e293001` (A1), `2ead7b1` (A2), `60ca29a` (A3), este (A4 — docs).

**Estado.** done.

---

## 6. CRDT formulas (subsystem D, v1)

- **Backend formula evaluator** via `formualizer = "0.6"`. Cells with a
  leading `=` are parsed, evaluated, and persisted with `{v, t, f, fs}`.
- **Intra-sheet eager recalc** — dependent formulas refresh in topo
  order on every `set_cell`/`set_range`/`run_python` write.
- **`crdt_doc_read(include_formulas: bool = false)`** — back-compat
  default for pandas; opt-in formula-aware shape `{v, f?, fs?}` per cell.
- **`crdt_doc_list_sheets`** now returns `formula_count` per sheet.
- **`needs_browser` fallback** — functions outside formualizer's set are
  persisted as placeholders with a warning so the agent can decide.
- **Tool warnings**: `set_cell` / `set_range` / `run_python` now surface
  `cells_recalculated` + structured warnings (`needs_browser`,
  `eval_error`, `cycle`, `parse_error`).
- **pandas write-back** strips formula metadata, emits a
  `formula_replaced_by_literal` CRDT event, and cascades dependent recalc.
- **Skill `crdt-doc-formulas`** — 3 patterns (write, read-evaluated,
  needs_browser fallback) auto-loaded by the skill registry.
- **Anti-divergence benchmark** (`#[ignore]`-gated) — 40 fixtures green
  against formualizer; Univer-side Playwright bridge tracked in BACKLOG.
- **⚠️ BREAKING**: strings starting with `=` passed to `crdt_doc_set_cell`
  are now parsed as formulas. To store a literal `=text`, prefix with `'`
  (Excel convention). No existing test graphs do this — verified via
  `grep -rn '"\\"=' tests/graphs/` returning zero matches.
- **⚠️ API CHANGE**: `apply_set_cell_in_proc` signature changed from
  `()` to `SetCellOutcome` (struct with `cells_recalculated` and
  `warnings`). `SetCellOutcome` is `#[must_use]`. 43 call sites updated
  in this repo; downstream consumers of this internal API must bind the
  return value.
- **⚠️ API CHANGE**: `df_writer::apply_records_to_doc` is new and
  requires the target sheet to exist (`WriterError::SheetNotFound`).
  The existing `write_records_as_new_sheet` continues to behave as before.

**Referencias:**
- Spec: [`docs/superpowers/specs/2026-06-04-crdt-formulas-design.md`](superpowers/specs/2026-06-04-crdt-formulas-design.md).
- Plan: [`docs/superpowers/plans/2026-06-04-crdt-formulas.md`](superpowers/plans/2026-06-04-crdt-formulas.md).
- Backlog: v1.1 follow-ups en `docs/BACKLOG.md` → "CRDT Documents v1.1 — Formulas (subsystem D follow-ups)".

**Adicional shippeado (D-T14, D-T15, D-T16).**
- **D-T14** (`6a14571`, `25633fa`): plumbing `f`/`fs` por el demo Univer
  estático (inbound `SET_RANGE_VALUES_MUTATION` + outbound
  `applyingFromYDoc` guard) + replayCells inicial.
- **D-T15** (`19bb419`): server-side `recalc_observer` que dispara
  `recompute_dependent` sobre ediciones que llegan vía WS desde un peer
  browser. Funciona end-to-end cuando `f` está intacta en el yrs Doc.
- **D-T16** (este commit): UX aliases en los dispatchers de `crdt_doc_*`
  (`address`→`addr`, `start`→`start_addr`, `values`→`values_2d`, `range`
  auto-expande single-cell `"C1"` a `"C1:C1"`) + dev guide §5.8.1
  "Frontend integration contract" + entrada top-priority en BACKLOG D
  documentando el gap Univer↔yrs (re-process strip `f`) con tres paths
  candidatos para el equipo del frontend del ADP.

**Limitación conocida (handover al frontend del ADP).** Cuando hay un browser
Univer conectado, el `UniverFormulaEnginePlugin` re-procesa la celda tras
aplicar la mutación inbound y emite una segunda mutación sin `f`. El outbound
del demo la propaga al yrs Doc y borra `f`/`fs`. El backend (D-T1..D-T15) es
correcto y funciona 100% sin browser; la fix vive en el frontend de ADP. Ver
BACKLOG → "Univer ↔ yrs formula round-trip".

**Estado.** D-T1..D-T16 done.
