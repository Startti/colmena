# Resumen semántico de conversación con conciencia de rol y recall lossless — Design

- **Fecha:** 2026-06-18
- **Estado:** Aprobado en brainstorm, pendiente de plan de implementación
- **Alcance:** memoria conversacional del nodo `llm_call` — la capa de compactación de historial en `src/libs/colmena/src/llm/application/agent_service.rs` (`compact_history_to_summary` / `summary_line_for_message`), el repositorio de conversación (`postgres_conversation_repository.rs`, `sqlite_conversation_repository.rs`), y la tool `recall_history`.
- **No afecta:** `dag_phase_summaries` del orquestador, `dag_task_memory`, ni el flujo de attachments (salvo la centralización opcional de modelos baratos).

## Problema

Hoy, cuando la historia de un `llm_call` supera ~8 mensajes, `compact_history_to_summary`
(F-T15) reemplaza la zona del medio por **un** mensaje `system` donde cada mensaje
original se reduce a **una línea truncada a 180 caracteres** (`COMPACT_SUMMARY_LINE_MAX_CHARS`),
y el texto/args del assistant a 90. Fallas concretas, varias verificadas el 2026-06-18
(ejercicio con un agente de cálculo, 17 mensajes en DB):

1. **Corta por posición, no por relevancia** (se queda con los primeros 180 bytes).
2. **No sintetiza** entre mensajes.
3. **Se recomputa en cada iteración** del loop ReAct (medido: el `system` de resumen
   creció 865 → 956 → 1047 → 1134 chars en iteraciones sucesivas de un mismo run).
4. **180 es arbitrario y mide la unidad equivocada.**
5. **Redundancia** (10 mensajes → 10 líneas casi iguales).

La persistencia **no** trunca: `add_message` guarda `content` completo (verificado: prompt
de 360 chars → 360 en `llm_node_history`). El problema es **solo** lo que se le envía al LLM.

### Por qué los ejemplos reales cambian el diseño

Una regla plana "resumir todo lo ≥250" es **demasiado burda**. Dos arquetipos lo exponen:

- **Creador de agentes (artefactos en uso activo):** emite un graph JSON / system prompt
  grande en un mensaje, y el usuario **itera** sobre él ("cambiá el modelo"). Si ese
  artefacto envejece y se resume a ~250 chars, el agente **pierde el JSON** justo cuando
  lo edita. Peor: `recall_history` tiene tope de **10 KB** → un artefacto grande **no se
  puede recuperar**. Es contenido **"referencia en uso"**, no historia vieja.
- **Agentes con herramientas (ReAct):** los `assistant` con `tool_calls` tienen `content`
  vacío y la info en `name`+`arguments` (estructurado) — no se "resumen" en prosa. Los
  `tool` results pueden ser triviales ("96") o enormes (50 KB JSON / 500 filas) donde un
  resumen NL puede tirar el dato exacto que se necesita después.

## Principio rector

> **Un resumen con pérdida es seguro si — y solo si — lo perdido es 100% recuperable.**

Por eso la pieza clave del diseño no es el resumidor, sino hacer **`recall_history`
lossless**. Con recuperación total garantizada, resumir agresivamente se vuelve seguro
para todo (incluidos artefactos): nada se pierde de verdad.

## Objetivo

Reemplazar el truncado por caracteres por una **política de compactación con conciencia
de rol**: resumen semántico **por mensaje** (~250 chars) para contenido de lenguaje
natural, tratamiento **estructural** para tool_calls, todo calculado **una sola vez por
mensaje** y **guardado en DB**, con **recuperación verbatim sin pérdida**. **Nunca
guardar información truncada.**

### No-objetivos (v1)

- Síntesis narrativa cruzada entre mensajes (se descartó a favor de simplicidad +
  addressability per-línea).
- Plegado de historial **dentro** de un mismo run (mid-run); recientes van full.
- **Digest estructurado** de tool-results (esquema + N filas + valores clave) → **v1.1**.
  En v1 se resumen como NL — con pérdida **pero recuperable** vía recall lossless.
- Búsqueda semántica / embeddings → **enhancement futuro** (`recall_search`, Parte B).

## Decisiones de diseño (cerradas en brainstorm)

