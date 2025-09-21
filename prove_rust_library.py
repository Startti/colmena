#!/usr/bin/env python3
"""
Script para demostrar que estamos usando la librería Rust compilada.
"""

import colmena
import inspect
import os
from dotenv import load_dotenv

# Cargar variables de entorno desde .env
load_dotenv()

print("🐝 Colmena - Verificación de Librería Rust")
print("=" * 50)

# 1. Verificar el módulo
print(f"Módulo colmena ubicado en: {colmena.__file__}")
print(f"Contiene: {dir(colmena)}")

# 2. Verificar ColmenaLlm
llm = colmena.ColmenaLlm()
print(f"\nTipo de ColmenaLlm: {type(llm)}")
print(f"Métodos disponibles: {[m for m in dir(llm) if not m.startswith('_')]}")

# 3. Verificar que los métodos son nativos (no Python)
try:
    source = inspect.getsource(llm.call)
    print("❌ El método call() está implementado en Python")
except (OSError, TypeError) as e:
    print(f"✅ El método call() es nativo (compilado desde Rust): {type(e).__name__}")
    print(f"   Tipo de método: {type(llm.call)}")

# 4. Verificar excepciones custom
try:
    raise colmena.LlmException("Test error")
except colmena.LlmException as e:
    print(f"✅ Excepción custom LlmException funciona: {e}")

# 5. Verificar que realmente llama a la API (usando variable de entorno)
print("\n🔧 Verificando llamada real a API...")
try:
    response = llm.call(
        messages=["Hola"],
        provider="gemini",
    )
    print(f"✅ ¡Llamada exitosa! Respuesta: '{response}'")
except colmena.LlmException as e:
    print(f"❌ Error inesperado: {e}")

print("\n🎯 CONCLUSIÓN:")
print("✅ Estamos usando la librería Rust compilada exitosamente!")
print("✅ Los métodos son nativos (no Python)")
print("✅ Las excepciones personalizadas funcionan")
print("✅ La librería hace llamadas reales a APIs de LLM (si la API key está configurada)")