---
name: gdocs-surgical-edits
description: Use when calling any gdocs_* edit tool (replace_text, apply_edits, delete_text, insert_after_text, replace_section, style_text, etc.). Covers how to scope find/replace operations, how to combine multiple edits in one atomic apply_edits call, what the surgical edit errors mean, and the canonical pattern for restyling existing content.
references:
  - name: replace_text_scoping
    description: When and how to use scope, anchor, occurrence. Read before doing any find/replace where the text might appear more than once (different days, different sections, recurring lines).
  - name: apply_edits_patterns
    description: How to bundle multiple sub-edits in one atomic apply_edits call. Covers ordering, overlapping ranges, when to split into separate calls. Read before composing 3 or more edits.
  - name: error_recovery
    description: Decoder for ConfirmManyMatches, AmbiguousMatch, TextNotFound, and overlapping-ranges errors. Each section says what triggered it and how to recover.
  - name: style_changes_pattern
    description: Canonical recipe for "add styles / numbering / bullets to a section". Read before trying to convert plain text into formatted lists or headings.
  - name: before_after_examples
    description: Concrete worked examples — including a real workout-plan corruption case — showing the wrong way and the right way side by side.
---

# gdocs — Surgical edits best practices

Cargá la reference que más se acerca a tu intent. Si tu pedido es genérico
("dale formato"), empezá por `style_changes_pattern`. Si recibís un error
estructurado, andá directo a `error_recovery`.

## Quick rules (siempre)

1. **Antes de cualquier edit que afecte texto repetido, leé el outline.**
   `gdocs_read_outline` te da el mapa (paragraph_n + kind por entry). Si
   tu find string ("Enfriamiento:", "Ejercicios:", "Resumen") podría
   aparecer en más de una sección, NECESITÁS scope.

2. **Preferí `scope.paragraph_range` o `Scope::Paragraph { n }` antes
   que un find string genérico.** El LLM controla el alcance — usalo.
   `Scope::All` es la default pero rara vez es la elección correcta
   cuando hay secciones repetidas.

3. **`apply_edits` aborta con `ConfirmManyMatches` cuando una sub-edit
   matchea 5 o más párrafos** (igual que el standalone `replace_text`).
   No hay bypass por `confirm_many` — el camino correcto es narrow-down
   vía scope. Si recibís el error, mirá el `preview` que devuelve y
   decidí cuál es el rango real que querías tocar.

4. **`apply_edits` rechaza byte-ranges solapados dentro del mismo
   párrafo.** Si dos sub-edits tocan el mismo párrafo, asegurate que
   sus ranges sean disjuntos. Si necesitan solaparse, dividilas en dos
   `apply_edits` calls separados.

5. **`dry_run: true` (donde aplica) es tu amigo cuando no estás seguro.**
   Te devuelve los hits que matchearían sin tocar el doc. Standalone
   `replace_text` lo soporta — `apply_edits` no, así que en compounds
   andá con la `read_outline` por delante.

6. **Errores son señal, no obstáculo.** `ConfirmManyMatches`,
   `AmbiguousMatch`, `TextNotFound` traen `preview` o
   `fuzzy_suggestions`. Leelos y pensá antes de reintentar — no metas
   `confirm_many: true` o un find más largo a ciegas.

## Anti-patterns comunes

- ❌ `apply_edits` con 7 `replace_text` cuyos `find` strings cortos
  matchean en múltiples días (caso real: "Enfriamiento: Estiramientos
  de 5-10 minutos." matchea 4 días).
- ❌ Asumir que `find: "Día 1"` es único — un párrafo puede contener
  el texto sin ser el heading; el outline manda.
- ❌ Convertir texto plano a markdown literal con `replace_text`. El
  formato visual de Google Docs no se cambia metiendo `**bold**` como
  caracteres — eso queda como texto crudo. Usá `style_text` para
  estilos visuales o `replace_section` con `gdocs_append_markdown` si
  necesitás reflujo estructural.

## Si tu intent no está cubierto por las references

Aplicá las Quick rules y empezá con la operación más conservadora:
1. `gdocs_read_outline` para entender la estructura.
2. `gdocs_read_as_markdown` con scope acotado para ver el contenido.
3. Una sola edit primero con scope explícito. Si funciona, sumá más.
