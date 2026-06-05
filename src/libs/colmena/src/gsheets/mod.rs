//! Google Sheets integration (Subsystem E).
//!
//! Hexagonal layout:
//! - [`domain`] — port (`SheetsClient` trait), value types, errors.
//! - [`infrastructure`] — REST adapter, auth, config.
//!
//! Tool dispatchers live in
//! `crate::dag_engine::infrastructure::nodes::llm_synthetic_tools::gsheets_tools`
//! and delegate to the `SheetsClient` trait.

pub mod domain;
