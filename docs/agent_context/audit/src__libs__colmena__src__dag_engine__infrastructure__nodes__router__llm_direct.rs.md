# src/libs/colmena/src/dag_engine/infrastructure/nodes/router/llm_direct.rs

**Layer:** infrastructure  
**Purpose:** Implements Mode A (LLM-direct) routing where the LLM selects a branch by name from declared descriptions using an inline schema validation. Orchestrates the LLM call, constructs a structured prompt, and validates the response against the router configuration.

## Symbols

- `ROUTING_SYSTEM_MSG` (const) — System prompt loaded from `routing_classifier_system.md` template that guides the LLM during branch selection
- `pick_branch` (async fn, pub) — Main entry point; orchestrates LLM call to select a router branch by name, builds an inline enum schema for structured output, constructs system/user messages with branch descriptions and optional instructions, calls `extract_with_schema` for LLM invocation, validates the response has a "branch" field matching a known branch name, and returns (branch_index, reason)

## File-level notes

- **Error handling is ad-hoc string-based** (lines 80, 92): `ok_or("RouterRuntimeError: ...")` and `ok_or_else(|| format!(...))` return untyped string errors instead of a proper error enum. Makes error handling in callers harder and loses type information. [FLAG: improvement]
- **No pre-validation that branches is non-empty**: If `cfg.branches` is empty, the schema will have no valid options and any LLM response will fail with "unknown branch" error. A guard at the start would provide clearer failure semantics. [FLAG: improvement]
- **Hardcoded temperature 0.1** (line 72): Routing uses a fixed low temperature for deterministic selection; no caller override option. Likely intentional for consistency, but not configurable.
- **Inline schema is manually validated at runtime** (lines 88–92) rather than leveraging JSON Schema constraints; the schema description is human-readable guidance only, actual validation is the `position()` check.
- Error propagation is sound: `extract_with_schema` failures, schema conversion failures, and JSON field extraction all properly use `?` operator and reach the caller.
