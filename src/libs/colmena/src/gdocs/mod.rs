//! Google Docs integration (Subsystem G).
//!
//! Hexagonal layout:
//! - [`domain`] — `DocsClient` port, value types, errors.
//! - [`application`] — high-level use cases (replace_text, insert, style,
//!   apply_edits, co_edit_guard, …).
//! - [`infrastructure`] — REST adapter, auth, config, postgres revision
//!   store, in-memory outline cache.
//!
//! Tool dispatchers live in
//! `crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::gdocs_tools`.

pub mod application;
pub mod domain;
pub mod infrastructure;
