//! Catalog management for lazy tool loading. Pure data types and pure functions
//! over conversation messages — no I/O, no provider awareness.

use crate::llm::domain::tools::{ToolDefinition, ToolParameters};
use std::collections::HashMap;

pub struct CatalogEntry {
    pub name: String,
    pub summary: String,
}

pub fn reconstruct_discovered_set(
    _messages: &[crate::llm::domain::LlmMessage],
    _catalog: &[CatalogEntry],
) -> std::collections::HashSet<String> {
    std::collections::HashSet::new()
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

pub fn build_describe_tool_definition(_pending: &[&CatalogEntry]) -> ToolDefinition {
    ToolDefinition {
        name: super::DESCRIBE_TOOL_NAME.to_string(),
        description: String::new(),
        parameters: ToolParameters {
            schema_type: "object".to_string(),
            properties: HashMap::new(),
            required: vec![],
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
}
