//! Port (in hexagonal terms) for any backend that can supply OAuth 2.0
//! bearer tokens for Google API calls.
//!
//! The production impl `infrastructure::OAuthRefreshTokenProvider` runs
//! the refresh-token flow against `oauth2.googleapis.com`. Tests use
//! a mocked impl that returns canned tokens to keep wiremock setups
//! independent of Google's actual OAuth server.

use crate::google_oauth::domain::{AccessToken, OAuthError};
use async_trait::async_trait;

/// Supplies an OAuth bearer token. Implementations decide whether to
/// cache, refresh, rotate, etc. — callers see only the resulting
/// token.
///
/// The trait is `Send + Sync` so providers can be shared across
/// concurrent tasks via `Arc`. Implementations must handle their own
/// internal synchronisation (e.g. via `tokio::sync::Mutex` on the
/// cache).
#[async_trait]
pub trait AuthTokenProvider: Send + Sync {
    /// Return a currently-valid `AccessToken`. May trigger a refresh
    /// against the OAuth server if the cached token is missing or
    /// near expiry. Concurrent calls from multiple tasks must
    /// coalesce into a single refresh (no thundering herd).
    async fn get_bearer_token(&self) -> Result<AccessToken, OAuthError>;
}
