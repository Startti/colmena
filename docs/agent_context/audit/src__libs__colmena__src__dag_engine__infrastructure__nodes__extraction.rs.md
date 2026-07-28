# src/libs/colmena/src/dag_engine/infrastructure/nodes/extraction.rs

**Layer:** infrastructure  **Purpose:** DAG node for extracting structured information from unstructured text using LLM-based extraction with JSON schema validation. Supports task state management (add/delete) and HITL suspension.

## Symbols

- `DEFAULT_EXTRACTION_SYSTEM_MSG` (const, private) — System prompt template loaded from `text/prompts/extraction_system.md`, uses `{user_instructions}` and `{schema}` placeholders
- `ExtractionNode` (struct, pub) — Main node struct holding optional `DagTaskMemoryRepository` for task persistence
- `ExtractionNode::new` (fn, pub) — Constructor accepting optional task memory repository reference
- `ExtractionNode::resolve_env_var` (fn, private) — Resolves environment variables in `${VAR_NAME}` format; returns error if variable not found [FLAG: improvement — potential duplication, consider extracting to shared utils if pattern repeats across nodes]
- `ExecutableNode` impl — Core trait implementation defining node behavior
- `execute` (fn, pub async) — Main execution: resolves provider/API key/model, builds system message with schema, gathers texts from inputs or config, calls LLM via `extract_with_schema` utility, processes add/delete task directives, optionally suspends if requested, returns structured JSON with result and extra_info
- `description` (fn, pub) — Returns: "Extracts structured information from unstructured text based on a provided JSON schema using an LLM."
- `default_output` (fn, pub) — Returns default output field name: "result"
- `schema` (fn, pub) — Returns JSON schema describing node type, required config fields (provider, api_key, model optional, schema, system_message optional), inputs (texts object, optional system_message override), and outputs (result object, extra_info with suspend status and task list)

## File-level notes

- **Quote-stripping logic (lines 114–118)**: Text values serialized to JSON strings have quotes stripped via substring slicing. Fragile; relies on specific serde_json serialization format. Consider using `trim_matches('"')` or a more robust deserializer pattern if input encoding varies. [FLAG: improvement]
- **Empty schema pattern (line 154)**: Passes `json!({})` to `extract_with_schema` because "ExtractionNode does not validate against an inline schema". Design smell; explicit optional validation parameter or separate code path would be clearer than a no-op placeholder. [FLAG: improvement]
- **Silently ignored delete error (line 223)**: `let _ = repo.delete_task(id_str).await;` swallows any error during task deletion. Should log warning or propagate; users may assume delete succeeded when it failed. [FLAG: improvement]
- **Asymmetric suspend behavior**: Suspend flag only affects output if `task_memory_repo` is `Some(_)`; if repo is `None`, suspend is silently ignored. Not necessarily wrong, but unintuitive; consider validating or documenting this contract.
- **Provider and model cloning**: Lines 156–158 clone `provider_kind`, `api_key`, `model` before passing to `extract_with_schema`. Necessary for `Arc` but adds latency for large strings; verify necessity if perf-sensitive.
