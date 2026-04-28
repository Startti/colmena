# Python Tool — Ideas Backlog

Ideas surgidas del brainstorm de python_script sandboxed tool (2026-04-27).
**No están en scope del sprint actual.** Quedan aquí para diseño futuro.

---

## ✅ Implementado: Python Sandbox Simple

Ver spec: `2026-04-27-python-sandbox-tool-design.md`

- `sandbox_mode: "restricted"` en `python_script` node
- AST validator (import whitelist + builtins blocklist) via Python `ast` stdlib
- Timeout via `tokio::time::timeout`
- Error messages informativos al LLM para retry
- `node_schema+fixed` para `sandbox_mode`, `code` expuesto al LLM

---

## 💡 Idea 1 — Tool Piping (sin intermediario LLM)

**Concepto:** El LLM configura una cadena `HTTP tool → Python filter → resultado filtrado al LLM`. El LLM nunca ve el raw del HTTP. Ahorra tokens masivamente en APIs con respuestas grandes (ej. Amadeus: 50 vuelos × 20 campos → 50 × 2 campos).

**Motivación principal:** Reducción de tokens. Para APIs como Amadeus el raw response puede costar 500+ tokens; el output filtrado por Python puede ser 20 tokens.

**Casos de uso:**
- Buscar vuelos → filtrar a [precio, fecha, id] → LLM elige → fetch detalle completo del vuelo elegido
- Consultar catálogo de productos → filtrar a [nombre, precio] → LLM recomienda
- SQL query masivo → Python agrupa/cuenta → LLM interpreta el resumen

**Preguntas abiertas:**
- ¿Quién define el pipeline: el DAG designer en JSON, o el LLM en runtime?
- Si el LLM lo define en runtime, ¿cómo especifica el código de filtro antes de ver los datos?
- ¿Se puede encadenar más de 2 tools? (HTTP → Python → otro Python → LLM)
- ¿Los side effects del HTTP call cuentan doble si hay dry run?

**Dependencia:** Requiere Idea 3 (schema caching) para que el LLM sepa qué filtrar sin ver los datos primero.

---

## 💡 Idea 2 — Code Library (reuso de snippets Python)

**Concepto:** Tabla en base de datos con snippets Python + descripción + metadata. El LLM puede buscar en la librería antes de escribir código nuevo. Si encuentra uno que sirve, lo reutiliza. Si no, genera uno nuevo y opcionalmente lo guarda.

**Motivación:** Evitar que el LLM regenere el mismo código en cada ejecución. Código validado y testeado reutilizable entre sesiones y grafos.

**Schema propuesto:**
```sql
CREATE TABLE python_snippets (
  id          TEXT PRIMARY KEY,
  name        TEXT NOT NULL,
  description TEXT NOT NULL,        -- para búsqueda semántica o keyword
  code        TEXT NOT NULL,
  input_vars  JSONB,                 -- variables esperadas y sus tipos
  output_type TEXT,                  -- tipo del 'output' retornado
  tags        TEXT[],
  created_at  TIMESTAMP,
  used_count  INTEGER DEFAULT 0
);
```

**Tools necesarias:**
- `search_python_snippets(query)` → retorna snippets relevantes
- `save_python_snippet(name, description, code, input_vars)` → guarda para reuso
- `run_saved_snippet(snippet_id, variables)` → ejecuta directamente

**Preguntas abiertas:**
- ¿Búsqueda por keyword o embeddings semánticos?
- ¿Snippets son privados por sesión/usuario o compartidos globalmente?
- ¿Cómo se valida que un snippet guardado es seguro (sandbox check antes de guardar)?
- ¿Quién tiene permisos para guardar vs. solo leer?

---

## 💡 Idea 3 — Schema Caching de Nodos Upstream

**Concepto:** Tool tipo `load_node_output("nombre_nodo")` análoga a `load_skill`. El LLM la llama para conocer la *estructura* del output de un nodo upstream sin recibir todos los datos. Solo se auto-activa cuando hay un `python_script` en las tool_configurations.

**Comportamiento híbrido:**
- Si el DAG designer declaró un schema estático → retorna ese
- Si no hay schema declarado → infiere desde el output real de la última ejecución del nodo en sesión (lazy, primer run captura y cachea)

**Lo que retorna al LLM:**
```json
{
  "node": "fetch_products",
  "variable_name": "rows",
  "schema": {
    "type": "array",
    "item_fields": {
      "id": "number",
      "name": "string",
      "price": "number",
      "stock": "number",
      "active": "boolean",
      "category": { "id": "number", "name": "string" }
    }
  },
  "sample": [
    { "id": 1, "name": "iPhone 9", "price": 549.99, "stock": 94, "active": true }
  ],
  "total_items_in_last_run": 30
}
```

**Campo nuevo en tool config:**
```json
"variable_schemas": {
  "rows": {
    "description": "Lista de productos del catálogo",
    "schema": { "id": "number", "name": "string", "price": "number" }
  }
}
```
Si no se declara, el engine infiere en el primer run real.

**Preguntas abiertas:**
- ¿Dónde se cachea el schema: en sesión (efímero), en SQLite (persistente por graph), o en memoria del proceso?
- ¿Cuántos items de sample incluir? ¿1, 3, configurable?
- ¿Qué pasa con nodos que aún no han ejecutado en la sesión actual?
- ¿El schema se invalida si el endpoint cambia su respuesta?

---

## 💡 Idea 4 — Auto-descripción base del tool Python

**Concepto:** Cuando `python_script` se usa como tool, auto-inyectar una descripción base al LLM que explique:
- Siempre retornar como diccionario: `output = {"result": value, "count": n}`
- Qué variables están disponibles (de `context`)
- Qué imports están permitidos
- Ejemplos de patrones comunes

**Motivación:** El usuario no debería tener que escribir esto manualmente en cada tool config. Debe ser parte de la infraestructura del nodo.

**Implementación propuesta:**
- El `python_script` node genera automáticamente una `base_description` cuando `sandbox_mode: "restricted"`
- Se concatena con la `description` del tool config (si existe)
- El usuario solo escribe la parte específica del dominio

**Formato preferido de output (siempre diccionario):**
```python
# Retornar un valor simple
output = {"result": 42}

# Retornar múltiples valores
output = {"count": 5, "total": 1250.0, "filtered_ids": [1, 3, 7]}

# Retornar lista procesada
output = {"items": [{"id": 1, "name": "A"}, {"id": 3, "name": "C"}]}
```

**Preguntas abiertas:**
- ¿La base description se puede desactivar con un flag?
- ¿Se incluye en todos los casos o solo cuando `sandbox_mode: "restricted"`?

---

## Orden de implementación sugerido

1. ✅ **Sandbox simple** — implementar ahora
2. **Idea 4** (auto-descripción base) — fácil, alto impacto, no requiere infraestructura nueva
3. **Idea 3** (schema caching) — medio, habilita las otras dos
4. **Idea 1** (tool piping) — complejo, depende de Idea 3
5. **Idea 2** (code library) — complejo, depende de SQLite/PostgreSQL setup
