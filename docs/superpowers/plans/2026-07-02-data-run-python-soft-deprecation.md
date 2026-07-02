# Plan — Soft-deprecation de `gsheets_run_python` + `attachment_run_python` (a favor de `data_run_python`)

**Fecha:** 2026-07-02
**Rama base:** `feat/data-run-python` (sobre `develop`)
**Estado:** aprobado por el usuario (2026-07-02) — reemplaza el "hard delete" del Phase 7 del plan `2026-07-01-data-run-python.md`.

## Por qué soft-deprecation y no borrado duro

Auditoría adversarial (2026-07-02, 3 subagentes) — el **código** de `data_run_python` tiene paridad + superset con las dos tools viejas (comparten `sheet_writer.rs`/`table_writer.rs`; incluso añade generación de output CSV/XLSX). Pero un borrado duro rompe en 3 lugares que el compilador NO atrapa, más un riesgo externo:

1. **11 archivos de skills** nombran `gsheets_run_python` y le ordenan al modelo llamarlo → tool-not-found en runtime tras el borrado (`gsheets-editing`, `gsheets-cross-sheet-analysis` SKILL+6 refs, `gsheets-table-exploration` SKILL+1 ref).
2. **Alias de toolkit `gsheets`** (`toolkit_packages.rs:37`) incluye `gsheets_run_python`; borrarlo deja el alias apuntando a un nombre sin dispatcher.
3. **Grafos persistidos en la DB de ADP** con `node_type: gsheets_run_python` / `attachment_run_python` — fuera del alcance del repo; fallan al ejecutarse si desaparecen los nombres. (El repo ADP está limpio salvo `test_stream_cloud.html` de dev + docs.)
4. **Regresión de eficacia:** la guía del modelo casi no se migró — `gsheets_run_python` tiene **307 líneas** de descripción (4 modos de escritura, fórmulas `{{Col}}`, `update_by_position`, anti-patrones), `attachment_run_python` **38 líneas**; `data_run_python.yaml` tiene **23 líneas** y NO menciona `output_tables`/`output_sheets`/`output_attachments`, modos ni fórmulas. La maquinaria está cableada pero al modelo no se le dice que existe.

**Estrategia:** convertir un breaking-change-con-riesgo-externo en un cambio **aditivo, reversible y sin reroute de motor**. Las dos tools viejas quedan vivas e intactas (cero riesgo de comportamiento); solo (a) enriquecemos `data_run_python` para que el modelo lo use bien, (b) reorientamos toda la guía/skills/docs hacia él, (c) marcamos las viejas como deprecadas. El borrado real del código queda **diferido** (Fase 2, gated en telemetría + verificación de grafos persistidos en ADP).

## Principios (de CLAUDE.md — respetar)
- `[lints.rust] warnings = "deny"` — sin imports/dead-code sin usar.
- Additive only en Fase 1 — NO tocar el comportamiento de `gsheets_run_python`/`attachment_run_python`/`crdt_doc_run_python`.
- Correr `cargo test --verbose` (no `--lib`) antes de push.
- Skills auto-contenidas (sin rutas de repo); frontmatter `name:` == filename de reference.
- Docs en español, comentarios de código en inglés.
- No IDs reales de spreadsheet en grafos commiteados (`<SPREADSHEET_ID>`).

---

## FASE 1 — Enriquecer + reorientar (este PR, additive, sin breaking)

### Task 1 — Portar la guía a `data_run_python.yaml`
**Archivo:** `src/libs/colmena/text/tools/data_run_python.yaml`

Expandir la descripción (hoy 23 líneas) para cubrir lo que hoy vive en `gsheets.yaml`/`sql.yaml`. Debe documentar explícitamente (con ejemplos de código):

- **Los 3 sinks** y su contrato de campo exacto:
  - `output_tables = {"schema.tabla": {"mode": "append|update|upsert|replace", "df": <records>, "key": "..."}}` (campo `df`).
  - `output_sheets = {"<Pestaña>": {"mode": "replace|update_in_place|update_by_position|overwrite", "df": <df>}}` + `write_to_spreadsheet`.
  - `output_attachments = {"nombre.csv"|"nombre.xlsx": <df|spec>}`.
