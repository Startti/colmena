//! LLM tool `data_run_python` — the unified cross-store sandbox tool.
//! Binds each tabular source ([`DataBinding`], from `tabular_bindings`)
//! where the data lives NOW (attachment, Google Sheets, SQL `SELECT`,
//! or inline JSON) and runs sandboxed Python over all of them at once.
//! Rows never enter the LLM's context — only the script's `output`
//! (plus any `output_tables` / `output_sheets` / `output_attachments`
//! sinks defined by later tasks) crosses the wire.
//!
//! This module owns the args struct, capability gating types, and the
//! dynamic-description tool builder only. Dispatch/execution lives in
//! later tasks (see `docs/superpowers/specs/2026-07-01-data-run-python-design.md`).

use crate::llm::domain::tools::ToolDefinition;
use crate::text;
use schemars::JsonSchema;
use serde::Deserialize;

use super::tabular_bindings::{deserialize_bindings_flexible, DataBinding};

pub const TOOL_DATA_RUN_PYTHON: &str = "data_run_python";

const DATA_PY_PRELUDE: &str =
    include_str!("../../../../../text/prompts/python_sandbox/data_run_python_prelude.md");
const DATA_PY_POSTLUDE: &str =
    include_str!("../../../../../text/prompts/python_sandbox/data_run_python_postlude.md");

/// Wrap user-supplied Python with the colmena prelude/postlude so the
/// sandboxed script has `pd`/`np`/`stats` in scope and, on exit, packages
/// `output`/`output_tables`/`output_sheets`/`output_attachments` into a
/// single `output` dict the dispatcher parses.
///
/// `#[allow(dead_code)]` until Task 14 wires the dispatcher that calls
/// this from `execute`.
#[allow(dead_code)]
fn wrap_user_code(user_code: &str) -> String {
    format!("{DATA_PY_PRELUDE}{user_code}{DATA_PY_POSTLUDE}")
}

// ── Args ─────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DataRunPythonArgs {
    /// Tabular bindings to load. Each becomes a Python global (a list of
    /// `{col: val}` dicts) under the name given by `var`. At least one
    /// entry required. Bindings are fetched IN PARALLEL, each from its
    /// own source (attachment, Google Sheets, SQL `SELECT`, or inline
    /// JSON data).
    #[serde(deserialize_with = "deserialize_bindings_flexible")]
    pub bindings: Vec<DataBinding>,

    /// Python code. Has access to `pandas as pd`, `numpy as np`,
    /// `scipy.stats as stats`, plus each binding's records list bound
    /// under its `var` name. Define `output` (any JSON-serializable
    /// value) — that is what the LLM sees.
    pub code: String,

    /// Optional target spreadsheet for `output_sheets` returned by the
    /// script (Google Sheets sink). Only meaningful when the `gsheets`
    /// capability is enabled.
    #[serde(default)]
    pub write_to_spreadsheet: Option<String>,
}

// ── Capability gating ────────────────────────────────────────────────

/// Which data-store capabilities the operator has enabled for this tool
/// instance. `attachment` and `inline` sources are always available and
/// have no corresponding flag here.
#[derive(Debug, Clone, Copy, Default)]
pub struct EnabledSources {
    pub sql: bool,
    pub gsheets: bool,
}

// ── Tool builder ─────────────────────────────────────────────────────

/// Build the [`ToolDefinition`] for `data_run_python`, appending a
/// dynamically-assembled "Available sources" section to the static base
/// description so the model only ever sees sources the operator actually
/// enabled.
pub fn tool_data_run_python(enabled: &EnabledSources) -> ToolDefinition {
    let mut description = String::from(text::tool_description(TOOL_DATA_RUN_PYTHON));

    description.push_str("\n\nAvailable sources:\n- Attachment (CSV/XLSX from the conversation catalog)\n- Inline JSON data");
    if enabled.gsheets {
        description.push_str("\n- Google Sheets");
    }
    if enabled.sql {
        description.push_str("\n- SQL database tables");
    }

    super::build_synthetic_tool_with_summary::<DataRunPythonArgs>(
        TOOL_DATA_RUN_PYTHON,
        &description,
        text::tool_summary(TOOL_DATA_RUN_PYTHON),
    )
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_definition_lists_only_enabled_sources() {
        let def = tool_data_run_python(&EnabledSources {
            sql: true,
            gsheets: false,
        });
        let desc = def.description.to_lowercase();
        assert!(desc.contains("sql") || desc.contains("database"));
        assert!(!desc.contains("google sheet"));
    }

    #[test]
    fn args_parse_minimal() {
        let a: DataRunPythonArgs = serde_json::from_value(serde_json::json!({
            "bindings":[{"var":"x","query":"SELECT 1"}], "code":"output=1"
        }))
        .unwrap();
        assert_eq!(a.bindings.len(), 1);
    }

    #[test]
    fn wrapped_code_packages_all_sinks() {
        let w = wrap_user_code("output = 1");
        assert!(w.contains("output_tables"));
        assert!(w.contains("output_sheets"));
        assert!(w.contains("output_attachments"));
        assert!(w.contains("user_output"));
    }

    #[test]
    fn prelude_imports_pandas() {
        let w = wrap_user_code("output = 1");
        assert!(w.contains("import pandas"));
    }
}
