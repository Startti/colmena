# Colmena - Ejemplos de Uso

Este documento contiene ejemplos prácticos de cómo usar la librería Colmena para la orquestación de agentes de IA.

## Instalación (Futuro)

```bash
pip install colmena
```

## Configuración de API Keys

### Opción 1: Variables de Entorno

```bash
export OPENAI_API_KEY="tu-api-key-aqui"
export GEMINI_API_KEY="tu-api-key-aqui"
export ANTHROPIC_API_KEY="tu-api-key-aqui"
```

### Opción 2: Archivo .env

```bash
# .env
OPENAI_API_KEY=tu-api-key-aqui
GEMINI_API_KEY=tu-api-key-aqui
ANTHROPIC_API_KEY=tu-api-key-aqui
```

## Ejemplos Básicos

### 1. Llamada Simple

```python
import colmena

# Inicializar la librería
llm = colmena.ColmenaLlm()

# Llamada básica con OpenAI
response = llm.call(
    messages=["¿Cuál es la capital de España?"],
    provider="openai"
)
print(response)  # Madrid
```

### 2. Especificar API Key y Modelo

```python
import colmena

llm = colmena.ColmenaLlm()

# Usar API key específica y modelo personalizado
response = llm.call(
    messages=["Explica la arquitectura hexagonal"],
    provider="openai",
    api_key="tu-api-key-específica",
    model="gpt-4",
    temperature=0.7,
    max_tokens=500
)
print(response)
```

### 3. Llamada con Contexto del Sistema

```python
import colmena

llm = colmena.ColmenaLlm()

# Llamada con mensaje del sistema
response = llm.call_with_context(
    system_message="Eres un experto en arquitectura de software. Responde de manera técnica y precisa.",
    messages=["¿Qué es Domain-Driven Design?"],
    provider="anthropic",
    model="claude-3-sonnet",
    temperature=0.3
)
print(response)
```

### 4. Conversación Completa

```python
import colmena

llm = colmena.ColmenaLlm()

# Conversación con historial completo
conversation = [
    ("system", "Eres un asistente de programación especializado en Rust."),
    ("user", "¿Cómo creo un struct en Rust?"),
    ("assistant", "En Rust, puedes crear un struct usando la palabra clave `struct`..."),
    ("user", "¿Y cómo implemento métodos para ese struct?")
]

response = llm.call_conversation(
    conversation=conversation,
    provider="gemini",
    model="gemini-2.5-flash",
    temperature=0.5
)
print(response)
```

## Streaming

### 5. Respuesta en Streaming

```python
import colmena

llm = colmena.ColmenaLlm()

# Streaming básico
stream = llm.stream(
    messages=["Cuenta una historia corta sobre un robot"],
    provider="openai",
    model="gpt-4",
    temperature=0.8
)

print("Respuesta en streaming:")
for chunk in stream:
    print(chunk, end="", flush=True)
print()  # Nueva línea al final
```

### 6. Streaming con Contexto

```python
import colmena

llm = colmena.ColmenaLlm()

# Streaming con mensaje del sistema
stream = llm.stream_with_context(
    system_message="Escribe en un estilo poético y melancólico.",
    messages=["Describe un atardecer en la ciudad"],
    provider="anthropic",
    model="claude-3-sonnet",
    temperature=0.9
)

response_text = ""
for chunk in stream:
    print(chunk, end="", flush=True)
    response_text += chunk

print(f"\n\nRespuesta completa: {len(response_text)} caracteres")
```

## Uso con Diferentes Proveedores

### 7. Comparar Respuestas de Múltiples Proveedores

```python
import colmena

llm = colmena.ColmenaLlm()

prompt = "Explica las ventajas de usar Rust para desarrollo de sistemas"
providers = ["openai", "gemini", "anthropic"]

print("Comparando respuestas de diferentes proveedores:\n")

for provider in providers:
    try:
        response = llm.call(
            messages=[prompt],
            provider=provider,
            temperature=0.7,
            max_tokens=300
        )
        print(f"=== {provider.upper()} ===")
        print(response)
        print()
    except Exception as e:
        print(f"Error con {provider}: {e}")
```

### 8. Health Check de Proveedores

