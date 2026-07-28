# src/libs/colmena/src/dag_engine/infrastructure/nodes/critic.rs

**Layer:** infrastructure  
**Purpose:** Implements the CriticNode, a DAG engine node that uses an LLM (OpenAI, Google, or Anthropic) to review specialist agent task results and decide whether the task is complete, needs revision, or requires user clarification.

## Symbols

- `critic_schema() -> Value` (fn, private) — Returns the fixed JSON schema that LLM outputs must conform to (`task_ok`, `feedback`, `suspend`, `question` fields).
- `DEFAULT_CRITIC_SYSTEM_MSG: &str` (const, private) — Default system prompt baked into every CriticNode, loaded from `text/prompts/critic_system.md`.
- `CriticNode` (struct, pub) — Marker struct (no fields) implementing the Critic node behavior.
- `Default for CriticNode` (impl, pub) — Provides default constructor via `Self::new()`.
- `CriticNode::new() -> Self` (pub fn) — Constructs a new CriticNode instance.
- `CriticNode::resolve_env_var(value: &str) -> Result<String, String>` (fn, private) — Resolves environment variable references in the form `${VAR_NAME}` to their actual values.
- `ExecutableNode for CriticNode` (impl, pub) — Trait implementation providing DAG node lifecycle.
  - `execute()` (async fn, pub) — Main execution: resolves provider config, collects input texts, composes system message with schema, calls LLM via AgentService, parses JSON response, returns structured output with `task_ok`, `feedback`, `suspend`, `question`, and `__colmena_status`.
  - `description()` (fn, pub) — Returns node purpose description.
  - `default_output()` (fn, pub) — Returns `"result"` as the default output key.
  - `schema()` (fn, pub) — Returns configuration schema documenting accepted config fields, inputs, and outputs (corrected in commit 56deba7d to align with actual return format).
- `EmptyToolExecutor` (struct, local to `execute()`) — Anonymous local implementation of `ToolExecutor` trait that provides no available tools.
- `ToolExecutor for EmptyToolExecutor` (impl, local to `execute()`) — Trait implementation that rejects all tool calls with `"No tools"` error and returns empty tool list.

## File-level notes

- No `todo!()`, `unimplemented!()`, or stub implementations detected.
- No dead code or unused symbols detected.
- Section numbering in execute() comments is off by one starting at line 150 (says "--- 4." should be "--- 3."), but this is cosmetic and does not affect functionality.
- All parameters passed to `execute()` are utilized: `_state` unused (required by trait), `_observer` initially unused but later passed to streaming callback at line 228.
- The node correctly separates concerns: `critic_schema()` defines LLM output validation schema, while `schema()` provides documentation/display metadata — both are necessary and distinct.
- Configuration resolves provider, API key, model, optional thinking budget, and optional system message override.
- Verbose logging support for debugging via `config.verbose` flag.
- Streaming support optional via `config.streaming` flag; when enabled, LLM tokens and usage are forwarded to the DAG execution observer.
- Error handling is comprehensive: missing config fields, invalid provider, JSON parse failures, and environment variable resolution all explicitly handled.
