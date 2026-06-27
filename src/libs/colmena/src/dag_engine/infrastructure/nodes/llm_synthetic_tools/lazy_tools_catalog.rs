//! Catalog management for lazy tool loading. Pure data types and pure functions
//! over conversation messages — no I/O, no provider awareness.

use crate::llm::domain::tools::{ParameterProperty, ToolDefinition, ToolParameters};
use crate::llm::domain::{LlmMessage, MessageRole};
use std::collections::{HashMap, HashSet};

/// Narrow `messages` to the CURRENT user-turn: everything from the last
/// `MessageRole::User` message onward. Lazy discovery is enforced **per turn**
/// (mirrors the gsheets inspect-before-code guard, which re-inspects each turn),
/// so a tool discovered in a previous turn must be re-discovered before reuse —
/// guaranteeing its schema/guidance is fresh in context even after history
/// compaction. With no user message (seeded history), the whole slice is kept.
pub fn current_turn_slice(messages: &[LlmMessage]) -> &[LlmMessage] {
    match messages
        .iter()
        .rposition(|m| *m.role() == MessageRole::User)
    {
        Some(idx) => &messages[idx..],
        None => messages,
    }
}

#[derive(Clone)]
pub struct CatalogEntry {
    pub name: String,
    pub summary: String,
}

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
/// Rule (2) handles three edge cases:
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
                if tc.function.name == super::DESCRIBE_TOOL_NAME {
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

/// Maximum length of a catalog summary entry, in chars.
pub const SUMMARY_MAX_CHARS: usize = 200;
/// Default truncation budget when falling back to the full description.
pub const FALLBACK_DESCRIPTION_CHARS: usize = 120;

/// Resolve the catalog summary string for a tool.
/// - If `summary` is present and ≤ 200 chars: return as-is (after trim).
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
    // Walk char indices, stop at the byte index that corresponds to char position max_chars.
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
ALWAYS call describe_tool for a tool BEFORE you invoke it — you need its schema to pass \
correct arguments and to see its usage notes. Available tools:\n{}\n\n\
Discovery is PER TURN: the first time you use a tool in a new turn, describe it again. \
If you skip this and call a tool directly without describing it this turn, you will NOT \
get its result — you'll receive its schema back as a redirect, and must then call the \
tool again with arguments that match it. \
Only describe a tool when you actually need it — not preemptively for every tool. \
After describe_tool, the revealed tool appears in your available tools and you can call it.",
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
        summary: None,
        parameters: ToolParameters {
            schema_type: "object".to_string(),
            properties,
            required: vec!["name".to_string()],
        },
        input_schema_override: None,
    }
}

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
    }

    #[test]
    fn truncates_summary_over_200_chars_at_word_boundary() {
        let long: String = "word ".repeat(80); // 400 chars
        let s = summary_for_catalog(Some(&long), "");
        assert!(s.len() <= 200, "got len {}", s.len());
        // After trimming the trailing space, last word should be "word".
        assert!(s.ends_with("word"));
    }

    #[test]
    fn returns_empty_string_when_neither_summary_nor_description() {
        let s = summary_for_catalog(None, "");
        assert_eq!(s, "");
    }

    use crate::llm::domain::{FunctionCall, ToolCall};

    fn entry(name: &str) -> CatalogEntry {
        CatalogEntry {
            name: name.to_string(),
            summary: format!("desc of {}", name),
        }
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
        let msg = assistant_with_call(
            super::super::DESCRIBE_TOOL_NAME,
            r#"{"name":"search_orders"}"#,
        );
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
    fn rule1_records_unknown_describe_tool_target() {
        // describe_tool faithfully records whatever name was passed; a later
        // catalog mismatch is the rebuild step's responsibility, not ours.
        let msg = assistant_with_call(
            super::super::DESCRIBE_TOOL_NAME,
            r#"{"name":"deleted_tool"}"#,
        );
        let set = reconstruct_discovered_set(&[msg], &[entry("search_orders")]);
        assert!(set.contains("deleted_tool"));
    }

    #[test]
    fn malformed_describe_tool_args_are_skipped_silently() {
        let msg = assistant_with_call(super::super::DESCRIBE_TOOL_NAME, r#"not-json"#);
        let set = reconstruct_discovered_set(&[msg], &[entry("search_orders")]);
        assert!(set.is_empty());
    }

    #[test]
    fn unions_rule1_and_rule2_across_messages() {
        let m1 = assistant_with_call(super::super::DESCRIBE_TOOL_NAME, r#"{"name":"a"}"#);
        let m2 = assistant_with_call("b", r#"{}"#);
        let set = reconstruct_discovered_set(&[m1, m2], &[entry("a"), entry("b")]);
        assert!(set.contains("a"));
        assert!(set.contains("b"));
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn definition_lists_pending_in_alphabetical_order() {
        let entries = [entry("zebra"), entry("apple"), entry("mango")];
        let pending: Vec<&CatalogEntry> = entries.iter().collect();
        let td = build_describe_tool_definition(&pending);
        let enum_values = td
            .parameters
            .properties
            .get("name")
            .unwrap()
            .enum_values
            .as_ref()
            .unwrap();
        assert_eq!(
            enum_values,
            &vec![
                "apple".to_string(),
                "mango".to_string(),
                "zebra".to_string()
            ]
        );
        let pos_a = td.description.find("apple").unwrap();
        let pos_m = td.description.find("mango").unwrap();
        let pos_z = td.description.find("zebra").unwrap();
        assert!(pos_a < pos_m && pos_m < pos_z);
    }

    #[test]
    fn definition_description_includes_summaries() {
        let entries = [CatalogEntry {
            name: "search_orders".into(),
            summary: "Find orders. Use for past purchases.".into(),
        }];
        let pending: Vec<&CatalogEntry> = entries.iter().collect();
        let td = build_describe_tool_definition(&pending);
        assert!(td.description.contains("search_orders"));
        assert!(td
            .description
            .contains("Find orders. Use for past purchases."));
    }

    #[test]
    fn definition_required_param_is_name() {
        let entries = [entry("a")];
        let pending: Vec<&CatalogEntry> = entries.iter().collect();
        let td = build_describe_tool_definition(&pending);
        assert_eq!(td.parameters.required, vec!["name".to_string()]);
        assert_eq!(td.name, "describe_tool");
    }

    // ---- current_turn_slice (per-turn discovery) ----------------------------

    #[test]
    fn current_turn_slice_no_user_returns_whole() {
        let msgs = vec![assistant_with_call("a", "{}")];
        assert_eq!(current_turn_slice(&msgs).len(), 1);
    }

    #[test]
    fn current_turn_slice_starts_at_last_user() {
        let msgs = vec![
            LlmMessage::user("turn 1".into()).unwrap(),
            assistant_with_call("a", "{}"),
            LlmMessage::user("turn 2".into()).unwrap(),
            assistant_with_call("b", "{}"),
        ];
        let slice = current_turn_slice(&msgs);
        // From the LAST user message onward: ["turn 2", assistant(b)].
        assert_eq!(slice.len(), 2);
        assert_eq!(slice[0].content(), "turn 2");
    }

    #[test]
    fn per_turn_discovery_ignores_prior_turn_describe() {
        // Tool `a` was described in turn 1, then a new user turn started with no
        // re-describe. Scoped to the current turn, `a` is NOT discovered → the
        // model must describe it again (per-turn enforcement).
        let msgs = vec![
            LlmMessage::user("turn 1".into()).unwrap(),
            assistant_with_call(super::super::DESCRIBE_TOOL_NAME, r#"{"name":"a"}"#),
            LlmMessage::user("turn 2".into()).unwrap(),
        ];
        let cat = [entry("a")];
        // History-wide would discover `a`; per-turn slice must not.
        assert!(reconstruct_discovered_set(&msgs, &cat).contains("a"));
        let set = reconstruct_discovered_set(current_turn_slice(&msgs), &cat);
        assert!(
            !set.contains("a"),
            "per-turn slice must drop prior-turn describe"
        );
    }

    #[test]
    fn per_turn_discovery_keeps_this_turn_describe() {
        let msgs = vec![
            LlmMessage::user("turn 2".into()).unwrap(),
            assistant_with_call(super::super::DESCRIBE_TOOL_NAME, r#"{"name":"a"}"#),
        ];
        let set = reconstruct_discovered_set(current_turn_slice(&msgs), &[entry("a")]);
        assert!(set.contains("a"));
    }
}
