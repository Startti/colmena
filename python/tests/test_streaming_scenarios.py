#!/usr/bin/env python3
"""
Tests for streaming scenarios to ensure the system correctly handles streaming responses.
"""
import os
import asyncio
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
GEMINI_PROVIDER = "gemini"
GEMINI_MODEL = "gemini-1.5-flash"
OPENAI_PROVIDER = "openai"
OPENAI_MODEL = "gpt-4o-mini"


# --- Streaming Scenarios ---

async def test_valid_streaming_conversation_succeeds():
    """Test a valid streaming conversation succeeds and prints chunks."""
    llm = colmena.ColmenaLlm()
    messages = [
        {"role": "system", "content": "You are a helpful assistant."},
        {"role": "user", "content": "Why is the sky blue?"},
    ]
    try:
        print("📄 Streaming response:")
        full_response = ""
        stream = await llm.stream(messages=messages, provider=GEMINI_PROVIDER, model=GEMINI_MODEL, max_tokens=1000)
        async for chunk in stream:
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

async def test_consecutive_user_messages_streaming_fails():
    """Test that consecutive user messages fail validation in streaming mode."""
    llm = colmena.ColmenaLlm()
    messages = [
        {"role": "user", "content": "Explain what a mathematical function is"},
        {"role": "user", "content": "Give me a simple example"}
    ]
    try:
        # The generator must be consumed for the error to be triggered.
        stream = await llm.stream(messages=messages, provider=GEMINI_PROVIDER, model=GEMINI_MODEL, max_tokens=100)
        async for _ in stream:
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

async def test_openai_streaming_succeeds():
    """Test a valid streaming conversation with OpenAI succeeds."""
    if not os.getenv("OPENAI_API_KEY"):
        print("\n⚠️ SKIPPED: OPENAI_API_KEY not set. Skipping OpenAI test.")
        return True  # Skip test if key is not set

    llm = colmena.ColmenaLlm()
    messages = [
        {"role": "system", "content": "You are a helpful assistant that replies in Spanish."},
        {"role": "user", "content": "Escribe un poema corto sobre la luna."},
    ]
    try:
        print("📄 Streaming response (OpenAI):")
        full_response = ""
        stream = await llm.stream(messages=messages, provider=OPENAI_PROVIDER, model=OPENAI_MODEL, max_tokens=1500)
        async for chunk in stream:
            print(f"  -> Received chunk: '{chunk}'")
            full_response += chunk

        if full_response:
            assert full_response.strip(), "Streamed response content should not be empty."
            print(f"\n📝 Full response (OpenAI): '{full_response}'")
            print("✅ PASSED: OpenAI streaming conversation succeeded.")
            return True
        else:
            print("\n❌ FAILED: OpenAI streaming response was empty.")
            return False
    except Exception as e:
        print(f"❌ FAILED: OpenAI streaming conversation failed unexpectedly with: {e}")
        return False


async def main():
    # Check for API key to provide a better error message
    if not os.getenv("GEMINI_API_KEY"):
        print("\n‼️  WARNING: GEMINI_API_KEY environment variable not set.")
        print("    The Gemini tests will likely fail. Please create a .env file with your key.")
        print("-" * 60)
    
    if not os.getenv("OPENAI_API_KEY"):
        print("\n‼️  WARNING: OPENAI_API_KEY environment variable not set.")
        print("    The OpenAI tests will be skipped. Please create a .env file with your key.")
        print("-" * 60)

    print("🧪 Streaming Scenarios Testing")
    print("="*60)

    tests = [
        #test_valid_streaming_conversation_succeeds,
        #test_consecutive_user_messages_streaming_fails,
        test_openai_streaming_succeeds,
    ]

    passed = 0
    total = len(tests)

    for test_func in tests:
        print(f"\n📋 Running {test_func.__name__}")
        try:
            if await test_func():
                passed += 1
        except Exception as e:
            print(f"❌ Test {test_func.__name__} crashed: {e}")

    print(f"\n🎯 Results: {passed}/{total} tests passed")
    print("="*60)

    if passed != total:
        exit(1)

if __name__ == "__main__":
    asyncio.run(main())
