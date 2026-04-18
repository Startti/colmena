#!/usr/bin/env python3
"""
Tests for streaming scenarios to ensure the system correctly handles streaming responses.
"""
import os

import pytest
from dotenv import load_dotenv

load_dotenv()

try:
    import colmena
except ImportError:
    colmena = None

# --- Test Configuration ---
GEMINI_PROVIDER = "gemini"
GEMINI_MODEL = "gemini-2.5-flash"
OPENAI_PROVIDER = "openai"
OPENAI_MODEL = "gpt-4o-mini"
ANTHROPIC_PROVIDER = "anthropic"
ANTHROPIC_MODEL = "claude-3-haiku-20240307"


@pytest.mark.skipif(
    not os.getenv("GEMINI_API_KEY"),
    reason="GEMINI_API_KEY not set",
)
async def test_valid_streaming_conversation_succeeds():
    """Test a valid streaming conversation succeeds and produces chunks."""
    llm = colmena.ColmenaLlm()
    messages = [
        {"role": "system", "content": "You are a helpful assistant."},
        {"role": "user", "content": "Why is the sky blue?"},
    ]
    full_response = ""
    options = colmena.LlmConfigOptions()
    options.model = GEMINI_MODEL
    options.max_tokens = 1000
    stream = await llm.stream(messages=messages, provider=GEMINI_PROVIDER, options=options)
    async for chunk in stream:
        full_response += chunk

    assert full_response.strip(), "Streaming response should not be empty"


async def test_consecutive_user_messages_streaming_fails():
    """Test that consecutive user messages fail validation in streaming mode."""
    llm = colmena.ColmenaLlm()
    messages = [
        {"role": "user", "content": "Explain what a mathematical function is"},
        {"role": "user", "content": "Give me a simple example"},
    ]
    options = colmena.LlmConfigOptions()
    options.model = GEMINI_MODEL
    options.max_tokens = 100

    with pytest.raises(colmena.LlmException, match="Consecutive messages"):
        stream = await llm.stream(messages=messages, provider=GEMINI_PROVIDER, options=options)
        async for _ in stream:
            pass


@pytest.mark.skipif(
    not os.getenv("OPENAI_API_KEY"),
    reason="OPENAI_API_KEY not set",
)
async def test_openai_streaming_succeeds():
    """Test a valid streaming conversation with OpenAI succeeds."""
    llm = colmena.ColmenaLlm()
    messages = [
        {"role": "system", "content": "You are a helpful assistant that replies in Spanish."},
        {"role": "user", "content": "Escribe un poema corto sobre la luna."},
    ]
    full_response = ""
    options = colmena.LlmConfigOptions()
    options.model = OPENAI_MODEL
    options.max_tokens = 1500
    stream = await llm.stream(messages=messages, provider=OPENAI_PROVIDER, options=options)
    async for chunk in stream:
        full_response += chunk

    assert full_response.strip(), "OpenAI streaming response should not be empty"


@pytest.mark.skipif(
    not os.getenv("ANTHROPIC_API_KEY"),
    reason="ANTHROPIC_API_KEY not set",
)
async def test_anthropic_streaming_succeeds():
    """Test a valid streaming conversation with Anthropic succeeds."""
    llm = colmena.ColmenaLlm()
    messages = [
        {"role": "user", "content": "Write a short haiku about a running stream."},
    ]
    full_response = ""
    options = colmena.LlmConfigOptions()
    options.model = ANTHROPIC_MODEL
    options.max_tokens = 100
    stream = await llm.stream(messages=messages, provider=ANTHROPIC_PROVIDER, options=options)
    async for chunk in stream:
        full_response += chunk

    assert full_response.strip(), "Anthropic streaming response should not be empty"
