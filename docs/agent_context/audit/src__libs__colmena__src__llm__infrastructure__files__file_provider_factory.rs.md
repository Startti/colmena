# src/libs/colmena/src/llm/infrastructure/files/file_provider_factory.rs

**Layer:** infrastructure  
**Purpose:** Factory implementation for instantiating `FileProviderRepository` adapters for each LLM provider (Anthropic, OpenAI, Google). Includes environment-variable-driven base URL override logic mirrored from the chat-side `LlmProviderFactory`.

## Symbols

- `base_url_override` (fn, private) — Resolves environment-variable base URL override for a given provider, checking per-provider vars (`*_BASE_URL`) first, then falling back to catch-all `COLMENA_LLM_BASE_URL`; returns `None` for Mock/Generated.
- `FileProviderFactory` (struct, pub) — Unit struct factory for creating `FileProviderRepository` instances; kept separate from chat-side `LlmProviderFactory` so changes to either path don't disturb the other.
- `FileProviderFactory::new` (fn, pub) — Constructor returning a `FileProviderFactory` instance (explicit for call-site clarity).
- `FileProviderFactory::create` (fn, pub) — Builds an `Arc<dyn FileProviderRepository>` for a given provider and API key; returns `LlmError::ProviderLimitation` for Mock and Generated.
- `Default` impl for FileProviderFactory — Standard trait, delegates to `Self::new()`.
- `FileProviderFactoryPort` impl for FileProviderFactory — Port trait implementation; `build()` delegates to `Self::create()`.
- `tests` module — Unit tests covering provider instance creation (Anthropic, OpenAI, Google) and rejection of Mock provider.
- `base_url_override_tests` module — Comprehensive environment-variable tests (clean env, per-provider override precedence, catch-all fallback, Mock/Generated immunity, end-to-end adapter behavior); uses `#[serial]` to coordinate with chat-side tests.
- `with_clean_env` (fn, private, in tests) — Test helper that clears all relevant env vars before and after running a closure, preventing cross-test pollution.

## File-level notes

- **Coupling to chat-side:** The comment at line 20–21 and line 148–150 explicitly state that `base_url_override()` mirrors `LlmProviderFactory::base_url_override()` exactly and shares serialization keys with its tests to prevent env-var race conditions. This is documented and intentional, but represents implicit coupling: if the chat-side function's precedence changes, this one must change too.
- **Environment-variable strategy:** Clear and well-documented precedence: per-provider env vars win over the catch-all `COLMENA_LLM_BASE_URL`, and both Mock and Generated always return `None`. Ensures consistent base URL steering across Files API and chat completions.
- **Error handling:** Explicit, typed errors (`ProviderLimitation`) with clear messages for unsupported providers; no silent fallbacks or defaults.
- **Test coverage:** Well-structured, using `#[serial]` correctly to avoid env-var clobbering under parallel `cargo test`; end-to-end tests verify that override resolution actually wires into adapters' `base_url()` getters.
