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

-   `test_complex_scenarios.py`: Valida lógica de negocio, validación de roles y llamadas reales.
-   `test_streaming_scenarios.py`: Verifica la integridad de chunks en flujos de streaming síncronos.
-   `test_async_mock_streaming.py`: **(Nuevo v0.3.0)** Valida el comportamiento asíncrono (`async for`) de los bindings utilizando proveedores mockeados.
-   `test_mock_streaming.py`: Versión síncrona de tests con mocks para mayor velocidad en CI.

## Ejecutar Tests

### Prerrequisitos

1.  **Recompilar Bindings**:
    Antes de ejecutar tests, siempre asegúrate de cargar los cambios de Rust en Python:
    ```bash
    maturin develop
    ```

2.  **Configurar variables de entorno**:
    Crea un fichero `.env` en la raíz del proyecto (especialmente para `test_complex_scenarios.py` y `test_streaming_scenarios.py`).

### Comandos para Ejecutar Tests

Aunque los archivos se pueden ejecutar como scripts, recomendamos el uso de **`pytest`** para una mejor visualización y gestión de la suite:

```bash
# Ejecutar todos los tests de Python
pytest python/tests/

# Ejecutar un test específico con pytest
pytest python/tests/test_async_mock_streaming.py

# Ejecutar con output detallado y ver prints
pytest -v -s python/tests/test_complex_scenarios.py
```

## Escribir Nuevos Tests (Async Support)

Para las funcionalidades de streaming de la v0.3.0, es fundamental usar `async/await`. Aquí tienes una plantilla idiomática:

```python
import pytest
import colmena
import asyncio

# Usar decorator si usas pytest-asyncio
@pytest.mark.asyncio
async def test_streaming_async():
    """Prueba el nuevo iterador asíncrono de streaming."""
    llm = colmena.ColmenaLlm()
    messages = [{"role": "user", "content": "Hola"}]
    
    # El iterador soporta 'async for'
    stream = llm.stream(messages, provider="openai")
    
    chunks = []
    async for chunk in stream:
        chunks.append(chunk)
        
    assert len(chunks) > 0
    assert "Hola" in "".join(chunks)
```

## Mejores Prácticas

1.  **Usa Mocks para CI**: Prefiere `test_mock_streaming.py` en entornos de integración continua para no depender de API Keys externas.
2.  **Maturin Develop**: Siempre corre `maturin develop` después de modificar cualquier archivo `.rs` en `src/`.
3.  **Resultados Claros**: Si escribes un script manual (sin pytest), asegúrate de usar `exit(1)` en caso de fallo para alertar al sistema de build.
4.  **Consistencia de Tipos**: Recuerda que los diccionarios de entrada en Python deben cumplir con la estructura `{"role": str, "content": str}` requerida por el dominio de Rust.
```