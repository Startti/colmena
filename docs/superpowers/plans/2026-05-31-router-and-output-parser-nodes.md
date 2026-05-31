# Router & Output Parser Nodes — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship two new DAG nodes — `output_parser` (LLM-driven structured extraction wrapper) and `router` (declarative branching with LLM-direct and extract+rules modes) — fully tested, registered, and documented.

**Architecture:** Two shared helpers in `nodes/util/` (`inline_schema.rs` for schema conversion/validation; `extract_with_schema.rs` for the LLM call + JSON cleanup) are extracted from `extraction.rs` first. `output_parser.rs` is a thin wrapper around them. `router/` is a directory with `mod.rs`, `config.rs`, `when_dsl.rs`, `llm_direct.rs`, `extract_and_route.rs`. Subgraph dispatch per branch reuses `SubGraphNode` internally via the same `OnceLock<SubGraphExecutorPort>` pattern.

**Tech Stack:** Rust 1.95.0, async-trait, serde_json, mockall (for LLM mocking), regex crate. No new external dependencies.

**Spec:** [docs/superpowers/specs/2026-05-31-router-and-output-parser-nodes-design.md](../specs/2026-05-31-router-and-output-parser-nodes-design.md)

---

## File Structure

```
src/libs/colmena/src/dag_engine/infrastructure/nodes/
  util/
    mod.rs                       # MODIFY — add `pub mod inline_schema; pub mod extract_with_schema;`
    inline_schema.rs             # CREATE — convert + validate against inline-required schema
    extract_with_schema.rs       # CREATE — LLM call + JSON parse helper
  extraction.rs                  # MODIFY — refactor to use the helper (no behavior change)
  output_parser.rs               # CREATE — new node
  router/
    mod.rs                       # CREATE — RouterNode + ExecutableNode impl
    config.rs                    # CREATE — config types + init validation
    when_dsl.rs                  # CREATE — WhenRule enum + parser + evaluator
    llm_direct.rs                # CREATE — mode A logic
    extract_and_route.rs         # CREATE — mode B logic
  mod.rs                         # MODIFY — register `output_parser` and `router` modules
  prompts/
    routing_classifier_system.md # CREATE — system prompt for mode A
  registry.rs                    # MODIFY — register both nodes; wire executor for router

tests/graphs/control_flow/
  output_parser_basic.json       # CREATE
  router_llm_direct.json         # CREATE
  router_extract_rules.json      # CREATE
  router_with_subgraph.json      # CREATE
  router_chained.json            # CREATE

docs/
  node_configurations.json       # MODIFY — entries for both nodes
  agent_context/node_ports_reference.md  # MODIFY — port semantics per branch
  developer_guide/37_router_and_output_parser.md  # CREATE — new guide
  DEVELOPER_GUIDE.md             # MODIFY — index pointer
  CHANGELOG_*.md                 # MODIFY — current rolling changelog
```

---

## Phase 1 — Shared helpers (foundation)

### Task 1: `inline_schema.rs` — converter to standard JSON Schema

**Files:**
- Create: `src/libs/colmena/src/dag_engine/infrastructure/nodes/util/inline_schema.rs`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/util/mod.rs`

The inline schema convention used by `tool_configurations.node_schema` and the new nodes:

```json
{ "intent": { "type": "string", "required": true, "description": "..." } }
```

needs to become standard JSON Schema before being sent to LLM provider APIs:

```json
{ "type": "object", "properties": { "intent": {"type":"string","description":"..."} }, "required": ["intent"] }
```

- [ ] **Step 1: Write the failing test (converter happy path)**

Add to `inline_schema.rs`:

```rust
use serde_json::{json, Value};

/// Converts an inline-required schema to standard JSON Schema.
///
/// Inline form: `{ field_name: { type, required?, description? } }`.
/// Standard form: `{ type: "object", properties: {...}, required: [...] }`.
pub fn inline_to_json_schema(inline: &Value) -> Result<Value, String> {
    todo!()
}

/// Validates a JSON value against an inline schema.
/// Checks: required fields present (and not null) + type matches per declared field.
/// Returns Err with a human-readable message on first violation.
pub fn validate_against_inline_schema(value: &Value, inline: &Value) -> Result<(), String> {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_single_required_string_field() {
        let inline = json!({
            "intent": { "type": "string", "required": true, "description": "user intent" }
        });
        let out = inline_to_json_schema(&inline).unwrap();
        assert_eq!(
            out,
            json!({
                "type": "object",
                "properties": { "intent": { "type": "string", "description": "user intent" } },
                "required": ["intent"]
            })
        );
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p colmena_dag_engine --lib inline_schema::tests::converts_single_required_string_field`

Expected: FAIL with `not yet implemented`.

- [ ] **Step 3: Implement `inline_to_json_schema`**

Replace the `todo!()` body with:

```rust
pub fn inline_to_json_schema(inline: &Value) -> Result<Value, String> {
    let obj = inline
        .as_object()
        .ok_or_else(|| "inline schema must be a JSON object".to_string())?;
    if obj.is_empty() {
        return Err("inline schema must declare at least one field".to_string());
    }

    let mut properties = serde_json::Map::new();
    let mut required: Vec<Value> = Vec::new();

    for (field_name, field_def) in obj {
        let def_obj = field_def
            .as_object()
            .ok_or_else(|| format!("field '{}' must be a JSON object", field_name))?;

        let type_str = def_obj
            .get("type")
            .and_then(|v| v.as_str())
            .ok_or_else(|| format!("field '{}' missing 'type'", field_name))?;

        if !matches!(
            type_str,
            "string" | "number" | "integer" | "boolean" | "array" | "object"
        ) {
            return Err(format!(
                "field '{}' has invalid type '{}'",
                field_name, type_str
            ));
        }

        let mut prop = serde_json::Map::new();
        prop.insert("type".to_string(), json!(type_str));
        if let Some(desc) = def_obj.get("description") {
            prop.insert("description".to_string(), desc.clone());
        }
        properties.insert(field_name.clone(), Value::Object(prop));

        if def_obj
            .get("required")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            required.push(json!(field_name));
        }
    }

    Ok(json!({
        "type": "object",
        "properties": Value::Object(properties),
        "required": Value::Array(required)
    }))
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p colmena_dag_engine --lib inline_schema::tests::converts_single_required_string_field`

Expected: PASS.

- [ ] **Step 5: Add table-driven coverage for converter edge cases**

Add to the `tests` mod:

```rust
#[test]
fn converter_rejects_non_object_root() {
    let err = inline_to_json_schema(&json!("not an object")).unwrap_err();
    assert!(err.contains("must be a JSON object"));
}

#[test]
fn converter_rejects_empty_object() {
    let err = inline_to_json_schema(&json!({})).unwrap_err();
    assert!(err.contains("at least one field"));
}

#[test]
fn converter_rejects_field_with_invalid_type() {
    let inline = json!({ "x": { "type": "weird" } });
    let err = inline_to_json_schema(&inline).unwrap_err();
    assert!(err.contains("invalid type 'weird'"));
}

#[test]
fn converter_omits_required_array_when_no_field_is_required() {
    let inline = json!({ "x": { "type": "string" } });
    let out = inline_to_json_schema(&inline).unwrap();
    assert_eq!(out["required"], json!([]));
}

#[test]
fn converter_preserves_multiple_fields_and_descriptions() {
    let inline = json!({
        "a": { "type": "string", "required": true, "description": "alpha" },
        "b": { "type": "number" }
    });
    let out = inline_to_json_schema(&inline).unwrap();
    assert_eq!(out["properties"]["a"]["description"], json!("alpha"));
    assert_eq!(out["properties"]["b"]["type"], json!("number"));
    assert_eq!(out["required"], json!(["a"]));
}
```

Run: `cargo test -p colmena_dag_engine --lib inline_schema::tests`

Expected: 5 tests passing.

- [ ] **Step 6: Write the failing test for `validate_against_inline_schema`**

Append:

```rust
#[test]
fn validator_accepts_value_matching_schema() {
    let schema = json!({
        "intent": { "type": "string", "required": true },
        "confidence": { "type": "number" }
    });
    let value = json!({ "intent": "sales", "confidence": 0.9 });
    assert!(validate_against_inline_schema(&value, &schema).is_ok());
}
```

Run: `cargo test -p colmena_dag_engine --lib inline_schema::tests::validator_accepts_value_matching_schema`

Expected: FAIL with `not yet implemented`.

- [ ] **Step 7: Implement `validate_against_inline_schema`**

Replace the `todo!()` body:

```rust
pub fn validate_against_inline_schema(value: &Value, inline: &Value) -> Result<(), String> {
    let obj = value
        .as_object()
        .ok_or_else(|| "value must be a JSON object".to_string())?;
    let schema_obj = inline
        .as_object()
        .ok_or_else(|| "schema must be a JSON object".to_string())?;

    for (field_name, field_def) in schema_obj {
        let def_obj = field_def
            .as_object()
            .ok_or_else(|| format!("schema field '{}' must be a JSON object", field_name))?;

        let required = def_obj
            .get("required")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let field_value = obj.get(field_name);

        if required && field_value.map_or(true, |v| v.is_null()) {
            return Err(format!("required field '{}' is missing or null", field_name));
        }

        if let Some(v) = field_value {
            if v.is_null() {
                continue;
            }
            let expected_type = def_obj
                .get("type")
                .and_then(|v| v.as_str())
                .ok_or_else(|| format!("schema field '{}' missing 'type'", field_name))?;
            let actual_ok = match expected_type {
                "string" => v.is_string(),
                "number" => v.is_number(),
                "integer" => v.is_i64() || v.is_u64(),
                "boolean" => v.is_boolean(),
                "array" => v.is_array(),
                "object" => v.is_object(),
                _ => false,
            };
            if !actual_ok {
                return Err(format!(
                    "field '{}' expected type '{}', got '{}'",
                    field_name,
                    expected_type,
                    type_label(v)
                ));
            }
        }
    }

    Ok(())
}

fn type_label(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}
```

- [ ] **Step 8: Add validator coverage**

Append:

```rust
#[test]
fn validator_rejects_missing_required_field() {
    let schema = json!({ "intent": { "type": "string", "required": true } });
    let err = validate_against_inline_schema(&json!({}), &schema).unwrap_err();
    assert!(err.contains("required field 'intent'"));
}

#[test]
fn validator_rejects_null_required_field() {
    let schema = json!({ "intent": { "type": "string", "required": true } });
    let err = validate_against_inline_schema(&json!({ "intent": null }), &schema).unwrap_err();
    assert!(err.contains("required field 'intent'"));
}

#[test]
fn validator_accepts_null_for_optional_field() {
    let schema = json!({ "x": { "type": "string" } });
    assert!(validate_against_inline_schema(&json!({ "x": null }), &schema).is_ok());
}

#[test]
fn validator_rejects_wrong_type() {
    let schema = json!({ "n": { "type": "number" } });
    let err = validate_against_inline_schema(&json!({ "n": "not a number" }), &schema).unwrap_err();
    assert!(err.contains("expected type 'number'"));
    assert!(err.contains("got 'string'"));
}

#[test]
fn validator_rejects_non_object_root() {
    let schema = json!({ "x": { "type": "string" } });
    let err = validate_against_inline_schema(&json!("scalar"), &schema).unwrap_err();
    assert!(err.contains("must be a JSON object"));
}
```

Run: `cargo test -p colmena_dag_engine --lib inline_schema::tests`

Expected: all 10 tests passing.

- [ ] **Step 9: Wire module declaration**

In `src/libs/colmena/src/dag_engine/infrastructure/nodes/util/mod.rs` add:

```rust
pub mod inline_schema;
```

Run: `cargo check -p colmena_dag_engine`

Expected: clean.

- [ ] **Step 10: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/util/inline_schema.rs \
        src/libs/colmena/src/dag_engine/infrastructure/nodes/util/mod.rs
git commit -m "feat(nodes/util): inline-required schema converter and validator

Adds inline_to_json_schema() and validate_against_inline_schema() helpers
in nodes/util/inline_schema.rs. Shared infrastructure for the upcoming
output_parser and router nodes — converts the {field: {type, required,
description}} convention (already used in tool_configurations.node_schema)
to standard JSON Schema before LLM calls, and validates parsed responses
against the same form."
```

---

### Task 2: `extract_with_schema.rs` — LLM call + JSON cleanup helper

**Files:**
- Create: `src/libs/colmena/src/dag_engine/infrastructure/nodes/util/extract_with_schema.rs`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/util/mod.rs`

This helper encapsulates the LLM call pattern used by `extraction.rs`: build LlmProvider, set temperature 0.1, run a one-shot AgentService call, strip markdown fences, parse JSON, validate against the inline schema.

- [ ] **Step 1: Write the failing test (validation rejection path with a fake LLM)**

Create the file with:

```rust
use crate::dag_engine::domain::observer::{ExecutionObserver, NodeEvent};
use crate::llm::application::{AgentRunParams, AgentService};
use crate::llm::domain::{
    ConversationKey, LlmConfig, LlmMessage, LlmProvider, NodeIdPath, ProviderKind, SessionId,
    ToolCall, ToolDefinition, ToolExecutor, ToolResult, LlmError,
};
use crate::llm::infrastructure::persistence::in_memory_conversation_repository::InMemoryConversationRepository;
use crate::llm::infrastructure::LlmProviderFactory;
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

use super::inline_schema::validate_against_inline_schema;

/// Inputs the helper needs to make one structured-output LLM call.
pub struct ExtractInput<'a> {
    pub provider_kind: ProviderKind,
    pub api_key: String,
    pub model: Option<String>,
    pub system_message: String,
    pub user_text: String,
    pub inline_schema: &'a Value,
    pub temperature: Option<f32>,
    pub observer: Option<Arc<dyn ExecutionObserver>>,
}

/// Calls the LLM once with the given system+user messages, strips markdown
/// code fences from the response, parses JSON, and validates against the
/// inline schema. Returns the parsed JSON on success.
pub async fn extract_with_schema<'a>(
    input: ExtractInput<'a>,
) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
    let provider = LlmProvider::new(input.provider_kind.clone(), input.api_key, input.model)?;
    let mut llm_config = LlmConfig::new(provider);
    llm_config = llm_config.with_temperature(input.temperature.unwrap_or(0.1))?;

    let llm_repo = LlmProviderFactory::create(input.provider_kind);
    let conversation_repo = Arc::new(InMemoryConversationRepository::new());
    let agent_service = AgentService::new(llm_repo, conversation_repo);

    let tid_val = uuid::Uuid::new_v4().to_string();
    let tid = ConversationKey {
        session_id: SessionId(tid_val.clone()),
        agent_session_id: None,
        node_id: NodeIdPath(tid_val),
    };
    let messages = vec![
        LlmMessage::system(input.system_message)?,
        LlmMessage::user(input.user_text)?,
    ];

    struct EmptyToolExecutor;
    #[async_trait]
    impl ToolExecutor for EmptyToolExecutor {
        async fn execute(&self, _: &ToolCall) -> Result<ToolResult, LlmError> {
            Err(LlmError::ToolExecutionFailed {
                message: "No tools available".into(),
            })
        }
        async fn available_tools(&self) -> Vec<ToolDefinition> {
            vec![]
        }
    }

    let params = AgentRunParams {
        session_id: &tid,
        prompt: None,
        messages: Some(messages),
        config: llm_config,
        tools: vec![],
        tool_executor: &EmptyToolExecutor,
        max_iterations: Some(1),
        on_token: None,
        tools_provider: None,
        attachment_resolver: None,
        agent_session_id: None,
    };

    let response = agent_service.run(params).await?;

    if let Some(obs) = input.observer.clone() {
        if let Some(usage) = response.usage() {
            obs.on_event(NodeEvent::LlmUsage {
                prompt_tokens: usage.prompt_tokens,
                completion_tokens: usage.completion_tokens,
                thinking_tokens: usage.thinking_tokens,
                cache_read_tokens: usage.cache_read_tokens,
                cache_write_tokens: usage.cache_write_tokens,
            });
        }
    }

    let raw = response.content();
    let parsed = parse_and_validate(&raw, input.inline_schema)?;
    Ok(parsed)
}

