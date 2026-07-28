# src/libs/colmena/src/llm/infrastructure/mod.rs

**Layer:** infrastructure  **Purpose:** Aggregates LLM provider adapters (Anthropic, OpenAI, Gemini), TTS adapters (Google, ElevenLabs, OpenAI), conversation persistence, and supporting utilities; exposes a clean public interface for the application layer.

## Symbols

### Module Declarations
- `anthropic_adapter` (mod, pub) — Anthropic LLM provider adapter implementation
- `attachment_summary` (mod, pub) — Attachment content summarization infrastructure
- `attachments` (mod, pub) — Attachment handling and storage integration
- `cheap_models` (mod, pub) — Cheap model fallback selection logic for cost optimization
- `elevenlabs_tts_adapter` (mod, pub) — ElevenLabs text-to-speech provider adapter
- `files` (mod, pub) — File handling and caching utilities
- `gemini_adapter` (mod, pub) — Google Gemini LLM provider adapter implementation
- `google_tts_adapter` (mod, pub) — Google Cloud text-to-speech provider adapter
- `llm_provider_factory` (mod, pub) — Factory pattern for instantiating LLM providers and override guards
- `message_summarizer` (mod, pub) — Conversation message summarization logic
- `mock_adapter` (mod, pub) — Mock LLM adapter for testing without API calls
- `openai_adapter` (mod, pub) — OpenAI LLM provider adapter implementation
- `openai_tts_adapter` (mod, pub) — OpenAI text-to-speech provider adapter
- `persistence` (mod, pub) — Conversation and session persistence repository implementations
- `scripted_adapter` (mod, pub) — Scripted/deterministic LLM adapter for testing and workflows
- `tts_provider_factory` (mod, pub) — Factory pattern for instantiating TTS providers

### Public Re-exports (Types & Functions)
- `AnthropicAdapter` (type, pub) — Anthropic LLM provider adapter
- `cheap_model_for` (fn, pub) — Selects a cheap fallback model for cost-optimized inference
- `ElevenLabsTtsAdapter` (type, pub) — ElevenLabs TTS provider adapter
- `GeminiAdapter` (type, pub) — Google Gemini LLM provider adapter
- `GoogleTtsAdapter` (type, pub) — Google Cloud TTS provider adapter
- `LlmProviderFactory` (type, pub) — Factory for instantiating LLM providers
- `OverrideGuard` (type, pub) — Guard pattern for provider-level parameter overrides
- `MockAdapter` (type, pub) — Mock LLM adapter for deterministic testing
- `OpenAiAdapter` (type, pub) — OpenAI LLM provider adapter
- `OpenAiTtsAdapter` (type, pub) — OpenAI TTS provider adapter
- `ConversationRepositoryFactory` (type, pub) — Factory trait for conversation persistence repositories
- `PostgresConversationRepository` (type, pub) — PostgreSQL conversation persistence implementation
- `SqliteConversationRepository` (type, pub) — SQLite conversation persistence implementation
- `ScriptedAdapter` (type, pub) — Scripted LLM adapter for deterministic workflows
- `ScriptedResponse` (type, pub) — Response envelope from scripted adapter
- `build_tts_repository` (fn, pub) — Factory function for instantiating TTS providers

## File-level notes
- This is a clean module façade with no business logic — purely organizational.
- All 16 submodules are active and re-exported, indicating a mature, well-structured infrastructure layer.
- The public interface balances modularity (per-adapter, per-persistence) with usability (centralized re-exports).
- No dead code, unfinished stubs, or obvious improvements detected.
