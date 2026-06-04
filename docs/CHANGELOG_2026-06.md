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
