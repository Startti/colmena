# src/libs/colmena/src/llm/domain/tools.rs

**Layer:** domain  
**Purpose:** Defines core domain types for representing LLM tool definitions, calls, and results using JSON Schema format. Provides immutable value objects and builders for composing tool schemas, tool calls, and execution results across all LLM providers (OpenAI, Anthropic, Gemini).

## Symbols

- `ToolDefinition` (pub struct) — Represents a tool/function definition passable to LLMs with name, description, summary, parameters, and optional raw JSON Schema override
- `ToolDefinition::new` (pub fn) — Creates a new tool definition from name, description, and parameters
- `ToolDefinition::with_summary` (pub fn) — Builder: attaches a one-line summary (≤200 chars) for lazy-loading catalogs
- `ToolDefinition::with_input_schema_override` (pub fn) — Builder: attaches raw JSON Schema object that providers send verbatim, bypassing structured parameters validation
- `ToolDefinition::validate` (pub fn) — Validates tool well-formedness: non-empty name/description, parameters type is "object", all required fields exist in properties
- `ToolParameters` (pub struct) — JSON Schema definition for tool parameters with schema type, properties map, and required field list
- `ToolParameters::new` (pub fn) — Creates new tool parameters schema (type="object", empty properties and required list)
- `ToolParameters::with_property` (pub fn) — Builder: adds a property to the schema
- `ToolParameters::with_required` (pub fn) — Builder: marks a property as required (idempotent)
- `impl Default for ToolParameters` (impl) — Default constructor delegates to `new()`
- `ParameterProperty` (pub struct) — Definition of a single parameter property with JSON Schema type, description, optional enum values, regex pattern, and nested items (for arrays)
- `ParameterProperty::new` (pub fn) — Creates new parameter property from type and description
- `ParameterProperty::with_enum` (pub fn) — Builder: adds enum constraint (list of allowed string values)
- `ParameterProperty::with_pattern` (pub fn) — Builder: adds regex pattern constraint for string validation
- `ParameterProperty::with_items` (pub fn) — Builder: sets item type for array properties (required by OpenAI/Gemini strict JSON Schema validators); creates nested ParameterProperty with empty description
- `ToolCall` (pub struct) — Represents a tool call requested by the LLM with id, type, function, optional response, and optional provider signature
- `ToolCall::new` (pub fn) — Creates new tool call from id and FunctionCall
- `FunctionCall` (pub struct) — The actual function call details with name and JSON-encoded arguments string
- `FunctionCall::new` (pub fn) — Creates new function call from name and arguments JSON string
- `FunctionCall::parse_arguments` (pub fn) — Parses arguments string as JSON into a generic type T
- `ToolResult` (pub struct) — Result of executing a tool with tool_call_id, success flag, output string, and optional error message
- `ToolResult::success` (pub fn) — Creates successful tool result
- `ToolResult::failure` (pub fn) — Creates failed tool result
- `tests` (mod) — Unit test module (9 tests covering creation, validation, serialization, enum/pattern/array properties, tool calls, function call parsing, and roundtrip serialization)

## File-level notes

- **Well-structured domain layer**: All types are pure value objects with zero infrastructure dependencies; suitable for cross-platform reuse (Python/TypeScript bindings)
- **Comprehensive validation**: `ToolDefinition::validate()` enforces schema invariants; override mechanism lets synthetic tools use schemars-derived schemas
- **Builder pattern consistently applied**: All domain objects support fluent method chaining for composition
- **Strong test coverage**: 9 unit tests verify creation, validation, serialization, and edge cases (empty name, missing required properties, enum/array/pattern constraints)
- **Documentation**: Public items have clear doc comments explaining purpose and constraints (e.g., `items` field notes OpenAI/Gemini strict validation requirement)
- **Serialization safety**: serde derives with `skip_serializing_if` and `default` attributes ensure backward compatibility; `input_schema_override` allows bypassing structured validation for complex nested schemas

No flags identified. This is a clean, mature domain layer file with no dead code, incomplete implementations, or obvious improvements.
