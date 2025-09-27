# Estructura de Testing Python

## Estructura de Directorios

El proyecto sigue una separación clara entre los tests de Rust y los tests/ejemplos de Python:

```
colmena/
├── src/                    # Código fuente Rust con tests integrados
│   └── **/*.rs            # Contiene módulos #[cfg(test)]
├── tests/                 # Tests de integración Rust (si son necesarios)
├── python/
│   ├── tests/             # Tests Python para los bindings de Python
│   └── examples/          # Ejemplos y demos de Python
├── target/                # Artefactos de compilación Rust
└── docs/                  # Documentación
```

## Detalles de la Estructura Python

### `python/tests/` - Archivos de Test

Contiene tests automatizados para los bindings de Python. Sigue estas convenciones de nomenclatura:

- **Nomenclatura de archivos**: `test_<caracteristica>.py`
- **Nomenclatura de funciones de test**: `test_<caso_especifico>()`
- **Nomenclatura de clases de test**: `Test<Caracteristica>` (si usas clases)

#### Archivos de Test Actuales:

- `test_basic_roles.py` - Tests básicos de funcionalidad de roles
- `test_dictionary_interface.py` - Tests de interfaz de mensajes basada en diccionarios
- `test_gemini_integration.py` - Tests de integración con API real de Gemini
- `test_mock_provider.py` - Tests usando proveedores simulados
- `test_role_scenarios.py` - Tests de escenarios complejos de roles

### `python/examples/` - Archivos de Ejemplo

Contiene scripts de demostración y ejemplos de uso:

- **Nomenclatura de archivos**: `<nombre_descriptivo>.py`
- **Propósito**: Demostrar patrones de uso del mundo real

#### Archivos de Ejemplo Actuales:

- `complete_demo.py` - Demostración completa de todas las características

## Categorías de Tests

### 1. Tests Unitarios
- Prueban funciones y métodos individuales
- Usan proveedores simulados cuando es posible
- Ejecución rápida
- Ubicación: `python/tests/test_<componente>.py`

### 2. Tests de Integración
- Prueban interacción con APIs reales de LLM
- Requieren claves API
- Pueden ser más lentos
- Ubicación: `python/tests/test_<proveedor>_integration.py`

### 3. Tests de Escenarios
- Prueban flujos de trabajo complejos de múltiples pasos
- Prueban casos límite y condiciones de error
- Ubicación: `python/tests/test_<escenario>_scenarios.py`

## Ejecutar Tests

### Prerrequisitos

1. **Instalar en modo desarrollo**:
   ```bash
   # Desde la raíz del proyecto
   source .venv/bin/activate
   maturin develop
   ```

2. **Configurar variables de entorno** (para tests de integración):
   ```bash
   export GEMINI_API_KEY="tu_clave_api_aqui"
   export OPENAI_API_KEY="tu_clave_api_aqui"
   # etc.
   ```

### Comandos para Ejecutar Tests

```bash
# Ejecutar todos los tests de Python
cd python && python -m pytest tests/

# Ejecutar archivo de test específico
python tests/test_basic_roles.py

# Ejecutar solo tests de integración
python -m pytest tests/test_*_integration.py

# Ejecutar con salida verbose
python -m pytest tests/ -v

# Ejecutar función de test específica
python -m pytest tests/test_basic_roles.py::test_system_role -v
```

### Ejecutar Ejemplos

```bash
# Ejecutar demo completo
python python/examples/complete_demo.py

# Ejecutar cualquier ejemplo
python python/examples/<nombre_ejemplo>.py
```

## Escribir Nuevos Tests

### Plantilla de Archivo de Test

```python
#!/usr/bin/env python3
"""
Descripción del test aquí
"""

import pytest
try:
    import colmena
except ImportError as e:
    pytest.skip(f"colmena no disponible: {e}", allow_module_level=True)

class TestNombreCaracteristica:
    def test_funcionalidad_basica(self):
        \"\"\"Prueba funcionalidad básica\"\"\"
        llm = colmena.ColmenaLlm()
        # Implementación del test
        assert True

    def test_condiciones_error(self):
        \"\"\"Prueba manejo de errores\"\"\"
        llm = colmena.ColmenaLlm()
        with pytest.raises(colmena.LlmException):
            # Test de condición de error
            pass

def test_funcion_independiente():
    \"\"\"Función de test independiente\"\"\"
    pass

if __name__ == "__main__":
    # Permite ejecutar el archivo de test directamente
    pytest.main([__file__])
```

