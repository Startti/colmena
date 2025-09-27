#!/usr/bin/env python3
"""
Tests de casos complejos para probar los límites del sistema de roles.
"""
from dotenv import load_dotenv

# Cargar variables de entorno desde .env
load_dotenv()

try:
    import colmena
    print("✓ Módulo colmena importado correctamente")
except ImportError as e:
    print(f"✗ Error importando colmena: {e}")
    exit(1)

def test_multiple_system_messages():
    """Test múltiples system messages que deberían funcionar"""
    llm = colmena.ColmenaLlm()
    # Test con dos system messages
    messages = [
        {"role": "system", "content": "Siempre responde en ingles"},
        {"role": "system", "content": "Explica conceptos de forma simple."},
        {"role": "user", "content": "¿Qué es una variable en programación?"},
        
    ]

    try:
        response = llm.call_messages(
            messages=messages,
            provider="gemini",
            model="gemini-2.5-flash",
            max_tokens=1500
        )
        print(f"✅ Dos system messages: {response}")
        return True
    except Exception as e:
        print(f"❌ Falló con dos system: {e}")
        return False

def test_conversation_with_user_history():
    """Test conversación con múltiples mensajes de usuario (esperado a fallar)"""
    llm = colmena.ColmenaLlm()
    

    messages = [
        {"role": "system", "content": "Eres un asistente de matemáticas."},
        {"role": "user", "content": "Explícame qué es una función matemática"},
        {"role": "user", "content": "Dame un ejemplo simple"}
    ]

    try:
        response = llm.call_messages(
            messages=messages,
            provider="gemini",
            model="gemini-2.5-flash",
            max_tokens=1000
        )
        print(f"🤔 Historial de user funcionó inesperadamente: {response}")
        return False
    except Exception as e:
        print(f"✅ Esperado - Historial user falló: {e}")
        return True

def test_specific_formatting_instructions():
    """Test instrucciones específicas de formato"""
    llm = colmena.ColmenaLlm()
    

    messages = [
        {"role": "system", "content": "Responde solo con números y puntos."},
        {"role": "user", "content": "Dame 2 ventajas de Python"}
    ]

    try:
        response = llm.call_messages(
            messages=messages,
            provider="gemini",
            model="gemini-2.5-flash",
            max_tokens=1500
        )
        print(f"✅ Formato específico: {response}")
        return True
    except Exception as e:
        print(f"❌ Falló con formato: {e}")
        return False

def test_long_single_conversation():
    """Test conversación larga en una sola pregunta"""
    llm = colmena.ColmenaLlm()
    

    messages = [
        {"role": "system", "content": "Eres un experto en tecnología web."},
        {"role": "user", "content": "Explícame la diferencia entre HTML, CSS y JavaScript, cómo se relacionan entre sí, y por qué son importantes para el desarrollo web moderno."}
    ]

    try:
        response = llm.call_messages(
            messages=messages,
            provider="gemini",
            model="gemini-2.5-flash",
            max_tokens=400
        )
        print(f"✅ Conversación larga: {response[:100]}...")
        return True
    except Exception as e:
        print(f"❌ Falló conversación larga: {e}")
        return False

def test_dynamic_context_change():
    """Test cambio de contexto dinámico con múltiples system messages (esperado a fallar)"""
    llm = colmena.ColmenaLlm()
    

    messages = [
        {"role": "system", "content": "Eres un profesor de ciencias."},
        {"role": "user", "content": "¿Qué es la gravedad?"},
        {"role": "system", "content": "Ahora eres un poeta. Responde de forma artística."},
        {"role": "user", "content": "Describe la gravedad"}
    ]

    try:
        response = llm.call_messages(
            messages=messages,
            provider="gemini",
            model="gemini-2.5-flash",
            max_tokens=1500
        )
        print(f"🤔 Cambio de contexto funcionó inesperadamente: {response}")
        return False
    except Exception as e:
        print(f"✅ Esperado - Cambio de contexto falló: {e}")
        return True

