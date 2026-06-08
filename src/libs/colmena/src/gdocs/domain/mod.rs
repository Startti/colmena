//! Domain layer — port + value types + errors. No infra deps.

pub mod errors;
pub mod traits;
pub mod types;

pub use errors::DocsError;
pub use traits::DocsClient;
pub use types::*;
