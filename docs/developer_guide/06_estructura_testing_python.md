# 🧪 Estructura de Testing en Python

## Estructura de Directorios

El proyecto mantiene una separación clara entre los tests de Rust y los de Python:

```
colmena/
├── src/                    # Código fuente Rust (con sus tests unitarios)
├── tests/                  # Tests de integración de Rust
├── python/
│   └── tests/              # Tests de Python para los bindings
├── target/                 # Artefactos de compilación de Rust
└── docs/                   # Documentación
```

## `python/tests/` - Archivos de Test

Este directorio contiene los tests automatizados para los bindings de Python.

### Archivos de Test Actuales:

-   `test_complex_scenarios.py`: Contiene tests de integración que validan la lógica de negocio principal. Prueba escenarios como la validación de roles de mensajes (ej. no permitir mensajes consecutivos del mismo rol) y realiza llamadas reales a los proveedores para asegurar la correcta integración.
-   `test_streaming_scenarios.py`: Se enfoca en verificar la funcionalidad de streaming. Asegura que los clientes puedan recibir chunks de respuesta de los proveedores que soportan streaming y que el manejo de errores también funcione en este modo.

## Categorías de Tests

Los tests de Python se centran principalmente en la **integración** y los **escenarios de uso**, ya que la lógica de dominio de bajo nivel es probada exhaustivamente en Rust.

### 1. Tests de Escenarios Complejos
-   Prueban flujos de trabajo y validaciones de la lógica de negocio.
-   Verifican el manejo de errores para entradas inválidas (ej. formato de mensajes incorrecto).
-   Ubicación: `python/tests/test_complex_scenarios.py`

### 2. Tests de Streaming
-   Prueban la funcionalidad de streaming de principio a fin.
-   Aseguran que los datos se reciben correctamente en formato de chunks.
-   Ubicación: `python/tests/test_streaming_scenarios.py`

## Ejecutar Tests

### Prerrequisitos

1.  **Instalar en modo de desarrollo**:
    ```bash
    # Desde la raíz del proyecto
    source .venv/bin/activate
    uv pip install -e ".[dev]"
    ```

2.  **Configurar variables de entorno** (para tests de integración):
    Crea un fichero `.env` en la raíz del proyecto con tus API keys:
    ```
    GEMINI_API_KEY="tu_clave_api_aqui"
    OPENAI_API_KEY="tu_clave_api_aqui"
    ANTHROPIC_API_KEY="tu_clave_api_aqui"
    ```

### Comandos para Ejecutar Tests

Los tests están diseñados para ser ejecutados directamente como scripts, lo que facilita el debugging.

```bash
# Ejecutar los tests de escenarios complejos
python python/tests/test_complex_scenarios.py

# Ejecutar los tests de streaming
python python/tests/test_streaming_scenarios.py
```

## Escribir Nuevos Tests

Sigue la estructura de los ficheros existentes. Cada fichero de test es un script ejecutable que reporta los resultados.

### Plantilla de Archivo de Test

```python
#!/usr/-bin/env python3
"""
Descripción del propósito de este fichero de test.
"""
import os
from dotenv import load_dotenv

load_dotenv()

try:
    import colmena
    print("✓ Módulo colmena importado correctamente")
except ImportError as e:
    print(f"✗ Error importando colmena: {e}")
    exit(1)

# --- Configuración de Test ---
PROVIDER = "gemini"
MODEL = "gemini-1.5-flash"

# --- Definición de Tests ---

def test_nombre_del_escenario():
    """Descripción de lo que prueba este test."""
    llm = colmena.ColmenaLlm()
    messages = [{"role": "user", "content": "Mensaje de prueba"}]
    
    try:
        response = llm.call(messages, provider=PROVIDER, model=MODEL)
        assert "condicion" in response, "La respuesta no fue la esperada"
        print("✅ PASSED: El escenario se completó correctamente.")
        return True
    except Exception as e:
        print(f"❌ FAILED: El test falló con un error: {e}")
        return False

if __name__ == "__main__":
    print("🧪 Ejecutando tests para Escenario X")
    print("="*60)

    tests = [test_nombre_del_escenario]
    passed = sum(1 for test in tests if test())
    total = len(tests)

    print(f"\n🎯 Resultados: {passed}/{total} tests pasados")
    print("="*60)

    if passed != total:
        exit(1)
```

## Mejores Prácticas

1.  **Independencia**: Cada función de test debe ser independiente.
2.  **Nombres Claros**: Usa nombres descriptivos que expliquen qué se está probando.
3.  **Feedback Claro**: Imprime mensajes claros sobre el resultado de cada test (`✅ PASSED` o `❌ FAILED`).
4.  **API Keys**: No hardcodees API keys. Usa siempre variables de entorno.
5.  **Salida Anticipada**: Usa `exit(1)` al final del script si algún test falla, para que los sistemas de CI puedan detectar el fallo.
```