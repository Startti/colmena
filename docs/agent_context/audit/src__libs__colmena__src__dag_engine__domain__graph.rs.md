# src/libs/colmena/src/dag_engine/domain/graph.rs

**Layer:** domain  **Purpose:** Defines the JSON-serializable data structures that represent a Colmena DAG graph configuration (nodes, edges, temporal/geographic context) and validates structural invariants before execution.

## Symbols

- `Graph` (struct, pub) — Root structure representing a complete graph.json file; contains node map, edges, and optional temporal/geographic/locale context
- `Graph::nodes` (field, pub) — HashMap mapping node IDs to their configurations
- `Graph::edges` (field, pub) — Vec of all directed edges between nodes
- `Graph::timezone` (field, pub) — Optional IANA timezone identifier for temporal-context injection into LLM prompts
- `Graph::location` (field, pub) — Optional human-readable location string for geographic-context injection
- `Graph::locale` (field, pub) — Optional BCP-47 locale tag for localization in prompt injection
- `Graph::validate()` (fn, pub) — Validates structural invariants: rejects node IDs containing `/` (reserved for subgraph path qualifiers) and validates all tool `node_schema` declarations for structural correctness (fail-fast before execution)
- `NodeConfig` (struct, pub) — Configuration for a single node in the graph
- `NodeConfig::node_type` (field, pub) — Type identifier of the node (e.g., "add", "log", "llm_call")
- `NodeConfig::config` (field, pub) — JSON value containing node-specific configuration parameters
- `NodeConfig::trigger_on` (field, pub) — Optional condition based on global `__colmena_loop_status` to determine if the node should run
- `NodeConfig::max_total_calls` (field, pub) — Optional maximum number of times this node can execute during a single DAG run
- `NodeConfig::max_calls_from` (field, pub) — Optional map of per-caller maximum call counts by caller node ID
- `Edge` (struct, pub) — Represents a directed connection from one node to another
- `Edge::from` (field, pub) — Source node ID
- `Edge::to` (field, pub) — Target node ID
- `Edge::cyclic` (field, pub) — Optional flag indicating if this edge forms a backward cycle (doesn't block target from initial execution)
- `tests` (module, test) — Test helpers and validation test cases
- `graph_with_node_id()` (fn, private) — Helper creating a test graph with a single node of given ID
- `validate_rejects_slash_in_node_id()` (test) — Verifies validation rejects node IDs containing `/`
- `validate_accepts_clean_node_id()` (test) — Verifies validation accepts node IDs without `/`
- `graph_with_tool_schema()` (fn, private) — Helper creating a test graph with a single tool having a given node_schema
- `validate_rejects_array_tool_field_without_items()` (test) — Regression test: validates that array fields without `items` are caught at validate() time
- `validate_accepts_array_tool_field_with_items()` (test) — Validates that array fields with proper `items` specification are accepted
- `validate_accepts_node_without_tools()` (test) — Validates that nodes without tool configurations pass validation
- `temporal_context_tests` (module, test) — Test suite for temporal/geographic/locale context parsing
- `graph_without_optional_fields_parses_with_none_defaults()` (test) — Verifies optional temporal/geographic fields default to None when omitted
- `graph_with_all_three_fields_parses_them()` (test) — Verifies all temporal/geographic/locale fields are correctly parsed when present
- `graph_with_partial_fields_parses()` (test) — Verifies partial temporal context fields are correctly deserialized

## File-level notes

- **Design**: Clean domain-layer data model with no infrastructure dependencies; serde handles JSON serialization/deserialization via `#[derive]` attributes
- **Validation**: `Graph::validate()` performs fail-fast structural validation, catching malformed tool schemas before execution and token spend; schema validation delegates to `parse_node_schema()` from tool_configuration module
- **Clone derive**: Explicitly added to all three main structs (noted in inline comments) to support graph cloning during execution or subgraph operations
- **Temporal context**: Three optional fields (timezone, location, locale) support prompt injection; all default to None with `#[serde(default)]`
- **Test coverage**: Comprehensive test suite covering node ID validation, tool schema validation, and temporal context parsing; tests use serde_json::json! for clarity
