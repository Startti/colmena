# src/libs/colmena/src/dag_engine/infrastructure/nodes/router/config.rs

**Layer:** infrastructure  
**Purpose:** Defines router node configuration structures (RouterMode, BranchConfig, RouterConfig) and provides validation/parsing of router configs from JSON, enforcing mode-specific constraints on branches.

## Symbols

- `RouterMode` (pub enum) — Two-variant enum: LlmDirect (branches chosen by LLM) or ExtractAndRoute (branches chosen by schema-based extraction)
- `BranchConfig` (pub struct) — Configuration for one router branch: name, optional description, optional when-rule, optional subgraph
- `RouterConfig` (pub struct) — Top-level router configuration: mode, branches list, inline schema, instructions
- `NAME_RE` (const, private) — Regex pattern for branch names: lowercase alphanumeric + underscore, 1–64 chars
- `parse_and_validate` (pub fn) — Parses serde_json::Value into RouterConfig with full validation: mode check, branches non-empty, name uniqueness, mode-specific constraints (LlmDirect requires description + rejects when; ExtractAndRoute requires schema + when per branch), subgraph path/inline validation
- `tests::rejects_invalid_mode` (test) — Ensures invalid mode string is rejected
- `tests::rejects_empty_branches` (test) — Ensures empty branches array is rejected
- `tests::rejects_duplicate_branch_names` (test) — Ensures duplicate branch names trigger error
- `tests::rejects_invalid_branch_name_regex` (test) — Ensures uppercase/non-matching names are rejected
- `tests::llm_direct_rejects_branch_without_description` (test) — Ensures LlmDirect mode requires description per branch
- `tests::llm_direct_rejects_branch_with_when` (test) — Ensures LlmDirect mode rejects when clauses
- `tests::extract_and_route_requires_schema` (test) — Ensures ExtractAndRoute mode requires schema
- `tests::extract_and_route_requires_when` (test) — Ensures ExtractAndRoute mode requires when per branch
- `tests::subgraph_rejects_both_path_and_inline` (test) — Ensures subgraph cannot declare both child_graph_path and child_graph_inline
- `tests::subgraph_rejects_neither_path_nor_inline` (test) — Ensures subgraph must declare one of child_graph_path or child_graph_inline
- `tests::happy_path_llm_direct_three_branches` (test) — Smoke test: valid LlmDirect config with three branches parses successfully

## File-level notes

- **Regex compilation efficiency:** Line 47 compiles `NAME_RE` regex inside the function on every call to `parse_and_validate`. Should use `lazy_static!`, `thread_local!`, or `OnceLock` to compile once at module load. [FLAG: improvement]
- All test cases are present and cover happy path + major validation branches.
- Validation logic enforces two distinct modes with incompatible constraints; necessary complexity is present.
- Uses `inline_schema::inline_to_json_schema()` for schema parsing, delegating to utility module.
