# QA — Nodo `reactor`

Fuente de código: `src/libs/colmena/src/dag_engine/infrastructure/nodes/reactor.rs`
Fuentes de doc revisadas:
- `docs/node_configurations.json` (config schema)
- `docs/node_as_tools_reference.json` (no aplica; el reactor no es un tool de LLM)
- `docs/agent_context/node_ports_reference.md` (puertos y outputs)
- `docs/developer_guide/20_orchestrator_architecture.md` (guía de orquestrador)

## 1) Config documentada NO soportada por el código

Sin discrepancias detectadas. La documentación en `node_configurations.json` describe todos los campos que el código realmente soporta: `provider`, `api_key`, `model`, `system_message`, `verbose`, y `texts`. No hay campos documentados que el código ignore ni rechace.

Nota: La temperatura está hardcodeada a 0.2 en el código (reactor.rs:238, `with_temperature(0.2)`), lo cual es correcto y coherente con la documentación que dice "Temperature is 0.2 for balanced quality". No es configurable por el usuario.

## 2) Código NO documentado

### 2.1) Campo `thinking_budget`
- **Ubicación**: `reactor.rs:239-241`
- **Comportamiento**: El código soporta un campo de config `thinking_budget` (tipo u64) que se pasa al LLM para limitar el presupuesto de tokens de pensamiento (extended thinking / o1-thinking en Claude/OpenAI). Si está presente, se asigna a la config LLM vía `with_thinking_budget()`.
- **Documentación**: NO aparece en `docs/node_configurations.json` (sección `config_fields` del reactor está incompleta).
- **Impacto para QA**: Operadores no saben que pueden configurar `thinking_budget` ni para qué sirve. Las pruebas E2E que intenten validar extended thinking no encontrarán documentación de cómo habilitarlo en el reactor.

### 2.2) Campo `streaming`
- **Ubicación**: `reactor.rs:275-307`
- **Comportamiento**: El código soporta un campo booleano `streaming` (default false) que, cuando está activado, habilita callbacks de token en tiempo real. Los tokens se emiten a través del observer como eventos `LlmToken`, junto con eventos de uso de tokens (`LlmUsage`, `LlmMessageStart`, `LlmMessageFinish`). La implementación es condicional: si `streaming=false`, no se registra callback; si `streaming=true` y hay observer, los tokens se forwarden.
- **Documentación**: NO aparece en `docs/node_configurations.json`.
- **Impacto para QA**: Los operadores no pueden activar streaming del reactor sin leer el código fuente. Las pruebas de integración que validen SSE events desde el reactor no tienen guía sobre cómo configurarlo.

### 2.3) Comportamiento de skip (no síntesis)
- **Ubicación**: `reactor.rs:170-185`
- **Comportamiento**: El nodo verifica si hay una síntesis sustantiva en los inputs (cualquier valor no-null y no-vacío EXCEPTO `system_message` y `user_request`). Si no hay síntesis, retorna `Value::Null` sin llamar al LLM. Esto se loguea como "⏩ [ReactorNode] Skipped — no synthesis to review yet."
- **Documentación**: NO aparece en `node_configurations.json` ni en `node_ports_reference.md`. La descripción en `node_configurations.json` dice que el reactor "evaluates the overall synthesis", pero no aclara qué pasa si NO hay síntesis.
- **Impacto para QA**: Operadores y agentes pueden pensar que el reactor siempre ejecuta y retorna un resultado estructurado, pero en realidad puede retornar `null` silenciosamente si no hay inputs de síntesis. Esto puede causar confusión en grafos donde la salida del reactor se espera que sea siempre un objeto con `result` y `extra_info`.

### 2.4) Tratamiento especial de `user_request`
- **Ubicación**: `reactor.rs:142-144`, `reactor.rs:170-175`
- **Comportamiento**: Los inputs que comienzan con `texts.` se tratan como contexto de síntesis. El input `system_message` se trata como instrucción adicional, NO como parte del contexto. El input `user_request` se ignora en la recolección de textos formateados, aunque se menciona en el comentario que indica "All keys that start with 'texts.' are treated as context" y se saltan `system_message` y (implícitamente) `user_request`. Esto está reflejado en la lógica de `has_synthesis` (línea 170), que excluye explícitamente ambas.
- **Documentación**: NO está claro en `node_configurations.json`. El schema de inputs dice "texts.<name>" son dinámicos, pero NO menciona que `user_request` será filtrado.
- **Impacto para QA**: Un operador podría pasar `user_request` como único input y esperar que el reactor lo revise, pero será ignorado (no contribuye a `has_synthesis`).

## 3) Plan de pruebas QA

