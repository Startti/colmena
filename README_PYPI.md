# 🐝 Colmena AI - Multi-Provider LLM Orchestration Library

A **high-performance** Rust library for AI agent orchestration with native Python bindings. Colmena provides a unified interface for multiple LLM providers with both synchronous and streaming support.

## ✨ Features

- **🔌 Multi-Provider Support**: Native support for OpenAI, Google Gemini, and Anthropic Claude
- **⚡ Streaming Responses**: Real-time text generation with chunk-by-chunk delivery
- **🦀 Rust Performance**: Native Rust implementation compiled with PyO3 (zero Python overhead)
- **🏗️ Clean Architecture**: Hexagonal architecture for maximum extensibility
- **🔧 Flexible Configuration**: API keys from environment variables or direct values
- **🛡️ Robust Error Handling**: Type-safe error management and recovery
- **🔒 Type Safety**: Compile-time guarantees from Rust's type system

## 🚀 Quick Start

### Installation

```bash
pip install colmena-ai
```

### Basic Usage

```python
from colmena import ColmenaLlm

# Initialize the library
llm = ColmenaLlm()

# Simple synchronous call
response = llm.call(
    messages=[
        {"role": "user", "content": "What is the capital of France?"}
    ],
    provider="openai",
    model="gpt-4o",
    temperature=0.7
)

print(response)
# Output: "The capital of France is Paris."
```

### Streaming Responses

```python
from colmena import ColmenaLlm

llm = ColmenaLlm()

# Stream responses in real-time
for chunk in llm.stream(
    messages=["Tell me a story about AI"],
    provider="anthropic",
    model="claude-3-sonnet-20240229"
):
    print(chunk, end="", flush=True)
```

### Multiple Providers

```python
from colmena import ColmenaLlm

llm = ColmenaLlm()

# OpenAI
openai_response = llm.call(
    messages=[{"role": "user", "content": "Hello!"}],
    provider="openai",
    model="gpt-4o"
)

# Google Gemini
gemini_response = llm.call(
    messages=[{"role": "user", "content": "Hello!"}],
    provider="gemini",
    model="gemini-pro"
)

# Anthropic Claude
claude_response = llm.call(
    messages=[{"role": "user", "content": "Hello!"}],
    provider="anthropic",
    model="claude-3-sonnet-20240229"
)
```

## 🔑 Configuration

### Environment Variables

Set API keys as environment variables:

```bash
export OPENAI_API_KEY="sk-..."
export GEMINI_API_KEY="AIza..."
export ANTHROPIC_API_KEY="sk-ant-..."
```

### Direct API Keys

Or pass them directly:

```python
llm.call(
    messages=[{"role": "user", "content": "Hello"}],
    provider="openai",
    api_key="sk-...",  # Direct API key
    model="gpt-4o"
)
```

## 📦 Supported Models

### OpenAI
- `gpt-4o` (default)
- `gpt-4-turbo`
- `gpt-3.5-turbo`

### Google Gemini
- `gemini-pro` (default)
- `gemini-2.0-flash-exp`
- `gemini-1.5-pro`

### Anthropic Claude
- `claude-3-sonnet-20240229` (default)
- `claude-3-opus-20240229`
- `claude-3-haiku-20240307`

## 🎯 Advanced Configuration

```python
from colmena import ColmenaLlm

llm = ColmenaLlm()

response = llm.call(
    messages=[
        {"role": "system", "content": "You are a helpful assistant."},
        {"role": "user", "content": "Explain quantum computing"}
    ],
    provider="openai",
    model="gpt-4o",
    temperature=0.7,        # Creativity (0.0 - 2.0)
    max_tokens=500,         # Maximum response length
    top_p=0.9,              # Nucleus sampling
    frequency_penalty=0.5,  # Reduce repetition
    presence_penalty=0.5    # Encourage new topics
)
```

## 🏗️ Architecture

Colmena is built using **Hexagonal Architecture** (Ports and Adapters):

- **Domain Layer**: Pure business logic and interfaces
- **Application Layer**: Use cases and orchestration
- **Infrastructure Layer**: Provider adapters (OpenAI, Gemini, Anthropic)

This design ensures:
- Easy to add new LLM providers
- Testable and maintainable code
- Clear separation of concerns

## 🔍 Error Handling

```python
from colmena import ColmenaLlm

llm = ColmenaLlm()

try:
    response = llm.call(
        messages=[{"role": "user", "content": "Hello"}],
        provider="openai"
    )
except Exception as e:
    print(f"Error: {e}")
    # Handle error appropriately
```

## 🧪 Health Checks

```python
from colmena import ColmenaLlm

llm = ColmenaLlm()

# Check if a provider is available
is_healthy = llm.health_check("openai")
print(f"OpenAI is {'available' if is_healthy else 'unavailable'}")
```

## 🌟 Why Colmena?

1. **Performance**: Native Rust implementation, no Python overhead
2. **Unified API**: One interface for all LLM providers
3. **Type Safety**: Compile-time guarantees from Rust
4. **Extensible**: Easy to add new providers following hexagonal architecture
5. **Production Ready**: Robust error handling and testing

## 📚 Documentation

- [GitHub Repository](https://github.com/Startti/colmena)
- [Developer Guide](https://github.com/Startti/colmena/tree/main/docs/developer_guide)
- [Architecture Details](https://github.com/Startti/colmena/blob/main/docs/dds/ARQUITECTURA_HEXAGONAL_GUIA.md)

## 🤝 Contributing

Contributions are welcome! Please see our [Contributing Guide](https://github.com/Startti/colmena/blob/main/docs/developer_guide/08_contributing.md).

## 📄 License

MIT License - see [LICENSE](https://github.com/Startti/colmena/blob/main/LICENSE) for details.

## 🔗 Links

- **Repository**: https://github.com/Startti/colmena
- **Issues**: https://github.com/Startti/colmena/issues
- **PyPI**: https://pypi.org/project/colmena-ai/

---

Built with ❤️ using Rust 🦀 and PyO3
