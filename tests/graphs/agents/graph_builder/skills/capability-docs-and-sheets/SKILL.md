---
name: capability-docs-and-sheets
description: Use when the user talks about spreadsheets or documents — including lay terms like "Excel", "planilla", "hoja", "tabla", "Word", "documento". Covers the gsheets and gdocs toolkits and the online-vs-downloadable disambiguation.
---

# Hojas de cálculo y documentos (gsheets / gdocs)

Cuando la persona habla de "Excel", "planilla", "hoja", "tabla", "Word" o
"documento", casi siempre quiere que el agente lea, escriba o cree archivos en
**Google Sheets** o **Google Docs**. Colmena expone esas capacidades como dos
toolkits que se activan con un **flag**, no como nodos que haya que configurar.

## Lo más importante: son TOOLKIT FLAGS

No agregás nodos ni configuración por nodo. Solo listás el alias del toolkit en
`enabled_tools` dentro del `llm_call`. Las credenciales vienen del entorno del
proceso (ver §"Dependencia de credenciales"), así que el flag por sí solo
alcanza:

```json
"enabled_tools": ["gsheets"]
```

```json
"enabled_tools": ["gdocs"]
```

```json
"enabled_tools": ["gdocsread"]
```

- `gsheets` → expande a los 15 tools `gsheets_*`.
- `gdocs` → expande a los 35 tools `gdocs_*`.
- `gdocsread` → subconjunto **de solo lectura** de gdocs (10 tools: `list_tabs`,
  `read_as_markdown`, `read_outline`, `list_named_ranges`, `read_tables`,
  `export`, `acknowledge_human_changes`, `list_documents`, `list_permissions`,
  `list_comments`).

### Excluir tools puntuales con `!`

Cualquier entrada que empiece con `!` es una exclusión; se aplica DESPUÉS de
expandir el alias (diferencia de conjuntos, el orden no importa). Útil para
quitar tools peligrosos o de creación:

```json
"enabled_tools": ["gsheets", "!gsheets_delete_sheet"]
```

```json
"enabled_tools": ["gdocs", "!gdocs_create", "!gdocs_create_from_markdown"]
```

Podés combinar varios aliases y tools individuales en la misma lista, p.ej.
`["gsheets", "gdocs", "current_time", "!gsheets_delete_sheet"]`.

## Tools clave de gsheets

Referencia completa en `docs/developer_guide/41_builtin_tools_index.md` (§gsheets)
y `docs/developer_guide/39_gsheets.md`.

| Tool | Para qué sirve |
|---|---|
| `gsheets_create_spreadsheet` | Crear una planilla nueva vacía. Devuelve `{spreadsheet_id, url}`. |
| `gsheets_read` | Leer un rango (devuelve tabla markdown por defecto; `format: "json"` para datos estructurados). |
| `gsheets_set_cell` | Escribir un valor o fórmula en una sola celda. |
| `gsheets_set_range` | Escribir un bloque 2-D de valores desde una dirección. |
| `gsheets_format_range` | Aplicar formato (negrita, colores, bordes, alineación, formato numérico) en un `batchUpdate` atómico. |
| `gsheets_share` | Dar acceso (reader / commenter / writer) a una cuenta Google. |
| `gsheets_run_python` | Análisis pandas server-side sobre rangos (las filas nunca pasan por el LLM). Preferido para comparar/cruzar tablas. |
| `gsheets_create_from_xlsx` | Subir un `.xlsx` adjunto y convertirlo en una Google Sheet nueva. |
| `gsheets_export_xlsx` | Descargar una Google Sheet existente como `.xlsx`. |

(Otros del toolkit: `gsheets_list_sheets`, `gsheets_list_spreadsheets`,
`gsheets_add_sheet`, `gsheets_delete_sheet`, `gsheets_list_permissions`,
`gsheets_unshare`.)

## Tools clave de gdocs

Referencia completa en `docs/developer_guide/41_builtin_tools_index.md` (§gdocs)
y `docs/developer_guide/45_gdocs.md`.

| Tool | Para qué sirve |
|---|---|
| `gdocs_create` | Crear un Google Doc vacío en una carpeta compartida. |
| `gdocs_create_from_markdown` | Crear un doc a partir de un string markdown (Drive convierte). |
| `gdocs_read_as_markdown` | Exportar el doc (o un tab) como markdown. |
| `gdocs_replace_text` | Buscar y reemplazar texto (content-addressed, con scope opcional). |
| `gdocs_insert_after_text` | Insertar markdown después de un ancla de texto. |
| `gdocs_append_markdown` | Agregar markdown al final del doc (o de un tab). |
| Tools de tabla | `gdocs_read_tables`, `gdocs_set_table_cell`, `gdocs_insert_table_row`, `gdocs_delete_table_row`, `gdocs_insert_table_column`, `gdocs_delete_table_column`, `gdocs_format_table` — edición quirúrgica de celdas (0-based; llamá `gdocs_read_tables` primero para descubrir coordenadas). |

