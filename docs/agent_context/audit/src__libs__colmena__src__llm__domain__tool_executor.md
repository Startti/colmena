# src/libs/colmena/src/llm/domain/tool_executor.rs

**Layer:** domain  
**Purpose:** Defines the `ToolExecutor` trait, a domain-layer port/abstraction that allows the LLM module to request tool execution without knowing implementation details (e.g., DAG nodes).

## Symbols

- `ToolExecutor` (trait, pub) — Port abstraction for executing LLM tool calls; requires `Send + Sync`.
  - `execute` (async fn, pub) — Execute a tool call and return the result or LlmError.
  - `available_tools` (async fn, pub) — Get list of available tools (documentation notes this is "typically not used directly"). [FLAG: dead_candidate — method marked "typically not used" in own docstring; trait method with no in-file callers]

**Test module (cfg(test)):**
- `MockToolExecutor` (struct, private) — Test-only mock implementation of `ToolExecutor`.
  - `execute` (async fn) — Mock: returns success for "test_tool", ToolNotFound for others.
  - `available_tools` (async fn) — Mock: returns empty vector.
- `test_mock_executor_success` (fn, test) — Verifies successful tool execution returns correct output.
- `test_mock_executor_tool_not_found` (fn, test) — Verifies ToolNotFound error is raised for unknown tools.

## File-level notes

- **Trait documentation clarity**: The extensive rustdoc (lines 4–83) includes a complete example implementation and method-level documentation. Clear design intent.
- **Note on `available_tools`**: Line 72–82 docstring states the method is "typically not used directly by the agent service" and that filtering happens at LlmNode level. This suggests the method may be present for interface completeness or future extensibility rather than active use. No production callers visible in-file; only tested in `test_mock_executor_success`.
- **Test coverage**: Two unit tests cover success path and error path; mocking strategy is straightforward.
- **Pure port/trait**: No infrastructure dependencies; all imports are domain types (`LlmError`, `ToolCall`, `ToolDefinition`, `ToolResult`) or async-trait. Correctly isolated in domain layer.
