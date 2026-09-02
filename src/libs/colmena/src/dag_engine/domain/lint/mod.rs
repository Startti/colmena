//! Static analysis of a `graph.json` before it runs.
//!
//! The engine deserializes a node's `config` into an untyped
//! [`serde_json::Value`], so a field the author invented — `modle` for `model` —
//! is silently ignored rather than rejected. This module exists to answer the
//! question the author actually has: which of these fields are real, and which
//! did I make up?
//!
//! This slice contributes the catalog: the typed view of
//! `docs/node_configurations.json` that says which fields each node type
//! accepts. The analysis built on top of it lands separately.

pub mod catalog;

pub use catalog::{FieldSpec, NodeCatalog, NodeCatalogEntry, Requiredness};
