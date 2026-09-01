# Cambios recientes — 2026-09

> **Alcance:** Commits sobre `develop` desde el cierre de `2026-08`.

## Cómo leer este documento

Una sección por feature. Cada sección contiene:
- **Qué cambió** — efecto observable.
- **Documentación de referencia** — spec, plan, dev guide, schema.
- **Commits** — rango o lista.
- **Estado** — done / partial.

---

## 1. `sql_query`: eliminado el flag fantasma `guardrail_enabled`

**Qué cambió.** El nodo `sql_query` anunciaba en su `schema()` un campo de config
`guardrail_enabled` ("enables static validation rules") que **ningún código leía
jamás**. No existía ni existió nunca un `config.get("guardrail_enabled")` en
`sql.rs`. Un operador que ponía `guardrail_enabled: false` esperando desactivar la
validación estática no obtenía ningún efecto, y tampoco ninguna advertencia.

El campo se **eliminó** en lugar de cablearse. La validación estática es lo que
bloquea `DROP`, `TRUNCATE` y `DELETE`/`UPDATE` sin `WHERE`: hacerla apagable
habría sido un downgrade de seguridad, no un arreglo. Ahora es explícitamente
incondicional, con una nota en `sql.rs` para que el flag no se reintroduzca.

`guardrail_llm` **no se tocó** — ese guardrail sí es real y sigue siendo opcional
(`guardrail_llm.enabled`, default `false`).

**Sin cambio de comportamiento.** El campo vivía en el bloque `config` del
`schema()`, que es puramente descriptivo: el motor solo consume el bloque `inputs`
para construir la tool definition del LLM, y nunca valida claves desconocidas.
Por eso:

- Los grafos persistidos (incluidos los de ADP) que aún pasan
  `guardrail_enabled` como campo `fixed` **siguen funcionando sin cambios** — la
  clave sobrante se ignora, exactamente igual que antes.
- No hay cambio de API pública → **ADP no afectado**.

Se limpiaron además los lugares que propagaban el campo: la guía 23, el
`node_configurations.json` canónico, cuatro grafos de `tests/graphs/agents/` y la
skill `capability-data-sql`, que le enseñaba a los operadores a declararlo.

**Verificación.**

| Chequeo | Resultado |
|---|---|
| `cargo test --lib sql` | 182 passed, 0 failed |
| `cargo test --lib static_validator` | 27 passed, 0 failed — los bloqueos siguen intactos |
| E2E real vía DAG engine (`sql_query_readonly_test.json`, OpenAI + Postgres) | exit 0, el tool consultó la BD y devolvió tablas y `row_count` reales |

**Documentación de referencia.**
- [`docs/developer_guide/23_sql_node.md`](developer_guide/23_sql_node.md) — tabla de configuración.
- [`docs/qa/nodes/sql_query.md`](qa/nodes/sql_query.md) — hallazgo A1, marcado como resuelto.
- [`docs/qa/nodes/RESUMEN_GAPS.md`](qa/nodes/RESUMEN_GAPS.md) — resumen priorizado del audit.

**Origen.** Hallazgo de severidad Alta A1 del audit doc-vs-código por nodo (PR #226).

**Estado.** done.
