# src/libs/colmena/src/dag_engine/infrastructure/sql_llm_critic.rs

**Layer:** infrastructure  
**Purpose:** Provides an LLM-based adapter for analyzing SQL queries. Implements the `SqlCriticPort` trait to send queries to a secondary LLM for security and optimization analysis when `guardrail_llm.enabled: true` in node config.

## Symbols

- `LlmCriticAdapter` (struct, pub) — Holds provider name, model, and API key for LLM critic requests
- `impl LlmCriticAdapter` (impl, pub) — Concrete implementation block
- `new` (fn, pub) — Constructor creating a new adapter with provider, model, and API key
- `CRITIC_SYSTEM_PROMPT` (const, private) — System prompt loaded from `text/prompts/sql_llm_critic.md` via `include_str!`
- `SqlCriticPort for LlmCriticAdapter` (impl, async trait) — Async trait implementation for the domain port
- `analyze` (async fn, pub) — Analyzes SQL query by building LLM request, calling provider, and parsing JSON response; returns `CriticResult` with security status and optimization hints [FLAG: improvement — silent JSON parsing failure (line 88–95) masks malformed LLM responses; should log/warn when unwrap_or_else fallback is triggered]

## File-level notes

- **Error handling strategy**: Explicit `.map_err()` chains on lines 60–72 are verbose but clear and consistent
- **Fail-open design**: Lines 88–95 intentionally default to "security: ok" if LLM response doesn't parse as JSON, but this silently masks malformed responses without any logging or diagnostic output
- **Cloning**: Lines 52–54 clone `provider_kind`, `api_key`, and `model` to satisfy `LlmProvider::new()` ownership; clones appear necessary since `provider_kind` is reused on line 81
- **No unused imports or symbols detected**
- **No TODO/FIXME/unimplemented!() markers**
