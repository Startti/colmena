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
- **Valores válidos:** `"openai"`, `"gemini"`, `"anthropic"`, `"mock"` (testing)
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
  - OpenAI: `"gpt-4"`, `"gpt-4o"`, `"gpt-4o-mini"`, `"gpt-3.5-turbo"`
  - Gemini: `"gemini-1.5-pro"`, `"gemini-1.5-flash"`, `"gemini-pro"`
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

The LLM node supports an optional `skills` config field that exposes built-in and/or user-provided markdown skill packages via a synthetic `load_skill` tool. The LLM decides at runtime which skills to activate. See [24_skills.md](24_skills.md) for the full guide.

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
    "session_id": "user_123_chat",
    "connection_url": "${DATABASE_URL}",
    "prompt": "What was my previous question?"
  }
}
```

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

**Versión:** 1.1  
**Fecha:** 2026-04-08  
**Status:** ✅ Completo