```python
import colmena

llm = colmena.ColmenaLlm()

# Verificar qué proveedores están disponibles
providers = llm.get_providers()
print("Proveedores disponibles:", providers)

print("\nEstado de salud de los proveedores:")
for provider in providers:
    is_healthy = llm.health_check(provider)
    status = "✅ Disponible" if is_healthy else "❌ No disponible"
    print(f"{provider}: {status}")
```

## Casos de Uso Avanzados

### 9. Generación de Código con Validación

```python
import colmena

llm = colmena.ColmenaLlm()

def generate_rust_function(description: str) -> str:
    \"\"\"Genera una función en Rust basada en una descripción.\"\"\"

    system_prompt = \"\"\"Eres un experto programador en Rust.
    Genera código Rust limpio, idiomático y bien documentado.
    Incluye comentarios explicativos y manejo de errores cuando sea apropiado.\"\"\"

    user_prompt = f\"\"\"Genera una función en Rust que: {description}

    Requisitos:
    - Usa tipos apropiados
    - Incluye documentación con ///
    - Maneja errores con Result<T, E> si es necesario
    - Sigue las convenciones de Rust\"\"\"

    response = llm.call_with_context(
        system_message=system_prompt,
        messages=[user_prompt],
        provider="openai",
        model="gpt-4",
        temperature=0.3,
        max_tokens=800
    )

    return response

# Ejemplo de uso
rust_code = generate_rust_function(
    "calcule el factorial de un número entero positivo"
)
print("Código generado:")
print(rust_code)
```

### 10. Análisis de Sentimiento Multi-Proveedor

```python
import colmena

def analyze_sentiment_consensus(text: str) -> dict:
    \"\"\"Analiza el sentimiento usando múltiples proveedores para obtener consenso.\"\"\"

    llm = colmena.ColmenaLlm()

    prompt = f\"\"\"Analiza el sentimiento del siguiente texto y responde solo con una palabra: "positivo", "negativo" o "neutro".

    Texto: "{text}"

    Sentimiento:\"\"\"

    results = {}
    providers = ["openai", "gemini", "anthropic"]

    for provider in providers:
        try:
            response = llm.call(
                messages=[prompt],
                provider=provider,
                temperature=0.1,  # Baja temperatura para consistencia
                max_tokens=10
            )
            results[provider] = response.strip().lower()
        except Exception as e:
            results[provider] = f"error: {e}"

    # Calcular consenso
    sentiments = [v for v in results.values() if v in ["positivo", "negativo", "neutro"]]
    consensus = max(set(sentiments), key=sentiments.count) if sentiments else "indeterminado"

    return {
        "text": text,
        "individual_results": results,
        "consensus": consensus,
        "confidence": sentiments.count(consensus) / len(sentiments) if sentiments else 0
    }

# Ejemplo de uso
analysis = analyze_sentiment_consensus(
    "¡Estoy muy emocionado por este nuevo proyecto! Va a ser increíble."
)
print("Análisis de sentimiento:")
for key, value in analysis.items():
    print(f"{key}: {value}")
```

### 11. Generación Asistida con Streaming

```python
import colmena
import time

def creative_writing_assistant(topic: str, style: str = "narrativo"):
    \"\"\"Asistente de escritura creativa con streaming.\"\"\"

    llm = colmena.ColmenaLlm()

    system_prompt = f\"\"\"Eres un escritor creativo experto.
    Escribe en estilo {style}, usando un lenguaje rico y evocativo.
    Crea contenido original y atractivo.\"\"\"

    user_prompt = f\"Escribe un texto creativo sobre: {topic}\"

    print(f"🖋️  Generando texto sobre '{topic}' en estilo {style}...\n")

    stream = llm.stream_with_context(
        system_message=system_prompt,
        messages=[user_prompt],
        provider="anthropic",
        model="claude-3-sonnet",
        temperature=0.8,
        max_tokens=600
    )

    full_text = ""
    for chunk in stream:
        print(chunk, end="", flush=True)
        full_text += chunk
        time.sleep(0.05)  # Simular efecto de escritura

    print(f"\n\n📊 Estadísticas: {len(full_text)} caracteres, {len(full_text.split())} palabras")
    return full_text

# Ejemplo de uso
texto = creative_writing_assistant(
    topic="un café en una estación de tren durante una tormenta",
    style="poético"
)
```

