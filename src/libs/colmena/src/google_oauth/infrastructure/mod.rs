//! Infrastructure layer — env-var config, HTTP refresh client, in-process
//! token cache, per-node Google Workspace credentials
//! (`workspace_auth`). Implements the `AuthTokenProvider` port defined in
//! `domain::traits`.

pub mod config;
pub mod host_refresh_provider;
pub mod provider_cache;
pub mod refresh_client;
pub mod token_provider;
pub mod workspace_auth;

pub use config::{parse_oauth_refresh_block, OAuthCredentials, OAuthRefreshBlock};
pub use host_refresh_provider::HostRefreshTokenProvider;
pub use provider_cache::OAuthProviderCache;
pub use refresh_client::{RefreshClient, RefreshResponse};
pub use token_provider::OAuthRefreshTokenProvider;
pub use workspace_auth::{GoogleWorkspaceAuth, GOOGLE_TOKEN_ENDPOINT};
