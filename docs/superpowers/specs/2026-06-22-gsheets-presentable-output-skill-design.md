# Rich formatting by default — `gsheets-presentable-output` skill + tool nudge — Design

- **Fecha:** 2026-06-22
- **Estado:** Aprobado, listo para plan de implementación
- **Backlog:** "Formato rico por default + mini-skill de presentable output" (Subsystem E v1.1)
- **Subsistema:** E (Google Sheets) — complementa `gsheets_format_range` (§47)

## Problema

`gsheets_format_range` (§47) es completo — verificado live: moneda, bordes,
background, bold, alineación, anchos. Pero en el E2E del 2026-06-22 se observó
que **`gemini-2.5-flash` lo subutiliza con prompts abiertos** ("armá un reporte
profesional") → solo aplica alineación. Con ops explícitas aplica todo. El gap
es de **guía al modelo**, no del tool.

Objetivo: que los agentes produzcan **formato presentable por default** sin que
el usuario lo pida campo por campo.

**Insight de diseño:** las skills de Colmena son **load-on-demand** — el modelo
ve un catálogo (name + description) y decide llamar `load_skill`. Una skill
**por sí sola no logra "por default"** (el modelo elige si cargarla). La única
superficie *siempre en el prompt* para un tool sintético es su **`description`**.
Por eso el diseño tiene **dos prongs**.

## Decisiones de diseño

| Punto | Decisión | Razón |
|---|---|---|
| Prongs | **(1)** nudge always-on en la `description` de `gsheets_format_range` + **(2)** skill deep `gsheets-presentable-output` | El nudge mueve el default real (siempre en prompt); la skill da la receta profunda on-demand |
| Auto-enroll de la skill | Cuando `gsheets_format_range` está en el catálogo del agente (toolkit `gsheets`, `*`, o nombre explícito; excluida si `!gsheets_format_range`) | Espeja el gate probado de `gdocs-surgical-edits`; matchea los casos reales de "armar hoja presentable"; cero ruido para read-only o agentes sin el tool |
| Alcance | **Solo gsheets** | El formato de tablas gdocs no existe aún (item separado en Subsystem G v1.1) |
| Cambios de código del tool | **Ninguno** | `gsheets_format_range` ya es completo; esto es solo guía (text + skill + gate) |

## Arquitectura

### Componente 1 — nudge always-on (`src/libs/colmena/text/tools/gsheets.yaml`)

Extender la `description` existente de `gsheets_format_range` con un bloque
"PRESENTABLE OUTPUT por default":
- Directiva: cuando generes una hoja para que la vea una persona, aplicá
  formato presentable **en una sola llamada multi-op**, sin esperar a que te lo
  pidan: encabezado (negrita + fondo + texto contrastante + centrado), formato
  de número apropiado en columnas numéricas (moneda `$#,##0`, `%`, fecha,
  miles), bordes en la tabla, fila de totales destacada (negrita + fondo +
  borde superior), y anchos de columna razonables.
- Un **ejemplo `ops` compacto** embebido (header + moneda + fila de totales)
  que el modelo puede copiar y adaptar.
- Mantenerlo breve (≈10-14 líneas extra) para no inflar demasiado la
  descripción; el detalle profundo vive en la skill.

Esto es always-on (la descripción del tool siempre está en el catálogo cuando
el tool está disponible) → mueve el comportamiento por default sin
discrecionalidad del modelo.

### Componente 2 — skill built-in `gsheets-presentable-output`

Nuevo directorio `src/libs/colmena/skills/gsheets-presentable-output/` con
`SKILL.md` + `references/`, compilado al binario vía el `include_dir!` ya
existente (`BUILTIN_SKILLS_DIR`). Mismo shape que `gdocs-surgical-edits` /
`sql-query-best-practices`.

`SKILL.md` frontmatter: `name: gsheets-presentable-output`, `description`
(catálogo: "Recetas de formato presentable para Google Sheets — header, moneda,
bordes, fila de totales, anchos, en una llamada multi-op. Cargá la reference de
tu escenario."), `references:` listando las sub-referencias.

References (markdown bajo `references/`):
- `01-recipe.md` — receta paso a paso de un reporte profesional; orden correcto
  **datos → fórmulas → formato** (formato al final, sobre el rango ya poblado).
- `02-palettes.md` — paletas hex listas que se ven bien (header azul `#1F4E78`
  / gris oscuro, texto blanco, zebra sutil, fila de totales gris claro
  `#D9D9D9`).
- `03-number-formats.md` — patrones `numberFormat`: moneda `$#,##0` / `$#,##0.00`,
  porcentaje `0.0%`, fecha, miles, cuándo usar cada uno.
- `04-multi-op-template.md` — el JSON `ops:[...]` COMPLETO de un reporte tipo
  (título, header, moneda en montos, totales, bordes, anchos) para copiar y
  adaptar rangos.
- `05-layout.md` — alineación (texto izquierda / números derecha), anchos,
  bordes de tabla, separación de la fila de totales, congelar header (si aplica).

El cuerpo de `SKILL.md` es un overview que enlaza las references y repite el
principio "formato ≠ valores; formato al final; una llamada multi-op".

### Componente 3 — auto-enroll gate (`src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs`)

Nuevo helper `agent_has_gsheets_format_tool(config, inputs) -> bool` que espeja
`agent_has_gdocs_edit_tools`: detecta si `gsheets_format_range` quedará en el
catálogo resuelto del agente — true si `enabled_tools` incluye el toolkit
`gsheets`, `"*"`, o el nombre `gsheets_format_range` explícito, y NO está
excluido vía `!gsheets_format_range`. (Reusar la misma lógica de resolución de
`enabled_tools` que ya usa el resto del archivo.)

En el bloque de auto-enroll (junto al de `gdocs-surgical-edits`, ~llm.rs:599):
```rust
if Self::agent_has_gsheets_format_tool(config, inputs)
    && !skills_config.builtin.iter().any(|n| n == "gsheets-presentable-output")
{
    skills_config.builtin.push("gsheets-presentable-output".to_string());
}
```
Idempotente (no-op si el operador ya la declaró). No dispara para agentes
read-only ni para los que excluyeron el tool.

## Testing

- **Unit (skill compila/parsea)**: reusar el patrón del test de
  `gdocs-surgical-edits` en `builtin_skill_repository.rs` — `BuiltinSkillRepository::new(&["gsheets-presentable-output"])`,
  `load_skill` del overview + cada reference resuelve. Verifica que el
  `include_dir!` la incluye y el frontmatter parsea.
- **Unit (gate)** en `llm.rs` tests (espejo de los tests de
  `gdocs-surgical-edits` enrollment ~llm.rs:5533): `gsheets` alias enrola;
  `["gsheets_read"]` (sin format) NO enrola; `["gsheets","!gsheets_format_range"]`
  NO enrola; `"*"` enrola; idempotente si el operador ya la puso.
- **Text**: `cargo test --lib text` — la `description` enriquecida de
  `gsheets_format_range` parsea (yaml válido).
- **E2E live (criterio de éxito real)**: un prompt **abierto** ("armá un reporte
  de ventas profesional con estos datos …", SIN instrucciones de formato) debe
  ahora producir formato rico (moneda + bordes + fila de totales destacada)
  — el caso que hoy falla con `gemini-2.5-flash`. Read-back del sheet confirma
  los atributos. Si tras los dos prongs el modelo sigue subutilizando, iterar
  el wording del nudge/skill (parte del E2E, no un task nuevo).

## No-objetivos

- Formato de tablas gdocs (el tool no existe; item separado en Subsystem G v1.1).
- Cambios al código de `gsheets_format_range` (ya es completo y verificado).
- Forzar formato (hard-coded) — el enfoque es *guía*, el modelo sigue
  decidiendo; un agente puede legítimamente no formatear (ej. salida intermedia).

## Impacto cross-repo

Ninguno. Solo `gsheets.yaml` (texto), un `SKILL.md` + references nuevos
(compilados vía `include_dir!`), y un gate aditivo en `llm.rs`. Sin cambios en
la firma pública de `EngineConfig`/`ColmenaEngine`/traits exportados → el worker
de ADP no se ve afectado.