## Manejo de Errores

### 12. Manejo Robusto de Errores

```python
import colmena

def safe_llm_call(messages, provider="openai", max_retries=3, **kwargs):
    \"\"\"Realiza una llamada LLM con manejo robusto de errores.\"\"\"

    llm = colmena.ColmenaLlm()

    for attempt in range(max_retries):
        try:
            response = llm.call(
                messages=messages,
                provider=provider,
                **kwargs
            )
            return {"success": True, "response": response, "attempts": attempt + 1}

        except colmena.LlmException as e:
            print(f"Intento {attempt + 1} falló: {e}")
            if attempt == max_retries - 1:
                return {"success": False, "error": str(e), "attempts": attempt + 1}
            time.sleep(2 ** attempt)  # Backoff exponencial

        except Exception as e:
            return {"success": False, "error": f"Error inesperado: {e}", "attempts": attempt + 1}

# Ejemplo de uso
result = safe_llm_call(
    messages=["Explica la computación cuántica"],
    provider="openai",
    model="gpt-4",
    max_tokens=400,
    max_retries=3
)

if result["success"]:
    print("✅ Llamada exitosa:")
    print(result["response"])
    print(f"Intentos necesarios: {result['attempts']}")
else:
    print("❌ Llamada falló:")
    print(result["error"])
    print(f"Intentos realizados: {result['attempts']}")
```

## Configuración Avanzada

### 13. Factory de Configuraciones

```python
import colmena

class LlmConfigFactory:
    \"\"\"Factory para crear configuraciones optimizadas por caso de uso.\"\"\"

    @staticmethod
    def creative_writing():
        return {
            "temperature": 0.9,
            "max_tokens": 800,
            "top_p": 0.95,
            "frequency_penalty": 0.3,
            "presence_penalty": 0.3
        }

    @staticmethod
    def code_generation():
        return {
            "temperature": 0.3,
            "max_tokens": 1000,
            "top_p": 0.8,
            "frequency_penalty": 0.0,
            "presence_penalty": 0.0
        }

    @staticmethod
    def factual_qa():
        return {
            "temperature": 0.1,
            "max_tokens": 300,
            "top_p": 0.7,
            "frequency_penalty": 0.0,
            "presence_penalty": 0.0
        }

# Uso del factory
llm = colmena.ColmenaLlm()

# Para escritura creativa
creative_response = llm.call(
    messages=["Escribe un poema sobre la tecnología"],
    provider="anthropic",
    **LlmConfigFactory.creative_writing()
)

# Para generación de código
code_response = llm.call(
    messages=["Crea una función que ordene una lista en Python"],
    provider="openai",
    **LlmConfigFactory.code_generation()
)

# Para respuestas factuales
factual_response = llm.call(
    messages=["¿Cuántos planetas hay en el sistema solar?"],
    provider="gemini",
    **LlmConfigFactory.factual_qa()
)
```

## Integración con Frameworks

### 14. Wrapper para FastAPI

```python
from fastapi import FastAPI, HTTPException
from pydantic import BaseModel
import colmena

app = FastAPI(title="Colmena API Gateway")
llm = colmena.ColmenaLlm()

class ChatRequest(BaseModel):
    message: str
    provider: str = "openai"
    model: str = None
    temperature: float = 0.7
    stream: bool = False

class ChatResponse(BaseModel):
    response: str
    provider: str
    model: str

@app.post("/chat", response_model=ChatResponse)
async def chat_endpoint(request: ChatRequest):
    try:
        if request.stream:
            # Para streaming, necesitarías usar StreamingResponse
            raise HTTPException(400, "Use /chat/stream for streaming responses")

        response = llm.call(
            messages=[request.message],
            provider=request.provider,
            model=request.model,
            temperature=request.temperature
        )

        return ChatResponse(
            response=response,
            provider=request.provider,
            model=request.model or f"default-{request.provider}"
        )

    except colmena.LlmException as e:
        raise HTTPException(400, f"LLM Error: {e}")
    except Exception as e:
        raise HTTPException(500, f"Internal Error: {e}")

@app.get("/health")
async def health_check():
    providers = llm.get_providers()
    health_status = {}

    for provider in providers:
        health_status[provider] = llm.health_check(provider)

    return {
        "status": "ok",
        "providers": health_status
    }

# Para ejecutar: uvicorn main:app --reload
```

