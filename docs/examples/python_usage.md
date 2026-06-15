# 🐍 Ejemplos de Uso en Python — Colmena

Guía práctica de cómo usar Colmena desde Python. **Todos los ejemplos coinciden con la API real**
expuesta por las bindings PyO3 (`src/libs/colmena/src/python_bindings/mod.rs`).

> El paquete se instala como `colmena-ai` (`pip install colmena-ai`) pero **el módulo a importar es
> `colmena`**.

## 📋 Tabla de Contenidos

- [Configuración Inicial](#configuración-inicial)
- [LLM: llamadas directas](#llm-llamadas-directas)
- [Streaming (async)](#streaming-async)
- [Conversaciones](#conversaciones)
- [Health checks y providers](#health-checks-y-providers)
- [Motor DAG desde Python](#motor-dag-desde-python)
- [Manejo de errores](#manejo-de-errores)
- [Buenas prácticas](#buenas-prácticas)

---

## ⚙️ Configuración Inicial

### Importar e inicializar

```python
import colmena

llm = colmena.ColmenaLlm()
```

`ColmenaLlm()` carga automáticamente las API keys desde el entorno al construirse.

### API Keys

```bash
# Recomendado: variables de entorno
export OPENAI_API_KEY="sk-..."
export GEMINI_API_KEY="AIza..."
export ANTHROPIC_API_KEY="sk-ant-..."
```

Para sobrescribir la key en una llamada puntual, usa `LlmConfigOptions.api_key` (ver abajo).

### Strings de provider

El parámetro `provider` acepta exactamente: `"openai"`, `"google"` (Gemini), `"anthropic"` y `"mock"`.

> ⚠️ Es `"google"`, **no** `"gemini"`.

---

## 🚀 LLM: llamadas directas

`call()` recibe:
- `messages`: lista de **dicts** `{"role": str, "content": str}` (roles: `system`, `user`, `assistant`).
- `provider`: string del proveedor.
- `options`: objeto `LlmConfigOptions` opcional con modelo y parámetros de sampling.

Devuelve la respuesta como `str`.

```python
import colmena

llm = colmena.ColmenaLlm()

opts = colmena.LlmConfigOptions()
opts.model = "gemini-2.5-flash"
opts.temperature = 0.7

respuesta = llm.call(
    messages=[{"role": "user", "content": "Hola, ¿cómo estás?"}],
    provider="google",
    options=opts,
)
print(respuesta)
```

### Configuración completa con `LlmConfigOptions`

Todos los parámetros de modelo/sampling viven en `LlmConfigOptions` y se pasan vía `options=`
(no existen kwargs sueltos como `model=` o `temperature=` en `call`):

```python
opts = colmena.LlmConfigOptions()
opts.api_key = "sk-..."         # opcional: override por llamada (si no, se toma del entorno)
opts.model = "gpt-4o"
opts.temperature = 0.8          # creatividad (0.0 - 2.0)
opts.max_tokens = 200           # longitud máxima de la respuesta
opts.top_p = 0.9                # nucleus sampling
opts.frequency_penalty = 0.5    # reduce repetición
opts.presence_penalty = 0.5     # fomenta temas nuevos

respuesta = llm.call(
    messages=[{"role": "user", "content": "Escribe un poema corto sobre Rust"}],
    provider="openai",
    options=opts,
)
print(respuesta)
```

Campos disponibles: `api_key`, `model`, `temperature`, `max_tokens`, `top_p`, `frequency_penalty`,
`presence_penalty`. Lo que no se asigna usa los defaults del proveedor.

### Mensaje de sistema + usuario

```python
respuesta = llm.call(
    messages=[
        {"role": "system", "content": "Eres un experto en Rust que responde en español."},
        {"role": "user", "content": "¿Qué ventajas tiene Rust sobre Python?"},
    ],
    provider="google",
)
print(respuesta)
```

### Comparar proveedores

```python
import colmena

def comparar_proveedores():
    llm = colmena.ColmenaLlm()
    pregunta = [{"role": "user", "content": "¿Qué es Rust en una frase?"}]

    for provider in ("openai", "google", "anthropic"):
        try:
            respuesta = llm.call(messages=pregunta, provider=provider)
            print(f"\n🤖 {provider.upper()}:\n{respuesta[:200]}...")
        except colmena.LlmException as e:
            print(f"❌ Error con {provider}: {e}")

comparar_proveedores()
```

---

## 🌊 Streaming (async)

`stream()` devuelve un **iterador asíncrono**. Debe consumirse con `async for` dentro de un event loop
— no es síncrono.

```python
import asyncio
import colmena

async def historia():
    llm = colmena.ColmenaLlm()
    # stream() devuelve un awaitable; hay que await-earlo para obtener el iterador async.
    stream = await llm.stream(
        messages=[{"role": "user", "content": "Cuenta una historia corta sobre un robot programador"}],
        provider="openai",
    )
    async for chunk in stream:
        print(chunk, end="", flush=True)
    print()

asyncio.run(historia())
```

`llm.stream(...)` devuelve un `Future`: primero `await` para obtener el iterador y luego `async for`.
Cada `chunk` es un `str` con el fragmento de texto. Los errores durante el streaming se propagan como
`colmena.LlmException` al iterar.

---

## 🗣️ Conversaciones

El historial se mantiene como una lista de dicts `{"role", "content"}`, alternando `user` y `assistant`:

```python
import colmena

def conversacion():
    llm = colmena.ColmenaLlm()
    historial = [
        {"role": "system", "content": "Eres un mentor de programación conciso."},
        {"role": "user", "content": "Soy dev Python y quiero aprender Rust. ¿Por dónde empiezo?"},
    ]

    respuesta = llm.call(messages=historial, provider="anthropic")
    print("🤖", respuesta)

    # Agregar la respuesta al historial y continuar
    historial.append({"role": "assistant", "content": respuesta})
    historial.append({"role": "user", "content": "¿Qué herramientas debo instalar?"})

    print("🤖", llm.call(messages=historial, provider="anthropic"))

conversacion()
```

---

## 🩺 Health checks y providers

```python
import colmena

llm = colmena.ColmenaLlm()

print(llm.get_providers())          # -> lista de proveedores disponibles, p.ej. ["openai", "google", ...]
print(llm.health_check("google"))   # -> bool
```

---

## 🧩 Motor DAG desde Python

Más allá de llamadas sueltas, Colmena ejecuta **workflows de agentes definidos como grafos JSON**
(nodos LLM, tools, HTTP, control de flujo, human-in-the-loop, etc.).

### Ejecutar un grafo: `run_dag`

Devuelve un **JSON string**; parséalo con `json.loads`.

```python
import colmena
import json

result_json = colmena.run_dag("tests/graphs/basic/power.json")
result = json.loads(result_json)
print(json.dumps(result, indent=2))
```

Firma completa:

```python
colmena.run_dag(
    file_path,                  # ruta al grafo JSON
    resume_id=None,             # id de resume para flujos suspend/resume
    resume_answer=None,         # respuesta en formato Q/A canónico (ver guía de suspend)
    inject_payload=None,        # dict inyectado como payload inicial del trigger
    include_extra_info=False,   # incluye metadata extra en el resultado
    agent_session_id=None,      # id estable de sesión de agente (memoria, resume, secure values)
)
```

> Para flujos con estado entre ejecuciones (suspend/resume, memoria conversacional), pasa siempre un
> `agent_session_id` estable.

### Validar un grafo en memoria: `validate_graph`

Acepta un **dict** y lanza `colmena.DagException` si el grafo no es válido.

```python
import colmena

graph = {
    "nodes": {
        "start":      {"type": "mock_input", "config": {"input": 5}},
        "pow_step":   {"type": "exponential", "config": {"exponent": 3}},
        "log_result": {"type": "log"},
    },
    "edges": [
        {"from": "start", "to": "pow_step"},
        {"from": "pow_step", "to": "log_result"},
    ],
}

colmena.validate_graph(graph)  # OK -> None ; inválido -> DagException
print("Grafo válido ✅")
```

### Servir webhooks: `serve_dag`

Levanta un servidor HTTP que expone los webhooks declarados en el grafo. **Es bloqueante.**

```python
import colmena

# El grafo declara un trigger_webhook en "/power".
# POST http://localhost:8080/power  con  {"input": 10}  -> 1000
colmena.serve_dag("tests/graphs/basic/power_webhook.json", host="0.0.0.0", port=8080)
```

### Inspeccionar el registro de nodos: `default_registry`

```python
import colmena

reg = colmena.default_registry()
print(reg.node_types())                    # -> lista de node types registrados

# Catálogo de sub-tools de un toolkit (sin conexión a DB)
catalogo = reg.toolkit_catalog("api_explorer", {})
print(catalogo)                            # -> [{"name", "description", "required"}, ...]
```

---

## 📄 `colmena.documents` (hojas CRDT) — 🚧 en progreso

> **El subsistema CRDT aún está en desarrollo.** El submódulo `colmena.documents` es funcional pero
> su superficie y su modelo de ejecución pueden cambiar; trátalo como experimental hasta que CRDT se
> cierre.

El submódulo `colmena.documents` expone operaciones sobre hojas de cálculo CRDT en proceso:

```python
import colmena

colmena.documents.add_sheet(artifact_id, name)              # -> sheet_id (str)
colmena.documents.list_sheets(artifact_id)                  # -> [{"sheet_id", "name"}, ...]
colmena.documents.read_sheet(artifact_id, sheet_id)         # -> {"A1": valor, "B2": valor, ...}
colmena.documents.write_sheet(artifact_id, sheet_id,
                              columns, rows, mode="replace") # mode: "replace" | "append"
```

El paquete repo-side `colmena_documents` (en `python/colmena_documents/`) añade ergonomía
**pandas** encima (read/write con DataFrames). No se publica en el wheel; es un helper del repo.

> ⚠️ **Limitación actual (CRDT en progreso):** `colmena.documents` requiere un runtime tokio activo
> en el contexto de Python; desde un script Python normal lanza
> `RuntimeError: no tokio runtime available`. A diferencia de `call`/`run_dag` (que crean su propio
> runtime), este submódulo aún no lo hace. **No es un descuido puntual sino parte del estado WIP de
> CRDT** — darle un runtime propio se abordará cuando el subsistema CRDT se termine (ver
> [`audit_python_bindings.md`](../developer_guide/audit_python_bindings.md)). Hoy es usable desde
> contextos que ya tienen runtime tokio (p.ej. el CLI), no desde Python plano.

## 📦 Identidad del paquete

- **`colmena-ai`** — nombre en PyPI (`pip install colmena-ai`).
- **`colmena`** — módulo a importar; es la extensión nativa (Rust/PyO3). Incluye el submódulo
  `colmena.documents` y, desde 0.4.0, type stubs (`colmena/__init__.pyi` + `py.typed`).
- **`colmena_documents`** — wrapper pandas puro-Python en el repo (`python/colmena_documents/`),
  **no** incluido en el wheel publicado.

## 🛡️ Manejo de errores

Las funciones de LLM lanzan `colmena.LlmException`; las del motor DAG lanzan `colmena.DagException`.

```python
import colmena

llm = colmena.ColmenaLlm()

try:
    respuesta = llm.call(
        messages=[{"role": "user", "content": "Explica qué es PyO3"}],
        provider="google",
    )
    print(respuesta)
except colmena.LlmException as e:
    print(f"❌ Error de LLM: {e}")

try:
    colmena.run_dag("grafo_inexistente.json")
except colmena.DagException as e:
    print(f"❌ Error de DAG: {e}")
```

### Wrapper con reintentos (patrón útil)

```python
import time
import colmena

class ColmenaWrapper:
    def __init__(self):
        self.llm = colmena.ColmenaLlm()

    def call_safe(self, messages, provider, options=None, max_retries=3):
        for attempt in range(max_retries):
            try:
                resp = self.llm.call(messages=messages, provider=provider, options=options)
                return {"success": True, "response": resp, "error": None}
            except colmena.LlmException as e:
                msg = str(e).lower()
                if "rate limit" in msg:
                    time.sleep(2 ** attempt)   # backoff exponencial
                    continue
                return {"success": False, "response": None, "error": str(e)}
        return {"success": False, "response": None, "error": "max retries reached"}

wrapper = ColmenaWrapper()
print(wrapper.call_safe([{"role": "user", "content": "Hola"}], "google"))
```

---

## 📝 Buenas prácticas

1. **Mensajes como dicts**: `messages` siempre es `list[dict]` con keys `role` y `content`. Si falta
   alguna, se lanza `LlmException`.
2. **Config vía `LlmConfigOptions`**: modelo y sampling van en el objeto `options`, no como kwargs.
3. **Streaming es async**: usa `async for` dentro de un event loop (`asyncio.run`).
4. **Provider `"google"`** para Gemini, nunca `"gemini"`.
5. **DAG con estado**: pasa `agent_session_id` estable para suspend/resume y memoria.
6. **`run_dag` devuelve JSON string**: recuerda `json.loads` sobre el resultado.

---

**🐝 Colmena** — *Orquestación de agentes de IA en Rust, con bindings nativos de Python.*