/// Strips markdown code fences from a string and parses it as JSON,
/// then validates against the inline schema. Public so callers (and
/// tests) can drive it without an LLM.
pub fn parse_and_validate(
    raw: &str,
    inline_schema: &Value,
) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
    let mut s = raw.trim();
    if let Some(stripped) = s.strip_prefix("```json") {
        s = stripped;
    } else if let Some(stripped) = s.strip_prefix("```") {
        s = stripped;
    }
    if let Some(stripped) = s.strip_suffix("```") {
        s = stripped;
    }
    let s = s.trim();
    let parsed: Value = serde_json::from_str(s).map_err(|e| {
        format!("failed to parse LLM response as JSON: {}. raw: {}", e, raw)
    })?;
    validate_against_inline_schema(&parsed, inline_schema)
        .map_err(|e| format!("schema validation failed: {}", e))?;
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_and_validate_strips_json_fence() {
        let raw = "```json\n{\"intent\":\"sales\"}\n```";
        let schema = json!({ "intent": { "type": "string", "required": true } });
        let out = parse_and_validate(raw, &schema).unwrap();
        assert_eq!(out["intent"], json!("sales"));
    }
}
```

- [ ] **Step 2: Wire module declaration**

In `src/libs/colmena/src/dag_engine/infrastructure/nodes/util/mod.rs` add:

```rust
pub mod extract_with_schema;
```

Run: `cargo test -p colmena_dag_engine --lib extract_with_schema::tests::parse_and_validate_strips_json_fence`

Expected: PASS (the fence stripping + validation path is pure and runnable without an LLM).

- [ ] **Step 3: Add table-driven coverage for `parse_and_validate`**

Append to `tests` mod:

```rust
#[test]
fn parse_and_validate_strips_plain_fence() {
    let raw = "```\n{\"intent\":\"x\"}\n```";
    let schema = json!({ "intent": { "type": "string", "required": true } });
    assert!(parse_and_validate(raw, &schema).is_ok());
}

#[test]
fn parse_and_validate_accepts_unwrapped_json() {
    let raw = "  {\"intent\":\"sales\"}  ";
    let schema = json!({ "intent": { "type": "string", "required": true } });
    assert!(parse_and_validate(raw, &schema).is_ok());
}

#[test]
fn parse_and_validate_fails_on_invalid_json() {
    let raw = "this is not json";
    let schema = json!({ "intent": { "type": "string", "required": true } });
    let err = parse_and_validate(raw, &schema).unwrap_err().to_string();
    assert!(err.contains("failed to parse LLM response as JSON"));
}

#[test]
fn parse_and_validate_fails_on_schema_mismatch() {
    let raw = r#"{"intent": 42}"#;
    let schema = json!({ "intent": { "type": "string", "required": true } });
    let err = parse_and_validate(raw, &schema).unwrap_err().to_string();
    assert!(err.contains("schema validation failed"));
    assert!(err.contains("expected type 'string'"));
}

#[test]
fn parse_and_validate_fails_on_missing_required_field() {
    let raw = "{}";
    let schema = json!({ "intent": { "type": "string", "required": true } });
    let err = parse_and_validate(raw, &schema).unwrap_err().to_string();
    assert!(err.contains("required field 'intent'"));
}
```

Run: `cargo test -p colmena_dag_engine --lib extract_with_schema::tests`

Expected: 5 tests passing.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/util/extract_with_schema.rs \
        src/libs/colmena/src/dag_engine/infrastructure/nodes/util/mod.rs
git commit -m "feat(nodes/util): extract_with_schema LLM helper

Wraps the one-shot LLM call pattern used by extraction.rs: builds
LlmProvider, forces low temperature, runs AgentService with empty tools,
strips markdown code fences from the response, parses JSON, and validates
against an inline schema. Exposes parse_and_validate() as a pure helper so
unit tests can drive the parsing/validation path without an LLM.
Reused by extraction.rs, output_parser, and router (modes A and B)."
```

---

### Task 3: Refactor `extraction.rs` to use the helper (no observable change)

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/extraction.rs`

The goal here is *zero observable change*: same node behavior, same outputs, same logs. We're just routing the LLM call through `extract_with_schema` so the helper is exercised by existing tests.

- [ ] **Step 1: Replace the inline LLM call block in `ExtractionNode::execute`**

In `extraction.rs`, locate the block starting at `// --- 4. Call LLM using AgentService ---` and ending at the line that produces `parsed_json` (the result of `serde_json::from_str(clean_json_str)`). Replace it with a call to `extract_with_schema`:

```rust
        // --- 4 + 5. Call LLM and parse via shared helper ---
        use crate::dag_engine::infrastructure::nodes::util::extract_with_schema::{
            extract_with_schema, ExtractInput,
        };
        let parsed_json = extract_with_schema(ExtractInput {
            provider_kind: provider_kind.clone(),
            api_key: api_key.clone(),
            model: model.clone(),
            system_message: system_message.clone(),
            user_text: formatted_texts.clone(),
            // ExtractionNode does not validate against an inline schema; pass an
            // empty schema object so the validator is a no-op.
            inline_schema: &json!({}),
            temperature: Some(0.1),
            observer: _observer.clone(),
        })
        .await?;
```

Remove the now-unused blocks: the provider construction (`let provider = LlmProvider::new(...)`), the `LlmConfig::new(...)`, the `AgentService::new(...)`, the `ConversationKey` / messages construction, the `EmptyToolExecutor` struct, the `agent_service.run(...).await?`, the markdown fence stripping, the `serde_json::from_str(...)` call. Keep the verbose-log block — move it after the helper call.

The post-helper code (task_memory_repo handling, suspend check, final `Ok(json!(...))`) stays unchanged.

- [ ] **Step 2: Update the inline schema validation behavior**

We need a no-op validator when the caller doesn't want validation. The helper currently always validates against `inline_schema`. Make the validation skip when the schema is an empty object.

In `util/extract_with_schema.rs`, change the validation step in `parse_and_validate`:

```rust
    if !inline_schema
        .as_object()
        .map(|o| o.is_empty())
        .unwrap_or(false)
    {
        validate_against_inline_schema(&parsed, inline_schema)
            .map_err(|e| format!("schema validation failed: {}", e))?;
    }
```

And add a test in `util/extract_with_schema.rs`:

```rust
#[test]
fn parse_and_validate_skips_validation_for_empty_schema() {
    let raw = r#"{"anything": "goes", "extra": 42}"#;
    let schema = json!({});
    let out = parse_and_validate(raw, &schema).unwrap();
    assert_eq!(out["extra"], json!(42));
}
```

Run: `cargo test -p colmena_dag_engine --lib extract_with_schema::tests`

Expected: 6 tests passing (5 existing + this new one).

- [ ] **Step 3: Build the whole engine to confirm no compile errors**

Run: `cargo check -p colmena_dag_engine`

Expected: clean.

- [ ] **Step 4: Run all existing extraction tests to confirm no behavior regression**

Run: `cargo test -p colmena_dag_engine --lib extraction`

Expected: all existing extraction unit tests pass unchanged.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/extraction.rs \
        src/libs/colmena/src/dag_engine/infrastructure/nodes/util/extract_with_schema.rs
git commit -m "refactor(extraction): route LLM call through extract_with_schema helper

ExtractionNode now delegates the provider/messages/parse pipeline to the
shared extract_with_schema helper. Behavior is unchanged — empty inline
schema means the validator is a no-op, matching the legacy
ExtractionNode contract (no schema validation on output). This unifies
the LLM call path with the upcoming output_parser and router nodes."
```

---

## Phase 2 — `output_parser` node

### Task 4: `output_parser.rs` — config validation + empty input check

**Files:**
- Create: `src/libs/colmena/src/dag_engine/infrastructure/nodes/output_parser.rs`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/mod.rs`

- [ ] **Step 1: Scaffold the file with the ExecutableNode impl skeleton**

Create `output_parser.rs`:

