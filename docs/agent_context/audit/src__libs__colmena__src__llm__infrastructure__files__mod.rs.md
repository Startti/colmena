# src/libs/colmena/src/llm/infrastructure/files/mod.rs

**Layer:** infrastructure  **Purpose:** Module facade and re-export barrel for LLM provider file APIs and cache adapters. Aggregates Anthropic, Gemini, OpenAI file handling plus persistent Postgres cache and signed URL downloads.

## Symbols

- `anthropic_files_api` (mod, pub) — Submodule for Anthropic Files API adapter implementation
- `file_provider_factory` (mod, pub) — Submodule for factory pattern to instantiate provider-specific file adapters
- `gemini_files_api` (mod, pub) — Submodule for Gemini Files API adapter implementation
- `openai_files_api` (mod, pub) — Submodule for OpenAI Files API adapter implementation
- `postgres_file_cache` (mod, pub) — Submodule for Postgres-backed file cache persistence layer
- `signed_url_downloader` (mod, pub) — Submodule for downloading file content from signed URLs
- `AnthropicFilesApiAdapter` (type, pub re-export) — Adapter implementing Anthropic Files API integration
- `FileProviderFactory` (type, pub re-export) — Factory for creating provider-specific file API adapters
- `GeminiFilesApiAdapter` (type, pub re-export) — Adapter implementing Gemini Files API integration
- `OpenAiFilesApiAdapter` (type, pub re-export) — Adapter implementing OpenAI Files API integration
- `PostgresFileCache` (type, pub re-export) — Postgres-backed cache for persisted file data
- `SignedUrlDownloader` (type, pub re-export) — Utility for downloading content from signed URLs

## File-level notes

- Pure module barrel file with no inline implementations, logic, or complex code.
- All public symbols are re-exports of types from submodules; no local definitions.
- Consistent naming: adapter per provider, factory pattern for instantiation, Postgres for persistence, utility for downloads.
- No dead code, unfinished implementations, or architectural improvements visible.
- Module documentation is in Spanish; consistent with project convention.