### Plantilla de Archivo de Ejemplo

```python
#!/usr/bin/env python3
\"\"\"
Ejemplo: Descripción de lo que este ejemplo demuestra
\"\"\"

try:
    import colmena
    print("✓ Colmena importado exitosamente")
except ImportError as e:
    print(f"✗ Error importando colmena: {e}")
    exit(1)

def demostrar_caracteristica():
    \"\"\"Demostrar uso específico de característica\"\"\"
    llm = colmena.ColmenaLlm()

    # Implementación del ejemplo
    print("🐝 Demostrando característica...")

    # Mostrar patrones de uso
    print("✅ Demostración de característica completa")

if __name__ == "__main__":
    demostrar_caracteristica()
```

## Mejores Prácticas

### Mejores Prácticas de Testing

1. **Independencia de Tests**: Cada test debe ser independiente y no depender de otros tests
2. **Nombres Claros**: Usar nombres descriptivos que expliquen qué se está probando
3. **Simular Dependencias Externas**: Usar mocks para APIs externas en tests unitarios
4. **Probar Casos Límite**: Incluir tests para condiciones de error y casos límite
5. **Retroalimentación Rápida**: Mantener tests rápidos; usar tests de integración con moderación

### Mejores Prácticas de Ejemplos

1. **Valor Educativo**: Los ejemplos deben enseñar a los usuarios cómo usar la biblioteca
2. **Escenarios del Mundo Real**: Mostrar patrones de uso prácticos
3. **Manejo de Errores**: Demostrar manejo adecuado de errores
4. **Salida Clara**: Proporcionar retroalimentación clara sobre lo que está sucediendo
5. **Auto-contenidos**: Los ejemplos deben funcionar independientemente

## Integración CI/CD

### Testing Automatizado

Los tests deben ejecutarse automáticamente en:
- Pull requests
- Commits a la rama main
- Ramas de release

### Entornos de Test

- **Tests Unitarios**: Ejecutar en todos los entornos
- **Tests de Integración**: Ejecutar solo cuando las claves API estén disponibles
- **Tests de Ejemplos**: Ejecutar para asegurar que los ejemplos funcionen

### Reportes de Test

Generar reportes de cobertura de tests y asegurar:
- Umbrales mínimos de cobertura
- Sin regresión en cobertura de tests
- Reporte claro de fallas de tests

## Solución de Problemas

### Problemas Comunes

1. **Errores de Importación**: Asegurar que `maturin develop` se ejecutó después de cambios en Rust
2. **Problemas de Clave API**: Verificar variables de entorno para tests de integración
3. **Problemas de Permisos**: Asegurar que el entorno virtual esté activado
4. **Problemas de Ruta**: Ejecutar tests desde el directorio correcto

### Tips de Debug

1. **Salida Verbose**: Usar flag `-v` con pytest
2. **Tests Específicos**: Ejecutar archivos de test individuales para aislar problemas
3. **Debug con Print**: Agregar declaraciones print para debugging
4. **Archivos de Log**: Verificar logs de error en directorios temporales

## Estándar de Nomenclatura

### Archivos de Test
- `test_` + descripción en inglés
- Usar snake_case
- Ser específico sobre lo que se prueba

### Archivos de Ejemplo
- Nombres descriptivos en inglés
- Sin prefijo especial
- Usar snake_case

### Funciones
- `test_` + descripción específica para tests
- Nombres descriptivos para ejemplos
- Documentar propósito en docstring

## Estructura Final Implementada

```
python/
├── tests/
│   ├── test_basic_roles.py          # Tests básicos de roles
│   ├── test_dictionary_interface.py # Tests de interfaz de diccionarios
│   ├── test_gemini_integration.py   # Tests de integración con Gemini
│   ├── test_mock_provider.py        # Tests con proveedores simulados
│   └── test_role_scenarios.py       # Tests de escenarios complejos
└── examples/
    └── complete_demo.py             # Demo completo del sistema
```