```rust
use crate::dag_engine::domain::node::{ExecutableNode, NodeInputs};
use crate::dag_engine::domain::observer::ExecutionObserver;
use crate::llm::domain::ProviderKind;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::error::Error;
use std::sync::Arc;

use super::util::extract_with_schema::{extract_with_schema, ExtractInput};
use super::util::inline_schema::inline_to_json_schema;

const DEFAULT_SYSTEM_MSG: &str = include_str!("prompts/extraction_system.md");

pub struct OutputParserNode;

impl OutputParserNode {
    fn resolve_env_var(value: &str) -> Result<String, String> {
        if value.starts_with("${") && value.ends_with("}") {
            let var_name = &value[2..value.len() - 1];
            std::env::var(var_name)
                .map_err(|_| format!("Environment variable {} not found", var_name))
        } else {
            Ok(value.to_string())
        }
    }

    fn is_empty_input(v: &Value) -> bool {
        match v {
            Value::Null => true,
            Value::String(s) => s.trim().is_empty(),
            Value::Array(a) => a.is_empty(),
            Value::Object(o) => o.is_empty(),
            _ => false,
        }
    }
}

#[async_trait]
impl ExecutableNode for OutputParserNode {
    async fn execute(
        &self,
        inputs: &NodeInputs,
        config: &Value,
        _state: &mut Value,
        observer: Option<Arc<dyn ExecutionObserver>>,
    ) -> Result<Value, Box<dyn Error + Send + Sync>> {
        let provider_str = config
            .get("provider")
            .and_then(|v| v.as_str())
            .ok_or("OutputParser: missing 'provider' in config")?;
        let provider_kind = match provider_str.to_lowercase().as_str() {
            "openai" => ProviderKind::OpenAi,
            "google" => ProviderKind::Google,
            "anthropic" => ProviderKind::Anthropic,
            _ => return Err(format!("OutputParser: invalid provider '{}'", provider_str).into()),
        };
        let api_key_raw = config
            .get("api_key")
            .and_then(|v| v.as_str())
            .ok_or("OutputParser: missing 'api_key' in config")?;
        let api_key = Self::resolve_env_var(api_key_raw)?;
        let model = config
            .get("model")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let inline_schema = config
            .get("schema")
            .ok_or("OutputParser: missing 'schema' in config")?;
        // Validate the inline schema by attempting conversion now (init-time check).
        let json_schema = inline_to_json_schema(inline_schema)
            .map_err(|e| format!("OutputParser config error: {}", e))?;

        let input_raw = inputs
            .get("input")
            .cloned()
            .unwrap_or(Value::Null);
        if Self::is_empty_input(&input_raw) {
            return Err("OutputParserRuntimeError: missing input — nothing to parse".into());
        }

        let user_text = match &input_raw {
            Value::String(s) => s.clone(),
            other => serde_json::to_string_pretty(other)?,
        };

        let instructions = config
            .get("instructions")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let instructions_section = if instructions.is_empty() {
            String::new()
        } else {
            format!("\n\nContext/Rules for extraction:\n{}\n", instructions)
        };
        let system_message = DEFAULT_SYSTEM_MSG
            .replace("{user_instructions}", &instructions_section)
            .replace("{schema}", &serde_json::to_string_pretty(&json_schema)?);

        let temperature = config
            .get("temperature")
            .and_then(|v| v.as_f64())
            .map(|f| f as f32);

        extract_with_schema(ExtractInput {
            provider_kind,
            api_key,
            model,
            system_message,
            user_text,
            inline_schema,
            temperature,
            observer,
        })
        .await
    }

    fn default_input(&self) -> Option<&str> {
        Some("input")
    }

    fn description(&self) -> Option<&str> {
        Some(
            "Parses unstructured text (typically the output of an LLM or agent) into a JSON \
             object matching the provided inline schema. Thin wrapper around the extraction \
             engine with a single 'input' port and inline-required schema declaration.",
        )
    }

    fn schema(&self) -> Value {
        json!({
            "type": "output_parser",
            "config": {
                "provider": "string (openai | google | anthropic)",
                "api_key": "string",
                "model": "string (optional)",
                "schema": "inline schema: { field: { type, required?, description? } }",
                "instructions": "string (optional)",
                "temperature": "number (optional, default 0.1)"
            },
            "inputs": {
                "input": "any — text or value to parse"
            },
            "outputs": {
                "<schema fields>": "extracted JSON"
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_inputs(input: Value) -> NodeInputs {
        let mut m = NodeInputs::new();
        m.insert("input".to_string(), input);
        m
    }

    #[tokio::test]
    async fn fails_when_input_is_null() {
        let node = OutputParserNode;
        let config = json!({
            "provider": "google",
            "api_key": "fake",
            "schema": { "x": { "type": "string", "required": true } }
        });
        let mut state = json!({});
        let err = node
            .execute(&make_inputs(Value::Null), &config, &mut state, None)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("missing input"));
    }

    #[tokio::test]
    async fn fails_when_input_is_empty_string() {
        let node = OutputParserNode;
        let config = json!({
            "provider": "google",
            "api_key": "fake",
            "schema": { "x": { "type": "string", "required": true } }
        });
        let mut state = json!({});
        let err = node
            .execute(&make_inputs(json!("   ")), &config, &mut state, None)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("missing input"));
    }

    #[tokio::test]
    async fn fails_when_input_is_empty_array() {
        let node = OutputParserNode;
        let config = json!({
            "provider": "google",
            "api_key": "fake",
            "schema": { "x": { "type": "string", "required": true } }
        });
        let mut state = json!({});
        let err = node
            .execute(&make_inputs(json!([])), &config, &mut state, None)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("missing input"));
    }

    #[tokio::test]
    async fn fails_when_input_is_empty_object() {
        let node = OutputParserNode;
        let config = json!({
            "provider": "google",
            "api_key": "fake",
            "schema": { "x": { "type": "string", "required": true } }
        });
        let mut state = json!({});
        let err = node
            .execute(&make_inputs(json!({})), &config, &mut state, None)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("missing input"));
    }

    #[tokio::test]
    async fn fails_when_schema_is_invalid_inline() {
        let node = OutputParserNode;
        let config = json!({
            "provider": "google",
            "api_key": "fake",
            "schema": { "x": { "type": "weird" } }
        });
        let mut state = json!({});
        let err = node
            .execute(&make_inputs(json!("hello")), &config, &mut state, None)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("invalid type 'weird'"));
    }
}
```

- [ ] **Step 2: Wire module declaration**

In `src/libs/colmena/src/dag_engine/infrastructure/nodes/mod.rs` add:

```rust
pub mod output_parser;
```

(Alphabetical order — between `orchestrator` and `planner`.)

- [ ] **Step 3: Run the tests to verify**

Run: `cargo test -p colmena_dag_engine --lib output_parser::tests`

Expected: all 5 tests pass. Note: the empty-input tests fail BEFORE reaching the LLM, and the invalid-schema test fails at the inline-schema converter — none of these tests need an LLM mock.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/output_parser.rs \
        src/libs/colmena/src/dag_engine/infrastructure/nodes/mod.rs
git commit -m "feat(nodes): output_parser node