| Decisión | Elección | Por qué |
|---|---|---|
| **Recall lossless** | **Pieza clave de v1.** `recall_history` paginado, sin tope efectivo | Hace seguro todo lo demás: artefactos grandes (creador de agentes) recuperables verbatim. |
| **Forma del resumen** | **Per-mensaje** (1 línea ~250 char), no un blob narrativo | Las reglas de 250 solo tienen sentido por mensaje; hace trivial el cache y la addressability `[Tn]`. |
| **Política por rol** | NL → resumen; `tool_calls` → línea estructural; trigger sobre forma renderizada | Resumir un tool_call en prosa es incorrecto/costoso; medir solo `content` ignora los args. |
| **Conciencia de andamiaje** | discovery tools (`describe_tool`/`load_skill`/`load_attachment`) → markers; fuera de la ventana reciente; sin resumen | Es plumbing efímero del lazy loading/skills, no contenido del user ni del LLM. |
| **Regla de skip** | `rendered_size < 250` → **verbatim, sin LLM** | La mayoría de mensajes cortos ya son "tamaño resumen". |
| **Tamaño del resumen** | Pedido **~250 chars por prompt**, sin hard-cut | Se limita por instrucción, no se recorta la salida. |
| **Recovery-aware** | Para mensajes grandes/artefactos, la línea cita "completo en `recall_history(turn=N)`" | El agente sabe que puede traerlo de vuelta para editar. |
| **Cuándo se calcula** | **Lazy al cargar el run** (Hook C); nunca por iteración | Solo cuando hace falta; vive donde se ensambla el contexto. |
| **Dentro del run** | Recientes **full**, sin plegado mid-run (v1) | Cache-friendly (el prefijo `system` no cambia durante el run). |
| **Dónde se guarda** | Columna aditiva **`summary TEXT NULL`** en `llm_node_history` | El cache *es* el estado; `[Tn]` = ordinal de la fila. |
| **Modelo summarizer** | Tier barato desde **`cheap_models.yaml`** (+ env + override por nodo) | Una sola fuente de verdad editable; sin segunda API key por defecto. |

## Política de compactación por rol/tipo

Al construir la línea de cada mensaje de la zona vieja, según rol:

| Rol / tipo | Tratamiento |
|---|---|
| `user` (texto NL) | `<250` → verbatim; si no → resumen semántico ~250. |
| `assistant` (texto NL) | `<250` → verbatim; si no → resumen semántico ~250. |
| `assistant` con `tool_calls` | **Línea estructural**: `llamó <name>(<args>)`. Nunca al summarizer. Si los `args` son enormes, se citan recuperables por recall (no se resumen en prosa). |
| `tool` result NL/grande | `<250` → verbatim; si no → resumen semántico ~250. |
| `tool` result **estructurado** (JSON/filas) | **v1.1 (shipped):** digest determinista (esquema + N filas + muestra + min/max), sin LLM. NL cae a resumen semántico. |
| `system` | En `keep_first` / merge target; no se resume. |

El **trigger de tamaño** se mide sobre la **forma renderizada** del mensaje (content +
tool_calls/args serializados), no solo `content`.

## Conciencia de andamiaje (lazy tools + skills)

Con lazy tool loading y skills por referencia, gran parte de la conversación son
round-trips de **descubrimiento** (`describe_tool` para entender una tool, `load_skill`
para leer una referencia). Son **andamiaje efímero**: una vez que el agente usó la tool,
el `describe_tool` no aporta nada. Sin tratamiento especial, ocupan la ventana reciente
(desplazando contenido real a la zona resumida) y hasta recibirían un resumen semántico
absurdo de un schema.

**Clase de valor por mensaje:**

| Clase | Qué incluye | Tratamiento |
|---|---|---|
| `scaffolding` | round-trips de `describe_tool`, `load_skill`, `load_attachment` (el `assistant(tool_call)` + su `tool` result) | Colapsar a **marker siempre** ("tool/skill X visto antes; llamá `describe_tool`/`load_skill` de nuevo"). **Nunca** se resume semánticamente. **No** cuenta para la ventana reciente. Recuperable re-llamando la tool. |
| `content` | user/assistant texto, resultados de tools con datos | Sujeto a la política por rol + resumen + ventana reciente. |

Ya hay precedente: `compact_old_load_skill_in_history` colapsa `load_skill` viejos, y
`load_attachment` ya es marker por Plan B. **Se generaliza** a un paso
`compact_discovery_tools_in_history` que cubre `load_skill` + `describe_tool` (y mantiene
`load_attachment`), con una pequeña ventana `keep_recent` propia (que el agente vea el
schema que acaba de pedir) y marker para lo más viejo.

**Consecuencias en el resto del diseño:**
- La **ventana de recientes se computa sobre mensajes `content`** (por presupuesto de
  tokens), NO sobre el conteo crudo — así los "últimos full" son del intercambio real,
  no del discovery noise.