(El toolkit tiene 35 tools en total: lectura, edición content-addressed,
named ranges, multi-tab, export, share, comments y co-edit guard.)

## La desambiguación de "Excel" / "Word" (CRÍTICO)

Las palabras "Excel", "planilla" u "hoja" son **ambiguas**: pueden significar
dos cosas técnicamente muy distintas. NO asumas — preguntá en lenguaje llano.

**"Excel" / "planilla" / "hoja" puede ser:**

1. Una **hoja editable en línea** (Google Sheet) — el caso por defecto. Se usa
   el toolkit `gsheets` (`gsheets_create_spreadsheet`, `gsheets_set_range`, etc.).
   El resultado es un link que la persona abre y edita en el navegador.
2. Un **archivo `.xlsx` descargable** — un archivo que la persona baja a su
   compu y abre en Microsoft Excel. Para entrada usá
   `gsheets_create_from_xlsx` (subir un `.xlsx`); para entregar un archivo
   descargable usá `gsheets_export_xlsx` (bajar una Sheet como `.xlsx`).

**Mismo patrón con "Word" / "documento" → `gdocs`:**

1. Un **documento editable en línea** (Google Doc) → toolkit `gdocs`
   (`gdocs_create`, `gdocs_create_from_markdown`, edición content-addressed).
2. Un **archivo descargable** (`.docx` / PDF) → `gdocs_export`
   (`docx` / `pdf` / `markdown` / `txt` / …) para entregar; importar `.docx`
   sería `gdocs_create_from_docx`.

**Cómo preguntar (en lenguaje de la persona, no técnico):**

> "¿Lo querés como algo **editable en línea** (un link de Google que abrís y
> editás en el navegador) o como un **archivo para descargar** (un .xlsx / Word
> que bajás a tu compu)?"

Y mapeá la respuesta:

| Respuesta de la persona | Toolkit / tools |
|---|---|
| "editable en línea", "que pueda editar", "compartir un link" | `gsheets` / `gdocs` (online por defecto) |
| "para descargar", "un archivo Excel", "un .xlsx", "un Word para bajar" | `gsheets_export_xlsx` / `gdocs_export` (o `*_create_from_*` para importar) |

## Dependencia de credenciales (OAuth / env)

Estos toolkits **no se configuran por nodo** — las credenciales se resuelven a
nivel del proceso vía OAuth user-scoped. Las env vars que el deploy debe
proveer son `COLMENA_GOOGLE_OAUTH_CLIENT_ID`,
`COLMENA_GOOGLE_OAUTH_CLIENT_SECRET`, `COLMENA_GOOGLE_OAUTH_REFRESH_TOKEN` y
`COLMENA_GOOGLE_SHARE_EMAIL`. Si la persona pregunta por el setup, mencioná que
hace falta tener esas credenciales de Google configuradas en el entorno donde
corre el agente — el grafo en sí no las lleva.

## Ejemplo ejecutable — agente que escribe datos en una hoja

Un `llm_call` con `"enabled_tools": ["gsheets"]`. La persona pide la planilla en
lenguaje natural; el modelo crea la hoja y escribe los datos con los tools del
toolkit:

```json
{
  "nodes": [
    {
      "id": "asistente_planillas",
      "node_type": "llm_call",
      "config": {
        "provider": "google",
        "model": "gemini-2.5-flash",
        "system_prompt": "Sos un asistente que ayuda a crear y llenar hojas de cálculo en Google Sheets. Cuando el usuario pida una planilla, creala con gsheets_create_spreadsheet, escribí los datos con gsheets_set_range y devolvé el link.",
        "enabled_tools": ["gsheets"],
        "input": "Creame una planilla con las ventas del primer trimestre: enero 1200, febrero 1500, marzo 1800."
      }
    }
  ],
  "edges": []
}
```

El modelo elige llamar `gsheets_create_spreadsheet`, luego `gsheets_set_range`
con la cabecera `Mes / Ventas` y las tres filas, y devuelve la URL de la hoja.

## Cableado

Para conectar este `llm_call` con un trigger, pasarle inputs desde otros nodos,
o encadenar el resultado, ver [[building-graphs-core]].
