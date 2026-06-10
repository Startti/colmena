//! Domain layer — port + value types + errors. No infra deps.

pub mod errors;
pub mod traits;
pub mod types;

pub use errors::OAuthError;
pub use traits::AuthTokenProvider;
pub use types::{AccessToken, CachedToken, RefreshTokenSecret};
