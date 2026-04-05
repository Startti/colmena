# 🔗 Conexión Entre Nodos y Flujo de Datos

## 🎯 Pregunta Principal: "¿Cómo hago que un nodo use datos de otro nodo?"

**Respuesta corta:** Los datos fluyen **automáticamente** a través de los **edges** (aristas) en el JSON. NO necesitas hacer nada especial — solo conecta los nodos con edges.

---

## 🔄 CÓMO FUNCIONAN LOS EDGES

### **Concepto Fundamental**

Cuando defines un edge entre dos nodos, Colmena:

1. **Ejecuta el nodo origen** → obtiene su output
2. **Resuelve qué datos enviar** → aplica reglas de resolución de puertos
3. **Inyecta datos en el nodo destino** → agrega al `inputs` del siguiente nodo
4. **Ejecuta el nodo destino** → ahora puede acceder a los datos interpolados

```
┌──────────────┐
│ Nodo A       │
│ output:      │
│ {            │
│   result: "data",
│   usage: {...}
│ }            │
└──────┬───────┘
       │ edge
       ↓
┌──────────────────┐
│ Nodo B (inputs)  │
│ {                │
│   result: "data",    ← Propagado
│   usage: {...}       ← Propagado
│ }                │
└──────────────────┘
```

---

## ✅ FORMA CORRECTA: Edges en JSON

### **Los edges van en el JSON, pero NO necesitas especificar qué campo propagar**

Colmena lo detecta automáticamente usando **Default Ports**.

#### **Forma SIMPLE (Recomendada)**

```json
{
  "nodes": {
    "fetch_data": {
      "type": "http_request",
      "config": {
        "endpoint": "/api/users",
        "method": "GET"
      }
    },
    "analyze": {
      "type": "llm_call",
      "config": {
        "provider": "openai",
        "api_key": "${OPENAI_API_KEY}",
        "prompt": "Analyze: {{data}}"
      }
    }
  },
  "edges": [
    {
      "from": "fetch_data",
      "to": "analyze"
    }
  ]
}
```

**¿Qué sucede?**

1. `fetch_data` ejecuta → output: `{ status: 200, body: { ... } }`
2. Edge `fetch_data → analyze` propaga automáticamente
3. `analyze` recibe en `inputs`: `{ status: 200, body: { ... } }`
4. El `prompt` interpola: `{{data}}` busca en inputs y lo encuentra

---

## 🎯 REGLAS DE RESOLUCIÓN DE PUERTOS (Default Ports)

Cada nodo tiene un **puerto de entrada por defecto** y uno de **salida por defecto**:

| Nodo | Input Default | Output Default | Descripción |
|------|---|---|---|
| `llm_call` | `prompt` | `result` | LLM lee `prompt`, emite `result` |
| `log` | `input` | `output` | Log lee `input`, emite `output` |
| `http_request` | — | `body` | HTTP emite `body` (requiere `url` explícito) |
| `output` | `input` | `result` | Output node lee `input` |
| `trigger_webhook` | — | `output` | Trigger emite `output` |
| `add`, `subtract`, `multiply`, `divide` | — | `output` | Math nodes SIN default input |

---

## 🔀 CASOS DE RESOLUCIÓN AUTOMÁTICA

### **Caso 1: Ambos nodos tienen defaults (MEJOR CASO) ✅**

```json
{
  "nodes": {
    "llm1": { "type": "llm_call", "config": {...} },
    "llm2": { "type": "llm_call", "config": {...} }
  },
  "edges": [
    { "from": "llm1", "to": "llm2" }
  ]
}
```

**Resolución:** `llm1.result` → `llm2.prompt`

**¿Por qué funciona?**
- `llm1` tiene `default_output = "result"`
- `llm2` tiene `default_input = "prompt"`
- El engine conecta automáticamente

---

### **Caso 2: Source tiene default, target no (Auto-flatten)**

```json
{
  "nodes": {
    "llm1": { "type": "llm_call", "config": {...} },
    "add_node": { "type": "add" }
  },
  "edges": [
    { "from": "llm1", "to": "add_node" }
  ]
}
```