Thin LLM-driven structured output extractor designed to be chained right
after an llm_call or agent. Single 'input' port (default), inline-required
schema declaration, fails fast on empty input (null/\"\"/[]/{}). Delegates
the LLM call to nodes/util/extract_with_schema and converts the inline
schema to standard JSON Schema via nodes/util/inline_schema.

Differences from information_extraction: single input port instead of
texts.{name}, inline schema instead of standard JSON Schema, hard error
on missing input instead of silent skip, no orchestrator mutations."
```

---

### Task 5: Register `output_parser` + integration test graph

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/registry.rs`
- Create: `tests/graphs/control_flow/output_parser_basic.json`

- [ ] **Step 1: Register the node in `registry.rs`**

Find the block where `extraction` is registered (around line 155). After it, add:

```rust
            // --- Registrar Output Parser ---
            nodes.insert(
                "output_parser".to_string(),
                Arc::new(
                    crate::dag_engine::infrastructure::nodes::output_parser::OutputParserNode,
                ),
            );
```

- [ ] **Step 2: Add a registry test**

Find the `tavily_client_registered_as_executable_node` test in `registry.rs`. Add right after it:

```rust
    #[test]
    fn output_parser_registered_as_executable_node() {
        let registry = HashMapNodeRegistry::new(None, None);
        assert!(
            registry.get_node("output_parser").is_some(),
            "output_parser must be registered as an ExecutableNode"
        );
    }
```

Run: `cargo test -p colmena_dag_engine --lib output_parser_registered`

Expected: PASS.

- [ ] **Step 3: Create the integration test graph**

Create `tests/graphs/control_flow/output_parser_basic.json`:

```json
{
  "nodes": {
    "in": {
      "type": "input",
      "config": {}
    },
    "llm": {
      "type": "llm_call",
      "config": {
        "provider": "google",
        "model": "gemini-2.5-flash",
        "api_key": "${GEMINI_API_KEY}",
        "prompt": "Reply with one sentence describing this user message: {{user_message}}"
      }
    },
    "parser": {
      "type": "output_parser",
      "config": {
        "provider": "google",
        "model": "gemini-2.5-flash",
        "api_key": "${GEMINI_API_KEY}",
        "schema": {
          "intent":     { "type": "string", "required": true,  "description": "sales | support | billing | unknown" },
          "confidence": { "type": "number", "required": false, "description": "0..1" },
          "summary":    { "type": "string", "required": false, "description": "one-line summary" }
        },
        "instructions": "If you cannot determine the intent, use 'unknown'."
      }
    },
    "log": { "type": "log", "config": {} },
    "out": { "type": "output", "config": {} }
  },
  "edges": [
    { "from": "in.user_message", "to": "llm.user_message" },
    { "from": "llm", "to": "parser.input" },
    { "from": "parser", "to": "log" },
    { "from": "parser", "to": "out" }
  ]
}
```

- [ ] **Step 4: Smoke-test the graph against a real provider (manual)**

Run: `source .env && cargo run --bin dag_engine -- run tests/graphs/control_flow/output_parser_basic.json --agent-session-id parser_demo --include-extra-info`

Expected: prints a JSON object with `intent`, `confidence`, `summary` fields. If schema validation fails (the LLM hallucinated a non-string), the run fails with `schema validation failed: ...` (which is the desired behavior).

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/registry.rs \
        tests/graphs/control_flow/output_parser_basic.json
git commit -m "feat(registry): register output_parser node + basic integration graph

Registers output_parser in HashMapNodeRegistry alongside information_extraction.
Adds tests/graphs/control_flow/output_parser_basic.json which chains
llm_call -> output_parser -> log/output to exercise the new node end-to-end
against a real Gemini provider."
```

---

## Phase 3 — Router config + `when` DSL

### Task 6: `router/config.rs` — config types and init validation

**Files:**
- Create: `src/libs/colmena/src/dag_engine/infrastructure/nodes/router/mod.rs` (skeleton)
- Create: `src/libs/colmena/src/dag_engine/infrastructure/nodes/router/config.rs`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/mod.rs`

- [ ] **Step 1: Scaffold the `router/mod.rs` skeleton**

Create `router/mod.rs` with just module declarations:

```rust
//! Router node — declarative branching with LLM-direct and extract+rules modes.
//!
//! See: docs/superpowers/specs/2026-05-31-router-and-output-parser-nodes-design.md

pub mod config;
pub mod when_dsl;
pub mod llm_direct;
pub mod extract_and_route;

mod node;
pub use node::RouterNode;
```

Create a placeholder `router/node.rs` (will be filled in Task 9):

```rust
pub struct RouterNode;
```

Create placeholder `router/llm_direct.rs`:

```rust
// Mode A implementation — filled in Task 9.
```

Create placeholder `router/extract_and_route.rs`:

```rust
// Mode B implementation — filled in Task 10.
```

- [ ] **Step 2: Wire `nodes/mod.rs`**

In `src/libs/colmena/src/dag_engine/infrastructure/nodes/mod.rs` add (alphabetical, after `reactor`):

```rust
pub mod router;
```

Run: `cargo check -p colmena_dag_engine`. Expected: clean.

- [ ] **Step 3: Write the failing config-validation test**

Create `router/config.rs`:

```rust
use super::when_dsl::WhenRule;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub enum RouterMode {
    LlmDirect,
    ExtractAndRoute,
}

#[derive(Debug)]
pub struct BranchConfig {
    pub name: String,
    pub description: Option<String>,
    pub when: Option<WhenRule>,
    pub subgraph: Option<Value>,
}

#[derive(Debug)]
pub struct RouterConfig {
    pub mode: RouterMode,
    pub branches: Vec<BranchConfig>,
    pub inline_schema: Option<Value>,
    pub instructions: Option<String>,
}

const NAME_RE: &str = r"^[a-z][a-z0-9_]{0,63}$";

pub fn parse_and_validate(config: &Value) -> Result<RouterConfig, String> {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn rejects_invalid_mode() {
        let cfg = json!({ "mode": "weird", "branches": [] });
        let err = parse_and_validate(&cfg).unwrap_err();
        assert!(err.contains("invalid mode"));
    }
}
```

Run: `cargo test -p colmena_dag_engine --lib router::config::tests::rejects_invalid_mode`

Expected: FAIL (`not yet implemented`).

- [ ] **Step 4: Implement `parse_and_validate`**

Replace the `todo!()` body:

```rust
pub fn parse_and_validate(config: &Value) -> Result<RouterConfig, String> {
    let mode_str = config
        .get("mode")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "RouterConfigError: 'mode' is required".to_string())?;
    let mode = match mode_str {
        "llm_direct" => RouterMode::LlmDirect,
        "extract_and_route" => RouterMode::ExtractAndRoute,
        other => return Err(format!("RouterConfigError: invalid mode '{}'", other)),
    };

    let branches_val = config
        .get("branches")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "RouterConfigError: 'branches' must be a non-empty array".to_string())?;
    if branches_val.is_empty() {
        return Err("RouterConfigError: at least one branch required".to_string());
    }

    let name_re = regex::Regex::new(NAME_RE).unwrap();
    let mut seen_names = std::collections::HashSet::new();
    let mut branches = Vec::with_capacity(branches_val.len());

    let inline_schema = match mode {
        RouterMode::LlmDirect => None,
        RouterMode::ExtractAndRoute => {
            let s = config
                .get("schema")
                .ok_or_else(|| {
                    "RouterConfigError: extract_and_route requires schema".to_string()
                })?
                .clone();
            super::super::util::inline_schema::inline_to_json_schema(&s)
                .map_err(|e| format!("RouterConfigError: schema invalid — {}", e))?;
            Some(s)
        }
    };

    for (idx, b) in branches_val.iter().enumerate() {
        let name = b
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| format!("RouterConfigError: branch #{} missing 'name'", idx))?
            .to_string();
        if !name_re.is_match(&name) {
            return Err(format!(
                "RouterConfigError: invalid branch name '{}'",
                name
            ));
        }
        if !seen_names.insert(name.clone()) {
            return Err(format!(
                "RouterConfigError: duplicate branch name '{}'",
                name
            ));
        }

        let description = b
            .get("description")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let when_val = b.get("when");
        let subgraph = b.get("subgraph").cloned();

        if let Some(sg) = &subgraph {
            let has_path = sg.get("child_graph_path").is_some();
            let has_inline = sg.get("child_graph_inline").is_some();
            if has_path && has_inline {
                return Err(format!(
                    "RouterConfigError: branch '{}' subgraph declares both child_graph_path and child_graph_inline — pick one",
                    name
                ));
            }
            if !has_path && !has_inline {
                return Err(format!(
                    "RouterConfigError: branch '{}' subgraph requires child_graph_path or child_graph_inline",
                    name
                ));
            }
        }

        match mode {
            RouterMode::LlmDirect => {
                if when_val.is_some() {
                    return Err(format!(
                        "RouterConfigError: 'when' not allowed in llm_direct mode (branch '{}')",
                        name
                    ));
                }
                if description.is_none() {
                    return Err(format!(
                        "RouterConfigError: llm_direct requires description per branch (branch '{}')",
                        name
                    ));
                }
                branches.push(BranchConfig {
                    name,
                    description,
                    when: None,
                    subgraph,
                });
            }
            RouterMode::ExtractAndRoute => {
                let when_val = when_val.ok_or_else(|| {
                    format!(
                        "RouterConfigError: extract_and_route requires 'when' per branch (branch '{}')",
                        name
                    )
                })?;
                let when = WhenRule::parse(when_val, inline_schema.as_ref().unwrap())
                    .map_err(|e| format!("RouterConfigError: branch '{}' — {}", name, e))?;
                branches.push(BranchConfig {
                    name,
                    description,
                    when: Some(when),
                    subgraph,
                });
            }
        }
    }

    Ok(RouterConfig {
        mode,
        branches,
        inline_schema,
        instructions: config
            .get("instructions")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
    })
}
```

This references `WhenRule::parse` which is implemented in Task 7.

- [ ] **Step 5: Stub `WhenRule::parse` so config tests can run**

Replace the `router/when_dsl.rs` placeholder content:

```rust
use serde_json::Value;

#[derive(Debug, Clone)]
pub enum WhenRule {
    // Filled in Task 7.
    Stub,
}

impl WhenRule {
    pub fn parse(_when: &Value, _schema: &Value) -> Result<Self, String> {
        // Real parser arrives in Task 7. Until then, accept anything so config
        // tests that don't care about 'when' can run.
        Ok(WhenRule::Stub)
    }
}
```

- [ ] **Step 6: Add config table-driven coverage**

Append to `router/config.rs` tests:

```rust
    #[test]
    fn rejects_empty_branches() {
        let cfg = json!({ "mode": "llm_direct", "branches": [] });
        let err = parse_and_validate(&cfg).unwrap_err();
        assert!(err.contains("at least one branch"));
    }

    #[test]
    fn rejects_duplicate_branch_names() {
        let cfg = json!({
            "mode": "llm_direct",
            "branches": [
                { "name": "a", "description": "x" },
                { "name": "a", "description": "y" }
            ]
        });
        let err = parse_and_validate(&cfg).unwrap_err();
        assert!(err.contains("duplicate branch name 'a'"));
    }

    #[test]
    fn rejects_invalid_branch_name_regex() {
        let cfg = json!({
            "mode": "llm_direct",
            "branches": [ { "name": "BadName", "description": "x" } ]
        });
        let err = parse_and_validate(&cfg).unwrap_err();
        assert!(err.contains("invalid branch name 'BadName'"));
    }

    #[test]
    fn llm_direct_rejects_branch_without_description() {
        let cfg = json!({
            "mode": "llm_direct",
            "branches": [ { "name": "sales" } ]
        });
        let err = parse_and_validate(&cfg).unwrap_err();
        assert!(err.contains("requires description per branch"));
    }

    #[test]
    fn llm_direct_rejects_branch_with_when() {
        let cfg = json!({
            "mode": "llm_direct",
            "branches": [ { "name": "sales", "description": "x", "when": { "field": "y", "equals": "z" } } ]
        });
        let err = parse_and_validate(&cfg).unwrap_err();
        assert!(err.contains("'when' not allowed in llm_direct"));
    }

    #[test]
    fn extract_and_route_requires_schema() {
        let cfg = json!({
            "mode": "extract_and_route",
            "branches": [ { "name": "a", "when": {} } ]
        });
        let err = parse_and_validate(&cfg).unwrap_err();
        assert!(err.contains("requires schema"));
    }

    #[test]
    fn extract_and_route_requires_when() {
        let cfg = json!({
            "mode": "extract_and_route",
            "schema": { "intent": { "type": "string", "required": true } },
            "branches": [ { "name": "a" } ]
        });
        let err = parse_and_validate(&cfg).unwrap_err();
        assert!(err.contains("requires 'when' per branch"));
    }

    #[test]
    fn subgraph_rejects_both_path_and_inline() {
        let cfg = json!({
            "mode": "llm_direct",
            "branches": [ {
                "name": "a",
                "description": "x",
                "subgraph": { "child_graph_path": "p.json", "child_graph_inline": {} }
            } ]
        });
        let err = parse_and_validate(&cfg).unwrap_err();
        assert!(err.contains("pick one"));
    }

    #[test]
    fn subgraph_rejects_neither_path_nor_inline() {
        let cfg = json!({
            "mode": "llm_direct",
            "branches": [ { "name": "a", "description": "x", "subgraph": {} } ]
        });
        let err = parse_and_validate(&cfg).unwrap_err();
        assert!(err.contains("requires child_graph_path or child_graph_inline"));
    }

    #[test]
    fn happy_path_llm_direct_three_branches() {
        let cfg = json!({
            "mode": "llm_direct",
            "branches": [
                { "name": "sales",   "description": "buy" },
                { "name": "support", "description": "help" },
                { "name": "billing", "description": "money" }
            ]
        });
        let cfg = parse_and_validate(&cfg).unwrap();
        assert_eq!(cfg.mode, RouterMode::LlmDirect);
        assert_eq!(cfg.branches.len(), 3);
    }
```

Run: `cargo test -p colmena_dag_engine --lib router::config::tests`

Expected: 11 tests passing.

- [ ] **Step 7: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/router/ \
        src/libs/colmena/src/dag_engine/infrastructure/nodes/mod.rs
git commit -m "feat(router): config types and init validation

Adds router/config.rs with RouterConfig/BranchConfig/RouterMode types and
a parse_and_validate() entry point that enforces every init-time invariant
from the spec: mode enum, non-empty branches, name regex, no duplicates,
mode A requires description and forbids when, mode B requires schema and
when, subgraph requires exactly one of path/inline. WhenRule is stubbed
for now and filled in the next task."
```

---

### Task 7: `router/when_dsl.rs` — DSL parser + evaluator

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/router/when_dsl.rs`

The DSL supports: `equals`, `not_equals`, `in`, `contains`, `gt`, `lt`, `gte`, `lte`, `matches`, `exists`, and the combinators `all` / `any` / `not`. `field` paths use dotted notation.

- [ ] **Step 1: Replace the stub with the full enum and parser**

Replace `router/when_dsl.rs` content:

```rust
use regex::Regex;
use serde_json::Value;

#[derive(Debug, Clone)]
pub enum WhenRule {
    Equals { field: String, value: Value },
    NotEquals { field: String, value: Value },
    In { field: String, values: Vec<Value> },
    Contains { field: String, value: Value },
    Gt { field: String, value: f64 },
    Lt { field: String, value: f64 },
    Gte { field: String, value: f64 },
    Lte { field: String, value: f64 },
    Matches { field: String, regex: Regex },
    Exists { field: String },
    All(Vec<WhenRule>),
    Any(Vec<WhenRule>),
    Not(Box<WhenRule>),
}

impl WhenRule {
    pub fn parse(when: &Value, inline_schema: &Value) -> Result<Self, String> {
        let obj = when
            .as_object()
            .ok_or_else(|| "'when' must be a JSON object".to_string())?;

        if let Some(rules_val) = obj.get("all") {
            let arr = rules_val
                .as_array()
                .ok_or_else(|| "'all' must be an array".to_string())?;
            let rules = arr
                .iter()
                .map(|r| WhenRule::parse(r, inline_schema))
                .collect::<Result<Vec<_>, _>>()?;
            return Ok(WhenRule::All(rules));
        }
        if let Some(rules_val) = obj.get("any") {
            let arr = rules_val
                .as_array()
                .ok_or_else(|| "'any' must be an array".to_string())?;
            let rules = arr
                .iter()
                .map(|r| WhenRule::parse(r, inline_schema))
                .collect::<Result<Vec<_>, _>>()?;
            return Ok(WhenRule::Any(rules));
        }
        if let Some(inner) = obj.get("not") {
            let rule = WhenRule::parse(inner, inline_schema)?;
            return Ok(WhenRule::Not(Box::new(rule)));
        }

        let field = obj
            .get("field")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "'when' requires 'field' (or all/any/not)".to_string())?
            .to_string();

        // Validate the top-level (first segment) field is declared in the schema.
        let first = field.split('.').next().unwrap_or(&field);
        if let Some(schema_obj) = inline_schema.as_object() {
            if !schema_obj.contains_key(first) {
                return Err(format!("'when' references unknown field '{}'", first));
            }
        }

        if let Some(v) = obj.get("equals") {
            return Ok(WhenRule::Equals { field, value: v.clone() });
        }
        if let Some(v) = obj.get("not_equals") {
            return Ok(WhenRule::NotEquals { field, value: v.clone() });
        }
        if let Some(v) = obj.get("in") {
            let values = v
                .as_array()
                .ok_or_else(|| "'in' must be an array".to_string())?
                .clone();
            return Ok(WhenRule::In { field, values });
        }
        if let Some(v) = obj.get("contains") {
            return Ok(WhenRule::Contains { field, value: v.clone() });
        }
        if let Some(v) = obj.get("gt") {
            let n = v.as_f64().ok_or_else(|| "'gt' must be a number".to_string())?;
            return Ok(WhenRule::Gt { field, value: n });
        }
        if let Some(v) = obj.get("lt") {
            let n = v.as_f64().ok_or_else(|| "'lt' must be a number".to_string())?;
            return Ok(WhenRule::Lt { field, value: n });
        }
        if let Some(v) = obj.get("gte") {
            let n = v.as_f64().ok_or_else(|| "'gte' must be a number".to_string())?;
            return Ok(WhenRule::Gte { field, value: n });
        }
        if let Some(v) = obj.get("lte") {
            let n = v.as_f64().ok_or_else(|| "'lte' must be a number".to_string())?;
            return Ok(WhenRule::Lte { field, value: n });
        }
        if let Some(v) = obj.get("matches") {
            let s = v
                .as_str()
                .ok_or_else(|| "'matches' must be a string".to_string())?;
            let regex = Regex::new(s).map_err(|e| format!("invalid regex: {}", e))?;
            return Ok(WhenRule::Matches { field, regex });
        }
        if let Some(v) = obj.get("exists") {
            let b = v
                .as_bool()
                .ok_or_else(|| "'exists' must be a boolean".to_string())?;
            if !b {
                return Err("'exists: false' is not supported — use 'not: { ..., exists: true }'".to_string());
            }
            return Ok(WhenRule::Exists { field });
        }

        Err("'when' has no operator (equals/in/gt/.../exists)".to_string())
    }

    pub fn evaluate(&self, extracted: &Value) -> bool {
        match self {
            WhenRule::All(rs) => rs.iter().all(|r| r.evaluate(extracted)),
            WhenRule::Any(rs) => rs.iter().any(|r| r.evaluate(extracted)),
            WhenRule::Not(r) => !r.evaluate(extracted),
            WhenRule::Equals { field, value } => resolve(field, extracted).map_or(false, |v| &v == value),
            WhenRule::NotEquals { field, value } => resolve(field, extracted).map_or(true, |v| &v != value),
            WhenRule::In { field, values } => resolve(field, extracted).map_or(false, |v| values.contains(&v)),
            WhenRule::Contains { field, value } => match resolve(field, extracted) {
                Some(Value::String(s)) => match value {
                    Value::String(needle) => s.contains(needle.as_str()),
                    _ => false,
                },
                Some(Value::Array(a)) => a.contains(value),
                _ => false,
            },
            WhenRule::Gt { field, value } => resolve(field, extracted)
                .and_then(|v| v.as_f64())
                .map_or(false, |n| n > *value),
            WhenRule::Lt { field, value } => resolve(field, extracted)
                .and_then(|v| v.as_f64())
                .map_or(false, |n| n < *value),
            WhenRule::Gte { field, value } => resolve(field, extracted)
                .and_then(|v| v.as_f64())
                .map_or(false, |n| n >= *value),
            WhenRule::Lte { field, value } => resolve(field, extracted)
                .and_then(|v| v.as_f64())
                .map_or(false, |n| n <= *value),
            WhenRule::Matches { field, regex } => resolve(field, extracted)
                .and_then(|v| v.as_str().map(|s| s.to_string()))
                .map_or(false, |s| regex.is_match(&s)),
            WhenRule::Exists { field } => {
                resolve(field, extracted).map_or(false, |v| !v.is_null())
            }
        }
    }
}

fn resolve(path: &str, root: &Value) -> Option<Value> {
    let mut cur = root;
    for seg in path.split('.') {
        cur = match cur {
            Value::Object(o) => o.get(seg)?,
            _ => return None,
        };
    }
    Some(cur.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn schema() -> Value {
        json!({
            "intent":     { "type": "string", "required": true },
            "urgency":    { "type": "string" },
            "confidence": { "type": "number" },
            "user":       { "type": "object" }
        })
    }

    fn parse(when: Value) -> WhenRule {
        WhenRule::parse(&when, &schema()).unwrap()
    }

    #[test]
    fn equals_string() {
        let r = parse(json!({ "field": "intent", "equals": "sales" }));
        assert!(r.evaluate(&json!({ "intent": "sales" })));
        assert!(!r.evaluate(&json!({ "intent": "support" })));
        assert!(!r.evaluate(&json!({})));
    }

    #[test]
    fn equals_is_type_strict() {
        let r = parse(json!({ "field": "confidence", "equals": 5 }));
        assert!(r.evaluate(&json!({ "confidence": 5 })));
        assert!(!r.evaluate(&json!({ "confidence": "5" })));
    }

    #[test]
    fn not_equals() {
        let r = parse(json!({ "field": "intent", "not_equals": "sales" }));
        assert!(r.evaluate(&json!({ "intent": "support" })));
        assert!(!r.evaluate(&json!({ "intent": "sales" })));
        // missing field is considered "not equal"
        assert!(r.evaluate(&json!({})));
    }

    #[test]
    fn in_operator() {
        let r = parse(json!({ "field": "intent", "in": ["sales", "support"] }));
        assert!(r.evaluate(&json!({ "intent": "sales" })));
        assert!(r.evaluate(&json!({ "intent": "support" })));
        assert!(!r.evaluate(&json!({ "intent": "billing" })));
    }

    #[test]
    fn contains_string_substring() {
        let r = parse(json!({ "field": "intent", "contains": "ale" }));
        assert!(r.evaluate(&json!({ "intent": "sales" })));
        assert!(!r.evaluate(&json!({ "intent": "support" })));
    }

    #[test]
    fn gt_lt_gte_lte() {
        let gt = parse(json!({ "field": "confidence", "gt": 0.5 }));
        let lt = parse(json!({ "field": "confidence", "lt": 0.5 }));
        let gte = parse(json!({ "field": "confidence", "gte": 0.5 }));
        let lte = parse(json!({ "field": "confidence", "lte": 0.5 }));
        assert!(gt.evaluate(&json!({ "confidence": 0.9 })));
        assert!(!gt.evaluate(&json!({ "confidence": 0.5 })));
        assert!(lt.evaluate(&json!({ "confidence": 0.1 })));
        assert!(gte.evaluate(&json!({ "confidence": 0.5 })));
        assert!(lte.evaluate(&json!({ "confidence": 0.5 })));
    }

    #[test]
    fn matches_regex() {
        let r = parse(json!({ "field": "intent", "matches": "^sa.*s$" }));
        assert!(r.evaluate(&json!({ "intent": "sales" })));
        assert!(!r.evaluate(&json!({ "intent": "support" })));
    }

    #[test]
    fn exists_true() {
        let r = parse(json!({ "field": "urgency", "exists": true }));
        assert!(r.evaluate(&json!({ "urgency": "high" })));
        assert!(!r.evaluate(&json!({})));
        assert!(!r.evaluate(&json!({ "urgency": null })));
    }

    #[test]
    fn all_combinator() {
        let r = parse(json!({
            "all": [
                { "field": "intent", "equals": "sales" },
                { "field": "urgency", "equals": "high" }
            ]
        }));
        assert!(r.evaluate(&json!({ "intent": "sales", "urgency": "high" })));
        assert!(!r.evaluate(&json!({ "intent": "sales", "urgency": "low" })));
    }

    #[test]
    fn any_combinator() {
        let r = parse(json!({
            "any": [
                { "field": "intent", "equals": "sales" },
                { "field": "intent", "equals": "support" }
            ]
        }));
        assert!(r.evaluate(&json!({ "intent": "sales" })));
        assert!(r.evaluate(&json!({ "intent": "support" })));
        assert!(!r.evaluate(&json!({ "intent": "billing" })));
    }

    #[test]
    fn not_combinator() {
        let r = parse(json!({ "not": { "field": "intent", "equals": "sales" } }));
        assert!(!r.evaluate(&json!({ "intent": "sales" })));
        assert!(r.evaluate(&json!({ "intent": "support" })));
    }

    #[test]
    fn dotted_field_path() {
        let schema = json!({ "user": { "type": "object" } });
        let r = WhenRule::parse(
            &json!({ "field": "user.tier", "equals": "gold" }),
            &schema,
        )
        .unwrap();
        assert!(r.evaluate(&json!({ "user": { "tier": "gold" } })));
        assert!(!r.evaluate(&json!({ "user": { "tier": "silver" } })));
        assert!(!r.evaluate(&json!({})));
    }

    #[test]
    fn rejects_unknown_field_at_parse_time() {
        let err = WhenRule::parse(
            &json!({ "field": "unknown", "equals": "x" }),
            &schema(),
        )
        .unwrap_err();
        assert!(err.contains("unknown field 'unknown'"));
    }

    #[test]
    fn rejects_when_with_no_operator() {
        let err = WhenRule::parse(
            &json!({ "field": "intent" }),
            &schema(),
        )
        .unwrap_err();
        assert!(err.contains("no operator"));
    }

    #[test]
    fn rejects_invalid_regex() {
        let err = WhenRule::parse(
            &json!({ "field": "intent", "matches": "[" }),
            &schema(),
        )
        .unwrap_err();
        assert!(err.contains("invalid regex"));
    }

    #[test]
    fn nested_combinators() {
        let r = parse(json!({
            "any": [
                { "all": [
                    { "field": "intent", "equals": "sales" },
                    { "field": "urgency", "equals": "high" }
                ]},
                { "field": "intent", "equals": "support" }
            ]
        }));
        assert!(r.evaluate(&json!({ "intent": "sales", "urgency": "high" })));
        assert!(r.evaluate(&json!({ "intent": "support" })));
        assert!(!r.evaluate(&json!({ "intent": "sales", "urgency": "low" })));
    }
}
```

Note: this file uses the `regex` crate. Verify it's already in `Cargo.toml` with `grep '^regex' src/libs/colmena/Cargo.toml`. If absent, add `regex = "1"` to the `[dependencies]` section.

- [ ] **Step 2: Run all DSL tests**

Run: `cargo test -p colmena_dag_engine --lib router::when_dsl::tests`

Expected: 16 tests passing.

- [ ] **Step 3: Re-run config tests to confirm WhenRule::parse integration works**

Run: `cargo test -p colmena_dag_engine --lib router::config::tests`

Expected: 11 tests still pass; nothing broken by replacing the stub.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/router/when_dsl.rs \
        src/libs/colmena/src/dag_engine/infrastructure/nodes/router/config.rs
git commit -m "feat(router): when DSL parser and evaluator

Implements WhenRule enum + parser + evaluator covering all spec operators:
equals, not_equals, in, contains, gt/lt/gte/lte, matches (regex), exists,
and combinators all/any/not. Dotted-path field resolution with miss → false
semantics. Regex is compiled once at parse time. Parser validates that the
top-level field name appears in the inline schema, surfacing typos at
init rather than at runtime."
```

---

## Phase 4 — Router execution

### Task 8: Routing classifier system prompt

**Files:**
- Create: `src/libs/colmena/src/dag_engine/infrastructure/nodes/prompts/routing_classifier_system.md`

- [ ] **Step 1: Create the prompt file**

```markdown
You are a routing classifier. Read the user input and pick the single most appropriate branch from the list below.

Branches:
{branches}

Rules:
- Reply with exactly one branch name from the list above, matching the exact spelling.
- Provide a brief reason (one sentence) explaining your choice.
{user_instructions}
Output ONLY valid JSON matching this schema. Do NOT wrap the JSON in markdown blocks.
{schema}
```

The `{branches}` placeholder is replaced at runtime with a bullet list of `- <name>: <description>` lines. `{user_instructions}` and `{schema}` follow the same convention as `extraction_system.md`.

- [ ] **Step 2: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/prompts/routing_classifier_system.md
git commit -m "feat(router): routing classifier system prompt"
```

---

### Task 9: `router/node.rs` — main execution path (LLM-direct mode A)

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/router/node.rs`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/router/llm_direct.rs`

This task ships the router executing **only mode A** (LLM-direct), with subgraph dispatch postponed to Task 11. Mode B follows in Task 10.

- [ ] **Step 1: Implement mode A logic**

Replace `router/llm_direct.rs`:

```rust
use serde_json::{json, Value};

use super::config::{BranchConfig, RouterConfig};
use crate::dag_engine::domain::observer::ExecutionObserver;
use crate::dag_engine::infrastructure::nodes::util::extract_with_schema::{
    extract_with_schema, ExtractInput,
};
use crate::llm::domain::ProviderKind;
use std::sync::Arc;

const ROUTING_SYSTEM_MSG: &str =
    include_str!("../prompts/routing_classifier_system.md");

/// Picks the winning branch for mode A and returns (branch_index, llm_reason).
pub async fn pick_branch(
    cfg: &RouterConfig,
    provider_kind: ProviderKind,
    api_key: String,
    model: Option<String>,
    user_text: String,
    observer: Option<Arc<dyn ExecutionObserver>>,
) -> Result<(usize, String), Box<dyn std::error::Error + Send + Sync>> {
    // Build the enum of valid names + the bullet-list prompt context.
    let names: Vec<String> = cfg.branches.iter().map(|b| b.name.clone()).collect();
    let bullets: String = cfg
        .branches
        .iter()
        .map(|b: &BranchConfig| {
            format!(
                "- {}: {}",
                b.name,
                b.description.as_deref().unwrap_or("(no description)")
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    // Schema fed to extract_with_schema for the LLM's structured reply.
    // We use an inline schema so the same validator catches off-enum answers.
    let inline_schema = json!({
        "branch": {
            "type": "string",
            "required": true,
            "description": format!("must be one of: {}", names.join(", "))
        },
        "reason": { "type": "string", "required": false }
    });
    let json_schema =
        crate::dag_engine::infrastructure::nodes::util::inline_schema::inline_to_json_schema(
            &inline_schema,
        )?;

    let instructions_section = match &cfg.instructions {
        Some(s) if !s.is_empty() => format!("\n\nAdditional rules:\n{}\n", s),
        _ => String::new(),
    };
    let system_message = ROUTING_SYSTEM_MSG
        .replace("{branches}", &bullets)
        .replace("{user_instructions}", &instructions_section)
        .replace("{schema}", &serde_json::to_string_pretty(&json_schema)?);

    let parsed = extract_with_schema(ExtractInput {
        provider_kind,
        api_key,
        model,
        system_message,
        user_text,
        inline_schema: &inline_schema,
        temperature: Some(0.1),
        observer,
    })
    .await?;

    let chosen = parsed
        .get("branch")
        .and_then(|v| v.as_str())
        .ok_or("RouterRuntimeError: llm response missing 'branch' field")?
        .to_string();
    let reason = parsed
        .get("reason")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let idx = cfg
        .branches
        .iter()
        .position(|b| b.name == chosen)
        .ok_or_else(|| {
            format!(
                "RouterRuntimeError: llm picked unknown branch '{}'",
                chosen
            )
        })?;
    Ok((idx, reason))
}
```

- [ ] **Step 2: Implement `RouterNode::execute` skeleton with mode A wired in**

Replace `router/node.rs`:

```rust
use crate::dag_engine::domain::node::{ExecutableNode, NodeInputs};
use crate::dag_engine::domain::observer::ExecutionObserver;
use crate::llm::domain::ProviderKind;
use async_trait::async_trait;
use serde_json::{json, Map, Value};
use std::error::Error;
use std::sync::Arc;

use super::config::{parse_and_validate, RouterMode};
use super::llm_direct::pick_branch as pick_llm_direct;

pub struct RouterNode;

impl RouterNode {
    fn resolve_env_var(value: &str) -> Result<String, String> {
        if value.starts_with("${") && value.ends_with("}") {
            let var_name = &value[2..value.len() - 1];
            std::env::var(var_name)
                .map_err(|_| format!("Environment variable {} not found", var_name))
        } else {
            Ok(value.to_string())
        }
    }

    fn is_empty_input(v: &Value) -> bool {
        match v {
            Value::Null => true,
            Value::String(s) => s.trim().is_empty(),
            Value::Array(a) => a.is_empty(),
            Value::Object(o) => o.is_empty(),
            _ => false,
        }
    }
}

#[async_trait]
impl ExecutableNode for RouterNode {
    async fn execute(
        &self,
        inputs: &NodeInputs,
        config: &Value,
        _state: &mut Value,
        observer: Option<Arc<dyn ExecutionObserver>>,
    ) -> Result<Value, Box<dyn Error + Send + Sync>> {
        // 1. Parse + validate config (re-runs every execute; cheap).
        let cfg = parse_and_validate(config).map_err(|e| -> Box<dyn Error + Send + Sync> { e.into() })?;

        // 2. Read input.
        let input_raw = inputs.get("input").cloned().unwrap_or(Value::Null);
        if Self::is_empty_input(&input_raw) {
            return Err("RouterRuntimeError: missing input — nothing to route".into());
        }
        let user_text = match &input_raw {
            Value::String(s) => s.clone(),
            other => serde_json::to_string_pretty(other)?,
        };

        // 3. Resolve LLM provider config (shared by both modes).
        let provider_str = config
            .get("provider")
            .and_then(|v| v.as_str())
            .ok_or("Router: missing 'provider' in config")?;
        let provider_kind = match provider_str.to_lowercase().as_str() {
            "openai" => ProviderKind::OpenAi,
            "google" => ProviderKind::Google,
            "anthropic" => ProviderKind::Anthropic,
            _ => return Err(format!("Router: invalid provider '{}'", provider_str).into()),
        };
        let api_key_raw = config
            .get("api_key")
            .and_then(|v| v.as_str())
            .ok_or("Router: missing 'api_key' in config")?;
        let api_key = Self::resolve_env_var(api_key_raw)?;
        let model = config
            .get("model")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        // 4. Pick a branch.
        let (idx, reason, extracted): (usize, String, Option<Value>) = match cfg.mode {
            RouterMode::LlmDirect => {
                let (i, r) = pick_llm_direct(
                    &cfg,
                    provider_kind,
                    api_key,
                    model,
                    user_text,
                    observer.clone(),
                )
                .await?;
                (i, r, None)
            }
            RouterMode::ExtractAndRoute => {
                return Err("RouterRuntimeError: extract_and_route mode not yet implemented".into());
            }
        };

        let selected = &cfg.branches[idx];
        let payload = match &extracted {
            Some(e) => json!({ "input": input_raw, "extracted": e }),
            None => json!({ "input": input_raw }),
        };

        // 5. Emit __decision + one payload per port (null for non-selected).
        let mut out = Map::new();
        out.insert(
            "__decision".to_string(),
            json!({
                "selected_branch": selected.name,
                "reason": reason,
                "extracted": extracted
            }),
        );
        for (i, b) in cfg.branches.iter().enumerate() {
            out.insert(
                b.name.clone(),
                if i == idx { payload.clone() } else { Value::Null },
            );
        }

        Ok(Value::Object(out))
    }

    fn default_input(&self) -> Option<&str> {
        Some("input")
    }

    fn description(&self) -> Option<&str> {
        Some(
            "Routes the input to one of N branches. Mode 'llm_direct' lets an LLM pick the \
             branch by name from descriptions. Mode 'extract_and_route' extracts a JSON object \
             against a schema and applies declarative 'when' rules to pick the branch. \
             Fails fast if no branch matches.",
        )
    }

    fn schema(&self) -> Value {
        json!({
            "type": "router",
            "config": {
                "mode": "string (llm_direct | extract_and_route)",
                "provider": "string",
                "api_key": "string",
                "model": "string (optional)",
                "schema": "inline schema (mode B only)",
                "branches": "array of branch configs"
            },
            "inputs": { "input": "any" },
            "outputs": {
                "<branch_name>": "object — non-null only on selected branch",
                "__decision": "object — { selected_branch, reason, extracted }"
            }
        })
    }
}
```

- [ ] **Step 3: Add unit tests for the empty-input fail-fast path**

Append to `router/node.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> Value {
        json!({
            "mode": "llm_direct",
            "provider": "google",
            "api_key": "fake",
            "branches": [
                { "name": "a", "description": "x" },
                { "name": "b", "description": "y" }
            ]
        })
    }

    fn inputs(v: Value) -> NodeInputs {
        let mut m = NodeInputs::new();
        m.insert("input".to_string(), v);
        m
    }

    #[tokio::test]
    async fn fails_when_input_is_null() {
        let node = RouterNode;
        let mut state = json!({});
        let err = node
            .execute(&inputs(Value::Null), &cfg(), &mut state, None)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("missing input"));
    }

    #[tokio::test]
    async fn fails_when_input_is_empty_string() {
        let node = RouterNode;
        let mut state = json!({});
        let err = node
            .execute(&inputs(json!("  ")), &cfg(), &mut state, None)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("missing input"));
    }

    #[tokio::test]
    async fn fails_on_invalid_config_at_runtime() {
        let node = RouterNode;
        let mut state = json!({});
        let err = node
            .execute(
                &inputs(json!("anything")),
                &json!({ "mode": "weird", "branches": [] }),
                &mut state,
                None,
            )
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("invalid mode"));
    }
}
```

Run: `cargo test -p colmena_dag_engine --lib router::node::tests`

Expected: 3 tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/router/node.rs \
        src/libs/colmena/src/dag_engine/infrastructure/nodes/router/llm_direct.rs
git commit -m "feat(router): mode A (llm_direct) implementation

RouterNode::execute now wires mode A end-to-end: parses+validates config,
checks for empty input (fail-fast), resolves the LLM provider, calls
llm_direct::pick_branch (which uses extract_with_schema with a single-field
enum schema), and emits per-branch ports plus __decision. Mode B returns a
'not yet implemented' error and is filled in the next task. Subgraph
dispatch per branch is added in Task 11."
```

---

### Task 10: Mode B (`extract_and_route`)

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/router/extract_and_route.rs`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/router/node.rs`

- [ ] **Step 1: Implement mode B logic**

Replace `router/extract_and_route.rs`:

```rust
use serde_json::{json, Value};
use std::sync::Arc;

use super::config::RouterConfig;
use crate::dag_engine::domain::observer::ExecutionObserver;
use crate::dag_engine::infrastructure::nodes::util::extract_with_schema::{
    extract_with_schema, ExtractInput,
};
use crate::dag_engine::infrastructure::nodes::util::inline_schema::inline_to_json_schema;
use crate::llm::domain::ProviderKind;

const EXTRACTION_SYSTEM_MSG: &str =
    include_str!("../prompts/extraction_system.md");

/// Returns (branch_index, extracted_json) when a branch matches.
/// On no-match, returns an error that includes the extracted JSON for diagnostics.
pub async fn pick_branch(
    cfg: &RouterConfig,
    provider_kind: ProviderKind,
    api_key: String,
    model: Option<String>,
    user_text: String,
    observer: Option<Arc<dyn ExecutionObserver>>,
) -> Result<(usize, Value), Box<dyn std::error::Error + Send + Sync>> {
    let inline_schema = cfg
        .inline_schema
        .as_ref()
        .ok_or("Router(mode B): inline schema missing — config validation should have caught this")?;
    let json_schema = inline_to_json_schema(inline_schema)?;

    let instructions_section = match &cfg.instructions {
        Some(s) if !s.is_empty() => format!("\n\nContext/Rules for extraction:\n{}\n", s),
        _ => String::new(),
    };
    let system_message = EXTRACTION_SYSTEM_MSG
        .replace("{user_instructions}", &instructions_section)
        .replace("{schema}", &serde_json::to_string_pretty(&json_schema)?);

    let extracted = extract_with_schema(ExtractInput {
        provider_kind,
        api_key,
        model,
        system_message,
        user_text,
        inline_schema,
        temperature: Some(0.1),
        observer,
    })
    .await?;

    for (idx, b) in cfg.branches.iter().enumerate() {
        if let Some(rule) = &b.when {
            if rule.evaluate(&extracted) {
                return Ok((idx, extracted));
            }
        }
    }

    Err(format!(
        "RouterRuntimeError: no branch matched. extracted: {}",
        serde_json::to_string(&extracted).unwrap_or_default()
    )
    .into())
}

/// Returns the extracted JSON when extraction succeeded but no branch matched,
/// so the caller can emit it as part of __decision before failing.
pub fn extract_extracted_from_error(err: &(dyn std::error::Error + Send + Sync)) -> Option<Value> {
    let msg = err.to_string();
    let marker = "extracted: ";
    let idx = msg.find(marker)?;
    let json_str = &msg[idx + marker.len()..];
    serde_json::from_str(json_str).ok()
}
```

- [ ] **Step 2: Wire mode B into `RouterNode::execute`**

In `router/node.rs`, replace the `RouterMode::ExtractAndRoute => return Err(...)` arm with:

```rust
            RouterMode::ExtractAndRoute => {
                let result = super::extract_and_route::pick_branch(
                    &cfg,
                    provider_kind,
                    api_key,
                    model,
                    user_text,
                    observer.clone(),
                )
                .await;
                match result {
                    Ok((i, ex)) => (i, String::new(), Some(ex)),
                    Err(e) => {
                        // Emit __decision with whatever we know before failing.
                        // We don't currently have a way to attach __decision to
                        // an Err return through ExecutableNode, so we surface
                        // the failure as-is. The error message already carries
                        // the extracted JSON for downstream debuggers.
                        return Err(e);
                    }
                }
            }
```

(Note: `__decision` on failure was specified as nice-to-have in §6.3 of the spec. Engine-level support for emitting partial state on error is not in scope for this plan — surfacing the diagnostic info in the error message satisfies the debugging goal.)

- [ ] **Step 3: Add unit tests for mode B init delegation**

Append to `router/node.rs` tests mod:

```rust
    #[tokio::test]
    async fn extract_and_route_requires_schema_at_runtime() {
        let node = RouterNode;
        let mut state = json!({});
        let cfg = json!({
            "mode": "extract_and_route",
            "provider": "google",
            "api_key": "fake",
            "branches": [ { "name": "a", "when": { "field": "x", "equals": "y" } } ]
        });
        let err = node
            .execute(&inputs(json!("anything")), &cfg, &mut state, None)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("requires schema"));
    }
