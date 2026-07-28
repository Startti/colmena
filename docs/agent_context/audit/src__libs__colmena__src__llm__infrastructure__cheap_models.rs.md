# src/libs/colmena/src/llm/infrastructure/cheap_models.rs

**Layer:** infrastructure  
**Purpose:** Provides runtime-configurable cheap model selection per LLM provider for internal summarization tasks. Implements priority resolution chain: environment variables (`COLMENA_CHEAP_MODEL_*`) → embedded YAML config → provider default.

## Symbols

- `CHEAP_MODELS_YAML` (const, private) — embedded YAML configuration from `text/config/cheap_models.yaml` loaded at compile-time
- `CHEAP_MODELS` (static, private) — lazy-initialized `OnceLock` cache holding parsed registry of cheap models per provider
- `registry()` (fn, private) — initializes and returns the parsed YAML registry; panics on malformed YAML
- `cheap_model_for()` (fn, pub) — resolves cheap model for a provider via env override → YAML registry → provider default
- `tests` (mod) — test module with 4 tests

## Detailed symbol descriptions

- `CHEAP_MODELS_YAML` — Compile-time embedding of YAML file containing cheap model mappings (e.g., `google: gemini-2.5-flash`)
- `CHEAP_MODELS` — Thread-safe once-initialized cache avoiding repeated YAML parsing on every `cheap_model_for()` call
- `registry()` — Parses YAML on first call, caches result; panics if YAML is malformed (fail-fast on deployment error)
- `cheap_model_for()` — Checks env var `COLMENA_CHEAP_MODEL_<PROVIDER>` (trimmed, non-empty) → registry → `provider.default_model()`; returns `String`

## Tests

- `yaml_has_the_three_real_providers` — verifies YAML contains google/openai/anthropic entries
- `yaml_default_when_no_env` — confirms fallback to YAML when env var absent
- `env_override_wins` — confirms env var takes precedence over YAML
- `never_returns_gemini_1_5` — regression guard: ensures gemini-1.5-flash is never returned (deprecated model)

## File-level notes

- No unused symbols or dead code
- No TODOs, FIXMEs, or stub implementations
- Clean env resolution pattern: trim input and empty-check before returning
- Comprehensive test coverage including explicit regression guard on deprecated gemini-1.5
- Panic on YAML parse failure is intentional (fail-fast on configuration deployment error)
- All env key construction uses `.to_uppercase()` on provider string (guard against case mismatches)