**Resolución:** `llm1` emite `{ result: "...", usage: {...} }` → todos los campos se envían a `add_node`:

```
add_node recibe en inputs:
{
  "result": "...",
  "usage": {...}
}
```

⚠️ **PROBLEMA:** `add` necesita específicamente `a` y `b`, no `result` y `usage`.

**SOLUCIÓN:** Ser explícito:

```json
{
  "edges": [
    { "from": "llm1.result", "to": "add_node.a" },
    { "from": "llm1.result", "to": "add_node.b" }
  ]
}
```

---

### **Caso 3: Ser Explícito (Siempre Seguro) ✅**

```json
{
  "edges": [
    { "from": "fetch_data.body", "to": "llm_node.prompt" }
  ]
}
```

**Resolución:** EXACTAMENTE `fetch_data.body` → `llm_node.prompt`

No hay ambigüedad, no hay auto-flatten. Funciona siempre.

---

## 📝 EJEMPLOS REALES

### **Ejemplo 1: HTTP → LLM (Lo más común)**

```json
{
  "nodes": {
    "fetch_api": {
      "type": "http_request",
      "config": {
        "base_url": "https://api.example.com",
        "endpoint": "/data",
        "method": "GET"
      }
    },
    "analyze_llm": {
      "type": "llm_call",
      "config": {
        "provider": "openai",
        "api_key": "${OPENAI_API_KEY}",
        "prompt": "Analyze this data: {{body}}"
      }
    },
    "log_result": {
      "type": "log"
    }
  },
  "edges": [
    { "from": "fetch_api", "to": "analyze_llm" },
    { "from": "analyze_llm", "to": "log_result" }
  ]
}
```

**Flow:**
1. `fetch_api` ejecuta → output: `{ status: 200, body: {...} }`
2. `analyze_llm` recibe: `{ status: 200, body: {...} }` en inputs
3. `prompt` interpola: `"Analyze: {{body}}"` → busca `body` en inputs
4. LLM ve: `"Analyze: {...}"`
5. `log_result` recibe: `{ result: "...", extra_info: {...} }`

---

### **Ejemplo 2: Trigger → LLM → Log (Cadena simple)**

```json
{
  "nodes": {
    "trigger": {
      "type": "trigger_webhook",
      "config": {
        "path": "/analyze",
        "method": "POST",
        "test_payload": {
          "user_input": "What is AI?",
          "company": "Acme Corp"
        }
      }
    },
    "llm": {
      "type": "llm_call",
      "config": {
        "provider": "openai",
        "api_key": "${OPENAI_API_KEY}",
        "system_message": "Answer for {{company}}",
        "prompt": "User asks: {{user_input}}"
      }
    },
    "log": { "type": "log" }
  },
  "edges": [
    { "from": "trigger", "to": "llm" },
    { "from": "llm", "to": "log" }
  ]
}
```

**Flow:**
1. Trigger emite: `{ user_input: "What is AI?", company: "Acme Corp" }`
2. LLM recibe en inputs: `{ user_input: "...", company: "..." }`
3. `system_message` interpola: `"Answer for {{company}}"` → `"Answer for Acme Corp"`
4. `prompt` interpola: `"User asks: {{user_input}}"` → `"User asks: What is AI?"`
5. LLM procesa y emite: `{ result: "AI is...", extra_info: {...} }`

---

### **Ejemplo 3: Múltiples Inputs (Investigación + Crítica + Síntesis)**

