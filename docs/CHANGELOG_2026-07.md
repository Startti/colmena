
# Cambios recientes — 2026-07

> **Alcance:** Commits sobre `feat/data-run-python` (y ramas siguientes) desde el cierre de `2026-06` hasta el merge eventual a `develop`.

## Cómo leer este documento

Una sección por feature. Cada sección contiene:
- **Qué cambió** — efecto observable.
- **Documentación de referencia** — spec, plan, dev guide, schema.
- **Commits** — rango o lista.
- **Estado** — done / partial.

---

## 1. `data_run_python` — soft-deprecation de `gsheets_run_python` + `attachment_run_python`

**Qué cambió.** `data_run_python` pasa a ser el **tool tabular primario** para leer, computar y escribir datos tabulares (attachments CSV/XLSX, Google Sheets, SQL SELECT, inline → pandas sandbox → write-back a `output_tables`/`output_sheets`/`output_attachments`). Los dos tools específicos previos, `gsheets_run_python` y `attachment_run_python`, quedan **soft-deprecados**:

- Ambos **siguen funcionando** y se mantienen registrados por compatibilidad con grafos persistidos (en la DB de ADP) que los nombran. `gsheets_run_python` permanece en el alias `gsheets` durante el bridge.
- Sus descripciones llevan ahora un prefijo `DEPRECATED — usá data_run_python`.
- Las 11 skills de gsheets instruyen al modelo a llamar `data_run_python`.
- El alias del toolkit `gsheets` ahora **incluye** `data_run_python` (`enabled_tools: ["gsheets"]` lo expone). Vía alias, la capacidad gsheets se auto-detecta; la capacidad SQL sigue requiriendo `fixed_config.sql`.
- `sql_inspect_attachment` y `sql_bulk_insert_from_attachment` **no** están deprecados (el volcado 1:1 crudo de un CSV vía COPY sigue siendo su dominio).

**Por qué importa.** Convierte lo que iba a ser un breaking-change-con-riesgo-externo (borrado duro que rompería skills, el alias, y grafos persistidos en ADP) en un cambio **aditivo y reversible, sin reroute de motor**. El código de los dos tools viejos no se toca (cero riesgo de comportamiento); solo se reorienta guía/skills/docs/alias hacia `data_run_python` y se marcan las viejas como deprecadas.

**Borrado real diferido (Fase 2, gated).** La eliminación del código de `gsheets_run_python`/`attachment_run_python` queda diferida a una Fase 2 gated en telemetría (~0 llamadas) + verificación de que no hay grafos persistidos en la DB de ADP usando esos `node_type` (o migración hecha). ADP no afectado en Fase 1 (sin cambio de API pública).

**Documentación de referencia.**
- Plan: [`docs/superpowers/plans/2026-07-02-data-run-python-soft-deprecation.md`](superpowers/plans/2026-07-02-data-run-python-soft-deprecation.md)
- Spec original: [`docs/superpowers/specs/2026-07-01-data-run-python-design.md`](superpowers/specs/2026-07-01-data-run-python-design.md)
- Dev guide: [`docs/developer_guide/48_data_run_python.md`](developer_guide/48_data_run_python.md) (primario), [`§39`](developer_guide/39_gsheets.md), [`§23`](developer_guide/23_sql_node.md)
- Índices: [`§41 builtin tools`](developer_guide/41_builtin_tools_index.md), [`§42 builtin skills`](developer_guide/42_builtin_skills_index.md)
- Schema: [`docs/node_as_tools_reference.json`](node_as_tools_reference.json) (`gsheets_run_python` marcado `deprecated`)

**Estado.** done (Fase 1 — soft-deprecation). Fase 2 (borrado) pendiente y gated.
