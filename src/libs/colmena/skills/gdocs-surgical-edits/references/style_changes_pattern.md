# Style changes — la receta canónica

"Agregale formato a esta sección", "convertí estos items en una lista
numerada", "ponele bold al título"... este patrón cubre el 80% de los
casos.

## TL;DR

**Para cambiar ESTILO VISUAL (bold, italic, font_size, color):**
→ `gdocs_style_text` con el rango exacto.

**Para cambiar ESTRUCTURA (texto plano → lista bullets, párrafos →
heading, etc.):**
→ `gdocs_replace_section` con el contenido nuevo en markdown (Google
Docs lo renderiza nativamente).

**NUNCA inyectes sintaxis markdown literal con `replace_text`.** Eso
mete los caracteres `**`, `-`, `1.` como TEXTO PLANO en el doc, no
como formato.

## Anti-pattern (caso real del bug)

El usuario pidió "los estilos no están bonitos" sobre un plan de
ejercicios. El LLM mandó:

```json
[
  { "replace_text": { "find": "Calentamiento:", "replace": "- **Calentamiento:**" } },
  { "replace_text": { "find": "Ejercicios:", "replace": "- **Ejercicios:**" } },
  { "replace_text": { "find": "Sentadillas: 3 series", "replace": "  1. Sentadillas: 3 series" } }
]
```

Dos problemas:
1. `find: "Ejercicios:"` matcheó Día 1 y Día 3 (sin scope).
2. Aún si hubiera scope-eado, el resultado en el doc sería el TEXTO
   `- **Calentamiento:**` mostrado literal — Google Docs no re-renderea
   markdown que vos inyectaste como string.

## Pattern correcto (3 pasos)

### Paso 1 — leé el outline para saber DÓNDE está la sección

```json
{ "tool": "read_outline" }
```

Te devuelve:
```
{ paragraph: 1, kind: "heading1", text_preview: "Plan de Trabajo" }
{ paragraph: 2, kind: "heading2", text_preview: "Día 1: Entrenamiento" }
{ paragraph: 3, kind: "paragraph", text_preview: "Calentamiento: 10 min..." }
{ paragraph: 4, kind: "paragraph", text_preview: "Ejercicios:" }
{ paragraph: 5, kind: "paragraph", text_preview: "Sentadillas: 3 series..." }
{ paragraph: 6, kind: "paragraph", text_preview: "Flexiones: 3 series..." }
{ paragraph: 7, kind: "paragraph", text_preview: "Peso muerto: 3 series..." }
{ paragraph: 8, kind: "paragraph", text_preview: "Press de hombros: 3 series..." }
{ paragraph: 9, kind: "paragraph", text_preview: "Enfriamiento: 5 min..." }
{ paragraph: 10, kind: "heading2", text_preview: "Día 2: ..." }
```

Ahora sabés: Día 1 = párrafos 3-9. Día 2 empieza en 10.

### Paso 2 — leé el contenido del rango con `read_as_markdown`

```json
{ "tool": "read_as_markdown", "scope": { "type": "BetweenHeadings", "start": "Día 1", "end": "Día 2" } }
```

Te devuelve el markdown actual de la sección. Lo usás como base para
construir la versión nueva.

### Paso 3 — reemplazá la sección entera con `replace_section`

```json
{
  "tool": "replace_section",
  "scope": { "type": "BetweenHeadings", "start": "Día 1", "end": "Día 2" },
  "new_markdown": "## Día 1: Entrenamiento de Fuerza\n\n- **Calentamiento:** 10 minutos de trote suave o saltos.\n- **Ejercicios:**\n  1. Sentadillas: 3 series de 12 repeticiones\n  2. Flexiones de brazos: 3 series de 10 repeticiones\n  3. Peso muerto con mancuernas: 3 series de 10 repeticiones\n  4. Press de hombros: 3 series de 12 repeticiones\n- **Enfriamiento:** Estiramientos de 5-10 minutos."
}
```

Google Docs PARSEA el markdown del lado servidor (vía
`append_markdown`-like conversion). Los `**` se convierten en bold real,
los `-` en bullets reales, los `1.` `2.` en numbered list real. NO
quedan como caracteres en el doc.

## Variantes

### "Solo cambiá el estilo, no el contenido"

`replace_section` borra y re-crea — pierde named ranges, comentarios,
suggestions, etc. Si solo querés cambiar visual sin tocar contenido,
usá `style_text`:

```json
{
  "tool": "style_text",
  "scope": { "type": "Paragraph", "n": 3 },
  "style": { "bold": true }
}
```

Aplica bold a todo el párrafo 3 sin borrarlo.

### "Convertí 4 párrafos consecutivos en una numbered list"

`style_text` puede aplicar `bullet_preset: "NUMBERED_DECIMAL"` o
similar. Si lo soporta tu versión, es más limpio que `replace_section`
porque preserva el contenido. Si no lo soporta, `replace_section` con
markdown `1. ... 2. ... 3. ...`.

## Reglas

1. **Si la pregunta es "cambiale el FORMATO", el primer instinct
   debería ser `style_text` o `replace_section`, NUNCA `replace_text`
   con sintaxis markdown literal.**

2. **Si tenés que reformar más de 2-3 párrafos consecutivos, casi
   siempre conviene `replace_section` con markdown que cubra todo el
   rango.**

3. **Si el LLM hizo `gdocs_append_markdown` para generar el contenido
   inicial, el formato YA está aplicado.** El usuario quejándose de
   "estilos feos" puede estar viendo un problema de renderizado del
   frontend, no del doc en sí. Releé el outline + `read_as_markdown`
   antes de asumir que necesita re-formatear.

4. **NO inyectes markdown como texto con replace_text. Nunca. Es la
   causa del 90% de los "los estilos no están bonitos" reportados
   después.**
