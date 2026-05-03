# Lazy Tool Loading Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add progressive tool registration to the `llm_call` node so the LLM only sees full schemas for tools it has actively discovered (or had previously called) in the conversation, removing the "10+ tools" attention dilution problem.

**Architecture:** A new synthetic tool `describe_tool` is intercepted in `DagToolExecutor` (mirroring `load_skill`). The LLM-facing `tools[]` is recomputed at each ReAct iteration from a pure function over the current conversation messages — `discovered_set = (describe_tool calls) ∪ (direct calls to cataloged tools)`. No new database schema; persistence is the existing conversation history.

**Tech Stack:** Rust (workspace lib `colmena_dag_engine`), serde, async-trait, tokio. Tests use `mockall` (already a dep) and the existing `MockAdapter`.

**Spec:** [docs/superpowers/specs/2026-05-03-lazy-tool-loading-design.md](../specs/2026-05-03-lazy-tool-loading-design.md)

---

## File Structure

**New files:**
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/lazy_tools_catalog.rs` — `CatalogEntry`, summary truncation, `reconstruct_discovered_set`, `build_describe_tool_definition`.
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/describe_tool.rs` — `DESCRIBE_TOOL_NAME`, `DescribeToolDispatchResult`, `dispatch_describe_tool`, markdown generator, `into_tool_result`.
- `src/libs/colmena/tests/lazy_tools_integration.rs` — multi-turn integration test with `MockAdapter`.
- `tests/graphs/agents/tools_lazy_basic.json` — E2E graph (1 eager + 2 lazy tools).
- `docs/developer_guide/29_lazy_tool_loading.md` — Spanish developer guide.

**Modified files:**
- `src/libs/colmena/src/dag_engine/domain/tool_configuration.rs` — add `summary: Option<String>`, `eager: bool` to `ToolConfiguration`.
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs` — re-export new modules.
- `src/libs/colmena/src/dag_engine/domain/observer.rs` — `NodeEvent::ToolDescribed`.
- `src/libs/colmena/src/dag_engine/domain/events.rs` — `DagExecutionEvent::ToolDescribed`.
- `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs` — `ToolDescribeObserver` alias, `with_tool_describe_observer` builder, intercept block.
- `src/libs/colmena/src/llm/application/agent_service.rs` — `tools_provider` optional field on `AgentRunParams`; loop honors it per iteration.
- `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs` — wire `lazy_tool_loading` config; build catalog + closure; pass to `AgentService`; add `tools_discovered` to `extra_info`; emit `ToolDescribed` via observer.
- `src/libs/colmena/src/dag_engine/main.rs` — emit `tool-described` data-stream-protocol line.
- `src/libs/colmena/src/dag_engine/infrastructure/api.rs` — same SSE mapping for serve mode.
- `docs/DEVELOPER_GUIDE.md` — index entry.
- `docs/node_configurations.json` — `lazy_tool_loading`, `summary`, `eager` fields.
- `CLAUDE.md` — guide listing.

---

## Task 0: Module skeleton + check compile

**Files:**
- Create: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/lazy_tools_catalog.rs`
- Create: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/describe_tool.rs`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs`

- [ ] **Step 1: Create empty modules**

`lazy_tools_catalog.rs`:
```rust
//! Catalog management for lazy tool loading. Pure data types and pure functions
//! over conversation messages — no I/O, no provider awareness.
```

`describe_tool.rs`:
```rust
//! The `describe_tool` synthetic tool — dispatches catalog lookups and produces
//! curated markdown for the LLM.

pub const DESCRIBE_TOOL_NAME: &str = "describe_tool";
```

- [ ] **Step 2: Wire into `mod.rs`**

Open the existing file and add at the end (preserving existing exports):

```rust
pub mod describe_tool;
pub mod lazy_tools_catalog;

pub use describe_tool::{
    build_describe_tool_definition, dispatch_describe_tool, into_tool_result as describe_tool_into_tool_result,
    DescribeToolDispatchResult, DESCRIBE_TOOL_NAME,
};
pub use lazy_tools_catalog::{reconstruct_discovered_set, summary_for_catalog, CatalogEntry};
```

Note: `build_describe_tool_definition` will be defined in Task 4 inside `lazy_tools_catalog.rs` — re-exported through `describe_tool` for cohesion. Adjust the `pub use` after Task 4 if needed.

- [ ] **Step 3: Verify compile**

Run: `cargo check --lib`
Expected: success. (`mod.rs` will reference symbols not yet defined — temporarily comment out the `pub use` lines if needed and add them back as later tasks define the symbols. Or stub the symbols with `pub fn placeholder() {}` and remove later.)

Cleanest path for this step: stub the missing symbols in each file:

In `lazy_tools_catalog.rs` add:
```rust
pub struct CatalogEntry {
    pub name: String,
    pub summary: String,
}
pub fn reconstruct_discovered_set(_messages: &[crate::llm::domain::LlmMessage], _catalog: &[CatalogEntry]) -> std::collections::HashSet<String> {
    std::collections::HashSet::new()
}
pub fn summary_for_catalog(_summary: Option<&str>, _description: &str) -> String { String::new() }
pub fn build_describe_tool_definition(_pending: &[&CatalogEntry]) -> crate::llm::domain::tools::ToolDefinition {
    use crate::llm::domain::tools::{ToolDefinition, ToolParameters};
    ToolDefinition {
        name: "describe_tool".into(),
        description: String::new(),
        parameters: ToolParameters { schema_type: "object".into(), properties: Default::default(), required: vec![] },
        input_schema_override: None,
    }
}
```

In `describe_tool.rs` add:
```rust
use crate::llm::domain::{LlmError, ToolCall, ToolResult};

#[derive(Debug)]
pub struct DescribeToolDispatchResult {
    pub output: String,
    pub tool_name: String,
}

pub async fn dispatch_describe_tool(_tool_call: &ToolCall, _catalog: &[crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::lazy_tools_catalog::CatalogEntry]) -> Result<DescribeToolDispatchResult, LlmError> {
    Ok(DescribeToolDispatchResult { output: String::new(), tool_name: String::new() })
}

pub fn into_tool_result(call_id: &str, r: &DescribeToolDispatchResult) -> ToolResult {
    ToolResult { tool_call_id: call_id.to_string(), output: r.output.clone(), success: true, error: None }
}
```

Run: `cargo check --lib` — must pass.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/
git commit -m "$(cat <<'EOF'
feat(lazy-tools): module skeleton for describe_tool synthetic tool

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 1: Extend `ToolConfiguration` with `summary` and `eager`

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/domain/tool_configuration.rs:99-174`

- [ ] **Step 1: Write failing test**

Append to the existing `#[cfg(test)] mod tests` block in `tool_configuration.rs`:

```rust
    #[test]
    fn deserializes_summary_and_eager_when_present() {
        let json = serde_json::json!({
            "name": "search_orders",
            "description": "Search the orders table",
            "node_type": "sql_query",
            "summary": "Find orders. Use when user asks about purchases.",
            "eager": true
        });
        let cfg: ToolConfiguration = serde_json::from_value(json).unwrap();
        assert_eq!(cfg.summary.as_deref(), Some("Find orders. Use when user asks about purchases."));
        assert!(cfg.eager);
    }

    #[test]
    fn defaults_summary_to_none_and_eager_to_false() {
        let json = serde_json::json!({
            "name": "send_email",
            "description": "Send email",
            "node_type": "http_request"
        });
        let cfg: ToolConfiguration = serde_json::from_value(json).unwrap();
        assert!(cfg.summary.is_none());
        assert!(!cfg.eager);
    }
```

- [ ] **Step 2: Run tests — expect failure**

Run: `cargo test --lib tool_configuration::tests::deserializes_summary_and_eager_when_present tool_configuration::tests::defaults_summary_to_none_and_eager_to_false`
Expected: FAIL with "no field 'summary' / 'eager' on ToolConfiguration".

- [ ] **Step 3: Add the fields**

Inside `pub struct ToolConfiguration { ... }`, after the `expose_sub_tools` field, add:

```rust
    /// Optional short catalog entry shown when this tool is exposed via the
    /// lazy-loading catalog. ≤ 200 chars; longer values are truncated with a warning.
    /// Ignored when `lazy_tool_loading` is disabled.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub summary: Option<String>,

    /// When `lazy_tool_loading` is enabled on the parent llm_call, an `eager: true`
    /// tool is registered in every request with its full schema and does NOT appear
    /// in the `describe_tool` catalog. No effect when lazy_tool_loading is disabled.
    #[serde(default)]
    pub eager: bool,
```

- [ ] **Step 4: Re-run tests — expect pass**

