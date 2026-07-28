# src/libs/colmena/src/dag_engine/infrastructure/nodes/mod.rs

**Layer:** infrastructure  **Purpose:** Public module aggregator that re-exports all 35+ node type implementations. Each node module contains one or more `ExecutableNode` trait implementations for specific DAG operations (HTTP, LLM, SQL, Python, etc.).

## Symbols

- `api_explorer` (mod, pub) — API specification exploration node
- `critic` (mod, pub) — Critic/feedback evaluation node  
- `current_time` (mod, pub) — Current timestamp getter node
- `debug` (mod, pub) — Debug/logging node
- `document_nodes` (mod, pub) — Document processing nodes (PDF, text parsing)
- `echo_toolkit` (mod, pub) — Echo/passthrough utility node
- `extraction` (mod, pub) — Data extraction node
- `for_each` (mod, pub) — Deterministic list iteration node (v1, shipped 2026-07-20)
- `http` (mod, pub) — HTTP request node with multipart streaming support
- `http_oauth` (mod, pub) — HTTP with OAuth2 authentication node
- `image_edit` (mod, pub) — Image editing node (multimedia generation v1, 2026-05-22)
- `image_generation` (mod, pub) — Image generation node (multimedia generation v1, 2026-05-22)
- `input` (mod, pub) — Graph input node
- `llm` (mod, pub) — LLM call node (multi-provider: OpenAI, Anthropic, Gemini)
- `llm_synthetic_tools` (mod, pub) — Synthetic tool generation for LLM integration
- `loop_controller` (mod, pub) — Loop control flow node
- `math` (mod, pub) — Mathematical operations node
- `orchestrator` (mod, pub) — Orchestrator node (phases, HITL, critic loop)
- `output` (mod, pub) — Graph output node
- `output_parser` (mod, pub) — Output parsing/transformation node
- `planner` (mod, pub) — Planner node (multi-step task orchestration)
- `python_node` (mod, pub) — Python code execution node
- `qa_response_parser` (mod, pub) — Q&A response parsing node
- `reactor` (mod, pub) — Reactor node (response refinement)
- `router` (mod, pub) — Routing/conditional logic node
- `secure_suspend` (mod, pub) — Suspend with secure value masking node
- `socketio` (mod, pub) — Socket.IO bidirectional communication node
- `sql` (mod, pub) — SQL query execution node with auto-schema creation (v1, 2026-05-28)
- `subgraph` (mod, pub) — Nested DAG execution node (agents-as-tools, v1, 2026-06-19)
- `suspend` (mod, pub) — Human-in-the-loop suspend/resume node
- `task_memory_writer` (mod, pub) — Task memory persistence node
- `tavily_client` (mod, pub) — Tavily web search node
- `trigger` (mod, pub) — Event trigger/conditional node
- `tts` (mod, pub) — Text-to-speech generation node (multimedia generation v1, 2026-05-22)
- `util` (mod, pub) — Utility node helpers and shared code

## File-level notes

- **Pure module aggregator**: this file contains only module re-exports; no functions, structs, traits, or implementations.
- **Completeness**: 35 public modules match the 37 node types referenced in docs (some modules like `document_nodes` and `llm_synthetic_tools` may contain multiple node implementations).
- **Architecture alignment**: all modules are infrastructure-layer implementations of the `ExecutableNode` trait; they are discovered and registered in `registry.rs` at startup.
- **No issues detected**: all exports appear active (referenced in registry, tests, or public documentation).
