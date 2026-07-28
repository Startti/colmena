# src/libs/colmena/src/llm/application/mod.rs

**Layer:** application  **Purpose:** Aggregates LLM application-layer submodules (use cases, services, utilities) and selectively re-exports public items. Entry point for callers accessing LLM orchestration logic.

## Symbols

- `agent_service` (mod, pub) — AgentService and related use-case logic for agent execution orchestration  
- `attachment_catalog` (mod, pub) — Attachment document catalog and resolution logic; not re-exported  
- `history_compaction` (mod, pub) — Conversation history compaction and semantic summarization; not re-exported  
- `llm_call_use_case` (mod, pub) — Core LLM call execution and streaming use case logic  
- `llm_health_check_use_case` (mod, pub) — LLM provider health-check use case  
- `llm_stream_use_case` (mod, pub) — LLM streaming orchestration use case  
- `tool_digest` (mod, pub) — Structured tool-result digest generation; not re-exported  
- `agent_service::*` (use, pub) — Re-exports all public items from agent_service module  
- `llm_call_use_case::*` (use, pub) — Re-exports all public items from llm_call_use_case module  
- `llm_health_check_use_case::*` (use, pub) — Re-exports all public items from llm_health_check_use_case module  
- `llm_stream_use_case::*` (use, pub) — Re-exports all public items from llm_stream_use_case module  

## File-level notes

- **Asymmetric re-exports:** Four modules (agent_service, llm_call_use_case, llm_health_check_use_case, llm_stream_use_case) are publicly re-exported, while three (attachment_catalog, history_compaction, tool_digest) are declared but not re-exported. This means external code can call `llm::agent_service::X` but must use `llm::application::attachment_catalog::X` for the unreexported modules. Consider whether this asymmetry is intentional (internal implementation) or an oversight (should be public). 
- **tool_digest visibility:** tool_digest was shipped as part of the v1.1 structured digest feature (2026-06-18) and is referenced in LLM streaming logic. Not being re-exported may limit discoverability; verify whether external callers require access or if it's intended as internal-only.
