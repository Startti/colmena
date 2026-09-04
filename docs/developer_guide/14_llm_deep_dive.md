# 📚 LLM Node - Guía Completa de Parámetros

## 🎯 Visión General

El nodo LLM (`type: "llm_call"`) es el corazón de la orquestación de agentes en Colmena. Permite comunicarse con modelos de lenguaje (OpenAI, Gemini, Anthropic, etc.) con soporte para memoria conversacional, herramientas (tool calling), y streaming.

---

## 🔧 PARÁMETROS DE ENTRADA (config + inputs)

El nodo LLM resuelve parámetros usando **3 niveles de precedencia**:

```
Level 1 (Highest Priority): inputs (del nodo anterior)
Level 2: config (del JSON del grafo)
Level 3 (Default): Environment variables o valores por defecto
```

### **NIVEL 1: PARÁMETROS CRÍTICOS** (Requeridos)

#### `provider`
- **Tipo:** `string`
- **Valores válidos:** `"openai"`, `"google"`, `"anthropic"`, `"mock"` (testing)
- **Fuente:** `inputs.provider` → `config.provider`
- **Descripción:** El proveedor de LLM a usar
- **Ejemplos:**
  ```json
  {
    "config": {
      "provider": "openai"
    }
  }
  ```

#### `api_key`
- **Tipo:** `string`
- **Formato:** `${ENV_VAR}` o hardcoded
- **Fuente:** `inputs.api_key` → `config.api_key`
- **Descripción:** Clave API para autenticar con el proveedor
- **Ejemplos:**
  ```json
  {
    "config": {
      "api_key": "${OPENAI_API_KEY}"
    }
  }
  ```

#### `prompt` (O `task`)
- **Tipo:** `string` o `JSON` (arrays, objects, etc.)
- **Fuente:** `inputs.prompt` → `config.prompt` → fallback a `inputs.task` → `config.task`
- **Descripción:** El mensaje/pregunta para enviar al LLM
- **Comportamiento especial:**
  - Si es `string`: se interpolan `{{variable}}` y `${context.variable}`
  - Si es `JSON` (array/object): se serializa a pretty-print antes de enviar
  - Si es `null` o está vacío: el nodo se salta (retorna `Value::Null`)
- **Ejemplos:**
  ```json
  {
    "inputs": {
      "prompt": "Analiza esto: {{data}}"
    }
  }
  ```

---

### **NIVEL 2: PARÁMETROS OPCIONALES DE CONFIGURACIÓN**

#### `model`
- **Tipo:** `string`
- **Fuente:** `inputs.model` → `config.model`
- **Descripción:** Nombre específico del modelo a usar
- **Valores típicos:**
  - OpenAI: `"gpt-4o"`, `"gpt-4o-mini"`, `"gpt-5"`, `"gpt-5-mini"`, `"gpt-5.6"`
  - Gemini: `"gemini-2.5-flash"` (recomendado), `"gemini-pro"`
  - Anthropic: `"claude-3-5-sonnet"`, `"claude-3-haiku"`, `"claude-opus"`
- **Ejemplos:**
  ```json
  {
    "config": {
      "provider": "openai",
      "model": "gpt-4o-mini"
    }
  }
  ```

#### OpenAI — ruteo automático a la Responses API (familia `gpt-5`)

Los modelos de razonamiento `gpt-5*` (`gpt-5`, `gpt-5-mini`, `gpt-5.6`, …)
**rechazan las function tools en `/v1/chat/completions`** salvo que
`reasoning_effort` sea `'none'`. OpenAI devuelve un `400`:

```
Function tools with reasoning_effort are not supported for gpt-5.6 in
/v1/chat/completions. To use function tools, use /v1/responses or set
reasoning_effort to 'none'.
```

Como Colmena inyecta la tool `recall_history` en **cada** turno de agente, un
grafo `gpt-5*` casi siempre lleva al menos una tool. Por eso el adapter OpenAI
**enruta automáticamente a `/v1/responses`** cuando el modelo es `gpt-5*` **y**
la request lleva tools — preservando razonamiento **y** tool calling juntos. No
hay que configurar nada; es transparente. El resto de modelos (`gpt-4o`,
`gpt-4.1`, …) siguen usando `/v1/chat/completions`.

Ajustes automáticos del ruteo `gpt-5*`: `temperature`/`top_p` se **omiten** (esa
familia solo acepta el default; dejarlos en el grafo es inofensivo),
`max_tokens` viaja como `max_output_tokens`, y `thinking_budget` se mapea a
`reasoning.effort` (`low` ≤1000 < `medium` ≤5000 < `high`). Streaming, tool
calling (incluye batches paralelos), memoria y subgraphs funcionan igual que en
chat completions. Verificado E2E:
[`tests/graphs/agents/gpt5_responses_tools_e2e.json`](../../tests/graphs/agents/gpt5_responses_tools_e2e.json).

#### `system_message`
- **Tipo:** `string` (opcional)
- **Fuente:** `inputs.system_message` → `config.system_message`
- **Descripción:** Instrucciones del sistema para el LLM (rol, comportamiento, etc.)
- **Comportamiento especial:**
  - Solo se agrega si NO hay historial previo en memoria
  - Soporta `{{variable}}` interpolación
  - Se añade como mensaje de rol "system" al inicio
- **Ejemplos:**
  ```json
  {
    "config": {
      "system_message": "You are a security analyst. Analyze the data and identify risks."
    }
  }
  ```

#### `skills` (on-demand knowledge loading)

The LLM node exposes built-in and/or user-provided markdown skill packages via a synthetic `load_skill` tool. The LLM decides at runtime which skills to activate. There are **three** independent config fields, all optional and combinable (the engine de-duplicates by skill name):

- `skills: ["name1", "name2", …]` — flat list of skill names (built-in or already-discovered). Read first.
- `skills_path: "/abs/or/relative/dir"` — single parent directory; every immediate subdir that contains a `SKILL.md` becomes a skill. Read after `skills`.
- `skills_paths: ["/dir1", "/dir2", …]` — array form of `skills_path` for multiple roots.

