# src/libs/colmena/src/llm/infrastructure/attachment_summary/cheap_tier.rs

**Layer:** infrastructure  
**Purpose:** Maps each LLM provider to its cheap-tier model name for attachment summary generation. Single function with four tests, centralized for easy updates when providers ship cheaper variants.

## Symbols

- `provider_cheap_tier` (fn, pub) — Returns the default cheap-tier model name for a given ProviderKind, matching on Google, OpenAi, Anthropic, Mock, and Generated (sentinel placeholder)
- `tests` (mod, private, cfg(test)) — Test module for cheap-tier model mappings
  - `google_cheap_tier_is_gemini_flash` (fn, test) — Asserts Google variant maps to "gemini-2.5-flash"
  - `openai_cheap_tier_is_gpt4o_mini` (fn, test) — Asserts OpenAi variant maps to "gpt-4o-mini"
  - `anthropic_cheap_tier_is_haiku` (fn, test) — Asserts Anthropic variant maps to "claude-haiku-4-5-20251001"
  - `mock_cheap_tier_is_mock` (fn, test) — Asserts Mock variant maps to "mock-model"

## File-level notes

- All four production providers (Google, OpenAi, Anthropic, Mock) have explicit test coverage.
- Generated variant is defensively handled with a sentinel placeholder string; comment correctly explains it is a no-op case for output-storage rows and should never reach an LLM call.
- No dead code, unfinished stubs, or obvious improvements.
