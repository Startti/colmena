# src/libs/colmena/src/web/application/api_spec_use_case.rs

**Layer:** application  
**Purpose:** Orchestrates fetch, cache, search, and build operations for OpenAPI/REST API specifications within a conversation scope. Provides session-registry-backed caching around an `ApiSpecPort` domain port and implements multiple accessor functions (list endpoints, search, get details, build HTTP requests).

## Symbols

### Core Types
- `ApiSpecUseCaseConfig` (struct, pub) — Tunables for the use case (enable_cache, cache_ttl, max_cached_specs, fuzzy_match_threshold, default_base_url_override) with sensible Spec C defaults
- `Default` impl for `ApiSpecUseCaseConfig` — Sets cache TTL to 86400s, max specs to 100, fuzzy threshold to 0.1
- `SpecCache` (struct, pub) — Per-conversation LRU cache of parsed specs, thread-safe via Mutex
- `SpecCache::new()` (fn, pub) — Creates a new spec cache with given capacity
- `CachedSpec` (struct, pub) — Wrapper holding parsed spec, etag, last_modified timestamp, and cache insertion time
- `ApiSpecUseCase` (struct, pub) — Main use case holding port, registry, and config

### ApiSpecUseCase Methods
- `new()` (fn, pub) — Constructor for the use case
- `fetch_spec()` (async fn, pub) — Fetches or reuses a spec for a conversation, managing cache TTL and HTTP 304 revalidation
- `lookup_cached()` (async fn, pub) — Looks up a previously-fetched spec; returns `SpecNotLoaded` error when missing (recoverable for LLM)
- `list_endpoints()` (async fn, pub) — Conversation-scoped wrapper around `list_endpoints()` free function
- `search_endpoint()` (async fn, pub) — Conversation-scoped wrapper around `search_endpoint()` free function with fuzzy matching
- `get_endpoint_details()` (async fn, pub) — Conversation-scoped wrapper around `get_endpoint_details()` free function
- `build_http_request()` (async fn, pub) — Conversation-scoped wrapper around `build_http_request()` free function
- `registry()` (fn, pub) — Public accessor for the registry Arc (for tests and lifecycle subscription)

### Data Types
- `EndpointListPage` (struct, pub) — Paginated list of endpoints (total, returned, offset, endpoints)
- `EndpointSummary` (struct, pub) — Compact endpoint representation (operation_id, method, path, summary, tags)
- `From<&Endpoint>` impl for `EndpointSummary` — Converts domain Endpoint to summary for list responses
- `EndpointSearchHit` (struct, pub) — Search result with score and match reason (operation_id, method, path, summary, score, match_reason)

### Free Functions — Listing & Search
- `list_endpoints()` (fn, pub) — Filters endpoints by optional tag and paginates results; returns `EndpointListPage`
- `search_endpoint()` (fn, pub) — Fuzzy-searches endpoints using nucleo_matcher; tokenizes query and scores against concatenated searchable string; filters by optional method; returns top-N hits above threshold

### Free Functions — Details & Schema
- `get_endpoint_details()` (fn, pub) → `Result<Value, WebDomainError>` — Returns verbose JSON description of endpoint's parameters, request body, responses, security; includes fuzzy suggestions on 404
- `params_to_json()` (nested fn, priv) — Converts ParameterSpec array to JSON array with name, type, required, description, style, explode
- `param_type_str()` (nested fn, priv) — Maps ParamType enum to string (string, integer, number, boolean, array, object, unknown → string)
- `resolve_refs()` (fn, pub(crate)) — Walks JSON schema recursively and inlines `#/components/schemas/X` refs; breaks cycles via path tracking and emits `x-cycle-to` placeholder
- `explain_match()` (fn, priv) — Formats human-readable match reason for search results; checks path, summary, operation_id for exact substring matches; falls back to "fuzzy match across..."

