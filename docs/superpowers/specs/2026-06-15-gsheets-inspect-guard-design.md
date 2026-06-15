# gsheets: inspect-before-python guard (estructural) — Design

- **Fecha:** 2026-06-15
- **Estado:** Aprobado, pendiente de plan de implementación
- **Alcance:** `DagToolExecutor` (router de tools gsheets) + dispatcher de `gsheets_run_python`.
- **Relación:** follow-up estructural de [`2026-06-14-gsheets-inspect-before-python-design.md`](2026-06-14-gsheets-inspect-before-python-design.md). El fix de **texto** (PR #98) hace que un modelo capaz lea-primero; este guard lo **fuerza** estructuralmente para que incluso un modelo barato (gemini-2.5-flash) no pueda correr código a ciegas.

## Problema

Verificado el 2026-06-14 contra Google real: con un pedido vago ("subí 10 al monto
de todas las frutas"), gemini-2.5-flash va **directo a `gsheets_run_python`** sin
leer la tabla, y **adivina la semántica** (filtra `Producto` por nombre en vez de
la columna `Categoria`). El código corre limpio, matchea 0 filas, y antes del fix
de texto incluso lo reportaba como éxito. El fix de texto mejoró el peor caso pero
**no** logra que flash lea-primero (techo de capacidad). gemini-2.5-pro **sí**
sigue las instrucciones de texto y lo hace bien.

Objetivo: un guard **estructural** que garantice que el agente vea la estructura
real de la tabla antes de que su código se ejecute — independiente de la
capacidad del modelo.

## Idea (del owner)

Antes de ejecutar `gsheets_run_python`, verificar si la hoja ya fue leída en este
turno. Si **no**, devolver un preview de la tabla (no ejecutar el código) y obligar
al agente a re-llamar con código informado. Solo interceptar si no hubo lectura
previa — el read sucede igual dentro de run_python, así que el costo extra es casi
nulo.

## Decisiones de diseño (cerradas en brainstorm)

| Decisión | Elección | Por qué |
|---|---|---|
| **Scope del read-state** | **Per-turno** | El `DagToolExecutor` se crea por ejecución de `llm_call` (= un turno), así que un `HashSet` en él vive exactamente un turno y se descarta solo. Consistente con la filosofía **no-cache** de expand-merges (cada turno re-confirma fresco; otros pudieron editar entre turnos). Cero infraestructura de estado persistente, sin riesgo de staleness. |
| **Qué devuelve el intercept** | **Preview acotado en markdown — header + 5 filas de datos (6 en total)** | Devolver la hoja completa rompería la propiedad central de `gsheets_run_python` (las filas NO inundan el contexto del LLM). 5 filas alcanzan para aprender columnas + semántica (incl. header que no esté en la fila 1). El agente carga las filas completas al re-llamar, donde quedan fuera del LLM. Markdown porque es lo más fácil de leer para el modelo. |
| **Forma del resultado** | **Envelope de éxito (sin `"error"`)** con `status: "inspect_first"` | Reusa el patrón de envelope estructurado existente (`SheetExists`). Sin clave `error` ⇒ el agente lo trata como informativo y re-llama, no como fallo. |
| **Dónde vive** | `DagToolExecutor` (`dag_tool_executor.rs`) | El dispatcher de run_python es stateless (solo ve `args`). El executor ya rutea las tools gsheets, persiste durante el loop ReAct, y ve tanto `gsheets_read` como `gsheets_run_python`. Mantiene el guard fuera del agent loop genérico (provider-agnostic). |

## Arquitectura

`DagToolExecutor` gana un campo per-turno:

```rust
/// Hojas ya leídas en este turno — clave "spreadsheet_id::sheet".
/// Per-turno: el executor se crea por ejecución de llm_call; el set se
/// descarta al terminar el turno (consistente con no-cache).
gsheets_seen_sheets: std::sync::Mutex<std::collections::HashSet<String>>,
```

Flujo en el router de tools (`dag_tool_executor.rs`):

- **`gsheets_read` (éxito):** insertar `(spreadsheet_id, sheet)` en el set.
- **`gsheets_run_python`:** antes de `dispatch_gsheets_run_python(args)`:
  1. Parsear las bindings de `args`.
  2. Bindings **inline** (`data:`) se ignoran (no necesitan read).
  3. Para cada binding de **hoja**, ver si `(spreadsheet_id, sheet)` está en el set.
  4. Si **todas** están vistas → ejecutar normal (comportamiento actual, sin cambios).
  5. Si **alguna** no fue vista → **short-circuit**: para cada hoja no-vista, leer un
     preview acotado, marcarla como vista, y devolver el envelope `inspect_first`
     **sin ejecutar el código**.

No hay loop: el intercept marca las hojas, así que la segunda llamada del agente
siempre ejecuta.

### Clave del read-state

`(spreadsheet_id, sheet)` — se ignora el `range`. Una vez que cualquier read de esa
hoja ocurrió, se considera "vista" (el objetivo es que el agente conozca las
columnas, no un range exacto).

## El envelope `inspect_first`

```json
{
  "status": "inspect_first",
  "inspected_sheets": {
    "v": {
      "spreadsheet_id": "1W7m...",
      "sheet": "Ventas",
      "columns": ["Categoria", "Producto", "Monto"],
      "preview_markdown": "| Categoria | Producto | Monto |\n| --- | --- | --- |\n| Frutas | Manzana | 100 |\n| Frutas | Banana | 200 |\n| Frutas | Pera | 50 |\n| Verduras | Lechuga | 30 |\n| Verduras | Tomate | 70 |"
    }
  },
  "advice": "Antes de correr código sobre una hoja hay que conocer sus columnas reales. Acá está el preview (primeras 6 filas). Volvé a llamar gsheets_run_python con el MISMO código, corregido si hace falta para usar estas columnas/valores reales (p.ej. filtrar por la columna correcta, no adivinar nombres).",
  "next_action": "re-call gsheets_run_python"
}
```

- **`preview_markdown`** es la tabla principal (renderizada con la misma lógica que
  `gsheets_read`, reusando `values_to_markdown`) — markdown para máxima legibilidad
  del modelo.
- **`columns`** es conveniencia explícita (la primera fila del preview).
- Sin clave `"error"` ⇒ resultado de éxito.

## Bordes

- **Binding con `range`:** el preview lee las primeras 6 filas (header + 5 de datos) de ese range
  (matchea lo que run_python cargaría).
- **Read del preview falla** (hoja inexistente / permiso): surface ese error
  (run_python habría fallado igual) — en ese caso el resultado SÍ lleva `error`.
- **Multi-binding mixto** (algunas vistas, otras no): preview solo de las no-vistas;
  igual se short-circuitea (el agente re-llama y ya están todas marcadas).
- **Inline-only** (sin bindings de hoja): nunca intercepta; ejecuta normal.

## Fuera de alcance (YAGNI)

- **`gsheets_set_range` / `gsheets_set_cell`** (escritura directa) — el fallo
  semántico "adiviné la columna" es propio de run_python (código sobre datos
  cargados); set_* escribe un bloque literal con direcciones explícitas. Posible
  extensión futura.
- **Forzar código correcto** — el guard garantiza que el agente VEA la tabla, no que
  use las columnas bien. Combinado con la regla detective de texto (0 filas → pará,
  PR #98) la red es fuerte, pero no es garantía del 100%.
- **Persistencia cross-turno** del read-state — descartada (ver decisiones).

## Testing

- **Unit** (`dag_tool_executor`): (a) `gsheets_read` marca la hoja; (b) run_python
  sin read previo → envelope `inspect_first`, código NO ejecutado; (c) run_python
  con read previo → ejecuta normal; (d) el intercept marca → segunda llamada
  ejecuta; (e) inline-only → nunca intercepta; (f) multi-binding mixto → preview de
  las no-vistas; (g) preview en markdown bien formado.
- **E2E real (obligatorio):** el prompt vago que falló con flash, contra el binario
  con guard → flash recibe el preview en el primer `gsheets_run_python`, re-llama,
  y aplica el +10 a las 3 frutas correctas (Manzana/Banana/Pera). Verificar leyendo
  el sheet directo de la API. Es la prueba de que el guard hace seguro a flash.

## Compat / ADP

- **Cambio de comportamiento** de `gsheets_run_python`: el primer uso ciego sobre una
  hoja devuelve el envelope `inspect_first` en vez de ejecutar. Aditivo desde la API
  Rust (firma de dispatch igual; el envelope es un nuevo `serde_json::Value` que el
  agente maneja en-loop).
- **No cruza el borde SSE** de forma que requiera cambios en ADP (es un tool result
  más que el agente consume y al que responde re-llamando). Sweep del worker igual
  antes de push.
