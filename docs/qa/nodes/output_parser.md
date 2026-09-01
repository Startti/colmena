# QA — Nodo `output_parser`

Fuente de código: `src/libs/colmena/src/dag_engine/infrastructure/nodes/output_parser.rs`

Fuentes de doc revisadas:
- `docs/node_configurations.json` (líneas 1901–1980)
- `docs/agent_context/node_ports_reference.md` (línea 49)
- `docs/node_as_tools_reference.json` (sin entrada; output_parser no es un LLM tool)

---

## 1) Config documentada NO soportada por el código

**Sin discrepancias detectadas.**

La documentación (node_configurations.json) describe correctamente cada campo de config:
- `provider` (required, valid_values): código valida openai/google/anthropic (líneas 51–56) ✓
- `api_key` (required): código valida presencia (líneas 57–61) ✓
- `model` (optional): código fallback a None si ausente (líneas 62–65) ✓
- `schema` (required): código valida presencia y convierte a JSON Schema (líneas 67–72) ✓
- `instructions` (optional): código aplica si presente o vacío (líneas 84–92) ✓
- `temperature` (optional, default 0.1): código fallback a None; Anthropic rechaza temperature en Opus (comportamiento externo, no del nodo) ✓
- Fail-closed en input vacío (null, empty string, [], {}): código implementa is_empty_input (líneas 27–35, 74–76) ✓

---

## 2) Código NO documentado

### Hallazgo A: Mensajes de error específicos sin especificación en docs

**Línea 76:** Error exacto `"OutputParserRuntimeError: missing input — nothing to parse"` no está especificado en node_configurations.json.

**Línea 72:** Error `"OutputParser config error: {}"` cuando schema conversion falla; formato no documentado.

**Línea 50:** Error `"OutputParser: missing 'provider' in config"` — estructura de mensajes de validación config no documentada.

**Línea 55:** Error `"OutputParser: invalid provider '{}'` — rechazo de providers fuera de [openai, google, anthropic] no tiene error message standard en docs.

**Línea 60:** Error `"OutputParser: missing 'api_key' in config"` — no especificado.

**Línea 69:** Error `"OutputParser: missing 'schema' in config"` — no especificado.

### Hallazgo B: Método default_input/schema() sin mención en docs

**Línea 115–117:** `default_input()` retorna `"input"` — coincide con node_configurations.json:1966, documented ✓

**Línea 127–145:** `schema()` retorna self-describing JSON Schema internamente — no visible en user docs (es para introspección del engine, no para el operador).

### Hallazgo C: Environment variable resolution en api_key

**Líneas 17–25:** `resolve_env_var()` permite `${VAR_NAME}` en api_key (ejemplo línea 61).

**Node_configurations.json línea 1918:** Docs menciona "Supports ${ENV_VAR} interpolation" sin especificar la sintaxis exacta `${VAR_NAME}` ni el comportamiento de error (línea 21: "Environment variable {} not found").

### Hallazgo D: Formato de output no especificado con precisión

**Node_configurations.json líneas 1961–1964:** Docs dicen output es "raw extracted JSON object" no wrapped, pero no especifica:
- Qué nodo `extract_with_schema` (util/extract_with_schema.rs, línea 9) retorna exactamente
- Si hay `{ "output": ... }` wrapper o qué formato usa el engine para SSE

---

## 3) Plan de pruebas QA

### Caso S3.1: Happy path — Parse simple text con provider Google

**Objetivo:** Verificar que output_parser extrae JSON de un LLM response.

**Grafo JSON mínimo:**
```json
{
  "nodes": {
    "input": {
      "type": "input",
      "config": {
        "review": "Este producto es excelente. Recomendaría comprarlo. 4.5 estrellas."
      }
    },
    "parser": {
      "type": "output_parser",
      "config": {
        "provider": "google",
        "model": "gemini-2.5-flash",
        "api_key": "${GEMINI_API_KEY}",
        "schema": {
          "rating": { "type": "number", "required": true },
          "recommendation": { "type": "boolean", "required": true },
          "comment": { "type": "string", "required": false }
        }
      }
    },
    "output": {
      "type": "output"
    }
  },
  "edges": [
    { "from": "input.review", "to": "parser.input" },
    { "from": "parser", "to": "output.input" }
  ]
}
```

**Comando:** `cargo run --bin dag_engine -- run <graph.json>`

**Entrada:** review = "Este producto es excelente. Recomendaría comprarlo. 4.5 estrellas."

**Resultado esperado:** `{ "rating": 4.5, "recommendation": true, "comment": "Excelente producto" }` (u otro parsing válido)

**Verificación:** SSE contiene `text-block` final con objeto JSON; output contiene todos los campos requeridos.