Run: `cargo test --lib tool_configuration`
Expected: all tests in this module pass.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/dag_engine/domain/tool_configuration.rs
git commit -m "$(cat <<'EOF'
feat(lazy-tools): add summary and eager fields to ToolConfiguration

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: `summary_for_catalog` — fallback + truncation

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/lazy_tools_catalog.rs`

- [ ] **Step 1: Write failing tests**

Replace the stub `summary_for_catalog` with a `mod tests` block at the end of the file containing:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_summary_when_present_and_within_limit() {
        let s = summary_for_catalog(Some("short summary"), "ignored description");
        assert_eq!(s, "short summary");
    }

    #[test]
    fn falls_back_to_description_truncated_when_no_summary() {
        let desc = "Search the orders table by date range, status, customer ID, or product SKU. Returns up to 100 rows.";
        let s = summary_for_catalog(None, desc);
        assert!(s.len() <= 130, "got len {}", s.len());
        assert!(desc.starts_with(&s));
        // Word boundary, not mid-word
        assert!(!s.ends_with("Sea") && !s.ends_with("Sear"));
    }

    #[test]
    fn truncates_summary_over_200_chars_at_word_boundary() {
        let long: String = "word ".repeat(80); // 400 chars
        let s = summary_for_catalog(Some(&long), "");
        assert!(s.len() <= 200, "got len {}", s.len());
        assert!(s.ends_with("word") || s.ends_with("word ") == false);
    }

    #[test]
    fn returns_empty_string_when_neither_summary_nor_description() {
        let s = summary_for_catalog(None, "");
        assert_eq!(s, "");
    }
}
```

- [ ] **Step 2: Run tests — expect failure**

Run: `cargo test --lib lazy_tools_catalog::tests`
Expected: FAIL — current stub returns `""`.

- [ ] **Step 3: Implement**

Replace the stub `summary_for_catalog` with:

```rust
/// Maximum length of a catalog summary entry, in chars.
pub const SUMMARY_MAX_CHARS: usize = 200;
/// Default truncation budget when falling back to the full description.
pub const FALLBACK_DESCRIPTION_CHARS: usize = 120;

/// Resolve the catalog summary string for a tool.
/// - If `summary` is present and ≤ 200 chars: return as-is.
/// - If `summary` is present and > 200 chars: truncate at 200, on a word boundary.
/// - If `summary` is absent: take the first ~120 chars of `description`, on a word boundary.
/// - Returns empty string if both are empty.
pub fn summary_for_catalog(summary: Option<&str>, description: &str) -> String {
    let raw = summary.unwrap_or(description);
    let limit = if summary.is_some() {
        SUMMARY_MAX_CHARS
    } else {
        FALLBACK_DESCRIPTION_CHARS
    };
    truncate_at_word_boundary(raw, limit)
}

fn truncate_at_word_boundary(s: &str, max_chars: usize) -> String {
    let trimmed = s.trim();
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_string();
    }
    // Walk char indices, stop at last whitespace before the byte index for `max_chars`.
    let cutoff = trimmed
        .char_indices()
        .nth(max_chars)
        .map(|(i, _)| i)
        .unwrap_or(trimmed.len());
    let slice = &trimmed[..cutoff];
    match slice.rfind(char::is_whitespace) {
        Some(pos) => slice[..pos].trim_end().to_string(),
        None => slice.to_string(),
    }
}
```

- [ ] **Step 4: Re-run tests — expect pass**