### Caso 3.1: Happy path — síntesis completa y satisfactoria
**Objetivo**: Validar que el reactor acepta inputs de contexto y produce una respuesta estructurada cuando `task_ok=true`.

**Grafo mínimo**:
```json
{
  "nodes": [
    {
      "id": "input_synthesis",
      "node_type": "input",
      "config": {
        "data": {
          "texts.research": "Investigación: Roma tiene 2.7M habitantes, es capital de Italia.",
          "texts.recommendations": "Recomendaciones: Visitar Coliseo, Vaticano, Fuente de Trevi."
        }
      }
    },
    {
      "id": "reactor",
      "node_type": "reactor",
      "config": {
        "provider": "google",
        "api_key": "${GEMINI_API_KEY}",
        "model": "gemini-2.5-flash"
      }
    },
    {
      "id": "output",
      "node_type": "output"
    }
  ],
  "edges": [
    { "from": "input_synthesis", "to": "reactor.texts.research" },
    { "from": "input_synthesis", "to": "reactor.texts.recommendations" },
    { "from": "reactor", "to": "output" }
  ]
}
```

**Entrada**: (via grafo config)

**Resultado esperado**:
- El reactor ejecuta y retorna un objeto con:
  - `result`: string con la respuesta sintetizada
  - `extra_info.task_ok`: boolean (esperado: true)
  - `extra_info.add_tasks`: array vacío o con tareas opcionales
  - `extra_info.suspend`: boolean (esperado: false)
  - `extra_info.question`: string vacío o null
  - `extra_info.__colmena_status`: "OK"

**Cómo verificar**:
```bash
source .env  # Incluye GEMINI_API_KEY
cargo run --bin dag_engine -- run tests/qa/reactor_happy_path.json | \
  jq '.result.extra_info | {task_ok, suspend, __colmena_status}'
# Debe mostrar: { task_ok: true, suspend: false, __colmena_status: "OK" }
```

---

### Caso 3.2: Sin síntesis — nodo salta y retorna null
**Objetivo**: Validar que el reactor retorna `null` cuando NO hay inputs de síntesis (solo `system_message` y/o `user_request`).

**Grafo mínimo**:
```json
{
  "nodes": [
    {
      "id": "input_empty",
      "node_type": "input",
      "config": {
        "data": {
          "system_message": "Eres un revisor imparcial.",
          "user_request": "¿Hay algo para revisar?"
        }
      }
    },
    {
      "id": "reactor",
      "node_type": "reactor",
      "config": {
        "provider": "google",
        "api_key": "${GEMINI_API_KEY}",
        "model": "gemini-2.5-flash"
      }
    },
    {
      "id": "output",
      "node_type": "output"
    }
  ],
  "edges": [
    { "from": "input_empty", "to": "reactor" },
    { "from": "reactor", "to": "output" }
  ]
}
```

**Resultado esperado**:
- El reactor salta (logs muestran "⏩ [ReactorNode] Skipped — no synthesis to review yet.")
- Output final es `null`

**Cómo verificar**:
```bash
cargo run --bin dag_engine -- run tests/qa/reactor_no_synthesis.json | \
  jq 'if . == null then "PASS: returned null" else "FAIL: \(.)" end'
```

---

### Caso 3.3: Streaming habilitado — tokens emitidos en SSE
**Objetivo**: Validar que el reactor emite eventos SSE de tokens cuando `streaming=true`.

**Grafo mínimo**:
```json
{
  "nodes": [
    {
      "id": "input_synthesis",
      "node_type": "input",
      "config": {
        "data": {
          "texts.context": "Pregunta: ¿Cuál es la capital de Francia?\nRespuesta: París."
        }
      }
    },
    {
      "id": "reactor",
      "node_type": "reactor",
      "config": {
        "provider": "google",
        "api_key": "${GEMINI_API_KEY}",
        "model": "gemini-2.5-flash",
        "streaming": true
      }
    },
    {
      "id": "output",
      "node_type": "output"
    }
  ],
  "edges": [
    { "from": "input_synthesis", "to": "reactor" },
    { "from": "reactor", "to": "output" }
  ]
}
```

**Resultado esperado**:
- El reactor ejecuta con `streaming=true`
- SSE stream contiene múltiples eventos `data: {"type":"llm-token","token":"..."}` durante la ejecución
- SSE final incluye `data: {"type":"llm-usage",...}` con token counts

**Cómo verificar**:
```bash
cargo run --bin dag_engine -- run tests/qa/reactor_streaming.json 2>&1 | \
  grep -c 'llm-token'
# Debe ser > 0 si streaming está funcionando
```

---

### Caso 3.4: thinking_budget configurado (extended thinking)
**Objetivo**: Validar que el reactor acepta y pasa `thinking_budget` a la config LLM para modelos que soportan extended thinking.

