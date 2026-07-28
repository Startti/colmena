# src/libs/colmena/src/google_oauth/domain/mod.rs

**Layer:** domain  
**Purpose:** Domain layer entry point for Google OAuth — re-exports error types, traits, and value objects with zero infrastructure dependencies.

## Symbols

- `errors` (pub mod) — Domain errors submodule
- `traits` (pub mod) — Domain traits submodule (authentication port)
- `types` (pub mod) — Domain value types submodule
- `OAuthError` (pub use) — Error type re-exported from `errors` module
- `AuthTokenProvider` (pub use) — Trait re-exported from `traits` module
- `AccessToken` (pub use) — Value type re-exported from `types` module
- `CachedToken` (pub use) — Value type re-exported from `types` module
- `RefreshTokenSecret` (pub use) — Value type re-exported from `types` module

## File-level notes

- Pure re-export aggregator; no implementation or logic in this file
- Hexagonal architecture: domain layer gates public API (ports, errors, value objects); all implementation resides in submodules
- Clean module organization: three submodules (errors, traits, types) each independently documented and tested
- Eight total public exports: 3 module declarations + 5 value/trait re-exports covering error handling, token provision interface, and token representations
