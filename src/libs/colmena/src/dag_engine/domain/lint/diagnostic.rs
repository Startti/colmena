//! What the linter reports, and how severe it is.

use std::fmt;

/// How much a finding matters.
///
/// Severity is advisory throughout: nothing here stops a graph from running.
/// It orders a report and decides the exit status of `dag_engine lint --strict`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// The graph will not behave as written. A field was invented, a node type
    /// does not exist, an edge points nowhere.
    Error,
    /// Probably wrong, but the linter cannot prove it from the graph alone.
    Warning,
    /// Context the reader needs to interpret the rest of the report — most
    /// importantly, that a node could not be checked at all.
    Info,
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Info => "info",
        })
    }
}

/// A stable identifier for the kind of finding.
///
/// Callers — the CLI's JSON output, and any UI built on the bindings — are
/// expected to branch on this rather than on the human-readable message, which
/// is free to change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DiagnosticCode {
    /// The node's `type` is not a node type the engine can run.
    UnknownNodeType,
    /// A key in the node's `config` that its node type does not accept.
    UnknownField,
    /// A key on the node object itself that the engine does not read.
    UnknownNodeProperty,
    /// A documented required field is absent.
    MissingRequiredField,
    /// A value outside the field's documented set of accepted values.
    InvalidFieldValue,
    /// A value whose JSON type does not match the documented type.
    FieldTypeMismatch,
    /// An edge endpoint naming a node that the graph does not define.
    EdgeUnknownNode,
    /// The node's type has no catalog entry, so its config was not checked.
    NoCatalogCoverage,
    /// A tool declares `fixed_config` alongside `node_schema`, which wins.
    DeadFixedConfig,
    /// A tool sets a key the target node type does not declare, on a node type
    /// that repurposes unknown keys instead of ignoring them.
    RepurposedToolField,
}

impl DiagnosticCode {
    /// The stable machine-readable name, e.g. `"UNKNOWN_FIELD"`.
    pub fn as_str(&self) -> &'static str {
        match self {
            DiagnosticCode::UnknownNodeType => "UNKNOWN_NODE_TYPE",
            DiagnosticCode::UnknownField => "UNKNOWN_FIELD",
            DiagnosticCode::UnknownNodeProperty => "UNKNOWN_NODE_PROPERTY",
            DiagnosticCode::MissingRequiredField => "MISSING_REQUIRED_FIELD",
            DiagnosticCode::InvalidFieldValue => "INVALID_FIELD_VALUE",
            DiagnosticCode::FieldTypeMismatch => "FIELD_TYPE_MISMATCH",
            DiagnosticCode::EdgeUnknownNode => "EDGE_UNKNOWN_NODE",
            DiagnosticCode::NoCatalogCoverage => "NO_CATALOG_COVERAGE",
            DiagnosticCode::DeadFixedConfig => "DEAD_FIXED_CONFIG",
            DiagnosticCode::RepurposedToolField => "REPURPOSED_TOOL_FIELD",
        }
    }
}

impl fmt::Display for DiagnosticCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One finding about one place in the graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub severity: Severity,
    pub code: DiagnosticCode,
    /// The node the finding is about. `None` for graph-level findings.
    pub node_id: Option<String>,
    /// The config field the finding is about, when it is about one.
    pub field: Option<String>,
    /// What is wrong, in the words the author needs to hear.
    pub message: String,
    /// A concrete next step — most often the field name the author meant.
    pub suggestion: Option<String>,
}

impl Diagnostic {
    /// A one-line rendering: `error [UNKNOWN_FIELD] node "chat".modle: …`.
    pub fn render(&self) -> String {
        let mut out = format!("{} [{}]", self.severity, self.code);
        match (&self.node_id, &self.field) {
            (Some(n), Some(f)) => out.push_str(&format!(" node \"{n}\".{f}")),
            (Some(n), None) => out.push_str(&format!(" node \"{n}\"")),
            (None, _) => {}
        }
        out.push_str(&format!(": {}", self.message));
        if let Some(s) = &self.suggestion {
            out.push_str(&format!(" — {s}"));
        }
        out
    }
}

