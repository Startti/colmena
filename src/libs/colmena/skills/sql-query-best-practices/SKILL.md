---
name: sql-query-best-practices
description: Use when calling the sql_query tool — covers multi-statement patterns, bulk loads, common pitfalls, what is blocked and why. Load the reference for the specific scenario you are about to run (bulk-insert, select-after-mutation, schema introspection, etc).
references:
  - name: multi_statement
    description: How multi-statement queries work — atomicity, output semantics, LIMIT, ordering. Read before sending more than one ; in a query.
  - name: bulk_insert
    description: Inline VALUES patterns, when to split into multiple INSERTs, when to ask the operator for sql_bulk_insert_from_attachment.
  - name: select_after_mutation
    description: Pattern for INSERT/UPDATE/DELETE followed by SELECT to confirm changes. Includes RETURNING vs trailing SELECT tradeoffs.
  - name: anti_patterns
    description: BEGIN/COMMIT, bind params ($1/?/:name), TRUNCATE, DROP, CREATE INDEX — what NOT to write and why. Includes example errors and fixes.
  - name: schema_discovery
    description: Using information_schema queries to discover columns, types, constraints. Useful before writing INSERTs against unfamiliar tables.
  - name: error_recovery
    description: Common error messages and what they mean. "cannot insert multiple commands" = multi-statement issue. "syntax error at or near" = quote/escape issue. Etc.
---

# sql_query — Best practices and patterns

Cargá la reference que corresponde al patrón que vas a usar. Si la pregunta
del usuario es genérica ("query la base"), empezá por `schema_discovery`
para ver qué tenés disponible.

## Quick rules (always)

1. **Una sola transacción atómica por call** — todos los statements en la
   query corren en la misma TX. Cualquier fallo → rollback completo.
2. **Output = último statement** — SELECTs anteriores se ejecutan pero su
   resultado se descarta.
3. **Sin BEGIN/COMMIT manuales** — la TX se abre y cierra automáticamente.
4. **Sin bind params** — pegá los valores literales escapando apóstrofes
   con `''`.
5. **WHERE obligatorio en DELETE/UPDATE**.
6. **Schema allowlist** — solo podés tocar schemas listados en tu config.
   La descripción del tool al inicio del turn te dice cuáles son.

Si tu intent no está cubierto por las references, tratá de mapear al más
cercano. Si no encontrás nada, escribí la query usando solo las
"Quick rules" de arriba.
