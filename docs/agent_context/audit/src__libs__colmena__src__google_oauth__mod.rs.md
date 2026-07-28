# src/libs/colmena/src/google_oauth/mod.rs

**Layer:** shared  
**Purpose:** Root module for Google OAuth user-scoped authentication. Declares hexagonal domain and infrastructure layers for token management shared by gsheets and gdocs subsystems, with public API re-exports.

## Symbols

- `domain` (pub mod) — Domain layer: `AuthTokenProvider` port trait, value objects (`AccessToken`, `RefreshTokenSecret`, `CachedToken`), and `OAuthError` enum with zero infrastructure dependencies.
- `infrastructure` (pub mod) — Infrastructure layer: OAuth token refresh implementation with env-var config, HTTP refresh client, and in-process token cache.
- `AccessToken` (pub use) — Value object representing an OAuth access token with expiry metadata.
- `AuthTokenProvider` (pub use) — Port trait defining the interface for obtaining bearer tokens; implemented by infrastructure with caching and refresh.
- `CachedToken` (pub use) — Value object representing a cached token with storage metadata.
- `OAuthError` (pub use) — Error enum for OAuth-related failures (invalid config, refresh errors, etc.).
- `RefreshTokenSecret` (pub use) — Value object wrapping a refresh token secret from env var `COLMENA_GOOGLE_OAUTH_REFRESH_TOKEN`.

## File-level notes

- Clean hexagonal separation: domain defines the port and value objects; infrastructure provides the concrete implementation.
- Documentation references the design spec: `docs/superpowers/specs/2026-06-10-oauth-user-scoped-design.md`.
- All re-exported types are stable public API used by gsheets and gdocs auth modules.
- No unused symbols, incomplete implementations, or code quality issues detected.
