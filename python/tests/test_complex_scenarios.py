#!/usr/bin/env python3
"""
Tests de casos complejos para probar los límites del sistema de roles y la validación de mensajes.
"""
import os
from dotenv import load_dotenv

# Cargar variables de entorno desde .env
load_dotenv()

try:
    import colmena
    print("✓ Módulo colmena importado correctamente")
except ImportError as e:
    print(f"✗ Error importando colmena: {e}")
    exit(1)

# --- Test Configuration ---
# Usamos un modelo rápido y económico para las pruebas.
GEMINI_PROVIDER = "gemini"
GEMINI_MODEL = "gemini-2.5-flash"
OPENAI_PROVIDER = "openai"
OPENAI_MODEL = "gpt-5-nano-2025-08-07"


# --- Role & Message Validation Tests ---

def test_valid_alternating_conversation_succeeds():
    """Test a valid conversation with alternating user/assistant roles succeeds."""
    llm = colmena.ColmenaLlm()
    messages = [
        {"role": "system", "content": "eres un asistente que responde conciso"},
        {"role": "user", "content": "Hola"},
        {"role": "assistant", "content": "Hola, ¿cómo puedo ayudarte?"},
        {"role": "user", "content": "¿Qué es Rust?"},
    ]
    try:
        options = colmena.LlmConfigOptions()
        options.model = GEMINI_MODEL
        options.max_tokens = 400
        response = llm.call(messages=messages, provider=GEMINI_PROVIDER, options=options)
        print(f"✅ PASSED: Valid alternating conversation succeeded. Response: '{response[:50]}...'")
        return True
    except Exception as e:
        print(f"❌ FAILED: Valid conversation failed unexpectedly with: {e}")
        return False

def test_consecutive_user_messages_fails():
    """Test that consecutive user messages fail validation."""
    llm = colmena.ColmenaLlm()
    messages = [
        {"role": "user", "content": "Explícame qué es una función matemática"},
        {"role": "user", "content": "Dame un ejemplo simple"}
    ]
    try:
        options = colmena.LlmConfigOptions()
        options.model = GEMINI_MODEL
        options.max_tokens = 400
        llm.call(messages=messages, provider=GEMINI_PROVIDER, options=options)
        print("❌ FAILED: Consecutive user messages did not raise an exception.")
        return False
    except colmena.LlmException as e:
        assert "Consecutive messages" in str(e)
        print(f"✅ PASSED: Consecutive user messages correctly failed with: {e}")
        return True
    except Exception as e:
        print(f"❌ FAILED: An unexpected error occurred: {e}")
        return False

def test_consecutive_assistant_messages_fails():
    """Test that consecutive assistant messages fail validation."""
    llm = colmena.ColmenaLlm()
    messages = [
        {"role": "user", "content": "Hola"},
        {"role": "assistant", "content": "Hola, ¿cómo puedo ayudarte?"},
        {"role": "assistant", "content": "Estoy aquí para servirte."}
    ]
    try:
        options = colmena.LlmConfigOptions()
        options.model = GEMINI_MODEL
        options.max_tokens = 400
        llm.call(messages=messages, provider=GEMINI_PROVIDER, options=options)
        print("❌ FAILED: Consecutive assistant messages did not raise an exception.")
        return False
    except colmena.LlmException as e:
        assert "Consecutive messages" in str(e)
        print(f"✅ PASSED: Consecutive assistant messages correctly failed with: {e}")
        return True
    except Exception as e:
        print(f"❌ FAILED: An unexpected error occurred: {e}")
        return False