Missing `skills_path` directory → hard error at graph load. Empty directory → no error, contributes nothing. See [24_skills.md](24_skills.md) for the full guide (auto-discovery, allowed-dirs whitelist, layered tool context, 64 KB cap).

#### `lazy_tool_loading` (on-demand tool schemas)

Boolean optional flag (`true | false`, default `false`). When enabled, tools in `tool_configurations` are exposed via a lightweight catalog inside the synthetic `describe_tool`; full schemas are revealed on demand and the discovered tool becomes callable on the next turn. Tools with `eager: true` remain always-on. See [29_lazy_tool_loading.md](29_lazy_tool_loading.md) for the full guide.

#### `instructions`
- **Tipo:** `string` (opcional)
- **Fuente:** `config.instructions`
- **Descripción:** Instrucciones adicionales que se combinan con `system_message`
- **Ejemplos:**
  ```json
  {
    "config": {
      "instructions": "Be thorough. Focus on vulnerabilities."
    }
  }
  ```

#### `temperature`
- **Tipo:** `number` (0.0 a 2.0)
- **Fuente:** `inputs.temperature` → `config.temperature`
- **Descripción:** Creatividad de la respuesta (0 = determinista, 2 = muy creativo)
- **Nota `gpt-5*`:** la familia de razonamiento OpenAI solo acepta el valor por
  defecto; Colmena **omite** `temperature`/`top_p` para esos modelos (ver
  "OpenAI — ruteo automático a la Responses API"). Dejarlos en el grafo es
  inofensivo: se ignoran.

#### `max_tokens`
- **Tipo:** `integer` (> 0)
- **Fuente:** `inputs.max_tokens` → `config.max_tokens`
- **Descripción:** Número máximo de tokens en la respuesta

#### `top_p`
- **Tipo:** `number` (0.0 a 1.0)
- **Fuente:** `inputs.top_p` → `config.top_p`
- **Descripción:** Nucleus sampling (diversidad de tokens)

#### `frequency_penalty` / `presence_penalty`
- **Tipo:** `number` (-2.0 a 2.0)
- **Descripción:** Penalizaciones para tokens frecuentes/vistos

#### `files` (adjuntos: imágenes y documentos)
- **Tipo:** `array<FileEntry>` (opcional)
- **Fuente:** `inputs.files` → `config.files`
- **Descripción:** Archivos adjuntos al request — imágenes para visión, PDFs para extracción/análisis de documentos. Soporta inline (base64), URL firmada (GCS) y path local (legacy).
- **Schema de cada entrada:**
  ```json
  {
    "id":         "doc-abc-123",       // requerido si usas `url` (clave de cache)
    "mime_type":  "application/pdf",
    "filename":   "report.pdf",
    "size_bytes": 47185920,            // hint, no ground truth
    "data":       null,                // base64 puro, < 30 MB
    "url":        "https://storage.googleapis.com/.../path?X-Goog-Signature=..."
    // alternativa: "path": "/local/path.pdf" (solo dev/tests, < 30 MB)
  }
  ```
- **Reglas (mutuamente excluyentes, prioridad `data > url > path`):**
  - `data`: inline base64 — solo válido si raw < 30 MB. El emisor decide el threshold a 30 MB.
  - `url`: signed URL HTTPS a GCS. **Requiere `id`** (es la llave de cache `(document_id, provider)`). TTL típico de la URL: 6 h.
  - `path`: legacy local, solo dev/tests, < 30 MB.
- **Comportamiento por provider** (auto-detectado):

  | Provider  | Imagen | PDF / documento |
  |-----------|--------|-----------------|
  | Anthropic | URL passthrough (no upload) | Files API + `file_id` |
  | OpenAI    | URL passthrough en chat completions | Files API + `file_id` en Responses API |
  | Gemini    | Files API resumable upload | Files API resumable (chunks de 8 MB) |

- **Cache**: si `DATABASE_URL` está set, los uploads se cachean por `(id, provider)` en la tabla `provider_file_cache`. Re-runs con el mismo `id` saltan el download/upload completos. Si `DATABASE_URL` no está set, cada run sube de nuevo (degradación graceful).
- **Errores específicos**:
  - `DataFieldTooLarge { size }` — `data` con `size_bytes > 30 MB`. Bug del emisor.
  - `UrlWithoutDocumentId` — `url` presente sin `id`. Bug de contrato.
  - `SignedUrlFetchFailed { status }` — GCS rechazó GET (URL expirada).
  - `InvalidMimeType { mime, message }` — mime mal formado (precondición del caller).
  - `FileApiUploadFailed { provider, message }` — provider rechazó upload (cuota, key inválida).
  - `ProviderFileNotFound { provider_file_id }` — archivo borrado del provider; se recupera automáticamente con snapshot+retry.
  - `AllFilesFailedToResolve` — todos los archivos del request fallaron.
- **Ejemplos:**
  ```json
  {
    "config": {
      "files": [
        {
          "id": "report-q1-2026",
          "mime_type": "application/pdf",
          "filename": "q1.pdf",
          "size_bytes": 52428800,
          "url": "https://storage.googleapis.com/bucket/q1.pdf?X-Goog-Signature=..."
        }
      ]
    }
  }
  ```

  Inline para archivos chicos:
  ```json
  {
    "files": [
      {
        "mime_type": "image/jpeg",
        "filename": "screenshot.jpg",
        "data": "<base64-sin-prefijo-data:>"
      }
    ]
  }
  ```
- **Más detalle**: ver [28_large_files_api.md](28_large_files_api.md) para arquitectura interna, flujo de resolución, límites de modelo por provider, y trazabilidad con `COLMENA_VERBOSE=1`.

---

### **NIVEL 3: PARÁMETROS DE MEMORIA CONVERSACIONAL**

#### `session_id`
- **Tipo:** `string` (opcional)
- **Fuente:** `inputs.__colmena_session_id` → `inputs.session_id` → `config.session_id`
- **Descripción:** ID único de la sesión/conversación. Si no se proporciona, se genera un UUID aleatorio
- **Comportamiento:**
  - Si está presente: se carga el historial de conversación previo
  - Si NO está: se crea una conversación efímera