- **Modos de escritura de sheets** — los 4, cuándo usar cada uno; en especial `update_by_position` para editar filas SIN clave única SIN calcular A1 (bindear la hoja completa, modificar in place, devolver el df entero).
- **Fórmulas `{{Col}}`** en escritura a sheets.
- **Política de colisión** `on_existing_sheet` (`fail`/`auto_suffix`/`overwrite`) y el envelope `SheetExists`.
- **Anti-patrones** portados de gsheets: "conocé las columnas primero", "sanity-check del row count", coerción de tipos (`pd.to_numeric`).
- **Framing de selección de tool** revisado: `data_run_python` es la tool primaria para tareas tabulares (cómputo sobre 1 fuente o entre fuentes); solo mantener el framing "cross-store" como uno de varios casos, NO como el único (hoy dice "pick a source-specific tool when the task stays inside one store" — eso desalienta el caso de 1 attachment que era de `attachment_run_python`).

Mantener el `summary` corto para lazy-loading. Objetivo: que un modelo que lea SOLO esta descripción sepa leer, computar y escribir a los 3 destinos correctamente.

**Verificación:** `grep` de `update_by_position|output_tables|output_attachments|{{|on_existing_sheet` en el yaml → todos presentes.

### Task 2 — Reescribir las 11 skills a `data_run_python`
**Archivos:** los 11 que nombran `gsheets_run_python`:
- `skills/gsheets-editing/SKILL.md` + `references/create-and-populate.md`
- `skills/gsheets-cross-sheet-analysis/SKILL.md` + `references/pattern-{a,b,c,d,e,f}-*.md`
- `skills/gsheets-table-exploration/SKILL.md` + `references/01-inspect-schema-first.md`

Reemplazar cada instrucción de "llamá `gsheets_run_python`" por "llamá `data_run_python`" **traduciendo el contrato**:
- binding: la forma `bindings: [{var, spreadsheet_id, sheet, range?}]` ya es idéntica → sin cambio salvo el nombre de la tool.
- salida: `output_sheets` + `write_to_spreadsheet` ya son idénticos → sin cambio.
- Verificar que ningún ejemplo dependa de un global que solo existía en el prelude viejo.

Mantener frontmatter `name:` == filename. Skills auto-contenidas.

**Verificación:** `grep -rl gsheets_run_python src/libs/colmena/skills/` → vacío. `grep -rl data_run_python src/libs/colmena/skills/` → los 11 (o los SKILL.md relevantes).

### Task 3 — Alias de toolkit `gsheets`: añadir `data_run_python`, mantener el viejo (bridge)
**Archivo:** `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/toolkit_packages.rs`

Añadir `"data_run_python"` a la lista de tools del alias `gsheets` (y actualizar el `description` de "11 tools" → "12 tools"). **Mantener** `"gsheets_run_python"` en la lista durante el bridge para no romper grafos persistidos que usan `["gsheets"]` + prompt hardcodeado que nombra la tool vieja. La deprecación en la descripción (Task 4) hace que el modelo prefiera la nueva.

> Decisión: redundancia temporal de 2 tools solapadas en el alias es el precio de "cero rotura de grafos persistidos". Se elimina la vieja en Fase 2.

`data_run_python` en el alias necesita config: sin `fixed_config`, `enable_gsheets` no está activo. Verificar cómo se resuelve `enable_gsheets` cuando la tool entra por alias (no por `tool_configurations`) — puede requerir que el alias-expansion marque `enable_gsheets: true` por defecto para `data_run_python`, análogo a cómo hoy `gsheets_run_python` opera con creds de env. **Investigar en Task 3 antes de codear**; si no hay hatch, documentar que vía alias `data_run_python` sólo habilita gsheets (no SQL, que requiere `fixed_config.sql`).

**Verificación:** grafo con `enabled_tools: ["gsheets"]` expande e incluye `data_run_python` funcional contra Google real.

### Task 4 — Nota de deprecación en las descripciones de las 2 viejas
**Archivos:** `text/tools/gsheets.yaml` (sección `gsheets_run_python`), `text/tools/sql.yaml` (sección `attachment_run_python`).

