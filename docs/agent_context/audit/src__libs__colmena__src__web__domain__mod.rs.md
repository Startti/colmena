# src/libs/colmena/src/web/domain/mod.rs

**Layer:** domain  
**Purpose:** Module root providing the public interface to the web toolkit's domain-layer ports (API spec parsing, web search) and value objects (session management). No logic; re-exports only.

## Symbols

### Modules (public)
- `api_spec_port` (mod, pub) — Domain port trait for fetching and parsing API specifications
- `errors` (mod, pub) — Domain error types for web toolkit operations
- `search_port` (mod, pub) — Domain port trait for web search integration
- `session` (mod, pub) — Value objects for conversation session management (registry, keys, entries)

### Re-exported types
- `ApiKeyLocation` (from api_spec_port) — Enum: location of API authentication in a request (header, query, etc.)
- `ApiSpecPort` (from api_spec_port) — Trait: port for API specification fetching and parsing
- `Endpoint` (from api_spec_port) — Value object: metadata and parameters for an API endpoint
- `HttpMethod` (from api_spec_port) — Enum: HTTP method (GET, POST, etc.)
- `ParamType` (from api_spec_port) — Enum: parameter data type in spec (string, integer, etc.)
- `ParameterSpec` (from api_spec_port) — Value object: parameter specification (name, type, required, etc.)
- `ParsedSpec` (from api_spec_port) — Value object: fully parsed API specification with endpoints and security
- `RequestBodySpec` (from api_spec_port) — Value object: request body schema for an endpoint
- `ResponseSpec` (from api_spec_port) — Value object: response schema for an endpoint
- `SecurityRequirement` (from api_spec_port) — Value object: required security scheme for an endpoint
- `SecurityScheme` (from api_spec_port) — Value object: security scheme definition (type, location, etc.)
- `SpecFetchResult` (from api_spec_port) — Enum: result of fetching a spec (success, error, format variants)
- `SpecFormat` (from api_spec_port) — Enum: API spec format (OpenAPI 3.0, 3.1, etc.)
- `WebDomainError` (from errors) — Enum: domain error type for web operations
- `ExtractFormat` (from search_port) — Enum: format to extract from search results (html, markdown, etc.)
- `FetchRequest` (from search_port) — Value object: URL and configuration for fetching content
- `FetchResponse` (from search_port) — Value object: fetched content with metadata
- `SearchDepth` (from search_port) — Enum: search depth level (basic, detailed, comprehensive)
- `SearchPort` (from search_port) — Trait: port for web search integration
- `SearchRequest` (from search_port) — Value object: search query and parameters
- `SearchResponse` (from search_port) — Value object: search results with metadata
- `SearchResult` (from search_port) — Value object: single search result (title, URL, snippet)
- `TimeRange` (from search_port) — Enum: time range filter for search
- `ConversationId` (from session) — Value object: unique conversation identifier
- `SessionEntry` (from session) — Value object: entry in session registry (ID, timestamp, TTL, etc.)
- `SessionKey` (from session) — Value object: key to look up sessions (agent session ID + conversation ID)
- `SessionRegistry` (from session) — Value object: registry of active conversation sessions
- `TtlConfig` (from session) — Value object: time-to-live configuration for session entries

## File-level notes

- **Pure re-export module:** Contains only module declarations and re-exports; no implementation logic.
- **Clear separation:** Submodules align with domain responsibilities: spec parsing (ports), errors, search, and session state.
- **Standard Rust pattern:** Well-organized public API surface that hides implementation details of each submodule.
- **No dead code:** All re-exports are intentional and comprise the module's public interface.