#### `connection_url`
- **Tipo:** `string` (optional)
- **Fuente:** `inputs.connection_url` → `config.connection_url`
- **Descripción:** URL de la base de datos para almacenar historial de conversación
- **Formatos válidos:**
  - PostgreSQL: `"postgres://user:pass@localhost:5432/colmena"`
  - SQLite: `"sqlite:///path/to/file.db"`

---

### **NIVEL 4: PARÁMETROS DE STREAMING**

#### `stream`
- **Tipo:** `boolean` (default: `false`)
- **Fuente:** `inputs.stream` → `config.stream`
- **Descripción:** Habilita respuestas en tiempo real (token por token)

#### `verbose`
- **Tipo:** `boolean` (default: `false`)
- **Fuente:** `inputs.verbose` → `config.verbose`
- **Descripción:** Imprime en consola: prompt, system_message, y respuesta completa

---

### **NIVEL 5: PARÁMETROS DE TOOL CALLING**

#### `enabled_tools`
- **Tipo:** `string` o `array<string>`
- **Fuente:** `inputs.enabled_tools` → `config.enabled_tools`
- **Descripción:** Herramientas que el LLM puede usar
- **Valores:**
  - `"*"` — Habilita TODAS las herramientas disponibles
  - `["tool1", "tool2"]` — Array de nombres específicos

#### `tool_configurations`
- **Tipo:** `Map<string, ToolConfiguration>`
- **Fuente:** `inputs.tool_configurations` → `config.tool_configurations`
- **Descripción:** Configuración detallada para herramientas HTTP

**RECOMENDADO: Usa `node_schema` (nueva opción, fuente única de verdad)**

```json
{
  "enabled_tools": ["search_flights"],
  "tool_configurations": {
    "search_flights": {
      "name": "search_flights",
      "node_type": "http_request",
      "description": "Search for flights from origin to destination",
      "node_schema": {
        "base_url": {
          "type": "string",
          "fixed": "https://api.amadeus.com"
        },
        "endpoint": {
          "type": "string",
          "fixed": "/v2/shopping/flight-offers"
        },
        "method": {
          "type": "string",
          "fixed": "GET"
        },
        "bearer_token": {
          "type": "string",
          "fixed": "${context.amadeus_token}"
        },
        "query_params": {
          "type": "object",
          "properties": {
            "max": {
              "type": "string",
              "fixed": "5"
            },
            "originLocationCode": {
              "type": "string",
              "required": true,
              "description": "Origin IATA code (e.g., MAD)"
            },
            "destinationLocationCode": {
              "type": "string",
              "required": true,
              "description": "Destination IATA code (e.g., BCN)"
            },
            "departureDate": {
              "type": "string",
              "required": true,
              "description": "Departure date (YYYY-MM-DD)",
              "pattern": "^\\d{4}-\\d{2}-\\d{2}$"
            },
            "adults": {
              "type": "string",
              "required": true,
              "description": "Number of adults (1-9)"
            },
            "children": {
              "type": "string",
              "required": false,
              "description": "Number of children (optional)"
            }
          }
        }
      }
    }
  }
}
```

**node_schema Reglas:**
- `"fixed": value` → Valor fijo, el LLM nunca lo ve
- `"required": true` → LLM debe enviar este parámetro
- `"required": false` o ausente → Opcional (LLM puede enviar)
- `"pattern"` → Restricción regex para validación
- Funciona para `body`, `query_params`, `headers`, etc.

**DEPRECATED (Legacy - aún soportado pero no recomendado):**
- `parameters` — Override de JSON Schema
- `field_mapping` — Mapeo manual de parámetros
- `mergeable_fields` — Fusión de containers
- `exposed_inputs` — Allowlist de parámetros (⚠️ no funciona bien, usa `node_schema`)

---

### **NIVEL 6: PARÁMETROS DE PERSISTENCIA**

#### `write_to_memory`
- **Tipo:** `boolean` (default: `false`)
- **Descripción:** Guarda el resultado en la base de datos de tareas

#### `task_id`
- **Tipo:** `string` (opcional, requerido si `write_to_memory: true`)
- **Descripción:** Identificador de la tarea para guardar resultados
- **Soporta interpolación:** `{{variable}}`

---

## 📤 PARÁMETROS DE SALIDA (Output)

El nodo LLM siempre retorna un JSON con la siguiente estructura:

```json
{
  "result": "string - respuesta del LLM",
  "extra_info": {
    "usage": {
      "prompt_tokens": number,
      "completion_tokens": number,
      "total_tokens": number
    },
    "tool_calls": [
      {
        "id": "string",
        "function": {
          "name": "string",
          "arguments": "string (JSON)"
        }
      }
    ],
    "all_tasks": [
      {
        "id": "string",
        "task_name": "string",
        "assigned_to": "string",
        "completed": boolean,
        "result": any
      }
    ]
  }
}
```

### Campos de Salida Detallados:

#### `result`
- **Tipo:** `string`
- **Descripción:** La respuesta de texto del modelo LLM

#### `extra_info.usage`
- **Tipo:** `object`
- **Campos:**
  - `prompt_tokens` (number) — Tokens en el prompt enviado
  - `completion_tokens` (number) — Tokens en la respuesta
  - `total_tokens` (number) — Total

#### `extra_info.tool_calls`
- **Tipo:** `array<object>`
- **Descripción:** Herramientas que el LLM decidió llamar
- **Nota:** Solo presente si el LLM usó herramientas

#### `extra_info.all_tasks`
- **Tipo:** `array<object>`
- **Descripción:** Todos los resultados de tareas si `write_to_memory: true`

---

## 🎯 prompt vs system_message vs instructions

### **Diferencias Clave**

