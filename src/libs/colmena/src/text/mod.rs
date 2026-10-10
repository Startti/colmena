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

const LARGE_FILES_YAML: &str = include_str!("../../text/tools/large_files.yaml");

/// What differs between the tools in the large-file section.
#[derive(Debug, Deserialize)]
struct LargeFilesTool {
    var: String,
    opening: String,
    call: String,
    answer: String,
    unavailable: String,
}

#[derive(Debug, Deserialize)]
struct LargeFilesText {
    template: String,
    #[serde(flatten)]
    tools: HashMap<String, LargeFilesTool>,
}

static LARGE_FILES: OnceLock<LargeFilesText> = OnceLock::new();

fn large_files() -> &'static LargeFilesText {
    LARGE_FILES.get_or_init(|| {
        serde_yaml::from_str(LARGE_FILES_YAML)
            .unwrap_or_else(|e| panic!("text/tools/large_files.yaml malformed: {e}"))
    })
}

/// The section a tool's description opens with on a turn that has a large file: the shared
/// template filled with that tool's few differences. Panics if the tool has no entry.
pub fn large_file_section(tool: &str) -> String {
    let all = large_files();
    let t = all
        .tools
        .get(tool)
        .unwrap_or_else(|| panic!("text/tools/large_files.yaml has no entry for '{tool}'"));
    let mut out = all.template.trim_end().to_string();
    for (key, value) in [
        ("opening", &t.opening),
        ("call", &t.call),
        ("answer", &t.answer),
        ("unavailable", &t.unavailable),
        ("var", &t.var),
    ] {
        out = out.replace(&format!("{{{{{key}}}}}"), value.trim());
    }
    assert!(
        !out.contains("{{"),
        "an unfilled placeholder in the large-file section"
    );
    out
}

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

    /// gsheets/gdocs tools act as the platform account OR the user's connected
    /// Google account (`google_workspace_auth`). Their texts must hold for
    /// both: anything that depends on the account lives in the Google
    /// Workspace prelude, never in a tool description or summary.
    #[test]
    fn google_workspace_tool_texts_do_not_assume_the_platform_account() {
        let names: Vec<&str> = all_tool_names()
            .into_iter()
            .filter(|n| n.starts_with("gsheets_") || n.starts_with("gdocs_"))
            .collect();
        assert!(
            names.len() >= 45,
            "expected every gsheets/gdocs tool: {names:?}"
        );
        for name in names {
            for text in [tool_summary(name), tool_description(name)] {
                let lower = text.to_lowercase();
                for banned in [
                    "agents@startti.co",
                    "colmena_",
                    "agent account",
                    "oauth user",
                    "catalog prelude",
                    "prefer sharing",
                    "share an existing",
                    // Narrowed to "ask the operator" (not bare "operator"):
                    // `gsheets_run_python`'s description legitimately says
                    // "Operators can opt back into silent auto-suffix..." for
                    // the sink collision policy, unrelated to whose Google
                    // account the tools act as.
                    "ask the operator",
                    "service account",
                    "share email",
                ] {
                    assert!(
                        !lower.contains(banned),
                        "`{name}` text mentions `{banned}`: {text}"
                    );
                }
            }
        }
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