- `keep_first` ancla al **primer mensaje real del usuario** (el objetivo), no a un
  `describe_tool`/`load_skill` inicial.
- Los markers de andamiaje son chicos (`<250`) → cuando caen en la zona vieja van
  verbatim, sin gastar una llamada de resumen.

## Arquitectura

### 1. Recall lossless (pieza clave)

`recall_history` gana paginación para recuperar contenido de cualquier tamaño sin
inundar el contexto en una sola llamada:

- Args: `turn: N` (obligatorio) + `offset: usize` y `limit: usize` opcionales.
- Devuelve la franja `[offset, offset+limit)` del `content` verbatim + `total_chars` +
  `next_offset` (o `null` si no hay más).
- Default `limit` acotado (p.ej. ~8 KB) por llamada; el agente pagina hasta reconstruir
  el artefacto completo. Se **elimina** el truncado silencioso de 10 KB (que perdía datos
  sin recuperación).
- Lee del **orden crudo de DB** (`get_by_id`), ordinal estable por append-only.

### 2. Cambio de esquema — columna `summary`

Migración aditiva en Postgres y SQLite:

```sql
ALTER TABLE llm_node_history ADD COLUMN IF NOT EXISTS summary TEXT;
```

- `summary IS NULL` → aún no resumido (o `<250` → verbatim).
- `summary` poblado → resumen semántico cacheado, reusable para siempre (mensaje inmutable).

El port `ConversationRepository` gana capacidades aditivas (no rompe impls externos):
traer `summary` al leer, y `set_summary(key, turn_ordinal, summary)` para persistir.

> **Nota:** exponer `summary` vía un struct interno (`StoredMessage { message, summary }`)
> para no contaminar el value object de dominio `LlmMessage`.

### 3. Use case `SummarizeMessageUseCase` (application)

Resume **un** mensaje **NL** en aislamiento (no se invoca para tool_calls). Patrón de
`LlmAttachmentSummaryGenerator`:

- Llamada one-shot, sin historia, que **bypassa `LlmCallUseCase`** → no entra a `llm_node_history`.
- Prompt: "resumí en ~250 caracteres, una línea, conservando lo accionable (hechos,
  decisiones, resultados); sin markdown ni comentarios". **No** hard-corta la salida.
- Timeout por llamada + degradación elegante (§Degradación).
- Modelo desde la config de modelos baratos (§5).

### 4. Punto de integración — reemplazo de `compact_history_to_summary`

