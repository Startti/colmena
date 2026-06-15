#!/usr/bin/env python3
"""
Tests for role validation and message validation in complex scenarios.
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
GEMINI_PROVIDER = "google"  # provider id is "google" (not "gemini")
GEMINI_MODEL = "gemini-2.5-flash"
OPENAI_PROVIDER = "openai"
OPENAI_MODEL = "gpt-4o-mini"
ANTHROPIC_PROVIDER = "anthropic"
ANTHROPIC_MODEL = "claude-haiku-4-5-20251001"


# --- Role & Message Validation Tests ---


@pytest.mark.skipif(
    not os.getenv("GEMINI_API_KEY"),
    reason="GEMINI_API_KEY not set",
)
def test_valid_alternating_conversation_succeeds():
    """Test a valid conversation with alternating user/assistant roles succeeds."""
    llm = colmena.ColmenaLlm()
    messages = [
        {"role": "system", "content": "eres un asistente que responde conciso"},
        {"role": "user", "content": "Hola"},
        {"role": "assistant", "content": "Hola, ¿cómo puedo ayudarte?"},
        {"role": "user", "content": "¿Qué es Rust?"},
    ]
    options = colmena.LlmConfigOptions()
    options.model = GEMINI_MODEL
    options.max_tokens = 400
    response = llm.call(messages=messages, provider=GEMINI_PROVIDER, options=options)
    assert response, "Response should not be empty"


def test_consecutive_user_messages_fails():
    """Test that consecutive user messages fail validation."""
    llm = colmena.ColmenaLlm()
    messages = [
        {"role": "user", "content": "Explícame qué es una función matemática"},
        {"role": "user", "content": "Dame un ejemplo simple"},
    ]
    options = colmena.LlmConfigOptions()
    options.model = GEMINI_MODEL
    options.max_tokens = 400

    with pytest.raises(colmena.LlmException, match="Consecutive messages"):
        llm.call(messages=messages, provider=GEMINI_PROVIDER, options=options)


def test_consecutive_assistant_messages_fails():
    """Test that consecutive assistant messages fail validation."""
    llm = colmena.ColmenaLlm()
    messages = [
        {"role": "user", "content": "Hola"},
        {"role": "assistant", "content": "Hola, ¿cómo puedo ayudarte?"},
        {"role": "assistant", "content": "Estoy aquí para servirte."},
    ]
    options = colmena.LlmConfigOptions()
    options.model = GEMINI_MODEL
    options.max_tokens = 400

    with pytest.raises(colmena.LlmException, match="Consecutive messages"):
        llm.call(messages=messages, provider=GEMINI_PROVIDER, options=options)


def test_multiple_system_messages_fails():
    """Test that multiple consecutive system messages fail validation."""
    llm = colmena.ColmenaLlm()
    messages = [
        {"role": "system", "content": "Siempre responde en ingles."},
        {"role": "system", "content": "Explica conceptos de forma simple."},
        {"role": "user", "content": "¿Qué es una variable en programación?"},
    ]
    options = colmena.LlmConfigOptions()
    options.model = GEMINI_MODEL
    options.max_tokens = 400

    with pytest.raises(Exception):
        llm.call(messages=messages, provider=GEMINI_PROVIDER, options=options)


def test_missing_role_key_fails():
    """Test that a message with a missing 'role' key fails validation."""
    llm = colmena.ColmenaLlm()
    messages = [{"content": "hola"}]
    options = colmena.LlmConfigOptions()
    options.model = GEMINI_MODEL
    options.max_tokens = 400

    with pytest.raises(colmena.LlmException, match="Missing 'role' key"):
        llm.call(messages=messages, provider=GEMINI_PROVIDER, options=options)


def test_missing_content_key_fails():
    """Test that a message with a missing 'content' key fails validation."""
    llm = colmena.ColmenaLlm()
    messages = [{"role": "user"}]
    options = colmena.LlmConfigOptions()
    options.model = GEMINI_MODEL
    options.max_tokens = 400

    with pytest.raises(colmena.LlmException, match="Missing 'content' key"):
        llm.call(messages=messages, provider=GEMINI_PROVIDER, options=options)


@pytest.mark.skipif(
    not os.getenv("OPENAI_API_KEY"),
    reason="OPENAI_API_KEY not set",
)
def test_openai_valid_call_succeeds():
    """Test a valid call to OpenAI succeeds."""
    llm = colmena.ColmenaLlm()
    messages = [
        {"role": "system", "content": "You are an assistant that provides short answers."},
        {"role": "user", "content": "What is the capital of France?"},
    ]
    options = colmena.LlmConfigOptions()
    options.model = OPENAI_MODEL
    options.max_tokens = 4000
    response = llm.call(messages=messages, provider=OPENAI_PROVIDER, options=options)
    assert response.strip(), "Response content should not be empty"


@pytest.mark.skipif(
    not os.getenv("ANTHROPIC_API_KEY"),
    reason="ANTHROPIC_API_KEY not set",
)
def test_anthropic_valid_call_succeeds():
    """Test a valid call to Anthropic succeeds."""
    llm = colmena.ColmenaLlm()
    messages = [
        {"role": "user", "content": "What are the primary colors?"},
    ]
    options = colmena.LlmConfigOptions()
    options.model = ANTHROPIC_MODEL
    options.max_tokens = 100
    response = llm.call(messages=messages, provider=ANTHROPIC_PROVIDER, options=options)
    assert response.strip(), "Response content should not be empty"
