#!/usr/bin/env python3
"""
Test de integración con API real de Gemini para demostrar el sistema de roles con diccionarios.
"""

try:
    import colmena
    print("✓ Módulo colmena importado correctamente")
except ImportError as e:
    print(f"✗ Error importando colmena: {e}")
    exit(1)

def test_with_real_gemini_api():
    """Test with real Gemini API using dictionary format"""

    llm = colmena.ColmenaLlm()

    # API key proporcionada
    api_key = "AIzaSyDttaCigUOn2H6-njFIzBNxWhd2J5dOUwU"

    print("🔑 Using provided Gemini API key")
    print("🎯 Testing dictionary-based role system\n")

    # Test 1: Conversación básica con roles múltiples
    print("📋 Test 1: Basic conversation with multiple roles")
    messages_basic = [
        {"role": "system", "content": "You are a helpful assistant. Respond concisely in Spanish."},
        {"role": "user", "content": "¿Puedes explicarme qué son los roles en una conversación con IA?"},
        {"role": "assistant", "content": "Los roles definen quién habla: 'system' da instrucciones, 'user' pregunta, 'assistant' responde."},
        {"role": "user", "content": "¿Y para qué sirve tener múltiples roles?"}
    ]

    try:
        response1 = llm.call_messages(
            messages=messages_basic,
            provider="gemini",
            api_key=api_key,
            model="gemini-2.5-flash",
            temperature=0.7,
            max_tokens=500
        )
        print(f"✅ Respuesta básica: {response1}")
        print()
    except Exception as e:
        print(f"❌ Error en test básico: {e}")
        return False

    # Test 2: Conversación compleja con cambios de contexto
    print("📋 Test 2: Complex conversation with context changes")
    messages_complex = [
        {"role": "system", "content": "Eres un tutor de programación. Sé práctico y conciso."},
        {"role": "user", "content": "Quiero aprender sobre funciones en Python."},
        {"role": "assistant", "content": "Las funciones se definen con 'def'. Ejemplo: def saludar(): print('Hola')"},
        {"role": "user", "content": "¿Cómo paso parámetros?"},
        {"role": "system", "content": "Enfócate en ejemplos prácticos con parámetros."},
        {"role": "user", "content": "Dame un ejemplo con parámetros."}
    ]

    try:
        response2 = llm.call_messages(
            messages=messages_complex,
            provider="gemini",
            api_key=api_key,
            model="gemini-2.5-flash",
            temperature=0.5,
            max_tokens=300
        )
        print(f"✅ Respuesta compleja: {response2}")
        print()
    except Exception as e:
        print(f"❌ Error en test complejo: {e}")
        return False

    # Test 3: Comparar formato legacy vs nuevo
    print("📋 Test 3: Comparing legacy tuple vs new dictionary format")

    # Formato legacy (tuplas)
    conversation_legacy = [
        ("system", "Responde muy brevemente."),
        ("user", "¿Cuál es la capital de España?")
    ]

    # Formato nuevo (diccionarios)
    messages_new = [
        {"role": "system", "content": "Responde muy brevemente."},
        {"role": "user", "content": "¿Cuál es la capital de Francia?"}
    ]

    try:
        # Test formato legacy
        response_legacy = llm.call_conversation(
            conversation=conversation_legacy,
            provider="gemini",
            api_key=api_key,
            model="gemini-2.5-flash",
            max_tokens=50
        )
        print(f"✅ Legacy format (tuplas): {response_legacy}")

        # Test formato nuevo
        response_new = llm.call_messages(
            messages=messages_new,
            provider="gemini",
            api_key=api_key,
            model="gemini-2.5-flash",
            max_tokens=50
        )
        print(f"✅ New format (dicts): {response_new}")
        print()
    except Exception as e:
        print(f"❌ Error en comparación de formatos: {e}")
        return False

    # Test 4: Validación de roles
    print("📋 Test 4: Role validation")

    # Test con rol inválido
    invalid_messages = [
        {
            "role": "invalid_role", 
            "content": "Esto debería fallar"
        },
        {
            "role": "user", 
            "content": "Hola"
        }
    ]

    try:
        response_invalid = llm.call_messages(
            messages=invalid_messages,
            provider="gemini",
            api_key=api_key,
            model="gemini-2.5-flash"
        )
        print(f"✅ Respuesta inválida: {response_invalid}")
        print("❌ ERROR: Debería haber rechazado el rol inválido!")
        return False
    except Exception as e:
        print(f"✅ Correctamente rechazó rol inválido: {e}")
        print()

    # Test 5: System instruction effectiveness demonstration
    print("📋 Test 5: System instruction effectiveness demonstration")
    messages_extended = [
        {
            "role": "system",
            "content": "Eres un asistente útil. Responde brevemente."
        },
        {
            "role": "user",
            "content": "¿Qué es Python?"
        }
    ]

    try:
        response_extended = llm.call_messages(
            messages=messages_extended,
            provider="gemini",
            api_key=api_key,
            model="gemini-2.5-flash",
            temperature=0.8,
            max_tokens=200
        )
        print(f"✅ Conversación extendida: {response_extended}")
        print()
    except Exception as e:
        print(f"❌ Error en conversación extendida: {e}")
        return False

    return True

if __name__ == "__main__":
    print("🐝 Testing Colmena with Real Gemini API\n")

    success = test_with_real_gemini_api()

    if success:
        print("\n🎉 ¡Todos los tests pasaron exitosamente!")
        print("\n✨ Características Demostradas:")
        print("   ✅ Formato de diccionarios funcionando con API real")
        print("   ✅ Múltiples roles (system, user, assistant)")
        print("   ✅ Cambios de contexto dinámicos")
        print("   ✅ Validación de roles")
        print("   ✅ Compatibilidad con formato legacy")

        print("\n🔧 Uso de la nueva interfaz:")
        print("   messages = [{'role': 'user', 'content': 'Hola'}]")
        print("   response = llm.call_messages(messages, 'gemini', api_key='...')")

        print("\n🚀 Próximos pasos:")
        print("   🛠️  Function calling")
        print("   ⏱️  Metadata y timestamps")
        print("   🔧 Configuraciones avanzadas")
        print("   📊 Métricas y monitoreo")
    else:
        print("\n❌ Algunos tests fallaron!")