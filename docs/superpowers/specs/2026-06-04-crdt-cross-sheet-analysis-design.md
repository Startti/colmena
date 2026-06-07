# Subsistema F — CRDT Cross-Sheet & Cross-Artifact Analysis

> **Estado:** spec.
> **Fecha:** 2026-06-04.
> **Branch:** `feature/docs`.
> **Predecesores:** subsistema B (recent changes + artifact discovery), subsistema C (pandas integration).
> **Roadmap:** F es el 3º de los 6 subsystems del MVP CRDT collaborative editing. Próximos: D (formulas), E (Google Sheets), A (microservice deploy).

---

## 1. Objetivo

Habilitar que un agente compare, una, enriquezca o transforme datos **entre dos o más sheets**, ya sea dentro del mismo artifact o trayéndolas desde otro artifact al workspace actual. La motivación viene del uso real con xlsx: la gente mantiene "varios excels con varias hojas que quieren comparar" — desde versionados del mismo reporte (Q3 vs Q4) hasta enrichments cross-tabla (ventas + catálogo + reglas de descuento).

**Anti-objetivo:** F no es "un tool de compare". El concepto unificador es **análisis cross-sheet**, donde "compare" es uno de seis patrones canónicos que pandas resuelve nativamente una vez los DataFrames están en el mismo proceso.

## 2. Decisiones de diseño clave

### 2.1 Modelo "principal + secundarios" con clonado

El agente siempre opera desde un artifact pinneado (el "principal", determinado por `ctx.artifact_id()`). Para incorporar datos de otro artifact (el "secundario"), un tool dedicado clona la sheet completa al principal bajo un nuevo `sheet_id`. A partir de ahí, toda la lógica de análisis es **multi-sheet pero single-artifact**, lo cual reusa íntegramente la infraestructura de C (`crdt_doc_run_python` con `sheet_ids: Vec<String>`).