```

Run: `cargo test -p colmena_dag_engine --lib router::node::tests`

Expected: 4 tests pass (the 3 existing + the new one).

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/router/extract_and_route.rs \
        src/libs/colmena/src/dag_engine/infrastructure/nodes/router/node.rs
git commit -m "feat(router): mode B (extract_and_route)

Implements pick_branch for mode B: builds the extraction system message,
calls extract_with_schema, walks branches in declaration order and returns
the first matching one. No-match returns an error embedding the extracted
JSON so operators can diagnose the case. Wires mode B into
RouterNode::execute. __decision on routing-failure is surfaced via the
error message (the spec's optional behavior); engine-level partial-output
on error is out of scope here."
```

---

### Task 11: Subgraph dispatch per branch

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/router/node.rs`

The router needs the same `SubGraphExecutorPort` that `SubGraphNode` uses. The cleanest reuse: instantiate a `SubGraphNode` internally for each branch that declares a subgraph, sharing the parent's `OnceLock<Arc<dyn SubGraphExecutorPort>>`.

- [ ] **Step 1: Update `RouterNode` to hold a SubGraphExecutorPort**

In `router/node.rs`, replace `pub struct RouterNode;` with:

```rust
use crate::dag_engine::application::ports::SubGraphExecutorPort;
use std::sync::OnceLock;

