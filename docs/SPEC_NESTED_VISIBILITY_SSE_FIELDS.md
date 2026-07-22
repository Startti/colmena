# SPEC — Visibilidad total anidada + campos `level`/`path` en el SSE

**Fecha:** 2026-07-05
**Autor:** investigación cruzada ADP ↔ colmena `dag_engine`
**Estado:** implementado en colmena (PR #146, `sse_mapper.rs` / `run_use_case.rs`) — contrato disponible para que ADP integre los campos nuevos
**Repos:** productor = `Startti/colmena` (`dag_engine`); consumidor = ADP (`apps/api` relay + `packages/shared` reducer/UI)

---

## 0. TL;DR para ADP

Colmena emite **más eventos** (los niveles anidados 2, 3, 4… del árbol de sub-agentes que hoy son invisibles) y a **agregar dos campos nuevos a TODOS los frames de subgraph y al heartbeat**:

- `level: number` — profundidad de anidamiento del nodo que emitió el evento. `0` = agente principal, `1` = primer subgrafo/rol embebido, `2` = orchestrator dentro del rol, `3` = sub-agente hijo, etc.
- `path: string` — linaje legible del nodo, p.ej. `"top>Builder>orch>calc_a"`.

**Los `type` existentes NO cambian** (`subgraph-text-delta`, `subgraph-tool-input-available`, `status`, etc. siguen igual). Solo se **agregan** `level` y `path`. Cambio 100% aditivo: si ADP ignora los campos nuevos, todo sigue funcionando como hoy. Si los usa, puede renderizar el árbol anidado (indentación / breadcrumbs / "colapsar por rol").

---

## 1. Por qué

Hoy, cuando el agente principal invoca un rol embebido (subgraph-as-tool) que a su vez corre un orchestrator con sub-agentes, **el trabajo de los niveles ≥2 es invisible en el stream**: el usuario ve un hueco largo (medido: 22 s mudos en una réplica; en el creador real supera 60 s → *falso* `Stream timeout`). Dos causas, mismo origen:

1. **Drop de eventos profundos.** El engine re-envuelve cada evento hijo en `SubgraphWrapped` (`run_use_case.rs`), pero el mapper solo desenvuelve **un** nivel (`sse_mapper.rs`); un `SubgraphWrapped` anidado (nivel ≥2) cae en `_ => None` y **se descarta**.
2. **Eventos de borde a `None`.** `LlmMessageStart/Finish`, `LlmUsage`, `TurnStart` mapean a `None` → invisibles y no resetean el watchdog de 60s del API, pero sí suprimen el heartbeat.

El fix del lado colmena hace visible todo nivel; ADP solo necesita saber leer los campos nuevos.

---

## 2. Campos nuevos (contrato)

Se agregan a **cada frame `subgraph-*`** y al frame de heartbeat `status`:

| Campo | Tipo | Semántica |
|---|---|---|
| `level` | `number` (entero ≥0) | Profundidad de anidamiento. `0` = agente principal (nivel del stream actual), incrementa con cada subgrafo/rol embebido. |
| `path` | `string` | Linaje `padre>...>nodo` separado por `>`. Estable dentro de un run. Útil para agrupar/colapsar por rama. |

Notas:
- Los frames del **nivel 0** (agente principal — `text-delta`, `tool-input-*`, etc., SIN prefijo `subgraph-`) llevarán `level: 0` y `path` = id del nodo raíz. (Aditivo; hoy no traen estos campos.)
- `node_id` y `node_label` **se mantienen** (no cambian). `path` es complementario: `node_id` identifica el nodo; `path` dice de qué rama del árbol viene.
- El heartbeat `{"type":"status","stage":"running",...}` también llevará `level`/`path` del nodo que está activo → el usuario ve *quién* está trabajando durante un silencio genuino, no un ping anónimo.

---

## 3. Eventos que ADP empezará a recibir (antes invisibles)

Con la visibilidad total, por cada sub-agente de cualquier nivel llegarán, **con el mismo esquema que hoy usa el nivel 1**, ahora también para niveles 2/3/4…:

- `subgraph-text-start` / `subgraph-text-delta` / `subgraph-text-end` — texto del sub-agente (streaming).
- `subgraph-reasoning-start` / `subgraph-reasoning-delta` / `subgraph-reasoning-end` — razonamiento (cuando el modelo lo expone).
- `subgraph-tool-input-*` / `subgraph-tool-output-available` — tool-calls del sub-agente.
- `subgraph-node-start` / `subgraph-node-end` — fronteras (ahora también para subgraph-as-tool, no solo agentes de orchestrator).
- `status` (heartbeat) — con `level`/`path`.

**Volumen:** aumenta (antes se dropeaba lo profundo). El reducer de ADP ya ignora sin romper los `type` que no maneja (`default: return state`), así que no hay riesgo funcional; el trabajo de ADP es **opcional y de UI**: usar `level`/`path` para render anidado.

---

## 4. Impacto en ADP (qué integrar)

| Capa ADP | Hoy | Con el cambio |
|---|---|---|
| Relay backend `apps/api/.../chat.service.ts:1364` | reenvía cualquier frame con `type` | Sin cambios — pasa `level`/`path` intactos ✅ |
| Reducer `packages/shared/.../colmena-events.reducer.ts` | reduce `subgraph-*`; ignora `status` | **Opcional:** leer `level`/`path` para construir/renderizar el árbol anidado. Si no se toca, funciona igual. |
| Tipos `packages/shared/.../event-tree.ts` | union `SseEvent` | **Recomendado:** agregar `level?: number` y `path?: string` a los frames `subgraph-*` y `status` (higiene de tipos). |
| UI | muestra nivel 1 plano | **Opcional:** indentar/breadcrumb por `level`/`path`; colapsar por rama. |

Contrato invariante: `type`, `node_id`, `node_label`, `id`, `toolCallId`, payloads — **idénticos**. Solo se suman `level` y `path`, y llegan más eventos (niveles profundos).

---

## 5. Ejemplo de frames (antes → después)

**Antes** (nivel 3 se dropeaba):
```
data: {"type":"subgraph-text-delta","id":"txt_x","delta":"...","node_id":"orch"}
(nada de calc_a / write_a / sus tools)
```

**Después:**
```
data: {"type":"subgraph-node-start","node_id":"orch","node_type":"orchestrator","level":1,"path":"top>Builder>orch"}
data: {"type":"subgraph-text-delta","id":"txt_a","delta":"planning...","node_id":"planner","level":2,"path":"top>Builder>orch>planner"}
data: {"type":"subgraph-tool-input-available","toolCallId":"c1","toolName":"calc","input":{...},"node_id":"calc_a","level":3,"path":"top>Builder>orch>calc_a"}
data: {"type":"subgraph-text-delta","id":"txt_b","delta":"96","node_id":"calc_a","level":3,"path":"top>Builder>orch>calc_a"}
data: {"type":"status","stage":"running","node_id":"critic","idleSecs":20,"level":2,"path":"top>Builder>orch>critic"}
```

---

## 6. Compatibilidad / rollout

- **Aditivo:** ADP puede desplegarse antes o después de colmena; los campos nuevos son opcionales. No hay breaking change.
- **Sin coordinación de deploy obligatoria:** colmena empieza a emitir → ADP viejo ignora `level`/`path` y renderiza como hoy (plano). ADP nuevo los aprovecha.
- **Higiene de tipos (no bloqueante):** agregar `level?`/`path?` al union `SseEvent`.

---

## 7. Referencias

- Plan de implementación colmena: `Startti/colmena` → `docs/superpowers/plans/2026-07-05-nested-visibility-liveness.md`
- Drop de niveles profundos: `sse_mapper.rs` (bloque `SubgraphWrapped`, catch-all `_ => None`)
- Re-wrap por nivel: `run_use_case.rs` (`yield SubgraphWrapped { inner }`)
- `path_prefix` (fuente del `path`): `run_use_case.rs`
