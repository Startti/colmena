//! Static analysis of a `graph.json` before it runs.
//!
//! The engine deserializes a node's `config` into an untyped
//! [`serde_json::Value`], so a field the author invented — `modle` for `model`
//! — is silently ignored rather than rejected. This module answers the question
//! the author actually has: which of these fields are real, and which did I
//! make up?
//!
//! Analysis is advisory. It never gates execution; see the `lint` CLI
//! subcommand for the reporting surface.

pub mod catalog;
pub mod diagnostic;
pub mod linter;

pub use catalog::{FieldSpec, NodeCatalog, NodeCatalogEntry, Requiredness, ANY_FIELD_KEY};
pub use diagnostic::{Diagnostic, DiagnosticCode, LintReport, Severity};
pub use linter::{lint_graph, lint_graph_json, KnownNodeTypes, LintContext};
