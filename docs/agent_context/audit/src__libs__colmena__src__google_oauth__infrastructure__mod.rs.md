# src/libs/colmena/src/google_oauth/infrastructure/mod.rs

**Layer:** infrastructure  
**Purpose:** Module root and public API for the OAuth infrastructure layer; re-exports env-var config, token caching, refresh-client adapters, and the AuthTokenProvider port implementation.

## Symbols

- `config` (mod, pub) — Submodule containing OAuth credentials configuration loaded from environment variables
- `provider_cache` (mod, pub) — Submodule for in-process OAuth token cache and lifecycle management
- `refresh_client` (mod, pub) — Submodule for HTTP-based token refresh client implementation
- `token_provider` (mod, pub) — Submodule implementing the `AuthTokenProvider` port trait from domain
- `OAuthCredentials` (type, pub) — Re-exported from `config`; OAuth credentials tuple from env vars
- `OAuthProviderCache` (type, pub) — Re-exported from `provider_cache`; manages in-process token cache
- `RefreshClient` (type, pub) — Re-exported from `refresh_client`; HTTP client for token refresh requests
- `RefreshResponse` (type, pub) — Re-exported from `refresh_client`; token refresh response structure
- `OAuthRefreshTokenProvider` (type, pub) — Re-exported from `token_provider`; port adapter implementing `AuthTokenProvider`

## File-level notes

- Clean module organization with proper separation of concerns across four submodules.
- Re-exports form the public API surface for the infrastructure layer.
- All submodule declarations are public and properly documented.
- No unfinished code, dead symbols, or structural issues detected.