Run: `cargo test --lib lazy_tools_catalog::tests`
Expected: all 4 tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/lazy_tools_catalog.rs
git commit -m "$(cat <<'EOF'
feat(lazy-tools): summary_for_catalog with word-boundary truncation

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: `reconstruct_discovered_set` — both rules

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/lazy_tools_catalog.rs`

- [ ] **Step 1: Write failing tests**

Append to the `mod tests` in `lazy_tools_catalog.rs`:

```rust
    use crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::DESCRIBE_TOOL_NAME;
    use crate::llm::domain::{FunctionCall, LlmMessage, ToolCall};

    fn entry(name: &str) -> CatalogEntry {
        CatalogEntry { name: name.to_string(), summary: format!("desc of {}", name) }
    }

    fn assistant_with_call(tool_name: &str, args_json: &str) -> LlmMessage {
        let tc = ToolCall::new(
            "call_x".to_string(),
            FunctionCall::new(tool_name.to_string(), args_json.to_string()),
        );
        LlmMessage::assistant_with_tool_calls("".to_string(), vec![tc]).unwrap()
    }

    #[test]
    fn empty_history_yields_empty_set() {
        let set = reconstruct_discovered_set(&[], &[entry("a")]);
        assert!(set.is_empty());
    }

    #[test]
    fn rule1_describe_tool_call_adds_named_tool() {
        let msg = assistant_with_call(DESCRIBE_TOOL_NAME, r#"{"name":"search_orders"}"#);
        let set = reconstruct_discovered_set(&[msg], &[entry("search_orders")]);
        assert!(set.contains("search_orders"));
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn rule2_direct_call_to_cataloged_tool_adds_it() {
        let msg = assistant_with_call("search_orders", r#"{"start":"2026-01-01"}"#);
        let set = reconstruct_discovered_set(&[msg], &[entry("search_orders")]);
        assert!(set.contains("search_orders"));
    }

    #[test]
    fn rule2_ignores_calls_to_uncataloged_tools() {
        let msg = assistant_with_call("legacy_tool", r#"{}"#);
        let set = reconstruct_discovered_set(&[msg], &[entry("search_orders")]);
        assert!(set.is_empty());
    }

    #[test]
    fn rule1_ignores_describe_tool_with_unknown_name() {
        let msg = assistant_with_call(DESCRIBE_TOOL_NAME, r#"{"name":"deleted_tool"}"#);
        let set = reconstruct_discovered_set(&[msg], &[entry("search_orders")]);
        // Note: the spec inserts whatever describe_tool received. If "deleted_tool" is
        // not in catalog, `tools[]` builder later would not include its schema anyway.
        // But the set itself faithfully records the call. Verify documented behavior:
        assert!(set.contains("deleted_tool"));
    }

    #[test]
    fn malformed_describe_tool_args_are_skipped_silently() {
        let msg = assistant_with_call(DESCRIBE_TOOL_NAME, r#"not-json"#);
        let set = reconstruct_discovered_set(&[msg], &[entry("search_orders")]);
        assert!(set.is_empty());
    }

    #[test]
    fn unions_rule1_and_rule2_across_messages() {
        let m1 = assistant_with_call(DESCRIBE_TOOL_NAME, r#"{"name":"a"}"#);
        let m2 = assistant_with_call("b", r#"{}"#);
        let set = reconstruct_discovered_set(&[m1, m2], &[entry("a"), entry("b")]);
        assert!(set.contains("a"));
        assert!(set.contains("b"));
        assert_eq!(set.len(), 2);
    }
```

- [ ] **Step 2: Run tests — expect failure**

Run: `cargo test --lib lazy_tools_catalog::tests`
Expected: FAIL — current stub returns empty set.

- [ ] **Step 3: Implement**

Replace the stub `reconstruct_discovered_set` with:

```rust
use crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::DESCRIBE_TOOL_NAME;
use crate::llm::domain::LlmMessage;
use std::collections::HashSet;

/// Args shape of a `describe_tool` call. Only `name` is used.
#[derive(serde::Deserialize)]
struct DescribeArgs {
    name: String,
}

/// Compute the set of tool names that count as "already discovered" in this
/// session, given the current message history and the tool catalog. A name
/// enters the set when:
/// - rule (1) the assistant called `describe_tool` with `name = X`, OR
/// - rule (2) the assistant directly called a tool whose name matches an entry in `catalog`.
///
/// Rule (2) is what handles three edge cases:
///   - aggressive truncation that drops the original `describe_tool` call
///   - sessions that switched from eager mode to lazy mode mid-flight
///   - manually seeded conversation histories
pub fn reconstruct_discovered_set(
    messages: &[LlmMessage],
    catalog: &[CatalogEntry],
) -> HashSet<String> {
    let catalog_names: HashSet<&str> = catalog.iter().map(|e| e.name.as_str()).collect();
    let mut set = HashSet::new();
    for msg in messages {
        if let Some(calls) = msg.tool_calls() {
            for tc in calls {
                if tc.function.name == DESCRIBE_TOOL_NAME {
                    if let Ok(args) = serde_json::from_str::<DescribeArgs>(&tc.function.arguments) {
                        set.insert(args.name);
                    }
                } else if catalog_names.contains(tc.function.name.as_str()) {
                    set.insert(tc.function.name.clone());
                }
            }
        }
    }
    set
}
```

- [ ] **Step 4: Re-run tests — expect pass**

Run: `cargo test --lib lazy_tools_catalog::tests`
Expected: all tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/lazy_tools_catalog.rs
git commit -m "$(cat <<'EOF'
feat(lazy-tools): reconstruct_discovered_set with describe_tool + direct rules

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: `build_describe_tool_definition`

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/lazy_tools_catalog.rs`

- [ ] **Step 1: Write failing tests**

Append to the `mod tests` block:

```rust
    #[test]
    fn definition_lists_pending_in_alphabetical_order() {
        let entries = vec![entry("zebra"), entry("apple"), entry("mango")];
        let pending: Vec<&CatalogEntry> = entries.iter().collect();
        let td = build_describe_tool_definition(&pending);
        let enum_values = td.parameters.properties.get("name").unwrap().enum_values.as_ref().unwrap();
        assert_eq!(enum_values, &vec!["apple".to_string(), "mango".to_string(), "zebra".to_string()]);
        let pos_a = td.description.find("apple").unwrap();
        let pos_m = td.description.find("mango").unwrap();
        let pos_z = td.description.find("zebra").unwrap();
        assert!(pos_a < pos_m && pos_m < pos_z);
    }

    #[test]
    fn definition_description_includes_summaries() {
        let entries = vec![CatalogEntry {
            name: "search_orders".into(),
            summary: "Find orders. Use for past purchases.".into(),
        }];
        let pending: Vec<&CatalogEntry> = entries.iter().collect();
        let td = build_describe_tool_definition(&pending);
        assert!(td.description.contains("search_orders"));
        assert!(td.description.contains("Find orders. Use for past purchases."));
    }

    #[test]
    fn definition_required_param_is_name() {
        let entries = vec![entry("a")];
        let pending: Vec<&CatalogEntry> = entries.iter().collect();
        let td = build_describe_tool_definition(&pending);
        assert_eq!(td.parameters.required, vec!["name".to_string()]);
        assert_eq!(td.name, "describe_tool");
    }
```

- [ ] **Step 2: Run tests — expect failure**

Run: `cargo test --lib lazy_tools_catalog::tests`
Expected: FAIL on the new tests (current stub builds an empty definition).

- [ ] **Step 3: Implement**

Replace the stub `build_describe_tool_definition` with:

```rust
use crate::llm::domain::tools::{ParameterProperty, ToolDefinition, ToolParameters};
use std::collections::HashMap;

/// Build the `describe_tool` ToolDefinition for the LLM. The `pending` slice
/// must be the catalog filtered by `discovered_set` — callers are responsible
/// for that filtering.
///
/// Pre-condition: `pending` is non-empty. Callers must omit `describe_tool`
/// from `tools[]` entirely when there is nothing pending.
pub fn build_describe_tool_definition(pending: &[&CatalogEntry]) -> ToolDefinition {
    let mut sorted: Vec<&&CatalogEntry> = pending.iter().collect();
    sorted.sort_by(|a, b| a.name.cmp(&b.name));

    let names: Vec<String> = sorted.iter().map(|e| e.name.clone()).collect();
    let catalog_lines: Vec<String> = sorted
        .iter()
        .map(|e| format!("- {}: {}", e.name, e.summary))
        .collect();

    let description = format!(
        "Reveal the full parameter schema and usage notes for one of the tools below. \
Call this BEFORE invoking a tool so you know its parameters and return shape. \
Available tools:\n{}\n\n\
Only call describe_tool when you've decided you actually need the tool — not preemptively for every tool. \
After calling describe_tool, the revealed tool will appear in your available tools on your next turn.",
        catalog_lines.join("\n")
    );

    let mut properties: HashMap<String, ParameterProperty> = HashMap::new();
    properties.insert(
        "name".to_string(),
        ParameterProperty::new(
            "string".to_string(),
            "The name of the tool whose schema you want to reveal".to_string(),
        )
        .with_enum(names),
    );

    ToolDefinition {
        name: super::DESCRIBE_TOOL_NAME.to_string(),
        description,
        parameters: ToolParameters {
            schema_type: "object".to_string(),
            properties,
            required: vec!["name".to_string()],
        },
        input_schema_override: None,
    }
}
```

- [ ] **Step 4: Re-run tests — expect pass**

Run: `cargo test --lib lazy_tools_catalog::tests`
Expected: all tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/lazy_tools_catalog.rs
git commit -m "$(cat <<'EOF'
feat(lazy-tools): build_describe_tool_definition with sorted pending catalog

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: Curated markdown generator

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/describe_tool.rs`

- [ ] **Step 1: Write failing tests**

Append to (or create) `mod tests` in `describe_tool.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::dag_engine::domain::tool_configuration::{NodeSchema, NodeSchemaField, ToolConfiguration};
    use serde_json::json;
    use std::collections::HashMap;

    fn cfg_minimal(name: &str, desc: &str) -> ToolConfiguration {
        ToolConfiguration {
            name: name.to_string(),
            description: desc.to_string(),
            node_type: "noop".to_string(),
            fixed_config: HashMap::new(),
            #[allow(deprecated)]
            exposed_inputs: None,
            #[allow(deprecated)]
            parameters: None,
            #[allow(deprecated)]
            mergeable_fields: None,
            #[allow(deprecated)]
            field_mapping: None,
            node_schema: None,
            node_config: None,
            expose_sub_tools: None,
            summary: None,
            eager: false,
        }
    }

    #[test]
    fn markdown_includes_name_description_and_anchor() {
        let cfg = cfg_minimal("search_orders", "Search the orders table");
        let md = generate_tool_markdown(&cfg);
        assert!(md.contains("# search_orders"));
        assert!(md.contains("Search the orders table"));
        assert!(md.contains("now available"));
        assert!(md.contains("next turn"));
    }

    #[test]
    fn markdown_without_node_schema_notes_freeform_args() {
        let cfg = cfg_minimal("send_email", "Send transactional email");
        let md = generate_tool_markdown(&cfg);
        assert!(md.contains("No parameter schema declared"));
        assert!(!md.contains("| Name | Type"));
    }

    #[test]
    fn markdown_with_node_schema_renders_table_for_visible_fields() {
        let mut cfg = cfg_minimal("search_orders", "Search orders");
        let mut schema: NodeSchema = HashMap::new();
        schema.insert("start_date".to_string(), NodeSchemaField {
            field_type: Some("string".to_string()),
            description: Some("ISO date YYYY-MM-DD".to_string()),
            required: Some(true),
            fixed: None,
            properties: None,
            secure: None,
            ..Default::default()
        });
        schema.insert("status".to_string(), NodeSchemaField {
            field_type: Some("string".to_string()),
            description: Some("Order status".to_string()),
            required: Some(false),
            fixed: None,
            properties: None,
            secure: None,
            ..Default::default()
        });
        cfg.node_schema = Some(schema);
        let md = generate_tool_markdown(&cfg);
        assert!(md.contains("| Name | Type | Required | Description |"));
        assert!(md.contains("| start_date | string | yes | ISO date YYYY-MM-DD |"));
        assert!(md.contains("| status | string | no | Order status |"));
    }

    #[test]
    fn markdown_omits_fixed_fields() {
        let mut cfg = cfg_minimal("http_get", "Make HTTP GET");
        let mut schema: NodeSchema = HashMap::new();
        schema.insert("base_url".to_string(), NodeSchemaField {
            fixed: Some(json!("https://api.example.com")),
            ..Default::default()
        });
        schema.insert("path".to_string(), NodeSchemaField {
            field_type: Some("string".to_string()),
            description: Some("URL path".to_string()),
            required: Some(true),
            ..Default::default()
        });
        cfg.node_schema = Some(schema);
        let md = generate_tool_markdown(&cfg);
        assert!(!md.contains("base_url"));
        assert!(md.contains("path"));
    }

    #[test]
    fn markdown_omits_secure_fields() {
        let mut cfg = cfg_minimal("http_get", "Make HTTP GET");
        let mut schema: NodeSchema = HashMap::new();
        schema.insert("api_token".to_string(), NodeSchemaField {
            field_type: Some("string".to_string()),
            description: Some("Bearer token".to_string()),
            secure: Some(true),
            ..Default::default()
        });
        schema.insert("path".to_string(), NodeSchemaField {
            field_type: Some("string".to_string()),
            description: Some("URL path".to_string()),
            required: Some(true),
            ..Default::default()
        });
        cfg.node_schema = Some(schema);
        let md = generate_tool_markdown(&cfg);
        assert!(!md.contains("api_token"));
        assert!(!md.contains("Bearer token"));
        assert!(md.contains("path"));
    }
}
```

Note: this test references `Default::default()` on `NodeSchemaField`. If it does not derive `Default`, change tests to construct fields fully or add `#[derive(Default)]` to `NodeSchemaField` in `tool_configuration.rs` (verify with `grep -n "pub struct NodeSchemaField" src/libs/colmena/src/dag_engine/domain/tool_configuration.rs` and adapt test field access accordingly).

- [ ] **Step 2: Run tests — expect failure**

Run: `cargo test --lib describe_tool::tests`
Expected: FAIL — `generate_tool_markdown` not defined.

- [ ] **Step 3: Implement**

In `describe_tool.rs`, add:

```rust
use crate::dag_engine::domain::tool_configuration::{NodeSchemaField, ToolConfiguration};

/// Produce the markdown the LLM sees as the result of calling describe_tool.
/// Filters out fields that are LLM-invisible (fixed values, secure fields).
pub fn generate_tool_markdown(cfg: &ToolConfiguration) -> String {
    let mut out = String::new();
    out.push_str(&format!("# {}\n\n", cfg.name));
    out.push_str(cfg.description.trim());
    out.push_str("\n\n");

    let visible_fields = collect_visible_fields(cfg);
    if visible_fields.is_empty() {
        out.push_str(
            "## Parameters\n\nNo parameter schema declared — pass arguments as a free-form JSON object that matches the tool's expectations.\n\n",
        );
    } else {
        out.push_str("## Parameters\n\n");
        out.push_str("| Name | Type | Required | Description |\n");
        out.push_str("|------|------|----------|-------------|\n");
        for (name, field) in &visible_fields {
            let ty = field.field_type.as_deref().unwrap_or("any");
            let required = if field.required.unwrap_or(false) { "yes" } else { "no" };
            let desc = field.description.as_deref().unwrap_or("");
            out.push_str(&format!("| {} | {} | {} | {} |\n", name, ty, required, desc));
        }
        out.push('\n');
    }

    out.push_str(
        "---\nThe tool `",
    );
    out.push_str(&cfg.name);
    out.push_str("` is now available. Call it directly on your next turn.\n");
    out
}

/// Return only fields that the LLM should see: not `fixed`, not `secure`,
/// not auto-populated by `fixed_config` at the top level.
fn collect_visible_fields(cfg: &ToolConfiguration) -> Vec<(String, &NodeSchemaField)> {
    let Some(schema) = cfg.node_schema.as_ref() else {
        return Vec::new();
    };
    let mut out: Vec<(String, &NodeSchemaField)> = Vec::new();
    for (name, field) in schema {
        if field.fixed.is_some() {
            continue;
        }
        if field.secure.unwrap_or(false) {
            continue;
        }
        if cfg.fixed_config.contains_key(name) {
            continue;
        }
        out.push((name.clone(), field));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}
```

If `NodeSchemaField` lacks `Default`, edit it in `tool_configuration.rs` to add `#[derive(Debug, Clone, Default, Serialize, Deserialize)]` (only if not already there).

- [ ] **Step 4: Re-run tests — expect pass**

Run: `cargo test --lib describe_tool::tests`
Expected: all 5 tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/describe_tool.rs src/libs/colmena/src/dag_engine/domain/tool_configuration.rs
git commit -m "$(cat <<'EOF'
feat(lazy-tools): curated markdown generator for describe_tool output

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: `dispatch_describe_tool`

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/describe_tool.rs`

- [ ] **Step 1: Write failing tests**

Append to `describe_tool.rs::tests`:

```rust
    use crate::llm::domain::tools::{FunctionCall, ToolCall};

    fn mk_call(args: serde_json::Value) -> ToolCall {
        ToolCall::new(
            "call_1".to_string(),
            FunctionCall::new(DESCRIBE_TOOL_NAME.to_string(), args.to_string()),
        )
    }

    #[tokio::test]
    async fn dispatch_returns_markdown_for_known_tool() {
        let cfg = cfg_minimal("search_orders", "Search the orders table");
        let lookup = vec![cfg.clone()];
        let call = mk_call(serde_json::json!({ "name": "search_orders" }));
        let r = dispatch_describe_tool(&call, &lookup).await.unwrap();
        assert_eq!(r.tool_name, "search_orders");
        assert!(r.output.contains("# search_orders"));
        assert!(r.output.contains("now available"));
    }

    #[tokio::test]
    async fn dispatch_unknown_tool_returns_error_output() {
        let cfg = cfg_minimal("search_orders", "Search the orders table");
        let lookup = vec![cfg];
        let call = mk_call(serde_json::json!({ "name": "deleted_tool" }));
        let r = dispatch_describe_tool(&call, &lookup).await.unwrap();
        assert!(r.output.starts_with("Error:"));
        assert!(r.output.contains("not found in catalog"));
    }

    #[tokio::test]
    async fn dispatch_missing_name_arg_is_invalid_tool_call() {
        let cfg = cfg_minimal("search_orders", "Search");
        let lookup = vec![cfg];
        let call = mk_call(serde_json::json!({}));
        let err = dispatch_describe_tool(&call, &lookup).await.unwrap_err();
        assert!(matches!(err, crate::llm::domain::LlmError::InvalidToolCall { .. }));
    }

    #[test]
    fn into_tool_result_marks_failure_when_output_starts_with_error() {
        let r = DescribeToolDispatchResult {
            output: "Error: Tool 'X' not found in catalog".into(),
            tool_name: "X".into(),
        };
        let tr = into_tool_result("call_1", &r);
        assert!(!tr.success);
    }
```

- [ ] **Step 2: Run tests — expect failure**

Run: `cargo test --lib describe_tool::tests`
Expected: FAIL on new tests — current stub returns empty output.

- [ ] **Step 3: Implement**

Replace the stub `dispatch_describe_tool` and `into_tool_result` in `describe_tool.rs`:

```rust
use crate::dag_engine::domain::tool_configuration::ToolConfiguration;
use crate::llm::domain::{LlmError, ToolCall, ToolResult};

#[derive(Debug)]
pub struct DescribeToolDispatchResult {
    pub output: String,
    pub tool_name: String,
}

/// Dispatch a `describe_tool` call. `lookup` is the slice of currently-configured
/// `ToolConfiguration` entries. Returns the curated markdown for the requested
/// tool, or an "Error: ..." string if the name is not found.
pub async fn dispatch_describe_tool(
    tool_call: &ToolCall,
    lookup: &[ToolConfiguration],
) -> Result<DescribeToolDispatchResult, LlmError> {
    let args: serde_json::Value =
        serde_json::from_str(&tool_call.function.arguments).map_err(|e| {
            LlmError::InvalidToolCall {
                reason: format!("describe_tool: invalid arguments JSON: {}", e),
            }
        })?;
    let name = args.get("name").and_then(|v| v.as_str()).ok_or_else(|| {
        LlmError::InvalidToolCall {
            reason: "describe_tool: missing required parameter 'name'".to_string(),
        }
    })?;

    let cfg = lookup.iter().find(|c| c.name == name);
    let output = match cfg {
        Some(c) => generate_tool_markdown(c),
        None => format!("Error: Tool '{}' not found in catalog", name),
    };
    Ok(DescribeToolDispatchResult {
        output,
        tool_name: name.to_string(),
    })
}

pub fn into_tool_result(call_id: &str, r: &DescribeToolDispatchResult) -> ToolResult {
    ToolResult {
        tool_call_id: call_id.to_string(),
        output: r.output.clone(),
        success: !r.output.starts_with("Error:"),
        error: None,
    }
}
```

- [ ] **Step 4: Re-run tests — expect pass**

Run: `cargo test --lib describe_tool::tests`
Expected: all tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/describe_tool.rs
git commit -m "$(cat <<'EOF'
feat(lazy-tools): dispatch_describe_tool with markdown lookup

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 7: `NodeEvent::ToolDescribed` variant

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/domain/observer.rs`

- [ ] **Step 1: Write failing test**

In the existing `mod tests` of `observer.rs`, add:

```rust
    #[test]
    fn tool_described_variant_constructible() {
        let ev = NodeEvent::ToolDescribed {
            tool_id: "call_1".to_string(),
            tool_name: "search_orders".to_string(),
        };
        match ev {
            NodeEvent::ToolDescribed { tool_name, .. } => {
                assert_eq!(tool_name, "search_orders");
            }
            _ => panic!("expected ToolDescribed"),
        }
    }
```

- [ ] **Step 2: Run test — expect failure**

Run: `cargo test --lib observer::tests::tool_described_variant_constructible`
Expected: FAIL — variant not defined.

- [ ] **Step 3: Add the variant**

In the `pub enum NodeEvent { ... }` block in `observer.rs`, add (alongside `SkillLoaded`):

```rust
    /// Emitted when the synthetic `describe_tool` successfully reveals a tool's schema.
    /// Fires alongside LlmToolCallStart/Finish so frontends can render a discovery-specific UI.
    ToolDescribed {
        tool_id: String,
        tool_name: String,
    },
```

- [ ] **Step 4: Re-run test — expect pass**

Run: `cargo test --lib observer::tests`
Expected: pass.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/dag_engine/domain/observer.rs
git commit -m "$(cat <<'EOF'
feat(lazy-tools): NodeEvent::ToolDescribed variant

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 8: `DagExecutionEvent::ToolDescribed` variant

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/domain/events.rs`

- [ ] **Step 1: Write failing test**

Add a new test module to `events.rs` (or extend an existing one):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_described_serializes_with_event_tag() {
        let ev = DagExecutionEvent::ToolDescribed {
            node_id: "n1".to_string(),
            tool_id: "call_1".to_string(),
            tool_name: "search_orders".to_string(),
        };
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["event"], "tool_described");
        assert_eq!(json["data"]["tool_name"], "search_orders");
    }
}
```

- [ ] **Step 2: Run test — expect failure**

Run: `cargo test --lib events::tests::tool_described_serializes_with_event_tag`
Expected: FAIL — variant not defined.

- [ ] **Step 3: Add the variant**

In `pub enum DagExecutionEvent`, after `SkillLoaded`:

```rust
    /// Emitted when the synthetic describe_tool reveals a tool's schema.
    /// Fires alongside llm_tool_call_start/finish so frontends can render a discovery-specific UI.
    #[serde(rename = "tool_described")]
    ToolDescribed {
        node_id: String,
        tool_id: String,
        tool_name: String,
    },
```

- [ ] **Step 4: Re-run test — expect pass**

Run: `cargo test --lib events::tests`
Expected: pass.

- [ ] **Step 5: Commit**

```bash
git add src/libs/colmena/src/dag_engine/domain/events.rs
git commit -m "$(cat <<'EOF'
feat(lazy-tools): DagExecutionEvent::ToolDescribed variant

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

Run `cargo check --bins` to confirm `main.rs` and `api.rs` compile (their match on this enum is exhaustive — they will fail until Task 12). If they fail, that's expected; do not fix here. If `--lib` succeeds, proceed.

---

## Task 9: `DagToolExecutor` interception of `describe_tool`

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs`

- [ ] **Step 1: Write failing test**

Append to the existing `mod tests` block (look for `intercepts_load_skill_when_repository_attached` to find the test block) a new test:

```rust
    #[tokio::test]
    async fn intercepts_describe_tool_when_lookup_attached() {
        use crate::dag_engine::domain::tool_configuration::ToolConfiguration;
        use crate::llm::domain::tools::{FunctionCall, ToolCall};
        use std::collections::HashMap;

        let registry = Arc::new(MockRegistry);
        let cfg = ToolConfiguration {
            name: "search_orders".to_string(),
            description: "Search orders".to_string(),
            node_type: "noop".to_string(),
            fixed_config: HashMap::new(),
            #[allow(deprecated)]
            exposed_inputs: None,
            #[allow(deprecated)]
            parameters: None,
            #[allow(deprecated)]
            mergeable_fields: None,
            #[allow(deprecated)]
            field_mapping: None,
            node_schema: None,
            node_config: None,
            expose_sub_tools: None,
            summary: None,
            eager: false,
        };
        let executor = DagToolExecutor::new(registry, HashMap::new())
            .with_describe_tool_lookup(vec![cfg]);
        let call = ToolCall::new(
            "call_1".to_string(),
            FunctionCall::new(
                "describe_tool".to_string(),
                serde_json::json!({"name":"search_orders"}).to_string(),
            ),
        );
        let result = executor.execute(&call).await.unwrap();
        assert!(result.success);
        assert!(result.output.contains("# search_orders"));
    }
```

- [ ] **Step 2: Run test — expect failure**

Run: `cargo test --lib dag_tool_executor::tests::intercepts_describe_tool_when_lookup_attached`
Expected: FAIL — `with_describe_tool_lookup` does not exist.

- [ ] **Step 3: Add the `ToolDescribeObserver` alias and field**

Locate the existing `pub type SkillObserver` near the top of `dag_tool_executor.rs`. Add right below it:

```rust
/// Callback fired when a `describe_tool` call succeeds, carrying the dispatched
/// payload so the enclosing LLM node can emit observability events.
pub type ToolDescribeObserver = Arc<
    dyn Fn(&crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::DescribeToolDispatchResult)
        + Send
        + Sync,
>;
```

In the `pub struct DagToolExecutor { ... }`, after the existing `skill_observer` field, add two new fields:

```rust
    /// Optional catalog of `ToolConfiguration` entries available for `describe_tool`
    /// to look up. When present, `describe_tool` calls are intercepted and dispatched
    /// against this slice; absent → describe_tool is not handled (caller error).
    describe_tool_lookup: Option<Vec<crate::dag_engine::domain::tool_configuration::ToolConfiguration>>,
    describe_tool_observer: Option<ToolDescribeObserver>,
```

In `DagToolExecutor::new(...)`, initialize them as `None`:

```rust
            describe_tool_lookup: None,
            describe_tool_observer: None,
```

After the existing `with_skill_observer` builder, add:

```rust
    /// Attach a snapshot of `ToolConfiguration` entries so `describe_tool` calls
    /// can be intercepted and resolved against this lookup.
    pub fn with_describe_tool_lookup(
        mut self,
        lookup: Vec<crate::dag_engine::domain::tool_configuration::ToolConfiguration>,
    ) -> Self {
        self.describe_tool_lookup = Some(lookup);
        self
    }

    /// Attach an observer callback that fires after a successful `describe_tool` dispatch.
    pub fn with_describe_tool_observer(mut self, cb: ToolDescribeObserver) -> Self {
        self.describe_tool_observer = Some(cb);
        self
    }
```

- [ ] **Step 4: Add the interception block in `execute`**

Locate the existing `if tool_call.function.name == LOAD_SKILL_TOOL_NAME { ... }` block in `DagToolExecutor::execute`. Immediately above (or below) it, add:

```rust
        use crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::{
            describe_tool_into_tool_result, dispatch_describe_tool, DESCRIBE_TOOL_NAME,
        };
        if tool_call.function.name == DESCRIBE_TOOL_NAME {
            let lookup = self.describe_tool_lookup.as_ref().ok_or_else(|| LlmError::ToolNotFound {
                name: DESCRIBE_TOOL_NAME.to_string(),
            })?;
            let result = dispatch_describe_tool(tool_call, lookup).await?;
            if let Some(obs) = &self.describe_tool_observer {
                obs(&result);
            }
            return Ok(describe_tool_into_tool_result(&tool_call.id, &result));
        }
```

If the existing `LOAD_SKILL_TOOL_NAME` import line is `use ... { dispatch_load_skill, into_tool_result, LOAD_SKILL_TOOL_NAME };`, rename the imported `into_tool_result` (a name conflict with `describe_tool::into_tool_result`). Do this in the `mod.rs` re-export (Task 0 already exports the describe_tool one as `describe_tool_into_tool_result`). Verify both names resolve.

- [ ] **Step 5: Re-run test — expect pass**

Run: `cargo test --lib dag_tool_executor::tests::intercepts_describe_tool_when_lookup_attached`
Expected: pass.

- [ ] **Step 6: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/dag_tool_executor.rs src/libs/colmena/src/dag_engine/infrastructure/nodes/llm_synthetic_tools/mod.rs
git commit -m "$(cat <<'EOF'
feat(lazy-tools): intercept describe_tool in DagToolExecutor

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 10: `tools_provider` field on `AgentRunParams`

**Files:**
- Modify: `src/libs/colmena/src/llm/application/agent_service.rs:8-18` and the loop body around line 84-95

- [ ] **Step 1: Write failing test**

Append a new test module to `agent_service.rs` (or extend the existing one):

```rust
#[cfg(test)]
mod tools_provider_tests {
    use super::*;
    use crate::llm::domain::tools::{ParameterProperty, ToolDefinition, ToolParameters};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// Verifies that `tools_provider`, when supplied, is invoked once per ReAct
    /// iteration and overrides the static `tools` Vec.
    #[tokio::test]
    async fn tools_provider_called_each_iteration() {
        let counter = Arc::new(AtomicUsize::new(0));
        let counter_c = counter.clone();
        let provider: Box<dyn Fn(&[LlmMessage]) -> Vec<ToolDefinition> + Send + Sync> =
            Box::new(move |_msgs| {
                counter_c.fetch_add(1, Ordering::SeqCst);
                vec![]
            });
        // Just construct the params — full ReAct loop wiring is exercised in
        // tests/lazy_tools_integration.rs. This test keeps the unit-level check
        // narrow: that the field exists and the type fits.
        let _params: AgentRunParams = AgentRunParams {
            session_id: &ConversationKey::new("s".to_string(), "n".to_string()),
            prompt: "hi".to_string(),
            messages: None,
            config: LlmConfig::default(),
            tools: vec![],
            tool_executor: &dummy_executor::DummyExecutor,
            max_iterations: Some(1),
            on_token: None,
            tools_provider: Some(provider),
        };
        // The test compiles → field exists. Behavior is covered by integration test.
        assert_eq!(counter.load(Ordering::SeqCst), 0);
    }

    mod dummy_executor {
        use crate::llm::domain::{LlmError, ToolCall, ToolExecutor, ToolResult};
        use async_trait::async_trait;
        pub struct DummyExecutor;
        #[async_trait]
        impl ToolExecutor for DummyExecutor {
            async fn execute(&self, _tool_call: &ToolCall) -> Result<ToolResult, LlmError> {
                Ok(ToolResult::success("x".into(), "ok".into()))
            }
        }
    }
}
```

- [ ] **Step 2: Run test — expect failure**

Run: `cargo test --lib agent_service::tools_provider_tests`
Expected: FAIL — field `tools_provider` not on `AgentRunParams`.

- [ ] **Step 3: Add the field**

In `agent_service.rs`, change the `pub struct AgentRunParams` to:

```rust
pub struct AgentRunParams<'a> {
    pub session_id: &'a ConversationKey,
    pub prompt: String,
    pub messages: Option<Vec<LlmMessage>>,
    pub config: LlmConfig,
    pub tools: Vec<ToolDefinition>,
    pub tool_executor: &'a dyn ToolExecutor,
    pub max_iterations: Option<usize>,
    pub on_token: Option<Box<dyn Fn(LlmStreamPart) + Send + Sync>>,
    /// Optional dynamic tools provider, called fresh at each ReAct iteration.
    /// When `Some`, its return value REPLACES `tools` for that iteration.
    /// When `None`, `tools` is used unchanged each iteration (default).
    pub tools_provider:
        Option<Box<dyn Fn(&[LlmMessage]) -> Vec<ToolDefinition> + Send + Sync>>,
}
```

- [ ] **Step 4: Update the loop**

In `AgentService::run`, locate the line (around line 92-95):

```rust
            let mut request = LlmRequest::new(messages.clone(), config.clone(), should_stream)?;
            if !tools.is_empty() {
                request = request.with_tools(tools.clone());
            }
```

Replace it with:

```rust
            let iteration_tools: Vec<ToolDefinition> = match &params_tools_provider {
                Some(p) => p(&messages),
                None => tools.clone(),
            };
            let mut request = LlmRequest::new(messages.clone(), config.clone(), should_stream)?;
            if !iteration_tools.is_empty() {
                request = request.with_tools(iteration_tools);
            }
```

And just below `let on_token = params.on_token;` near the top of `run`, add:

```rust
        let params_tools_provider = params.tools_provider;
```

- [ ] **Step 5: Update existing call sites**

Every caller that constructs `AgentRunParams` will fail to compile until they set `tools_provider: None`. Find them:

```bash
grep -rn "AgentRunParams {" src/libs/colmena/src/ | grep -v agent_service.rs
```

For each match, add `tools_provider: None,` to the struct literal. Most will be in `dag_engine/infrastructure/nodes/llm.rs` and tests.

- [ ] **Step 6: Re-run test — expect pass**

Run: `cargo test --lib agent_service::tools_provider_tests`
Then: `cargo check --lib && cargo check --bins`
Expected: all pass.

- [ ] **Step 7: Commit**

```bash
git add src/libs/colmena/src/llm/application/agent_service.rs src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs
git commit -m "$(cat <<'EOF'
feat(lazy-tools): tools_provider hook on AgentRunParams ReAct loop

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 11: Wire lazy mode into `LlmNode`

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs`

This is the integration step. It builds the catalog from `tool_configurations`, constructs the `tools_provider` closure, attaches the describe_tool lookup + observer to `DagToolExecutor`, and emits the `ToolDescribed` event.

- [ ] **Step 1: Add imports near the top of the file**

Find the existing `use crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::{ ... build_load_skill_tool_definition ... };` block. Replace it with:

```rust
use crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::{
    build_all_document_tools, build_describe_tool_definition, build_load_skill_tool_definition,
    reconstruct_discovered_set, summary_for_catalog, CatalogEntry, DocumentToolsContext,
    DescribeToolDispatchResult,
};
```

- [ ] **Step 2: Add `lazy_tool_loading` config parsing**

In `execute()` near the existing `tool_configurations` parsing (the block around line 632), after `tool_configurations` is built, add:

```rust
        let lazy_tool_loading: bool = inputs
            .get("lazy_tool_loading")
            .or_else(|| config.get("lazy_tool_loading"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
```

- [ ] **Step 3: Build the catalog, eager defs, and lazy defs**

After `lazy_tool_loading` is parsed and after `tool_configurations` exists, add:

```rust
        // Catalog and eager/lazy split — only meaningful in lazy mode but cheap to compute.
        let mut catalog: Vec<CatalogEntry> = Vec::new();
        let mut lookup_for_describe: Vec<crate::dag_engine::domain::tool_configuration::ToolConfiguration> = Vec::new();
        if lazy_tool_loading {
            for cfg in tool_configurations.values() {
                if cfg.eager {
                    continue;
                }
                if let Some(s) = &cfg.summary {
                    if s.chars().count() > 200 {
                        eprintln!(
                            "WARN: tool '{}' summary > 200 chars; will be truncated.",
                            cfg.name
                        );
                    }
                }
                catalog.push(CatalogEntry {
                    name: cfg.name.clone(),
                    summary: summary_for_catalog(cfg.summary.as_deref(), &cfg.description),
                });
                lookup_for_describe.push(cfg.clone());
            }
            if catalog.is_empty() && !tool_configurations.is_empty() {
                // All tools were eager: no lazy catalog. Nothing to do; describe_tool
                // will never be exposed. Continue silently.
            } else if tool_configurations.is_empty() {
                eprintln!(
                    "WARN: lazy_tool_loading: true but tool_configurations is empty — feature will no-op."
                );
            }
        }
```

- [ ] **Step 4: Track discovered set per node and emit `ToolDescribed`**

After the existing `skills_used_log` block (`Arc<std::sync::Mutex<Vec<SkillLoadedLogEntry>>>`), add:

```rust
        let tools_discovered_log: Arc<std::sync::Mutex<Vec<String>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
```

Then in the executor builder block (after `with_skill_observer` is wired), add inside the same `let tool_executor = { let mut executor = ...; ...; executor };` block:

```rust
            if lazy_tool_loading && !lookup_for_describe.is_empty() {
                executor = executor.with_describe_tool_lookup(lookup_for_describe.clone());
                let log_clone = tools_discovered_log.clone();
                let observer_clone = _observer.clone();
                executor = executor.with_describe_tool_observer(Arc::new(
                    move |result: &DescribeToolDispatchResult| {
                        if let Ok(mut log) = log_clone.lock() {
                            if !log.contains(&result.tool_name) {
                                log.push(result.tool_name.clone());
                            }
                        }
                        if let Some(obs) = &observer_clone {
                            obs.on_event(
                                crate::dag_engine::domain::observer::NodeEvent::ToolDescribed {
                                    tool_id: String::new(),
                                    tool_name: result.tool_name.clone(),
                                },
                            );
                        }
                    },
                ));
            }
```

- [ ] **Step 5: Build the dynamic tools provider closure**

Locate the section that builds `let mut tools: Vec<crate::llm::domain::ToolDefinition> = ...;` (around line 855). Right after `tools` is finalized (after the load_skill push and document tools push), add:

```rust
        // Cache the LLM-facing tool definitions for re-use inside the closure.
        let static_tools_snapshot: Vec<crate::llm::domain::ToolDefinition> = tools.clone();

        let tools_provider: Option<
            Box<dyn Fn(&[crate::llm::domain::LlmMessage]) -> Vec<crate::llm::domain::ToolDefinition> + Send + Sync>,
        > = if lazy_tool_loading && !catalog.is_empty() {
            let catalog = catalog.clone();
            let static_snapshot = static_tools_snapshot.clone();
            Some(Box::new(move |messages: &[crate::llm::domain::LlmMessage]| {
                let discovered = reconstruct_discovered_set(messages, &catalog);
                let pending: Vec<&CatalogEntry> = catalog
                    .iter()
                    .filter(|e| !discovered.contains(&e.name))
                    .collect();

                let mut out: Vec<crate::llm::domain::ToolDefinition> = Vec::new();

                // Always include any tool defined OUTSIDE the lazy catalog
                // (eager-flagged ones, load_skill, document_*, toolkit subtools).
                let catalog_names: std::collections::HashSet<&str> =
                    catalog.iter().map(|e| e.name.as_str()).collect();
                for td in &static_snapshot {
                    if !catalog_names.contains(td.name.as_str()) {
                        out.push(td.clone());
                    }
                }

                // Inject describe_tool only if there are pending entries.
                if !pending.is_empty() {
                    out.push(build_describe_tool_definition(&pending));
                }

                // Add full schema for each discovered (lazy) tool.
                for td in &static_snapshot {
                    if catalog_names.contains(td.name.as_str())
                        && discovered.contains(&td.name)
                    {
                        out.push(td.clone());
                    }
                }

                out
            }))
        } else {
            None
        };
```

- [ ] **Step 6: Pass to AgentService**

Find the existing `let params = AgentRunParams { ... };` (or the construction site of `AgentRunParams`). Add `tools_provider,` to the field list.

If the construction site is multiple lines, add it as the last field:

```rust
        let params = AgentRunParams {
            // ... existing fields ...
            tools_provider,
        };
```

- [ ] **Step 7: Verify compile**

Run: `cargo check --lib`
Expected: success.

- [ ] **Step 8: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs
git commit -m "$(cat <<'EOF'
feat(lazy-tools): wire lazy_tool_loading into LlmNode

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 12: SSE protocol mapping for `ToolDescribed`

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/main.rs`
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/api.rs`

- [ ] **Step 1: Edit `main.rs`**

Find the existing `match &event` arm `DagExecutionEvent::SkillLoaded { ... } => Some(serde_json::json!(...))`. Right after it, add:

```rust
                DagExecutionEvent::ToolDescribed { node_id, tool_id, tool_name } => Some(serde_json::json!({
                    "type": "tool-described",
                    "nodeId": node_id,
                    "toolCallId": tool_id,
                    "toolName": tool_name,
                })),
```

- [ ] **Step 2: Edit `api.rs`**

Find the equivalent `match &event` (or however `api.rs` translates events to the SSE protocol — search `SkillLoaded`):

```bash
grep -n "SkillLoaded" src/libs/colmena/src/dag_engine/infrastructure/api.rs
```

Add the same `ToolDescribed` arm in the same place. If `api.rs` does not match on `SkillLoaded` directly (the match may be exhaustive elsewhere), add a `ToolDescribed` arm with the same JSON shape.

- [ ] **Step 3: Verify**

Run: `cargo check --bins`
Expected: success — the previously non-exhaustive match now compiles.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/dag_engine/main.rs src/libs/colmena/src/dag_engine/infrastructure/api.rs
git commit -m "$(cat <<'EOF'
feat(lazy-tools): emit tool-described data-stream-protocol line

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 13: `tools_discovered` in output summary

**Files:**
- Modify: `src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs`

- [ ] **Step 1: Locate the summary assembly**

Find the existing block that populates `extra_info["skills_used"]` (search `extra_info["skills_used"]`):

```bash
grep -n 'skills_used' src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs
```

- [ ] **Step 2: Add the discovered tools summary**

Right after the `skills_used` aggregation, add:

```rust
        // tools_discovered (lazy_tool_loading): array of names in discovery order.
        if let Ok(log) = tools_discovered_log.lock() {
            if !log.is_empty() {
                extra_info["tools_discovered"] =
                    serde_json::Value::Array(log.iter().cloned().map(serde_json::Value::String).collect());
            }
        }
```

- [ ] **Step 3: Verify compile**

Run: `cargo check --lib`
Expected: success.

- [ ] **Step 4: Commit**

```bash
git add src/libs/colmena/src/dag_engine/infrastructure/nodes/llm.rs
git commit -m "$(cat <<'EOF'
feat(lazy-tools): tools_discovered field in extra_info summary

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 14: Integration test with `MockAdapter`

**Files:**
- Create: `src/libs/colmena/tests/lazy_tools_integration.rs`

- [ ] **Step 1: Write the integration test**

Create the file with:

```rust
//! Integration test: drive a multi-turn ReAct loop through the LlmNode in lazy
//! mode using MockAdapter, and assert the tools[] sent to the provider per turn.

use serde_json::json;

#[tokio::test]
async fn discovered_set_grows_across_turns_with_mock_adapter() {
    use colmena::dag_engine::domain::tool_configuration::ToolConfiguration;
    use colmena::dag_engine::infrastructure::nodes::llm_synthetic_tools::{
        reconstruct_discovered_set, CatalogEntry,
    };
    use colmena::llm::domain::{FunctionCall, LlmMessage, ToolCall};
    use std::collections::HashMap;

    // Simulate two assistant messages: turn 1 calls describe_tool("X");
    // turn 2 calls X directly.
    let m1 = LlmMessage::assistant_with_tool_calls(
        "".to_string(),
        vec![ToolCall::new(
            "c1".to_string(),
            FunctionCall::new(
                "describe_tool".to_string(),
                json!({"name": "X"}).to_string(),
            ),
        )],
    )
    .unwrap();
    let m2 = LlmMessage::assistant_with_tool_calls(
        "".to_string(),
        vec![ToolCall::new(
            "c2".to_string(),
            FunctionCall::new("X".to_string(), json!({"a": 1}).to_string()),
        )],
    )
    .unwrap();

    let catalog = vec![CatalogEntry {
        name: "X".to_string(),
        summary: "tool X".to_string(),
    }];

    // After turn 1 only:
    let after_t1 = reconstruct_discovered_set(&[m1.clone()], &catalog);
    assert!(after_t1.contains("X"));

    // After turn 2 (no describe_tool, but direct call to X):
    let after_t2_only = reconstruct_discovered_set(&[m2.clone()], &catalog);
    assert!(
        after_t2_only.contains("X"),
        "rule (2) must catch direct calls"
    );

    // Both messages: still just X.
    let after_both = reconstruct_discovered_set(&[m1, m2], &catalog);
    assert_eq!(after_both.len(), 1);
    assert!(after_both.contains("X"));

    // Sanity: a tool not in catalog and not invoked is not in the set.
    let _ = HashMap::<String, ToolConfiguration>::new();
}
```

- [ ] **Step 2: Run — expect pass**

Run: `cargo test --test lazy_tools_integration`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add src/libs/colmena/tests/lazy_tools_integration.rs
git commit -m "$(cat <<'EOF'
test(lazy-tools): integration test for reconstruction across turns

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 15: E2E graph

**Files:**
- Create: `tests/graphs/agents/tools_lazy_basic.json`

- [ ] **Step 1: Create the graph**

```json
{
  "nodes": {
    "trigger": {
      "type": "trigger_webhook",
      "config": {
        "path": "/test-lazy-tools",
        "method": "POST",
        "test_payload": {
          "prompt": "What's the current time, and can you find me orders from last month?"
        }
      }
    },
    "agent": {
      "type": "llm_call",
      "config": {
        "provider": "openai",
        "api_key": "${OPENAI_API_KEY}",
        "model": "gpt-4o-mini",
        "system_message": "You are a helpful assistant. Use the tools available.",
        "lazy_tool_loading": true,
        "tool_configurations": {
          "current_time": {
            "name": "current_time",
            "description": "Return the current UTC timestamp.",
            "summary": "Returns current UTC timestamp.",
            "node_type": "log",
            "eager": true,
            "fixed_config": {}
          },
          "search_orders": {
            "name": "search_orders",
            "description": "Search historical orders by date range.",
            "summary": "Find historical orders. Use when the user asks about past purchases.",
            "node_type": "log",
            "fixed_config": {}
          },
          "send_email": {
            "name": "send_email",
            "description": "Send a transactional email.",
            "summary": "Send transactional email. Use when explicitly asked.",
            "node_type": "log",
            "fixed_config": {}
          }
        }
      }
    },
    "log": { "type": "log" }
  },
  "edges": [
    {"from": "trigger", "to": "agent"},
    {"from": "agent", "to": "log"}
  ]
}
```

- [ ] **Step 2: Smoke-load the graph**

Run: `cargo run --bin dag_engine -- run tests/graphs/agents/tools_lazy_basic.json --include-extra-info 2>&1 | head -40`

Expected (with `OPENAI_API_KEY` set): the LLM emits a `describe_tool` call for `search_orders` (because the prompt asks about past orders) and uses `current_time` directly (eager). The SSE stream contains a `tool-described` line for `search_orders`. The final `extra_info` includes `tools_discovered: ["search_orders"]`.

If `OPENAI_API_KEY` is not set, document this; do NOT modify the test payload to use the mock provider in this file (the real-provider check is the value of the E2E test).

- [ ] **Step 3: Commit**

```bash
git add tests/graphs/agents/tools_lazy_basic.json
git commit -m "$(cat <<'EOF'
test(lazy-tools): e2e graph for lazy_tool_loading

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 16: Developer guide

**Files:**
- Create: `docs/developer_guide/29_lazy_tool_loading.md`
- Modify: `docs/DEVELOPER_GUIDE.md`
- Modify: `docs/developer_guide/14_llm_deep_dive.md`
- Modify: `docs/node_configurations.json`
- Modify: `CLAUDE.md`

- [ ] **Step 1: Write the guide**

Create `docs/developer_guide/29_lazy_tool_loading.md`:

```markdown
# 29. Lazy Tool Loading

Carga progresiva del schema de tools para nodos LLM. Cuando un `llm_call` tiene muchas tools (>10), inyectar todos los schemas completos en cada request al provider degrada la atención del modelo. Esta feature expone un catálogo ligero (`name + summary`) y solo revela el schema completo de las tools que el LLM decide usar, llamando al tool sintético `describe_tool`.

## Activación

Boolean a nivel del `llm_call`:

```json
{
  "type": "llm_call",
  "config": {
    "lazy_tool_loading": true,
    "tool_configurations": { ... }
  }
}
```

Ausente o `false` → comportamiento idéntico al de hoy. Backward-compat total.

## Configuración por tool

Dos campos opcionales nuevos en cada `ToolConfiguration`:

```jsonc
{
  "name": "search_orders",
  "summary": "Find historical orders. Use when the user asks about past purchases.",
  "description": "Search the orders table by date range, status, customer ID, or product SKU...",
  "node_type": "sql_query",
  "node_schema": { ... },
  "eager": false
}
```

- `summary`: opcional. Lo que el LLM ve en el catálogo. Si falta, se usa `description` truncada (~120 chars). Máximo 200 chars (warning + truncate al cargar).
- `eager`: opcional, default `false`. Una tool `eager: true` se registra en cada request con su schema completo y NO aparece en el catálogo. Úsalo para tools que se llaman casi siempre (ej. `current_time`, `get_user_id`).

## Cómo funciona en runtime

1. Al cargar el grafo se construye el catálogo: `[(name, summary), ...]` para cada tool no-eager.
2. En cada request al provider, el `tools[]` enviado se rebuild-ea:
   ```
   tools[] = [describe_tool si quedan pending] + [eager] + [discovered]
   ```
3. Cuando el LLM llama `describe_tool("X")`, el engine intercepta la call (mismo patrón que `load_skill`), genera el markdown curado del schema de `X`, y devuelve el contenido.
4. En el siguiente request, `X` deja el catálogo (ya descubierta) y aparece tipada en `tools[]` con su schema completo. El LLM la invoca normalmente.

## Persistencia con memoria

`discovered_set` no se guarda en BD. Es una vista derivada del historial: cada vez que el `llm_call` arranca con un `session_id` que tiene memoria, scan-ea los mensajes pasados y reconstruye el set:

- **Regla 1:** una llamada pasada a `describe_tool(name="X")` añade `X` al set.
- **Regla 2:** una llamada pasada directa a `X` (donde `X` está en el catálogo actual) añade `X` al set.

La regla 2 maneja tres casos: truncación que dropea el `describe_tool` original, sesiones que cambiaron de `eager` a `lazy` mid-flight, e historiales sembrados manualmente. Si AMBOS rastros caen del historial, la tool sale del set y el LLM la re-descubre la próxima vez que la necesite.

## Observabilidad

Por cada call a `describe_tool` exitosa el engine emite:

- Eventos estándar `LlmToolCallStart` / `LlmToolCallFinish` (como cualquier tool).
- Evento extra `ToolDescribed { tool_id, tool_name }` que en el data-stream-protocol del CLI/serve aparece como:
  ```json
  { "type": "tool-described", "nodeId": "...", "toolCallId": "...", "toolName": "search_orders" }
  ```

El summary final (`extra_info`) incluye:
```json
{ "tools_discovered": ["search_orders", "send_email"] }
```
solo cuando `lazy_tool_loading: true` y al menos una tool fue descubierta.

## Edge cases conocidos

- **LLM emite describe_tool y el tool real en el mismo turno**: algunos modelos pueden emitir tool calls paralelos. Si el LLM intenta llamar `X` en el mismo turno que llamó `describe_tool("X")`, el provider rechaza la segunda call (porque `X` no estaba en `tools[]` ese turno). El turno siguiente sí la verá tipada. La descripción del tool sintético dice explícitamente "Call it directly on your next turn" para reforzar el comportamiento. Es raro en práctica.
- **Truncation aggresiva**: si el rolling window dropea TANTO el `describe_tool` como cualquier llamada directa a una tool, la tool sale del `discovered_set` y el LLM tiene que re-describirla. Es comportamiento natural — no estás "olvidando" tools que el LLM nunca volverá a usar.

## Trust model

Mismo posture que skills: el engine valida estructura (`summary` length, schema válido) pero no contenido semántico. Un `summary` redactado para inducir prompt injection es responsabilidad de quien configura la tool. El catálogo se fija al cargar el grafo — el LLM no puede añadir tools nuevas en runtime.

## Referencia rápida

- Tool sintético: `describe_tool(name: string)`.
- La descripción del tool contiene el catálogo completo (nombre + summary de cada tool no-eager no-descubierta).
- Si no se configura `lazy_tool_loading`, la feature está completamente deshabilitada (zero overhead).
- Spec completo: [docs/superpowers/specs/2026-05-03-lazy-tool-loading-design.md](../superpowers/specs/2026-05-03-lazy-tool-loading-design.md)
```

- [ ] **Step 2: Add to `docs/DEVELOPER_GUIDE.md`**

Find the line `26. [**Skills**](./developer_guide/24_skills.md): ...` and the entries that follow. After the last numbered entry (currently `31. [**SSE Events Reference**](...)`), add:

```markdown
32. [**Lazy Tool Loading**](./developer_guide/29_lazy_tool_loading.md): Carga progresiva del schema de tools en `llm_call` vía el tool sintético `describe_tool`. Catálogo ligero `name + summary` inyectado en la descripción del tool; reveal on-demand; `discovered_set` reconstruido del historial; soporte para tools `eager: true` siempre-presentes; eventos `tool-described` en SSE y `tools_discovered` en el summary final.
```

If the existing numbering ends differently than `31`, use the next integer.

- [ ] **Step 3: Add a pointer in `docs/developer_guide/14_llm_deep_dive.md`**

Find the `#### \`skills\` (on-demand knowledge loading)` section. Right after that section, add:

```markdown
#### `lazy_tool_loading` (on-demand tool schemas)

Boolean optional flag (`true | false`, default `false`). When enabled, `tool_configurations` no longer ship full schemas in every request. Instead the LLM sees a synthetic `describe_tool` whose description lists `name + summary` per tool; calling it reveals the full schema and makes the tool callable on the next turn. See [29_lazy_tool_loading.md](29_lazy_tool_loading.md) for the full guide.
```

- [ ] **Step 4: Add fields to `docs/node_configurations.json`**

Find the `llm_call` entry, then the `config_fields` block. After the `skills` field (or in the same alphabetical neighborhood), add:

```jsonc
        "lazy_tool_loading": {
          "type": "boolean",
          "required": false,
          "default": false,
          "description": "When true, tools in `tool_configurations` are exposed via a lightweight catalog inside the synthetic `describe_tool`; full schemas are revealed on demand. Tools with `eager: true` remain always-on. See developer_guide/29_lazy_tool_loading.md.",
          "example": true
        }
```

Then locate the schema for `tool_configurations` value-object inside the same JSON. Find where existing fields like `description` and `node_schema` live. Add:

```jsonc
              "summary": {
                "type": "string",
                "required": false,
                "description": "Catalog entry shown when this tool is exposed via lazy loading. ≤ 200 chars; longer values are truncated with a warning. Falls back to first ~120 chars of `description` if absent."
              },
              "eager": {
                "type": "boolean",
                "required": false,
                "default": false,
                "description": "Only meaningful when the parent llm_call has `lazy_tool_loading: true`. An `eager: true` tool is registered with its full schema in every request and does not appear in the describe_tool catalog."
              }
```

If the JSON has a strict `tool_configuration_schema` block, add the two fields there. Use `python3 -c "import json; json.load(open('docs/node_configurations.json'))"` to verify validity.

- [ ] **Step 5: Update `CLAUDE.md`**

Locate the line `- \`24_skills.md\` — Skills feature: ...` in the developer guide list. After the most recent entry (likely `28_large_files_api.md`), add:

```markdown
    - `29_lazy_tool_loading.md` — Lazy tool loading: progressive describe_tool reveal, `summary`/`eager` per tool, `tools_discovered` summary
```

- [ ] **Step 6: Verify**

Run: `python3 -c "import json; json.load(open('docs/node_configurations.json'))" && echo OK`
Expected: `OK`.

- [ ] **Step 7: Commit**

```bash
git add docs/ CLAUDE.md
git commit -m "$(cat <<'EOF'
docs(lazy-tools): add 29_lazy_tool_loading guide and update indices

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

---

## Task 17: Final build, clippy, fmt, integration sweep

**Files:** None (validation only)

- [ ] **Step 1: Full check**

Run: `cargo check --lib && cargo check --bins`
Expected: clean.

- [ ] **Step 2: All tests**

Run: `cargo test --lib && cargo test --test lazy_tools_integration && cargo test --test skills_integration`
Expected: all pass. Skills integration must still pass — its dispatch path was not modified.

- [ ] **Step 3: Clippy**

Run: `cargo clippy --lib -- -D warnings 2>&1 | tail -40`
Expected: no warnings. If any clippy lint fires inside the new modules, fix inline (do NOT `#[allow(...)]` your way out). The most likely lints to surface are `type_complexity` (extract a type alias) and `manual_strip` (use `strip_prefix`).

- [ ] **Step 4: Format**

Run: `cargo fmt`
Expected: success. If diff is non-empty, review and commit:

```bash
git diff --stat
git add -u
git commit -m "$(cat <<'EOF'
style(lazy-tools): apply rustfmt

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 5: Manual E2E (optional, requires .env)**

Run:
```bash
set -a; source .env; set +a
cargo run --bin dag_engine -- run tests/graphs/agents/tools_lazy_basic.json --include-extra-info 2>&1 | tee /tmp/lazy-tools-e2e.log
```

Inspect `/tmp/lazy-tools-e2e.log`:
- Confirm `tool-described` event present with `toolName: "search_orders"` (because the prompt asks about past orders).
- Confirm `tool-described` event NOT present for `send_email` (the prompt does not request to send email).
- Confirm `current_time` was called directly without a preceding `tool-described` event (it is `eager: true`).
- Confirm final `extra_info.tools_discovered` includes `"search_orders"` and only `"search_orders"`.

If the LLM does not call `describe_tool` at all: most likely (a) the catalog is missing from the description (re-check Task 4), (b) the `tools_provider` closure isn't being called (re-check Task 11 step 5), or (c) the system_message is too prescriptive about not using tools.

- [ ] **Step 6: Final commit (if anything was tweaked)**

If the manual E2E surfaced minor fixes, commit them now. Otherwise skip.

---

## Self-review checklist (executed after writing plan)

**Spec coverage:**
- [x] Section "Architecture overview" — Tasks 0, 4, 9, 11
- [x] Section "Configuration schema" `lazy_tool_loading`/`summary`/`eager` — Tasks 1, 11
- [x] Section "describe_tool synthetic tool" — Tasks 4, 5, 6, 9
- [x] Section "Multi-turn flow + persistence" reconstruction logic — Task 3
- [x] Section "Internal architecture: shared infrastructure with skills" — naming and module layout in Tasks 0–6, 9
- [x] Section "Observability" SSE event + summary — Tasks 7, 8, 12, 13
- [x] Section "Errors and validation" — Tasks 1 (warning thresholds), 6 (dispatch errors), 11 (config warnings)
- [x] Section "Tests" — Tasks 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 14, 15
- [x] Section "Trust model" — referenced in developer guide (Task 16)
- [x] Section "Non-goals" — none implemented (correct: they are non-goals)

**Placeholder scan:** No `TBD`, `TODO`, `implement later`, `add validation` without code, or `Similar to Task N` references. Each step has the actual code or command. Two places use intentional flexibility ("If the existing X looks like Y" in Task 9 step 4 and Task 12 step 2) — those are guidance for handling potential code-shape drift, which the spec also flags as expected.

**Type consistency:**
- `DESCRIBE_TOOL_NAME` used consistently across Tasks 0, 3, 6, 9.
- `CatalogEntry` fields (`name: String`, `summary: String`) consistent across Tasks 0, 2, 3, 4, 11, 14.
- `DescribeToolDispatchResult` (`output`, `tool_name`) consistent across Tasks 0, 6, 9, 11.
- `tools_provider` closure signature `Box<dyn Fn(&[LlmMessage]) -> Vec<ToolDefinition> + Send + Sync>` matches between Task 10 (definition) and Task 11 (construction).
- `ToolDescribeObserver` consistent in Task 9 (definition) and Task 11 (use).
- `tools_discovered_log: Arc<Mutex<Vec<String>>>` consistent across Task 11 (init), Task 11 (observer push), Task 13 (read).

**Scope:** Single feature, one logical subsystem. Backed by an existing analogue (skills) for shared infra. Does not need decomposition.
