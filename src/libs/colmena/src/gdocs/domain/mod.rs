//! Domain layer — port + value types + errors. No infra deps.
//!
//! Submodules (`traits`) and additional re-exports (`DocsClient`) are
//! added by Task 4.

pub mod errors;
pub mod types;

pub use errors::DocsError;
pub use types::*;
