# 🐍 Ejemplos de Uso en Python - Colmena

Esta guía contiene ejemplos prácticos y completos de cómo usar Colmena en Python.

## 📋 Tabla de Contenidos

- [Configuración Inicial](#configuración-inicial)
- [Ejemplos Básicos](#ejemplos-básicos)
- [Ejemplos Avanzados](#ejemplos-avanzados)
- [Casos de Uso Reales](#casos-de-uso-reales)
- [Mejores Prácticas](#mejores-prácticas)
- [Recetas Útiles](#recetas-útiles)

## ⚙️ Configuración Inicial

### Importar Colmena

```python
import colmena
import os
from typing import List, Dict, Optional

# Inicializar la librería
llm = colmena.ColmenaLlm()
```

### Configurar API Keys

```python
# Método 1: Variables de entorno (recomendado)
os.environ['OPENAI_API_KEY'] = 'tu-openai-key'
os.environ['GEMINI_API_KEY'] = 'tu-gemini-key'
os.environ['ANTHROPIC_API_KEY'] = 'tu-anthropic-key'

# Método 2: Configuración directa (para desarrollo)
OPENAI_KEY = "tu-openai-key"
GEMINI_KEY = "tu-gemini-key"
ANTHROPIC_KEY = "tu-anthropic-key"
```

## 🚀 Ejemplos Básicos

### 1. Primera Llamada Simple

```python
import colmena

def primera_llamada():
    """Ejemplo más básico posible"""
    llm = colmena.ColmenaLlm()

    response = llm.call(
        messages=["Hola, ¿cómo estás?"],
        provider="gemini",
        api_key="tu-gemini-api-key"
    )

    print(f"Respuesta: {response}")

# Ejecutar
primera_llamada()
```

### 2. Llamada con Configuración

```python
def llamada_configurada():
    """Llamada con parámetros de configuración"""
    llm = colmena.ColmenaLlm()

    response = llm.call(
        messages=["Escribe un poema corto sobre Rust"],
        provider="openai",
        model="gpt-4",
        api_key="tu-openai-key",
        temperature=0.8,      # Más creatividad
        max_tokens=200,       # Respuesta corta
        top_p=0.9            # Diversidad en la selección
    )

    print(f"Poema generado:\n{response}")

llamada_configurada()
```

### 3. Comparar Proveedores

```python
def comparar_proveedores():
    """Comparar respuestas de diferentes proveedores"""
    llm = colmena.ColmenaLlm()
    pregunta = "¿Qué ventajas tiene Rust sobre Python?"

    proveedores = [
        ("openai", "gpt-4", "tu-openai-key"),
        ("gemini", "gemini-1.5-flash", "tu-gemini-key"),
        ("anthropic", "claude-3-sonnet-20240229", "tu-anthropic-key")
    ]

    for provider, model, api_key in proveedores:
        try:
            response = llm.call(
                messages=[pregunta],
                provider=provider,
                model=model,
                api_key=api_key
            )
            print(f"\n🤖 {provider.upper()}:")
            print(f"{response[:200]}...")
        except colmena.LlmException as e:
            print(f"❌ Error con {provider}: {e}")

comparar_proveedores()
```

## 🌊 Streaming

### 4. Streaming Básico

```python
def streaming_basico():
    """Ejemplo de streaming con output en tiempo real"""
    llm = colmena.ColmenaLlm()

    print("🤖 Generando historia...")

    chunks = llm.stream(
        messages=["Cuenta una historia corta sobre un robot que aprende a programar"],
        provider="gemini",
        api_key="tu-gemini-key"
    )

    print("\n📖 Historia:")
    for chunk in chunks:
        print(chunk, end="", flush=True)

    print("\n\n✅ Historia completada!")

streaming_basico()
```

### 5. Streaming con Control

```python
import time

def streaming_controlado():
    """Streaming con control de velocidad y paradas"""
    llm = colmena.ColmenaLlm()

    chunks = llm.stream(
        messages=["Explica paso a paso cómo compilar un proyecto Rust"],
        provider="openai",
        model="gpt-4",
        api_key="tu-openai-key"
    )

    print("🔧 Explicación paso a paso:\n")

    chunk_count = 0
    for chunk in chunks:
        print(chunk, end="", flush=True)

        chunk_count += 1
        if chunk_count % 10 == 0:  # Pausa cada 10 chunks
            time.sleep(0.1)

    print("\n\n✅ Explicación completada!")

streaming_controlado()
```

## 🗣️ Conversaciones

### 6. Conversación Simple

```python
def conversacion_simple():
    """Mantener contexto en múltiples intercambios"""
    llm = colmena.ColmenaLlm()

    # Historial de conversación
    mensajes = [
        "Hola, soy un desarrollador Python que quiere aprender Rust",
        "¿Por dónde debería empezar?",
        "¿Qué herramientas necesito instalar?"
    ]

    response = llm.call(
        messages=mensajes,
        provider="anthropic",
        api_key="tu-anthropic-key",
        temperature=0.7
    )

    print("🤖 Asistente:")
    print(response)

conversacion_simple()
```

### 7. Conversación Interactiva

```python
def conversacion_interactiva():
    """Conversación interactiva con el usuario"""
    llm = colmena.ColmenaLlm()
    historial = []

    print("🤖 ¡Hola! Soy tu asistente de programación. Escribe 'salir' para terminar.")

    while True:
        # Obtener input del usuario
        user_input = input("\n👤 Tú: ")

        if user_input.lower() in ['salir', 'exit', 'quit']:
            print("👋 ¡Hasta luego!")
            break

        # Agregar mensaje del usuario al historial
        historial.append(user_input)

        try:
            # Generar respuesta
            response = llm.call(
                messages=historial,
                provider="gemini",
                api_key="tu-gemini-key",
                temperature=0.7
            )

            # Mostrar respuesta
            print(f"\n🤖 Asistente: {response}")

            # Agregar respuesta del asistente al historial
            historial.append(response)

        except colmena.LlmException as e:
            print(f"❌ Error: {e}")
            # No agregar al historial si hay error

# conversacion_interactiva()  # Descomenta para ejecutar
```

## 🧠 Casos de Uso Avanzados

### 8. Análisis de Código

```python
def analizar_codigo():
    """Usar IA para analizar y mejorar código"""

    codigo_python = """
def calcular_fibonacci(n):
    if n <= 1:
        return n
    else:
        return calcular_fibonacci(n-1) + calcular_fibonacci(n-2)

# Usar la función
resultado = calcular_fibonacci(10)
print(resultado)
"""

    llm = colmena.ColmenaLlm()

    prompt = f"""
Analiza este código Python y sugiere mejoras:

```python
{codigo_python}
```

Por favor proporciona:
1. Análisis del algoritmo
2. Problemas de performance
3. Versión optimizada
4. Explicación de las mejoras
"""

    response = llm.call(
        messages=[prompt],
        provider="openai",
        model="gpt-4",
        api_key="tu-openai-key",
        temperature=0.3  # Menos creatividad, más precisión
    )

    print("🔍 Análisis de Código:")
    print(response)

analizar_codigo()
```

### 9. Generación de Documentación

```python
def generar_documentacion():
    """Generar documentación automática para funciones"""

    funcion_rust = """
pub fn merge_sort<T: Ord + Clone>(arr: &mut [T]) {
    let len = arr.len();
    if len <= 1 {
        return;
    }

    let mid = len / 2;
    let mut left = arr[0..mid].to_vec();
    let mut right = arr[mid..].to_vec();

    merge_sort(&mut left);
    merge_sort(&mut right);

    merge(&left, &right, arr);
}
"""

    llm = colmena.ColmenaLlm()

    prompt = f"""
Genera documentación completa para esta función Rust:

```rust
{funcion_rust}
```

Incluye:
1. Descripción de la función
2. Parámetros
3. Valor de retorno
4. Complejidad temporal
5. Ejemplo de uso
6. Notas sobre performance
"""

    response = llm.call(
        messages=[prompt],
        provider="anthropic",
        api_key="tu-anthropic-key",
        temperature=0.2
    )

    print("📖 Documentación Generada:")
    print(response)

generar_documentacion()
```

### 10. Traductor de Código

```python
def traducir_codigo():
    """Traducir código entre lenguajes"""

    codigo_python = """
class CalculadoraBasica:
    def __init__(self):
        self.historial = []

    def sumar(self, a, b):
        resultado = a + b
        self.historial.append(f"{a} + {b} = {resultado}")
        return resultado

    def obtener_historial(self):
        return self.historial.copy()

# Uso
calc = CalculadoraBasica()
print(calc.sumar(5, 3))
print(calc.obtener_historial())
"""

    llm = colmena.ColmenaLlm()

    prompt = f"""
Traduce este código Python a Rust manteniendo la misma funcionalidad:

```python
{codigo_python}
```

Requisitos:
1. Usar structs e impl en lugar de clases
2. Manejar ownership apropiadamente
3. Usar tipos seguros
4. Incluir comentarios explicativos
5. Seguir convenciones de Rust
"""

    response = llm.call(
        messages=[prompt],
        provider="gemini",
        api_key="tu-gemini-key",
        temperature=0.3
    )

    print("🔄 Código Traducido:")
    print(response)

traducir_codigo()
```

## 🛠️ Utilidades Prácticas

### 11. Wrapper con Manejo de Errores

```python
class ColmenaWrapper:
    """Wrapper con manejo robusto de errores"""

    def __init__(self):
        self.llm = colmena.ColmenaLlm()
        self.default_config = {
            "temperature": 0.7,
            "max_tokens": 1000,
        }

    def call_safe(self, messages, provider, api_key=None, **kwargs):
        """Llamada con manejo de errores y reintentos"""

        # Combinar configuración por defecto con parámetros
        config = {**self.default_config, **kwargs}

        max_retries = 3
        for attempt in range(max_retries):
            try:
                response = self.llm.call(
                    messages=messages,
                    provider=provider,
                    api_key=api_key,
                    **config
                )
                return {"success": True, "response": response, "error": None}

            except colmena.LlmException as e:
                error_msg = str(e)

                # Diferentes estrategias según el error
                if "rate limit" in error_msg.lower():
                    wait_time = 2 ** attempt  # Backoff exponencial
                    print(f"⏳ Rate limit alcanzado, esperando {wait_time}s...")
                    time.sleep(wait_time)
                    continue
                elif "api key" in error_msg.lower():
                    return {"success": False, "response": None, "error": "API key inválida"}
                else:
                    return {"success": False, "response": None, "error": error_msg}

            except Exception as e:
                return {"success": False, "response": None, "error": f"Error inesperado: {e}"}

        return {"success": False, "response": None, "error": "Máximo de reintentos alcanzado"}

    def stream_safe(self, messages, provider, api_key=None, **kwargs):
        """Streaming con manejo de errores"""
        config = {**self.default_config, **kwargs}

        try:
            return self.llm.stream(
                messages=messages,
                provider=provider,
                api_key=api_key,
                **config
            )
        except Exception as e:
            print(f"❌ Error en streaming: {e}")
            return None

# Ejemplo de uso
def usar_wrapper():
    wrapper = ColmenaWrapper()

    result = wrapper.call_safe(
        messages=["Explica qué es PyO3"],
        provider="gemini",
        api_key="tu-gemini-key"
    )

    if result["success"]:
        print(f"✅ Respuesta: {result['response']}")
    else:
        print(f"❌ Error: {result['error']}")

usar_wrapper()
```

### 12. Sistema de Cache

```python
import hashlib
import json
import os
from pathlib import Path

class ColmenaCache:
    """Sistema de cache para respuestas de Colmena"""

    def __init__(self, cache_dir="./cache"):
        self.cache_dir = Path(cache_dir)
        self.cache_dir.mkdir(exist_ok=True)
        self.llm = colmena.ColmenaLlm()

    def _get_cache_key(self, messages, provider, **kwargs):
        """Generar clave de cache basada en parámetros"""
        # Crear un hash de los parámetros
        data = {
            "messages": messages,
            "provider": provider,
            **kwargs
        }
        json_str = json.dumps(data, sort_keys=True)
        return hashlib.md5(json_str.encode()).hexdigest()

    def _get_cache_path(self, cache_key):
        """Obtener ruta del archivo de cache"""
        return self.cache_dir / f"{cache_key}.json"

    def call_cached(self, messages, provider, api_key=None, use_cache=True, **kwargs):
        """Llamada con cache automático"""

        if use_cache:
            cache_key = self._get_cache_key(messages, provider, **kwargs)
            cache_path = self._get_cache_path(cache_key)

            # Verificar si existe en cache
            if cache_path.exists():
                with open(cache_path, 'r', encoding='utf-8') as f:
                    cached_data = json.load(f)
                print(f"📄 Respuesta obtenida del cache")
                return cached_data["response"]

        # Si no está en cache, hacer llamada real
        try:
            response = self.llm.call(
                messages=messages,
                provider=provider,
                api_key=api_key,
                **kwargs
            )

            # Guardar en cache si está habilitado
            if use_cache:
                cache_data = {
                    "messages": messages,
                    "provider": provider,
                    "response": response,
                    "timestamp": time.time()
                }
                with open(cache_path, 'w', encoding='utf-8') as f:
                    json.dump(cache_data, f, ensure_ascii=False, indent=2)
                print(f"💾 Respuesta guardada en cache")

            return response

        except Exception as e:
            print(f"❌ Error en llamada: {e}")
            raise

    def clear_cache(self):
        """Limpiar todo el cache"""
        for cache_file in self.cache_dir.glob("*.json"):
            cache_file.unlink()
        print("🗑️ Cache limpiado")

    def cache_stats(self):
        """Estadísticas del cache"""
        cache_files = list(self.cache_dir.glob("*.json"))
        total_size = sum(f.stat().st_size for f in cache_files)

        print(f"📊 Estadísticas del Cache:")
        print(f"   Archivos: {len(cache_files)}")
        print(f"   Tamaño total: {total_size / 1024:.2f} KB")

# Ejemplo de uso
def usar_cache():
    cache = ColmenaCache()

    # Primera llamada (se guarda en cache)
    response1 = cache.call_cached(
        messages=["¿Qué es Rust?"],
        provider="gemini",
        api_key="tu-gemini-key"
    )

    # Segunda llamada (se obtiene del cache)
    response2 = cache.call_cached(
        messages=["¿Qué es Rust?"],
        provider="gemini",
        api_key="tu-gemini-key"
    )

    cache.cache_stats()

usar_cache()
```

### 13. Batch Processing

```python
import concurrent.futures
from typing import List, Dict

def procesar_lote():
    """Procesar múltiples consultas en paralelo"""

    llm = colmena.ColmenaLlm()

    # Lista de consultas a procesar
    consultas = [
        {
            "id": "rust_basics",
            "messages": ["¿Cuáles son los conceptos básicos de Rust?"],
            "provider": "gemini"
        },
        {
            "id": "python_vs_rust",
            "messages": ["Compara Python y Rust para desarrollo web"],
            "provider": "openai",
            "model": "gpt-4"
        },
        {
            "id": "async_programming",
            "messages": ["Explica programación asíncrona en Rust"],
            "provider": "anthropic"
        }
    ]

    def procesar_consulta(consulta):
        """Procesar una consulta individual"""
        try:
            response = llm.call(
                messages=consulta["messages"],
                provider=consulta["provider"],
                model=consulta.get("model", ""),
                api_key=f"tu-{consulta['provider']}-key",
                temperature=0.7
            )

            return {
                "id": consulta["id"],
                "success": True,
                "response": response,
                "error": None
            }

        except Exception as e:
            return {
                "id": consulta["id"],
                "success": False,
                "response": None,
                "error": str(e)
            }

    # Procesar en paralelo
    print("🔄 Procesando consultas en paralelo...")

    with concurrent.futures.ThreadPoolExecutor(max_workers=3) as executor:
        # Enviar todas las consultas
        futures = {
            executor.submit(procesar_consulta, consulta): consulta["id"]
            for consulta in consultas
        }

        # Recoger resultados
        resultados = {}
        for future in concurrent.futures.as_completed(futures):
            resultado = future.result()
            resultados[resultado["id"]] = resultado

            if resultado["success"]:
                print(f"✅ {resultado['id']}: Completado")
            else:
                print(f"❌ {resultado['id']}: Error - {resultado['error']}")

    # Mostrar resultados
    print("\n📋 Resultados:")
    for consulta_id, resultado in resultados.items():
        if resultado["success"]:
            print(f"\n🔍 {consulta_id}:")
            print(f"{resultado['response'][:150]}...")

procesar_lote()
```

## 📝 Mejores Prácticas

### 14. Configuración de Producción

```python
import logging
from dataclasses import dataclass
from typing import Optional

@dataclass
class ColmenaConfig:
    """Configuración robusta para producción"""
    openai_key: Optional[str] = None
    gemini_key: Optional[str] = None
    anthropic_key: Optional[str] = None
    default_provider: str = "gemini"
    default_temperature: float = 0.7
    default_max_tokens: int = 1000
    enable_logging: bool = True
    log_level: str = "INFO"

class ColmenaProduction:
    """Clase para uso en producción"""

    def __init__(self, config: ColmenaConfig):
        self.config = config
        self.llm = colmena.ColmenaLlm()

        # Configurar logging
        if config.enable_logging:
            logging.basicConfig(
                level=getattr(logging, config.log_level),
                format='%(asctime)s - %(name)s - %(levelname)s - %(message)s'
            )
            self.logger = logging.getLogger('colmena')
        else:
            self.logger = None

    def _log(self, level: str, message: str):
        """Log interno"""
        if self.logger:
            getattr(self.logger, level)(message)

    def call(self, messages, provider=None, **kwargs):
        """Llamada con configuración de producción"""

        # Usar proveedor por defecto si no se especifica
        if provider is None:
            provider = self.config.default_provider

        # Obtener API key
        api_key = kwargs.get('api_key')
        if not api_key:
            key_map = {
                'openai': self.config.openai_key,
                'gemini': self.config.gemini_key,
                'anthropic': self.config.anthropic_key
            }
            api_key = key_map.get(provider)

            if not api_key:
                raise ValueError(f"No API key configurada para {provider}")

        # Aplicar configuración por defecto
        call_kwargs = {
            'temperature': self.config.default_temperature,
            'max_tokens': self.config.default_max_tokens,
            **kwargs,
            'api_key': api_key
        }

        self._log('info', f"Llamada a {provider} con {len(messages)} mensajes")

        try:
            response = self.llm.call(
                messages=messages,
                provider=provider,
                **call_kwargs
            )

            self._log('info', f"Respuesta exitosa de {provider} ({len(response)} caracteres)")
            return response

        except Exception as e:
            self._log('error', f"Error en llamada a {provider}: {e}")
            raise

# Configuración y uso
def ejemplo_produccion():
    config = ColmenaConfig(
        gemini_key="tu-gemini-key",
        openai_key="tu-openai-key",
        default_provider="gemini",
        enable_logging=True
    )

    colmena_prod = ColmenaProduction(config)

    try:
        response = colmena_prod.call(
            messages=["Explica arquitectura hexagonal brevemente"]
        )
        print(f"Respuesta: {response}")
    except Exception as e:
        print(f"Error: {e}")

ejemplo_produccion()
```

## 🎯 Recetas Rápidas

### One-liners Útiles

```python
# Respuesta rápida
respuesta = colmena.ColmenaLlm().call(["Tu pregunta"], "gemini", api_key="key")

# Streaming en una línea
list(colmena.ColmenaLlm().stream(["Cuenta algo"], "gemini", api_key="key"))

# Comparar proveedores rápidamente
[colmena.ColmenaLlm().call(["¿Qué es Rust?"], p, api_key="key") for p in ["openai", "gemini"]]
```

### Scripts de Utilidad

```python
# test_providers.py - Verificar todos los proveedores
def test_all_providers():
    providers = {
        "openai": "tu-openai-key",
        "gemini": "tu-gemini-key",
        "anthropic": "tu-anthropic-key"
    }

    llm = colmena.ColmenaLlm()

    for provider, key in providers.items():
        try:
            response = llm.call(["Test"], provider, api_key=key)
            print(f"✅ {provider}: OK")
        except:
            print(f"❌ {provider}: FAIL")

# benchmark.py - Medir performance
import time

def benchmark_provider(provider, api_key, iterations=5):
    llm = colmena.ColmenaLlm()
    times = []

    for i in range(iterations):
        start = time.time()
        llm.call([f"Test {i}"], provider, api_key=api_key)
        times.append(time.time() - start)

    avg_time = sum(times) / len(times)
    print(f"{provider}: {avg_time:.2f}s promedio")
```

---

**🐝 Colmena** - *Potenciando el desarrollo de IA con Python y Rust*