//! Domain layer — port + value types + errors. No infra deps.
//!
//! Submodules (`errors`, `traits`) and additional re-exports
//! (`DocsError`, `DocsClient`) are added by Tasks 3-4.

pub mod types;

pub use types::*;
