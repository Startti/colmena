# Python Examples for Colmena DAG Engine

This directory contains examples demonstrating how to use the Colmena DAG Engine from Python.

## Prerequisites

Install the package with Python bindings:

```bash
python3 -m maturin develop --features python
```

Or activate the virtual environment:

```bash
source .venv/bin/activate  # On macOS/Linux
# or
.venv\Scripts\activate  # On Windows
```

## Examples

### 1. Running a DAG (`example_run_dag.py`)

Execute a DAG file and get results:

```bash
python python/tests/example_run_dag.py
```

This demonstrates:
- Loading and executing a DAG from JSON
- Parsing execution results
- Error handling with `DagException`

### 2. Serving a DAG as HTTP API (`example_serve_dag.py`)

Start an HTTP server with webhook endpoints:

```bash
python python/tests/example_serve_dag.py
```

Then test with curl:

```bash
curl -X POST http://localhost:3000/hello \
  -H "Content-Type: application/json" \
  -d '{"name": "World"}'
```

This demonstrates:
- Starting a DAG server
- Exposing webhook endpoints
- Handling HTTP requests through DAG execution

### 3. LLM with Tool Calling (`example_llm_dag.py`)

Run a DAG with LLM nodes and tool calling:

```bash
# Set your OpenAI API key
export OPENAI_API_KEY="your-key-here"

python python/tests/example_llm_dag.py
```

This demonstrates:
- LLM integration
- Tool calling capabilities
- Environment variable configuration

## API Reference

### `colmena.run_dag(file_path: str) -> str`

Execute a DAG from a JSON file.

**Parameters:**
- `file_path`: Path to the DAG JSON file (relative or absolute)

**Returns:**
- JSON string with execution results

**Raises:**
- `colmena.DagException`: If DAG execution fails

### `colmena.serve_dag(file_path: str, port: int = 3000) -> None`

Start an HTTP server to serve a DAG.

**Parameters:**
- `file_path`: Path to the DAG JSON file
- `port`: Port number to listen on (default: 3000)

**Raises:**
- `colmena.DagException`: If server fails to start

**Note:** This is a blocking call. The server runs until interrupted (Ctrl+C).

## Creating Your Own DAG

See the `tests/` directory in the project root for example DAG files:
- `tests/power.json` - Simple math operations
- `tests/basic_webhook.json` - Webhook trigger example
- `tests/agent_with_tools.json` - LLM with tool calling
