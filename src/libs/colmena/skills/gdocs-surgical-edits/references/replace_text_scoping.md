# Scoping find/replace operations

`replace_text` y `delete_text` (standalone y dentro de `apply_edits`)
buscan substrings en el doc. Sin scope, buscan en todo el documento.
Esta reference cubre las 4 formas de acotar dónde buscan.

## Las 4 herramientas de scope

### 1. `Scope::Paragraph { n }` — un solo párrafo

Cuando ya sabés el número de párrafo (lo conseguiste de
`gdocs_read_outline`):

```json
{
  "tool": "replace_text",
  "find": "Enfriamiento: Estiramientos de 5-10 minutos.",
  "replace": "- **Enfriamiento:** Estiramientos de 5-10 minutos.",
  "scope": { "type": "Paragraph", "n": 9 }
}
```

Si el find no aparece en ese párrafo → `TextNotFound`. Si aparece más de
una vez en ese mismo párrafo → todos los hits dentro del párrafo se
reemplazan (raro pero posible).

### 2. `Scope::Tab { tab_id }` — solo en una tab específica

Para docs multi-tab. Usa el `tab_id` que devuelve `gdocs_list_tabs`.

### 3. `Scope::UnderHeading { heading }` — bajo un heading

Cuando querés tocar todo el contenido de "Día 1: Entrenamiento de
Fuerza" pero no sabés los números de párrafo exactos:

```json
{
  "tool": "replace_text",
  "find": "Sentadillas",
  "replace": "Sentadillas con peso",
  "scope": { "type": "UnderHeading", "heading": "Día 1: Entrenamiento de Fuerza" }
}
```

Resolvé el rango correcto comparando el heading con los siguientes
heading-of-same-or-higher-level. Si el heading aparece más de una vez
en el doc, el comportamiento puede ser ambiguo — preferí `Paragraph`
o `BetweenHeadings`.

### 4. `Scope::BetweenHeadings { start, end }` — entre dos headings

Para "todo lo que está entre el heading X y el heading Y":

```json
{
  "scope": { "type": "BetweenHeadings", "start": "Día 1", "end": "Día 2" }
}
```

Si no hay heading que coincida con `end`, llega hasta el final del doc
(o de la tab). Útil para "todo Día 1" cuando los headings son únicos.

## El parámetro `anchor` (solo standalone replace_text)

Independiente de scope. Filtra los hits a aquellos cuyo párrafo TAMBIÉN
contenga `anchor`:

```json
{
  "find": "rojo",
  "replace": "verde",
  "anchor": "Capítulo 3"
}
```

Hits = párrafos donde aparece "rojo" Y "Capítulo 3" en el mismo
párrafo. Útil cuando dos secciones distintas hablan de "rojo" pero solo
querés la sección del capítulo 3.

`apply_edits` NO acepta `anchor` — para multi-edit usá scope.

## El parámetro `occurrence` (solo standalone replace_text)

Pinea un ordinal específico:

```json
{ "find": "Resumen", "replace": "Conclusión", "occurrence": 2 }
```

Toma SOLO el 2do hit en el orden del doc, ignora los demás. Útil
después de ver el `preview` que devuelve `ConfirmManyMatches`.

## Cómo elegir cuál usar

| Caso | Mejor opción |
|---|---|
| Sé el número exacto de párrafo (lo vi en outline) | `Scope::Paragraph` |
| Querés toda una sección bajo un heading | `Scope::UnderHeading` o `Scope::BetweenHeadings` |
| Multi-tab y querés solo una | `Scope::Tab` |
| Querés el N-ésimo hit en orden de aparición (solo standalone) | `occurrence: N` |
| Querés disambiguar por contexto en el mismo párrafo (solo standalone) | `anchor: "..."` |
| Querés tocar en todo el doc — confirmadamente, NO es ambiguo | `Scope::All` (default) |

## Pattern recomendado para multi-sección

1. `gdocs_read_outline` → mapeás párrafos a headings/sections.
2. Identificás el `paragraph_n` (o rango) de la sección que querés tocar.
3. Mandás el edit con `Scope::Paragraph { n }` explícito.
4. Si el primer edit funcionó, sumás más con el mismo enfoque.

NO mandes 7 edits sin scope esperando que el find sea único — el doc
suele tener más repetición de la que pensás.