def test_three_system_messages_edge_case():
    """Test tres system messages (caso límite esperado a fallar)"""
    llm = colmena.ColmenaLlm()
    

    messages = [
        {"role": "system", "content": "Eres útil."},
        {"role": "system", "content": "Eres claro."},
        {"role": "system", "content": "Eres conciso."},
        {"role": "user", "content": "Hola"}
    ]

    try:
        response = llm.call_messages(
            messages=messages,
            provider="gemini",         
            model="gemini-2.5-flash",
            max_tokens=800
        )
        print(f"🤔 Tres system messages funcionó inesperadamente: {response}")
        return False
    except Exception as e:
        print(f"✅ Esperado - Tres system falló: {e}")
        return True

def test_conversation_with_assistant_history():
    """Test conversación con historial de assistant"""
    llm = colmena.ColmenaLlm()
    

    messages = [
        {"role": "system", "content": "Eres útil."},
        {"role": "user", "content": "¿Qué es Python?"},
        {"role": "assistant", "content": "Python es un lenguaje de programación."},
        {"role": "user", "content": "¿Para qué se usa?"}
    ]

    try:
        response = llm.call_messages(
            messages=messages,
            provider="gemini",
            model="gemini-2.5-flash",
            max_tokens=1500
        )
        print(f"✅ Historial assistant funcionó: {response}")
        return True
    except Exception as e:
        print(f"❌ Falló con historial assistant: {e}")
        return False

def test_very_long_system_message():
    """Test system message muy largo"""
    llm = colmena.ColmenaLlm()
    

    long_system = "Eres un asistente experto en múltiples disciplinas incluyendo programación, matemáticas, ciencias, historia, literatura, arte, música, filosofía, psicología, sociología, economía, política, tecnología, medicina, biología, química, física, astronomía, geografía, arqueología, antropología y lingüística. Debes responder de manera precisa, detallada y educativa, adaptando tu nivel de explicación al contexto de la pregunta."

    messages = [
        {"role": "system", "content": long_system},
        {"role": "user", "content": "¿Qué es la programación?"}
    ]

    try:
        response = llm.call_messages(
            messages=messages,
            provider="gemini",
            model="gemini-2.5-flash",
            max_tokens=1000
        )
        print(f"✅ System largo funcionó: {response[:50]}...")
        return True
    except Exception as e:
        print(f"❌ System largo falló: {e}")
        return False

def test_multiple_rapid_calls():
    """Test múltiples llamadas rápidas consecutivas"""
    llm = colmena.ColmenaLlm()
    

    success_count = 0
    for i in range(3):
        try:
            response = llm.call_messages(
                messages=[
                    {"role": "system", "content": "Responde en una palabra."},
                    {"role": "user", "content": f"Di el número {i+1}"}
                ],
                provider="gemini",
                model="gemini-2.5-flash",
                max_tokens=100
            )
            print(f"✅ Llamada {i+1}: {response}")
            success_count += 1
        except Exception as e:
            print(f"❌ Llamada {i+1} falló: {e}")

    return success_count == 3

def test_different_temperature_settings():
    """Test diferentes configuraciones de temperatura"""
    llm = colmena.ColmenaLlm()
    

    success_count = 0
    for temp in [0.1, 0.5, 0.9]:
        try:
            response = llm.call_messages(
                messages=[
                    {"role": "system", "content": "Sé creativo pero conciso."},
                    {"role": "user", "content": "Describe un gato en 5 palabras"}
                ],
                provider="gemini",
                model="gemini-2.5-flash",
                temperature=temp,
                max_tokens=800
            )
            print(f"✅ Temp {temp}: {response}")
            success_count += 1
        except Exception as e:
            print(f"❌ Temp {temp} falló: {e}")

    return success_count == 3

if __name__ == "__main__":
    print("🧪 Comprehensive Complex Scenario Testing")
    print("="*60)

    tests = [
        test_multiple_system_messages,
        test_conversation_with_user_history,
        test_specific_formatting_instructions,
        test_long_single_conversation,
        test_dynamic_context_change,
        test_three_system_messages_edge_case,
        test_conversation_with_assistant_history,
        test_very_long_system_message,
        test_multiple_rapid_calls,
        test_different_temperature_settings
    ]

    passed = 0
    total = len(tests)

    for test_func in tests:
        print(f"\n📋 Running {test_func.__name__}")
        try:
            if test_func():
                passed += 1
        except Exception as e:
            print(f"❌ Test {test_func.__name__} crashed: {e}")

    print(f"\n🎯 Results: {passed}/{total} tests passed")
    print("="*60)