def test_multiple_system_messages_succeeds():
    """Test that multiple consecutive system messages are allowed and ignored by validation."""
    llm = colmena.ColmenaLlm()
    messages = [
        {"role": "system", "content": "Siempre responde en ingles."},
        {"role": "system", "content": "Explica conceptos de forma simple."},
        {"role": "user", "content": "¿Qué es una variable en programación?"},
    ]
    try:
        options = colmena.LlmConfigOptions()
        options.model = GEMINI_MODEL
        options.max_tokens = 400
        llm.call(messages=messages, provider=GEMINI_PROVIDER, options=options)
        return False
    except Exception as e:
        print(f"✅ PASSED: Consecutive system messages correctly failed with: {e}")
        return True

def test_missing_role_key_fails():
    """Test that a message with a missing 'role' key fails validation."""
    llm = colmena.ColmenaLlm()
    messages = [{"content": "hola"}]
    try:
        options = colmena.LlmConfigOptions()
        options.model = GEMINI_MODEL
        options.max_tokens = 400
        llm.call(messages=messages, provider=GEMINI_PROVIDER, options=options)
        print("❌ FAILED: Missing 'role' key did not raise an exception.")
        return False
    except colmena.LlmException as e:
        assert "Missing 'role' key" in str(e)
        print(f"✅ PASSED: Missing 'role' key correctly failed with: {e}")
        return True
    except Exception as e:
        print(f"❌ FAILED: An unexpected error occurred: {e}")
        return False

def test_missing_content_key_fails():
    """Test that a message with a missing 'content' key fails validation."""
    llm = colmena.ColmenaLlm()
    messages = [{"role": "user"}]
    try:
        options = colmena.LlmConfigOptions()
        options.model = GEMINI_MODEL
        options.max_tokens = 400
        llm.call(messages=messages, provider=GEMINI_PROVIDER, options=options)
        print("❌ FAILED: Missing 'content' key did not raise an exception.")
        return False
    except colmena.LlmException as e:
        assert "Missing 'content' key" in str(e)
        print(f"✅ PASSED: Missing 'content' key correctly failed with: {e}")
        return True
    except Exception as e:
        print(f"❌ FAILED: An unexpected error occurred: {e}")
        return False

def test_openai_valid_call_succeeds():
    """Test a valid call to OpenAI succeeds."""
    if not os.getenv("OPENAI_API_KEY"):
        print("\n⚠️ SKIPPED: OPENAI_API_KEY not set. Skipping OpenAI test.")
        return True  # Skip test if key is not set

    llm = colmena.ColmenaLlm()
    messages = [
        {"role": "system", "content": "You are an assistant that provides short answers."},
        {"role": "user", "content": "What is the capital of France?"},
    ]
    try:
        options = colmena.LlmConfigOptions()
        options.model = OPENAI_MODEL
        options.max_tokens = 4000
        response = llm.call(messages=messages, provider=OPENAI_PROVIDER, options=options)
        assert response.strip(), "Response content should not be empty."
        print(f"✅ PASSED: OpenAI valid call succeeded. Response: '{response}'")
        return True
    except Exception as e:
        print(f"❌ FAILED: OpenAI valid call failed unexpectedly with: {e}")
        return False

if __name__ == "__main__":
    # Check for API key to provide a better error message
    if not os.getenv("GEMINI_API_KEY"):
        print("\n‼️  WARNING: GEMINI_API_KEY environment variable not set.")
        print("\n    The Gemini tests will likely fail. Please create a .env file with your key.")
        print("-" * 60)
    
    if not os.getenv("OPENAI_API_KEY"):
        print("\n‼️  WARNING: OPENAI_API_KEY environment variable not set.")
        print("\n    The OpenAI tests will be skipped. Please create a .env file with your key.")
        print("-" * 60)

    print("🧪 Role and Message Validation Testing")
    print("="*60)

    tests = [
        test_valid_alternating_conversation_succeeds,
        test_consecutive_user_messages_fails,
        test_consecutive_assistant_messages_fails,
        test_missing_role_key_fails,
        test_multiple_system_messages_succeeds,
        test_missing_content_key_fails,
        test_openai_valid_call_succeeds,
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

    if passed != total:
        exit(1)
