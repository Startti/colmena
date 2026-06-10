# Ejemplos before/after

Casos reales de uso, con la versión que el LLM tiende a producir y la
versión correcta. Léelos antes de actuar sobre un doc con estructura
repetida.

## Caso 1 — agregar formato a una sección con secciones hermanas similares

**User:** "Los estilos del Día 1 no están bonitos. Hacelo lista con bullets
y números."

**Doc actual (5 días, mismo template):**
```
Día 1: Entrenamiento de Fuerza
  Calentamiento: 10 minutos de trote.
  Ejercicios:
    Sentadillas: 3 series de 12 reps
    Flexiones de brazos: 3 series de 10 reps
  Enfriamiento: Estiramientos de 5-10 minutos.

Día 2: Cardio y Core
  Calentamiento: 5 minutos de saltos.
  Ejercicios de Cardio:
    Correr 30 minutos.
  Enfriamiento: Estiramientos de 5-10 minutos.

... (Días 3, 4, 5 con la misma estructura)
```

### ❌ Wrong way (el bug original)

```json
{
  "tool": "apply_edits",
  "edits": [
    { "tool": "replace_text", "find": "Calentamiento: 10 minutos de trote.", "replace": "- **Calentamiento:** 10 minutos de trote." },
    { "tool": "replace_text", "find": "Ejercicios:", "replace": "- **Ejercicios:**" },
    { "tool": "replace_text", "find": "Sentadillas: 3 series de 12 reps", "replace": "  1. Sentadillas: 3 series de 12 reps" },
    { "tool": "replace_text", "find": "Flexiones de brazos: 3 series de 10 reps", "replace": "  2. Flexiones de brazos: 3 series de 10 reps" },
    { "tool": "replace_text", "find": "Enfriamiento: Estiramientos de 5-10 minutos.", "replace": "- **Enfriamiento:** Estiramientos de 5-10 minutos." }
  ]
}
```

Problemas:
- `Ejercicios:` matchea Día 1 y Día 3 → ambos se modifican.
- `Enfriamiento: Estiramientos...` matchea 4 días → 4 se modifican.
- Aún si scope-eara: el resultado son CARACTERES literales `**`, `-`
  en el doc. No estilo visual.

### ✅ Right way

Paso 1 — `read_outline`:
```
{ paragraph: 1, kind: "heading1", text: "Plan de Trabajo" }
{ paragraph: 2, kind: "heading2", text: "Día 1: Entrenamiento de Fuerza" }
{ paragraph: 3, kind: "paragraph", text: "Calentamiento: 10 minutos..." }
{ paragraph: 4, kind: "paragraph", text: "Ejercicios:" }
{ paragraph: 5, kind: "paragraph", text: "Sentadillas..." }
{ paragraph: 6, kind: "paragraph", text: "Flexiones..." }
{ paragraph: 7, kind: "paragraph", text: "Enfriamiento..." }
{ paragraph: 8, kind: "heading2", text: "Día 2: Cardio..." }
...
```

Día 1 = párrafos 3-7. Día 2 empieza en 8.

Paso 2 — `replace_section` con el contenido bien formateado:
```json
{
  "tool": "replace_section",
  "scope": { "type": "BetweenHeadings", "start": "Día 1", "end": "Día 2" },
  "new_markdown": "## Día 1: Entrenamiento de Fuerza\n\n- **Calentamiento:** 10 minutos de trote.\n- **Ejercicios:**\n  1. Sentadillas: 3 series de 12 reps\n  2. Flexiones de brazos: 3 series de 10 reps\n- **Enfriamiento:** Estiramientos de 5-10 minutos."
}
```

Google Docs convierte el markdown a bullets/numbered list/bold reales.
Día 2-5 quedan intactos.

## Caso 2 — renombrar un término en todo el doc

**User:** "Cambiá 'cliente' por 'customer' en todo el documento."

### ❌ Wrong way (cuando hay >5 ocurrencias)

```json
{ "tool": "replace_text", "find": "cliente", "replace": "customer" }
```

Si hay ≥5 ocurrencias → `ConfirmManyMatches`. Si el LLM reintenta con
`confirm_many: true` sin pensar:
- Reemplaza "cliente" pero también "clientela", "clientes", "clientelar"
  (substring match).
- Pierde mayúsculas: "Cliente" → "customer", no "Customer".

### ✅ Right way

```json
{
  "tool": "replace_text",
  "find": "cliente",
  "replace": "customer",
  "whole_word": true,
  "case_sensitive": true,
  "confirm_many": true
}
```

`whole_word: true` evita matchear dentro de "clientela". Si querés
cubrir capitalización, hacés DOS calls: una para "cliente"→"customer"
y otra para "Cliente"→"Customer".

## Caso 3 — borrar todos los TODOs de un documento

**User:** "Eliminá todas las notas TODO del doc."

### ❌ Wrong way

```json
{ "tool": "delete_text", "find": "TODO" }
```

Borra los caracteres "TODO" pero deja el resto de la línea: `TODO: revisar fórmula` queda como `: revisar fórmula`.

### ✅ Right way

Si las notas son párrafos enteros, usá `replace_section` con
`Scope::Paragraph { n }` por cada uno, devolviendo string vacío.

Si son inline, usá `delete_text` con un find que cubra toda la nota:
```json
{ "tool": "delete_text", "find": "TODO: revisar fórmula\n" }
```

Incluí el `\n` para que el párrafo se colapse limpiamente.

## Caso 4 — agregar un párrafo después de una sección específica

### ❌ Wrong way

```json
{
  "tool": "insert_after_text",
  "anchor": "Conclusión",
  "new_markdown": "Notas adicionales:..."
}
```

Si "Conclusión" aparece en varios capítulos → matchea el PRIMERO. Si
querías el último, problema.

### ✅ Right way

Paso 1 — `read_outline` para confirmar dónde está cada "Conclusión".
Paso 2 — `insert_after_text` con un anchor MÁS LARGO que sea único:
```json
{
  "tool": "insert_after_text",
  "anchor": "Conclusión del experimento de Junio",
  "new_markdown": "..."
}
```

O scope a un párrafo específico con otra tool si tu anchor sigue
siendo ambiguo.
