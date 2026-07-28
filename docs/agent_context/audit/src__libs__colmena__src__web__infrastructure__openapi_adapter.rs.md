# src/libs/colmena/src/web/infrastructure/openapi_adapter.rs

**Layer:** infrastructure  
**Purpose:** Reqwest-backed implementation of `ApiSpecPort` trait for fetching and parsing OpenAPI/Swagger specifications from remote URLs. Handles URL normalization, streaming with size caps, format detection (JSON/YAML), version detection (OpenAPI 3.x vs Swagger 2.0), automatic conversion, and ETag-based caching.

## Symbols

- `OpenApiAdapterConfig` (struct, pub) — configuration container with `max_bytes` and `timeout` limits for the fetch pipeline
- `OpenApiAdapterConfig::default()` (impl, pub) — default config: 10 MB limit, 60-second timeout
- `OpenApiAdapter` (struct, pub) — holds `reqwest::Client` and config; main adapter entry point
- `OpenApiAdapter::new()` (fn, pub) — constructor that builds a configured reqwest client with timeout and user-agent
- `OpenApiAdapter::fetch_raw()` (async fn, pub(crate)) — lower-level fetch with streaming body, size enforcement, HTML detection, and conditional GET support; returns metadata (content-type, ETag, Last-Modified, resolved URL)
- `FetchRawResult` (enum, pub(crate)) — result type for `fetch_raw`: `Fresh` (with body + headers) or `NotModified` (304 response)
- `BodyFormat` (enum, pub(crate)) — detected format: `Json` or `Yaml`
- `detect_body_format()` (fn, pub(crate)) — sniff first non-whitespace byte to classify format (assumes `{` or `[` → JSON, else YAML)
- `detect_spec_kind()` (fn, pub(crate)) — detect spec version by checking root keys (`openapi` for 3.x, `swagger` for 2.0); returns `SpecFormat` or `UnsupportedSpecFormat` error
- `parse_body_to_spec()` (fn, pub(crate)) — orchestrator: parse raw bytes (JSON or YAML) to `ParsedSpec`, dispatches to OpenAPI 3.x or Swagger 2.0 handler with automatic conversion
- `parse_oas3_value()` (fn, private) — internal: deserialize `oas3::OpenApiV3Spec`, extract top-level fields (title, version, description, servers, tags), and dispatch to endpoint/security extraction
- `extract_endpoints()` (fn, private) — walk JSON `paths` object, merge path-level and operation-level parameters (op-level wins on conflict), classify params by location (path/query/header), extract request body, responses, and security requirements per operation
- `generate_operation_id()` (fn, private) — derive stable operation ID by combining HTTP method prefix with sanitized path (replaces `/`, `{`, `}`, `-` with `_`)
- `parameter_from_json()` (fn, private) — extract `ParameterSpec` from parameter JSON object (name, description, required, schema type, style, explode)
- `classify_schema()` (fn, private) — recursively classify parameter type from JSON schema (string/integer/number/boolean/array/object/unknown, with special handling for `$ref`)
- `request_body_from_json()` (fn, private) — extract `RequestBodySpec` from requestBody JSON, preferring JSON content-type, falling back to form-urlencoded/multipart/first available
- `response_spec_from_json()` (fn, private) — extract `ResponseSpec` from response JSON object (description and content media types with schemas)
- `security_requirements_from_array()` (fn, private) — parse security requirement array into `Vec<SecurityRequirement>` with scheme name and scopes
- `extract_security_schemes()` (fn, private) — walk `components.securitySchemes` and map each scheme by type (http, apiKey, oauth2, openIdConnect) to `SecurityScheme` variant
- `ApiSpecPort for OpenApiAdapter` (impl) — async `fetch_and_parse()` trait method that chains `fetch_raw` → `parse_body_to_spec`, returning `SpecFetchResult`
- `tests_parse_openapi3` (mod, test) — unit tests for format detection (JSON braces, YAML fallback), spec kind detection (OpenAPI 3.x, Swagger 2.0, rejection of AsyncAPI), and petstore YAML parsing end-to-end
- `tests_fetch` (mod, test) — integration tests using wiremock mock server: fetch returns YAML with ETag, rejects HTML by content-type, sniffs HTML when content-type absent, enforces size cap, propagates conditional-get If-None-Match header, maps 500 to upstream error
- `tests_parse_swagger2` (mod, test) — Swagger 2.0 conversion tests: petstore 2.0 YAML roundtrips to OpenAPI 3.0 format with server and security scheme preservation; conditional GET with ETag 304 support

## File-level notes

- **Clean infrastructure layer**: `ApiSpecPort` trait fully implemented; all domain dependencies injected via imports from `web::domain` (error types, data structures). Zero application/domain logic leaks into this adapter.
- **Robust error handling**: timeout vs upstream error distinction (line 81), HTML rejection at two levels (content-type header + body sniff for missing header), streaming with per-chunk size validation, parse errors with context strings.
- **Format flexibility**: dual JSON/YAML parsing with content-type hint + body format detection fallback; Swagger 2.0 → OpenAPI 3.0 automatic conversion before parsing (delegated to separate module).
- **ETag / Last-Modified support**: conditional GET propagated from caller into reqwest headers; 304 response handled and returned to caller (caching responsibility on application layer).
- **Parameter merging logic** (line 328–373): path-level parameters inherited by all operations under the path; operation-level parameters override path-level by name+location. Implemented via `retain` + `push` pattern; intentional but dense.
- **Generous fallbacks**: missing schema → Null → ParamType::Unknown (safe); missing operationId → generated from method+path (stable); missing content-type → body format sniff (defensive); missing request-body content → first available or None (graceful).
- **Test structure**: three test modules organized by pipeline stage (parse OpenAPI 3.x, fetch HTTP behavior, parse Swagger 2.0 conversion); uses wiremock for HTTP mocking and includes fixture YAML specs.
- **No dependencies on application/domain logic**: pure infrastructure adapter; all orchestration (trying JSON then YAML, dispatching to appropriate parser) happens in public functions, not impl trait methods.
