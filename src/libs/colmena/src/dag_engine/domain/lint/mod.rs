//! Static analysis of a `graph.json` before it runs.
//!
//! The engine deserializes a node's `config` into an untyped
//! [`serde_json::Value`], so a field the author invented — `modle` for `model` —
//! is silently ignored rather than rejected. This module exists to answer the
//! question the author actually has: which of these fields are real, and which
//! did I make up?
//!
//! Analysis is advisory. It never gates execution.
//!
//! So far this module contributes the catalog — the typed view of
//! `docs/node_configurations.json` saying which fields each node type accepts —
//! and the vocabulary a finding is reported in. The analysis itself lands
//! separately.

pub mod catalog;
pub mod diagnostic;

pub use catalog::{FieldSpec, NodeCatalog, NodeCatalogEntry, Requiredness};
pub use diagnostic::{Diagnostic, DiagnosticCode, LintReport, Severity};