**Grafo mínimo**:
```json
{
  "nodes": [
    {
      "id": "input_complex",
      "node_type": "input",
      "config": {
        "data": {
          "texts.problem": "Problema: Optimiza una ruta de viaje de 10 ciudades minimizando distancia."
        }
      }
    },
    {
      "id": "reactor",
      "node_type": "reactor",
      "config": {
        "provider": "google",
        "api_key": "${GEMINI_API_KEY}",
        "model": "gemini-2.5-flash",
        "thinking_budget": 5000
      }
    },
    {
      "id": "output",
      "node_type": "output"
    }
  ],
  "edges": [
    { "from": "input_complex", "to": "reactor" },
    { "from": "reactor", "to": "output" }
  ]
}
```

**Resultado esperado**:
- El reactor ejecuta con `thinking_budget=5000`
- La request LLM incluye el parámetro de presupuesto de pensamiento
- SSE puede contener eventos de `thinking-delta` si el modelo emite bloques de pensamiento

**Cómo verificar**:
```bash
source .env
cargo run --bin dag_engine -- run tests/qa/reactor_thinking_budget.json 2>&1 | \
  grep -E '(thinking|budget)' || echo "Nota: Verificar en logs de debug"
# El test pasa si no hay error; el comportamiento real del thinking depende del modelo
```

---

### Caso 3.5: system_message adicional en config
**Objetivo**: Validar que `system_message` en config se apenda al prompt del reactor.

**Grafo mínimo**:
```json
{
  "nodes": [
    {
      "id": "input_synthesis",
      "node_type": "input",
      "config": {
        "data": {
          "texts.draft": "Borrador: 'El cambio climático es un reto global que requiere acción inmediata'."
        }
      }
    },
    {
      "id": "reactor",
      "node_type": "reactor",
      "config": {
        "provider": "google",
        "api_key": "${GEMINI_API_KEY}",
        "model": "gemini-2.5-flash",
        "system_message": "Valida que la respuesta esté estructurada como un párrafo con máximo 3 oraciones."
      }
    },
    {
      "id": "output",
      "node_type": "output"
    }
  ],
  "edges": [
    { "from": "input_synthesis", "to": "reactor" },
    { "from": "reactor", "to": "output" }
  ]
}
```

**Resultado esperado**:
- El reactor ejecuta con las instrucciones adicionales
- `result` contiene una respuesta validada siguiendo la restricción de "máximo 3 oraciones"

**Cómo verificar**:
```bash
source .env
cargo run --bin dag_engine -- run tests/qa/reactor_custom_system.json | \
  jq '.result | split(".") | length'
# Debe ser <= 3 si el modelo respetó la instrucción
```

---

### Caso 3.6: suspend=true — usuario solicita aclaración
**Objetivo**: Validar que cuando el reactor detecta que necesita input del usuario (suspend=true), emite la pregunta y marca el estado como SUSPENDED.

**Grafo mínimo**:
```json
{
  "nodes": [
    {
      "id": "input_unclear",
      "node_type": "input",
      "config": {
        "data": {
          "texts.incomplete": "Tarea incompleta: 'El usuario no especificó...' (falta información clave)."
        }
      }
    },
    {
      "id": "reactor",
      "node_type": "reactor",
      "config": {
        "provider": "google",
        "api_key": "${GEMINI_API_KEY}",
        "model": "gemini-2.5-flash",
        "system_message": "Si la síntesis tiene gaps o ambigüedad, suspende y pregunta al usuario qué falta."
      }
    },
    {
      "id": "output",
      "node_type": "output"
    }
  ],
  "edges": [
    { "from": "input_unclear", "to": "reactor" },
    { "from": "reactor", "to": "output" }
  ]
}
```

**Resultado esperado**:
- `extra_info.suspend`: true
- `extra_info.question`: string no vacío con la pregunta al usuario
- `extra_info.__colmena_status`: "SUSPENDED"

**Cómo verificar**:
```bash
source .env
cargo run --bin dag_engine -- run tests/qa/reactor_suspend.json | \
  jq '{suspend: .extra_info.suspend, has_question: (.extra_info.question | length > 0), status: .extra_info.__colmena_status}'
# Debe mostrar: { suspend: true, has_question: true, status: "SUSPENDED" }
```

---

### Caso 3.7: add_tasks — reactor propone tareas adicionales
**Objetivo**: Validar que el reactor puede proponer tareas adicionales (incluyendo bridge tasks) cuando detect gaps.

