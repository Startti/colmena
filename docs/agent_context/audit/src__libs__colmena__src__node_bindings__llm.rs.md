# src/libs/colmena/src/node_bindings/llm.rs

**Layer:** bindings  **Purpose:** Exposes the core LLM functionality (call, stream, health check, provider management) to TypeScript/Node.js via napi-rs bindings. Bridges the application-layer LLM use cases and multi-provider infrastructure to the Node.js runtime.

## Symbols

- `NodeLlmConfigOptions` (struct, pub) — napi object holding optional LLM configuration parameters (api_key, model, temperature, max_tokens, top_p, frequency_penalty, presence_penalty) with Clone and Default derives
- `NodeLlmMessage` (struct, pub) — napi object representing a single conversation message with role (string) and content (string)
- `ColmenaLlm` (struct, pub) — main napi class wrapping provider-agnostic LLM functionality; stores cached Arc-wrapped ServiceContainer instances keyed by provider name
- `ColmenaLlm::new()` (fn, pub) — napi constructor that loads environment configuration via ConfigResolver and initializes all multi-provider service containers via ServiceContainerFactory
- `ColmenaLlm::call()` (fn, pub) — async napi method that executes a single non-streaming LLM call; parses provider, resolves container, converts NodeLlmMessage to domain LlmMessage, merges config options, delegates to llm_call use case, returns response content as string
- `ColmenaLlm::stream()` (fn, pub) — async napi method that initiates a streaming LLM call; mirrors call() setup logic then delegates to llm_stream use case and wraps result in LlmStreamHandle
- `ColmenaLlm::health_check()` (fn, pub) — async napi method that checks provider health; resolves container, invokes llm_health_check use case, returns boolean health status
- `ColmenaLlm::get_providers()` (fn, pub) — napi method that returns list of all available provider names from cached container keys

## File-level notes

- **Improvement: code duplication** — `call()` and `stream()` share ~12 lines of identical setup logic (provider parsing, container lookup, message conversion, config creation). Candidate for extraction into a private helper method to reduce duplication and improve maintainability.
- **Type boundary well-guarded**: conversion from napi types (NodeLlmMessage, NodeLlmConfigOptions) to domain types (LlmMessage, LlmConfig) is centralized, with explicit error mapping at each boundary via `map_err()`.
- **Arc caching strategy** — ServiceContainer instances are wrapped in Arc and cached in HashMap on construction; sound for thread-safety across napi async calls.
- **No panics or unwrap()** — all error paths properly mapped to napi Result<T> and Status codes (InvalidArg, GenericFailure).
