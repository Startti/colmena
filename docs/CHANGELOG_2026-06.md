
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

---

### E — Google Sheets integration (subsystem E, v1)

- **9 synthetic LLM tools** mirroring `crdt_doc_*` shape: create
  spreadsheet, create_from_xlsx, export_xlsx, list_sheets, add_sheet,
  delete_sheet, read, set_cell, set_range. Tool descriptions are
  deliberately parallel so skills transfer with find-and-replace.
- **Hexagonal layout** at `src/libs/colmena/src/gsheets/`: `SheetsClient`
  trait in domain, `GoogleSheetsHttpClient` REST adapter in
  infrastructure. Tests mock the trait; integration tests `#[ignore]`-gated
  behind `GOOGLE_APPLICATION_CREDENTIALS` + `COLMENA_GSHEETS_TEST_SPREADSHEET_ID`.
- **Auth via existing `yup-oauth2`** — Service Account JSON via
  `GOOGLE_APPLICATION_CREDENTIALS` or Application Default Credentials.
  Same pattern as `image_generation.rs`. **Zero new dependencies.**
- **Formulas evaluated by Google** — write a `"=SUM(...)"` string and
  read back via `value_render: "UNFORMATTED_VALUE"` (computed) or
  `"FORMULA"` (text). No client-side formula engine here.
- **Auto-retry on 429 / 5xx** with exponential backoff (1s/2s/4s
  production, 50ms/100ms/200ms in tests via configurable
  `retry_base_delay`).
- **UX aliases** carried forward from D-T16: `address`/`addr`,
  `start`/`start_addr`, `values`/`values_2d`, `name`/`sheet`.
- **Skill `gsheets-cross-sheet-analysis`** — mirror of F's CRDT skill,
  6 pattern references.
- **No OAuth user-scoped flow in v1.** Deferred to v1.1 so ADP (or any
  downstream) can build it on top.
- **xlsx upload/export deferred to E-T7b** — tool definitions published
  but require attachment-byte plumbing that doesn't exist yet. The other
  7 tools are fully wired and functional.

Refs: spec `docs/superpowers/specs/2026-06-05-google-sheets-design.md`,
plan `docs/superpowers/plans/2026-06-05-google-sheets.md`.

---

### E-T14 — `gsheets_run_python`: analyze sheets without loading rows through LLM context

- **New synthetic LLM tool** `gsheets_run_python` (10th gsheets tool).
  Mirrors `crdt_doc_run_python` (subsystem C) one-for-one: same
  `execute_sandboxed_helper`, same auto-prelude (`pandas as pd`,
  `numpy as np`, `scipy.stats as stats`), same output/stdout/error caps
  (10 KB each), same 30-second timeout. Reuse only — zero new
  dependencies.
- **The point.** Before E-T14, the only way to do pandas analysis over
  a Google Sheet was `gsheets_read` → dump every row into the LLM
  context → `run_python`. 5000 sales rows = ~150k tokens just to feed
  pandas. With E-T14 the LLM describes the analysis as code; the
  dispatcher fetches each binding **in parallel** server-side, runs
  the sandbox, and only the final `output` returns to the LLM.
- **Bindings shape.** `bindings: [{var, spreadsheet_id, sheet, range?}]`.
  Each binding becomes a Python global under `var` (a list of
  `{col: val}` dicts ready for `pd.DataFrame(<var>)`). The LLM picks
  the variable names, not a sheet-id-keyed `dfs` dict — every binding
  is already explicitly named in the call.
- **Parallel fetch** via `futures::future::join_all` — the win is
  perf, not just token savings: two sheets that each take 400 ms
  round-trip now finish together in ~400 ms instead of ~800 ms.
- **Error envelope carries `loaded_columns`** per binding — same
  pattern as `crdt_doc_run_python.loaded_sheet_columns`. A KeyError
  surfaces the actual column set so the LLM self-corrects in one
  round-trip.
- **Wired everywhere:** router in `dag_tool_executor.rs` (gsheets
  block — the catch-all that previously forwarded to `set_range` is
  now an explicit unknown-tool error so misrouted names can't silently
  do the wrong write), `llm.rs` registration (`enabled_tools: "*"`
  now opts into 10 tools instead of 9; explicit
  `enabled_tools: ["gsheets_run_python"]` works too).
- **Skill update.** `gsheets-cross-sheet-analysis/SKILL.md` adds a
  "Loading rows without burning tokens" section with a realistic
  two-sheet pandas merge example. Every pattern reference (A-F) now
  carries a top-of-file directive: use `gsheets_run_python` for >50
  rows, `gsheets_read` only for inspection / small reads /
  `value_render: "FORMULA"`.
- **Tests.** Wiremock-backed dispatcher tests (parallel fetch, KeyError
  → `loaded_columns`, empty/duplicate binding rejection). pandas
  available case is skipped in CI envs without pandas installed; the
  `loaded_columns` contract is exercised regardless. All 28 gsheets
  module tests still green.

Files:
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/gsheets_run_python.rs` — new.
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs` — re-export.
- `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs` — router (catch-all hardened).
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs` — 9-tool array bumped to 10.
- `src/libs/colmena/src/gsheets/infrastructure/http_client.rs` — `#[cfg(test)] pub async fn token_test_seed` so sibling-module wiremock tests can seed the token cache.
- `docs/developer_guide/39_gsheets.md`, `docs/node_as_tools_reference.json` — schema + guidance.

**Estado.** done.

---

### E-T15 — Synthetic tool summaries for lazy tool loading

- **Every Rust-side synthetic tool** now declares a `summary` (10-200
  chars) via `build_synthetic_tool_with_summary` or direct
  `ToolDefinition.summary` field. Enforced in CI by the
  `every_synthetic_tool_has_summary` test in `llm_synthetic_tools/mod.rs`.
  Builds refuse to ship if any synthetic tool is missing a summary.
- **31 tools covered:** gsheets (10), crdt_doc (6), document (7),
  api_explorer (5) + load_skill, describe_tool, load_attachment (3
  built-ins, always present in `lazy_tool_loading: true`).
- **Exemption:** DAG nodes used as tools via `tool_configurations` are
  exempt — their descriptions are user-supplied per agent and dynamic.
  Lazy catalog falls back to truncated `description` for those.
- **Impact:** `lazy_tool_loading: true` catalogs are now always
  consistent and informative. No incomplete tool entries reaching the LLM.

**Estado.** done.

---

### E-T16 — Toolkit packages: enable many tools via single alias

- **New concept:** a toolkit package is a static registry of related
  tools bundled under a short alias. Instead of
  `enabled_tools: ["gsheets_list_sheets", "gsheets_read", ...]`, use
  `enabled_tools: ["gsheets"]` and the engine expands to all 10
  gsheets_* tools at runtime.
- **First package: gsheets** — 10 tools (create, create_from_xlsx,
  export, list_sheets, add_sheet, delete_sheet, read, set_cell,
  set_range, run_python).
- **Exclusion syntax:** `enabled_tools: ["gsheets", "!gsheets_delete_sheet"]`
  removes a single tool from a package.
- **Naming convention enforced:** package aliases must NOT contain `_`.
  Tool names MUST contain `_` after namespace (e.g.
  `gsheets_read`). CI test `package_aliases_have_no_underscore` enforces
  this for visual disambiguation.
- **Edge cases handled:** unknown aliases silent (return 0 tools), exclude
  tool not in includes is no-op, exclude-alone is empty result
  (no panic).
- **Back-compat:** `api_explorer`'s `__` prefix-rule still works.
- **Registration:** new packages appended to `TOOLKIT_PACKAGES` in
  `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/toolkit_packages.rs`
  with one struct literal.
- **Integration:** works seamlessly with `lazy_tool_loading` (catalog
  is unpacked at load time).
- **Docs:** new dev guide §40 `toolkit_packages.md` with full syntax,
  naming convention, exclusion semantics, edge cases, and registration
  how-to.

**Referencias:**
- Spec: `docs/superpowers/specs/2026-06-06-toolkit-packages-design.md`.
- Plan: `docs/superpowers/plans/2026-06-06-toolkit-packages-and-summaries.md`.
- Dev guide §40: `docs/developer_guide/40_toolkit_packages.md`.

**Estado.** done.

---

## 7. Text centralization + built-in tools index (E-T17 + E-T18)

- **E-T17 shipped 2026-06-06** — LLM-facing text centralization. Every
  Rust-inline tool description, summary, system prelude, and Python
  sandbox auto-prelude moved into a top-level
  [`src/libs/colmena/text/`](../src/libs/colmena/text/) folder organized
  as `prompts/*.md` (monolithic) plus `tools/*.yaml` (structured). New
  loader at `src/text/mod.rs` resolves names with `text::tool_summary` /
  `text::tool_description`. Builders panic at startup if any tool is
  missing from the registry. CI tests verify: YAML parses, no orphan
  entries, every registered tool has an entry, ToolDefinition.summary
  matches the YAML.
- **E-T18 shipped 2026-06-06** — new
  [`docs/developer_guide/41_builtin_tools_index.md`](developer_guide/41_builtin_tools_index.md)
  lists every built-in synthetic tool (31 total) with its summary and
  detailed-doc link. CI test (`index_doc_covers_all_registered_tools`)
  refuses to ship if a new tool is added without an index entry.

---

## 8. Built-in skills index (E-T19)

- **E-T19 shipped 2026-06-06** — built-in skills index. New
  [`docs/developer_guide/42_builtin_skills_index.md`](developer_guide/42_builtin_skills_index.md)
  lists every Rust-native skill (8 today) with a one-line description and
  a link to its `SKILL.md`.
  [`src/libs/colmena/skills/README.md`](../src/libs/colmena/skills/README.md)
  upgraded with contributor-side navigation + add-a-skill recipe. New CI
  test (`index_doc_covers_all_registered_skills`) refuses to ship if a
  skill folder containing a `SKILL.md` is missing from the index.

**Estado.** done.

---

## 9. E-T20 — pandas multi-sheet write-back

- **E-T20 shipped 2026-06-06** — pandas multi-sheet write-back.
  `gsheets_run_python` gains a `write_to_spreadsheet` arg and both
  `gsheets_run_python` + `crdt_doc_run_python` recognise an
  `output_sheets = {name: DataFrame, ...}` global in the user code. The
  dispatcher creates one new tab per entry (auto-suffix on collision)
  and returns metadata-only `wrote_sheets: [...]` to the LLM. Row
  contents NEVER pass through the LLM. Existing `output_sheet` +
  `write_to_sheet` single-sheet path preserved for back-compat.

---

## 10. E-T21 — Two new table-exploration skills

- **E-T21 shipped 2026-06-06** — two new skills under
  `src/libs/colmena/skills/`:
  `gsheets-table-exploration` and `crdt-doc-table-exploration`. Each
  bundles `SKILL.md` + 6 references covering inspect-first, top-N via
  `nlargest`, filter+query, group+aggregate, type coercion, and output
  shaping (with multi-tab write-back). Tool descriptions in
  `text/tools/{gsheets,crdt_doc}.yaml` updated to point at the new
  skills.

---

## 11. Sheets write safety — collision policy + `update_in_place` (P1+P2)

- **Shipped 2026-06-07** — both `gsheets_run_python` and `crdt_doc_run_python`
  gain a per-tab collision policy (`fail` default) and a new
  `update_in_place` mode that diff-writes only changed cells.
- **What changed:**
  - `output_sheets` entries now accept either a bare DataFrame (mode =
    `replace`, current behavior) or a spec dict `{mode, df, key,
    columns, strict_match, allow_schema_change}`. Three modes:
    `replace` (default), `update_in_place` (diff-write), `overwrite`
    (explicit consent + schema-change guard).
  - **Collision policy default = `fail`.** When a target tab exists and
    the entry is bare DataFrame (`replace` mode), the dispatcher cuts
    BEFORE writing and returns a structured `SheetExists` error with
    `current_state` (row/col count, header columns), `advice`, and
    three `valid_next_moves` (rename / update_in_place / overwrite).
    Operators can opt back into the old auto-suffix behavior by setting
    `fixed_config.on_existing_sheet: "auto_suffix"`. Operators who
    want destructive replace can set `"overwrite"`.
  - **`update_in_place`** fetches the current tab, diffs vs the new
    DataFrame using a unique `key` column, and writes only the changed
    cells — **one HTTPS `batchUpdate` for gsheets, per-cell ops for
    crdt_doc.** Validations: duplicate keys in either side reject;
    column mismatch rejects with rename suggestion; row count
    discrepancies surface as `skipped.rows_not_in_target` /
    `skipped.rows_null_key`.
  - **New trait method** `SheetsClient::batch_update_cells(id, sheet,
    Vec<(A1, CellValue)>)` for one-round-trip diff writes.
  - **Two new shared modules** under `llm_synthetic_tools/`:
    `sheet_collision.rs` (policy enum + structured-error builder) and
    `diff_writer.rs` (pure records-diff with NaN-safe equality + 6
    validation variants).
  - **Legacy single-tab path removed** from `crdt_doc_run_python`:
    `write_to_sheet: Option<String>` arg, `output_sheet` (singular)
    Python global, dispatcher branch, `wrote_sheet` response field,
    `PREVIEW_ROWS_IN_WROTE_SHEET` constant. 3 in-repo test graphs
    (`c_pandas_smoke`, `c_import_analysis`, `f_cross_artifact_smoke`)
    migrated to use `output_sheets = {name: df}`. 2 integration tests
    were rewritten to assert on `wrote_sheets`; 2 obsolete tests
    deleted. ADP has no consumers depending on the legacy API
    (confirmed).
- **Docs updated:** `text/tools/{gsheets,crdt_doc}.yaml` document the
  3 modes + collision policy. The two
  `{gsheets,crdt-doc}-cross-sheet-analysis` skills gain an "Updating
  existing tabs in place" section with the canonical pandas pattern.
  New E2E graph at `tests/graphs/agents/gsheets_update_in_place.json`
  for operator-driven validation against a real spreadsheet.
- **Verification:**
  - Unit tests: 1388 passed including 3 new gsheets dispatcher tests
    (`update_in_place_writes_only_changed_cells`,
    `replace_mode_default_fail_returns_sheet_exists`,
    `auto_suffix_policy_preserves_old_behavior`) + 1 new crdt_doc
    dispatcher test + 14 diff_writer tests.
  - **E2E live verified:**
    - **P1 collision `fail`** — `output_sheets = {<existing_tab>: df}`
      returned structured `SheetExists` with 12 real column names
      surfaced from the live sheet, no destructive write occurred.
    - **P2 `update_in_place` dispatch** — mode dispatched correctly,
      diff computed (4997 rows compared), response shape
      `{mode, changes, unchanged, skipped}` returned. Zero-change
      filter (pandas matched 0 rows) → 0 cells written, NO
      `batch_update_cells` call (safety guard verified).
  - `overwrite` mode covered by unit tests only (live test would
    risk destroying a real tab; deferred to operator-driven testing).
- **Breaking changes:**
  - `RunPythonArgs::write_to_sheet` and the `output_sheet` (singular)
    Python global in `crdt_doc_run_python` are **removed** — any
    downstream consumer must migrate to `output_sheets = {name: df}`.
    ADP confirmed clean.
  - Default collision behavior changed from silent `auto_suffix` to
    `fail`. Existing graphs that depended on the old behavior must
    set `fixed_config.on_existing_sheet: "auto_suffix"` explicitly.
- **References:**
  - Spec: [`docs/superpowers/specs/2026-06-06-sheets-write-safety-design.md`](superpowers/specs/2026-06-06-sheets-write-safety-design.md)
  - Plan: [`docs/superpowers/plans/2026-06-06-sheets-write-safety.md`](superpowers/plans/2026-06-06-sheets-write-safety.md)
  - 12 commits on `feature/docs`: `bdc3fc0` → `2988c5e`.

---

## 12. Suspend `resume_answer` routing fix (Approach B) — from develop

- **Shipped 2026-06-05** (merged into feature/docs via 2026-06-07).
- **Origin:** reported by ADP 2026-06-04 during HITL testing.
- **Bug fixed:** `dag_engine` stopped injecting `__colmena_resume_answer`
  into nodes that were NOT suspended in the persisted snapshot. Fixes
  the error `llm_call resume: no pending tool call found in
  conversation history` when a fresh `llm_call` is downstream of a
  `suspend`, plus the `suspend → suspend` cascade that was failing
  with `missing answer`. No public API change.
- **What changed:**
  - New helper `DagRunUseCase::compute_resuming_node_ids(all_outputs,
    resume_answer)` returns the `HashSet<String>` of node IDs whose
    persisted output carries `__colmena_status: "SUSPENDED"`
    (recursively, so orchestrator/subgraph wrap is honored).
  - In the main loop of `DagRunUseCase::run`, the set is snapshotted
    at run start. The `inputs.insert("__colmena_resume_answer", ...)`
    line is now gated by `if resuming_node_ids.contains(&node_id)`.
  - `llm.rs`: defensive fallthrough. When `resume_answer` is set but
    `find_pending_tool_call` returns `None`, log `warn!` and fall
    through to the fresh-run path instead of erroring with
    `.ok_or(...)?`. Belt-and-suspenders complement to the engine fix.
  - 4 unit tests in `resuming_node_ids_tests` mod + integration test
    `tests/suspend_resume_routing.rs`.
- **References:**
  - Spec: [`docs/superpowers/specs/2026-06-05-suspend-resume-answer-routing-fix-design.md`](superpowers/specs/2026-06-05-suspend-resume-answer-routing-fix-design.md)
  - Plan: [`docs/superpowers/plans/2026-06-05-suspend-resume-answer-routing-fix.md`](superpowers/plans/2026-06-05-suspend-resume-answer-routing-fix.md)
  - Develop commits: `8eab740`, `af87e4f`, `f9f7242`, `f674204`, `14d466e`.

