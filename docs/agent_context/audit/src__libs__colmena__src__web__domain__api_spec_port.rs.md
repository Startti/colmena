# src/libs/colmena/src/web/domain/api_spec_port.rs

**Layer:** domain  **Purpose:** Defines the port contract for fetching and parsing OpenAPI 3.x / Swagger 2.0 specs, and domain value objects representing normalized parsed specs with full endpoint/parameter/security metadata.

## Symbols

- `ApiSpecPort` (trait, pub) — async port for fetching spec at URL with conditional GET (ETag/Last-Modified) revalidation
- `SpecFetchResult` (enum, pub) — result of spec fetch: Fresh (spec + cache headers) or NotModified
- `ParsedSpec` (struct, pub) — normalized domain representation of a parsed spec (endpoints, servers, schemas, security schemes)
- `SpecFormat` (enum, pub) — original spec format: OpenApi3x or Swagger20
- `SpecFormat::as_str()` (fn, pub) — formats SpecFormat as "openapi-3.x" or "swagger-2.0"
- `HttpMethod` (enum, pub) — HTTP verbs: Get, Put, Post, Delete, Options, Head, Patch, Trace
- `HttpMethod::as_str()` (fn, pub) — converts HttpMethod to uppercase verb string
- `HttpMethod::parse()` (fn, pub) — parses case-insensitive string to HttpMethod (returns None for invalid)
- `Endpoint` (struct, pub) — represents a single API endpoint with operation_id, method, path, params, request/response bodies, security
- `ParameterSpec` (struct, pub) — represents a path/query/header parameter with name, type, required flag, style/explode
- `ParamType` (enum, pub) — parameter type: String, Integer, Number, Boolean, Array(Box), Object, Unknown (opaque for unrecognized)
- `RequestBodySpec` (struct, pub) — request body metadata: content_type, required flag, and verbatim JSON-Schema describing the body
- `ResponseSpec` (struct, pub) — response metadata: description and map of content_type → schema
- `ApiKeyLocation` (enum, pub) — where API key is placed: Header, Query, Cookie
- `SecurityScheme` (enum, pub) — security scheme types: Http (scheme+bearer_format), ApiKey (name+location), OAuth2 (flows metadata), OpenIdConnect (url)
- `SecurityRequirement` (struct, pub) — references a security scheme by name and lists required scopes
- `tests` (mod, private) — unit tests for HttpMethod parsing/roundtrip, SpecFormat string stability, ParsedSpec clone

## File-level notes

- Well-structured port trait with clear async contract and HTTP caching semantics (ETag/Last-Modified conditional GET).
- Domain value objects preserve both `input_url` (as given by agent, for errors) and `resolved_url` (post-normalization).
- `components_schemas` stored verbatim to support ref-resolution in infrastructure layer before exposing to LLM (avoids `#/components/schemas/X` strings reaching model).
- `#[allow(clippy::large_enum_variant)]` acknowledged for `SpecFetchResult::Fresh` containing `ParsedSpec`; design choice documented.
- Comprehensive test coverage: HttpMethod parsing (case-insensitive, roundtrip, invalid), SpecFormat string stability, ParsedSpec cloning.
- No external-crate dependencies except `async_trait`, `serde_json`; pure domain abstractions with zero infrastructure coupling.
