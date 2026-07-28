# src/libs/colmena/src/dag_engine/domain/node.rs

**Layer:** domain  **Purpose:** Defines the `ExecutableNode` trait — the foundational port/contract that all DAG nodes must implement, plus the `NodeInputs` type alias for passing named inputs between nodes.

## Symbols

- `NodeInputs` (type alias, pub) — HashMap<String, Value> for passing named node outputs as inputs to downstream nodes
- `ExecutableNode` (trait, pub) — Async trait defining the core contract all executable nodes must implement; requires Send + Sync for thread-safe execution
- `ExecutableNode::execute` (async method) — Primary execution method: takes inputs, config, state, and optional observer; returns Value output or error
- `ExecutableNode::schema` (method) — Returns JSON Schema describing node configuration fields, expected inputs, and output structure
- `ExecutableNode::description` (method) — Returns human/LLM-readable description of node behavior; defaults to None
- `ExecutableNode::default_input` (method) — Returns optional default input port name for implicit edge mapping; defaults to None
- `ExecutableNode::default_output` (method) — Returns optional default output port name for implicit edge mapping; defaults to None
- `ExecutableNode::as_initializable` (method) — Returns optional InitializableNode reference for pre-flight schema enrichment with database/capability context (currently used by sql_query); defaults to None

## File-level notes

- Clean hexagonal domain layer: zero infrastructure dependencies, only imports shared traits and serde_json Value
- All trait methods have appropriate defaults (returning None) for optional overrides
- Well-documented with Spanish/English comments explaining purpose and usage
- Traits correctly marked Send + Sync to support thread-safe async execution across DAG engine
- Comment on `state` parameter mentions future phases ("lo usaremos más en M2") but reflects current design phase, not an issue

No flagged items.
