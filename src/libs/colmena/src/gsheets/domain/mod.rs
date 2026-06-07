//! Domain layer — port + value types + errors. No infra deps.

pub mod errors;
pub mod traits;
pub mod types;

pub use errors::SheetsError;
pub use traits::SheetsClient;
pub use types::*;