**Alternativas descartadas:**
- **Loader cross-artifact en `run_python`** — requeriría que `sheet_ids` aceptara union de `string | {artifact_id, sheet_id}`. Schemas con union types son frágiles entre providers de LLM (Gemini's proto schema, OpenAI strict mode). Además rompería el contrato del tool ya validado en C.
- **Tool `crdt_doc_compare` dedicado** — fuerza a enumerar tipos de comparación upfront; cualquier caso custom cae a `run_python` de todas formas, terminando con dos code paths que mantener.

El modelo clonado tiene un trade-off conocido: duplica datos en el principal. Para v1 esto es aceptable — los caps (100 MB combinado a nivel `run_python`, 100 sheets máximo por artifact) protegen contra abuso, y la composabilidad post-clone vale la duplicación.

### 2.2 Forward-compatible con multi-session

Una preocupación explícita del owner: "diferentes agentes en diferentes turnos incluso con diferentes agent session id pueda crear artefactos que otros agentes modifiquen, lean o comparen". El modelo session-scoped que heredamos de B (donde `crdt_doc_list_my_artifacts` filtra por sesión) **NO** se cambia en F, pero F sí debe estar diseñado para no bloquear esa extensión futura.

La arquitectura propuesta lo logra naturalmente:

- `crdt_doc_list_sheets_of({artifact_id})` y `crdt_doc_import_sheet({source_artifact_id, source_sheet_id, ...})` **NO** enforcean session ownership a nivel de tool. Cualquier artifact_id válido del registry es accesible.
- El único punto session-scoped es el **descubrimiento** (`list_my_artifacts`). En v1 ese es el bottleneck.
- Cuando shipping `list_workspace_artifacts` o equivalente (BACKLOG v1.1), los tools de F siguen funcionando sin cambios — solo se reemplaza/extiende el discovery.

### 2.3 Sin cambios a `crdt_doc_run_python`

`run_python` ya acepta `sheet_ids: Vec<String>` desde C (validado en smoke con 1000-row dataset). Una vez que la sheet secundaria está clonada en el principal, su nuevo `sheet_id` se pasa junto al original. F no toca el dispatcher ni el contrato del tool de C — cero riesgo de regresión sobre C.

### 2.4 Snapshot, no live link

La copia es por valor (snapshot del state actual del Y.Doc). Si después alguien modifica la sheet origen, la clonada NO se actualiza. Live linking (suscribirse a cambios del origen y re-aplicar al clon) sería v1.1 — agrega complejidad significativa (sincronización bidireccional, conflict resolution, cleanup de subscripciones) sin valor demostrado en v1.

### 2.5 Auditoría cross-session-friendly

El evento que se registra en `crdt_doc_events` cuando se importa una sheet incluye el `artifact_id` origen en el `summary`:

```
"imported sheet 'Inventory' (1001 rows × 4 cols) from artifact art_01KT…"
```

Esto da trazabilidad de "qué entró desde dónde" incluso si el origen fue creado en otra sesión. El consumo de esa auditoría se hace via `crdt_doc_get_recent_changes` extendido con `artifact_id?` opcional (ver §3.3).

## 3. Especificación de tools

### 3.1 `crdt_doc_list_sheets_of` — peek a otro artifact

```rust
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListSheetsOfArgs {
    /// ID del artifact cuyo listado de sheets queremos.
    /// Puede ser cualquier artifact del registry. NO enforce session ownership
    /// — el agente debe haber obtenido este ID legítimamente (list_my_artifacts,
    /// prompt explícito, o futuro workspace listing).
    pub artifact_id: String,
}
```

**Response (success):**

```json
{
  "artifact_id": "art_01KT9XM632G5F9PDGP22MRS09B",
  "name": "Reporte Q4 2026",
  "sheets": [
    { "sheet_id": "sh_01KTA0…", "name": "Inventory", "n_rows": 1001, "n_cols": 4 },
    { "sheet_id": "sh_01KTA1…", "name": "Pricing",   "n_rows":  250, "n_cols": 6 }
  ]
}
```

**Response (errors):**

- `{ "error": "invalid_artifact_id", "value": "..." }` — el string no parsea como ULID de artifact.
- `{ "error": "artifact_not_found", "artifact_id": "..." }` — válido pero no existe en el registry.

**Implementación:** lookup en `runtime.registry.get(&aid)` → si presente, recorre `ydoc.getMap("workbook").get("sheets")` y proyecta name + counts. `n_rows`/`n_cols` se computan on-the-fly desde el `cells` Y.Map (no requiere migration SQL).

### 3.2 `crdt_doc_import_sheet` — el core de F

```rust
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ImportSheetArgs {
    /// Artifact origen — de donde se copia la sheet.
    pub source_artifact_id: String,
    /// Sheet origen dentro del artifact de arriba.
    pub source_sheet_id: String,
    /// Nombre que tendrá la sheet en el artifact destino (el ctx actual).
    /// Default si no se pasa: "<original_name> (from <art_xxxx>)" donde
    /// xxxx son los primeros 4 chars del ULID. Si hay collision, se aplica
    /// auto-suffix " (2)", " (3)" igual que df_writer (C).
    #[serde(default)]
    pub new_name: Option<String>,
}
```

**Response (success):**

```json
{
  "sheet_id": "sh_01KTB1…",
  "name": "Inventory (from art_KT9X)",
  "n_rows": 1001,
  "n_cols": 4,
  "source": {
    "artifact_id": "art_01KT9XM632G5F9PDGP22MRS09B",
    "sheet_id":    "sh_01KTA0…",
    "name":        "Inventory"
  }
}
```

**Response (errors):**

- `{ "error": "invalid_artifact_id", "value": "..." }`
- `{ "error": "source_artifact_not_found", "artifact_id": "..." }`
- `{ "error": "source_sheet_not_found", "artifact_id": "...", "sheet_id": "..." }`
- `{ "error": "self_import_forbidden", "artifact_id": "..." }` — el source es el mismo artifact del ctx (no tiene sentido y previene loops accidentales).
- `{ "error": "load_size_exceeded", "actual_bytes": N, "limit_bytes": 104857600 }` — mismo cap de 100 MB que `run_python`.
- `{ "error": "max_sheets_in_artifact_exceeded", "current": 100, "limit": 100 }` — el destino ya tiene 100 sheets. Cap hardcoded como `MAX_SHEETS_PER_ARTIFACT` constante en el módulo de import_sheet.

**Semántica:**

- **Snapshot, no live link.** Clona el estado actual de la sheet origen al momento del call; cambios futuros en el source (después del import) no se propagan al clone.
- **Solo valores de celdas** (`v` + `t` en el Y.Map). No copia formato (que no existe en CRDT v1).
- **Atomic** dentro de un solo `ctx.doc().transact_mut()` sobre el artifact destino — éxito total o nada se escribe. El source artifact NO se modifica en ningún caso.
- **No copia auditoría:** los eventos viejos del source no se traen. Se registra UN evento nuevo del import en el artifact destino (ver Side-effects).
- **Resolución de nombre:** si `new_name` no se pasa, usa los primeros 4 caracteres del ULID del source artifact (sin el prefijo `art_`) para identificación legible. Ejemplo: source artifact `art_01KT9XM632G5F9PDGP22MRS09B` → sheet name `"Inventory (from art_01KT)"`. Collision check + auto-suffix reusando `df_writer::resolve_unique_sheet_name` de C.

**Side-effects:**

1. Nueva sheet en `ctx.doc()` (el principal) → WS propaga al browser → tab nueva aparece live.
2. `ctx.mark_dirty()` → snapshot writer flushea en el próximo tick.
3. Evento de auditoría en `crdt_doc_events`:
   ```
   artifact_id = ctx.artifact_id()           (el principal)
   sheet_id    = <nueva sheet_id>
   origin      = "agent:{session_id}"
   summary     = "imported sheet '<source_name>' (N rows × M cols) from artifact art_xxxx"
   ```

### 3.3 `crdt_doc_get_recent_changes` — extensión con `artifact_id?` opcional

Mantiene contrato actual de B. Agrega un arg opcional para auditar artifacts distintos del ctx:

```rust
#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetRecentChangesArgs {
    /// Filtro: solo eventos de esta sheet.
    #[serde(default)]
    pub sheet_id: Option<String>,
    /// Cuántos eventos devolver. Default 20, max 100.
    #[serde(default)]
    pub limit: Option<usize>,
    /// NUEVO en F: si se pasa, audita ESE artifact en vez del ctx actual.
    /// Habilita inspección cross-artifact ("quién tocó X?") sin requerir
    /// que el agente esté pinneado al artifact que audita.
    /// Default: ctx.artifact_id() (comportamiento de B sin cambios).
    #[serde(default)]
    pub artifact_id: Option<String>,
}
```

**Backward compatibility:** todas las invocaciones de B existentes funcionan sin cambios (campo nuevo es opcional con default coincidente con el comportamiento anterior).

**Response (sin cambios respecto a B):**

```json
{
  "events": [
    { "id": 42, "artifact_id": "...", "sheet_id": "...", "origin": "agent:s_001",
      "summary": "...", "created_at": "2026-06-04T10:00:00Z" }
  ]
}
```

**Auth note:** el tool no aplica filtros de permisos sobre `artifact_id` arbitrario. En v1 cualquier agente con un artifact_id válido puede leer su audit log. Permisos por artifact son BACKLOG.

## 4. Skill `crdt-doc-cross-sheet-analysis`

Skill builtin cargada vía `config.skills.builtin: ["crdt-doc-cross-sheet-analysis"]`. Documenta 6 patrones canónicos con snippets pandas verbatim. Va en `src/libs/colmena/skills/crdt-doc-cross-sheet-analysis/SKILL.md`.

### 4.1 Frontmatter

```yaml
---
name: crdt-doc-cross-sheet-analysis
description: Use when comparing two sheets, joining/enriching data from one sheet into another, or transforming rows based on conditions from another sheet. Activates the workflow list_my_artifacts → list_sheets_of → import_sheet → run_python. Documents 6 canonical pandas patterns with verbatim code snippets. Load this BEFORE writing any compare/join/enrich code.
---
```

### 4.2 Contenido (resumen — texto completo en el SKILL.md)

**Sección 1 — Flujo canónico:** 4 pasos siempre iguales (discovery → peek → bring → analyze).

**Sección 2 — Los 6 patrones canónicos con código pandas verbatim:**

- **Patrón A** — Cell-by-cell diff (mismo shape, qué valor cambió). Usa `DataFrame.compare(align_axis=1)` con flatten del MultiIndex.
- **Patrón B** — Row diff por key column (el más común). Usa `pd.merge(..., how='outer', indicator=True)` + post-process para clasificar `only_in_A`/`only_in_B`/`changed`/`unchanged`.
- **Patrón C** — Schema diff (qué columnas existen y con qué dtype en cada uno). Set ops sobre `df.columns`.
- **Patrón D** — Statistical comparison (mean/std/median/t-test por columna numérica). Usa `scipy.stats.ttest_ind` con `equal_var=False`.
- **Patrón E** — Join/enrich (traer info de otra tabla). `pd.merge(..., how='left')` + reporte de unmatched.
- **Patrón F** — Conditional transform (aplicar reglas desde otra tabla). Merge + mask + asignación condicional.

Cada patrón especifica: cuándo usarlo, código completo (asume el shape contract de C — promote headers si A1 es título), output esperado.

**Sección 3 — Anti-patterns:**

- No importar el principal a sí mismo (el tool lo rechaza, igual el reminder ayuda).
- Antes de importar, chequear `crdt_doc_list_sheets` por si ya fue clonada en otra iteración.
- No forzar merge con typing distinto sin `pd.to_numeric`.

**Sección 4 — Cleanup:** las sheets clonadas se acumulan; la eliminación es v1.1. El cap de 100 MB protege el sandbox.

## 5. Cambios fuera de tools

| Componente | Cambio |
|---|---|
| `crdt_doc_run_python` | **NINGUNO**. La sheet clonada vive en el mismo artifact que el principal, `ctx.doc()` ya la ve. |
| SQL migrations | **NINGUNAS**. Reusa `crdt_doc_events` de B. |
| `df_records` / `df_writer` | **NINGUNOS**. El clonado opera al nivel Y.Map (raw cells), no necesita la projection records. |
| Frontend | **NINGUNO**. La sheet nueva aparece por WS sync igual que cualquier otra mutación del Y.Doc. |
| ADP | **CERO** modificaciones (restricción de proyecto). |
| `node_configurations.json` | Agregar entradas para los 2 tools nuevos. |
| `developer_guide/38_crdt_documents.md` | Nueva sección §5.7 documentando F. |

## 6. Testing strategy

### 6.1 Unit tests (Rust, sin Python)

Todos en `cargo test --lib` o `cargo test --test <name>`, sin `#[ignore]`.

**Archivo `tests/crdt_doc_import_sheet_test.rs` (nuevo):**

- `import_sheet_clones_cells_and_headers` — happy path con verificación celda por celda
- `import_sheet_auto_suffixes_on_name_collision` — collision → " (2)"
- `import_sheet_default_name_includes_short_source_id` — sin `new_name`, formato `"X (from art_xxxx)"`
- `import_sheet_rejects_source_not_found`
- `import_sheet_rejects_sheet_not_found`
- `import_sheet_rejects_self_import` — source = dest → error
- `import_sheet_rejects_when_exceeds_size_cap` — fabricar sheet >100 MB y verificar rechazo
- `import_sheet_rejects_max_sheets_in_dest` — pre-poblar 100 sheets en dest
- `import_sheet_records_audit_event_with_source` — verifica summary incluye source artifact_id
- `import_sheet_marks_dirty_for_snapshot_writer`

**Archivo `tests/crdt_doc_list_sheets_of_test.rs` (nuevo):**

- `list_sheets_of_returns_sheets_for_any_artifact` — cross-artifact sin filtro session
- `list_sheets_of_includes_row_col_counts`
- `list_sheets_of_rejects_not_found`
- `list_sheets_of_rejects_invalid_id`

**Archivo `tests/crdt_doc_recent_changes_test.rs` (extiende existente):**

- `get_recent_changes_artifact_filter_works` — con `artifact_id` distinto al ctx
- `get_recent_changes_backward_compat_no_arg` — comportamiento de B sin cambios

### 6.2 Integration test (con Python sandbox)

**Archivo `tests/crdt_doc_cross_sheet_e2e_test.rs` (nuevo, `#[ignore]`):**

Test end-to-end que crea 2 artifacts en memoria con datasets que se solapan parcialmente (5 SKUs en Q3, 5 en Q4 con 2 cambios + 1 nuevo + 1 borrado), ejecuta `import_sheet` + `run_python` con el patrón B (row diff por key), y assert el resultado:

- Artifact principal tiene 3 sheets post-flow (original + clonada + diff)
- La diff sheet contiene 1 `only_in_A` + 1 `only_in_B` + N `both` (matching SKUs)
- `crdt_doc_events` tiene 2 filas para el principal (import + write)

### 6.3 Browser smoke

**Archivo `tests/graphs/crdt_documents/f_cross_artifact_smoke.json` (nuevo):**

Grafo `trigger_webhook → llm_call → log` donde el agente recibe el prompt:

> "Tengo dos artifacts de ventas: el actual es Q3 y quiero compararlo con el Q4 ($DYNAMIC:art_q4). Hacé tres hojas en el actual: (1) row diff por SKU mostrando qué se agregó/borró/cambió, (2) schema diff de columnas entre ambos, (3) enrichment: agregar a Q3 una columna nueva 'Q4_Price' con el precio de cada SKU que también existe en Q4."

**Setup automático (`tests/graphs/crdt_documents/fixtures/gen_f_fixtures.py` nuevo):**

Genera 2 xlsx (`q3.xlsx`, `q4.xlsx`) con SKUs overlapping (10 en común, 3 únicos por lado, 2 con precio cambiado) — suficiente diversidad para que los 3 outputs sean visibles a simple vista.

**Tool calls esperados (sin retries idealmente):**

1. `crdt_doc_list_sheets_of(art_q4)`
2. `crdt_doc_import_sheet(art_q4, sh_q4_inv)`
3. `load_skill('crdt-doc-cross-sheet-analysis')`
4. `crdt_doc_run_python` (patrón B — row diff)
5. `crdt_doc_run_python` (patrón C — schema diff)
6. `crdt_doc_run_python` (patrón E — join/enrich)

**Verificaciones post-run (script bash):**

- Artifact principal tiene 5 sheets (orig + cloned + 3 diff)
- Audit log incluye el evento de import con `summary` que contiene `"from artifact art_"`
- Tokens razonables (<30K total)
- Browser muestra las 3 nuevas hojas live

### 6.4 Lo que NO se testea en v1

| Caso | Por qué se difiere |
|---|---|
| Cross-session real (sesión A crea, sesión B compara) | Bloqueado por workspace concept (BACKLOG); la API ya está lista |
| Live linking (cambios en source actualizan clone) | Decisión explícita: snapshot-only en v1 |
| Eliminación de sheets clonadas | BACKLOG; los caps protegen mientras tanto |
| Performance con sheets de 1M filas | Manual on-demand; no automatizado |
| Permisos por artifact (read-only vs read-write) | BACKLOG; v1 confía en posesión del artifact_id |

## 7. Plan de implementación (resumen — el detalle se materializa via writing-plans)

Granularidad por tarea esperada (siguiendo patrón de B/C):

| Task | Descripción |
|---|---|
| F-T1 | `crdt_doc_list_sheets_of` — tool + dispatcher + unit tests |
| F-T2 | `crdt_doc_import_sheet` — tool + dispatcher (clone logic) + unit tests (todos los happy + error paths) |
| F-T3 | Extender `crdt_doc_get_recent_changes` con `artifact_id?` opcional + tests de backward compat |
| F-T4 | Skill `crdt-doc-cross-sheet-analysis` — SKILL.md con los 6 patrones |
| F-T5 | Integration test e2e (`#[ignore]`) con clone + run_python (patrón B) |
| F-T6 | Browser smoke graph `f_cross_artifact_smoke.json` + fixtures generator |
| F-T7 | Docs — `developer_guide/38_crdt_documents.md` §5.7 + `node_configurations.json` |
| F-T8 | BACKLOG entries — multi-session workspace, live linking, sheet deletion, permisos |
| F-T9 | CHANGELOG — entrada nueva sección "4. F — cross-sheet analysis" |
| F-T10 | Final sweep — `cargo test --lib`, `cargo clippy`, `cargo fmt`, browser smoke |

Estimación: ~6-8 horas dev. La parte más sustancial es F-T2 (la clone logic + sus 8 paths de error con sus tests).

## 8. Riesgos y mitigaciones

| Riesgo | Probabilidad | Impacto | Mitigación |
|---|---|---|---|
| El clone duplica memoria | Cierto | Bajo (caps lo limitan) | Cap de 100 MB + max 100 sheets; documentar en skill que no acumule |
| Agente confunde source vs dest en import | Medio | Medio (hace import "al revés") | El tool rechaza `self_import` y el response incluye nombres legibles; skill clarifica con ejemplos |
| Schema mismatch entre A y B rompe el merge | Alto | Bajo (KeyError fácil de debuggear) | El error context de `run_python` (loaded_sheet_columns, ya shipped) + patrón C (schema diff) del skill ayudan |
| Sheets con headers desfasados (título row) | Alto | Medio (off-by-one como en C) | El skill referencia explícitamente `crdt-doc-run-python` para promotion pattern |
| Cross-session API silenciosamente leak data | Bajo en v1 | Alto si llega a prod sin workspace | Documentar explícitamente que v1 confía en posesión del artifact_id; BACKLOG el permission model como bloqueante para A (microservice deploy) |

## 9. BACKLOG (entries que voy a agregar)

Items deferidos cuya semilla nace en F pero que excede el v1:

1. **CRDT Documents v1.1 — Multi-session workspace visibility** — workspace concept, `list_workspace_artifacts`, permisos read/write/compare por artifact, mecanismo de share/link. Es bloqueante de Subsistema A (microservice deploy multi-tenant).
2. **CRDT Documents v1.1 — Live linking de sheets clonadas** — suscribirse a cambios en el source y re-aplicar al clone. Requiere conflict resolution policy + cleanup de subscripciones.
3. **CRDT Documents v1.1 — Eliminar sheets** — tool `crdt_doc_delete_sheet`. Necesario para que el agente limpie sheets clonadas temporales y para edición humana en el canvas.

## 10. Cómo se compone F con el resto del MVP

- **Sobre B:** F reusa `crdt_doc_events` para auditoría, `crdt_doc_list_my_artifacts` para discovery dentro de sesión. El tool extendido de B sigue siendo backward-compatible.
- **Sobre C:** F reusa `crdt_doc_run_python` sin tocar su contrato. Reusa `df_writer::resolve_unique_sheet_name` para naming collision.
- **Habilita D (formulas):** una vez que las formulas estén implementadas, las sheets clonadas pueden tenerlas (snapshot incluye fórmulas si las hay), permitiendo "comparar reportes con formulas" sin trabajo extra.
- **Habilita E (Google Sheets):** import vía Google Sheets API crearía un artifact nuevo; F lo trata igual que un xlsx importado.
- **Bloqueado por A:** sin permission model, no podemos exponer cross-session compare a prod multi-tenant. F debe documentar este gate.

---

## Apéndice — Referencias

- Spec subsistema B: [`2026-06-03-crdt-recent-changes-design.md`](2026-06-03-crdt-recent-changes-design.md)
- Spec subsistema C: [`2026-06-03-crdt-pandas-integration-design.md`](2026-06-03-crdt-pandas-integration-design.md)
- Developer guide: [`docs/developer_guide/38_crdt_documents.md`](../../developer_guide/38_crdt_documents.md)
- BACKLOG: [`docs/BACKLOG.md`](../../BACKLOG.md)