pub struct RouterNode {
    pub executor: Arc<OnceLock<Arc<dyn SubGraphExecutorPort>>>,
}

impl Default for RouterNode {
    fn default() -> Self {
        Self::new()
    }
}

impl RouterNode {
    pub fn new() -> Self {
        Self {
            executor: Arc::new(OnceLock::new()),
        }
    }
}
```

Update the existing `impl RouterNode { fn resolve_env_var ... }` block to merge with the above (one `impl RouterNode` block).

- [ ] **Step 2: Dispatch the selected branch's subgraph when present**

In `RouterNode::execute`, **replace** the existing block:

```rust
        let payload = match &extracted {
            Some(e) => json!({ "input": input_raw, "extracted": e }),
            None => json!({ "input": input_raw }),
        };
```

with:

```rust
        // If the selected branch declares a subgraph, run it and use its
        // output as the payload. Otherwise the payload is the raw {input, extracted?}.
        let payload = match &selected.subgraph {
            Some(sg_config) => {
                use crate::dag_engine::infrastructure::nodes::subgraph::SubGraphNode;
                let sg_node = SubGraphNode {
                    executor: self.executor.clone(),
                };
                // Pass the payload we'd have emitted as the subgraph's inputs.
                let mut sg_inputs = NodeInputs::new();
                sg_inputs.insert("input".to_string(), input_raw.clone());
                if let Some(e) = &extracted {
                    sg_inputs.insert("extracted".to_string(), e.clone());
                }
                // Forward standard subgraph wiring keys from the router's inputs.
                for k in [
                    "__colmena_session_id",
                    "__colmena_agent_session_id",
                    "__colmena_node_id_path",
                    "__colmena_resume_answer",
                ] {
                    if let Some(v) = inputs.get(k) {
                        sg_inputs.insert(k.to_string(), v.clone());
                    }
                }
                let sg_result = sg_node
                    .execute(&sg_inputs, sg_config, _state, observer.clone())
                    .await
                    .map_err(|e| -> Box<dyn Error + Send + Sync> {
                        format!("router branch '{}': {}", selected.name, e).into()
                    })?;
                sg_result
            }
            None => match &extracted {
                Some(e) => json!({ "input": input_raw, "extracted": e }),
                None => json!({ "input": input_raw }),
            },
        };