| Aspecto | `system_message` | `prompt` | `instructions` |
|---------|------------------|----------|---|
| **Propósito** | Define el **rol** y **personalidad** del LLM | El **contenido principal** que analizará | **Instrucciones** sobre cómo procesar |
| **Se envía como** | Mensaje SYSTEM | Mensaje USER | Parte del USER message |
| **Cuándo se agrega** | Solo si NO hay historial previo | Siempre | Siempre (junto con prompt) |
| **Se repite en cada turno** | NO (solo al inicio) | SÍ (cada vez) | SÍ (cada vez) |
| **Ejemplo** | `"You are a security analyst"` | `"Analyze this: {{data}}"` | `"Focus on vulnerabilities"` |

### **Flujo en Mensajes LLM**

```
┌────────────────────────────────────────┐
│         LLM Message Structure           │
├────────────────────────────────────────┤
│ Role: SYSTEM                           │
│ Content: "You are a security analyst"  │ ← system_message
│ (solo si NO hay historial)             │
├────────────────────────────────────────┤
│ Role: USER                             │
│ Content: prompt + instructions         │
│ "Analyze: {{data}}"                    │ ← prompt
│ "Focus on vulnerabilities"             │ ← instructions
└────────────────────────────────────────┘
```

### **Ejemplo Completo**

```json
{
  "config": {
    "provider": "openai",
    "model": "gpt-4o-mini",
    "api_key": "${OPENAI_API_KEY}",
    
    "system_message": "You are Dr. Elena Santos, a world-class security analyst with 15 years of experience.",
    
    "prompt": "Review this code:\n{{code}}",
    
    "instructions": "Focus on: 1) Auth flaws, 2) Input validation, 3) Output encoding. Rate severity as CRITICAL, HIGH, MEDIUM, LOW."
  }
}
```

**Lo que el LLM recibe:**

```
System: "You are Dr. Elena Santos, a world-class security analyst..."

User: "Review this code:
[código aquí]

Focus on: 1) Auth flaws, 2) Input validation, 3) Output encoding. Rate severity..."
```

---

## ✅ Interpolación de Variables

### **Sintaxis Soportada**

```json
{
  "system_message": "Help {{user}}",           // ← {{var}} desde inputs
  "prompt": "Data: ${context.field}",          // ← ${context.var}
  "api_key": "${OPENAI_API_KEY}",              // ← ${ENV_VAR}
  "temperature": 0.7                           // ❌ NO interpola
}
```

### **Dónde SÍ Funciona**

- ✅ `prompt`
- ✅ `system_message`
- ✅ `instructions`
- ✅ `api_key`
- ✅ `connection_url`
- ✅ `model`

### **Dónde NO Funciona**

- ❌ `temperature` (esperada número)
- ❌ `max_tokens` (esperado número)
- ❌ `stream` (esperado boolean)
- ❌ `enabled_tools` (esperado array JSON válido)

---

## 📚 Ejemplos Completos

### **Ejemplo 1: Simple Prompt**

```json
{
  "type": "llm_call",
  "config": {
    "provider": "openai",
    "model": "gpt-4o-mini",
    "api_key": "${OPENAI_API_KEY}",
    "prompt": "Explain quantum computing in 2 sentences"
  }
}
```

### **Ejemplo 2: Con Memoria Conversacional**

```json
{
  "type": "llm_call",
  "config": {
    "provider": "openai",
    "model": "gpt-4o-mini",
    "api_key": "${OPENAI_API_KEY}",
    "connection_url": "${DATABASE_URL}",
    "prompt": "What was my previous question?"
  }
}
```

El nodo no lleva ningún campo de sesión: el hilo lo determina el **run**, no el JSON.
Para que la conversación sobreviva entre runs hay que pasar el mismo
`--agent-session-id` en cada invocación:

```bash
cargo run --bin dag_engine -- run graph.json --agent-session-id user_123_chat
```

> Un `"session_id"` en el `config` de un `llm_call` **no hace nada** desde 2026-04-28: el
> nodo lee el id que el motor le inyecta y nunca el del JSON. Se lo quitó del catálogo y
> del `schema()` en 2026-09-03 justamente porque prometía una memoria que no daba. Ver
> [`15_memory_guide.md`](15_memory_guide.md).

### **Ejemplo 3: Con Secure Values**

```json
{
  "type": "llm_call",
  "config": {
    "provider": "openai",
    "api_key": "${OPENAI_API_KEY}",
    "system_message": "The <value_1>, <value_2> are secure hashes - don't reverse them",
    "prompt": "Analyze: API Token: {{api_token}}\nPassword: {{db_password}}"
  }
}
```

---

## 🔄 Guarda de bucle y rescate (loop guard + rescue)

> **Aplica solo a nodos `llm_call`** con tool calling habilitado. Los nodos
> de un solo turno (`planner`, `reactor`, `critic`, `orchestrator`,
> `information_extraction`) no son afectados — siempre ejecutan exactamente un turno.

### Motivación

El bucle ReAct de un agente puede atascarse repitiendo la misma llamada de
herramienta indefinidamente (mismo tool + mismos argumentos una y otra vez).
El parámetro `max_iterations` solía contar *turnos totales* y mataba agentes
productivos demasiado pronto — un agente que leer cuatro sheets y iteraba pandas
agotaba el límite aunque estuviera haciendo progreso real.

La nueva semántica mide **lo que importa**: repeticiones *consecutivas* de la
*misma* firma `(nombre + argumentos)`. El conteo se reinicia en el instante en
que el modelo emite cualquier llamada distinta.

---

### Parámetro `max_iterations` — nuevo significado (presupuesto de bucle)

| Aspecto | Antes (< 2026-06-14) | Ahora |
|---|---|---|
| Qué cuenta | Turnos totales del LLM | Repeticiones *consecutivas* de la misma firma `(nombre+args)` |
| Default | 10 | **3** |
| Nombre interno | `max_iterations` | `max_tool_repeats` (interno; la clave JSON pública es la misma: `max_iterations`) |

```json
// Ejemplo: la guarda por defecto permite 3 repeticiones consecutivas
{
  "config": {
    "max_iterations": 3
  }
}
```

Un grafo legacy con `max_iterations: 10` ahora permite 10 repeticiones
consecutivas de una firma — estrictamente más permisivo que antes; nunca muere
antes de tiempo.

---

### Mecánica de repetición (default `max_iterations = 3`)

