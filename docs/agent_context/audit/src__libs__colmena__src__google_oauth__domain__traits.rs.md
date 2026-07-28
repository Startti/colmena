# src/libs/colmena/src/google_oauth/domain/traits.rs

**Layer:** domain  
**Purpose:** Defines `AuthTokenProvider`, a hexagonal port trait for supplying OAuth 2.0 bearer tokens to Google API consumers. Decouples token acquisition strategy (refresh, caching, rotation) from callers; production implementation runs the refresh-token flow, tests inject mocks.

## Symbols

- `AuthTokenProvider` (trait, pub) — Port trait for acquiring OAuth bearer tokens; Send + Sync for concurrent Arc usage; implementations handle internal synchronization (e.g. tokio::sync::Mutex on cache).
- `get_bearer_token` (async fn, pub method) — Returns a currently-valid AccessToken; may refresh if cached token missing or near expiry; coalesces concurrent calls to prevent thundering herd.

## File-level notes

- Minimal and focused: single trait, single method, exemplary doc comments.
- No TODOs, FIXMEs, panics, or incomplete implementations.
- Trait is fundamentally a port: design is hexagonal-compliant (domain has zero infrastructure deps; only depends on `AccessToken` and `OAuthError` domain types).
- `#[async_trait]` macro usage is correct for trait async methods.
