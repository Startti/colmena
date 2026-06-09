# `docs/proposals/`

Propuestas de diseño que aún no son spec formal ni plan ejecutable.

Aquí viven **ideas trabajadas** (con suficiente detalle como para ser
revisadas), pero todavía pendientes de aprobación final del owner para
pasar a `docs/superpowers/specs/` (spec formal) y luego
`docs/superpowers/plans/` (plan de implementación).

## Ciclo de vida

1. **`docs/proposals/`** — idea descrita con lógica, trade-offs, edge
   cases. Lo suficientemente concreta para que el owner diga "sí, así".
2. **`docs/superpowers/specs/`** — spec formal del diseño aprobado
   (contratos, schemas, errores, tests).
3. **`docs/superpowers/plans/`** — plan de implementación paso a paso
   (tareas, archivos, orden, criterios de done).
4. **`docs/developer_guide/`** — guía de usuario una vez shipped.

Cuando una propuesta avanza a spec, dejar un puntero en este README
("→ promovido a spec X") en vez de borrarla — sirve de historia.

## Naming

`<YYYY-MM-DD>-<slug>.md` — misma convención que specs/plans.

## Índice actual

- [`2026-06-09-gdocs-oauth-user-flow.md`](2026-06-09-gdocs-oauth-user-flow.md) —
  OAuth user-scoped para Google Docs (v1.1 item 1). Permite al agente actuar
  AS el usuario en vez de como Service Account: docs creados quedan con
  owner=usuario, acceso natural a su Drive, soporta Gmail personal.