| Repetición consecutiva | Acción |
|---|---|
| **1ª vez** | Ejecuta la herramienta normalmente; guarda el resultado. |
| **2ª vez (nudge)** | **No re-ejecuta**; devuelve el resultado anterior + una línea de redirección: *"Ya llamaste esta herramienta con estos argumentos — usá ese resultado o probá algo diferente."* |
| **3ª vez (rescate)** | Dispara la síntesis forzada (ver abajo). |

> **Valores `< 2`:** la primera llamada de una firma siempre se ejecuta (el
> chequeo de rescate vive dentro de "es repetición"), así que `max_iterations: 0`
> o `1` se comportan igual que `2` — 1 ejecución + 1 nudge y luego rescate. No
> bloquean la primera llamada ni causan bucles; simplemente no hay un valor
> "bloquear de entrada".

La firma es `canonical_string(nombre, argumentos)` con las claves de objetos
ordenadas recursivamente, de modo que dos llamadas semánticamente idénticas con
campos en distinto orden colapsan a la misma firma.

**Reinicio del contador:** cualquier firma distinta reinicia el contador a cero.
Ejemplo: `A,A,B,A` — la segunda racha de `A` parte de cero porque `B` la cortó;
la guarda nunca se activa.

**Múltiples tool calls en un turno:** cada llamada se evalúa independientemente.
Si alguna alcanza el umbral de rescate, el engine termina de responder a *todas*
las llamadas de ese turno (para mantener el historial válido) y luego inicia la
síntesis.

---

### Techo duro de turnos (`COLMENA_HARD_TURN_CAP`)

Independiente de la guarda de bucle existe un **techo absoluto de turnos** por
ejecución:

- **Variable de entorno:** `COLMENA_HARD_TURN_CAP` (entero positivo)
- **Fallback:** `50` turnos si la variable no está seteada
- **No configurable desde el JSON del grafo** (es un límite operacional, no de agente)

El techo evita que un agente que llama tools distintas (sin activar la guarda de
bucle) consuma recursos ilimitados. Cuando lo alcanza, también dispara la síntesis
forzada.

Los nodos de un solo turno `planner`, `reactor`, `critic` y `orchestrator`
setean internamente `max_turns = 1` (ver `nodes/planner.rs:385`,
`nodes/reactor.rs:302`, `nodes/critic.rs:265`, `nodes/orchestrator.rs:731`),
preservando su comportamiento anterior sin cambios. `information_extraction`
(`nodes/extraction.rs`) ni siquiera pasa por el loop de tool-calling — hace
una única llamada directa al LLM — por lo que el concepto de `max_turns` no
le aplica.

---

### Rescate — síntesis forzada

Cuando se activa la guarda de bucle **o** se alcanza el techo de turnos, en lugar
de retornar un error, el engine realiza **una llamada LLM final sin herramientas**
con la instrucción:

> *"Llegaste al límite. Dá tu mejor respuesta final con lo que ya tenés y aclará
> qué quedó incompleto."*

- La respuesta de síntesis se **persiste en memoria conversacional** y se retorna
  como `Ok(respuesta)` — una respuesta exitosa normal.
- Si `stream: true` está habilitado, la síntesis también se transmite en streaming.
- La llamada de síntesis **no cuenta** contra el techo de turnos.
- El error `MaxIterationsReached` sigue existiendo en el enum `LlmError` para
  compatibilidad, pero **ya no se retorna** en el flujo normal.

---

### Resumen de flujos

```
Turno N — modelo emite tool call con firma S
│
├─ ¿S == firma actual de la racha?
│   ├─ No  → reiniciar racha (streak=1), ejecutar tool
│   └─ Sí  → streak++
│               ├─ streak < max_iterations → nudge (no re-ejecuta)
│               └─ streak >= max_iterations → RESCATE → síntesis forzada → Ok(respuesta)
│
└─ ¿turno N >= COLMENA_HARD_TURN_CAP?
    └─ Sí → RESCATE → síntesis forzada → Ok(respuesta)
```

---

### Textos LLM-facing (registro `text/`)

Los mensajes que el modelo recibe durante nudge y rescate viven en el registro
de texto (no hardcodeados en Rust):

- Nudge: `text/prompts/agent_loop/repeat_nudge.md`
- Rescate: `text/prompts/agent_loop/rescue_synthesis.md`

Para personalizar los mensajes, editar esos archivos — no se requieren cambios en Rust.

---

## 🛠️ Troubleshooting

### **"Missing 'provider' in inputs or config"**
- **Causa:** No se especificó `provider` en config o inputs
- **Solución:** Agregar `"provider": "openai"` al config

### **"Prompt resolved to empty"**
- **Causa:** `prompt` es `null` o template resolvió a string vacío
- **Solución:** Verificar que `prompt` está bien especificado

### **"LLM never sees real values" (Secure Values)**
- **Verificar:** En la salida del nodo, buscar `<value_N>` en lugar de valores reales
- **Debug:** Usar `verbose: true` para ver el prompt exacto enviado

### **"Failed to parse arguments for tool: trailing characters" (Gemini)**
- **Causa:** Bug corregido en v0.3.0 — Gemini enviaba múltiples tool calls en paralelo y el adapter de streaming les asignaba el mismo índice (`0`), causando que `agent_service.rs` concatenara los argumentos JSON de herramientas distintas
- **Fix:** El adapter de Gemini ahora usa un contador incremental `tool_call_index` para asignar índices únicos a cada tool call chunk
- **Archivo:** `src/libs/colmena/src/llm/infrastructure/gemini_adapter.rs`

### **"migration was previously applied but is missing" (PostgreSQL)**
- **Causa:** La tabla `_sqlx_migrations` tiene registros de migraciones que ya no existen en disco (tras consolidación de esquema)
- **Solución:** `psql $DATABASE_URL -c "DROP TABLE IF EXISTS _sqlx_migrations;"` y re-ejecutar
- **Protección:** El migrador usa `set_ignore_missing(true)` para tolerar migraciones faltantes sin error

---

