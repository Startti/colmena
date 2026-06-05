//! Domain layer — port + value types + errors. No infra deps.

pub mod errors;
pub mod types;

pub use errors::SheetsError;
pub use types::*;