```

- [ ] **Step 3: Add a subgraph-failure unit test**

Append to `router/node.rs` tests mod:

```rust
    #[tokio::test]
    async fn rejects_subgraph_with_both_path_and_inline() {
        // The validation runs inside parse_and_validate, but we verify the
        // router surfaces it at runtime too.
        let node = RouterNode::new();
        let cfg = json!({
            "mode": "llm_direct",
            "provider": "google",
            "api_key": "fake",
            "branches": [ {
                "name": "a",
                "description": "x",
                "subgraph": { "child_graph_path": "p.json", "child_graph_inline": {} }
            } ]
        });
        let mut state = json!({});
        let err = node
            .execute(&inputs(json!("anything")), &cfg, &mut state, None)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("pick one"));
    }
```

Run: `cargo test -p colmena_dag_engine --lib router::node::tests`

Expected: 5 tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/router/node.rs
git commit -m "feat(router): inline subgraph dispatch per branch

When the selected branch declares a subgraph, RouterNode instantiates
a SubGraphNode sharing its parent's SubGraphExecutorPort and forwards
the branch payload as the child graph's initial state. Subgraph errors
propagate with prefix 'router branch '<name>':'. SUSPENDED bubbles up
naturally (SubGraphNode's existing behavior). Wires RouterNode to hold
an Arc<OnceLock<SubGraphExecutorPort>> just like SubGraphNode."
```

---

## Phase 5 — Registry, integration tests, docs

### Task 12: Register `router` + wire executor + registry tests

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/registry.rs`

- [ ] **Step 1: Register the router**

Find the block where `subgraph` is registered (search for `"subgraph"` in `registry.rs`). After the subgraph registration, the executor is wired via something like `subgraph_node.executor.set(...)`. Add a parallel block for the router:

```rust
            // --- Registrar Router ---
            let router_node = crate::dag_engine::infrastructure::nodes::router::RouterNode::new();
            // Share the same SubGraphExecutorPort instance the SubGraphNode uses.
            // The OnceLock wrapper allows late binding by the engine.
            nodes.insert(
                "router".to_string(),
                Arc::new(crate::dag_engine::infrastructure::nodes::router::RouterNode {
                    executor: router_node.executor.clone(),
                }),
            );
```

**Important:** the engine's wiring code (search for `subgraph_node.executor.set` or similar in `src/libs/colmena/src/dag_engine/`) must also set the executor on the router instance. Find the wiring site and add the symmetric call. If the wiring goes through `NodeRegistryPort` lookup, no extra change is needed beyond registering the node — the `executor` OnceLock is keyed by the node instance, which is shared via Arc.

To find the wiring site:

Run: `grep -rn "executor.set\|SubGraphExecutorPort" src/libs/colmena/src/dag_engine/ | head -20`

Identify the engine init function that calls `subgraph.executor.set(...)`. Add a matching `router.executor.set(...)` next to it with the same `Arc<dyn SubGraphExecutorPort>`.

- [ ] **Step 2: Add registry tests**

After the existing `output_parser_registered_as_executable_node` test, add:

```rust
    #[test]
    fn router_registered_as_executable_node() {
        let registry = HashMapNodeRegistry::new(None, None);
        assert!(
            registry.get_node("router").is_some(),
            "router must be registered as an ExecutableNode"
        );
    }
```

Run: `cargo test -p colmena_dag_engine --lib router_registered`

Expected: PASS.

- [ ] **Step 3: Full unit test sweep**

Run: `cargo test -p colmena_dag_engine --lib router && cargo test -p colmena_dag_engine --lib output_parser && cargo test -p colmena_dag_engine --lib util::inline_schema && cargo test -p colmena_dag_engine --lib util::extract_with_schema && cargo test -p colmena_dag_engine --lib extraction`

Expected: every test passes.

- [ ] **Step 4: Lint + format**

Run: `cargo clippy -p colmena_dag_engine --all-targets -- -D warnings && cargo fmt --check`

Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/registry.rs
git commit -m "feat(registry): register router node and wire SubGraphExecutorPort

Router shares the same OnceLock<SubGraphExecutorPort> wiring path as
SubGraphNode so inline subgraphs per branch get the same executor. Adds a
registry test confirming router_registered_as_executable_node."
```

---

### Task 13: Integration test graphs

**Files:**
- Create: `tests/graphs/control_flow/router_llm_direct.json`
- Create: `tests/graphs/control_flow/router_extract_rules.json`
- Create: `tests/graphs/control_flow/router_with_subgraph.json`
- Create: `tests/graphs/control_flow/router_chained.json`

- [ ] **Step 1: Mode A end-to-end graph**

Create `tests/graphs/control_flow/router_llm_direct.json`:

```json
{
  "nodes": {
    "in": { "type": "input", "config": {} },
    "router": {
      "type": "router",
      "config": {
        "mode": "llm_direct",
        "provider": "google",
        "model": "gemini-2.5-flash",
        "api_key": "${GEMINI_API_KEY}",
        "branches": [
          { "name": "sales",   "description": "User wants to buy, asks for pricing, quotes, or available products." },
          { "name": "support", "description": "User has a technical issue or asks how to use something." },
          { "name": "billing", "description": "Invoices, payments, subscriptions, refunds." }
        ]
      }
    },
    "log_sales":   { "type": "log", "config": { "prefix": "SALES:" } },
    "log_support": { "type": "log", "config": { "prefix": "SUPPORT:" } },
    "log_billing": { "type": "log", "config": { "prefix": "BILLING:" } },
    "out": { "type": "output", "config": {} }
  },
  "edges": [
    { "from": "in.user_message", "to": "router.input" },
    { "from": "router.sales",   "to": "log_sales" },
    { "from": "router.support", "to": "log_support" },
    { "from": "router.billing", "to": "log_billing" },
    { "from": "router.__decision", "to": "out" }
  ]
}
```

- [ ] **Step 2: Mode B end-to-end graph**

Create `tests/graphs/control_flow/router_extract_rules.json`:

```json
{
  "nodes": {
    "in": { "type": "input", "config": {} },
    "router": {
      "type": "router",
      "config": {
        "mode": "extract_and_route",
        "provider": "google",
        "model": "gemini-2.5-flash",
        "api_key": "${GEMINI_API_KEY}",
        "schema": {
          "intent":     { "type": "string", "required": true,  "description": "sales | support | billing" },
          "urgency":    { "type": "string", "required": false, "description": "low | medium | high" },
          "confidence": { "type": "number", "required": false, "description": "0..1" }
        },
        "branches": [
          {
            "name": "urgent_sales",
            "when": { "all": [
              { "field": "intent",  "equals": "sales" },
              { "field": "urgency", "equals": "high"  }
            ]}
          },
          { "name": "sales",   "when": { "field": "intent", "equals": "sales" } },
          { "name": "support", "when": { "field": "intent", "in": ["support", "technical"] } },
          { "name": "billing", "when": { "field": "intent", "equals": "billing" } }
        ]
      }
    },
    "log_urgent":  { "type": "log", "config": { "prefix": "URGENT_SALES:" } },
    "log_sales":   { "type": "log", "config": { "prefix": "SALES:" } },
    "log_support": { "type": "log", "config": { "prefix": "SUPPORT:" } },
    "log_billing": { "type": "log", "config": { "prefix": "BILLING:" } },
    "out": { "type": "output", "config": {} }
  },
  "edges": [
    { "from": "in.user_message",      "to": "router.input" },
    { "from": "router.urgent_sales",  "to": "log_urgent" },
    { "from": "router.sales",         "to": "log_sales" },
    { "from": "router.support",       "to": "log_support" },
    { "from": "router.billing",       "to": "log_billing" },
    { "from": "router.__decision",    "to": "out" }
  ]
}
```

- [ ] **Step 3: Subgraph-per-branch graph**

Create `tests/graphs/control_flow/router_with_subgraph.json`:

```json
{
  "nodes": {
    "in": { "type": "input", "config": {} },
    "router": {
      "type": "router",
      "config": {
        "mode": "llm_direct",
        "provider": "google",
        "model": "gemini-2.5-flash",
        "api_key": "${GEMINI_API_KEY}",
        "branches": [
          {
            "name": "answerable",
            "description": "User asks a question that can be answered with general knowledge.",
            "subgraph": {
              "child_graph_inline": {
                "nodes": {
                  "sg_in":  { "type": "input", "config": {} },
                  "sg_llm": {
                    "type": "llm_call",
                    "config": {
                      "provider": "google",
                      "model": "gemini-2.5-flash",
                      "api_key": "${GEMINI_API_KEY}",
                      "prompt": "Answer concisely: {{input}}"
                    }
                  },
                  "sg_out": { "type": "output", "config": {} }
                },
                "edges": [
                  { "from": "sg_in.input", "to": "sg_llm.input" },
                  { "from": "sg_llm", "to": "sg_out" }
                ]
              }
            }
          },
          {
            "name": "escalate",
            "description": "User asks something that requires human follow-up."
          }
        ]
      }
    },
    "log_escalate": { "type": "log", "config": { "prefix": "ESCALATE:" } },
    "out": { "type": "output", "config": {} }
  },
  "edges": [
    { "from": "in.user_message", "to": "router.input" },
    { "from": "router.answerable", "to": "out" },
    { "from": "router.escalate",   "to": "log_escalate" }
  ]
}
```

- [ ] **Step 4: Chained-routers graph**

Create `tests/graphs/control_flow/router_chained.json`:

```json
{
  "nodes": {
    "in": { "type": "input", "config": {} },
    "intent_router": {
      "type": "router",
      "config": {
        "mode": "llm_direct",
        "provider": "google",
        "model": "gemini-2.5-flash",
        "api_key": "${GEMINI_API_KEY}",
        "branches": [
          { "name": "question",   "description": "User asks a question." },
          { "name": "command",    "description": "User issues a command (e.g., 'create', 'delete', 'update')." }
        ]
      }
    },
    "question_lang_router": {
      "type": "router",
      "config": {
        "mode": "extract_and_route",
        "provider": "google",
        "model": "gemini-2.5-flash",
        "api_key": "${GEMINI_API_KEY}",
        "schema": {
          "lang": { "type": "string", "required": true, "description": "ISO 639-1 like 'es' or 'en'" }
        },
        "branches": [
          { "name": "es", "when": { "field": "lang", "equals": "es" } },
          { "name": "en", "when": { "field": "lang", "equals": "en" } },
          { "name": "other", "when": { "field": "lang", "exists": true } }
        ]
      }
    },
    "log_command": { "type": "log", "config": { "prefix": "COMMAND:" } },
    "log_es":      { "type": "log", "config": { "prefix": "ES:" } },
    "log_en":      { "type": "log", "config": { "prefix": "EN:" } },
    "log_other":   { "type": "log", "config": { "prefix": "OTHER:" } },
    "out":         { "type": "output", "config": {} }
  },
  "edges": [
    { "from": "in.user_message",            "to": "intent_router.input" },
    { "from": "intent_router.command",      "to": "log_command" },
    { "from": "intent_router.question.input", "to": "question_lang_router.input" },
    { "from": "question_lang_router.es",    "to": "log_es" },
    { "from": "question_lang_router.en",    "to": "log_en" },
    { "from": "question_lang_router.other", "to": "log_other" },
    { "from": "question_lang_router.__decision", "to": "out" }
  ]
}
```

- [ ] **Step 5: Smoke-test each graph against a real provider**

Run each of the following with a representative input and verify behavior:

```bash
source .env
cargo run --bin dag_engine -- run tests/graphs/control_flow/router_llm_direct.json    --agent-session-id router_demo_a --include-extra-info
cargo run --bin dag_engine -- run tests/graphs/control_flow/router_extract_rules.json --agent-session-id router_demo_b --include-extra-info
cargo run --bin dag_engine -- run tests/graphs/control_flow/router_with_subgraph.json --agent-session-id router_demo_c --include-extra-info
cargo run --bin dag_engine -- run tests/graphs/control_flow/router_chained.json       --agent-session-id router_demo_d --include-extra-info
```

The default input is empty — supply one by editing the graph's `in` node config to embed a `user_message`, or pipe through `--include-extra-info` and use a test harness that injects the input.

Expected: each graph completes; the `__decision.selected_branch` matches the user's clear intent; only the matching branch's `log_*` fires (others are null).

- [ ] **Step 6: Commit**

```bash
git add tests/graphs/control_flow/
git commit -m "test(graphs/control_flow): integration graphs for router + output_parser

Adds four router integration graphs covering both modes, subgraph-per-branch
dispatch, and two-stage chained routing. Each graph is gated on
GEMINI_API_KEY at runtime via \${GEMINI_API_KEY}."
```

---

### Task 14: Documentation updates

**Files:**
- Modify: `docs/node_configurations.json`
- Modify: `docs/agent_context/node_ports_reference.md`
- Create: `docs/developer_guide/37_router_and_output_parser.md`
- Modify: `docs/DEVELOPER_GUIDE.md`
- Modify: the current rolling changelog under `docs/CHANGELOG_*.md`

- [ ] **Step 1: Add entries to `docs/node_configurations.json`**

Insert two new entries — `output_parser` (after `information_extraction`) and `router` (after `loop_controller`). Use the canonical schema format from the existing file:

For `output_parser` (extract from the spec §3.2 / §3.3):

```json
"output_parser": {
  "name": "Output Parser",
  "description": "Parses unstructured text (typically the output of an LLM or agent) into a structured JSON object matching the provided inline schema. Thin wrapper over the information_extraction engine designed to be chained right after an llm_call. Fails fast on missing input.",
  "category": "llm_ai",
  "config_fields": {
    "provider":    { "type": "string", "required": true,  "description": "openai | google | anthropic" },
    "api_key":     { "type": "string", "required": true,  "description": "API key, supports ${ENV_VAR}" },
    "model":       { "type": "string", "required": false, "description": "Model id (e.g., gemini-2.5-flash)" },
    "schema":      { "type": "object", "required": true,  "description": "Inline schema: { field: { type, required?, description? } }" },
    "instructions":{ "type": "string", "required": false, "description": "Appended to the built-in extraction system message" },
    "temperature": { "type": "number", "required": false, "default": 0.1 }
  },
  "input_ports":  { "input": { "type": "any", "description": "Text or value to parse; non-strings are serialized to JSON" } },
  "output_ports": { "output": { "type": "object", "description": "Extracted JSON matching schema" } },
  "default_input":  "input",
  "default_output": null,
  "requires": [],
  "supports_env_vars": true
}
```

For `router` (extract from spec §4):

```json
"router": {
  "name": "Router",
  "description": "Routes the input to one of N branches. Mode 'llm_direct' lets an LLM pick a branch by name from descriptions; mode 'extract_and_route' extracts a JSON object against a schema then evaluates declarative 'when' rules. Each branch is an output port; non-selected ports emit null. Optional inline subgraph per branch. Fails fast on no-match (no default).",
  "category": "control_flow",
  "config_fields": {
    "mode":         { "type": "string", "required": true,  "valid_values": ["llm_direct", "extract_and_route"] },
    "provider":     { "type": "string", "required": true },
    "api_key":      { "type": "string", "required": true,  "description": "API key, supports ${ENV_VAR}" },
    "model":        { "type": "string", "required": false },
    "schema":       { "type": "object", "required": "mode B", "description": "Inline schema for mode B" },
    "instructions": { "type": "string", "required": false },
    "branches":     { "type": "array",  "required": true,  "description": "Array of { name, description? (mode A), when? (mode B), subgraph? }" }
  },
  "input_ports":  { "input": { "type": "any", "description": "Text or value to route" } },
  "output_ports": {
    "<branch_name>": { "type": "object", "description": "Non-null only on the selected branch; { input, extracted? }" },
    "__decision":    { "type": "object", "description": "{ selected_branch, reason?, extracted? } — always emitted on success" }
  },
  "default_input":  "input",
  "default_output": null,
  "requires": [],
  "supports_env_vars": true
}
```

- [ ] **Step 2: Update `docs/agent_context/node_ports_reference.md`**

Add a section for each node listing its input/output ports per the spec §3.3 and §4.4.

- [ ] **Step 3: Create the developer guide**

Create `docs/developer_guide/37_router_and_output_parser.md` with sections:

```markdown
# 37. Router & Output Parser

> Shipped 2026-05-31. Spec: [docs/superpowers/specs/2026-05-31-router-and-output-parser-nodes-design.md](../superpowers/specs/2026-05-31-router-and-output-parser-nodes-design.md)

## Cuándo usar cada uno

- **`output_parser`**: cuando tenés la salida en texto de un `llm_call` o un agente y necesitás convertirla a JSON estructurado matching un schema. Wrapper liviano sobre el motor de `information_extraction`, pensado para encadenarlo directo después de un nodo (un solo input port).
- **`router`**: cuando necesitás bifurcar el flujo entre N ramas nombradas. Dos modos:
  - `llm_direct` — el LLM lee las descripciones de las ramas y elige una por nombre.
  - `extract_and_route` — el LLM extrae un JSON (como en `output_parser`); reglas declarativas sobre ese JSON eligen la rama.

## `output_parser` — ejemplo

(insertar el ejemplo del spec §3.2 con explicación)

## `router` — modo A (LLM-direct)

(insertar el ejemplo del spec §4.5 + explicación de cuándo conviene este modo)

## `router` — modo B (extract + rules)

(insertar el ejemplo del spec §4.6)

## DSL `when` — referencia

(reproducir la tabla del spec §4.7)

## Subgraphs por rama

(explicar el ejemplo del spec §4.9)

## Errores comunes

- "missing input — nothing to parse/route" → tu upstream produjo null, "", [] o {}. El nodo no se skipea silenciosamente (a diferencia de `llm_call`).
- "no branch matched. extracted: {...}" → ninguna regla `when` matcheó. Agregá una rama final que cubra los casos restantes (`when: { field: "intent", exists: true }`) o ajustá la lista.
- "llm picked unknown branch 'X'" → el LLM alucinó un nombre fuera del enum (raro con structured output). Revisá si las descripciones son demasiado ambiguas.

## Cuándo NO usarlos

- Si la decisión depende solo de un valor ya estructurado que viene de un nodo upstream (sin necesidad de LLM), un `python_node` con `output = item if condition else None` es más simple.
- Si necesitás múltiples ramas activas simultáneamente (no XOR), el router no aplica — usá edges independientes con `loop_status` o `python_node` por rama.
```

- [ ] **Step 4: Update `docs/DEVELOPER_GUIDE.md`**

Add the new guide to the index, in the appropriate ordered position (just below `36_*`).

- [ ] **Step 5: Add a changelog entry**

In the current rolling changelog file under `docs/CHANGELOG_*.md`, add a 2026-05-31 entry summarizing the two new nodes with a link to the spec and the guide.

- [ ] **Step 6: Commit**

```bash
git add docs/node_configurations.json docs/agent_context/node_ports_reference.md \
        docs/developer_guide/37_router_and_output_parser.md docs/DEVELOPER_GUIDE.md \
        docs/CHANGELOG_*.md
git commit -m "docs: router and output_parser nodes

Adds canonical entries in node_configurations.json, port semantics in
node_ports_reference.md, developer guide 37, index pointer, and changelog
entry for the two new nodes shipped today."
```

---

### Task 15: Final verification + push

- [ ] **Step 1: Full test suite (verbose, includes doctests)**

Run: `cargo test --verbose`

Expected: all tests pass.

- [ ] **Step 2: Clippy + fmt**

Run: `cargo clippy --all-targets -- -D warnings && cargo fmt --check`

Expected: clean.

- [ ] **Step 3: Optional — ignored integration tests against a real provider**

Run: `source .env && cargo test -- --ignored router && cargo test -- --ignored output_parser`

Expected: passes (skip this step if API keys are unavailable; CI doesn't run it).

- [ ] **Step 4: Sweep colmena consumers (ADP worker)**

Per CLAUDE.md "Breaking-change discipline": confirm no public API surface changed (`EngineConfig`, `ColmenaEngine`, exported trait signatures). The two new nodes are additive; no existing trait or struct signature was modified. The only modification to existing code is `extraction.rs`'s internal delegation. Verify by searching the ADP worker (`/Users/danielgarcia/startti/adp/apps/service/ia/platform/{worker,api}/src/`) for any direct reference to `ExtractionNode` or its inputs/outputs — none should require updating.

Run: `grep -rn "ExtractionNode\|output_parser\|RouterNode" /Users/danielgarcia/startti/adp/apps/service/ia/platform/ 2>/dev/null`

Expected: no hits (or only references to graph JSON `node_type` strings, which are runtime-resolved and don't need code changes).

- [ ] **Step 5: Final commit (if any docs/index changes were missed)**

Make sure the working tree is clean and all tasks are committed.

Run: `git status`

Expected: clean.

- [ ] **Step 6: Push the branch**

Push when ready. Per CLAUDE.md, do NOT force push, do NOT skip hooks, and do NOT push to main directly.

```bash
git push -u origin <current-branch>
```

---

## Summary of testing matrix

| Layer | Files | Coverage |
|---|---|---|
| Unit — `inline_schema` | `util/inline_schema.rs` | 10 tests (converter happy path + 4 rejections + validator happy + 4 rejections + null-handling) |
| Unit — `extract_with_schema` | `util/extract_with_schema.rs` | 6 tests (parse + validate, all paths) |
| Unit — `extraction.rs` regression | inline tests already in file | unchanged behavior |
| Unit — `output_parser` | `output_parser.rs` | 5 tests (empty-input × 4 + invalid-schema) |
| Unit — `router/config` | `router/config.rs` | 11 tests (init validation matrix) |
| Unit — `router/when_dsl` | `router/when_dsl.rs` | 16 tests (DSL operators + combinators + dotted paths + parse errors) |
| Unit — `router/node` | `router/node.rs` | 5 tests (empty input × 2 + config errors × 3) |
| Unit — registry | `registry.rs` | 2 new tests (output_parser_registered + router_registered) |
| Integration — control_flow graphs | `tests/graphs/control_flow/` | 5 graphs (basic + 4 router scenarios) — smoke-tested manually against Gemini |
| Lint | — | clippy clean + fmt check |
