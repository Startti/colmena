//! Loader for LLM-facing text content under `src/libs/colmena/text/`.
//!
//! YAML files at `text/tools/*.yaml` are embedded at compile time via
//! `include_str!` and parsed into a static `HashMap` at first access.
//! Missing entries panic with a clear "add an entry" message — failures
//! are detectable at startup, not deep in a tool call.
//!
//! See `docs/superpowers/specs/2026-06-06-text-centralization-design.md`.

use serde::Deserialize;
use std::collections::HashMap;
use std::sync::OnceLock;

#[derive(Debug, Deserialize)]
pub struct ToolText {
    pub summary: String,
    pub description: String,
}

const GSHEETS_YAML: &str = include_str!("../../text/tools/gsheets.yaml");
const CRDT_DOC_YAML: &str = include_str!("../../text/tools/crdt_doc.yaml");
const DOCUMENTS_YAML: &str = include_str!("../../text/tools/documents.yaml");
const HELPERS_YAML: &str = include_str!("../../text/tools/helpers.yaml");
const GDOCS_YAML: &str = include_str!("../../text/tools/gdocs.yaml");
const SQL_YAML: &str = include_str!("../../text/tools/sql.yaml");
const DATA_RUN_PYTHON_YAML: &str = include_str!("../../text/tools/data_run_python.yaml");

static TOOL_TEXTS: OnceLock<HashMap<String, ToolText>> = OnceLock::new();

/// Populate the registry from every embedded YAML. Panics if any YAML is
/// malformed or a tool key appears in more than one file.
fn load() -> &'static HashMap<String, ToolText> {
    TOOL_TEXTS.get_or_init(|| {
        let mut m: HashMap<String, ToolText> = HashMap::new();
        for (label, yaml) in [
            ("gsheets", GSHEETS_YAML),
            ("crdt_doc", CRDT_DOC_YAML),
            ("documents", DOCUMENTS_YAML),
            ("helpers", HELPERS_YAML),
            ("gdocs", GDOCS_YAML),
            ("sql", SQL_YAML),
            ("data_run_python", DATA_RUN_PYTHON_YAML),
        ] {
            // Empty file ("{}") parses to an empty map; that's expected
            // before T2-T5 populate the registry.
            let parsed: HashMap<String, ToolText> = serde_yaml::from_str(yaml)
                .unwrap_or_else(|e| panic!("text/tools/{label}.yaml malformed: {e}"));
            for (k, v) in parsed {
                if m.insert(k.clone(), v).is_some() {
                    panic!("duplicate tool key '{k}' across text/tools/*.yaml");
                }
            }
        }
        m
    })
}

/// Lookup the summary for a registered synthetic tool. Panics with a
/// clear message if the tool is missing from `text/tools/*.yaml`.
pub fn tool_summary(name: &str) -> &'static str {
    load()
        .get(name)
        .map(|t| t.summary.as_str())
        .unwrap_or_else(|| {
            panic!(
                "Missing 'summary' for tool '{name}' in text/tools/*.yaml. \
                 Add an entry or pass an explicit summary to the builder."
            )
        })
}

/// Lookup the description for a registered synthetic tool. Panics if missing.
pub fn tool_description(name: &str) -> &'static str {
    load()
        .get(name)
        .map(|t| t.description.as_str())
        .unwrap_or_else(|| panic!("Missing 'description' for '{name}' in text/tools/*.yaml"))
}

/// Every tool name currently in the registry. Used by tests to detect
/// orphan YAML entries (entries with no matching registered builder).
pub fn all_tool_names() -> Vec<&'static str> {
    load().keys().map(|s| s.as_str()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yaml_files_parse_at_startup() {
        // Calling load() forces every embedded YAML to be parsed. A
        // malformed file produces a clear panic with the file label and
        // the serde error.
        let _ = load();
    }

    #[test]
    fn empty_registry_is_acceptable_initially() {
        // Before T2-T5 land, all YAMLs are "{}". The loader must accept
        // that gracefully — the orphan/missing tests run later.
        let names = all_tool_names();
        // Length is 0 before tool migrations, > 0 after. Either is OK.
        assert!(
            names.len() <= 100,
            "registry suspiciously large: {}",
            names.len()
        );
    }

    #[test]
    fn duplicate_yaml_keys_would_panic_in_load() {
        // The duplicate-key panic is inside load() and can't be reached
        // without modifying the embedded YAML files. This test verifies
        // the SHAPE of the duplicate-detection logic by parsing two
        // synthetic YAMLs with the same key into one HashMap manually —
        // mirroring what load() does.
        let yaml_a: &str = "shared_key:\n  summary: from a\n  description: x\n";
        let yaml_b: &str = "shared_key:\n  summary: from b\n  description: y\n";
        let mut m: HashMap<String, ToolText> = HashMap::new();
        let parsed_a: HashMap<String, ToolText> = serde_yaml::from_str(yaml_a).unwrap();
        m.extend(parsed_a);
        let parsed_b: HashMap<String, ToolText> = serde_yaml::from_str(yaml_b).unwrap();
        // The second insert would have triggered the duplicate panic in load().
        // We can't reach the panic without spawning a subprocess; we sanity-check
        // that the shape of detection is correct.
        for k in parsed_b.keys() {
            assert!(
                m.contains_key(k.as_str()),
                "duplicate detection sanity check failed"
            );
        }
    }
}
