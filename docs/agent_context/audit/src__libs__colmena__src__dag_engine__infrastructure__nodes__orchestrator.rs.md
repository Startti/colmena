# src/libs/colmena/src/dag_engine/infrastructure/nodes/orchestrator.rs

**Layer:** infrastructure  
**Purpose:** Implements OrchestratorNode, the autonomous orchestrator that manages the complete Plan → Execute → Critique → React lifecycle internally across multiple phases. Orchestrates planner, agents (as subgraphs), reactors, and critic LLMs with support for dynamic replanning, human-in-the-loop suspends at multiple decision points, and retry logic with critic feedback.

## Symbols

### Constants (9)
- `PHASE_REACTOR_TEMPLATE` (const) — Template for phase reactor schema constraints injected to guide new_tasks generation; loaded from `orchestrator_phase_reactor.md`
- `ORCHESTRATOR_GROUNDING` (const) — Anti-hallucination grounding rules appended to both reactor system messages; loaded from `orchestrator_grounding.md`
- `LLM_DEFAULT_SYSTEM` (const) — Fallback system message for final_reactor when user doesn't provide one; loaded from `llm_default_system.md`
- `KEY_PLANNER` (const) — `"planner"` config key and SSE node_id
- `KEY_PHASE_REACTOR` (const) — `"phase_reactor"` config key and SSE node_id
- `KEY_FINAL_REACTOR` (const) — `"final_reactor"` config key and SSE node_id
- `KEY_CRITIC` (const) — `"critic"` config key and SSE node_id
- `NODE_TYPE_PLANNER` (const) — `"planner"` node type for SSE events
- `NODE_TYPE_REACTOR` (const) — `"reactor"` node type for SSE events
- `NODE_TYPE_CRITIC` (const) — `"critic"` node type for SSE events

### Structs (4)
- `DirectThinkingObserver` (private struct) — ExecutionObserver wrapper that intercepts LlmToken events and forwards them as ThinkingToken to parent, dropping other event types; enables per-node thinking-token attribution for internal sub-nodes
  - `impl ExecutionObserver` — on_event filters LlmToken/LlmUsage/Reasoning* and passes through, drops others
  
- `SuspendQuestion` (private struct) — Single question in multi-question HITL suspend; serialized to JSON for SSE
  - `id: String` — question identifier
  - `question: String` — question text
  - `question_type: String` — either `"open"` (free text) or `"choice"` (predefined options)
  - `options: Option<Vec<String>>` — optional list of choice options (skipped if None)

- `OrchestratorSuspend` (private enum) — Return type for phase completion and finalization methods
  - `Done(Value)` — execution succeeded, carry output value
  - `Suspended(Value)` — HITL suspend encountered, carry suspend response with questions

- `OrchestratorNode` (public struct) — Main orchestrator node implementing ExecutableNode
  - `task_memory_repo: Option<Arc<dyn DagTaskMemoryRepository>>` — DB persistence for tasks, phases, summaries
  - `registry: Weak<dyn NodeRegistryPort>` — weak ref to node registry for spawning internal nodes

### Functions — Emission & Observation (3)
- `emit_internal_node_start` (private fn) — Emits NodeStart SSE event (SubgraphChildEvent) for internal sub-node with inputs and config
- `emit_internal_node_finish` (private fn) — Emits NodeFinish SSE event (SubgraphChildEvent) for internal sub-node with output
- `direct_thinking_observer` (private fn) — Factory: wraps observer to convert LlmToken→ThinkingToken with node attribution

### Functions — Database & Config (2)
- `seed_db_manually` (private async fn) — Backward-compat fallback to seed task DB from static `config["plan"]` if no internal planner; parses task_name, assigned_to, phase, parallel, context fields
- `resolve_env_var` (private fn) — Resolves `${VAR_NAME}` environment variable references in config strings; returns error if var not found

### Functions — Prompt Assembly (2)
- `build_enriched_prompt` (private fn) — Assembles 5-part agent prompt: USER CLARIFICATION (highest priority) → TASK CONTEXT → WHAT HAS HAPPENED SO FAR (phase summaries) → PREVIOUS ATTEMPT FEEDBACK (retry-only) → YOUR CURRENT TASK
- `inject_reactor_schema_constraints` (private fn) — Injects phase_reactor_template (with {agents_list} substituted) and ORCHESTRATOR_GROUNDING into reactor config's system_message

