# src/libs/colmena/src/web/application/mod.rs

**Layer:** application  **Purpose:** Module file declaring and re-exporting web-toolkit application-layer submodules: use cases for API spec discovery, search, URL normalization, and Swagger-to-OpenAPI conversion.

## Symbols

- `search_use_case` (mod, pub) — Submodule providing search use case and configuration
- `swagger2_to_oas3` (mod, pub) — Submodule for Swagger 2.0 to OpenAPI 3.x conversion
- `url_normalizer` (mod, pub) — Submodule for URL normalization utilities
- `api_spec_use_case` (mod, pub) — Submodule providing API spec discovery, endpoint listing, search, and HTTP request building
- `SearchUseCase` (type, pub) — Re-exported search use case type from search_use_case module
- `SearchUseCaseConfig` (type, pub) — Re-exported search configuration type from search_use_case module
- `build_http_request` (fn, pub) — Re-exported function to construct HTTP requests from API endpoint specs
- `get_endpoint_details` (fn, pub) — Re-exported function to fetch detailed endpoint information from API spec
- `list_endpoints` (fn, pub) — Re-exported function to list all endpoints in an API spec with pagination
- `search_endpoint` (fn, pub) — Re-exported function to search endpoints by keyword
- `ApiSpecUseCase` (type, pub) — Re-exported use case type for API spec discovery and manipulation
- `ApiSpecUseCaseConfig` (type, pub) — Re-exported configuration type for API spec use case
- `CachedSpec` (type, pub) — Re-exported cached API specification type
- `EndpointListPage` (type, pub) — Re-exported paginated endpoint list response type
- `EndpointSearchHit` (type, pub) — Re-exported endpoint search result type
- `EndpointSummary` (type, pub) — Re-exported endpoint metadata summary type
- `SpecCache` (type, pub) — Re-exported API spec cache type

## File-level notes

- Simple module-level re-export file; no application logic present.
- All 4 submodules (`search_use_case`, `swagger2_to_oas3`, `url_normalizer`, `api_spec_use_case`) declared but only 2 (`search_use_case`, `api_spec_use_case`) have items re-exported at this level.
- No dead code, unfinished stubs, or improvement opportunities detected.