## 💾 Provider prompt caching (default ON, shipped 2026-06-09)

Los tres providers reducen el costo de tokens repetidos vía caching nativo.
**Caching es ON por default — no requiere opt-in.** Los stats se surface en
`response.extra_info.usage.cache_read_tokens` (y `cache_write_tokens` para
Anthropic).

### Tabla resumen

| Provider | Mecanismo | Qué se cachea | TTL | Mínimo prefix | Cache read | Cache write |
|---|---|---|---|---|---|---|
| **OpenAI** | Automatic server-side | Prefix completo del request | ~5-10 min | 1024 tokens | 0.10× (0.50× en gpt-4o y anteriores) | **1.25× desde GPT-5.6**; gratis antes |
| **Anthropic** | `cache_control: ephemeral` markers | System message + tools[] | 5 min / 1 h | ninguno explícito | 0.10× | 1.25× (5 min) · 2× (1 h) |
| **Gemini 2.5+** | Implicit caching (automatic) | Prefix del request | ~3-5 min | 1024 (flash) / 2048 (pro) | 0.10× | gratis (implicit) |

> **Multiplicadores relativos al precio de input base.** Verificado contra la
> documentación de cada provider el 2026-08-23.
>
> Dos correcciones respecto de lo que decía esta guía antes: el descuento de
> lectura de OpenAI pasó de 50% a **90%** a partir de la serie gpt-5.x, y el de
> Gemini no es "25-75%" sino **90%** en los modelos 2.5 y posteriores (era 75%
> en los 2.0).
>
> **OpenAI ahora sí cobra por escribir.** Desde GPT-5.6 la creación de cache
> cuesta 1.25× y se reporta en `prompt_tokens_details.cache_write_tokens`. En
> modelos anteriores la creación es gratuita y el campo no existe. Igual que
> `cached_tokens`, es un **subconjunto** de `prompt_tokens`: las tres categorías
> particionan el input (`cached + written + uncached = prompt_tokens`), así que
> el adapter resta ambas.
>
> **Gemini tiene un segundo modo con costo de escritura** que Colmena no usa: el
> *explicit caching* vía la API `CachedContent`, que cobra almacenamiento por
> hora ($1.00 por 1M tokens/hora en 2.5 Flash, $4.50 en 2.5 Pro). El *implicit
> caching* que usamos no cobra creación ni almacenamiento.
### Cómo funciona internamente

**OpenAI** — el adapter no hace nada en el request body. La API server-side
detecta prefixes repetidos y los cachea automáticamente. El adapter lee
`prompt_tokens_details.cached_tokens` del usage response.

**Anthropic** — el adapter `build_request_body` agrega 2 markers en cada
request:

```jsonc
{
  "system": [
    {
      "type": "text",
      "text": "<system message>",
      "cache_control": {"type": "ephemeral"}   // ← marker 1
    }
  ],
  "tools": [
    { "name": "tool_a", "description": "...", "input_schema": {...} },
    { "name": "tool_b", "description": "...", "input_schema": {...} },
    {
      "name": "tool_c",
      "description": "...",
      "input_schema": {...},
      "cache_control": {"type": "ephemeral"}    // ← marker 2 (último tool)
    }
  ]
}
```

Anthropic interpreta cada marker como "todo el contenido hasta este punto
es cacheable". El system message marker cachea ese block; el last-tool
marker cachea todo el array `tools[]`.

> **Más de un mensaje `system` en el request.** La Messages API de Anthropic
> tiene UN campo `system` top-level, pero un `LlmRequest` puede traer varios
> mensajes de rol `system` (están explícitamente permitidos, incluso
> intercalados). El adapter los emite **todos, en orden, como bloques
> separados** y pone el marker `cache_control` **solo en el primero**, de modo
> que lo estable se cachea y lo volátil queda fuera del prefijo cacheado.
> Antes del fix del 2026-08-22 **sobrescribía**: solo sobrevivía el último
> `system` y los anteriores se perdían sin error ni log.
>
> Esa ruta ya **no** es latente: desde el 2026-08-23 toda conversación
> compactada manda dos `system` — el prompt estable del agente y, aparte, el
> `## Conversation summary`. La separación es deliberada y existe justamente
> para este marker: el resumen se recomputa en cada carga, así que fusionarlo
> dentro del bloque 0 movía el prefijo cacheado en cada turno y el caching
> nunca acertaba. Medición antes/después en
> [§15 — Dónde termina el resumen](15_memory_guide.md).
>
> Sostener eso requiere además que `LlmRequest::new` **no** vuelva a fusionar
> los dos `system`: `coalesce_consecutive_same_role` exime a `System` igual que
> a `Tool`. Sin esa exención el dominio deshace la separación antes de que el
> adapter vea nada.

Cualquier llamada siguiente del **mismo agente** dentro de los 5 minutos
siguientes paga ~10% del precio normal sobre la porción cacheada.

**Importante**: el conversational tail (user/assistant messages) NO se
cachea — cambia cada turn y cachearlo causaría cache-write churn sin read
benefit.

**Gemini 2.5+** — el adapter no toca el request body. Gemini 2.5 models
tienen **implicit caching** automático server-side (lanzado mayo 2025) que
cachea cualquier prefix repetido ≥1024 tokens (flash) o ≥2048 tokens (pro).
El adapter solo lee `usageMetadata.cachedContentTokenCount` y lo mapea a
`LlmUsage::cache_read_tokens`.

### Precios por modelo (USD por millón de tokens, 2026-08-23)

**Anthropic** — los únicos con dos duraciones de cache:

| Modelo | Input | Write 5 min | Write 1 h | Read | Output |
|---|---|---|---|---|---|
| Claude Fable 5 | $10 | $12.50 | $20 | $1 | $50 |
| Claude Opus 5 | $5 | $6.25 | $10 | $0.50 | $25 |
| Claude Sonnet 5 | $2 | $2.50 | $4 | $0.20 | $10 |
| Claude Sonnet 4.6 | $3 | $3.75 | $6 | $0.30 | $15 |
| Claude Haiku 4.5 | $1 | $1.25 | $2 | $0.10 | $5 |