### Functions — Reactor & Resume Helpers (7)
- `extract_reactor_output_with_fallback` (private fn) — Parses reactor output: tries to deserialize result as JSON for summary/new_tasks, falls back to plain text summary + extra_info.add_tasks
- `read_suspend_meta` (private fn) — Reads `__orchestrator_suspend` from global_shared_state; returns (suspended_at, phase, task_id?, questions)
- `make_suspend_response` (private fn) — Writes suspend metadata to state and returns SUSPENDED output with questions array
- `propagate_agent_suspend` (private fn) — Propagates SUSPENDED from agent subgraph: tries canonical `questions` array, falls back to legacy single `question` string, wraps as SuspendQuestion
- `clear_suspend_meta` (private fn) — Removes `__orchestrator_suspend` from state after successful resume
- `allow_suspend_for` (private fn) — Checks component config's `allow_suspend` flag (default true)
- `debug_print_questions` (private fn) — Logs blocked suspend questions when `allow_suspend=false`

### Functions — Suspend Utility (2)
- `questions_to_context_string` (private fn) — Converts SuspendQuestion array + answer string to Q&A markdown for injection into LLM prompts
- `build_tasks_json` (private fn) — Converts Vec<DagTask> to JSON array with id, task_name, assigned_to, completed, phase, parallel, result.content

### Methods — OrchestratorNode (5)

- `new(task_memory_repo, registry)` (public fn, lines 147-155) — Constructor; stores DB repo and weak registry ref

- `handle_phase_completion` (private async fn, lines 157-559) — Executes phase_reactor after phase tasks complete
  - Collects phase task results, assembles reactor inputs (phase + result texts per agent)
  - Injects schema constraints, agent list, completed-task history into reactor config
  - Handles resuming from previous phase_reactor suspend (injects Q&A context)
  - Calls reactor_node, checks for suspend (HITL)
  - Parses reactor output: summary + new_tasks for dynamic replanning
  - Validates new_tasks: checks agent exists, deduplicates by (task_name, assigned_to)
  - Seeds next phase with new tasks, marks bridge tasks to run in current phase
  - Fallback (no reactor): concatenates phase results with 4000-char truncation; persists as summary
  - Returns OrchestratorSuspend::Done or ::Suspended

- `finalize_execution` (private async fn, lines 561-766) — Generates final response after all phases
  - Requires `final_reactor` config; assembles system message (user-provided or default + grounding) and user message (original prompt + phase summaries)
  - Configures LLM (provider, model, temperature, thinking_budget) from final_reactor config
  - Creates in-memory conversation repo and LLM agent service
  - Calls agent_service.run with pre-populated messages, max_turns=1, no tools
  - Streams LLM tokens via observer as plain LlmToken (user-facing, not thinking)
  - Returns final_response in FINISHED output

- `execute` (public async fn, lines 771-1786) — Main orchestrator loop; ~1000 lines
  - Detects resume context: reads `__colmena_resume_answer` from inputs, reads suspend metadata from state
  - **Resume detection block (lines 796-1416):** Handles 5 suspend scenarios:
    - `KEY_CRITIC`: Prepares Q&A context for agent re-execution; does NOT clear suspend_meta (resuming_critic guard does)
    - `KEY_PLANNER`: Accumulates planner Q&A, saves as phase 0 summary, clears suspend_meta, falls through to re-plan
    - `"agent"`: Flags resuming_agent_suspend, clears suspend_meta; answer flows to subgraph_node via __colmena_resume_answer
    - (Implicit: phase_reactor suspend handled inline in phase loop; critic_max_retries handled inline in task loop)
  - **Phase 1 — Auto-planning (lines 885-1097):**
    - If `config["planner"]` exists and DB empty: calls planner_node with user inputs + agent descriptions + context instruction
    - Checks for planner suspend (HITL); if resuming from planner suspend, injects accumulated Q&A as phase 0 summary
    - Parses planner result, seeds DB with initial plan
  - **Phase 2 — Main loop (lines 1107-1781):** For each phase:
    1. **Bridge task check:** If reactor already ran for this phase (flag set), all incomplete tasks are bridges; collects bridge results, advances to next phase
    2. **Phase reactor execution:** Calls handle_phase_completion; handles suspend; sets reactor-done flag if bridge tasks seeded
    3. **Task dispatch & execution (lines 1289-1757):** For each incomplete task in phase:
       - **Max-retries resume guard (lines 1292-1385):** If resuming from critic_max_retries suspend for this task: parses user answer (accept/skip/retry/cancel); accept restores stashed result, skip/cancel marks task done with note, retry injects Q&A context
       - **Critic suspend resume guard (lines 1389-1416):** If resuming from critic suspend for this task: discards stash, injects Q&A context as USER CLARIFICATION
       - **Agent execution (lines 1418-1523):** Builds enriched prompt; calls subgraph_node for agent
       - **Agent suspend propagation (lines 1534-1564):** If agent returned SUSPENDED, propagates up (no critic, no completion)
       - **Critic execution (lines 1566-1737):** If critic config exists:
         - Calls critic_node with agent result
         - Checks for critic suspend (HITL)
         - Reads critic's task_ok flag
         - If task rejected: captures feedback into state; increments retry counter; if max_retries exceeded and allow_suspend, returns max_retries suspend
         - Otherwise clears retry counter
       - **Task completion:** If task_ok, updates DB with result, cleans up stashes
    4. **Phase completion check:** After all tasks in phase, calls handle_phase_completion if not already called
    5. **Loop advance:** Continues to next phase
  - Falls back to error if no task_memory_repo

