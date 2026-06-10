# Error recovery — decoder for surgical edit failures

Cada error que devuelven los gdocs edit tools trae structured data útil
para decidir el próximo paso. NO reintentes a ciegas. Esta reference
cubre los 4 errores más comunes y su recovery canónico.

## `ConfirmManyMatches`

**Qué dispara:** standalone `replace_text` con ≥5 hits sin
`confirm_many: true`; `apply_edits` con cualquier sub-edit que matchea
≥5 párrafos (no hay bypass por confirm_many).

**Payload:**
```json
{
  "find": "Enfriamiento: Estiramientos de 5-10 minutos.",
  "count": 5,
  "preview": [
    { "n": 1, "paragraph": 9, "preview": "...Día 1 Enfriamiento: Estiramientos..." },
    { "n": 2, "paragraph": 18, "preview": "...Día 2 Enfriamiento: Estiramientos..." },
    { "n": 3, "paragraph": 26, "preview": "...Día 3 Enfriamiento: Estiramientos..." },
    { "n": 4, "paragraph": 32, "preview": "...Día 4 Enfriamiento: Estiramientos..." },
    { "n": 5, "paragraph": 39, "preview": "...Día 5 Enfriamiento: Estiramientos..." }
  ]
}
```

**Cómo recuperar:**

1. Leé el `preview`. Identificá CUÁL de los hits es el que querías.
2. Tomá el `paragraph` de ese hit.
3. Re-mandá con `scope: { type: "Paragraph", n: <ese paragraph> }`.

**Anti-pattern:** pasar `confirm_many: true` para hacer replace-all es
casi siempre incorrecto. Solo usalo cuando explícitamente querés
afectar TODOS los hits (rename masivo intencional).

**Apply_edits no acepta `confirm_many` ni `occurrence`** — la única
salida es scope. Si necesitás reemplazar el mismo texto en N párrafos
específicos, mandá N sub-edits con `Scope::Paragraph { n }` distinto
cada una.

## `AmbiguousMatch`

**Qué dispara:** standalone tools con resultado ambiguo bajo
condiciones específicas (anchor matchea pero hay múltiples en el mismo
párrafo, etc).

**Payload típico:** lista de matches con preview.

**Recuperar:** similar a ConfirmManyMatches — agregá `anchor` más
específico o pinea con `occurrence: N` después de leer el preview.

## `TextNotFound`

**Qué dispara:** el find string no aparece en el scope dado.

**Payload:**
```json
{
  "find": "Enfriamento: Estiramientos.",
  "fuzzy_suggestions": ["Enfriamiento: Estiramientos.", "Estiramiento: 5 min."]
}
```

**Causas comunes (en orden de probabilidad):**

1. **Typo en el find.** Mirá `fuzzy_suggestions` — suele estar ahí la
   versión correcta.
2. **Scope demasiado estricto.** Pediste `Scope::Paragraph { n: 5 }`
   pero el texto está en el párrafo 6.
3. **El texto cambió desde tu última lectura.** Otro peer editó. Releé
   con `gdocs_read_outline` o `read_as_markdown`.
4. **Diferencia invisible** — non-breaking space, dash en vez de hyphen,
   mayúscula diferente. La búsqueda es exacta y case-sensitive.

**Recuperar:**

1. Aceptá `fuzzy_suggestions[0]` si parece tu find con un typo
   corregido — útil cuando vos generaste el find string de memoria.
2. Si no hay suggestions útiles, releé el doc con
   `gdocs_read_as_markdown` (scope acotado al área que esperabas) y
   copiá el texto literal antes de re-mandar.
3. Si fuiste muy específico en `Scope::Paragraph`, probá expandir a
   `Scope::UnderHeading` o `Scope::All`.

## `InvalidArgs` con mensaje "overlapping edits on paragraph N"

**Qué dispara:** `apply_edits` detectó que dos sub-edits tocan rangos
de bytes solapados dentro del mismo párrafo.

**Payload:**
```
apply_edits: overlapping edits on paragraph 5 — range 10..25 intersects
20..35. Two replace/delete ranges that overlap cannot be reordered
safely; split them into separate apply_edits calls or pick
non-overlapping find strings.
```

**Causas:**

- Dos `replace_text` con finds que comparten un fragmento de texto en
  el mismo párrafo.
- Un `insert_after_text` cuyo anchor cae dentro del rango de otro
  `replace_text`.

**Recuperar:**

1. Identificá los dos finds del mensaje (los offsets están en el
   error). Pensá cuál querés aplicar PRIMERO.
2. Mandá un `apply_edits` con solo el primero.
3. Después, otro `apply_edits` con el segundo (que ahora se resuelve
   contra el doc post-primer-edit).

## `InvalidArgs` con mensaje "two inserts share the same anchor"

**Qué dispara:** dos `insert_after_text` con el mismo anchor en el
mismo párrafo.

**Recuperar:** combinalos en un solo `insert_after_text` con el
markdown concatenado, o mové uno a un anchor distinto.

## Patrones generales

- **Cuando un error te dice "hay 5 matches", asumí que estabas
  pensando en 1.** El find string estaba mal scope-eado. Releé outline.
- **Cuando recibís un error, NUNCA reintentes con el mismo input
  agregando `confirm_many: true`.** Eso convierte el error en
  corrupción silenciosa.
- **Errores estructurados son una conversación, no una pared.** El
  payload trae lo que necesitás para corregir; usalo.
