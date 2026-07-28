# src/libs/colmena/src/dag_engine/infrastructure/nodes/planner.rs

**Layer:** infrastructure  
**Purpose:** Implements the PlannerNode, an LLM-backed executor that breaks down user requests into structured task lists with assignment, phasing, and parallelism metadata. Handles provider configuration, agent enumeration, schema construction, LLM invocation, and suspension for clarification requests.

## Symbols

- `default_planner_schema()` (fn, private) — Builds the built-in JSON schema template for task-list output (array of objects with task/assigned_to/completed/phase/parallel fields)
- `DEFAULT_PLANNER_SYSTEM_MSG` (const, private) — Loaded markdown file defining the system prompt for planner instructions
- `PlannerNode` (struct, pub) — Container holding an optional task memory repository for persistence
- `PlannerNode::new()` (fn, pub) — Constructor accepting optional DagTaskMemoryRepository
- `PlannerNode::resolve_env_var()` (fn, private) — Resolves `${VAR_NAME}` syntax in config strings to environment variables; errors if var missing
- `ExecutableNode for PlannerNode` (trait impl, pub) — Main trait implementation enabling planner execution in DAG
- `execute()` (async fn, pub via trait) — Core executor: checks for existing plan in DB, resolves provider/model/api_key, reads agent list (supports object/string formats), composes system + user messages with agent catalogue, calls LLM with temperature 0.1 and optional thinking budget, normalizes response (handles schema-wrapped outputs from some models), detects suspend-for-clarification requests, stores plan in shared state, returns tasks or questions
- `description()` (fn, pub via trait) — Returns user-facing description of node functionality
- `default_output()` (fn, pub via trait) — Returns "result" as the default output field name
- `schema()` (fn, pub via trait) — Returns introspection schema documenting config (provider, api_key, model, system_message), inputs (request, system_message), and outputs (result.items, extra_info.raw_response)
- `EmptyToolExecutor` (struct, private to execute()) — Stub tool executor satisfying ToolExecutor trait; used because planner never invokes tools
- `EmptyToolExecutor::execute()` (async fn, private) — Always returns error "No tools" when tools are attempted
- `EmptyToolExecutor::available_tools()` (async fn, private) — Always returns empty vector

## File-level notes

- **DB short-circuit (lines 86–102)**: Skips LLM call on Turn 2+ if tasks already exist in DB for the session, preventing wasted API calls. Relies on task_memory_repo; silently proceeds if repo unavailable.
- **Agent dual-format support (lines 138–169)**: Accepts agents as either `{ "name": "...", "description": "..." }` objects OR bare strings (looking up description from `__graph_nodes` in state). Gracefully handles empty agent lists with warning.
- **Schema adaptation (lines 201–238)**: Generates schema with enum constraint on `assigned_to` when agents are present; falls back to unconstrained default schema if no agents. Prevents LLM from inventing agent names.
- **Verbose logging (lines 288–301, 398–404)**: Full prompt and raw response are logged when `verbose=true` in config. Useful for debugging but not enabled by default.
- **Response normalization (lines 431–444)**: Handles three response shapes: bare task array (wraps as `{"tasks": array}`), schema-wrapped `{"type":"array", "items":[...]}` (defensive against models echoing schema back), or normal object. Defensive logic has a comment noting gpt-4o-mini as the trigger.
- **Suspension for clarification (lines 446–459)**: If LLM returns `{"questions": [...]}`  instead of tasks, emits `SUSPENDED` status with questions, allowing upstream to collect clarification before retry.
- **Streaming support (lines 344–376)**: Conditional streaming callback that forwards LLM tokens and usage events to observer only if `streaming=true` in config and observer is present.
- **Recent schema() fix (commit 4dceba33)**: schema() method output was recently corrected but remains incomplete relative to actual config fields accepted (agents, texts, verbose, thinking_budget, streaming not documented).

## Flagged Symbols

- `schema()` — **improvement** — schema() method documents only 4 of 9 config fields (missing agents, texts, verbose, thinking_budget, streaming); users and tools cannot discover full configuration surface from introspection (lines 488–506)