---

## 13. Gemini non-object tool response wrapping — from develop

- **Shipped 2026-06-01** (merged into feature/docs via 2026-06-07).
- **Bug fixed:** Gemini's `functionResponse.response` field is typed as
  `google.protobuf.Struct` and only accepts JSON objects. The previous
  adapter only wrapped tool content in `{result: ...}` when JSON parse
  failed. If a tool returned a valid JSON scalar (`output = 5040`,
  `output = [1,2,3]`, `output = true`, `output = null`), Gemini
  silently rejected it with `400 INVALID_ARGUMENT` — observable as a
  Gemini agent dying after one turn with empty result, 0 completion
  tokens, no error event in the SSE stream.
- **What changed:**
  - `gemini_adapter.rs::adapt_messages`: the wrap logic is now a
    `match` covering all three cases:
    - `Ok(v) if v.is_object()` → pass through unchanged.
    - `Ok(v)` (any other valid JSON) → wrap as `{result: v}`.
    - `Err(_)` (free-form string) → wrap as `{result: <string>}`.
  - Objects pass through unchanged so callers that already return
    dicts keep their keys (no double-wrapping).
  - Regression tests covering scalar number, scalar string, array,
    bool, null.
- **Audit:** OpenAI and Anthropic adapters audited clean — they pass
  tool content as opaque strings, never as raw JSON values.
- **ADP impact:** wire-format change between Colmena and the Gemini
  REST API only; never crosses the SSE boundary that ADP consumes.
- **Verified end-to-end** against the live Gemini API with a
  `python_script` tool returning `output = 5040`: Gemini now correctly
  responds "The output is 5040." where it previously went mute.
- **References:**
  - Plan: [`docs/superpowers/plans/2026-06-01-gemini-scalar-tool-response-fix.md`](superpowers/plans/2026-06-01-gemini-scalar-tool-response-fix.md)
  - Develop commits: `b6412a6`, `d99e975`.

---

## 14. HITL email approval demo graph — from develop

- **Shipped 2026-06-06** (merged into feature/docs via 2026-06-07).
- **What changed:** new graph at
  [`tests/graphs/basic/suspend_email_approval_demo.json`](../tests/graphs/basic/suspend_email_approval_demo.json)
  demonstrating `suspend` + `router` for a HITL approval workflow.
  Companion to subsystem 12 (suspend resume_answer routing fix) —
  validates the fix via a realistic graph.
- **Also lands:** `tests/graphs/basic/suspend_cascade.json` and
  `tests/graphs/basic/suspend_then_llm_resume.json` — canonical
  smoke graphs that the integration test runs against.
- **References:** develop commit `772c9f3`.

---

## 15. Google Docs integration (subsystem G) — shipped 2026-06-08

**Qué cambió.** 22 tools sintéticos `gdocs_*` que reflejan el modelo de
edición quirúrgica direccionada por contenido — el agente describe **qué**
cambiar (`find`, anchor, heading, named range), nunca offsets UTF-16.
Soporte multi-tab, conversión markdown ↔ Docs con `lossy_conversions`
estructurado, y seguridad ante co-edición vía Drive Revisions diff +
tabla postgres `gdocs_session_state(agent_session_id, document_id,
last_revision_id)`. Auth por Service Account JSON o ADC; requiere
`COLMENA_GDOCS_DEFAULT_PARENT_FOLDER_ID` (o `parent_folder_id` per-call)
para creación.

**Toolkit aliases.**
- `gdocs` — los 22 tools completos.
- `gdocsread` — subset de solo-lectura (6 tools: list_tabs,
  read_as_markdown, read_outline, list_named_ranges, export,
  acknowledge_human_changes).

**Por qué importa.** Google Docs no expone un modelo "celda" como
Sheets — internamente es un stream lineal indexado por UTF-16. Pedirle
al LLM que calcule esos índices es una receta para off-by-one. v1
elimina por completo esa superficie con tres decisiones de diseño:
content-addressed, markdown como I/O, co-edit guard por revisión.

**Documentación de referencia.**
- Dev guide: [`docs/developer_guide/45_gdocs.md`](developer_guide/45_gdocs.md)
- Spec: [`docs/superpowers/specs/2026-06-08-google-docs-design.md`](superpowers/specs/2026-06-08-google-docs-design.md)
- Plan: [`docs/superpowers/plans/2026-06-08-google-docs.md`](superpowers/plans/2026-06-08-google-docs.md)
- Smoke graph: [`tests/graphs/agents/gdocs_smoke.json`](../tests/graphs/agents/gdocs_smoke.json)

**Breaking changes.** Ninguno — puramente aditivo. Nueva tabla postgres
`gdocs_session_state` con migración propia.

**Impacto ADP.** Cero breaking changes. ADP opta-in habilitando el
toolkit `gdocs` o `gdocsread` en `enabled_tools` del agente, más
`GOOGLE_APPLICATION_CREDENTIALS` + `COLMENA_GDOCS_DEFAULT_PARENT_FOLDER_ID`
en el worker. El requisito de `SECURE_VALUES_KEY` del §1 sigue
aplicando.

**Limitaciones v1 (deferidas a v1.1 — ver BACKLOG "Subsystem G v1.1").**
- `mode: "suggest"` no implementado (los writes son siempre `direct`).
- Tablas markdown en `gdocs_insert_*`, `gdocs_replace_section`,
  `gdocs_append_markdown` y `gdocs_apply_edits` se rechazan con
  `invalid_args`. Tablas en `gdocs_create_from_markdown` sí funcionan
  (Drive las convierte nativamente).
- Edits quirúrgicos a celdas de tabla (`set_table_cell`,
  `insert_table_row`) no existen aún.
- `gdocs_create_from_docx` devuelve `not_yet_wired` (falta plumbing de
  attachment-fetcher).
- `gdocs_export` devuelve raw `byte_len` sin registrar attachment.
- `gdocs_add_tab` no siembra `markdown` (responde con
  `pending_markdown_seed: true`).
- `gdocs_read_as_markdown` con `tab_id` devuelve el doc completo (slicing
  per-tab v1.1).
- `gdocs_create_named_range` solo soporta `Scope::Paragraph` a nivel del
  API.
- No hay Drive Comments API, ni insertion de imágenes, ni Apps Script,
  ni rollback de revisiones, ni OAuth user-scoped.

**Estado.** done.

**Commits (G-T1 a G-T24).** Ver `git log develop --grep="gdocs"`.

---

## 16. Subsystem G live-verified + 9 fixes 2026-06-09

**Qué pasó.** End-to-end verification del Subsystem G contra un Google
Doc real compartido por el usuario
(`1QkeEG4PU0PFBwDs8dP6WaYUIEafVwjL3D27eA1w8f0k`, SA
`colmena-sheets-tester@startti-dev.iam.gserviceaccount.com`). El flujo
canónico funciona end-to-end:

1. **Phase 1** — el agente arma su plan en un tab nuevo (`add_tab` +
   content seeding via `apply_edits`).
2. **Out-of-band** — el usuario edita el doc manualmente desde el browser.
3. **Phase 2** — la siguiente `replace_text` del agente detecta drift y
   devuelve `human_changes_pending`.
4. **Acknowledge** — el agente llama `gdocs_acknowledge_human_changes`.
5. **Retry** — la operación procede y completa exitosamente.

Verificado live 2026-06-09 04:25 UTC.

**Nueve fixes shipped durante la verification** (commits sobre `develop`):

| Commit | Bug |
|---|---|
| `b82bd35` | `llm.rs` no exponía los gdocs tools — el LLM solo veía `recall_history` |
| `79eae72` | Default scope `drive.file` rechazaba docs user-shared con `appNotAuthorizedToFile`. Cambiado a `drive` |
| `8c9ea0e` | `append_markdown` usaba placeholder `index: 0` → HTTP 400 de Google |
| `940d508` | `list_tabs` con `includeTabsContent=false` devolvía vacío (Google omite el field). Cambiado a `true` |
| `baaf48d` | `markdown_to_docs_ops` no emitía `location.tabId`; `add_tab` no invalidaba el cache → contenido iba a tab 1 |
| `8de4922` | Use cases application emitían `range`/`location` JSON sin `tabId` → `replace_text` decía "ok" pero escribía en la tab equivocada |
| `081b0d2` | Dispatcher keyaba el cursor de co-edit en `session_id` (UUID ephemeral por CLI run). Cambiado a `agent_session_id` (estable, alineado con la regla de CLAUDE.md) |
| `c868794` | **Pivot de diseño.** Drive Revisions API no devuelve log per-edit para Google Docs nativos (solo named versions). v1 ya NO ofrece diff párrafo-por-párrafo en `human_changes_pending` — solo señal "algo cambió". El diff completo queda para v1.1 con snapshot caching en postgres (ver BACKLOG) |
| `de05cbf` | Use cases guardaban el `writeControl.requiredRevisionId` de `batch_update` (~25 chars) en vez del `revisionId` de `documents.get` (~111 chars) → falso positivo `human_changes_pending` en cada turno. Fixed capturando snapshot post-write |