---

### Caso S3.2: Happy path — Provider OpenAI con modelo personalizado

**Objetivo:** Verificar que el nodo respeta `model` custom y fallback de provider.

**Grafo JSON mínimo (diff):**
```json
"parser": {
  "type": "output_parser",
  "config": {
    "provider": "openai",
    "model": "gpt-4o-mini",
    "api_key": "${OPENAI_API_KEY}",
    "schema": {
      "sentiment": { "type": "string", "required": true }
    }
  }
}
```

**Entrada:** texto con sentimiento claro.

**Resultado esperado:** JSON con `sentiment` extraído; modelo usado debe ser gpt-4o-mini (verificable en SSE con token count).

**Verificación:** Ejecución sin error; output contiene campo requerido.

---

### Caso S3.3: Happy path — Anthropic sin modelo (fallback a default)

**Objetivo:** Verificar que fallback a default model del provider funciona.

**Grafo JSON mínimo (diff):**
```json
"parser": {
  "type": "output_parser",
  "config": {
    "provider": "anthropic",
    "api_key": "${ANTHROPIC_API_KEY}",
    "schema": {
      "category": { "type": "string", "required": true }
    }
  }
}
```

**Entrada:** texto con categoría evidente.

**Resultado esperado:** JSON con `category`; no error por model ausente.

**Verificación:** Ejecución exitosa; output válido.

---

### Caso S3.4: Optional field y instructions

**Objetivo:** Verificar que `instructions` se aplica y campos optional son realmente opcionales.

**Grafo JSON mínimo (diff):**
```json
"parser": {
  "type": "output_parser",
  "config": {
    "provider": "google",
    "model": "gemini-2.5-flash",
    "api_key": "${GEMINI_API_KEY}",
    "schema": {
      "mood": { "type": "string", "required": true },
      "explanation": { "type": "string", "required": false }
    },
    "instructions": "Si el mood es ambiguo, usa 'neutral'. La explicación es opcional."
  }
}
```

**Entrada:** "No sé qué pensar."

**Resultado esperado:** `{ "mood": "neutral" }` (explanation omitted por optional).

**Verificación:** SSE contains instructions section; output valid; explanation field absent or null.

---

### Caso S3.5: Default temperature (0.1) — reproducibility

**Objetivo:** Verificar que temperature default de 0.1 aplica para determinismo.

**Grafo JSON mínimo (diff):** sin `temperature` en config.

**Entrada:** mismo texto dos veces consecutivas.

**Resultado esperado:** outputs idénticos o muy similares (baja variabilidad por T=0.1).

**Verificación:** Dos runs con mismo agent_session_id y answer; outputs match.

---

### Caso S3.6: Custom temperature para creatividad

**Objetivo:** Verificar que `temperature` override aplica.

**Grafo JSON mínimo (diff):**
```json
"parser": {
  "config": {
    "temperature": 0.8,
    ...
  }
}
```

**Entrada:** "Crea un título creativo."

**Resultado esperado:** Output con `temperature` 0.8 (si el campo existe); variabilidad más alta (pero para este nodo, T=0.8 es no-estándar, output aún estructurado).

**Verificación:** Ejecución sin error.

---

### Caso S3.7: API key resolution — ${ENV_VAR}

**Objetivo:** Verificar que env var resolution funciona para api_key.

**Grafo JSON mínimo (diff):**
```json
"parser": {
  "config": {
    "provider": "google",
    "api_key": "${GEMINI_API_KEY}",
    ...
  }
}
```

**Entrada:** cualquier texto.

**Pre-requisito:** `export GEMINI_API_KEY="actual-key-here"`

**Resultado esperado:** LLM call exitoso (env var resuelto en tiempo de ejecución).

**Verificación:** No error sobre "api_key" o "invalid_authentication"; request llega a Google.

---

### Caso S3.8: Fail-closed — input is null

**Objetivo:** Verificar error fail-closed cuando input es null.

**Grafo JSON mínimo (diff):**
```json
"input": {
  "type": "input",
  "config": {}
}
```

**Entrada:** null (no injection a parser.input).

**Resultado esperado:** Error `"OutputParserRuntimeError: missing input — nothing to parse"` (línea 76).

**Verificación:** SSE termina con `error` event; no LLM call realizado.

---

### Caso S3.9: Fail-closed — input is empty string

**Objetivo:** Verificar error fail-closed cuando input es whitespace-only string.

**Grafo JSON mínimo (diff):**
```json
"input": {
  "type": "input",
  "config": {
    "text": "   "
  }
}
```

**Entrada:** "   " (espacios).