### Free Functions — HTTP Request Building
- `build_http_request()` (fn, pub) → `Result<Value, WebDomainError>` — Given endpoint, params, auth secret ref, and optional base_url_override, emits JSON for the `http_request` node with url, method, headers, query_params, body
- `coerce_scalar()` (fn, priv) → `Result<String, WebDomainError>` — Coerces a JSON Value to a declared ParamType; handles string↔integer, string↔number, bool↔string("true"/"false") conversions
- `encode_param()` (fn, priv) → `Result<Value, WebDomainError>` — Encodes array/scalar parameters per OpenAPI style/explode rules; handles form, spaceDelimited, pipeDelimited serialization
- `build_body()` (fn, priv) — Content-type dispatcher; routes to json, form-urlencoded, or multipart builder
- `build_body_json()` (fn, priv) — Assembles JSON request body from params; collects required and extra properties
- `build_body_form_urlencoded()` (fn, priv) — Assembles form-encoded body with percent-encoding
- `build_body_multipart()` (fn, priv) — Marks body as multipart by inserting `__multipart: true` flag
- `required_fields_from_schema()` (fn, priv) — Extracts `required` array from JSON schema
- `schema_property_names()` (fn, priv) — Extracts `properties` object keys from JSON schema
- `percent_encode()` (fn, priv) — RFC 3986 percent-encoding (preserves unreserved a-z A-Z 0-9 - . _ ~)
- `apply_security_scheme()` (fn, priv) — Injects auth headers/query params; supports Http (Basic/Bearer), ApiKey (header/query/cookie), OAuth2, OpenIdConnect with `$SECURE:` placeholder injection

### Test Modules
- `tests_build_http_request` (mod, cfg(test)) — 10 integration tests for HTTP request building (Stripe-like POST, Stripe auth, form encoding, path params, array query, missing auth scheme, ApiKey header/query)
- `spec_stripe_like()` (fn, priv) — Creates a test spec resembling Stripe API
- `spec_pet_by_id()` (fn, priv) — Creates a test spec with path parameter
- `spec_search_with_array_query()` (fn, priv) — Creates a test spec with array query parameter
- `tests` (mod, cfg(test)) — 6 async integration tests for use case lifecycle (fetch-hits-port, second-fetch-cached, force-reload, separate-conversations, not-modified, lookup-cached)
- `CountingPort` (struct, priv) — Mock ApiSpecPort that counts calls and allows response injection
- `tiny_spec()` (fn, priv) — Minimal test spec
- `use_case_with()` (fn, priv) — Factory for use case with a counting port
- `tests_list_and_search` (mod, cfg(test)) — 6 tests for list/search (list-all, pagination, tag-filter, search-by-summary, search-filter-by-method, respects-threshold, returns-top-n)
- `spec_with()` (fn, priv) — Creates a spec with arbitrary endpoints
- `ep()` (fn, priv) — Factory for endpoint test data
- `sample()` (fn, priv) — Creates a sample spec with 5 endpoints
- `tests_details` (mod, cfg(test)) — 2 tests for endpoint details (happy-path, fuzzy-suggestions)
- `tests_resolve_refs` (mod, cfg(test)) — 5 tests for schema ref resolution (simple-ref, unknown-ref, self-cycle, nested-refs)

## File-level notes

- **Code organization**: Structs and use-case impl (lines 14–250) followed by free functions (lines 252–977) followed by comprehensive test suites (lines 979–1835); this structure is clean and maintainable.
- **Error handling**: Uses domain-layer `WebDomainError` throughout; proper use of `?` propagation and error context (hints, suggestions).
- **Testing**: Four test modules with comprehensive coverage of happy paths, error cases, edge cases (cycles, missing refs, pagination, auth, body encoding); ~40 tests total.
- **Concurrency**: Proper use of `Mutex<LruCache>` for thread-safe caching; `Arc` wrapping for shared ownership across sessions.
- **Performance**: Cache-aware design with TTL-based revalidation and HTTP 304 support; fuzzy search uses nucleo_matcher for efficient scoring.

## Flagged Observations

1. **Simplification candidate (line 806–808)**: The `encode_param()` function's match arm for `("form", false)` and `("form", true)` produce identical output (`strs.join(",")`). Both branches could be consolidated into a single arm `("form", _)` for clarity, keeping the comment explaining the rationale.

2. **Magic number (line 384)**: The fuzzy search normalization divides by `hay.chars().count().max(1) * 16.0`. The constant `16.0` lacks an explanation — why 16 and not 15 or 20? Consider a named constant or inline comment.