```json
{
  "nodes": {
    "trigger": {
      "type": "trigger_webhook",
      "config": {
        "test_payload": { "topic": "quantum computing" }
      }
    },
    
    "researcher": {
      "type": "llm_call",
      "config": {
        "provider": "openai",
        "api_key": "${OPENAI_API_KEY}",
        "prompt": "Research {{topic}} and provide 5 key points"
      }
    },
    
    "critic": {
      "type": "llm_call",
      "config": {
        "provider": "openai",
        "api_key": "${OPENAI_API_KEY}",
        "system_message": "You are a critical reviewer",
        "prompt": "Review this research:\n{{researcher_result}}\n\nCriticize the weak points"
      }
    },
    
    "synthesizer": {
      "type": "llm_call",
      "config": {
        "provider": "openai",
        "api_key": "${OPENAI_API_KEY}",
        "prompt": "Original research:\n{{researcher_result}}\n\nCriticism:\n{{critic_result}}\n\nSynthesize a better answer"
      }
    },
    
    "output": { "type": "output" }
  },
  
  "edges": [
    { "from": "trigger", "to": "researcher" },
    { "from": "researcher", "to": "critic" },
    { "from": "researcher.result", "to": "synthesizer.researcher_result" },
    { "from": "critic.result", "to": "synthesizer.critic_result" },
    { "from": "synthesizer", "to": "output" }
  ]
}
```

**Flow detallado:**

```
trigger {topic: "quantum computing"}
    ↓
researcher (prompt: "Research {{topic}}")
    outputs: {result: "1) Superposition...", extra_info: {...}}
    ↙                    ↘
critic                  synthesizer
(prompt: "Review        (inputs: {researcher_result, critic_result})
{{researcher_result}}")
    outputs:               outputs:
    {result: "Weak        {result: "Combined view...",
    points..."}           extra_info: {...}}
    ↓                     ↓
    └──→ synthesizer ←────┘
         outputs: {result: "..."}
         ↓
      output
```

**¿Cómo recibe `synthesizer` dos inputs?**

```json
{
  "from": "researcher.result", "to": "synthesizer.researcher_result"
}
```

→ Envía SOLO `researcher.result` al campo `researcher_result` en inputs

```json
{
  "from": "critic.result", "to": "synthesizer.critic_result"
}
```

→ Envía SOLO `critic.result` al campo `critic_result` en inputs

**synthesizer recibe en inputs:**
```json
{
  "researcher_result": "1) Superposition...",
  "critic_result": "Weak points...",
  "topic": "quantum computing"
}
```

**Y el prompt interpola:**
```
"Original research:
1) Superposition...

Criticism: Weak points...

Synthesize..."
```

---

### **Ejemplo 4: HTTP → LLM con Secure Values**

```json
{
  "nodes": {
    "fetch_secrets": {
      "type": "http_request",
      "config": {
        "endpoint": "https://api.example.com/secrets",
        "secure": true
      }
    },
    
    "analyze_secure": {
      "type": "llm_call",
      "config": {
        "provider": "openai",
        "api_key": "${OPENAI_API_KEY}",
        "system_message": "The <value_1>, <value_2> are secure hashes.",
        "prompt": "Analyze these secrets:\nPassword: {{body.password}}\nAPI Key: {{body.api_key}}"
      }
    },
    
    "log": { "type": "log" }
  },
  
  "edges": [
    { "from": "fetch_secrets", "to": "analyze_secure" },
    { "from": "analyze_secure", "to": "log" }
  ]
}
```

**Flow:**

1. `fetch_secrets` → output: 
```json
{
  "body": {
    "password": "<value_1>",  // ← Hashed (real: "prod_secret")
    "api_key": "<value_2>"    // ← Hashed (real: "sk_live_abc")
  }
}
```

2. `analyze_secure` recibe: `{ body: { password: "<value_1>", api_key: "<value_2>" } }`

3. Prompt interpola:
```
"Analyze these secrets:
Password: <value_1>
API Key: <value_2>"
```

4. **LLM VE:** `<value_1>` y `<value_2>` (NUNCA los valores reales)

5. Database almacena:
```
<value_1> → AES-256(prod_secret)
<value_2> → AES-256(sk_live_abc)
```

---

## 🔑 PUNTOS CLAVE

### **✅ Lo que SÍ funciona**

```json
{
  "edges": [
    { "from": "nodeA", "to": "nodeB" },
    { "from": "nodeA.field", "to": "nodeB.field" },
    { "from": "nodeA.result", "to": "nodeB.prompt" }
  ]
}
```

