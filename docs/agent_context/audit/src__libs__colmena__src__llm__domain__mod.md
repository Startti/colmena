# src/libs/colmena/src/llm/domain/mod.rs

**Layer:** domain  **Purpose:** Public API facade for the LLM domain layer, re-exporting types, traits, and errors from submodules to provide a clean interface to application and infrastructure layers.

## Symbols

### Module declarations
- `attachments` (mod, pub) — Domain types for attachment registry, sources, and conversation attachment persistence
- `file_cache_repository` (mod, pub) — File caching repository trait and cache entry type
- `file_provider_factory_port` (mod, pub) — File provider factory port trait for stream-based file loading
- `file_provider_repository` (mod, pub) — File provider repository trait for multi-source file access
- `llm_config` (mod, pub) — LLM configuration and token usage tracking types
- `llm_error` (mod, pub) — Error types for LLM operations
- `llm_message` (mod, pub) — Message types for LLM communication (roles, content, file data)
- `llm_provider` (mod, pub) — LLM provider definitions and provider kind enum
- `llm_repository` (mod, pub) — LLM repository trait (main port for LLM providers)
- `llm_request` (mod, pub) — LLM request types and message builders
- `llm_response` (mod, pub) — LLM response types, stream chunks, and suspend info
- `memory` (mod, pub) — Conversation memory, session management, and persistence types
- `message_summarizer` (mod, pub) — Message summarizer trait for conversation compression
- `signed_url_fetcher` (mod, pub) — Signed URL fetcher trait for secure file retrieval
- `tool_executor` (mod, pub) — Tool executor trait for executing LLM tool calls
- `tools` (mod, pub) — Tool definitions, calls, results, and parameter schemas
- `tts` (mod, pub) — Text-to-speech request/response types and audio formats
- `tts_repository` (mod, pub) — TTS repository trait and error type
- `value_objects` (mod, pub) — Shared domain value objects (re-exported via wildcard)

### Re-exported types from attachments
- `AttachmentError` (type, pub) — Error type for attachment operations
- `AttachmentRegistry` (type, pub) — Registry for managing conversation attachments
- `AttachmentSource` (enum, pub) — Source type for attachments (inline, URL, etc.)
- `ConversationAttachment` (struct, pub) — Persisted attachment metadata
- `StaleAttachmentQuery` (type, pub) — Query type for identifying stale attachments
- `UpsertAttachmentInput` (struct, pub) — Input for creating/updating attachments

### Re-exported types from file_cache_repository
- `CachedFileEntry` (struct, pub) — Cache entry for downloaded files
- `FileCacheRepository` (trait, pub) — File caching repository trait

### Re-exported types from file_provider_factory_port
- `FileProviderFactoryPort` (trait, pub) — Factory for creating file provider instances

### Re-exported types from file_provider_repository
- `BoxedByteStream` (type, pub) — Boxed stream of bytes for file reading
- `FileProviderRepository` (trait, pub) — File provider repository for multi-source access

### Re-exported types from llm_config
- `LlmConfig` (struct, pub) — Configuration for LLM provider, model, and parameters
- `LlmUsage` (struct, pub) — Token usage tracking (input, output, cache)

### Re-exported types from llm_error
- `LlmError` (enum, pub) — Error type for all LLM operations

### Re-exported types from llm_message
- `FileData` (struct, pub) — Metadata for file content in messages
- `FileSource` (enum, pub) — Source of file data (inline base64, URL, etc.)
- `LlmMessage` (struct, pub) — Message in LLM conversation (role, content, attachments)
- `MessageRole` (enum, pub) — Role of message sender (user, assistant, system, tool)
- `ProviderFileRef` (struct, pub) — Provider-specific file reference
- `is_text_like` (fn, pub) — Helper to check if message content is text-based

### Re-exported types from llm_provider
- `LlmProvider` (struct, pub) — LLM provider configuration
- `ProviderKind` (enum, pub) — Provider type (OpenAI, Anthropic, Gemini, etc.)

### Re-exported types from llm_repository
- `LlmRepository` (trait, pub) — Main trait for LLM provider adapters
- `LlmStream` (type, pub) — Stream type for LLM responses
- `MockLlmRepository` (type, pub) — Mock implementation for testing (cfg(test) only)

### Re-exported types from llm_request
- `LlmRequest` (struct, pub) — Request to send to LLM provider

### Re-exported types from llm_response
- `LlmResponse` (struct, pub) — Complete response from LLM
- `LlmStreamChunk` (struct, pub) — Chunk in SSE stream
- `LlmStreamPart` (enum, pub) — Part of a stream chunk (message, tool call, suspend, etc.)
- `SuspendInfo` (struct, pub) — Suspension details (node id, question, options)
- `ToolCallChunk` (struct, pub) — Tool call chunk for streaming tool invocations

### Re-exported types from memory
- `AgentSessionId` (type, pub) — Agent session identifier
- `Conversation` (struct, pub) — Conversation metadata and history
- `ConversationKey` (struct, pub) — Key for conversation lookup
- `ConversationRepository` (trait, pub) — Conversation persistence trait
- `NodeIdPath` (type, pub) — Path to a node in DAG
- `SessionId` (type, pub) — Session identifier
- `StoredMessage` (struct, pub) — Message as persisted in conversation history

### Re-exported types from message_summarizer
- `MessageSummarizer` (trait, pub) — Trait for summarizing conversation messages

### Re-exported types from signed_url_fetcher
- `SignedUrlFetcher` (trait, pub) — Trait for fetching content from signed URLs

### Re-exported types from tool_executor
- `ToolExecutor` (trait, pub) — Trait for executing LLM tool calls

### Re-exported types from tools
- `FunctionCall` (struct, pub) — Function call details with arguments
- `ParameterProperty` (struct, pub) — JSON schema property for tool parameter
- `ToolCall` (struct, pub) — Tool call from LLM with id and function
- `ToolDefinition` (struct, pub) — Tool definition for LLM to call
- `ToolParameters` (struct, pub) — Tool parameter schema (JSON schema)
- `ToolResult` (struct, pub) — Result of executing a tool

### Re-exported types from tts
- `AudioFormat` (enum, pub) — Audio format for TTS output
- `TtsRequest` (struct, pub) — Text-to-speech request
- `TtsResponse` (struct, pub) — TTS response with generated audio

### Re-exported types from tts_repository
- `TtsError` (enum, pub) — Error type for TTS operations
- `TtsRepository` (trait, pub) — TTS provider trait
- `MockTtsRepository` (type, pub) — Mock implementation for testing (cfg(test) only)

### Re-exported from value_objects
- (wildcard re-export `pub use value_objects::*;`)

## File-level notes

- **All modules exist and are actively imported** across the codebase (dag_engine, infrastructure, nodes)
- **Clean module facade** — no logic, only declarations and re-exports
- **Conditional compilation** for test mocks (MockLlmRepository, MockTtsRepository) is appropriate
- **Wildcard re-export** on value_objects (line 54) follows Rust barrel-file convention; acceptable for a domain facade
- **No unfinished code** — all submodules are complete and exported
- **No dead symbols** — all re-exported types are used throughout the application and infrastructure layers
- **Consistent structure** — domain layer properly separates ports (traits) from value objects; all public items are either types, traits, or enums with no runtime logic
