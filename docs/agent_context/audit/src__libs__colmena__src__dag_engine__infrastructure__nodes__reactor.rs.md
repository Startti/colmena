# src/libs/colmena/src/dag_engine/infrastructure/nodes/reactor.rs

**Layer:** infrastructure  **Purpose:** Final reviewer node that evaluates synthesized multi-agent responses and produces the user-facing answer or requests corrections/clarifications via structured LLM output.

## Symbols

- `reactor_schema()` (fn, private) — Returns JSON Schema object defining the fixed reactor output structure (task_ok, response, add_tasks, suspend, question)
- `DEFAULT_REACTOR_SYSTEM_MSG` (const, private) — Embeds the default system prompt from `text/prompts/reactor_system.md`
- `ReactorNode` (struct, pub) — Main node struct implementing the final reviewer logic in the orchestrator pipeline
- `task_memory_repo` (field, private) — Optional arc-wrapped DagTaskMemoryRepository dependency [FLAG: dead_candidate — field declared but never accessed in impl; #[allow(dead_code)] indicates acknowledged but retained]
- `ReactorNode::new()` (fn, pub) — Constructor accepting optional task memory repository
- `ReactorNode::resolve_env_var()` (fn, private) — Helper to resolve ${ENV_VAR} syntax in config string values
- `ExecutableNode::execute()` (fn, async, pub) — Core execution: collects review texts, calls LLM with fixed reactor schema, parses structured output (task_ok/response/add_tasks/suspend/question), returns result with extra_info
- `EmptyToolExecutor` (struct, private) — Local struct implementing ToolExecutor trait with no available tools
- `EmptyToolExecutor::execute()` (fn, async) — Tool execution stub returning error (no tools available)
- `EmptyToolExecutor::available_tools()` (fn, async) — Returns empty tool vector
- `ExecutableNode::description()` (fn, pub) — Returns node description: "Final reviewer that evaluates a synthesized response and either produces the user-facing answer or requests corrections/more info."
- `ExecutableNode::default_output()` (fn, pub) — Returns default output field name "result"
- `ExecutableNode::schema()` (fn, pub) — Returns node metadata schema documenting config (provider/api_key/model/verbose/system_message/texts), inputs (texts.*/system_message), and outputs (result/extra_info.task_ok/add_tasks/suspend/question/__colmena_status)

## File-level notes

- The node composes a system message combining `DEFAULT_REACTOR_SYSTEM_MSG`, optional user-supplied `system_message`, and the reactor JSON schema; sends to LLM with temperature 0.2 and optional thinking budget
- Input collection: all inputs prefixed with `texts.` are formatted as named sections; also accepts static texts from `config.texts`; requires at least one non-empty, non-system, non-user-request input to avoid skip (lines 170–175)
- Streaming support: when `config.streaming=true`, wraps observer to forward LLM tokens and usage as NodeEvents
- Output structure: wraps LLM response in `{ result, extra_info: { task_ok, add_tasks, suspend, question, __colmena_status } }` with __colmena_status set to "SUSPENDED" if suspend=true
- Parser resilience: strips markdown code fences (```json/```), handles escaped `\$` to prevent JSON parse failures, provides detailed error messages on parse failure
- Persistence note (lines 361–363): phase summary persistence is delegated to OrchestratorNode when reactor is used as an internal phase_reactor; standalone use does not auto-save (user must add a dedicated save node)