Interpolación en config:
```json
{
  "prompt": "Data: {{field_from_previous_node}}",
  "system_message": "Context: {{another_field}}",
  "api_key": "${ENV_VARIABLE}"
}
```

### **❌ Lo que NO funciona**

```json
{
  "temperature": "{{value}}",       // ❌ Espera número, no interpola
  "max_tokens": "{{value}}",        // ❌ Espera número, no interpola
  "stream": "{{value}}",            // ❌ Espera boolean, no interpola
  "enabled_tools": "{{array}}"      // ❌ Complejo, requiere JSON válido
}
```

---

## 📋 TABLA RESUMIDA

| Campo | ¿Interpola? | En Edge | Ejemplo |
|---|---|---|---|
| `prompt` | ✅ SÍ | `{ from, to }` | `"{{var}}"` |
| `system_message` | ✅ SÍ | `{ from, to }` | `"{{var}}"` |
| `api_key` | ✅ SÍ | `{ from, to }` | `"${ENV}"` |
| `temperature` | ❌ NO | — | `0.7` |
| `stream` | ❌ NO | — | `true` |

---

## 🚀 FLUJO COMPLETO (Paso a Paso)

### **1. Definir Grafo con Edges**

```json
{
  "nodes": { ... },
  "edges": [
    { "from": "nodeA", "to": "nodeB" }
  ]
}
```

### **2. Engine Ejecuta nodeA**

```
nodeA executes() → returns: { result: "data", extra: {...} }
```

### **3. Engine Resuelve Edge (Default Ports)**

```
Edge: from nodeA (default_output: "result")
      to nodeB (default_input: "prompt")
→ Resolution: nodeA.result → nodeB.prompt
```

### **4. Engine Propaga Datos**

```
nodeB.inputs = { result: "data", extra: {...} }
```

### **5. Engine Interpola Campos**

```
nodeB.config.prompt = "Analyze: {{result}}"
→ Resuelve: "Analyze: data"
```

### **6. Engine Ejecuta nodeB**

```
nodeB executes(inputs={result: "data", ...})
→ returns: { result: "...", extra_info: {...} }
```

---

## ❓ RESPUESTAS A PREGUNTAS COMUNES

### **P: ¿Necesito poner nada especial en el JSON de los edges?**
**R:** No, solo `from` y `to`. El engine detecta automáticamente qué propagar.

### **P: ¿Puedo enviar solo un campo específico?**
**R:** Sí, sé explícito: `{ "from": "nodeA.body", "to": "nodeB.prompt" }`

### **P: ¿Puedo enviar a múltiples campos?**
**R:** Sí, múltiples edges hacia el mismo nodo:
```json
[
  { "from": "nodeA.result", "to": "nodeC.input1" },
  { "from": "nodeB.result", "to": "nodeC.input2" }
]
```

### **P: ¿Qué sucede si hay nombres en conflicto?**
**R:** El edge ÚLTIMO gana. O sé explícito para evitar ambigüedad.

### **P: ¿Los datos se pierden entre turnos?**
**R:** Por defecto SÍ (efímero). Usa memoria (`session_id`) para persistencia.

### **P: ¿Cómo accedo a datos anidados?**
**R:** Usa `{{object.property}}` o sé explícito en el edge:
```json
{ "from": "nodeA.body.nested.field", "to": "nodeB.prompt" }
```

---

## 📚 RESUMEN: CONEXIÓN DE NODOS

1. **Define edges** en el JSON con `from` y `to`
2. **No necesitas especificar qué campo enviar** — Colmena lo detecta con Default Ports
3. **Interpola datos en configuración** usando `{{variable}}` o `${ENV}`
4. **Sé explícito cuando dudes** — `{ from: "A.field1", to: "B.field2" }` siempre funciona
5. **Los datos fluyen automáticamente** entre nodos según los edges

---

**Versión:** 1.0  
**Fecha:** 2026-04-04  
**Status:** ✅ Completo