**OpenAI** — `—` significa que ese modelo no cobra por escribir:

| Modelo | Input | Cached (read) | Write | Output |
|---|---|---|---|---|
| gpt-5.6-sol | $4.00 | $0.40 | $5.00 | $20.00 |
| gpt-5.6-terra | $2.00 | $0.20 | $2.50 | $12.00 |
| gpt-5.6-luna | $0.20 | $0.02 | $0.25 | $1.20 |
| gpt-5.5 | $5.00 | $0.50 | — | $30.00 |
| gpt-5.4 | $2.50 | $0.25 | — | $15.00 |
| gpt-4o | $2.50 | $1.25 | — | $10.00 |
| gpt-4o-mini | $0.15 | $0.075 | — | $0.60 |

**Gemini** — la columna de almacenamiento aplica **solo al explicit caching**:

| Modelo | Input | Cache hit | Output | Storage (explicit) |
|---|---|---|---|---|
| 2.5 Flash | $0.30 | $0.03 | $2.50 | $1.00 /1M/hora |
| 2.5 Pro (≤200k) | $1.25 | $0.125 | $10.00 | $4.50 /1M/hora |
| 2.5 Flash-Lite | $0.10 | $0.01 | $0.40 | $1.00 /1M/hora |
| 3.7 / 3.6 Flash | $0.75 | $0.075 | $3.75 | $0.50 /1M/hora |
| 3.5 Flash | $1.50 | $0.15 | $9.00 | $1.00 /1M/hora |