Prepend una línea al `description`: `DEPRECATED — usá \`data_run_python\` en su lugar (misma capacidad, unificada). Esta tool se mantiene solo por compatibilidad con grafos existentes.` No cambiar args ni comportamiento. El `summary` (lazy) puede prefijarse con `[deprecated]`.

**Verificación:** las tools siguen despachando igual; solo cambia el texto que ve el modelo.

### Task 5 — Docs
**Archivos:** `docs/developer_guide/39_gsheets.md`, `48_data_run_python.md`, `41_builtin_tools_index.md`, `42_builtin_skills_index.md`, `docs/node_as_tools_reference.json`, `CLAUDE.md` (línea de status), `docs/CHANGELOG_2026-07.md` (crear/append).

- Presentar `data_run_python` como la tool tabular primaria.
- Marcar `gsheets_run_python`/`attachment_run_python` como "deprecadas — disponibles por compat, ver `data_run_python`".
- Actualizar el índice de skills (las 11 ahora apuntan a `data_run_python`).
- CHANGELOG: entrada de soft-deprecation.

### Task 6 — Verificación E2E (live)
Correr contra servicios reales (OAuth de Secret Manager `startti-dev`, in-memory), guardar SSE en `/tmp/colmena_e2e/`:
1. `["gsheets"]` alias → el modelo elige `data_run_python` (no el viejo) y escribe a un sheet real. ✔ steering.
2. Skill `gsheets-editing` activa → el modelo llama `data_run_python` (verifica Task 2). ✔
3. Grafo que nombra `gsheets_run_python` explícito → sigue funcionando (verifica no-rotura del bridge). ✔
4. `update_by_position` vía `data_run_python` con guía nueva → celdas correctas (regresión de la demo previa).

`cargo test --verbose` + `cargo clippy --all-targets` verdes.

### Task 7 — Commit + PR
Commits Conventional (`refactor:`/`docs:`/`feat:` — NUNCA `plan`/`spec`). Sweep ADP worker antes de push (es breaking-adjacent solo si tocáramos API pública — no lo hacemos, pero confirmar).

---

## FASE 2 — Borrado real (DIFERIDO — gated)

**Gates antes de arrancar:**
- Telemetría muestra ~0 llamadas a `gsheets_run_python`/`attachment_run_python` en un período razonable, O
- Verificación con ADP de que no hay grafos persistidos en la DB usando esos `node_type` (o migración de datos hecha), Y
- Fase 1 desplegada y estable.

**Trabajo (para no perder la paridad de attachment al borrar):**
1. Añadir a `data_run_python` un **adaptador de compatibilidad**: aceptar la forma vieja de args de attachment (`attachment_id` top-level → binding sintético) y aliasear los globals del prelude (`df` = DataFrame del primer binding de attachment; `result` espejado a `output`). Esto permite que llamadas con forma vieja funcionen sobre el motor nuevo.
2. Rerutear los match-arms de los nombres viejos → motor de `data_run_python` (o registrar los nombres como aliases del mismo dispatcher).
3. Borrar `gsheets_run_python.rs` (mantener `sheet_writer.rs`), `attachment_run_python.rs`, sus tests, entradas de `mod.rs`/`llm.rs`/`text`, y sacar `gsheets_run_python` del alias `gsheets`.
4. Migrar los grafos in-repo `tests/graphs/agents/gsheets_*.json` + `attachment_run_python_e2e.json` a `data_run_python`.
5. `cargo test --verbose` verde (deny-warnings atrapa refs colgantes).

---

## Blast radius / no-tocar
- **Mantener:** `sheet_writer.rs`, `table_writer.rs`, `sheet_collision.rs`, `tabular_bindings.rs`, `crdt_doc_run_python` (comparte infra).
- **ADP:** solo `apps/service/ia/platform/` si algo. `test_stream_cloud.html` (dev harness) y docs de ADP mencionan las viejas — actualizar en coordinación, no bloquea Fase 1.
- **Grafos persistidos en DB de ADP:** el motivo por el que Fase 1 NO borra nada.
