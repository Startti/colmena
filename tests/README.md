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

- **`http_tool_configured.json`**: Demonstrates how to configure HTTP nodes as tools for an LLM agent. The LLM can "call" these tools (e.g., `fetch_users`, `create_user`) to interact with an external API.
- **`agent_with_tools.json`**: A basic example of an agent with tools enabled.
- **`agent_with_tools_postgres.json`**: An agent using PostgreSQL for conversation memory.
- **`agent_with_tools_postgres_recall.json`**: Demonstrates an agent recalling information from PostgreSQL memory.
- **`memory_postgres_example.json`**: Example focusing on PostgreSQL memory persistence.
- **`memory_sqlite_example.json`**: Example focusing on SQLite memory persistence.
- **`dynamic_http.json`**: Demonstrates dynamic HTTP requests.
- **`trigger.json`**: Shows how to use the TriggerWebhookNode.

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
