//! Google OAuth user-scoped auth — shared across `gsheets` and `gdocs`.
//!
//! Replaces the per-subsystem Service Account JWT signing path with an
//! OAuth 2.0 refresh-token flow on a dedicated Workspace user
//! (`agents@startti.co` in the canonical production deploy). The user
//! consents once via `colmena_oauth_setup` (operator's laptop); the
//! resulting `refresh_token` lives in Google Secret Manager and is
//! mounted into the runtime as the env var
//! `COLMENA_GOOGLE_OAUTH_REFRESH_TOKEN`.
//!
//! Hexagonal layout:
//! - [`domain`] — `AuthTokenProvider` port, `AccessToken` /
//!   `RefreshTokenSecret` value types, `OAuthError` enum. Zero infra
//!   dependencies.
//! - [`infrastructure`] — env-var config, HTTP refresh client against
//!   `oauth2.googleapis.com`, in-process token cache.
//!
//! Consumers in `gsheets::infrastructure::auth` and
//! `gdocs::infrastructure::auth` hold an `Arc<dyn AuthTokenProvider>`
//! and call `get_bearer_token()` per request. The provider handles
//! caching + refresh internally — callers never see the refresh
//! protocol.
//!
//! See [`docs/superpowers/specs/2026-06-10-oauth-user-scoped-design.md`]
//! for the full design.

pub mod domain;
pub mod infrastructure;

pub use domain::{AccessToken, AuthTokenProvider, CachedToken, OAuthError, RefreshTokenSecret};