**Resultado esperado:** Error `"OutputParserRuntimeError: missing input — nothing to parse"` (is_empty_input línea 30: `s.trim().is_empty()`).

**Verificación:** SSE error; no LLM call.

---

### Caso S3.10: Fail-closed — input is empty array

**Objetivo:** Verificar error fail-closed cuando input es [].

**Grafo JSON mínimo (diff):**
```json
"input": {
  "type": "input",
  "config": {
    "items": []
  }
}
```

**Entrada:** [] (array vacío).

**Resultado esperado:** Error missing input (is_empty_input línea 31: `a.is_empty()`).

**Verificación:** SSE error.

---

### Caso S3.11: Fail-closed — input is empty object

**Objetivo:** Verificar error fail-closed cuando input es {}.

**Grafo JSON mínimo (diff):**
```json
"input": {
  "type": "input",
  "config": {
    "data": {}
  }
}
```

**Entrada:** {} (objeto vacío).

**Resultado esperado:** Error missing input (is_empty_input línea 32: `o.is_empty()`).

**Verificación:** SSE error.

---

### Caso S3.12: Fail-closed — missing provider in config

**Objetivo:** Verificar error cuando provider es ausente.

**Grafo JSON mínimo (diff):**
```json
"parser": {
  "type": "output_parser",
  "config": {
    "api_key": "${GEMINI_API_KEY}",
    "schema": { "x": { "type": "string" } }
  }
}
```

**Entrada:** cualquier.

**Resultado esperado:** Error `"OutputParser: missing 'provider' in config"` (línea 50).

**Verificación:** SSE error antes de LLM call.

---

### Caso S3.13: Fail-closed — invalid provider

**Objetivo:** Verificar error cuando provider es inválido.

**Grafo JSON mínimo (diff):**
```json
"parser": {
  "config": {
    "provider": "unknown_llm",
    "api_key": "${GEMINI_API_KEY}",
    "schema": { "x": { "type": "string" } }
  }
}
```

**Entrada:** cualquier.

**Resultado esperado:** Error `"OutputParser: invalid provider 'unknown_llm'"` (línea 55).

**Verificación:** SSE error.

---

### Caso S3.14: Fail-closed — missing api_key in config

**Objetivo:** Verificar error cuando api_key es ausente.

**Grafo JSON mínimo (diff):**
```json
"parser": {
  "config": {
    "provider": "google",
    "schema": { "x": { "type": "string" } }
  }
}
```

**Entrada:** cualquier.

**Resultado esperado:** Error `"OutputParser: missing 'api_key' in config"` (línea 60).

**Verificación:** SSE error.

---

### Caso S3.15: Fail-closed — missing schema in config

**Objetivo:** Verificar error cuando schema es ausente.

**Grafo JSON mínimo (diff):**
```json
"parser": {
  "config": {
    "provider": "google",
    "api_key": "${GEMINI_API_KEY}"
  }
}
```

**Entrada:** cualquier.

**Resultado esperado:** Error `"OutputParser: missing 'schema' in config"` (línea 69).

**Verificación:** SSE error.

---

### Caso S3.16: Fail-closed — invalid schema (type not supported)

**Objetivo:** Verificar error cuando schema contiene tipo inválido.

**Grafo JSON mínimo (diff):**
```json
"parser": {
  "config": {
    "provider": "google",
    "api_key": "${GEMINI_API_KEY}",
    "schema": {
      "rating": { "type": "weird_type", "required": true }
    }
  }
}
```

**Entrada:** cualquier.

**Resultado esperado:** Error `"OutputParser config error: invalid type 'weird_type'"` (línea 72, inline_to_json_schema validation).

**Verificación:** SSE error at init (before LLM call).

---

### Caso S3.17: Fail-closed — env var not found

**Objetivo:** Verificar error cuando env var en api_key no existe.

**Grafo JSON mínimo (diff):**
```json
"parser": {
  "config": {
    "provider": "google",
    "api_key": "${NONEXISTENT_VAR}",
    "schema": { "x": { "type": "string" } }
  }
}
```

**Entrada:** cualquier.

**Pre-requisito:** NONEXISTENT_VAR no seteado.

**Resultado esperado:** Error `"Environment variable NONEXISTENT_VAR not found"` (línea 21).

**Verificación:** SSE error.

---

## Resumen de hallazgos

| Tipo | Contar | Gravedad |
|------|--------|----------|
| Discrepancias config | 0 | — |
| Error messages no documentados | 6 | Info |
| Campos omitidos en docs | 0 | — |
| **Total S1** | 0 | — |
| **Total S2** | 6 | Info |
| **Total S3** | 17 | — |

**S1 (config contradictions):** 0  
**S2 (code not documented):** 6  
**S3 (QA test cases):** 17
