# src/libs/colmena/src/llm/infrastructure/attachment_summary/mod.rs

**Layer:** infrastructure  
**Purpose:** Module re-export hub for attachment summary adapters and infrastructure: text extraction, byte acquisition, cheap-tier LLM provider mapping, and the LLM-backed summary generator.

## Symbols

- `byte_acquisition` (mod, public) — submodule for byte acquisition adapters
- `cheap_tier` (mod, public) — submodule for provider cheap-tier mapping logic
- `llm_summary_generator` (mod, public) — submodule for LLM-backed summary generator implementation
- `text_extractor` (mod, public) — submodule for text extraction and character truncation
- `acquire_bytes` (fn, public) — acquires attachment bytes via the byte acquisition adapter [re-exported from byte_acquisition]
- `AcquireError` (type, public) — error type for byte acquisition failures [re-exported from byte_acquisition]
- `provider_cheap_tier` (fn, public) — maps an LLM provider to its cheap-tier variant [re-exported from cheap_tier]
- `LlmAttachmentSummaryGenerator` (struct, public) — the primary LLM-backed attachment summary generator implementation [re-exported from llm_summary_generator]
- `extract_text` (fn, public) — extracts text content from attachment bytes [re-exported from text_extractor]
- `truncate_chars` (fn, public) — truncates text to a maximum character count [re-exported from text_extractor]
- `ExtractError` (type, public) — error type for text extraction failures [re-exported from text_extractor]

## File-level notes

- Purely a re-export barrel/aggregation module; all business logic resides in submodules.
- Well-structured organization: each concern (byte acquisition, provider selection, LLM generation, text extraction) isolated into its own submodule.
- Module-level documentation clearly describes the purpose of each submodule.
- No implementation code, no tests, no dead or stub code.