## Mejores Prácticas

### 15. Clase Wrapper Reutilizable

```python
import colmena
from typing import List, Dict, Optional, Union
import asyncio
from concurrent.futures import ThreadPoolExecutor

class ColmenaManager:
    \"\"\"Wrapper de alto nivel para Colmena con funcionalidades adicionales.\"\"\"

    def __init__(self):
        self.llm = colmena.ColmenaLlm()
        self.executor = ThreadPoolExecutor(max_workers=3)

    def quick_ask(self, question: str, provider: str = "openai") -> str:
        \"\"\"Pregunta rápida con configuración optimizada.\"\"\"
        return self.llm.call(
            messages=[question],
            provider=provider,
            temperature=0.3,
            max_tokens=200
        )

    def creative_generate(self, prompt: str, provider: str = "anthropic") -> str:
        \"\"\"Generación creativa con parámetros optimizados.\"\"\"
        return self.llm.call(
            messages=[prompt],
            provider=provider,
            temperature=0.8,
            max_tokens=600,
            top_p=0.9,
            frequency_penalty=0.3,
            presence_penalty=0.3
        )

    def code_help(self, request: str, language: str = "python") -> str:
        \"\"\"Asistente de código especializado.\"\"\"
        system_msg = f"Eres un experto programador en {language}. Proporciona código limpio, bien documentado y sigue las mejores prácticas."

        return self.llm.call_with_context(
            system_message=system_msg,
            messages=[request],
            provider="openai",
            model="gpt-4",
            temperature=0.2,
            max_tokens=800
        )

    def parallel_ask(self, questions: List[str], provider: str = "openai") -> List[str]:
        \"\"\"Realiza múltiples preguntas en paralelo.\"\"\"
        def ask_single(question):
            return self.quick_ask(question, provider)

        futures = [self.executor.submit(ask_single, q) for q in questions]
        return [future.result() for future in futures]

    def get_system_status(self) -> Dict[str, bool]:
        \"\"\"Obtiene el estado de todos los proveedores.\"\"\"
        providers = self.llm.get_providers()
        return {provider: self.llm.health_check(provider) for provider in providers}

# Ejemplo de uso
manager = ColmenaManager()

# Pregunta rápida
answer = manager.quick_ask("¿Qué es la arquitectura hexagonal?")
print("Respuesta rápida:", answer)

# Generación creativa
story = manager.creative_generate("Una historia sobre un programador que encuentra un bug mágico")
print("Historia:", story)

# Ayuda con código
code = manager.code_help("Crea una función que calcule números primos", "rust")
print("Código:", code)

# Preguntas en paralelo
questions = [
    "¿Qué es REST?",
    "¿Qué es GraphQL?",
    "¿Qué es gRPC?"
]
answers = manager.parallel_ask(questions)
for q, a in zip(questions, answers):
    print(f"P: {q}")
    print(f"R: {a}\n")

# Estado del sistema
status = manager.get_system_status()
print("Estado del sistema:", status)
```

---

## Notas Importantes

1. **API Keys**: Siempre mantén tus API keys seguras y no las incluyas en el código fuente.

2. **Rate Limits**: Cada proveedor tiene límites de velocidad diferentes. Implementa lógica de retry con backoff exponencial.

3. **Costos**: Las llamadas a LLMs tienen costo. Monitorea tu uso, especialmente con modelos grandes como GPT-4.

4. **Timeouts**: Para aplicaciones de producción, siempre configura timeouts apropiados.

5. **Logging**: Implementa logging para debugging y monitoreo en producción.

6. **Validación**: Valida las respuestas de los LLMs antes de usarlas en aplicaciones críticas.

Este documento cubre los casos de uso más comunes. Para casos más específicos, consulta la documentación técnica en `docs/dds/MODULO_LLM_DISEÑO.md`.