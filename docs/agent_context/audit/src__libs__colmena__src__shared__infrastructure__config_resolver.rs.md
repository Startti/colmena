# src/libs/colmena/src/shared/infrastructure/config_resolver.rs

**Layer:** infrastructure  
**Purpose:** Provides concrete utilities to resolve LLM API keys from environment variables or explicit values and construct configured `LlmConfig` instances with validated parameters.

## Symbols

- `ConfigResolver` (struct, pub) — Stateless struct providing static methods for API key resolution and LLM config creation
- `resolve_api_key` (fn, pub) — Resolves an LLM API key from explicit value or environment variable, validating non-emptiness and falling back to env var lookup [FLAG: improvement — sparse error context on env lookup failure]
- `create_config` (fn, pub) — Creates an `LlmConfig` from provider kind and optional parameters (temperature, max_tokens, top_p, frequency_penalty, presence_penalty), applying builder pattern for each present parameter
- `load_env` (fn, pub) — Loads environment variables from .env file via `dotenvy::dotenv().ok()`, always returning `Ok(())` [FLAG: dead_candidate — no callers visible in-file; unclear if used elsewhere]

## File-level notes

- No domain imports other than LlmConfig/LlmError/LlmProvider/ProviderKind from llm::domain, confirming infrastructure-layer proper separation
- `create_config()` carries `#[allow(clippy::too_many_arguments)]`, signaling acknowledged but unresolved parameter explosion (8 args); could benefit from builder pattern or parameter struct
- `load_env()` wraps `dotenvy::dotenv()` with `.ok()` suppression, suggesting deliberate "fail silently" on missing .env; however, always returns `Ok(())` regardless of outcome, making error type unused
- Trim/empty-string validation in `resolve_api_key()` is defensive but does not distinguish "blank explicit key" from "missing env var" in the error message context
