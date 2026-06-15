# 🐝 Colmena AI - Multi-Provider LLM Orchestration Library

A **high-performance** Rust library for AI agent orchestration with native Python bindings. Colmena
provides a unified interface for multiple LLM providers (synchronous calls + async streaming) **and** a
JSON-defined DAG engine for running multi-step agent workflows.

## ✨ Features

- **🔌 Multi-Provider Support**: Native support for OpenAI, Google Gemini, and Anthropic Claude
- **🧩 DAG Orchestration**: Run multi-node agent graphs (LLM + tools + control flow) from JSON
- **⚡ Async Streaming**: Real-time, chunk-by-chunk generation via `async for`
- **🦀 Rust Performance**: Native Rust implementation compiled with PyO3
- **🏗️ Clean Architecture**: Hexagonal architecture for maximum extensibility
- **🔧 Flexible Configuration**: API keys from environment variables or per-call overrides

## 🚀 Quick Start

### Installation

```bash
pip install colmena-ai
```

The import name is `colmena` (the PyPI package is `colmena-ai`):

```python
import colmena
```

### Basic Usage — a single LLM call

`call()` takes **messages as a list of `{"role", "content"}` dicts**, a `provider` string, and an optional
`LlmConfigOptions` object for model/sampling parameters. It returns the response text as a `str`.

```python
import colmena

llm = colmena.ColmenaLlm()

opts = colmena.LlmConfigOptions()
opts.model = "gemini-2.5-flash"
opts.temperature = 0.7

response = llm.call(
    messages=[{"role": "user", "content": "What is the capital of France?"}],
    provider="google",          # one of: "openai", "google", "anthropic"
    options=opts,
)

print(response)
# "The capital of France is Paris."
```

> **Provider strings:** use `"google"` for Gemini (not `"gemini"`), plus `"openai"` and `"anthropic"`.

### Streaming Responses (async)

`stream()` returns an **async iterator** — consume it with `async for` inside an event loop:

```python
import asyncio
import colmena

async def main():
    llm = colmena.ColmenaLlm()
    # stream() returns an awaitable; await it to get the async iterator.
    stream = await llm.stream(
        messages=[{"role": "user", "content": "Tell me a short story about AI"}],
        provider="anthropic",
    )
    async for chunk in stream:
        print(chunk, end="", flush=True)

asyncio.run(main())
```

### Configuration with `LlmConfigOptions`

All model and sampling parameters live on `LlmConfigOptions` and are passed via `options=`:

```python
import colmena

llm = colmena.ColmenaLlm()

opts = colmena.LlmConfigOptions()
opts.api_key = "sk-..."        # optional per-call override (else taken from env)
opts.model = "gpt-4o"
opts.temperature = 0.7         # creativity (0.0 - 2.0)
opts.max_tokens = 500          # maximum response length
opts.top_p = 0.9               # nucleus sampling
opts.frequency_penalty = 0.5   # reduce repetition
opts.presence_penalty = 0.5    # encourage new topics

response = llm.call(
    messages=[
        {"role": "system", "content": "You are a helpful assistant."},
        {"role": "user", "content": "Explain quantum computing"},
    ],
    provider="openai",
    options=opts,
)
```

Available fields on `LlmConfigOptions`: `api_key`, `model`, `temperature`, `max_tokens`, `top_p`,
`frequency_penalty`, `presence_penalty`. Any field left unset falls back to provider defaults.

## 🧩 DAG Orchestration

Beyond single calls, Colmena runs **agent workflows defined as JSON graphs** — LLM nodes, tool calls,
HTTP requests, control flow, human-in-the-loop, and more. Run a graph from Python with `run_dag()`:

```python
import colmena
import json

# run_dag accepts a file path or an in-memory graph dict; returns a JSON string.
result_json = colmena.run_dag("path/to/graph.json")
result = json.loads(result_json)
print(result)
```

Validate a graph (as an in-memory dict) before running it:

```python
import colmena

graph = {
    "nodes": {
        "start":     {"type": "mock_input", "config": {"input": 5}},
        "pow_step":  {"type": "exponential", "config": {"exponent": 3}},
        "log_result": {"type": "log"},
    },
    "edges": [
        {"from": "start", "to": "pow_step"},
        {"from": "pow_step", "to": "log_result"},
    ],
}

colmena.validate_graph(graph)   # raises colmena.DagException if invalid
```

Serve a graph's webhook triggers as an HTTP API (blocking call):

```python
import colmena

# Exposes the webhook paths declared in the graph (e.g. POST /power).
colmena.serve_dag("path/to/graph_with_webhook.json", host="0.0.0.0", port=8080)
```

Inspect the node registry without a database connection:

```python
import colmena

reg = colmena.default_registry()
print(reg.node_types())   # -> list of registered node type names
```

`run_dag` also accepts `resume_id`, `resume_answer`, `inject_payload`, `include_extra_info`, and
`agent_session_id` for suspend/resume and stateful agent flows.

## 🔑 Configuration

### Environment Variables (recommended)

```bash
export OPENAI_API_KEY="sk-..."
export GEMINI_API_KEY="AIza..."
export ANTHROPIC_API_KEY="sk-ant-..."
```

`ColmenaLlm()` loads these automatically at construction. To override per call, set
`LlmConfigOptions.api_key`.

## 📦 Models

Pass any model id your provider supports via `LlmConfigOptions.model`. Commonly used:

- **OpenAI** (`provider="openai"`): `gpt-4o`, `gpt-4o-mini`
- **Google Gemini** (`provider="google"`): `gemini-2.5-flash`, `gemini-2.5-pro`
- **Anthropic Claude** (`provider="anthropic"`): `claude-3-5-sonnet-20241022`

If `model` is unset, each provider falls back to its own default.

## 🔍 Error Handling

LLM calls raise `colmena.LlmException`; DAG functions raise `colmena.DagException`:

```python
import colmena

llm = colmena.ColmenaLlm()

try:
    response = llm.call(
        messages=[{"role": "user", "content": "Hello"}],
        provider="openai",
    )
except colmena.LlmException as e:
    print(f"LLM error: {e}")
```

## 🧪 Health Checks

```python
import colmena

llm = colmena.ColmenaLlm()

print(llm.get_providers())          # -> list of available provider names
print(llm.health_check("openai"))   # -> bool
```

## 🏗️ Architecture

Colmena is built using **Hexagonal Architecture** (Ports and Adapters):

- **Domain Layer**: Pure business logic and interfaces
- **Application Layer**: Use cases and orchestration
- **Infrastructure Layer**: Provider adapters (OpenAI, Gemini, Anthropic) + DAG nodes

## 📚 Documentation

- [GitHub Repository](https://github.com/Startti/colmena)
- [Developer Guide](https://github.com/Startti/colmena/tree/main/docs/developer_guide)
- [Architecture Details](https://github.com/Startti/colmena/blob/main/docs/dds/ARQUITECTURA_HEXAGONAL_GUIA.md)

## 📄 License

MIT License - see [LICENSE](https://github.com/Startti/colmena/blob/main/LICENSE) for details.

## 🔗 Links

- **Repository**: https://github.com/Startti/colmena
- **Issues**: https://github.com/Startti/colmena/issues
- **PyPI**: https://pypi.org/project/colmena-ai/

---

Built with ❤️ using Rust 🦀 and PyO3
