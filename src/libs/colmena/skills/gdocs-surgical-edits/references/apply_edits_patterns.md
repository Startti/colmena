# `apply_edits` — composing multiple sub-edits atomically

`apply_edits` toma una lista de sub-edits y los aplica COMO UNA SOLA
transacción contra Google Docs. Si cualquier sub-edit falla en la fase
de resolución (`TextNotFound`, `ConfirmManyMatches`, overlap detectado),
el compound entero aborta sin tocar el doc.

Esta reference cubre cuándo usar `apply_edits` vs múltiples standalone
calls, y cómo evitar los modos de falla específicos del compound.

## Cuándo SÍ usar apply_edits

- Múltiples edits que conceptualmente son una unidad (renombrar una
  variable en 3 lugares específicos, agregar formato a 4 párrafos
  contiguos de una sección, etc).
- Querés rollback automático si cualquiera falla.
- Querés ahorrar round-trips contra la API (1 batchUpdate en vez de N).

## Cuándo NO usar apply_edits

- Los edits son independientes y querés que algunos sigan aunque
  otros fallen → standalone tools, uno por call.
- Necesitás scope.UnderHeading / BetweenHeadings que dependen del
  estado POST-edit anterior (e.g. agregaste un heading nuevo en sub-edit
  1, querés scopear sub-edit 2 a su rango). El compound resuelve todos
  los scopes contra el SNAPSHOT inicial — no ve los cambios intermedios.
- Necesitás `anchor` u `occurrence` — el compound no los soporta;
  esos parámetros son solo del standalone `replace_text`.

## Anatomía del flujo interno

`apply_edits` corre en dos fases:

### Fase A — resolve (no escribe nada todavía)

1. Lee el snapshot actual del doc UNA vez.
2. Para cada sub-edit, calcula los hits contra el snapshot.
3. Aplica los guards: `TextNotFound` si no hay hits, `ConfirmManyMatches`
   si hay ≥5.
4. Resuelve cada hit a un range (byte_off, byte_len) en su párrafo.
5. Detecta overlaps cross-edit en el mismo párrafo → error estructurado
   si dos sub-edits chocan.

### Fase B — sort + emit

6. Ordena TODOS los emits write-backwards globalmente (párrafo
   descendente, byte_off descendente).
7. Aplana en un solo batchUpdate y lo manda.

### Por qué el sort cross-edit importa

Google's batchUpdate aplica requests SECUENCIALMENTE. Cada request ve
los índices mutados por las requests previas. El write-backwards
asegura que cada operación toque una parte intacta del doc.

Si el sort fuera per-sub-edit en vez de global, sub-edit N+1
trabajaría con índices del snapshot original pero el doc ya estaría
shifteado por sub-edit N. Eso fue el bug que motivó este skill.

## Reglas de oro

1. **`apply_edits` ve UN snapshot. Si necesitás "leer después de
   editar", split en calls separados.**
   ```
   ❌ apply_edits([insert "<NEW>", replace "<NEW>" with "<v2>"])
       (la 2da resolve no ve el insert)
   ✅ insert_after_text "<NEW>" → luego replace_text "<NEW>" con
       el doc fresh-loaded
   ```

2. **NO uses `apply_edits` para edits que pueden fallar
   independientemente.** Si renombrar 3 variables son 3 tareas, y
   querés que las 2 que existan se hagan aunque la 3ra (con typo) no
   matchee — usá 3 standalone `replace_text`, no `apply_edits`.

3. **El compound no soporta `anchor` ni `occurrence`.** Si necesitás
   esos, no es para `apply_edits`.

4. **Si vas a meter ≥5 sub-edits, pensá dos veces.** Probablemente
   estés haciendo dos cosas distintas que merecen 2 calls separados,
   o estás replicando el mismo cambio en lugares que merecen un find
   más amplio con scope adecuado.

5. **Overlapping ranges en el mismo párrafo = error estructurado.**
   Si dos sub-edits tocan rangos solapados de un párrafo, el compound
   aborta con `InvalidArgs` y un mensaje accionable. Si necesitan
   realmente solaparse, divididos en dos `apply_edits` separados (el
   primero deja el doc en estado intermedio, el segundo opera sobre él).

## Ejemplo bueno

```json
{
  "tool": "apply_edits",
  "edits": [
    {
      "tool": "replace_text",
      "find": "Variable A",
      "replace": "Variable Alpha",
      "scope": { "type": "UnderHeading", "heading": "Definiciones" }
    },
    {
      "tool": "replace_text",
      "find": "Variable B",
      "replace": "Variable Beta",
      "scope": { "type": "UnderHeading", "heading": "Definiciones" }
    }
  ]
}
```

Atómico, scope claro, cada find es único bajo ese heading.

## Ejemplo malo (caso real del bug)

```json
{
  "tool": "apply_edits",
  "edits": [
    { "tool": "replace_text", "find": "Calentamiento:", "replace": "- **Calentamiento:**" },
    { "tool": "replace_text", "find": "Ejercicios:", "replace": "- **Ejercicios:**" },
    { "tool": "replace_text", "find": "Sentadillas: 3 series...", "replace": "  1. Sentadillas: 3 series..." },
    { "tool": "replace_text", "find": "Enfriamiento: Estiramientos...", "replace": "- **Enfriamiento:** Estiramientos..." }
  ]
}
```

Problemas:
- "Ejercicios:" matchea Día 1 y Día 3 (mismo header en distintas secciones).
- "Enfriamiento: Estiramientos..." matchea 4 días (texto idéntico al final
  de cada uno).
- Sin scope → todos los días se modifican, no solo Día 1.
- Hoy esto se aborta con `ConfirmManyMatches` para "Enfriamiento..." si
  son 5+ días. Para 4 días el guard no dispara — usá scope.
- Aún si pasara el guard, el resultado visual sería texto markdown
  literal en el doc (los `**` y `-` se ven como caracteres, no como
  bullets/bold). Para cambiar estilo VISUAL → `style_text` o
  `replace_section` con markdown. NUNCA inyectar sintaxis markdown
  como texto plano.