**Grafo mínimo**:
```json
{
  "nodes": [
    {
      "id": "input_missing_steps",
      "node_type": "input",
      "config": {
        "data": {
          "texts.partial_plan": "Plan: 1. Investigar. 2. [falta paso]. 4. Escribir. 5. Revisar."
        }
      }
    },
    {
      "id": "reactor",
      "node_type": "reactor",
      "config": {
        "provider": "google",
        "api_key": "${GEMINI_API_KEY}",
        "model": "gemini-2.5-flash",
        "system_message": "Si detectas pasos faltantes, propón tareas adicionales en add_tasks con {task, assigned_to}."
      }
    },
    {
      "id": "output",
      "node_type": "output"
    }
  ],
  "edges": [
    { "from": "input_missing_steps", "to": "reactor" },
    { "from": "reactor", "to": "output" }
  ]
}
```

**Resultado esperado**:
- `extra_info.add_tasks`: array no vacío con objetos `{task, assigned_to}`
- Cada objeto tiene strings válidos en ambos campos

**Cómo verificar**:
```bash
source .env
cargo run --bin dag_engine -- run tests/qa/reactor_add_tasks.json | \
  jq '.extra_info.add_tasks | length > 0'
# Debe ser true si el reactor propuso tareas
```

---

### Caso 3.8: Validación de JSON parse error — response inválida del LLM
**Objetivo**: Validar que el reactor falla gracefully cuando el LLM no retorna JSON válido.

**Grafo mínimo**:
```json
{
  "nodes": [
    {
      "id": "input_synthesis",
      "node_type": "input",
      "config": {
        "data": {
          "texts.content": "Contenido a revisar."
        }
      }
    },
    {
      "id": "reactor",
      "node_type": "reactor",
      "config": {
        "provider": "mock",
        "api_key": "mock-key",
        "model": "mock-model"
      }
    },
    {
      "id": "output",
      "node_type": "output"
    }
  ],
  "edges": [
    { "from": "input_synthesis", "to": "reactor" },
    { "from": "reactor", "to": "output" }
  ]
}
```

**Resultado esperado**:
- Si el LLM (o un mock configurado) retorna texto NO-JSON, el reactor retorna un error Err(...) que no es capturado (fail-closed)
- El mensaje de error menciona "Failed to parse LLM JSON"

**Cómo verificar**:
```bash
# Esto requeriría un mock personalizado que retorne no-JSON
# O forzar la prueba alterando la respuesta en desarrollo
cargo run --bin dag_engine -- run tests/qa/reactor_json_error.json 2>&1 | \
  grep -i 'Failed to parse' && echo "PASS: Error detectado" || echo "FAIL: No error"
```

---

### Caso 3.9: Escape de caracteres especiales ($) en JSON
**Objetivo**: Validar que el reactor limpia escapes inválidos de `\$` en JSON que algunos LLMs emiten.

**Grafo mínimo**:
```json
{
  "nodes": [
    {
      "id": "input_with_dollars",
      "node_type": "input",
      "config": {
        "data": {
          "texts.pricing": "Precios: Opción A cuesta \\$100, Opción B \\$200."
        }
      }
    },
    {
      "id": "reactor",
      "node_type": "reactor",
      "config": {
        "provider": "google",
        "api_key": "${GEMINI_API_KEY}",
        "model": "gemini-2.5-flash"
      }
    },
    {
      "id": "output",
      "node_type": "output"
    }
  ],
  "edges": [
    { "from": "input_with_dollars", "to": "reactor" },
    { "from": "reactor", "to": "output" }
  ]
}
```

**Resultado esperado**:
- El reactor parsea exitosamente el JSON incluso si contiene `\$` escapados (que son inválidos en JSON)
- El resultado contiene el contenido con `$` restaurado

**Cómo verificar**:
```bash
source .env
cargo run --bin dag_engine -- run tests/qa/reactor_dollar_escape.json 2>&1 | \
  jq '.result' | grep -q '\$' && echo "PASS: $ preservado" || echo "VERIFY: Check output"
```

---

### Caso 3.10: Temperature hardcoded a 0.2
**Objetivo**: Validar que la temperatura del reactor es siempre 0.2 (no configurable) para consistencia determinística.

**Prueba**: Inspección de código + ejecución múltiple

**Verificación**:
```bash
# Verificar en el código que temperature está hardcoded
grep -n 'with_temperature(0.2)' src/libs/colmena/src/dag_engine/infrastructure/nodes/reactor.rs
# Debe retornar la línea reactor.rs:238

# Ejecutar el mismo grafo 3 veces y verificar que los tokens son iguales
source .env
for i in {1..3}; do
  cargo run --bin dag_engine -- run tests/qa/reactor_determinism.json 2>/dev/null | \
    jq -r '.extra_info.thinking_tokens // .result' | md5sum
done
# Debe mostrar 3 hashes idénticos si la temperatura es constante
```