Al **cargar** (Hook C), sobre el **orden crudo de DB** (NO la lista post-shim; invariante #2):

0. **Colapsar andamiaje** (`compact_discovery_tools_in_history`): `describe_tool`/
   `load_skill`/`load_attachment` viejos → markers. No cuentan para la ventana reciente.
1. Ventana de **recientes** por presupuesto de **tokens** (est. `chars/4`), default
   ~2.500, **computada sobre mensajes `content`** (los markers de andamiaje no consumen
   presupuesto), alineada a límites de turno (no partir `assistant(tool_calls)+tool`) → borde `B`.
2. Zona vieja `[keep_first..B)`: por mensaje, aplicar la **política por rol** (arriba).
   Para los que requieren resumen NL: usar `summary` cacheado; si vacío →
   `SummarizeMessageUseCase` → **persistir en la columna** → usar.
3. `keep_first` + recientes `[B..N]` → **full**.
4. Mantener `COMPACT_SUMMARY_MAX_LINES`, tag `[Tn]`, merge-en-`system` (evita `system`
   consecutivos en Gemini) y guard de pares tool.

Se conserva la estructura de `compact_history_to_summary`; cambia la **generación de
cada línea** (truncado → política por rol con cache). Cambio chico, de bajo riesgo.

### 5. Config de modelos baratos

Archivo único, versionado, embebido con `include_str!` (patrón de `text/`):

```yaml
# src/libs/colmena/text/config/cheap_models.yaml
google:    gemini-2.5-flash
openai:    gpt-4o-mini
anthropic: claude-haiku-4-5-20251001
```

**Cadena de resolución** (env → archivo → default), igual que otros patrones del repo:
1. `summary_model` en el config del nodo `llm_call` (opcional).
2. Env `COLMENA_CHEAP_MODEL_<PROVIDER>` (runtime, sin rebuild).
3. `cheap_models.yaml` (default versionado).

Provider por defecto = el del nodo (reusa su `api_key`). El summarizer de attachments
puede adoptar este archivo más adelante (fuera de scope v1). Regla de proyecto:
`gemini-2.5-flash`, nunca `gemini-1.5-flash`.

## Flujo de datos (al cargar un run)

```
get_by_id (orden crudo DB: mensajes + columna summary)
   │
   ├─ colapsar andamiaje (describe_tool/load_skill/load_attachment viejos → markers)
   │
   ├─ ventana recientes por tokens, SOLO sobre mensajes content → borde B
   │
   ├─ zona vieja [keep_first..B):  por rol/clase →
   │      scaffolding         → marker (sin LLM)
   │      tool_calls          → línea estructural (sin LLM)
   │      NL <250             → verbatim
   │      NL ≥250             → summary cacheado ?? summarize(cheap) + persistir
   │      grande/artefacto    → línea + "completo en recall_history(turn=N)"
   │
   └─ contexto = keep_first(full) + [system: líneas [Tn]] + recientes[B..N](full)
                 + sufijo temporal volátil (sin cambios)
```

> **Amendment (2026-08-22):** cuando el mensaje más nuevo (`N-1`) por sí solo excede el
> presupuesto de recientes, "recientes[B..N](full)" degenera a exactamente ese único mensaje y
> viaja **verbatim**, sin importar el rol — no hay truncamiento ni transformación de contenido, lo
> que se acota es el índice `B` del borde (`recent_boundary_by_tokens`), nunca el tamaño del
> mensaje. Detalle en
> [`docs/developer_guide/15_memory_guide.md`](../../developer_guide/15_memory_guide.md)
> §Compactación → "Ventana de recientes cuando el mensaje más nuevo excede el presupuesto" y
> [`docs/CHANGELOG_2026-08.md`](../../CHANGELOG_2026-08.md) §3.

Durante el loop: nada (recientes crecen full; el bloque de resumen no cambia →
cache-friendly). Los mensajes nuevos se guardan full; se resumen en un load futuro.

## Recuperación e invariantes

- `recall_history` lossless (paginado) — ver §Arquitectura 1.
- El resumen **cita `[Tn]`** para lo recuperable; los artefactos lo dicen explícitamente.

### Invariantes críticos

- **#1 — Alineación de índices.** `[Tn]` = ordinal absoluto en DB (`created_at`), el mismo
  que indexa `recall_history`. El código numera sobre el orden crudo de DB.
- **#2 — Shim temporal que dropea mensajes.** `agent_service` (load) hace `filter_map` que
  puede eliminar un `system` solo-temporal, desalineando la lista en memoria del ordinal
  de DB. El cálculo de líneas/`[Tn]` **debe** hacerse sobre el orden crudo de DB.
- **#4 — Migración de conversaciones largas existentes.** Sin `summary` poblado, el primer
  load resume muchos mensajes. Ver §Migración.

## Migración / backlog sin resumir

Dos escenarios distintos:

- **Día a día (crecimiento normal):** solo 1–3 mensajes envejecen por load → backlog
  diminuto → se resume a tiempo → **cero truncado** en estado estable.
- **Migración (una vez por conversación):** una conversación ya larga se carga con todo
  sin resumir. Estrategia:
  - Resumir el backlog en **paralelo con concurrencia acotada (~5) + timeout de lote
    (~15s/load)**; lo que no entra queda `NULL` y se reintenta el próximo load. (Reemplaza
    el "K mágico" por un límite auto-ajustable.)
  - Lo que aún no tiene resumen ese load va con **truncado runtime temporal** — recorte
    **solo en memoria, nunca almacenado**, **seguro porque es recuperable** vía recall
    lossless. Se auto-cura en 1–2 loads. Nunca peor que el comportamiento actual.

## Trade-offs documentados

- **#3 — Costo en el camino caliente.** Con Hook C, los mensajes NL ≥250 que recién
  envejecen se resumen al cargar. La regla de skip y el cache reducen la frecuencia, pero
  cuesta más cómputo que el truncado (gratis). Justificación: **calidad**, no costo.
  Palanca futura: Hook B/async.
- **#5 — Cache de prompt.** El bloque de resumen es estable **dentro** de un run; cambia
  entre runs cuando entra un mensaje a la zona vieja — pero **menos** que hoy (que recomputa
  por iteración).

## Degradación / errores

- Si `SummarizeMessageUseCase` falla/timeout: `summary` queda `NULL`; ese mensaje va con
  truncado runtime temporal (recuperable) y se reintenta el próximo load. El run nunca se cuelga.

## Backward compatibility

- Migración aditiva (`summary` nullable) en pg + sqlite; conversaciones existentes arrancan
  con `summary = NULL` y se resumen lazy (§Migración).
- `recall_history`: cambio **aditivo** de contrato (args `offset`/`limit` opcionales; el
  comportamiento por defecto recupera más, no menos). Se quita el truncado a 10 KB.
- Se elimina el truncado a 180 char; `KEEP_FIRST` y `MAX_LINES` se conservan;
  `LINE_MAX_CHARS` se reusa como umbral de skip (250).
- `compact_old_load_skill_in_history` se **generaliza** a `compact_discovery_tools_in_history`
  (cubre `describe_tool` + `load_skill`; `load_attachment` ya era marker por Plan B),
  conservando su `keep_recent` propio.
- ADP: sin cambios de API pública (columna aditiva + cambio de wire interno). Verificar que
  el worker no asuma el set de columnas anterior de `llm_node_history` (aditivo → seguro).

## Estrategia de testing

- **Unit:** política por rol (NL `<250` verbatim / `≥250` resumen / tool_calls estructural /
  trigger sobre forma renderizada); **andamiaje** (`describe_tool`/`load_skill` viejos →
  markers, no consumen la ventana reciente, no se resumen); ventana reciente computada
  solo sobre `content`; numeración `[Tn]` sobre orden crudo incl. caso shim-drop (#2);
  resolución de modelo (nodo > env > yaml); degradación → truncado-runtime; paginación
  de recall (offset/limit/next_offset).
- **Integración (`#[ignore]`, DB):** round-trip de columna `summary` en pg y sqlite; cache
  hit (segundo load no re-llama); recall lossless de un mensaje grande en varias páginas.
- **E2E real:** (a) conversación >8 msgs con un mensaje NL largo → el modelo recibe resumen
  semántico (no prefijo de 180); (b) **creador de agentes**: artefacto JSON grande envejece,
  se resume con cita, y `recall_history` paginado lo reconstruye verbatim para editar;
  (c) agente con tools → tool_calls quedan estructurales, tool results grandes recuperables.
  Guardar SSE en `/tmp/colmena_e2e/` y reportar.

## Enhancements futuros (fuera de v1)

- **Digest estructurado de tool-results (v1.1) — SHIPPED 2026-06-19:** para
  resultados de tools estructurados (JSON object / array-of-objects / scalar
  array), `tool_digest::digest_tool_result` produce un digest determinista
  (esquema + N filas + muestra + min/max) en vez de prosa NL. Sin LLM, sin
  cache, sin cambio de DB. Resultados NL caen al resumen semántico v1. Ver
  [`docs/superpowers/plans/2026-06-19-tool-result-structured-digest-v1-1.md`](../plans/2026-06-19-tool-result-structured-digest-v1-1.md).
- **Digest v1.2 — drill de identificadores (mapa nominal) — SHIPPED 2026-06-19:** el
  drill-down lista identidades de fila (`<type> "<name>"`) en vez de solo columnas, con
  presupuesto de profundidad de 1 hop. Ver CHANGELOG §41 y
  [`docs/superpowers/plans/2026-06-19-tool-digest-v1-2-identifier-drill.md`](../plans/2026-06-19-tool-digest-v1-2-identifier-drill.md).
- **Parte B — `recall_search(query)`:** búsqueda por keyword sobre el `content` full
  (ILIKE/tsvector en PG, FTS5 en SQLite; **sin embeddings**) para el punto ciego de lo no
  citado. Opcional `recall_range(from,to)`. Disparador: medir con Opción A bien citada.
- **Guía de uso (no-motor):** el creador de agentes debería **externalizar** artefactos
  grandes (store + id, como attachments) en vez de depender de la memoria de chat.

## Parámetros por defecto (ajustables)

| Parámetro | Default | Notas |
|---|---|---|
| Umbral de skip (verbatim) | `250` chars (forma renderizada) | Reusa `LINE_MAX_CHARS`. |
| Target del resumen (prompt) | `~250` chars | Soft, sin hard-cut. |
| Ventana de recientes | `~2.500` tokens (est. `chars/4`) | Alineada a límites de turno. |
| `KEEP_FIRST` | `2` | Objetivo original siempre preservado/citado. |
| `MAX_LINES` | `100` | Drop de las más viejas (recuperables). |
| Concurrencia de resumen | `5` | Migración en paralelo. |
| Timeout de lote por load | `~15s` | El resto queda `NULL`, reintento próximo load. |
| Timeout por llamada de resumen | `~10s` | Patrón de attachments. |
| `recall_history` limit/página | `~8 KB` | Paginable vía `offset`/`limit`. |
| Andamiaje `keep_recent` | `8` msgs | Reusa `COMPACT_LOAD_SKILL_KEEP_RECENT_MSGS`; ve el schema recién pedido, marker para lo viejo. |
