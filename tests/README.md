# Colmena Tests & Examples

This directory contains integration tests, example DAGs, and standalone Rust examples for the Colmena framework.

## JSON DAG Examples

These JSON files define Directed Acyclic Graphs (DAGs) that can be executed using the `dag_engine` binary. They demonstrate various features of Colmena, including tool calling, memory persistence, and different node types.

### How to Run

Use the `dag_engine` binary to run these examples:

```bash
cargo run --bin dag_engine run tests/<example_file>.json
```

### Available Examples

#### 🐍 Python Node Examples

- **`python_simple_graph.json`**: Demonstrates basic Python code execution with variables injected from inputs. Calculates `x * y + 2` where x=10 and y=5, producing output `52`.
- **`python_llm_graph.json`**: Advanced example showing LLM-to-Python integration. An LLM (GPT-4) generates Python code to calculate factorial(5), which is then executed by the Python Node, producing output `120`.

#### 🤖 Agent & Tool Examples

- **`agent_with_tools.json`**: A basic example of an agent with tools enabled.
- **`agent_with_tools_postgres.json`**: An agent using PostgreSQL for conversation memory.
- **`agent_with_tools_postgres_recall.json`**: Demonstrates an agent recalling information from PostgreSQL memory.
- **`http_tool_configured.json`**: Demonstrates how to configure HTTP nodes as tools for an LLM agent. The LLM can "call" these tools (e.g., `fetch_users`, `create_user`) to interact with an external API.

#### 💾 Memory Examples

- **`memory_postgres_example.json`**: Example focusing on PostgreSQL memory persistence.
- **`memory_sqlite_example.json`**: Example focusing on SQLite memory persistence.

#### 🌐 HTTP & Trigger Examples

- **`dynamic_http.json`**: Demonstrates dynamic HTTP requests.
- **`trigger.json`**: Shows how to use the TriggerWebhookNode.

## 🐍 Python Node

The **Python Node** (`python_script`) allows you to execute arbitrary Python code within a DAG workflow. It seamlessly integrates with JSON inputs/outputs and works especially well with LLM-generated code.

### Features

- **JSON Integration**: Automatically converts inputs to Python variables and outputs back to JSON
- **LLM Compatible**: Strips markdown code blocks from LLM-generated code
- **Function Support**: Define and use functions within the same script
- **Standard Library**: Access to Python's standard library (import allowed)
- **Safe Execution**: Runs in isolated thread with GIL management

### Usage

**Config Schema:**
```json
{
  "type": "python_script",
  "config": {
    "code": "output = some_expression"  // Optional: fallback code
  }
}
```

**Inputs:**
- `code` (string, optional): Python code to execute (overrides config.code)
- Any other inputs are injected as variables into the Python script

**Outputs:**
- `output`: The value assigned to the `output` variable in the script

### Examples

#### Basic Math Operation
```json
{
  "type": "python_script", 
  "config": {
    "code": "output = x * y + 2"
  }
}
```
With inputs `x=10, y=5` → Output: `52`

#### With Functions
```json
{
  "type": "python_script",
  "config": {
    "code": "def factorial(n):\\n    return 1 if n <= 1 else n * factorial(n-1)\\noutput = factorial(5)"
  }
}
```
Output: `120`

#### LLM-Generated Code
The Python Node automatically strips markdown from LLM responses:
```
Input (from LLM): "```python\noutput = 42\n```"
Extracted: "output = 42"
```

### Best Practices

1. **Always assign to `output`**: This is the convention for returning values
2. **Use inputs for dynamic data**: Let the DAG pass data to your script
3. **Keep code simple**: Complex logic might be better as a separate node type
4. **Test with static config first**: Then try dynamic LLM generation

## Rust Integration Tests & Examples

These Rust files contain standalone examples and integration tests.

- **`agent_with_tools.rs`**: A standalone Rust program demonstrating how to programmatically set up an agent with tools.
- **`gemini_tool_test.rs`**: Integration test for Gemini provider tool calling.
- **`openai_tool_test.rs`**: Integration test for OpenAI provider tool calling.

### How to Run Rust Examples

To run the standalone Rust examples (if they are configured as binaries or examples in `Cargo.toml`), you would typically use:

```bash
cargo run --example agent_with_tools
```

*Note: You may need to move these back to `examples/` or configure `Cargo.toml` to point to them if you want to run them as cargo examples. Currently, they are located here for consolidation.*

To run tests:

```bash
cargo test
```