**Limitaciones operacionales documentadas** (ver
[`developer_guide/45_gdocs.md`](developer_guide/45_gdocs.md) §"Limitaciones
en v1"):

- Realidad de ownership: `create_*` falla en folders personales de Gmail
  por `storageQuotaExceeded` (SA con quota cero). Patrón v1 robusto =
  "user-creates-first, agent-edits-only".
- `add_tab` funciona en docs legacy single-tab (lo que se creía
  inicialmente como limitación era el bug #4 enmascarando).
- `add_tab` con arg `markdown` responde `pending_markdown_seed: true` y
  no siembra contenido (v1.1).
- `dispatch_create_from_docx` devuelve `not_yet_wired` (attachment
  plumbing v1.1).
- `dispatch_export` devuelve `byte_len` sin envolver bytes como
  attachment (v1.1).

**Documentación actualizada.**
- [Spec §17 "Live verification findings 2026-06-09"](superpowers/specs/2026-06-08-google-docs-design.md)
- [Dev guide](developer_guide/45_gdocs.md) — nueva sección "Limitaciones
  en v1", co-edit pipeline reescrito alrededor del pivot, scope default
  documentado.
- [BACKLOG → Subsystem G v1.1](BACKLOG.md) — refreshed con 10 items
  priorizados (OAuth user-scoped y paragraph-level diff arriba).

**Impacto ADP.** Cero breaking changes adicionales. El feature shipped
en §15 sigue siendo el mismo wire-format público; los 9 fixes son
correcciones internas. La SA `colmena-sheets-tester` queda como SA de
testing oficial para gdocs (mismo patrón que gsheets).

**Estado.** done.

---

## 17. Google Docs co-edit guard v1.1 — paragraph-level human-change diff (2026-06-09)

**Qué cambió.** Cuando el guard detecta drift (humano editó entre dos
writes del agente), ahora devuelve una **lista concreta de cambios
paragraph-level** — particionada por overlap con el scope intencionado:

- `changes_overlapping_scope` (con `before_text` / `after_text` /
  `tab_id` por cambio) → block con `human_changes_pending` poblado.
- `changes_outside_scope` → el edit procede, con cambios listados como
  `soft_warnings` para awareness del agente.

Implementación: persistimos el `DocumentSnapshot` post-write en
`gdocs_session_state.last_snapshot_json` (cap 1 MB; opt-out vía
`COLMENA_GDOCS_MAX_SNAPSHOT_BYTES`) y corremos Myers diff via crate
`similar`. Cero API calls extra — el snapshot que persistimos es el
mismo que ya hidratamos para construir `EditResult.outline_snapshot`.

Instancias sin la migración aplicada degradan grácilmente a v1
behavior (`gdocs.snapshot.column_missing` warn al boot; listas vacías
y block conservador en cada drift).

**Por qué importa.** Antes de v1.1, el LLM recibía
`human_changes_pending` con listas vacías y tenía que llamar
`read_outline` / `read_as_markdown` para entender qué cambió — turn
extra + tokens + comparación contra memoria potencialmente truncada.
Ahora el contexto completo del cambio llega en el error mismo,
incluido `before_text` y `after_text` por párrafo.

**Documentación de referencia.**
- Spec: [`docs/superpowers/specs/2026-06-09-gdocs-paragraph-diff-design.md`](superpowers/specs/2026-06-09-gdocs-paragraph-diff-design.md)
- Plan: [`docs/superpowers/plans/2026-06-09-gdocs-paragraph-diff.md`](superpowers/plans/2026-06-09-gdocs-paragraph-diff.md)
- Propuesta original (parking lot): [`docs/proposals/2026-06-09-gdocs-oauth-user-flow.md`](proposals/2026-06-09-gdocs-oauth-user-flow.md)
- Dev guide §"Co-edit safety pipeline" reescrito:
  [`docs/developer_guide/45_gdocs.md`](developer_guide/45_gdocs.md)

**Cambios técnicos clave.**
- Migration `20260609000000_gdocs_session_state_snapshot.sql` —
  additive `ALTER TABLE ADD COLUMN IF NOT EXISTS
  last_snapshot_json JSONB, last_snapshot_size_bytes INTEGER`.
- `HumanChange` extendido con `tab_id`, `before_text`, `after_text`
  (additive, `#[serde(skip_serializing_if = "Option::is_none")]`).
- `ResolvedScope::contains_paragraph` helper para partition.
- `gdocs/application/diff.rs` — `paragraph_diff` puro vía Myers
  (`similar = "2"` ya estaba en `Cargo.toml`).
- `RevisionStore` extendido con `get_with_snapshot` /
  `put_with_snapshot` (legacy `get`/`put` son default shims, backward-compat).
- `PostgresRevisionStore::new` ahora async, probea
  `information_schema.columns` y degrada con warn si las columnas no
  existen.
- 8 use cases (`delete_text`, `style`, `replace_text`,
  `replace_section` ×2, `named_range`, `insert`, `apply_edits`)
  actualizados para pasar `Some(&fresh)` a `put_with_snapshot`.

**Tests.** 1547 unit tests pasan (incluido 9 nuevos en
`diff::tests` + 3 nuevos en `co_edit_guard::guard_tests`).
ADP worker recompila clean contra esta colmena (verificado vía
`cargo check`).

**Impacto ADP.** Migration additive — instancia sin schema aplicado
arranca con warn + funciona en modo degraded. **Pending del lado ADP:**
agregar `lastSnapshotJson Json?` + `lastSnapshotSizeBytes Int?` al
schema Prisma. Detalles en
[`ADP_PRISMA_PENDING_TABLES.md`](../ADP_PRISMA_PENDING_TABLES.md) §5.

**Estado.** done.

---

## 18. SQL node — multi-statement support (Política C) + LLM-facing docs (2026-06-09)

**Qué cambió.** El nodo `sql_query` ahora ejecuta queries con múltiples
statements separados por `;` de forma nativa. Antes fallaba con error
críptico `cannot insert multiple commands into a prepared statement`
porque `sqlx::query().execute()` usa el extended protocol de Postgres
(PREPARE + BIND + EXECUTE) que solo acepta UN comando por mensaje.

**Política C — atomic loop sobre AST statements.** El refactor:
- Parsea la query con `sqlparser`, obtiene `Vec<Statement>`.
- Inicia UNA transacción.
- Itera los statements ejecutándolos con `sqlx::query(stmt.to_string())`.
- El output devuelto es el del ÚLTIMO statement:
  - SELECT → rows (con LIMIT auto solo si no tiene)
  - INSERT/UPDATE/DELETE → `{rows_affected: SUM_de_todos}`
  - CREATE TABLE → `{created: true, type: "table"}`
  - CREATE FUNCTION → `{created: true}`
- SELECTs intermedios se ejecutan pero su resultado se descarta.
- Cualquier fallo → rollback completo (atomicidad preservada).

**Por qué importa.** El LLM tiende a escribir naturalmente:
```sql
INSERT INTO orders (...) VALUES (...);
INSERT INTO order_items (...) VALUES (...);
UPDATE inventory SET ... WHERE ...;
```
Cada `;\n` rompía la query. Ahora corre todo en una TX atómica.

**Docs LLM-facing.** Dos capas:
- `build_description_supplement` (always-on): bloque corto con
  multi-statement note + lista visual `NO: ... → ...` con anti-patterns
  (BEGIN/COMMIT, $1/?/:name, TRUNCATE/DROP, etc.). ~150 tokens extras por
  turn.
- Nueva skill built-in `sql-query-best-practices` (opt-in vía
  `llm_call.skills.paths`): 6 references on-demand con ejemplos
  visuales ✅/❌ — `multi_statement`, `bulk_insert`,
  `select_after_mutation`, `anti_patterns`, `schema_discovery`,
  `error_recovery`.

**Cambios técnicos clave.**
- `sql_pool_adapter.rs::execute_query` — refactor completo a Política C
  (~80 LOC modificadas + helper `marshall_rows()` extraído).
- `sql.rs:396` — fix UTF-8 panic en log preview (`chars().take(100)` en
  vez de byte slicing).
- `sql.rs::build_description_supplement` — append L1 anti-patterns block.
- Nuevo dir `src/libs/colmena/skills/sql-query-best-practices/` con
  `SKILL.md` + 6 references.
- 7 nuevos tests `#[ignore]`-gated en
  `sql_pool_adapter::tests::pc_*` cubriendo: single insert, multi-insert
  aggregation, rollback atomicity, insert+select, intermediate-select
  discard, LIMIT en final SELECT, multi-line formato de 1 stmt.
- 1 test agregado en `builtin_skill_repository::tests::sql_query_best_practices_is_loadable`
  para catch regresiones del skill.
- Live verification graph `tests/graphs/agents/sql_multistatement_repro.json`.

**Documentación actualizada.**
- Dev guide: nueva sección §"Multi-statement queries (Política C)" en
  [`docs/developer_guide/23_sql_node.md`](developer_guide/23_sql_node.md).
- BACKLOG: item 12 ("SQL INSERT multi-line bug") marked done.

**Tests.** 7/7 nuevos integration tests pasan (`cargo test --ignored`).
1572+ unit tests total pasan. ADP worker recompila clean.

**Impacto ADP.** Cero breaking changes. La señal externa (output
shape) no cambia para queries single-statement; multi-statement antes
fallaba con error, ahora funciona. ADP worker recompila clean sin
modificaciones.

**Estado.** done.

---

## 19. SQL node — fix NUMERIC columns marshalling as null (2026-06-09)

**Qué cambió.** Las columnas `NUMERIC` / `DECIMAL` que retornaban `null`
en SELECT outputs ahora vuelven como JSON numbers (f64). Bug detectado
durante el E2E LLM-in-the-loop del item §18 (`SELECT amount FROM …` para
testear Política C devolvía `amount: null` aunque la DB tenía
`amount: 100.50`).

**Root cause.** `sqlx::query().fetch_all()` devuelve `PgRow`s; en
`marshall_rows` el código intentaba `row.try_get::<f64, _>(name)` para
columnas con `type_info.name() == "NUMERIC"`. sqlx-postgres NO soporta
esa coerción directamente — NUMERIC es de precisión arbitraria
(hasta ~131k dígitos en Postgres) y f64 tiene precisión limitada
(~15-17 sig digits). sqlx te obliga a pedir el tipo intermedio
explícitamente: `BigDecimal` (feature `bigdecimal`) o `Decimal`
(feature `rust_decimal`). Ninguno estaba habilitado → `try_get`
fallaba y `.unwrap_or(Value::Null)` silenciaba el error.

**Fix.** Habilitamos `sqlx` feature `bigdecimal` + agregamos la dep
`bigdecimal = "0.4"` al `Cargo.toml`. La rama `NUMERIC` en
`marshall_rows` ahora hace `BigDecimal → string → f64`:

```rust
"NUMERIC" => row
    .try_get::<sqlx::types::BigDecimal, _>(name)
    .ok()
    .and_then(|bd| bd.to_string().parse::<f64>().ok())
    .map(|v| json!(v))
    .unwrap_or(Value::Null),
```

**Trade-off documentado.** Para valores con >15 sig digits
(financieros estrictos), el LLM debe castear a TEXT en su SELECT
(`amount::TEXT`). Documentado en:
- Dev guide §"Tipo-mapping de columnas" en
  [`23_sql_node.md`](developer_guide/23_sql_node.md).
- Skill `sql-query-best-practices`, reference `error_recovery`.

**Bug scope.** El bug existía desde la implementación inicial del
nodo — `marshall_rows` (antes inline en `execute_query`) tenía la
misma lógica fallida. **No fue introducido por Política C** del §18;
solo se hizo más visible al E2E live verification con un modelo real.

**Otros nodos afectados.** Ninguno. El único punto de marshalling
genérico de `PgRow → JSON` está en `sql_pool_adapter::marshall_rows`.
Los demás usos de sqlx en colmena (load_table_metadata,
sql_function_registry, dag_state_repository, secure_value_mappings,
llm_node_history) decodifican tipos específicos conocidos (TEXT, INT,
BOOL) en queries fixas del sistema. `crdt_doc_*` / `gsheets_*` /
`gdocs_*` no usan sqlx para resultados LLM-facing.

**Tests.** 1 nuevo integration test `pc_numeric_column_marshalls_as_f64_not_null`
(7→8 `pc_*` tests). Verifica que NUMERIC con valores 100.00, 1.81,
999.99 y 12345.6789 vuelven como JSON numbers, no nulls.

**Live verification re-runned.** El graph
`tests/graphs/agents/sql_multistatement_e2e_llm.json` ahora muestra
`amount: 100.0, 200.0, 350.0` en vez de `amount: null`.

**Impacto ADP.** Cero breaking changes. La señal cambia de
`{amount: null}` (silencioso) a `{amount: 100.50}` (correcto). ADP
worker recompila clean.

**Estado.** done.

---

## 20. Provider-level prompt caching enabled by default (Anthropic + Gemini, 2026-06-09)

**Origen.** Item 11 de la cola priorizada del BACKLOG (lvl-up de "CRDT
Documents v1.1 — Provider-level prompt caching"). Antes del fix, dos de
los tres providers ignoraban completamente prompt caching:

| Provider | Lectura de stats | Marker en request | Estado pre-fix |
|---|---|---|---|
| OpenAI | ✅ `cached_tokens` | N/A (cache automático server-side) | funcionando |
| Anthropic | ✅ Leía `cache_read_input_tokens` | ❌ NUNCA seteaba `cache_control` | **stats siempre = 0** |
| Gemini | ❌ NO leía `cachedContentTokenCount` | ❌ Ni implicit ni explicit | **0% visibilidad** |

**Fix shipped.** Cambios localizados a 2 adapters:

- **Anthropic** (`anthropic_adapter.rs::build_request_body`):
  agregadas 2 cache-control markers `{type: "ephemeral"}` en el body de
  cada request:
  1. **System message** ahora serializado como content-block array
     (`[{type: "text", text: "...", cache_control: {type: "ephemeral"}}]`)
     en vez de plain string. Anthropic acepta ambas formas y la billing
     es idéntica en uncached, pero solo la forma con marker activa
     caching en repeats.
  2. **Último tool de `tools[]`** recibe `cache_control: ephemeral`,
     marcando el array completo de tool definitions como prefix
     cacheable.
  
  Calls subsecuentes del mismo agente dentro de los 5 minutos siguientes
  se billan al ~10% del precio normal sobre la porción cacheada (system
  + tools). Conversational tail (user/assistant messages) NO se cachea
  porque cambia cada turno — cachearla causaría cache-write churn sin
  read benefit.

- **Gemini** (`gemini_adapter.rs`): agregado el campo
  `cachedContentTokenCount` al struct `GeminiUsage` y mapeado a
  `LlmUsage::cache_read_tokens` en ambos paths (call sync + streaming).
  Gemini 2.5+ models (gemini-2.5-flash default + 2.5-pro) tienen
  **implicit caching automático server-side** (lanzado mayo 2025) —
  no requiere ningún marker en el request. Mínimos: 1024 tokens para
  2.5-flash, 2048 para 2.5-pro. Solo hacía falta surface las stats.

**OpenAI: sin cambios.** Su caching es 100% automático server-side para
prefixes ≥1024 tokens y el adapter ya lee `cached_tokens`. Quedaba como
único provider funcionando out-of-the-box; ahora los tres están en
paridad.

**Por qué Path A (implicit) y no Path B (CachedContent API explícita) en Gemini.**
El BACKLOG original proponía implementar Cached Content API explícita
para Gemini (~3h de complejidad: state mgmt del cache ID, TTL refresh).
Pero Gemini 2.5+ tiene implicit caching que cubre el mismo use case con
~30 LOC de cambio (solo lectura de stats). Path B se justifica solo si
operadores quieren forzar cache hits para system prompts especialmente
grandes — diferido al BACKLOG como follow-up si aparece el use case.

**Tests agregados** (`cargo test --lib`):
- `anthropic_adapter::tests::cache_control_marker_on_system_message_block`
- `anthropic_adapter::tests::cache_control_marker_on_last_tool_only`
- `anthropic_adapter::tests::cache_control_works_without_tools`
- `gemini_adapter::tests::usage_metadata_with_cached_content_populates_cache_read_tokens`
- `gemini_adapter::tests::usage_metadata_without_cache_omits_cache_read_tokens`
- `gemini_adapter::tests::usage_metadata_with_zero_cached_tokens_does_not_set_field`

Suite full pasa (verificado en T5).

**Impacto ADP / breaking changes.** **CERO breaking changes.**
- Wire-format change Colmena ↔ provider API — nunca cruza el SSE boundary.
- `LlmUsage` shape no cambia (campos ya existían).
- ADP worker recompila clean sin tocar nada.
- Operadores no necesitan opt-in: caching es ON por default.

**Ahorro esperado en producción.**
- Anthropic: ~90% descuento sobre tokens cacheados de system + tools.
  Para un agente típico con ~5K tokens de system + tools y 10 turnos,
  ahorro = ~45K tokens × 90% = ~40K tokens/conversación.
- Gemini 2.5: 25-75% descuento sobre tokens cacheados (server-side
  determina exact rate). Implicit caching aplica automáticamente cuando
  el prefix supera 1024 tokens.
- OpenAI: sin cambios (ya estaba activo).

**Verificación.** Tests unit confirman shape correcto del request body
(Anthropic) y parsing de stats (Gemini).

**E2E LLM-in-the-loop:**
`tests/graphs/agents/provider_cache_anthropic_e2e.json` — agente
claude-haiku-4-5 con system message padded a ≥2048 tokens (mínimo
cacheable para haiku). Patrón two-run: correr el mismo graph dos veces
seguidas con el mismo `--agent-session-id` dentro de los 5 min de TTL;
el segundo SSE debe traer `cache_read_tokens > 0`. Comando:

```bash
set -a && source .env && set +a
SESS=cache_test_$(date +%s)
cargo run --bin dag_engine -- run tests/graphs/agents/provider_cache_anthropic_e2e.json \
  --agent-session-id $SESS --include-extra-info | tee /tmp/cache_run1.sse
cargo run --bin dag_engine -- run tests/graphs/agents/provider_cache_anthropic_e2e.json \
  --agent-session-id $SESS --include-extra-info | tee /tmp/cache_run2.sse
grep -oE '"cache_(read|write)_tokens":[0-9]+' /tmp/cache_run1.sse /tmp/cache_run2.sse
```

El test `cache_control_marker_on_last_tool_only` previene regresión del
shape en el request body.

**Estado.** done.

---

## 21. gsheets / gdocs `enabled_tools` alias + exclusions fix (2026-06-09)

**Qué cambió.** `enabled_tools: ["gsheets"]` ahora expande correctamente a los 10 sub-tools `gsheets_*`. La sintaxis `!sub_tool` para excluir tools del paquete también funciona (e.g. `["gsheets", "!gsheets_export_xlsx"]` → 9 tools expuestos). Mismo fix aplicado a `gdocs` / `gdocsread`. La lógica wants/excludes se extrajo al helper testable `resolve_synthetic_enabled_tools` (en `dag_engine/infrastructure/nodes/llm.rs`), que mantiene paridad de semántica con `filter_enabled_tools` (usado para el catálogo del executor).

**Por qué importa.** Bug observado desde ADP: un agente con `enabled_tools: ["gsheets", "!gsheets_create_from_xlsx", "!gsheets_export_xlsx"]` no recibía ningún tool gsheets. El bloque sintético en `llm.rs:2122` hacía un literal-match sin pasar por `find_package()`, mientras que el bloque equivalente de `gdocs` (escrito después) sí expandía el alias. Ninguno de los dos manejaba las exclusiones `!entry`. Resultado: el agente respondía "no tengo herramientas para Google Sheets" — comportamiento idéntico a no tener tools.

**Documentación de referencia.**
- Tests de regresión: `dag_engine::infrastructure::nodes::llm::resolve_synthetic_enabled_tools_tests` (11 tests, incluye el payload exacto que rompía desde ADP).
- CLAUDE.md sección "Tool Config Standard — `enabled_tools`" actualizada: gsheets/gdocs/gdocsread ahora son flag-only oficialmente.

**Verificación.**
- 1562 tests unit pasan (`cargo test --verbose`).
- E2E con el grafo ADP (gpt-4o-mini, `lazy_tool_loading: false`): el agente enumera correctamente los 8 sub-tools gsheets disponibles (10 - 2 excluidos).
- E2E con `lazy_tool_loading: true`: el catálogo se inyecta vía `tools_discovered` en el system message; el `describe_tool` puede listarlos a demanda.

**BREAKING.** Ninguno. Cambios puramente aditivos para `enabled_tools: ["gsheets"]` (antes: 0 tools; ahora: 10 tools). Si algún graph en producción dependía del comportamiento previo (que probablemente no, porque era roto), tendría que migrar a un listado explícito de sub-tools — pero ningún grafo conocido se comportaba así intencionalmente.

**Estado.** done.

---

## 22. gdocs `apply_edits` — fix critical index-drift cross sub-edit (2026-06-10)

**Qué cambió.** `gdocs_apply_edits` ahora hace **resolve → sort global write-backwards → emit**. La implementación previa sorteaba write-backwards solo DENTRO de cada sub-edit, no a través del batch entero. Cuando un compound combinaba múltiples replace/delete con hits en distintos párrafos (o multi-hit por sub-edit), el primer sub-edit modificaba el doc y los snapshot-derived offsets de los siguientes ya estaban corridos respecto al estado actual. Resultado: la API de Google aplicaba deletes/inserts en posiciones incorrectas, corrompiendo texto vecino.

**Cambios estructurales en [`apply_edits.rs`](../src/libs/colmena/src/gdocs/application/apply_edits.rs):**
- Nuevo `struct ResolvedEmit { paragraph, byte_off, byte_len, requests, change }` captura cada edit atómico con su posición en el snapshot original.
- Nuevo helper `find_hits(snap, find, scope)` deduplica la lógica de búsqueda que tenían `ReplaceText` y `DeleteText`.
- Nuevo helper `check_no_overlaps_within_paragraph(emits)` detecta ranges solapados en el mismo párrafo (que no se pueden interleavar de forma segura) y devuelve `InvalidArgs` con un mensaje accionable.
- El flujo principal ahora es: PHASE A resuelve cada `ApplyEditOp` en `Vec<ResolvedEmit>` sin emitir requests; PHASE B detecta overlaps, hace un `sort_by_key(|r| Reverse((r.paragraph, r.byte_off)))` global, luego flatten a `all_requests` + `all_changes`. Dentro de un `ResolvedEmit` (e.g. markdown insert con múltiples requests) el orden se preserva porque Google evalúa cada request contra el estado tras las previas.

**Por qué importa.** Bug observado en `agent_session_id=cmq7kem1h003001s6mr36uwe8` (2026-06-10, dev): el agent llamó `apply_edits` con 7 `replace_text` para añadir estilos markdown a un plan de ejercicios; el doc resultante tenía párrafos como `"Crunche- **Enfriamiento:** Estiramientos de 5-10 minutos.: Estiramientos..."` con texto cortado a media palabra y fragmentos pegados. El root cause era que el sort por-sub-edit no protegía la invariante write-backwards cross-batch.

**Tests de regresión** (en [`apply_edits.rs:app_tests`](../src/libs/colmena/src/gdocs/application/apply_edits.rs)):
- `apply_edits_global_write_backwards_sort_across_sub_edits` — replica el escenario del bug (3 sub-edits, 5 hits totales repartidos en 3 párrafos) y assertea que las `deleteContentRange.startIndex` salgan en orden estrictamente decreciente.
- `apply_edits_overlapping_ranges_in_same_paragraph_rejected` — assertea que dos replace cuyos byte ranges se solapan dentro de un párrafo aborten con `InvalidArgs` antes de cualquier write.
- `apply_edits_disjoint_ranges_same_paragraph_ok` — assertea que ranges disjuntos en el mismo párrafo sigan funcionando con orden write-backwards correcto.

**BREAKING.** Ninguno semánticamente para los happy paths. Cambia el comportamiento solo en dos escenarios que antes corrompían silenciosamente:
- Compounds con multi-hit + multi-paragraph: ahora funcionan correctamente (antes corrompían).
- Overlapping ranges en el mismo paragraph: ahora devuelven `InvalidArgs` con mensaje accionable (antes corrompían o se aplicaban inconsistentemente).

**Verificación.**
- 1603 unit tests pasan (`cargo test --verbose`).
- 7 tests específicos de `apply_edits` (4 existentes + 3 nuevos) pasan.
- 1556 → 1603 = +47 tests netos respecto al snapshot anterior (incluye los 11 de `resolve_synthetic_enabled_tools` + nuevos).

**Bugs secundarios identificados** (al backlog, no shipped acá):
- `apply_edits` no enforça `ConfirmManyMatches` threshold (≥5 hits) como sí lo hace standalone `replace_text`. El LLM puede replace-all sin signal.
- El LLM no usa `scope`/`anchor` para limitar finds que matchean en múltiples días/secciones. Educable vía skill auto-loaded.

**Estado.** done (P0 fix). Backlog: items menores en §Subsystem G v1.1.

---

## 23. Google Workspace prelude — auto-inyectado para agentes con gsheets/gdocs (2026-06-10)

**Qué cambió.** Todo `llm_call` cuyo catálogo expone algún tool `gsheets_*` o `gdocs_*` ahora recibe un bloque adicional en el system message:
1. Exige el ID del documento explícitamente (extraído de URL del usuario, o pedido al usuario si no está confirmado).
2. Le dice al LLM cuál es el SA email con el que el usuario debe compartir el doc (resuelto en runtime).
3. Le indica que NO debe adivinar IDs ni operar sin doc_id confirmado.

**Resolución del SA email** (en orden): env var `COLMENA_GOOGLE_SA_EMAIL` → `client_email` del JSON apuntado por `GOOGLE_APPLICATION_CREDENTIALS` → `None` con fallback degraded (le pide al user que consulte al operador para la dirección).

**Implementación.**
- Nuevo módulo `dag_engine/infrastructure/nodes/llm_synthetic_tools/google_workspace_prelude.rs` con:
  - `resolve_sa_email() -> Option<String>` — cadena env → JSON → None.
  - `build_google_workspace_prelude(sa_email: Option<&str>) -> String` — prompt v1 hardcoded en ES.
  - `has_google_workspace_tools(tool_names)` — gate para gating.
- Re-export desde `llm_synthetic_tools/mod.rs`.
- Inyección en `llm.rs` en el system-message section builder, después del CRDT prelude y antes del `system_message` del usuario. Gateado por `has_google_workspace_tools(tools.iter().map(|t| t.name.as_str()))`.

**Costo.** ~140 tokens fijos por turno con email; ~110 sin email. Always-on (no detecta si el doc_id ya está en scope). El LLM aprende rápido a saltearlo cuando ya tiene contexto. Trade-off: pequeño overhead constante vs comportamiento determinístico del turno 1.

**Por qué importa.** Antes del fix: el LLM con `gsheets`/`gdocs` enabled, cuando el usuario decía "agregale una fila al sheet", inventaba un doc_id o llamaba la tool con info incompleta → `PermissionDenied` o tool error. Frustrante en turno 1. Ahora el LLM pide el ID y avisa del share con la SA en el primer turno.

**Verificación.**
- 8 tests unit en `google_workspace_prelude::tests` (resolución de email, fallback degraded, detección de tools, casos de JSON corrupto/missing).
- E2E con grafo OpenAI gpt-4o-mini, `enabled_tools: ["gsheets", ...]`, prompt "agregale una fila al sheet":
  - Sin fix: LLM hallucinaba o ejecutaba tool con args incompletos.
  - Con fix: LLM responde `"Por favor, proporciona el ID del documento de Google Sheets al que deseas agregar una fila. Además, asegúrate de que el documento esté compartido como Editor con colmena-sheets-tester@startti-dev.iam.gserviceaccount.com."` ✓
- Token bump observado: 2278 → 2529 (+251) en grafo de prueba.

**BREAKING.** Ninguno. El prelude solo se inyecta en agentes con tools de Google Workspace; agentes sin gsheets/gdocs no ven cambios. ADP worker recompila clean.

**Configuración requerida en operador.**
- Si la SA JSON está en `GOOGLE_APPLICATION_CREDENTIALS`: el email se auto-resuelve. **Cero config.**
- Si corre con ADC (Cloud Run sin JSON file): setear `COLMENA_GOOGLE_SA_EMAIL=<sa-email>` como env var. ADP debe agregar esta env var a `deploy_gcp.sh` antes del próximo deploy a prod.
- Si no se setea ninguna: el prelude usa el path degraded (pide el doc_id y le dice al user que consulte al operador para el share). Funciona pero menos fluido.

**Estado.** done.

---

## 22. Tabular attachment auto-summary (CSV/XLSX) in catalog block (2026-06-10)

**Origen.** Pregunta del owner post item 13: "¿el LLM puede leer solo una parte
para entender el schema sin tener que leer todo?" La respuesta era "sí, vía
`sql_inspect_attachment`, pero solo cuando el operador habilita SQL tools".
Para agentes sin SQL configurado, el LLM tenía 2 caminos malos:
1. `load_attachment` → ~50K tokens para un CSV de 1487 rows
2. Adivinar desde el filename → erróneo

**Fix shipped.** Auto-summary estructurado para CSV/XLSX en el catalog block
del system message. **Zero LLM tokens** consumidos en summarization — el
parser local de `parse_inspect_bytes` produce el summary directamente.

**Resultado live verificado** (`bulk_e2e_auto_002` session, 2026-06-10):

```
CSV, 5 cols × 100 rows (delimiter ',')
schema: product_id (integer), sku (text), name (text), price (numeric), stock (integer)
sample rows:
  1, SKU001, Product 1, 10.49, 10
  2, SKU002, Product 2, 10.99, 20
  3, SKU003, Product 3, 11.49, 30
```

~150 tokens persistidos en `conversation_attachments.description`. El LLM
los ve desde el turn 1 sin ningún round-trip de tool call.

**Cambios:**

| Componente | LOC | Función |
|---|---|---|
| `sql_bulk_tools::build_tabular_summary(mime, filename, bytes)` | ~110 | Public helper que detecta CSV/XLSX, parsea con `parse_inspect_bytes`, formatea como string compacto. Reusa `MAX_ATTACHMENT_BYTES` cap (50 MB). |
| `sql_bulk_tools::format_tabular_summary` (interno) | ~50 | Render del response como string (header line + schema line + sample lines). Trunca cells > 40 chars con `...` para evitar runaway tokens. |
| `generate_one_summary` (en `llm.rs`) | +10 | Short-circuit: si `build_tabular_summary` devuelve Some, retorna `SummaryOutcome::Generated(text)` directo (skip extract_text + LLM summarizer). |
| Tests `tabular_summary_*` | 7 tests | CSV simple, mime no-tabular, text/plain ambiguo, .csv extension fallback, oversized, cell truncation, only-header degenerate |

**Cobertura mimes:**

| Mime / extension | Fires auto-summary? |
|---|---|
| `text/csv` | ✅ |
| `application/csv` | ✅ |
| `application/vnd.openxmlformats-officedocument.spreadsheetml.sheet` (XLSX) | ✅ |
| `application/vnd.ms-excel` | ✅ |
| `text/plain` + filename `.csv` | ✅ (filename extension fallback) |
| `text/plain` + filename `.txt` | ❌ (ambiguo, no fire) |
| `application/pdf`, `image/*`, `text/markdown`, etc. | ❌ (path LLM-based existente) |

**Cuándo NO fire:**
- Bytes > 50 MB (mismo cap que `sql_inspect_attachment`)
- Mime no tabular ni filename .csv/.xlsx
- Parser de CSV/XLSX falla (e.g. archivo corrupto) → cae al path LLM existente

**Impacto LLM tokens:**

| Scenario | Pre-fix | Post-fix |
|---|---|---|
| Usuario sube CSV 1487 rows, agente sin SQL config | LLM ve solo `filename, mime, size` en catalog. Para conocer schema necesita `load_attachment` (~50K tokens). | LLM ve schema + sample + total en catalog automáticamente. **Zero LLM calls.** |
| Agente CON SQL config | `sql_inspect_attachment` cuesta 1 tool call (~300 tokens response) | Catalog ya tiene la info → no necesita ni siquiera `sql_inspect_attachment` salvo que quiera `target_table_schema` |
| Agente que invoca `summary_enabled: true` con CSV pre-fix | Cheap-tier LLM corre sobre raw CSV truncado → 1 LLM call extra | Zero LLM calls de summarization para CSV/XLSX |

**Impacto ADP.** Cero breaking changes. El campo
`conversation_attachments.description` ya existe — el cambio es solo qué
contenido se persiste para mimes tabulares. Agentes existentes ven mejor
contexto desde el primer turn. ADP worker recompila clean.

**Tests:** 7 nuevos en `sql_bulk_tools::tests::tabular_summary_*`.
Suite full: 1686 PASS / 0 FAIL / 92 IGNORED.

**Verificación live (E2E).** `tests/graphs/agents/sql_bulk_insert_e2e.json` ahora
tiene `summary_enabled: true`. Run 1 dispara la tarea concurrente de summary;
DB query confirma la persistencia con shape correcto.

**Estado.** done.

---

## 23. `attachment_run_python` — pandas on registered CSV/XLSX without dumping rows (2026-06-10)

**Origen.** Pregunta del owner post auto-summary (§22): "¿el LLM puede usar
pandas para responder preguntas específicas sobre el archivo sin gastar
muchos tokens viendo los CSV/Excel?". Hoy la única forma era
`load_attachment` (~50K tokens para un CSV de 1487 rows), o adivinar
desde el catalog auto-summary que muestra solo 3 sample rows.

**Fix shipped.** Nuevo synthetic tool `attachment_run_python(attachment_id, code, ...)`
que carga el attachment en un pandas DataFrame server-side, ejecuta el código
del LLM en el mismo sandbox restricted que usa `gsheets_run_python`, y
devuelve **solo stdout + result global**. La data del archivo nunca cruza
al contexto del LLM.

**Verificación live (LLM-in-the-loop)** — Gemini 2.5-flash con CSV de
100 productos:

```
User: "Cuál es el producto con el precio más alto, su precio, y cuál es
       la suma total de stock entre todos los productos?"

LLM:  → attachment_run_python({
          attachment_id: "products_csv",
          code: "df['price'] = pd.to_numeric(df['price'])
                 df['stock'] = pd.to_numeric(df['stock'])
                 top = df.loc[df['price'].idxmax()]
                 result = {'top_product': top['name'],
                           'max_price': float(top['price']),
                           'total_stock': int(df['stock'].sum())}"
       })
      ← {row_count: 100, columns: [...],
         result: {top_product: 'Product 100', max_price: 59.99, total_stock: 50500},
         duration_ms: 84}
      → "El producto con el precio más alto es Product 100 con un precio de
         59.99, y la suma total de stock es 50500"
```

Cálculo verificado: precio máx = `9.99 + 50 = 59.99` ✓; stock total =
`10 × Σ(1..100) = 50500` ✓.

**Token economy:**

| Approach | Tokens consumidos |
|---|---|
| `load_attachment` (todo el CSV de 100 rows) | ~5K tokens |
| `attachment_run_python` | ~80 tokens response |
| **Ratio** | **~60×** mejor (para CSV pequeños) |

Para CSVs grandes (1487 rows): ~50K vs ~80 = **~600× mejor**.

**Cambios:**

| Componente | LOC | Función |
|---|---|---|
| `llm_synthetic_tools/attachment_run_python.rs` (NEW) | ~280 | Tool definition + Args + Response + dispatcher + Python wrapper |
| `sql_bulk_tools::parse_attachment_to_records` | ~150 | Helper público que devuelve ALL rows (no solo sample) como Vec<JSON records>. Reusa la lógica CSV/XLSX existente |
| `dag_tool_executor.rs` | +12 | Match arm en `execute` para el tool name |
| `llm.rs` registration | +10 | Push del ToolDefinition cuando `attachment_run_python` está en `configured_aliases` |
| `text/tools/sql.yaml` | +35 | Entry con workflow + caps + sandbox restrictions + comparación con otros tools |
| `developer_guide/41_builtin_tools_index.md` | +1 row | Listado en el index doc (3 tools en SQL section) |

**Sandbox.** Reusa `execute_sandboxed_helper` (PyO3) en modo `restricted`,
idéntico a `gsheets_run_python` y `crdt_doc_run_python`:
- Allowed imports: `pandas`, `numpy`, `scipy.stats`, `math`, `datetime`,
  `decimal`, `json`, `re`, `statistics`, `string`, `collections`,
  `functools`, `itertools`
- Blocked imports: `os`, `sys`, `subprocess`, `socket`, `urllib`,
  `requests`, `importlib`, `builtins`, `ctypes`
- No filesystem access, no network access
- Bytes del attachment viven en memoria Python solo durante la call

**Wrapper Python.** El dispatcher carga el DataFrame antes del código del
LLM y serializa `result` después:

```python
import pandas as pd
import numpy as np
import scipy.stats as stats

df = pd.DataFrame(_attachment_records)   # records loaded by Rust
result = None

<user code>

# postlude: aliases result → output (helper extracts `output`)
def __col_serialise(v):
    if hasattr(v, 'to_dict'):
        return v.to_dict(orient='records')
    if hasattr(v, 'to_list'):
        return v.to_list()
    if isinstance(v, np.generic):
        return v.item()
    return v
output = __col_serialise(result)
```

Pandas DataFrames y Series se serializan estructuralmente; numpy scalars
se desbox-ean con `.item()`.

**Soporta inline + signed URL uniformemente.** El dispatcher usa
`DagToolExecutor::fetch_attachment_bytes` (Bulk T0), que va por
`OutputStorageRepository`. Ese adapter persiste bytes para AMBOS source
types al registrar el attachment. El LLM no sabe (ni le importa) de
dónde vino el archivo.

**Limits (v1, hardcoded):**

| Cap | Valor |
|---|---|
| Attachment size | 50 MB |
| DataFrame rows | 100 000 |
| Code wall-clock | 30 s |
| Stdout / error per response | 50 KB |

**Tests:**
- 5 unit tests en `attachment_run_python::tests` (args deserialize, wrapper,
  truncate)
- E2E LLM-in-the-loop en
  `tests/graphs/agents/attachment_run_python_e2e.json` — verificado contra
  Gemini 2.5-flash + CSV de 100 productos. PASS.
- Full suite: 1691 PASS / 0 FAIL / 92 IGNORED.

**Impacto ADP.** **Cero breaking changes.** Tool opt-in vía
`tool_configurations`. El worker image YA tiene pandas/numpy/scipy
instalado (resuelto en commit `ee08598e` 2026-06-07). ADP recompila clean.

**Para correr local con PyO3:**

```bash
PYTHONPATH=".venv/lib/python3.14/site-packages" \
  cargo run --bin dag_engine -- run <graph.json>
```

(El binario lockea contra Python 3.14 del sistema en macOS; pandas vive
en `.venv` solo. En Cloud Run el binary usa el Python del Dockerfile.)

**Comparación con tools relacionados** (matriz completa en
[`docs/developer_guide/23_sql_node.md` §"Elegir la herramienta correcta para un attachment"](developer_guide/23_sql_node.md)):

| Tool | Cuándo usar |
|---|---|
| Catalog auto-summary (§22) | "Qué columnas tiene este archivo?" — gratis, 0 calls (CSV/XLSX/PDF/text) |
| `attachment_run_python` | Cálculos analíticos sobre CSV/XLSX: max, mean, filter, group_by, etc. |
| `sql_inspect_attachment` (item 13) | Pre-bulk: conocer el schema del table destino antes de COPY |
| `sql_bulk_insert_from_attachment` (item 13) | Cargar el CSV a Postgres |
| [`load_attachment`](developer_guide/31_load_attachment.md) | Reader general-purpose para CUALQUIER mime. Es la herramienta correcta cuando el LLM necesita ver el contenido literal (PDF, imagen, markdown, código, o filas verbatim de un CSV). NO es fallback — es la primaria para non-tabular. |

`load_attachment` **no fue deprecado**. Item 13 + auto-summary +
`attachment_run_python` agregaron paths más eficientes solo para el
caso tabular. Para todo lo demás (PDFs, imágenes, código, markdown,
texto plain), o cuando el LLM genuinamente necesita ver el contenido
literal de un CSV, `load_attachment` sigue siendo la herramienta
primaria.

**Estado.** done.

---

## 24. gdocs `apply_edits` — ConfirmManyMatches threshold guard (2026-06-10)

**Qué cambió.** `apply_edits` ahora aborta el compound entero con
`DocsError::ConfirmManyMatches` cuando una sub-edit individual
`ReplaceText` o `DeleteText` resuelve a 5 o más párrafos (igual
threshold que el standalone `replace_text`). El guard corre durante la
fase A (resolve), antes de cualquier batchUpdate request — el documento
queda intacto.

**Por qué importa.** Antes del fix: el LLM podía mandar `find: "Enfriamiento: ..."`
y el compound reescribía silenciosamente 4 párrafos cuando el LLM creía
estar tocando 1 (ver bug en `agent_session cmq7kem1h003001s6mr36uwe8`).
Ahora: el guard fuerza al LLM a reconsiderar — su único camino de
recovery es narrow-down vía `scope.paragraph_range` o usar un find
string más específico. **Deliberadamente NO se expone `confirm_many`
ni `occurrence` como bypass** — la disciplina correcta es disambiguar
por scope, no waivar el guard.

**Implementación.**
- Nueva constante pública `APPLY_EDITS_MANY_HITS_THRESHOLD: usize = 5`
  documentada con rationale + sync con standalone.
- Helper `enforce_many_hits_threshold(&hits, &find, snap)` corre justo
  después de `find_hits` en ambos brazos (`ReplaceText`, `DeleteText`).
- Helpers `build_previews` + `take_slice_around` copiados desde
  `replace_text.rs` para producir el MISMO shape de `MatchPreview` que
  el standalone — el LLM ve el contrato idéntico sin importar qué tool
  usó.

**Tests de regresión** (en `apply_edits.rs:app_tests`):
- `apply_edits_replace_with_5_hits_triggers_confirm_many_matches`
  — 5 párrafos → `ConfirmManyMatches { find, count: 5, preview[5] }`.
- `apply_edits_replace_with_4_hits_proceeds` — boundary: 4 hits
  pasan sin error (pin del threshold en 5).
- `apply_edits_threshold_bypassed_by_scope_narrowing` — 5 párrafos
  contienen el find pero `Scope::Paragraph { n: 1 }` lo reduce a 1
  → call procede.
- `apply_edits_delete_with_5_hits_triggers_confirm_many_matches`
  — DeleteText obedece el mismo threshold.

**BREAKING.** Comportamiento, no API: compounds que dependían del
replace-all silencioso de ≥5 hits ahora fallan con
`ConfirmManyMatches`. Ningún grafo conocido se beneficiaba del bug;
todos los usos legítimos quedan bajo 5 hits o ya usan `scope`.

**Verificación.**
- 11 tests de `apply_edits` pasan (7 previos + 4 nuevos).
- Suite full: 1627 tests pasan, 0 fallos.

**Estado.** done. Pareja conceptual con item §25 (skill auto-loaded
para scope-discipline) que enseña al LLM a no llegar al guard en
primer lugar.

---

## 25. gdocs-surgical-edits builtin skill — auto-enrolado para agentes con tools de edición (2026-06-10)

**Qué cambió.** Nuevo skill `gdocs-surgical-edits` (en
[`src/libs/colmena/skills/gdocs-surgical-edits/`](../src/libs/colmena/skills/gdocs-surgical-edits/))
con SKILL.md + 5 references on-demand. Auto-enrolado por
`LlmNode::build_skill_repository_from_config` cuando el agente expone
al menos un tool de edición gdocs. El LLM lo descubre vía el catálogo
del tool `load_skill` y lo carga a demanda.

**Por qué importa.** Tema A (ConfirmManyMatches en apply_edits) puso un
freno backend que aborta cuando una sub-edit matchea ≥5 párrafos. Tema
B (este) enseña al LLM a NO llegar al freno: usar `scope`/`anchor`
desde el primer call, entender qué hace cada error estructurado, no
inyectar markdown literal como texto. Defensa en capas: rails + educación.

**Contenido del skill.**
- **SKILL.md** (~700 tokens) con 6 quick rules siempre presentes
  + anti-patterns + fallback path "si tu intent no está cubierto".
- **`references/replace_text_scoping.md`** — Las 4 herramientas de
  scope (`Paragraph`, `Tab`, `UnderHeading`, `BetweenHeadings`) +
  `anchor` + `occurrence`. Tabla de decisión "qué usar cuándo".
- **`references/apply_edits_patterns.md`** — Cuándo SÍ y NO usar
  compound vs standalone, anatomía del flujo interno (Phase A
  resolve / Phase B sort+emit), por qué el sort cross-edit importa.
- **`references/error_recovery.md`** — Decoder de
  `ConfirmManyMatches`, `AmbiguousMatch`, `TextNotFound`,
  overlapping-ranges. Patrón de recovery canónico para cada uno.
- **`references/style_changes_pattern.md`** — Receta canónica para
  "agregale formato a esta sección": 3 pasos (read_outline →
  read_as_markdown → replace_section con markdown). Lección clave:
  NUNCA `replace_text` con sintaxis markdown literal.
- **`references/before_after_examples.md`** — 4 casos worked, incluyendo
  el bug del usuario (`agent_session cmq7kem1h003001s6mr36uwe8`) con
  wrong way vs right way side by side.

**Implementación.**
- Skill files compilan en el binario vía `include_dir!` (mismo
  pipeline que sql-query-best-practices, etc.).
- Constante `LlmNode::GDOCS_SURGICAL_EDIT_TOOL_NAMES` lista los 11
  nombres de tools de edición (excluye los read-only).
- Helper `LlmNode::agent_has_gdocs_edit_tools(config, inputs)` detecta
  enrollment:
  - `enabled_tools` contains `"*"` → true.
  - `enabled_tools` contains alias `"gdocs"` → true.
  - `enabled_tools` contains any edit tool name → true.
  - `tool_configurations.<edit_tool>` declared → true.
  - `enabled_tools: ["gdocsread"]` (read-only alias) → false.
  - Exclusion entries `"!gdocs_apply_edits"` → ignored, no false trigger.
- `build_skill_repository_from_config` auto-inserta
  `gdocs-surgical-edits` en `skills_config.builtin` cuando el helper
  retorna true Y el operador no lo agregó manualmente (idempotente).

**Tests** (en `dag_engine::infrastructure::nodes::llm`):
- 10 unit tests de `agent_has_gdocs_edit_tools_tests` cubriendo cada
  caso de la matriz de decisión (alias, exclusiones, tool_configurations,
  precedencia inputs vs config).
- 2 integration tests del wiring end-to-end:
  `build_skill_repository_auto_enrolls_gdocs_surgical_edits`
  (positive: gdocs alias → skill aparece en
  `repo.list_available()`) y
  `build_skill_repository_does_not_enroll_for_read_only_agents`
  (negative: gdocsread sin opt-in → repo None).
- Test `skills::infrastructure::builtin_skill_repository::tests::gdocs_surgical_edits_is_loadable`
  confirma frontmatter parsea + las 5 references existen.
- Suite full: 1640 tests pass, 0 fallos.

**BREAKING.** Ninguno. Skill puramente additive. Agentes existentes con
`gdocs_*` edit tools van a tener el skill en su catálogo `load_skill`
pero no están obligados a cargarlo. Operadores que ya tenían
`gdocs-surgical-edits` en su config no se duplican (idempotente).

**Entrada en `docs/developer_guide/42_builtin_skills_index.md`** actualizada
con la línea del nuevo skill.

**Estado.** done. Pareja conceptual con item §24 (ConfirmManyMatches guard
backend). Juntos cubren backend rails + LLM education.

---

## 26. Google OAuth user-scoped auth (hard cutover) — shipped 2026-06-10

**Qué cambió.** Reemplazo completo del path Service Account de auth en
`gsheets` y `gdocs` por **OAuth user-scoped** sobre un user dedicado
de Workspace (`agents@startti.co` en el deploy canónico). El refresh_token
se obtiene una sola vez por el operador via `colmena_oauth_setup` y se
guarda en Google Secret Manager. Cada API call del worker hace un
refresh contra `oauth2.googleapis.com` con cache de 1h.

**Por qué importa.**
- **Identity leak eliminada**: el activity log de cada doc ahora muestra
  `agents@startti.co` (humano, dominio Startti) en vez del email feo de
  la SA que revelaba el GCP project ID (`startti-dev`), el tooling
  usado (Colmena), y el env tier.
- **Identity continuity**: rotación del refresh_token no rompe docs ya
  compartidos. Comparado al SA donde rotación implicaba re-share por
  cada doc.
- **Setup mínimo viable**: 1 cuenta Workspace, 1 GCP OAuth client, 1
  consent flow, 3 secrets en Secret Manager. ~3 hs operacional + 0
  cambios de código del usuario después del deploy.

**Implementación.**

- **Nuevo módulo `src/libs/colmena/src/google_oauth/`** (hexagonal):
  - `domain/types.rs` — `AccessToken`, `RefreshTokenSecret` (debug
    redactado), `CachedToken`.
  - `domain/errors.rs` — `OAuthError` (RefreshTokenRevoked,
    ClientCredsInvalid, Transient con retry annotation, ConfigMissing
    con lista de TODAS las vars faltantes).
  - `domain/traits.rs` — `AuthTokenProvider` async trait.
  - `infrastructure/config.rs` — `OAuthCredentials::from_env()` con
    tres env vars + tratamiento de empty/whitespace como missing.
  - `infrastructure/refresh_client.rs` — POST a
    `oauth2.googleapis.com/token`, mapeo de errores Google → variantes
    OAuthError, retry con backoff (1s, 2s, 2 attempts) en 5xx.
  - `infrastructure/token_provider.rs` — `OAuthRefreshTokenProvider`
    con `tokio::sync::Mutex<Option<CachedToken>>`. 60s margin, mutex
    coalescing de concurrent refreshes, WARN log + no-persist en
    rotated refresh_token.
- **Refactor de `gsheets/infrastructure/auth.rs` + `gdocs/infrastructure/auth.rs`**:
  enum `Inner { OAuth, Static }` donde OAuth envuelve un
  `Arc<OAuthRefreshTokenProvider>` y Static es el path de test sticky.
  Borre 100% del path yup-oauth2 ADC.
- **Refactor de `gsheets/infrastructure/config.rs` + `gdocs/infrastructure/config.rs`**:
  borré `credentials_path` (era el SA JSON), agregué `share_email`
  desde `COLMENA_GOOGLE_SHARE_EMAIL`.
- **Refactor de `gsheets/infrastructure/http_client.rs` + `gdocs/infrastructure/http_client.rs`**:
  borré la extracción del `client_email` JSON (~10 líneas), renombré
  `sa_email` → `share_email`. `from_config` ahora llama
  `OAuthCredentials::from_env()` y mapea ConfigMissing → NotConfigured.
- **`google_workspace_prelude.rs`**: nueva chain de resolución
  `COLMENA_GOOGLE_SHARE_EMAIL` → `COLMENA_GOOGLE_SA_EMAIL` (legacy
  para tests) → SA JSON (legacy) → None. `resolve_sa_email` queda
  como deprecated alias.
- **Nuevo binary `src/bin/colmena_oauth_setup.rs`** (~330 líneas):
  CLI con clap + axum localhost server + webbrowser open. Parsea
  client_secret.json (acepta `installed` y `web` shapes), abre browser
  a consent URL con `access_type=offline + prompt=consent` (manda
  refresh_token garantizado), capta callback en localhost:8080,
  exchange code → refresh_token, lo imprime con instrucciones de
  Secret Manager + history clear.
- **Dependencia nueva**: `webbrowser = "1"` (~30 KB, solo para el
  binary target, no la lib).

**Tests.**
- **27 tests nuevos en `google_oauth`** cubriendo cada path:
  - 8 domain tests (newtype redaction, error variants, partial_eq).
  - 5 config tests (presencia/ausencia/empty/whitespace/trim).
  - 7 refresh_client wiremock tests (happy, rotation, invalid_grant,
    invalid_client, retry-success, retry-exhausted, no-retry on 4xx).
  - 6 token_provider tests (first-call refreshes, cache hit,
    near-expiry refresh, concurrent coalescing, rotation handling,
    failed-refresh leaves cache empty).
- **3 tests nuevos en prelude** cubriendo precedencia de la chain:
  share_email gana sobre todo, fallback al legacy SA var, empty
  share_email lo ignora y usa fallback.
- **4 tests del CLI binary** (auth URL shape, client_secret parsing
  ambas variantes, error on missing block).
- **Wiremock tests existentes de gsheets + gdocs siguen pasando**
  (48 + 29) — el path test usa `for_tests_static()` que bypassa OAuth.
- **Suite total: 1673 tests pasan, 0 fallos.** `cargo clippy --lib`
  limpio.

**BREAKING para deploys.**
- `GOOGLE_APPLICATION_CREDENTIALS` ya no se lee en producción.
- `COLMENA_GOOGLE_SA_EMAIL` deprecated (fallback solo).
- **ADP deploy_gcp.sh debe actualizarse** antes del próximo deploy
  contra colmena develop o el worker boot-paniquea con `ConfigMissing`
  listando exactamente qué env vars faltan. Ver
  [`docs/developer_guide/47_google_oauth.md`](../developer_guide/47_google_oauth.md)
  paso F.

**BREAKING para usuarios de docs ya compartidos.** Como discutimos con
el operador, hard cutover: docs que estaban compartidos con la SA
vieja dejan de funcionar; el usuario debe re-compartir con
`agents@startti.co` cuando el agent diga "PermissionDenied — pedile al
user que comparta con agents@startti.co". El prelude ya orquesta esto
naturalmente.

**Configuración operacional requerida en ADP.**
- Crear 3 secrets en Secret Manager: `colmena-oauth-client-id`,
  `colmena-oauth-client-secret`, `colmena-oauth-refresh-token`.
- IAM binding `secretAccessor` al worker SA en los 3.
- `deploy_gcp.sh`: `--update-secrets=COLMENA_GOOGLE_OAUTH_*=...:latest`
  + `--update-env-vars=COLMENA_GOOGLE_SHARE_EMAIL=agents@startti.co`
  + `--remove-secrets=GOOGLE_APPLICATION_CREDENTIALS`
  + `--remove-env-vars=COLMENA_GOOGLE_SA_EMAIL`.

**Estado.** done en colmena develop. ADP pending (T47-T50 del plan).

**Spec + plan.**
- Design: [`docs/superpowers/specs/2026-06-10-oauth-user-scoped-design.md`](../superpowers/specs/2026-06-10-oauth-user-scoped-design.md)
- Plan: [`docs/superpowers/plans/2026-06-10-oauth-user-scoped.md`](../superpowers/plans/2026-06-10-oauth-user-scoped.md)
- Guía operacional: [`docs/developer_guide/47_google_oauth.md`](../developer_guide/47_google_oauth.md)

---

## 24. Bundle 1 — attachment plumbing dependents (G items 4 + 5, E-T7b) shipped 2026-06-11

**Origen.** Post-Bulk T0 (shared attachment plumbing, commit `479c321`), tres
features que estaban con placeholders `not_yet_wired` quedaron desbloqueadas
para wiring:

| Feature | Antes |
|---|---|
| `gdocs_create_from_docx` | dispatcher devolvía `{error: "not_yet_wired"}` |
| `gdocs_export` | devolvía `{format, byte_len}` sin envolver bytes como attachment |
| `gsheets_create_from_xlsx` + `gsheets_export_xlsx` | NO existían en el router — los tool definitions estaban publicados pero los dispatchers no se llamaban |

**Hallazgo clave.** Los métodos HTTP de Drive Files API
(`DocsClient::create_from_docx`, `SheetsClient::create_from_xlsx`,
`SheetsClient::export_xlsx`) **ya estaban implementados** en
`http_client.rs`. El bloqueo era puramente la falta del cable
`fetch_attachment_bytes` / `register_attachment_bytes` en
`DagToolExecutor`. Bulk T0 (commit `479c321`) provee ese cable. Bundle 1
solo añadió **variantes `_via_executor`** que invocan el cable y luego
delegan al método HTTP existente.

**Cambios:**

| Componente | LOC | Función |
|---|---|---|
| `gdocs_tools::dispatch_export_via_executor` | ~60 | Export bytes → `register_attachment_bytes` → response con `attachment_id` |
| `gdocs_tools::dispatch_create_from_docx_via_executor` | ~50 | `fetch_attachment_bytes` → `DocsClient::create_from_docx` → response con `doc_id` |
| `gsheets_tools::dispatch_create_from_xlsx_via_executor` | ~40 | Idem patron para xlsx → Sheets |
| `gsheets_tools::dispatch_export_xlsx_via_executor` | ~40 | Idem patron para Sheets → xlsx con `register_attachment_bytes` |
| `dag_tool_executor.rs` router | +15 | Match arms que llaman las variantes _via_executor |

**Wire-format final** (todas las respuestas vienen como JSON):

```jsonc
// gdocs_create_from_docx
{
  "ok": true,
  "doc_id": "1abc...",
  "url": "https://docs.google.com/document/d/1abc...",
  "title": "Mi doc",
  "revision_id": "rev123",
  "tabs": [...]
}

// gdocs_export
{
  "ok": true,
  "format": "pdf",
  "byte_len": 45000,
  "attachment_id": "<storage_key>",
  "mime_type": "application/pdf",
  "filename": "1abc....pdf"
}

// gsheets_create_from_xlsx
{
  "ok": true,
  "spreadsheet_id": "1xyz...",
  "title": "Mi planilla",
  "url": "https://docs.google.com/spreadsheets/d/1xyz...",
  "sheets": [...]
}

// gsheets_export_xlsx
{
  "ok": true,
  "attachment_id": "<storage_key>",
  "byte_len": 12000,
  "mime_type": "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
  "filename": "1xyz....xlsx",
  "spreadsheet_id": "1xyz..."
}
```

**Soporta inline AND signed URL uniformemente.** El cable de Bulk T0 va
por `OutputStorageRepository`, que abstrae el source del attachment. Un
usuario que sube un .docx inline (`data:` base64) y otro que sube un
.docx vía URL firmada (`files[].url`) ambos terminan en el mismo path
de bytes.

**G item 8 (`gdocs_insert_image_after_text`) DIFERIDO.** Originalmente
Bundle 1 lo incluía. Pero a diferencia de los 3 features de arriba, este
requiere construir desde cero: nuevo tipo `InsertInlineImageRequest`,
nuevo método HTTP, resolución de anchor (no existe), decisión de diseño
sobre Drive image permissions. Bundle 1 ships con 3/4 features
shipped; G item 8 vuelve al backlog con notas de scope.

**Tests.** Suite full: **1688 PASS / 0 FAIL / 40 IGNORED**. Sin tests
nuevos en este commit — los dispatchers _via_executor son thin wrappers
de métodos HTTP ya cubiertos por wiremock tests existentes en
`http_client.rs::tests`.

**Impacto ADP.** **Cero breaking changes.** Los tool names son los
mismos; los wire formats nuevos solo añaden campos
(`attachment_id`/`mime_type`/`filename` para export, `doc_id` real en
vez de `not_yet_wired` envelope para create). ADP recompila clean.

**Estado.** done.

### Cómo correr Bundle 1 manualmente

Dos E2E LLM-in-the-loop graphs cubren el round-trip end-to-end con gemini-2.5-flash:

```bash
# Configurar OAuth user-scoped (única vez):
# Ver docs/developer_guide/47_google_oauth.md
set -a && source .env && set +a

# Round-trip gdocs (G items 4 + 5): create → export docx → create_from_docx → read
cargo run --bin dag_engine -- run tests/graphs/agents/gdocs_bundle1_e2e.json \
  --agent-session-id g_bundle1_$(date +%s) --include-extra-info

# Round-trip gsheets (E-T7b): create → set_range → export_xlsx → create_from_xlsx → read
cargo run --bin dag_engine -- run tests/graphs/agents/gsheets_etb7_e2e.json \
  --agent-session-id e_t7b_$(date +%s) --include-extra-info
```

Cada graph crea recursos nuevos en Drive (no requiere fixtures); el agente
reporta PASS/FAIL por step en su respuesta final.

---

## 25. Bundle 2A — Drive discovery (gdocs_list_documents + gsheets_list_spreadsheets) shipped 2026-06-11

**Origen.** Post-OAuth migration (commit `c5ad3c6`, 2026-06-10), discovery
finalmente tiene sentido real: `agents@startti.co` tiene Drive útil con
docs/spreadsheets compartidos (la SA vieja tenía Drive vacío y quota 0).

**Fix shipped.** Dos tools nuevos para Drive discovery:

| Tool | Endpoint | Devuelve |
|---|---|---|
| `gdocs_list_documents` | Drive `files.list?q=mimeType='application/vnd.google-apps.document'` | `Vec<DocumentListItem>` + `next_page_token` |
| `gsheets_list_spreadsheets` | Drive `files.list?q=mimeType='application/vnd.google-apps.spreadsheet'` | `Vec<SpreadsheetListItem>` + `next_page_token` |

**Args (idéntico shape para ambos):**

| Field | Tipo | Descripción |
|---|---|---|
| `query` | string? | Substring match en `name` (Drive `name contains`) |
| `parent_folder_id` | string? | Limitar a folder específico (sin recursion) |
| `modified_after` | string? | RFC 3339 timestamp lower bound (`modifiedTime >= ...`) |
| `limit` | u32? | Page size, default 20, max 100 |
| `page_token` | string? | Pagination cursor |

**Response shape:**

```jsonc
{
  "ok": true,
  "documents": [   // o "spreadsheets"
    {
      "doc_id": "1abc...",
      "name": "Plan Q3 2026",
      "url": "https://docs.google.com/document/d/1abc...",
      "modified_time": "2026-06-05T14:23:00Z",
      "owners": ["humano@cliente.com"]
    }
  ],
  "next_page_token": "..."  // Some(...) cuando hay más
}
```

**Sin breaking changes.** Solo wire + nuevos métodos. Tools opt-in vía
`enabled_tools` / `tool_configurations`. ADP worker recompila clean.

**Cambios:**

| Componente | LOC | Función |
|---|---|---|
| `gdocs::domain::types::DocumentListItem/Result/Filter` | ~50 | Types públicos |
| `DocsClient::list_documents` (trait + HTTP impl) | ~110 | Drive `files.list?q=...&pageSize=...&fields=...&orderBy=...` con quoting safe |
| `gsheets::domain::types::SpreadsheetListItem/Result/Filter` | ~50 | Types simétricos |
| `SheetsClient::list_spreadsheets` (trait + HTTP impl) | ~100 | Idem patrón |
| `gdocs_tools::ListDocumentsArgs + tool_list_documents + dispatch_list_documents` | ~60 | Builder + dispatcher |
| `gsheets_tools::ListSpreadsheetsArgs + tool_list_spreadsheets + dispatch_list_spreadsheets[_with_client]` | ~70 | Idem |
| `dag_tool_executor.rs` router | +20 | 2 match arms |
| `mod.rs` re-exports + `all_synthetic_tools()` | +6 | Coverage |
| `text/tools/gsheets.yaml`, `gdocs.yaml` | +80 | YAML entries |
| `41_builtin_tools_index.md` | +2 | Index rows + counts |

**Tests.** Suite full: **1760 PASS / 0 FAIL / 92 IGNORED**. `build_all_returns_23_tools`
ajustado (era 22). Coverage test `index_doc_covers_all_registered_tools` PASS.

**Estado.** done.

**E2E LLM-in-the-loop:** `tests/graphs/agents/gworkspace_bundle2a_e2e.json`
— el agente crea un doc + spreadsheet con un token único en el título,
luego los descubre vía `gdocs_list_documents({query: TOK})` y
`gsheets_list_spreadsheets({query: TOK})`. Run:
`set -a && source .env && set +a; cargo run --bin dag_engine -- run tests/graphs/agents/gworkspace_bundle2a_e2e.json --agent-session-id g_b2a_$(date +%s) --include-extra-info`.

**Próximo (Bundle 2B, deferred):** 5 tools de permissions —
`drive_list_permissions`, `gsheets_share`, `gdocs_unshare`,
`gsheets_unshare`, `gsheets_list_permissions` /
`gdocs_list_permissions`. Comparten endpoint `drive.permissions.*`.

---

## 26. Bundle 2B — Drive permissions (5 tools) shipped 2026-06-11

**Origen.** Completar el set Drive UX iniciado en Bundle 2A (discovery). El
LLM ahora puede no solo encontrar un doc/sheet sino también gestionar quién
tiene acceso: listar permisos, compartir con email, revocar.

**Fix shipped.** 5 tools nuevos:

| Tool | Endpoint Drive | Función |
|---|---|---|
| `gdocs_list_permissions` | `permissions.list` | "quién tiene acceso al doc" |
| `gdocs_unshare` | `permissions.delete` | Revocar acceso al doc |
| `gsheets_share` | `permissions.create` | Compartir spreadsheet con email |
| `gsheets_list_permissions` | `permissions.list` | "quién tiene acceso al sheet" |
| `gsheets_unshare` | `permissions.delete` | Revocar acceso al sheet |

`gdocs_share` ya existía en v1; ahora gsheets tiene su simétrico.

**Args (uniformes):**

| Tool | Args |
|---|---|
| `gdocs_list_permissions` | `{doc_id}` |
| `gdocs_unshare` | `{doc_id, permission_id}` |
| `gsheets_share` | `{spreadsheet_id, email, role}` con role ∈ `reader/commenter/writer` |
| `gsheets_list_permissions` | `{spreadsheet_id}` |
| `gsheets_unshare` | `{spreadsheet_id, permission_id}` |

**Response shape (lista):**
```jsonc
{
  "ok": true,
  "permissions": [
    {
      "permission_id": "perm123",
      "type": "user",
      "role": "writer",
      "email": "daniel@cliente.com",
      "display_name": "Daniel García"
    }
  ]
}
```

**Workflow típico para revoke:**

```
User: "sacale acceso a daniel@cliente.com del plan Q3"
LLM: → gdocs_list_documents({query: "Plan Q3"})    ← Bundle 2A
       ← {documents: [{doc_id: "1abc"}]}
     → gdocs_list_permissions({doc_id: "1abc"})    ← Bundle 2B
       ← {permissions: [{permission_id: "perm789", email: "daniel@cliente.com"}]}
     → gdocs_unshare({doc_id: "1abc", permission_id: "perm789"})  ← Bundle 2B
       ← {ok: true}
```

**Decisión importante.** `permission_id` (NO el email) es el id estable
de Drive. Un email puede tener múltiples grants (e.g. reader y writer
via diferentes grants); cada uno tiene su propio `permission_id`. Esto
está documentado verbatim en cada YAML de unshare para que el LLM no
caiga en el patrón "unshare by email".

**Cambios:**

| Componente | LOC | Función |
|---|---|---|
| `gdocs::domain::types::PermissionEntry/PermissionList` | ~30 | Types compartidos |
| `DocsClient::list_permissions + delete_permission` (trait + HTTP) | ~110 | Drive endpoints |
| `gsheets::domain::types::ShareRole + PermissionEntry/List` | ~50 | Types simétricos |
| `SheetsClient::share + list_permissions + delete_permission` (trait + HTTP) | ~150 | Drive endpoints + custom delete loop (no helper) |
| 5 dispatchers + Args structs + tool defs | ~300 | gdocs_tools + gsheets_tools |
| Mock impl en `FakeClient` (gsheets_tools tests) | +20 | Trait completeness |
| `dag_tool_executor.rs` router (gdocs + gsheets) | +50 | 5 match arms + matchers |
| `mod.rs` re-exports + `all_synthetic_tools()` | +12 | Coverage |
| `text/tools/gdocs.yaml` + `gsheets.yaml` | +180 | 5 YAML entries con workflows |
| `41_builtin_tools_index.md` | +5 | Index rows + counts (gsheets 11→14, gdocs 23→25) |

**Tests.** Full suite: **1760 PASS / 0 FAIL / 92 IGNORED**.
`build_all_returns_25_tools` ajustado (era 23). Coverage tests PASS.
Clippy + fmt clean. ADP worker recompila clean.

**Estado.** done.

**E2E LLM-in-the-loop:** `tests/graphs/agents/gworkspace_bundle2b_e2e.json`
— el agente crea doc + sheet, comparte ambos con
`SHARE_EMAIL=daniel@startti.co` (ajustar a tu email), verifica con
`list_permissions`, revoca con `unshare`, y vuelve a listar para
confirmar la baja. Cubre los 4 nuevos tools (`gdocs_list_permissions`,
`gdocs_unshare`, `gsheets_share`, `gsheets_list_permissions`,
`gsheets_unshare`) + reuso de `gdocs_share` v1. Run:
`set -a && source .env && set +a; cargo run --bin dag_engine -- run tests/graphs/agents/gworkspace_bundle2b_e2e.json --agent-session-id g_b2b_$(date +%s) --include-extra-info`.

**Bundle 2 (A+B) cerrado** — Drive discovery + sharing/permissions
totalmente cubiertos. Próximos bundles del backlog:

- Bundle 3 — markdown content quick wins (~1d)
- Bundle 4 — Comments + Apps Script (~3d)
- Bundle 5-8 — table cells / formatting / suggest / webhooks

---

## 27. Bundle 3 — `add_tab` markdown seeding (G item 3) shipped 2026-06-11

**Origen.** Backlog G v1.1 item 3: hoy `gdocs_add_tab` con el arg `markdown`
acepta el contenido pero devuelve `pending_markdown_seed: true` y el tab
queda vacío. El LLM tenía que hacer dos calls (`add_tab` + `append_markdown`)
para crear un tab con contenido.

**Fix shipped.** `dispatch_add_tab` ahora invoca
`replace_section::run_append_markdown` con el `tab_id` recién creado cuando
el caller proveyó `markdown` no-vacío. Reusa toda la infra existente del
converter de markdown + batchUpdate + co-edit guard.

**Antes:**
```jsonc
// LLM call
{"name": "gdocs_add_tab", "arguments": {
  "doc_id": "1abc...",
  "title": "Nueva sección",
  "markdown": "# Intro\n\nContenido inicial"
}}

// Response
{
  "ok": true,
  "tab_id": "t.xyz",
  "title": "Nueva sección",
  ...
  "pending_markdown_seed": true   // ← placeholder, contenido NO landeó
}
```

**Después:**
```jsonc
// Mismo LLM call

// Response
{
  "ok": true,
  "tab_id": "t.xyz",
  "title": "Nueva sección",
  ...
  "markdown_seeded": true   // ← contenido sí landeó
}
```

**Failure-mode handling.** Si la creación del tab tiene éxito pero el seed
falla post-creación, el dispatcher surface una response de éxito parcial
para que el LLM no piense que el tab no se creó:

```jsonc
{
  "ok": true,
  "tab_id": "t.xyz",
  ...
  "markdown_seeded": false,
  "markdown_seed_error": {"error": ...}   // el error envelope del seed
}
```

El LLM puede entonces re-intentar con `gdocs_append_markdown({tab_id: "t.xyz", ...})`.

**Cambios:**

| Componente | LOC | Función |
|---|---|---|
| `dispatch_add_tab` | +40 | Llama a `run_append_markdown` post-creación cuando hay markdown; mantiene short-circuit para markdown vacío + failure-mode handling |

**No breaking changes.** Agentes que llamaban `add_tab` SIN markdown no
notan diferencia. Agentes que pasaban markdown obtienen el shape esperado
(con `markdown_seeded` en vez del `pending_markdown_seed` placeholder).

**Tests.** Full suite: **1760 PASS / 0 FAIL / 92 IGNORED**. Sin tests
nuevos en este commit — el dispatcher es wiring de primitives ya cubiertas
por sus tests propios (replace_section + co-edit guard).

**E2E LLM-in-the-loop:** `tests/graphs/agents/gdocs_bundle3_e2e.json` —
el agente crea un doc, llama `add_tab` con `markdown` no vacío,
verifica que el response trae `markdown_seeded: true`, luego lee el
tab como markdown para confirmar que el heading + items quedaron
escritos en una sola call. Run:
`set -a && source .env && set +a; cargo run --bin dag_engine -- run tests/graphs/agents/gdocs_bundle3_e2e.json --agent-session-id g_b3_$(date +%s) --include-extra-info`.

**Markdown tables NO incluidas en este bundle.** Originalmente Bundle 3
planeaba incluir support para tablas en `insert/replace`. Tras
investigación quedó claro que NO califica como quick win — el converter
`markdown_to_docs_ops` ya emite `insertTable` + `insertText` per cell,
pero el cursor math post-tabla es heurística. El fix correcto requiere
pipeline de 2 batchUpdates con snapshot intermedio para resolver índices
reales de celda. Esfuerzo: ~4-5h. Queda al backlog con scope clarificado.

**Estado.** done.


---

## 28. Bundle 4A — Drive Comments (3 tools) shipped 2026-06-11

**Origen.** Bundle 4 del BACKLOG (G v1.1 § "Drive Comments API — mensajería
humano ↔ agente in-doc"). Cierra el flujo bidireccional dentro del doc
sin tocar el contenido: el agente puede flagear preguntas, decisiones, o
blockers; el humano resuelve desde la UI; el agente puede listar para ver
respuestas.

Bundle 4 originalmente iba a incluir también **Apps Script** (`scripts.run`),
pero ese sub-bundle requiere agregar el scope `script.scripts.execute` a
`REQUESTED_SCOPES` en `colmena_oauth_setup.rs` + re-correr el consent flow +
regenerar el `refresh_token` en Secret Manager + redeploy ADP. Es un cambio
prod-impacting que necesita coordinación operacional — diferido como
**Bundle 4B** hasta confirmar la ventana del re-consent.

**Fix shipped.** 3 tools nuevos sobre `drive.comments.*`:

| Tool | Endpoint Drive | Función |
|---|---|---|
| `gdocs_add_comment` | `comments.create` | Postea un nuevo comment (doc-wide o pinned). |
| `gdocs_list_comments` | `comments.list` | Lista comments (open por default, `include_resolved` opcional). |
| `gdocs_resolve_comment` | `comments.replies.create` + `action: "resolve"` | Cierra un comment posteando una reply con la acción de resolve (no hay PATCH directo a `resolved`). |

Scope OAuth: usa el `drive.file` que ya está activo — **sin cambios en
producción**.

**Cómo se ve para el LLM:**

```
LLM call:  gdocs_add_comment({
  doc_id: "1abc",
  content: "@reviewer — should this cite the 2025 study or the 2026 update?"
})

Result:    {
  ok: true,
  comment: {
    comment_id: "AAA001",
    content: "@reviewer — ...",
    created_time: "2026-06-11T17:23:45.123Z",
    resolved: false,
    anchor: null,
    author_display_name: "Agents Startti",
    author_email: "agents@startti.co"
  }
}
```

**Workflow típico — humano deja TODO, agente resuelve:**

```
Humano: comment "Add stats on engagement"
↓
Agente (turn N):   gdocs_list_comments({doc_id}) → ve el TODO, captura comment_id
Agente (turn N):   gdocs_apply_edits(...) → agrega los stats
Agente (turn N+1): gdocs_resolve_comment({doc_id, comment_id, content: "Added in §3"})
↓
Humano: ve el thread cerrado con la nota del agente
```

**Workflow inverso — agente pregunta antes de editar:**

```
Agente: gdocs_add_comment({doc_id, content: "..."})
Humano: responde / resuelve en la UI
Agente: gdocs_list_comments({include_resolved: true}) → ve la respuesta
```

**Cambios:**

| Archivo | LOC | Qué cambia |
|---|---|---|
| `gdocs/domain/types.rs` | +50 | `CommentEntry`, `CommentList`, `CommentListFilter<'a>` |
| `gdocs/domain/traits.rs` | +28 | 3 trait methods (`add_comment`, `list_comments`, `resolve_comment`) |
| `gdocs/infrastructure/http_client.rs` | +160 | 3 HTTP impls + `parse_comment` helper |
| `llm_synthetic_tools/gdocs_tools.rs` | +85 | 3 Args structs + 3 `tool_*()` builders + 3 dispatchers + builder count test (25→28) |
| `llm_synthetic_tools/mod.rs` | +6 | Re-exports |
| `llm_synthetic_tools/toolkit_packages.rs` | +20 | `gdocs` 22→28, `gdocsread` 6→9 (las 3 listings son reads) |
| `dag_engine/infrastructure/dag_tool_executor.rs` | +25 | Router: 3 imports + 3 match arms + 3 `is_gdocs_tool` checks |
| `text/tools/gdocs.yaml` | +72 | YAML entries con descripción + workflows |
| `docs/developer_guide/41_builtin_tools_index.md` | +5 | 3 filas + counts |
| `docs/developer_guide/45_gdocs.md` | +50 | Nueva sección "Drive Comments" con workflows |

**No breaking changes.** Tools son aditivos; el alias `gdocs` ahora
expande a 28 (era 22 — Bundle 2A/2B no había updateado el alias todavía,
sweep incluida en este commit). El alias `gdocsread` sube a 9 (las 3 list
tools son reads).

**Tests.** Full suite: **1737 PASS / 0 FAIL** (140 gdocs, 6 toolkit_packages,
coverage tests). Tests nuevos:
- `build_all_returns_28_tools` (gdocs_tools)
- `add_comment_args_deserialize_with_optional_anchor`
- `list_comments_args_default_include_resolved_is_false`
- `resolve_comment_args_deserialize_with_optional_content`
- `gdocs_package_has_all_tools` actualizado a 28
- `gdocsread_readonly_package_subset` actualizado a 9 (cambió contadores +
  agregó nuevas write substrings al filter: `add_comment`,
  `resolve_comment`, `unshare`)

**E2E LLM-in-the-loop:** `tests/graphs/agents/gdocs_bundle4a_e2e.json` —
el agente crea un doc, postea un comment, lo lista, lo resuelve, y vuelve
a listar (default + `include_resolved: true`) para confirmar que el
`resolved` flag flipó. Run:
`set -a && source .env && set +a; cargo run --bin dag_engine -- run tests/graphs/agents/gdocs_bundle4a_e2e.json --agent-session-id g_b4a_$(date +%s) --include-extra-info`.

**Bundle 4B (Apps Script) — diferido.** Necesita:
1. Agregar `https://www.googleapis.com/auth/script.scripts.execute` a
   `REQUESTED_SCOPES` en `src/libs/colmena/src/bin/colmena_oauth_setup.rs`.
2. Re-correr `colmena_oauth_setup` para regenerar el `refresh_token` con
   el scope nuevo.
3. Actualizar Secret Manager (`COLMENA_GOOGLE_OAUTH_REFRESH_TOKEN`).
4. Redeploy ADP worker.

Los scripts ejecutados también necesitan que su deployment ID sea
explicitamente whitelist-ed; ese es config en el ADP / Apps Script side.
Spike de 1d cuando se confirme la ventana del re-consent.

**Estado.** done (4A). 4B diferido.

---

## 29. Cache-safe temporal context — timestamp fresco sin romper prompt caching (2026-06-11)

**Origen.** Investigación durante la verificación E2E del feature de cache
(item 11). Se descubrió que el bloque **Temporal & Geographic Context** (§35)
iba como **primera sección** del system message con un timestamp de
granularidad de segundos, DENTRO del prefijo que los 3 providers cachean.

**Lo que NO era un bug (aclaración importante).** El cache de item 11 **funciona
correctamente**. Verificado live 2026-06-11:
- Anthropic sonnet-4-6: turn 1 `cache_write 1824` → turn 2 `cache_read 1824`
  cross-proceso (mismo `agent_session_id`).
- El timestamp NO rompía el cache porque el gate `if !history_exists`
  **congela** el system en turn 1 y lo reusa desde memoria en turns siguientes.

**El costo oculto que SÍ se arregló.** El freeze implicaba que en conversaciones
largas el modelo veía una **hora vieja** (turn 40 con la hora del turn 1).
Trade-off forzado: hora-fresca XOR cache.

**Bonus encontrado:** `claude-haiku-4-5` tiene un mínimo cacheable
**empíricamente mayor a ~2900 tokens** (no cachea ni ahí), pese a que el doc
dice 2048. El E2E original usaba haiku → daba `cache_read=0` y parecía bug del
feature. Migrado a sonnet-4-6.

**Fix shipped.** El bloque temporal ahora se inyecta como **suffix volátil al
FINAL** del system, FUERA del prefijo cacheado, regenerado **cada turno**:

- Nuevo campo `LlmConfig::volatile_system_suffix` (aditivo, `#[serde(default)]`
  → no-breaking; ADP no construye `LlmConfig` directo, verificado).
- `llm.rs`: el temporal se computa cada turno (fuera de `if !history_exists`)
  y se setea como suffix; sale del `sections` estable (que se persiste
  congelado y cacheable).
- **Anthropic** (`anthropic_adapter`): system de 2 bloques —
  `[estable (cache_control: ephemeral), temporal (sin marker)]`. El marker
  cubre solo el bloque estable.
- **OpenAI** (`openai_adapter`, ambos paths Chat Completions + Responses): el
  suffix se concatena al final del system content.
- **Gemini** (`gemini_adapter`): el suffix se concatena al final del
  `systemInstruction`.
- **Strip-on-load** (`agent_service`): migración para conversaciones
  pre-fix que tienen el temporal horneado al frente del system persistido —
  se borra al cargar de historial para evitar doble-temporal.

**Resultado: las dos cosas a la vez** — timestamp fresco cada turno Y cache
intacto.

| | Hora fresca | Cache intra-conv | Cache cross-conv |
|---|---|---|---|
| Antes (freeze) | ❌ vieja | ✅ | ❌ |
| Después (suffix volátil) | ✅ | ✅ | ✅ |

**Tests.** 1701 unit PASS / 0 FAIL. Nuevos:
- `anthropic_adapter`: `volatile_suffix_emits_two_system_blocks_marker_on_first_only`,
  `no_suffix_keeps_single_marked_system_block`.
- `openai_adapter`: `chat_completions_appends_volatile_suffix_after_stable_system`,
  `responses_appends_volatile_suffix_after_stable_system`, `no_suffix_leaves_system_unchanged`.
- `gemini_adapter`: `volatile_suffix_appended_to_system_instruction`,
  `no_suffix_leaves_system_instruction_unchanged`.
- `agent_service`: `strip_temporal_removes_leading_block_keeps_rest`,
  `strip_temporal_drops_block_when_only_section`,
  `strip_temporal_leaves_non_temporal_system_untouched`.

**E2E live — los 3 providers** (`tests/graphs/agents/provider_cache_temporal_{anthropic,openai,gemini}_e2e.json`):
- **Anthropic** sonnet-4-6: turn 1 `cache_write 1573` → turn 2 `cache_read 1573`,
  con timestamp cambiando (prompt 404→418). ✅
- **OpenAI** gpt-4o: turn 2 `cache_read 4224` con prefijo ≥2K. ✅
- **Gemini** 2.5-flash: turn 5 `cache_read 7818` tras warmup (~3-5 calls).
  Curl crudo probó que cachea el prefijo estable (3047 tok) **aunque el suffix
  temporal cambie cada call** → fix 100% compatible con implicit caching. ✅

Spec completo:
[`docs/superpowers/specs/2026-06-11-temporal-block-cache-safe-design.md`](superpowers/specs/2026-06-11-temporal-block-cache-safe-design.md).
Dev guide: §14 (mínimos reales + cómo funciona el suffix), §35 (nota de
actualización).

**Estado.** done.

---

## 30. Quick-wins batch — gsheets collision envelope + CRDT aliases (2026-06-11)

Lote de tres mejoras chicas de ergonomía/correctness (🟢 quick wins de la
priorización por impacto×esfuerzo en `BACKLOG.md`). Todas additive, sin
breaking changes. ADP no implementa `SheetsClient` → sin sweep necesario
(verificado: 0 impls fuera de colmena).

**QW1 — Header range deja de truncar en columna Z.** `fetch_tab_meta`
(`gsheets_run_python.rs`) leía el header con un `A1:Z1` hardcodeado, así que
sheets con >26 columnas reportaban `current_state.columns` truncado y el LLM
decidía colisiones con info incompleta. Ahora el rango se computa desde
`meta.col_count` vía `a1_addr` (e.g. 30 cols → `A1:AD1`). Test:
`header_range_spans_past_column_z` (boundary Z/AA/AD) +
`fail_envelope_has_wide_columns_and_last_modified` (30 headers surface).

**QW2 — `crdt_doc_run_python` acepta `sheets`/`sheet_names` como alias de
`sheet_ids`.** Cierra el "Also:" del item de UX aliases — el lado gsheets
(`var`/`binding_name`/`name`, `sheet`/`sheet_name`, drop de `output_sheets`
como arg) ya estaba shipped. `#[serde(alias = "sheets", alias =
"sheet_names")]`. Test: `sheet_ids_accepts_sheets_and_sheet_names_aliases`.

**QW3 — `last_modified` en el envelope `SheetExists`.** Nuevo método
best-effort `SheetsClient::get_modified_time` (Drive `files.get?fields=
modifiedTime`, impl HTTP + FakeClient). `TabMeta` gana `last_modified:
Option<String>`; `build_sheet_exists_error` lo expone en
`current_state.last_modified` cuando está presente. Permite al LLM (y a la
persona leyendo el reporte) distinguir data fresca de data del año pasado
antes de overwrite. Falla del Drive call → degrada a `None`, nunca tumba el
fetch. CRDT pasa `None` (no tiene concepto de `modifiedTime`). Tests en
`sheet_collision.rs` (presencia + omisión) + el wiremock combinado arriba.

**Archivos:** `gsheets/domain/traits.rs`, `gsheets/infrastructure/http_client.rs`,
`llm_synthetic_tools/{sheet_collision,gsheets_run_python,crdt_doc_run_python,gsheets_tools}.rs`.

**E2E live (real Google Sheets, OAuth user-scoped `agents@startti.co`)** —
`tests/graphs/agents/gsheets_collision_envelope_e2e.json` (app-created sheet) +
`gsheets_collision_envelope_existing_e2e.json` (operator-shared sheet):
- **QW1 ✅** ambos runs: el envelope `SheetExists` surface **30 columnas**
  (`c0..c29`, `n_cols: 30`) — el `A1:Z1` viejo habría cortado en `c25`.
- **QW3 ⚠️ hallazgo de scope:** `last_modified` aparece SOLO en sheets
  **creados por la app** (run sobre sheet propio → `2026-06-11T20:01:44.922Z`).
  En un sheet **operator-shared** (creado por el usuario, compartido con
  `agents@startti.co`) el campo **degrada a ausente** porque el scope OAuth
  actual `drive.file` NO cubre `files.get` de archivos que la app no creó.
  El best-effort se comporta como diseñado (None, sin crash). Para tener
  `last_modified` en sheets compartidos hace falta agregar
  `drive.metadata.readonly` al consent — ver follow-up en BACKLOG
  ("OAuth scope para last_modified en sheets compartidos"). El Sheets API
  (scope `spreadsheets`) sí cubre lectura/escritura del sheet compartido,
  por eso QW1 funciona ahí.
- Gotcha operacional: el binario release embebe Homebrew python@3.14;
  `gsheets_run_python` necesita
  `PYTHONPATH=.venv/lib/python3.14/site-packages` (ABI matching) o cada call
  muere con `ModuleNotFoundError: pandas`.

**Estado.** done (con caveat de scope documentado para QW3 en sheets compartidos).

---

## 31. Google Workspace prelude — preferir COMPARTIR un doc existente sobre CREAR uno nuevo (2026-06-11)

Cambio de guía LLM-facing (no rompe nada, additive). El prelude auto-inyectado
para agentes con tools `gsheets_*`/`gdocs_*` (`google_workspace_prelude.rs`)
ahora incluye un bloque de **preferencia explícita**: cuando el usuario necesita
una planilla o documento, la opción por defecto es pedirle que **comparta uno
EXISTENTE** (como Editor, con el share email del agente) en vez de crear uno
nuevo. Razón: un doc creado por el agente vive en la **cuenta del agente**
(`agents@startti.co`), no en el Drive del usuario — el usuario no lo posee ni lo
ve a menos que el agente lo comparta de vuelta. Crear queda como fallback para
cuando el usuario lo pide explícitamente o no tiene nada que compartir.

Presente en ambas variantes del prelude (con y sin share email). Reforzado en
las descripciones de los tools de creación (`gsheets_create_spreadsheet`,
`gdocs_create`, `gdocs_create_from_markdown`) con un bloque "PREFER SHARING OVER
CREATING". Las skills `gsheets-*`/`gdocs-*` no necesitaron cambios (son de
análisis/edición, no de creación).

Token cost del prelude: ~140→~215 tokens con email. Tests:
`prelude_prefers_share_over_create_in_both_variants` + los pins existentes de
repetición de email / anti-compresión siguen verdes (13 tests del módulo).

**Estado.** done.

---

## 32. gdocs_insert_image_after_text — path (i) URL-only inline image insert (2026-06-12)

Quick win (🟢): nuevo tool `gdocs_insert_image_after_text` que inserta una imagen
inline justo después de un anchor content-addressed. `image_url` debe ser una URL
http(s) **pública** (PNG/JPEG/GIF, ≤50 MB, ≤2000 chars) — Google baja los bytes
server-side. `width_pt`/`height_pt` opcionales (PT); omitidos → tamaño nativo.

**Hallazgo que simplificó el scope:** NO hizo falta un método nuevo en
`DocsClient`. `insert.rs` ya resuelve el anchor a un índice (`find_anchor`) y
emite un `Request` JSON genérico vía `DocsClient::batch_update`; insertar imagen
es el mismo patrón con un request `insertInlineImage` en vez de `insertText`.
Reusa `find_anchor` + `apply_and_finalize`.

**Scope path (i) URL-only.** Insertar desde un `attachment_id` (signed-URL o
imagen generada que requiere subir bytes a Drive) queda como follow-up v1.1 —
ver BACKLOG. `validate_image_url` rechaza non-http / attachment-ids / URLs >2000
chars con un mensaje accionable.

**Wiring (espejo de `insert_after_text`):** `insert.rs` (use case + helpers +
7 tests), `gdocs_tools.rs` (const + args + tool builder + dispatcher +
`build_all_gdocs_tools` 28→29), router en `dag_tool_executor.rs` (is_gdocs_tool
+ match), `mod.rs` re-exports, `llm.rs` exposure (`gdocs_entries` 22→23),
`toolkit_packages.rs` (alias `gdocs` 28→29), `text/tools/gdocs.yaml`, índice §41,
dev guide §45. 1712 unit tests pass.

**Nota — gap encontrado (separado, no fixeado acá):** las herramientas de Bundle
2A/2B/4A (`gdocs_list_documents`, `gdocs_list_permissions`, `gdocs_unshare`,
`gdocs_add_comment`, `gdocs_list_comments`, `gdocs_resolve_comment`) están en
`build_all_gdocs_tools()` (28) y en el router de dispatch, pero **NO** en el
array `gdocs_entries[22]` de `llm.rs` que es el que construye las
`ToolDefinition` que el LLM ve. Es decir: dispatch-ready pero posiblemente
**no expuestas** al modelo vía el alias `gdocs`. Flagged para verificación.
(Confirmado y fixeado en §33.)

**Verificación E2E live (real Google Docs):** sobre un doc compartido con
`agents@startti.co`, el agente ubicó un anchor vía `gdocs_read_outline` (Docs
API) e insertó la imagen pública — change record
`{kind:insert, after:"[image] https://…googlelogo…", tab_id:"t.0"}` + nuevo
`revision_id_after`. El `insertInlineImage` fue aceptado por la Docs API real.
Graph: `tests/graphs/agents/gdocs_insert_image_e2e.json`. **Hallazgo de scope:**
`gdocs_read_as_markdown`/`gdocs_export`/`gdocs_list_documents` (Drive API,
`drive.file`) fallan con `403 appNotAuthorizedToFile` en docs compartidos; las
tools Docs-API (`read_outline`, `insert_*`, `replace_*`) sí funcionan — ver dev
guide §45 "Caveat de scope en docs compartidos".

**Estado.** done (feature + E2E live verificados).

---

## 34. Agent loop guard + graceful rescue (2026-06-14)

**Qué cambió.** El bucle ReAct del agente ya no muere con `Err(MaxIterationsReached)`.
En su lugar, dos mecanismos coordinados garantizan que el agente siempre devuelva
una respuesta útil:

1. **Guarda de bucle por firma** (`max_tool_repeats`). La clave pública
   `max_iterations` ya no cuenta *turnos totales* — ahora es el presupuesto de
   *repeticiones consecutivas* de la misma firma `(nombre + argumentos)`. Default
   **3**. El contador se reinicia a cero cada vez que el modelo emite una firma
   distinta (cualquier progreso real). Mecánica con el default:

   | Repetición consecutiva | Acción |
   |---|---|
   | 1ª (primera vez) | Ejecuta la herramienta; guarda resultado. |
   | 2ª (nudge) | **No re-ejecuta**; devuelve resultado anterior + línea de redirección. |
   | 3ª (rescate) | Dispara síntesis forzada. |

2. **Techo duro de turnos** (`COLMENA_HARD_TURN_CAP`, default `50`). Variable de
   entorno — no configurable desde el JSON del grafo. Cuando se alcanza, también
   dispara la síntesis forzada. Los nodos de un solo turno (`planner`, `reactor`,
   `critic`, `orchestrator`, `extract_with_schema`) setean internamente
   `max_turns = 1`, preservando su comportamiento de un único turno.

**Rescate (síntesis forzada).** Cuando cualquiera de los dos límites se activa,
el engine hace **una llamada LLM final sin herramientas** con la instrucción de
dar la mejor respuesta posible con el contexto acumulado. Esa respuesta se
persiste en memoria conversacional y se retorna como `Ok(respuesta)`.
`MaxIterationsReached` sigue en el enum `LlmError` por compatibilidad, pero
**ya no se retorna** en el flujo normal del bucle.

**Por qué importa.** Antes del cambio, un agente productivo (sheets + pandas,
FRIKO comparison) podía agotar `max_iterations: 10` en turns legítimos y
fallar en seco — el usuario recibía un error, no una respuesta. Ahora el mismo
agente recibe un nudge si repite y una respuesta de síntesis si alcanza cualquier
límite. Un grafo legacy con `max_iterations: 10` permite ahora 10 repeticiones
consecutivas + techo de 50 turnos — siempre más permisivo, nunca muere antes.

**Textos LLM-facing** (mensajes de nudge y rescate) en el registro de texto:
`text/prompts/agent_loop/repeat_nudge.md` y `rescue_synthesis.md`.

**Documentación de referencia.**
- Dev guide: nuevo §"Guarda de bucle y rescate" en
  [`docs/developer_guide/14_llm_deep_dive.md`](developer_guide/14_llm_deep_dive.md)
- Spec:
  [`docs/superpowers/specs/2026-06-13-agent-loop-guard-and-rescue-design.md`](superpowers/specs/2026-06-13-agent-loop-guard-and-rescue-design.md)

**Commits (feat/agent-loop-guard-rescue).** Ver `git log --oneline feat/agent-loop-guard-rescue`.

**Impacto ADP.**
- La clave pública `max_iterations` en los grafos/config del agente es la misma —
  **ADP no necesita cambiar ningún grafo**.
- El worker ADP debe ser verificado para cualquier dependencia en `MaxIterationsReached`
  (`apps/service/ia/platform/{worker,api}/src/`). En todos los casos documentados el
  worker solo propaga el `Result<_, LlmError>` — recibir `Ok` donde antes llegaba
  `Err` es estrictamente mejor — pero la sweep es obligatoria antes del merge a
  `develop` por la disciplina de breaking-change.
- Wire-format del SSE sin cambios. ADP worker recompila clean.

**Estado.** done.

---

## 33. Fix — 6 gdocs tools were dispatch-ready but invisible to the LLM (2026-06-12)

**Bug (latent since Bundle 2A/2B/4A):** the LLM-facing exposure of gdocs synthetic
tools is built in `llm.rs` from two hand-maintained parallel arrays (`all_gdocs`
names + `gdocs_entries` name→builder). The build loop only emits a `ToolDefinition`
for tools present in `gdocs_entries`; **there is no by-name fallback**. Bundle
2A/2B/4A added 6 tools — `gdocs_list_documents`, `gdocs_list_permissions`,
`gdocs_unshare`, `gdocs_add_comment`, `gdocs_list_comments`,
`gdocs_resolve_comment` — to `build_all_gdocs_tools()` (collector), the `gdocs`
toolkit alias, AND the dispatch router, but **NOT** to `gdocs_entries`. Net effect:
with `enabled_tools: ["gdocs"]` those 6 were dispatch-ready yet never presented to
the model → effectively unreachable.

**Fix:** added the 6 to both `all_gdocs` (23→29) and `gdocs_entries` (23→29) in
`llm.rs`, plus the 3 missing `gdocs_tool_*` builder re-export aliases in `mod.rs`
(`list_documents`, `list_permissions`, `unshare` — the comment trio was already
re-exported). Added a loud CONTRACT comment at the arrays listing every site that
must stay in sync. 1712 unit tests pass.

**Verificación E2E live:** agente con `enabled_tools: ["gdocs"]` ahora **ve y
llama** `gdocs_list_documents` (11 selecciones en el ReAct loop) — antes del fix
el modelo no podía seleccionarlo porque no estaba en su lista. (El dispatch de
estos 6 ya estaba probado por los E2E de Bundle 2A/2B/4A.) Graph:
`tests/graphs/agents/gdocs_exposure_list_documents_e2e.json`.

**Follow-up recomendado (no hecho acá):** hacer estructural el contrato — derivar
la exposición de `llm.rs` desde una única tabla `(name, builder)` compartida con
`build_all_gdocs_tools()` para que el drift sea imposible. Se evaluó el refactor
eager (build-all-then-filter) pero regresaría a construir 29 schemas por turno;
la tabla de fn-pointers preserva el lazy-build. Ver BACKLOG.

**Estado.** done (exposure fixed + verified; structural refactor deferred).

---

## 35. gsheets expand-merges — forward-fill de celdas combinadas en lectura (2026-06-14)

**Qué cambió.** Al leer un Google Sheet, las celdas combinadas (merged cells)
ahora se rellenan automáticamente: cada celda de un merge devuelve el valor del
ancla (top-left), no un hueco. Antes, `spreadsheets.values.get` devolvía el valor
solo en la celda ancla y el resto del rectángulo venía vacío/`null`, lo que rompía
**en silencio** un `groupby`/`join`/comparación sobre una columna con merges
(p.ej. una "Categoria" que visualmente abarca varias filas llegaba a pandas como
1 valor + N `NaN`).

**Cómo.** `SheetsClient::read_range` pasó de `spreadsheets.values.get` a
`spreadsheets.get` con `includeGridData=true` (**Approach B**): una sola llamada
trae valores **y** rectángulos de merge juntos — fresco en cada lectura, sin
cache (otros editores podrían cambiar la estructura de merges durante el run) y
sin round-trip extra. El forward-fill vive en un módulo puro nuevo
`gsheets/infrastructure/merge_fill.rs` (`forward_fill_merges`). Como las dos
superficies LLM (`gsheets_read` y `gsheets_run_python`) llaman a `read_range`,
**ambas heredan el fill sin lógica propia**.

**Decisiones (always-on, sin flag).** Una celda combinada *realmente* contiene
ese valor en todo su span, así que rellenar **es** mostrar la verdad del sheet —
no hay flag de opt-out ni cache. Sub-rangos que cortan un merge: **B1** — solo se
rellena con anclas presentes en la grilla devuelta; si el ancla cae fuera del
rango leído, esas celdas quedan vacías (igual que antes), caso de borde
improbable.

**Mapeo de render options** (1:1 con campos de `CellData`): `FormattedValue` →
`formattedValue`; `UnformattedValue` → `effectiveValue`; `Formula` →
`userEnteredValue.formulaValue` (o el literal si no es fórmula).

**Cambio menor de comportamiento.** `ReadResponse.range` en lecturas de sheet
completo ahora es el nombre de la hoja en vez del extent A1 de Google; el extent
sigue disponible vía `dimensions` (calculado de `values`).

**Compat / ADP.** Sin break de API Rust — firma de `read_range`/`ReadOptions`/
`ReadResponse` intacta (cambio interno del adapter). El output observable cambia
(celdas antes vacías ahora traen valor) — intencional, solo afecta lo que ve el
LLM. ADP no requiere cambios.

**Documentación de referencia.**
- Spec: [`docs/superpowers/specs/2026-06-14-gsheets-expand-merges-design.md`](superpowers/specs/2026-06-14-gsheets-expand-merges-design.md)
- Plan: [`docs/superpowers/plans/2026-06-14-gsheets-expand-merges.md`](superpowers/plans/2026-06-14-gsheets-expand-merges.md)

**Verificación E2E live (Google Sheets real).** Sheet `colmena_expand_merges_e2e`
(tab `Ventas`, columna Categoria con merges verticales Frutas A2:A4 / Verduras
A5:A6). `gsheets_read` → el agente responde "Frutas" para la fila Pera (antes:
hueco). `gsheets_run_python` con `groupby('Categoria')['Monto'].sum()` → totales
correctos **Frutas=350 / Verduras=100** (sin fill darían 100/30). Grafos:
`tests/graphs/agents/gsheets_expand_merges_{read,python}.json`.

**Estado.** done (unit + E2E verificados contra Google real).

---

## 36. gsheets — instrucción "inspeccioná la tabla antes de correr python" (2026-06-14)

**Qué cambió.** Texto LLM-facing de gsheets para cerrar un fallo silencioso: con
un pedido vago ("subí 10 al monto de todas las frutas") un agente iba directo a
`gsheets_run_python` y **adivinaba la semántica** de los datos (filtraba
`Producto` por nombre en vez de la columna `Categoria`), matcheando 0 filas pero
reportándolo como éxito — sin ningún error.

**Causa raíz.** Contradicción entre dos capas always-on: el Google Workspace
prelude decía "leéla primero con `gsheets_read`", mientras la descripción de
`gsheets_run_python` implicaba autosuficiencia ("load each table as a binding").
El agente le creyó a la descripción de la tool.

**Cómo.** Dos instrucciones complementarias en la descripción de
`gsheets_run_python` (`gsheets.yaml`): **preventiva** (KNOW THE COLUMNS FIRST —
bindear carga las FILAS, no el esquema; leé primero si no conocés las columnas) +
**detective** (SANITY-CHECK ROW COUNT — 0 filas matcheadas ⇒ no reportes éxito,
reconsiderá). Alineación del `SHEET_WORKFLOW_PRELUDE`
(`google_workspace_prelude.rs`) para que el "read first" aplique explícitamente
al path de `gsheets_run_python` y para sumar la regla de 0 filas; la nota de
merged-cells se actualizó al nuevo auto forward-fill (§35).

**Verificación (sheet real con merges).** Las instrucciones son correctas:
gemini-2.5-pro las siguió al pie de la letra (leyó la tabla, entendió la columna
`Categoria`, aplicó +10 a las 3 frutas → 110/210/60). El fallo residual de
gemini-2.5-flash es **techo de capacidad**, no defecto de instrucción — pero la
regla detective igual mejora flash: elimina el falso éxito silencioso (ahora
frena y ofrece inspeccionar). Guard estructural para flash = follow-up opcional.

**Compat.** Solo texto LLM-facing. Sin cambios de API, sin impacto en ADP. Spec:
[`docs/superpowers/specs/2026-06-14-gsheets-inspect-before-python-design.md`](superpowers/specs/2026-06-14-gsheets-inspect-before-python-design.md).

**Estado.** done.

---

## 37. gsheets — guard estructural "inspeccioná antes de correr python" (2026-06-15)

**Qué cambió.** Follow-up estructural de §36 (el fix de texto). El `DagToolExecutor`
ahora **intercepta** el primer `gsheets_run_python` que bindea una hoja **no leída
en este turno**: en vez de ejecutar el código a ciegas, devuelve un envelope
`inspect_first` con un **preview markdown acotado** (header + 5 filas) de cada hoja
no-vista, y obliga al agente a re-llamar con código informado. Garantiza que el
agente vea las columnas reales **independiente de la capacidad del modelo** — cierra
el techo de capacidad que el fix de texto (§36) no podía superar en gemini-2.5-flash.

**Cómo.** Read-state per-turno (`gsheets_seen_sheets: Mutex<HashSet<String>>` keyed
`"spreadsheet_id::sheet"`) en `DagToolExecutor` — el executor se construye una vez
por ejecución de `llm_call`, así que el set es naturalmente per-turno (sin
persistencia cross-turno; consistente con el no-cache de §35). `gsheets_read` (éxito)
marca la hoja vista; `gsheets_run_python` chequea sus bindings de hoja antes de
ejecutar. El intercept reusa `dispatch_gsheets_read` para el preview (markdown, con
expand-merges ya aplicado) y lo trunca a 6 filas. Bindings inline (`data:`) se
ignoran; el envelope no lleva clave `error` (el agente lo trata como informativo y
re-llama). No-loop: el intercept marca la hoja antes de devolver. Helpers puros en
módulo nuevo `gsheets_inspect_guard.rs`.

**Límite honesto.** El guard garantiza que el agente VEA la tabla, no que use las
columnas bien. Combinado con la regla detective de texto (§36, 0 filas → pará) la red
es fuerte, pero no es garantía del 100% en un modelo débil.

**Compat.** Cambio de comportamiento de `gsheets_run_python` (intercepta el primer
uso ciego). Aditivo desde la API Rust; el envelope es un tool result nuevo que el
agente maneja en-loop, no cruza el borde SSE de forma que requiera cambios en ADP.
Spec: [`docs/superpowers/specs/2026-06-15-gsheets-inspect-guard-design.md`](superpowers/specs/2026-06-15-gsheets-inspect-guard-design.md).

**Estado.** done (unit + E2E verificados).

---

## 38. Memoria conversacional — resumen semántico por rol + recall lossless (2026-06-19)

**Qué cambió.** El historial del `llm_call` ya no se trunca por caracteres. Al cargar
cada run (lazy, una sola vez) se compacta: los turnos recientes (presupuesto ~2.500
tokens) y los primeros 2 van **completos**; los del medio se colapsan en un mensaje
`system` `## Conversation summary` con **una línea `[Tn]` por mensaje** según política
por rol — texto `<250` chars verbatim, texto `≥250` **resumen semántico** (modelo
barato, cacheado en la nueva columna `summary` de `llm_node_history`), `tool_calls`
como línea estructural, y andamiaje viejo (`load_skill`/`describe_tool`/`load_attachment`)
como marker. `recall_history` ahora es **lossless y paginado** (`offset`/`limit`/
`next_offset`, sin el viejo cap de 10 KB) → cualquier turno (incl. artefactos grandes)
se reconstruye verbatim. Nuevo registro editable `text/config/cheap_models.yaml`
(provider→modelo barato) con cadena de resolución `summary_model` (config) > env
`COLMENA_CHEAP_MODEL_<PROVIDER>` > yaml.

**Por qué importa.** El truncado a 180 chars cortaba por posición (no por relevancia),
se recomputaba en cada iteración y no sintetizaba. Ahora el modelo conserva el hilo de
la conversación al crecer, sin perder direccionabilidad: cada `[Tn]` mapea al ordinal
de DB y se recupera verbatim. Verificado E2E: en una charla de 15 mensajes con tools, el
modelo recuperó un dato del medio vía `recall_history(turn=5)` y respondió correcto.

**Compatibilidad.** Aditivo: migración `summary TEXT` nullable (pg + sqlite); métodos
nuevos del trait `ConversationRepository` (`get_with_summaries`/`set_summary`) con default
impls; `AgentService::with_message_summarizer` builder aditivo. **No rompe la API pública**
→ seguro para el worker de ADP. Si `llm_node_history` está espejada en el schema Prisma de
ADP, agregar la columna `summary` como follow-up.

**Documentación de referencia.**
- Spec: [`docs/superpowers/specs/2026-06-18-conversation-semantic-summary-design.md`](superpowers/specs/2026-06-18-conversation-semantic-summary-design.md)
- Planes: [`docs/superpowers/plans/2026-06-18-*`](superpowers/plans/) (recall-pagination, cheap-models, summary-column-and-repo, semantic-summary-core)
- Dev guide: [`developer_guide/15_memory_guide.md`](developer_guide/15_memory_guide.md) §"Compactación y recuperación de memoria"

**Commits.** PR [#108](https://github.com/Startti/colmena/pull/108) (`8794ea0`..`4c20d638`).

**Estado.** done (1788 unit tests + E2E real con gemini-2.5-flash + Postgres).

---

## 39. Subgrafos / LLMs como tools — agents-as-tools (2026-06-19)

**Qué cambió.** `node_type: "subgraph"` ahora es válido en `tool_configurations`,
así un `llm_call` puede exponer un grafo hijo existente (`child_graph_path`) o un
`llm_call` inline (`child_graph_inline`) como una sola tool que el LLM decide invocar
en su loop (patrón agents-as-tools). Entrada por defecto: un único `task` string
(inyectado como `{{task}}` en el hijo); estructurada vía `node_schema`. Memoria
**stateless por llamada** (path qualifier efímero determinista derivado del
`tool_call_id`). Streaming **transparente** (eventos `subgraph-*` del hijo se emiten al
stream del padre). **HITL completo** (el sub-agente puede suspender; el `SUSPENDED`
hace bubble-up por el loop de tools y el resume reanuda al hijo en la misma tool call).
Guard de recursión `MAX_SUBGRAPH_TOOL_DEPTH = 5`.

**Por qué importa.** Cierra el hueco entre "nodos hoja como tools", el nodo `subgraph`
(disparado por edges, determinista) y el `orchestrator` (Planner por adelantado): ahora
el LLM decide en su loop reutilizar una capability ya construida sin reescribirla.

**Gaps cerrados (descubiertos vs el código real).**
- `SubGraphNode` lee `child_graph_path`/`inline` desde `inputs` (el tool path mergea
  `fixed_config` en inputs y pasa `config={}`), fallback a `config` (camino por-edges intacto).
- `SubGraphNode::schema()` expone `task` por defecto (el builder de tools lee `schema["inputs"]`).
- `DagToolExecutor` inyecta `__colmena_node_id_path = tool/<tool_call_id>` (determinista,
  clave para que el resume HITL reconstruya el scope) y `__colmena_subgraph_depth`.
- Observer enhebrado en el tool path (`with_observer`) → streaming `subgraph-*`.
- **Bonus `suspend`:** el nodo `suspend` ahora resuelve `id`/`question`/`options` desde
  `inputs` (helper `cfg_or_input`), habilitando `suspend` como tool — requisito para que
  un sub-agente decida preguntarle al usuario.

**Hallazgos del E2E.** (1) Con `node_schema` presente el executor ignora `fixed_config`,
así que `child_graph_path` debe ir DENTRO de `node_schema` como `fixed`. (2) Un hijo con
entrada estructurada necesita un `prompt` explícito que template las variables (el
`llm_call` usa el input `task` como prompt implícito y en structured no hay `task`).

**Documentación de referencia.**
- Spec: [`docs/superpowers/specs/2026-06-18-subgraph-as-tool-design.md`](superpowers/specs/2026-06-18-subgraph-as-tool-design.md)
- Plan: [`docs/superpowers/plans/2026-06-18-subgraph-as-tool.md`](superpowers/plans/2026-06-18-subgraph-as-tool.md)
- Dev guide §19: [`docs/developer_guide/19_nested_agents_and_subgraphs.md`](developer_guide/19_nested_agents_and_subgraphs.md) ("Subgrafo como Tool")
- Schema de tools: [`docs/node_as_tools_reference.json`](node_as_tools_reference.json) (entry `subgraph` + whitelist)
- Grafos E2E: `tests/graphs/agents/subgraph_tool_*.json` + `tests/graphs/agents/sub/`

**Commits.** `bcfec6c`..`969abbaf` (rango en `claude/magical-banzai-7af56a`).

**Compat.** Puramente aditivo: nuevos builders (`with_observer`, `with_subgraph_depth`),
campos privados, una const. `ExecutableNode::execute` sin cambios de firma. ADP no afectado
(el frontend ya renderiza `subgraph-*`).

**Estado.** done (1794 unit tests + clippy limpio; E2E T1–T7 verificados contra
gemini-2.5-flash + Tavily, incl. ciclo HITL suspend→resume real).

---

## 40. Digest estructurado de tool-results (v1.1) — 2026-06-19

`build_compacted_messages` ahora renderiza un **digest determinista** para
mensajes `tool` cuyo contenido es JSON estructurado (objeto, array de objetos,
array de escalares), en lugar de un resumen NL con pérdida.

- Nuevo módulo puro `llm/application/tool_digest.rs` → `digest_tool_result(content) -> Option<String>`.
- Digest: esquema (columnas/campos) + N filas + muestra de filas + min/max de
  hasta 3 columnas numéricas; para objetos: inventario de campos, escalares
  inline, marcadores de anidados (`items[8]`, `customer{2}`) y drill-down en el
  array-de-objetos dominante.
- **Sin LLM, sin cache, sin migración, sin cambio de API pública** — se computa
  fresco cada load porque es determinista y barato. Resultados de tools en NL
  (no-JSON) caen al resumen semántico de v1, sin cambios.
- La línea cita `recall_history(turn=N)`; el resultado completo se recupera
  verbatim (recall lossless de v1).
- Motivación: que el modelo conserve la FORMA de los datos (qué columnas/campos
  existían) al envejecer el mensaje, evitando alucinación o recall a ciegas.
  Caso real: agente de datos/soporte que reusa campos específicos turnos después.

**Documentación de referencia.**
- Spec: [`docs/superpowers/specs/2026-06-18-conversation-semantic-summary-design.md`](superpowers/specs/2026-06-18-conversation-semantic-summary-design.md) §"Enhancements futuros"
- Plan: [`docs/superpowers/plans/2026-06-19-tool-result-structured-digest-v1-1.md`](superpowers/plans/2026-06-19-tool-result-structured-digest-v1-1.md)
- Dev guide: [`developer_guide/15_memory_guide.md`](developer_guide/15_memory_guide.md) §"Digest estructurado de tool-results (v1.1)"

**Tests.** 11 unit en `tool_digest`, 1 de wiring en `history_compaction`, 2 E2E reales (simple + multi-tool).

**Estado.** done.

---
