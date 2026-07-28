# src/libs/colmena/src/llm/domain/llm_error.rs

**Layer:** domain  
**Purpose:** Defines the `LlmError` enum for all LLM domain operations, covering API/provider/configuration/network/parsing/tool/file-handling errors with semantic variants and convenience constructors.

## Symbols

- `LlmError` (enum, pub) — Error type with 30+ variants covering: API keys, provider support, request failures, configuration validation (temperature, tokens, top_p, penalties), networking, parsing, rate limits, message validation, tool execution, ReAct iterations, and file API limits (30 MB data/path fields, document IDs, MIME types, signed URLs, provider uploads)
- `impl LlmError` (impl block, pub) — 11 convenience constructor methods using `impl Into<String>` for variants with string fields: `request_failed`, `network_error`, `parsing_error`, `internal_error`, `invalid_message_role`, `too_many_system_messages`, `provider_limitation`, `tool_not_found`, `tool_execution_failed`, `invalid_tool_call`, `max_iterations_reached`
- `files_error_tests` (mod, private) — Test module covering 3 file-related error variants (`DataFieldTooLarge`, `UrlWithoutDocumentId`, `ProviderFileNotFound`)

## File-level notes

- **Inconsistent constructor coverage:** Variants like `InvalidApiKey`, `EmptyMessages`, `EmptyMessageContent`, `MaxTokensIsZero`, `RateLimitExceeded` lack dedicated constructors despite other simple variants having helpers; inconsistency could hinder ergonomics [FLAG: improvement]
- **Semantic duplication:** Five variants have identical structure (`{ message: String }`): `RequestFailed`, `NetworkError`, `ParsingError`, `InternalError`, `ToolExecutionFailed`; duplication is intentional for error semantics clarity but could be consolidated if strict DRY is prioritized [FLAG: improvement]
- **Minimal test coverage:** Only 3 of 30+ variants tested; significant variants like configuration errors (`InvalidTemperature`, `InvalidTopP`, etc.), message validation (`ConsecutiveRoles`, `TooManySystemMessages`), and tool errors lack coverage [FLAG: improvement]
- **No doc comments:** Public enum and variants lack `///` doc comments for API documentation; improves IDE discoverability and generated rustdoc
- **Derives are appropriate:** `#[derive(Debug, Error, PartialEq)]` is correct for a domain error type; `thiserror::Error` with `#[error(...)]` messages provides ergonomic Display impl