> Los precios cambian. Verificar contra
> [Anthropic](https://platform.claude.com/docs/en/about-claude/pricing),
> [OpenAI](https://developers.openai.com/api/docs/pricing) y
> [Gemini](https://ai.google.dev/gemini-api/docs/pricing) antes de usarlos para
> facturar.

### Cómo verificar que el caching está activo

```python
# Run 1 (uncached)
response_1 = llm_call(system="You are an agent...", tools=[...], prompt="hi")
print(response_1.extra_info.usage.cache_read_tokens)  # → None o 0

# Run 2 (cache hit, mismo system + tools, dentro de 5 min)
response_2 = llm_call(system="You are an agent...", tools=[...], prompt="hi again")
print(response_2.extra_info.usage.cache_read_tokens)  # → >0 (Anthropic/Gemini/OpenAI)
```

Si `cache_read_tokens` es 0 en el run 2 sospechá:
- TTL expiró (esperaste >5 min).
- El system message o las tools cambiaron entre runs (cualquier byte difference invalida el cache).
- El prefix es menor al mínimo del provider (ver tabla abajo).
- **Gemini necesita warmup**: el implicit cache recién popula tras ~3-5 calls con el mismo prefijo (verificado live 2026-06-11). Los primeros 2-3 turnos de una conversación pueden no mostrar `cache_read`; a partir del turn ~4-5 sí.

### Mínimos cacheables reales por modelo (verificado live 2026-06-11)

| Modelo | Mínimo documentado | Realidad empírica |
|---|---|---|
| `claude-sonnet-4-6`, opus | 1024 | ✅ cachea a ~1.5K |
| **`claude-haiku-4-5`** | "2048" | ❌ **NO cachea ni a ~2900 tokens** — el mínimo real es bastante mayor. **Preferí sonnet/opus para cachear**, o garantizá prefijos grandes en haiku. |
| `gpt-4o` | 1024 | ✅ cachea confiablemente a ≥~2K (a ~1.2K es intermitente) |
| `gemini-2.5-flash` | 1024 | ✅ cachea el prefijo a ~3K **tras warmup** |

> **Lección del E2E:** un test de cache con `claude-haiku-4-5` + prefijo de ~2K dará `cache_read=0` y parecerá un bug del feature — pero es el mínimo del modelo. Los grafos `tests/graphs/agents/provider_cache_temporal_{anthropic,openai,gemini}_e2e.json` usan sonnet/gpt-4o/2.5-flash justamente por esto.

### Bloque temporal cache-safe (2026-06-11)

El bloque **Temporal & Geographic Context** (date/time/location, ver §35) se
inyecta como **suffix volátil al FINAL del system message**, fuera del prefijo
cacheado. Esto permite que el timestamp se refresque **cada turno** (hora
correcta en conversaciones largas) **sin romper el cache** del prefijo estable.

- **Anthropic**: el adapter emite el system como un array de bloques —
  `[primero (cache_control: ephemeral), ...siguientes (sin marker)]`. El marker
  cubre solo el **primer** bloque; todo lo que llegue después (más mensajes
  `system`, el bloque temporal) queda fuera del prefijo cacheado.
- **OpenAI / Gemini**: el temporal se concatena al final del **último** mensaje
  `system` / del `systemInstruction`; su prefix-cache automático cachea el
  prefijo estable.

Antes del fix el timestamp iba al FRENTE del system y quedaba **congelado** en
turn 1 (gate `if !history_exists`) para no romper el cache — al costo de mostrar
una hora vieja en chats largos. Ahora se obtienen las dos cosas. Mecanismo:
campo `LlmConfig::volatile_system_suffix`. Spec:
[`docs/superpowers/specs/2026-06-11-temporal-block-cache-safe-design.md`](../superpowers/specs/2026-06-11-temporal-block-cache-safe-design.md).

### Cuándo NO querés caching

Caching es ON por default y no hay flag para apagarlo. Si el operador necesita
deshabilitarlo (e.g. para benchmarks de billing aislados), el workaround es:
- Anthropic: editar `build_request_body` localmente para no inyectar el marker.
- Gemini: caching es server-side, no se puede apagar — los stats se pueden
  ignorar a nivel del consumer.

Casos típicos donde caching es siempre net-positive: agentes con
system_message largo (>2K tokens), agentes con muchos tools (`tools[]`
agrega ~1-5K tokens), workflows multi-turn con state persistente.

### Surfacing de cache tokens en el SSE

Los cache stats salen en **2 lugares del stream SSE**, en formatos
distintos para distintos consumidores:

| Evento | Naming convention | Scope | Consumidor típico |
|---|---|---|---|
| `node-end` (por llm_call) | `cache_read_tokens` / `cache_write_tokens` (snake_case) | Por-nodo, aggregate de todas las iteraciones de ese `llm_call` | ADP backoffice, dashboards por nodo |
| `finish` (run-level) | `cacheReadTokens` / `cacheWriteTokens` (camelCase) | Aggregate de TODAS las `LlmUsage` events del run | ADP frontend (costo total del turn) |

**Esquema del `finish.usage` aggregate:**

```jsonc
{
  "type": "finish",
  "finishReason": "stop",
  "usage": {
    "promptTokens": 4193,            // siempre presente — input FRESCO, sin cache
    "completionTokens": 52,          // siempre presente
    "cacheReadTokens": 725,          // siempre presente (0 si no hubo hit)
    "cacheWriteTokens": 24882,       // siempre presente (0 si no hubo write)
    "totalTokens": 29876,            // siempre presente — incluye thinking Y cache
    "thinkingTokens": 76             // solo si > 0
  },
  "output": { ... }
}
```

**Esquema del `node-end.output.extra_info.usage` por nodo:**

```jsonc
{
  "type": "node-end",
  "node_id": "agent",
  "output": {
    "extra_info": {
      "usage": {
        "prompt_tokens": 4193,        // input FRESCO, sin cache
        "completion_tokens": 52,
        "cache_read_tokens": 725,     // siempre presente (0 si no hubo hit)
        "cache_write_tokens": 24882,  // siempre presente (0 si no hubo write)
        "total_tokens": 29876,        // incluye thinking Y cache
        "thinking_tokens": 76         // solo si > 0
      },
      "tool_calls": [ ... ]
    }
  }
}
```

**Los campos de cache están SIEMPRE presentes** (desde 2026-08-23), incluso en
`0`. Antes tenían gate `> 0`, lo que hacía imposible distinguir "no hubo cache
hit" de "este provider no reporta el dato" — dos situaciones con implicaciones
de costo opuestas. `thinkingTokens` conserva su gate `> 0`.

**Read y write NO se colapsan en un solo número.** Anthropic cobra ~10% del
input rate por un cache read y ~125% por un cache write: un factor de más de
10x entre ambos. Un campo único de "cache" no sería facturable.

### Semántica de `promptTokens`: normalizada, sin restas del consumidor

Las tres APIs discrepan en si los tokens cacheados cuentan dentro del input:

| Provider | Campo crudo de la API | ¿El cache está dentro del input? |
|---|---|---|
| **Anthropic** | `input_tokens` | **No** — disjunto de `cache_read_input_tokens` y `cache_creation_input_tokens` |
| **OpenAI** | `prompt_tokens` | **Sí** — `prompt_tokens_details.cached_tokens` es un subconjunto |
| **Gemini** | `promptTokenCount` | **Sí** — `cachedContentTokenCount` es un subconjunto |

**Colmena normaliza esto en el adapter**, que es el único lugar donde se conoce
la semántica del provider. Los adapters de OpenAI y Gemini restan el cache del
prompt; el de Anthropic lo deja intacto porque ya viene neto. El resultado es
que `promptTokens` significa **siempre lo mismo — input fresco** — y por lo tanto:

```
promptTokens + cacheReadTokens + cacheWriteTokens  =  input real del turno
```

Esa identidad se sostiene en los tres providers, así que el agregado run-level
sigue siendo sumable incluso cuando un grafo mezcla providers.

> **Verificado en vivo contra las tres APIs reales el 2026-08-23**, no inferido
> de su documentación.
>
> **OpenAI** (`gpt-4o`) — la evidencia más limpia, dos corridas del mismo grafo:
>
> | | `prompt_tokens` | `cache_read_tokens` | `total_tokens` |
> |---|---|---|---|
> | run 1 (sin cache) | 2550 | 0 | 2554 |
> | run 2 (cache hit) | **118** | **2432** | 2554 |
>
> `118 + 2432 = 2550`, exactamente el prompt de run 1, y el total no se mueve: el
> mismo trabajo, redistribuido entre la columna cara y la barata.
>
> **Gemini** — en un cache hit (`cache_read_tokens: 8820`) el `promptTokenCount`
> **no cayó** respecto al turno anterior sin hit (9235 → 9259), lo que prueba que
> el cache va adentro.
>
> **Anthropic** — `prompt_tokens: 404` con `cache_read_tokens: 1809`: imposible
> si estuviera adentro.

**Cómo calcular costo real (ejemplo Anthropic):**

```
costo_input  = promptTokens      × rate_full_input           ← YA es input fresco
             + cacheReadTokens   × rate_full_input × 0.10
             + cacheWriteTokens  × rate_full_input × 1.25    (una vez)
costo_output = completionTokens  × rate_output
costo_total  = costo_input + costo_output
```

> ⚠️ **No restes `cacheReadTokens` de `promptTokens`.** Esta guía documentó
> `(promptTokens − cacheReadTokens)` hasta el 2026-08-23; con la normalización
> vigente eso resta el cache dos veces. Con los números reales medidos arriba
> daba `404 − 1809 = −1405`, es decir un costo de input **negativo**.

(Rates exactos: ver pricing del provider; Anthropic cobra ~10% para cache
reads y ~125% para cache writes, balance neto positivo si hay ≥2 hits.)

**Cómo verificar live que el surfacing funciona:**

```bash
set -a && source .env && set +a
./target/release/dag_engine run <graph.json> --include-extra-info > /tmp/run.sse
# Cache tokens en node-end events:
grep -oE '"cache_read_tokens":[0-9]+|"cache_write_tokens":[0-9]+' /tmp/run.sse
# Cache tokens en finish event aggregate:
grep -oE '"cacheReadTokens":[0-9]+|"cacheWriteTokens":[0-9]+' /tmp/run.sse
```

Si ves valores > 0 en al menos uno de los dos formatos, el caching está activo
y el SSE lo está propagando correctamente.

---

**Versión:** 1.3  
**Fecha:** 2026-08-23  
**Status:** ✅ Completo