/// Everything the linter found, in the order it should be read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LintReport {
    pub diagnostics: Vec<Diagnostic>,
}

impl LintReport {
    /// Whether anything at all was found.
    pub fn is_clean(&self) -> bool {
        self.diagnostics.is_empty()
    }

    /// How many findings there are at `severity`.
    pub fn count(&self, severity: Severity) -> usize {
        self.diagnostics
            .iter()
            .filter(|d| d.severity == severity)
            .count()
    }

    /// Whether the report contains anything that should fail a strict run.
    ///
    /// Info never does — "this node could not be checked" is a statement about
    /// the linter's own coverage, not about the graph.
    pub fn has_blocking_findings(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|d| matches!(d.severity, Severity::Error | Severity::Warning))
    }

    /// Sorts findings most-severe first, then by node, then by field, so two
    /// runs over the same graph always read the same way.
    pub fn sort(&mut self) {
        self.diagnostics.sort_by(|a, b| {
            a.severity
                .cmp(&b.severity)
                .then_with(|| a.node_id.cmp(&b.node_id))
                .then_with(|| a.field.cmp(&b.field))
                .then_with(|| a.code.cmp(&b.code))
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn diag(severity: Severity, code: DiagnosticCode, node: &str) -> Diagnostic {
        Diagnostic {
            severity,
            code,
            node_id: Some(node.to_string()),
            field: None,
            message: "m".into(),
            suggestion: None,
        }
    }

    #[test]
    fn severity_orders_error_before_warning_before_info() {
        assert!(Severity::Error < Severity::Warning);
        assert!(Severity::Warning < Severity::Info);
    }

    #[test]
    fn sort_puts_errors_first_then_orders_by_node() {
        let mut report = LintReport {
            diagnostics: vec![
                diag(Severity::Info, DiagnosticCode::NoCatalogCoverage, "z"),
                diag(Severity::Error, DiagnosticCode::UnknownNodeType, "b"),
                diag(Severity::Error, DiagnosticCode::UnknownNodeType, "a"),
                diag(Severity::Warning, DiagnosticCode::UnknownField, "c"),
            ],
        };
        report.sort();
        let order: Vec<_> = report
            .diagnostics
            .iter()
            .map(|d| (d.severity, d.node_id.clone().unwrap()))
            .collect();
        assert_eq!(
            order,
            vec![
                (Severity::Error, "a".to_string()),
                (Severity::Error, "b".to_string()),
                (Severity::Warning, "c".to_string()),
                (Severity::Info, "z".to_string()),
            ]
        );
    }

    /// A node the linter could not check is coverage information, not a defect
    /// in the graph. Failing a strict run on it would punish the author for a
    /// gap in our own catalog.
    #[test]
    fn info_alone_does_not_block_a_strict_run() {
        let report = LintReport {
            diagnostics: vec![diag(Severity::Info, DiagnosticCode::NoCatalogCoverage, "n")],
        };
        assert!(!report.is_clean(), "the finding is still reported");
        assert!(!report.has_blocking_findings());
    }

    #[test]
    fn a_warning_blocks_a_strict_run() {
        let report = LintReport {
            diagnostics: vec![diag(Severity::Warning, DiagnosticCode::UnknownField, "n")],
        };
        assert!(report.has_blocking_findings());
    }

    #[test]
    fn render_names_the_node_and_field() {
        let d = Diagnostic {
            severity: Severity::Warning,
            code: DiagnosticCode::UnknownField,
            node_id: Some("chat".into()),
            field: Some("modle".into()),
            message: "'modle' is not a field of llm_call".into(),
            suggestion: Some("did you mean 'model'?".into()),
        };
        assert_eq!(
            d.render(),
            "warning [UNKNOWN_FIELD] node \"chat\".modle: \
             'modle' is not a field of llm_call — did you mean 'model'?"
        );
    }

    #[test]
    fn codes_are_stable_strings() {
        assert_eq!(DiagnosticCode::UnknownField.as_str(), "UNKNOWN_FIELD");
        assert_eq!(
            DiagnosticCode::NoCatalogCoverage.as_str(),
            "NO_CATALOG_COVERAGE"
        );
    }
}
