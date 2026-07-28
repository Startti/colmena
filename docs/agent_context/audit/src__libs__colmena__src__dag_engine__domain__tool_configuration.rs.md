# src/libs/colmena/src/dag_engine/domain/tool_configuration.rs

**Layer:** domain  
**Purpose:** Defines data structures and parsing logic for the three tool-configuration approaches (node_schema, $DYNAMIC placeholders, deprecated fallback) that expose DAG nodes as LLM-callable tools. Provides validation, type safety, and serialization support for tool definitions.

## Symbols

- `DYNAMIC_PLACEHOLDER` (const) — Marker string "$DYNAMIC" used in fixed_config to indicate LLM-provided fields at runtime.
- `SubToolFilter` (enum) — Discriminated union for toolkit sub-tool filtering: explicit allow-list or "all" keyword.
- `SubToolKeyword` (enum) — Enum wrapper (`All`) to distinguish the string literal "all" from arbitrary bare strings in serde.
- `SubToolFilter::all()` (fn) — Constructor returning `Self::Keyword(SubToolKeyword::All)`.
- `SubToolFilter::is_all()` (fn) — Returns true if filter matches the `All` keyword.
- `SubToolFilter::includes()` (fn) — Checks whether a given sub-tool name is included in the filter.
- `ToolConfiguration` (struct) — Top-level configuration for exposing a node as an LLM tool, with fields for name, description, node_type, fixed_config, node_schema, tool-specific node_config, sub-tool exposure, lazy-load summary, and eager flag. Includes deprecated fallback fields.
- `ToolConfiguration::is_toolkit()` (fn) — Returns true if this entry exposes sub-tools (i.e., `expose_sub_tools` is present).
- `NodeSchemaField` (struct) — Single field entry in a node_schema, supporting fixed values, required flags, descriptions, patterns, nested properties (containers), and array items.
- `NodeSchema` (type alias) — HashMap mapping field names to NodeSchemaField; top-level schema structure passed to `parse_node_schema()`.
- `ParsedNodeSchema` (struct) — Output of parsing, containing fixed_values, llm_properties (parameter definitions), required_params list, and param_to_container mapping.
- `apply_array_items()` (fn, private) — Populates `prop.items` from `field.items` when field_type is "array"; validates items.type is present and returns helpful error for missing/incomplete array declarations.
- `parse_node_schema()` (fn, public) — Three-pass parser: (1) iterate top-level fields, (2) collect container children for collision detection, (3) apply dot-prefixes to child keys that collide across containers. Validates array fields, required types for LLM-visible fields, returns ParsedNodeSchema or descriptive error.

## File-level notes

- **Module-level documentation (lines 1–23)** provides complete explanation of the three configuration strategies with priority order and use-case guidance.
- **Deprecated fields** (`exposed_inputs`, `parameters`, `mergeable_fields`, `field_mapping`, lines 127–153) are all marked `#[deprecated(since = "0.3.0")]` and preserved for backward-compatibility deserialization; execution is handled by the executor (not this module).
- **Test coverage** is comprehensive (14 tests, lines 486–1110): fixed-only configs, required fields, nested containers, body containers, array items validation (with/without items), pattern passthrough, deeply nested containers, collision handling with dot-prefix logic, toolkit deserialization, lazy-load summary/eager flags, and regressions (fixed fields without type, LLM-visible fields with missing type).
- **Collision detection logic** (lines 457–476) ensures that when multiple containers have children with the same name, LLM parameters are exposed with dot-prefixed names (e.g., `"source_params.name"`, `"target_params.name"`) to prevent overwrites.
- **Deep nesting support** (lines 343–357): nested containers (e.g., `payload.edge`) collect their fixed sub-properties into a sub-object so the executor can deep-merge them with LLM-provided overlays.
- **Array validation**: array fields MUST declare `items.type` (lines 283–308, 400–407) to satisfy OpenAI and Gemini strict schema validators; error messages name the field path and show remedies.
- **No external integrations**: file has zero infrastructure dependencies (only serde, serde_json, std collections); pure domain logic.