- `description()` (public fn) — Returns node description: "Autonomous Orchestrator that manages the full Plan -> Execute -> Critique -> React lifecycle internally."

- `default_output()` (public fn) — Returns `"final_response"` as default output field (implicit edges carry this field); note in comment: prior behavior returned `"result"` which silently fell back to whole object; downstream that relied on full object should switch to explicit `from: "<id>.all_tasks"` edges

- `schema()` (public fn) — Returns JSON schema: type=orchestrator, config keys (planner, agents, critic, phase_reactor, final_reactor), outputs (__colmena_loop_status, current_phase, phase_tasks, all_tasks, final_response)

### Trait Implementation — ExecutableNode for OrchestratorNode (line 769)
- Async execute method implementing full orchestration logic (see execute() above)

## File-level notes

- **Size:** 2244 lines; main execute() method is ~1000 lines with deeply nested logic (resume detection + planning + phase loop with task dispatch, criticism, retry)
- **Design:** Complex but well-structured for autonomous multi-phase orchestration; extensive use of stashing to state for survival across suspends
- **Logging:** Comprehensive emoji-prefixed colmena_log! output throughout for observability
- **Language mix:** Comments in Spanish and English (reflects bilingual codebase)
- **Include-str resources:** Loads text templates from `text/prompts/` directory at compile time
- **State management:** Uses `global_shared_state` (_state parameter) to persist suspend metadata, Q&A context, stashed results, retry counters across suspend/resume cycles
- **Weak registry ref:** Uses Weak<dyn NodeRegistryPort> to avoid circular ownership; upgrades on demand with error handling
- **No unsafe:** Pure safe Rust; relies on async_trait for async trait methods

## Flagged Symbols

### improvement: Line 1077 — println! should be colmena_log!
**Issue:** In the plan-printing loop (end of auto-planning section), one output uses `println!("{}", t)` instead of the project's `colmena_log!` macro.  
**Impact:** Inconsistent logging; output bypasses structured logging system used throughout the rest of the codebase.  
**Fix:** Replace with `colmena_log!("{}", t)` to maintain consistency.

### improvement: execute() method complexity
**Issue:** The execute() method (lines 771-1786, ~1000 lines) combines multiple concerns: resume detection (5 scenarios: planner, critic, phase_reactor, agent, critic_max_retries), auto-planning, phase loop control, bridge task logic, task dispatch, agent execution, criticism, and retry logic. Resume detection block alone (lines 796-1416) handles overlapping patterns.  
**Impact:** Difficult to test individual resume paths in isolation; high cognitive load for readers; potential for subtle bugs in guard conditions (e.g., resuming_critic vs. resuming_critic_max_retries guard ordering at lines 1292, 1389).  
**Improvement opportunity:** Extract helper methods for: resume-context detection/dispatch, auto-planning phase, bridge-task handling, task-execution loop. This would reduce nesting depth and allow independent testing of each resume path.

### improvement: Fallback manual seeding path ambiguity
**Issue:** Lines 1084-1095 implement backward-compat manual seeding (no internal planner). The preceding comment pattern "(lógica anterior de siembra manual)" suggests this is deferred/optional, but the call to seed_db_manually() is unconditional.  
**Impact:** Unclear intent; could confuse future maintainers about whether this code path is actively maintained or deprecated.  
**Suggestion:** Clarify with a comment (e.g., "Backward-compatibility fallback; prefer configuring 'planner' internally" or mark as deprecated if planned for removal).
