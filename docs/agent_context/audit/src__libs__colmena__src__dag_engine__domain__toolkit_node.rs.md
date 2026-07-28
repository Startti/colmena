# src/libs/colmena/src/dag_engine/domain/toolkit_node.rs

**Layer:** domain  
**Purpose:** Defines the `ToolkitNode` trait and `SubToolDefinition` struct for nodes that expose multiple sub-tools to the LLM. Provides the reserved input key and marker interface for toolkit dispatch.

## Symbols

- `SUB_TOOL_INPUT_KEY` (const) — Reserved input key "__sub_tool" injected by DagToolExecutor to identify which sub-tool the LLM invoked.
- `SubToolDefinition` (struct) — One sub-tool within a toolkit node; holds name (Cow-wrapped for static/dynamic), description, LLM-visible JSON-Schema properties, and required parameter names.
- `SubToolDefinition::name` (field, pub) — Short programmatic name (e.g., "search", "navigate"); Cow<'static, str> allows static string literals or runtime-computed names.
- `SubToolDefinition::description` (field, pub) — Rich description shown to LLM; accuracy depends on this field.
- `SubToolDefinition::properties` (field, pub) — JSON-Schema-style HashMap of LLM-visible parameter definitions.
- `SubToolDefinition::required` (field, pub) — Vec of parameter names the LLM must supply.
- `ToolkitNode` (trait) — Marker trait for nodes exposing multiple sub-tools; extends ExecutableNode and requires `sub_tool_catalog()` implementation.
- `ToolkitNode::sub_tool_catalog()` (trait method) — Returns the sub-tools this node exposes given its static config; implementations should return empty Vec rather than panic on unexpected shape.
- `sub_tool_input_key_is_reserved_constant()` (test fn, private) — Unit test verifying SUB_TOOL_INPUT_KEY equals "__sub_tool".
- `sub_tool_definition_clone_is_cheap()` (test fn, private) — Unit test confirming SubToolDefinition clones efficiently (Cow-wrapped name stays cheap).

## File-level notes

- Clean, minimal domain-layer design; no infrastructure dependencies beyond imports of ExecutableNode (trait) and ParameterProperty (type).
- Doc comment references spec: `docs/superpowers/specs/2026-04-23-web-nodes-unified-design.md` § "Runtime extension: multi-tool per node" — design is intentional and spec-driven.
- Cow<'static, str> pattern documented inline; allows both static toolkits (string literals) and future dynamic toolkits (API explorer, computed at runtime).
- Tests are minimal but meaningful: constant value verification and clone-cheapness sanity check.
