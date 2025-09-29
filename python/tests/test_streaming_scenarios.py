#!/usr/bin/env python3
"""
Tests for streaming scenarios to ensure the system correctly handles streaming responses.
"""
import os
from dotenv import load_dotenv

# Load environment variables from .env
load_dotenv()

try:
    import colmena
    print("✓ colmena module imported successfully")
except ImportError as e:
    print(f"✗ Error importing colmena: {e}")
    exit(1)

# --- Test Configuration ---
# Use a fast and cheap model for testing.
TEST_PROVIDER = "gemini"
TEST_MODEL = "gemini-2.5-flash"

# --- Streaming Scenarios ---

def test_valid_streaming_conversation_succeeds():
    """Test a valid streaming conversation succeeds and prints chunks."""
    llm = colmena.ColmenaLlm()
    messages = [
        {"role": "system", "content": "You are a helpful assistant."},
        {"role": "user", "content": "Explain what a mathematical function is"},
    ]
    try:
        print("📄 Streaming response:")
        full_response = ""
        # We need to consume the generator to check for errors
        for chunk in llm.stream(messages=messages, provider=TEST_PROVIDER, model=TEST_MODEL, max_tokens=400):
            print(f"  -> Received chunk: '{chunk}'")
            full_response += chunk
        
        if full_response:
            print(f"\n📝 Full response: '{full_response}'")
            print("✅ PASSED: Valid streaming conversation succeeded.")
            return True
        else:
            print("\n❌ FAILED: Streaming response was empty.")
            return False
    except Exception as e:
        print(f"❌ FAILED: Valid streaming conversation failed unexpectedly with: {e}")
        return False

def test_consecutive_user_messages_streaming_fails():
    """Test that consecutive user messages fail validation in streaming mode."""
    llm = colmena.ColmenaLlm()
    messages = [
        {"role": "user", "content": "Explain what a mathematical function is"},
        {"role": "user", "content": "Give me a simple example"}
    ]
    try:
        # The generator must be consumed for the error to be triggered.
        stream = llm.stream(messages=messages, provider=TEST_PROVIDER, model=TEST_MODEL, max_tokens=400)
        for _ in stream:
            pass  # Consume the generator
        print("❌ FAILED: Consecutive user messages in streaming did not raise an exception.")
        return False
    except colmena.LlmException as e:
        assert "Consecutive messages" in str(e)
        print(f"✅ PASSED: Consecutive user messages in streaming correctly failed with: {e}")
        return True
    except Exception as e:
        print(f"❌ FAILED: An unexpected error occurred: {e}")
        return False

if __name__ == "__main__":
    # Check for API key to provide a better error message
    if not os.getenv("GEMINI_API_KEY"):
        print("\n‼️  WARNING: GEMINI_API_KEY environment variable not set.")
        print("    The tests will likely fail. Please create a .env file with your key.")
        print("-" * 60)

    print("🧪 Streaming Scenarios Testing")
    print("="*60)

    tests = [
        test_valid_streaming_conversation_succeeds,
        test_consecutive_user_messages_streaming_fails,
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