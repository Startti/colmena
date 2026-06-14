# gsheets: inspect-before-python — instrucciones LLM-facing — Design

- **Fecha:** 2026-06-14
- **Estado:** Aprobado, implementación inline
- **Alcance:** texto LLM-facing de gsheets (descripción de `gsheets_run_python` + Google Workspace prelude).
- **Relación:** follow-up de [`2026-06-14-gsheets-expand-merges-design.md`](2026-06-14-gsheets-expand-merges-design.md). NO toca código de lectura ni el feature de expand-merges.

## Problema

Un agente con `enabled_tools: ["gsheets"]` y un pedido vago ("subí 10 al monto de
todas las frutas") va **directo a `gsheets_run_python`** sin leer la tabla primero,
y **adivina la semántica** de los datos. Caso real verificado (2026-06-14): el
agente asumió que "frutas" era parte del nombre del producto
(`df['Producto'].str.contains('fruta')`) en vez de filtrar por la columna
`Categoria`. Resultado: **0 filas matcheadas, 0 cambios, reportado como éxito** —
un fallo semántico **silencioso** (el código corrió limpio, sin KeyError).

### Causa raíz: contradicción entre capas always-on

Dos textos que el agente siempre ve se contradicen:
- **Google Workspace prelude** (system message): *"leéla primero con `gsheets_read`"*.
- **Descripción de `gsheets_run_python`**: *"load each table as a binding and
  compare in code"* → implica que run_python es **autosuficiente** (se carga la
  tabla solo, no hace falta leer).

El agente le hizo caso a la descripción de la tool (más específica, más cercana a
la decisión). Peor: el prelude **también** dice "cargá cada tabla como un binding",
así que la contradicción vive incluso dentro del prelude.

### Por qué texto y no un guard estructural

El código del agente no tiró error (usó columnas reales, corrió limpio, matcheó 0
filas). No hay forma robusta de detectar por código que el agente *entendió mal*
los datos (0 filas a veces es legítimo). El único defensa real es que **entienda
los datos antes de operar** → el lever correcto es **texto**, no estructura.

## Diseño

Dos instrucciones complementarias — **preventiva + detective** — en la capa de
mayor leverage (la descripción de la tool, always-on, per-tool, visible al
componer el código), más una alineación del prelude para que las dos capas
always-on dejen de contradecirse.

### 1. Preventiva — descripción de `gsheets_run_python` (`gsheets.yaml`)

Reemplazar la apertura de la descripción para: (a) matar la implicación de
autosuficiencia ("binding carga las FILAS, no el esquema"), (b) imperativo
condicional ("si no conocés las columnas y qué significan, leé primero con
`gsheets_read`"), (c) anti-patrón concreto ("frutas" → columna Categoria), (d) el
why (resultados mal en silencio). Condicional a propósito: si ya leíste la tabla
este turno o conocés las columnas, no hace falta releer.

### 2. Detective — misma descripción

Agregar: *"Si tu filtro/groupby matchea 0 filas (o muchas menos de las esperadas),
NO reportes éxito — es señal de que malinterpretaste los datos; inspeccioná la
tabla y reconsiderá."* Es la red de seguridad que cacha **este** fallo aunque el
agente saltee el read (en el caso real, el agente calculó `mask.sum() == 0` y aun
así reportó éxito).

### 3. Alineación — Google Workspace prelude (`google_workspace_prelude.rs`)

Ajustar `SHEET_WORKFLOW_PRELUDE` para que el "read first" aplique **explícitamente
aunque el objetivo sea `gsheets_run_python`** (hoy la frase "cargá cada tabla como
un binding" lo contradice), y sumar la nota detective de 0 filas. Así las dos
capas always-on quedan consistentes.

### Fuera de alcance

- **Prelude del sandbox** (`gsheets_run_python_prelude.md`) — leverage bajo: se
  envuelve en ejecución, el agente no lo ve al componer el código.
- **Hacer always-on el skill `gsheets-table-exploration`** — pesado, infla cada
  turno; sigue load-on-demand.
- Cambios de código / comportamiento de tools — esto es solo texto.

## Verificación (obligatoria — memoria `feedback_realistic_prompts`)

Re-correr el **prompt vago** que falló (run 2: "subí 10 al monto de todas las
frutas y guardá", sin dictar código) contra un sheet real con merges, y verificar
leyendo el sheet directo de la API que:
- el agente ahora **lee la tabla primero** (o frena ante 0 filas), y
- aplica el +10 a las **3 frutas correctas** (Manzana/Banana/Pera → vía Categoria).

Honestidad sobre el límite: esto reduce el fallo, no lo elimina al 100% en un
modelo barato (flash). Si flash igual falla tras el cambio, se documenta y se
evalúa algo estructural en un follow-up — pero el texto es el primer movimiento
correcto y el de mayor leverage.

## Impacto / compat

Solo texto LLM-facing. Sin cambios de API, sin impacto en ADP. Cambia lo que el
modelo lee (mejor guía), nada más.
