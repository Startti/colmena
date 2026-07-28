# src/libs/colmena/src/dag_engine/infrastructure/nodes/api_explorer.rs

**Layer:** infrastructure  **Purpose:** Implements the `api_explorer` toolkit node that exposes five LLM sub-tools (load_spec, list_endpoints, search_endpoint, get_endpoint_details, build_http_request) for discovering and building HTTP requests against OpenAPI 3.x / Swagger 2.0 specifications.

## Symbols

### Structs
- `ApiExplorerNode` (pub struct) — Holds an `ApiSpecUseCase` and `SessionRegistry` for cached spec management across sub-tool calls
- `FakePort` (private struct, cfg test only) — Test stub implementing `ApiSpecPort` that returns hand-built `ParsedSpec` with call tracking

### Methods & Functions
- `ApiExplorerNode::new()` (pub fn) — Constructs node with default OpenApiAdapter, initializes TTL-sweep background task (guarded by Tokio Handle check)
- `ApiExplorerNode::new_with_port()` (pub(crate) fn, cfg test) — Constructs node with custom `ApiSpecPort` for test injection
- `ApiExplorerNode::new_with_port_and_config()` (pub(crate) fn, cfg test) — Constructs node with both custom port and config
- `ApiExplorerNode::with_secure_values()` (pub fn) — Builder method to attach `SecureValueService` for auth secret resolution
- `ApiExplorerNode::registry()` (pub fn) — Returns cloned Arc handle to the session registry
- `ApiExplorerNode::extract_conversation_id()` (private fn) — Extracts `conversation_id` from node inputs, falls back to "default"
- `ApiExplorerNode::require_str()` (pub(crate) fn) — Validates and returns required string field from LLM args; returns structured error JSON on missing/empty
- `ApiExplorerNode::handle_load_spec()` (async private fn) — Fetches or caches OpenAPI spec; returns summary with metadata (title, version, endpoints_count, security_schemes, cached flag)
- `ApiExplorerNode::handle_list_endpoints()` (async private fn) — Lists endpoints from loaded spec with pagination (limit, offset, optional tag filter)
- `ApiExplorerNode::handle_search_endpoint()` (async private fn) — Fuzzy-searches endpoints by query with optional method filter; returns ranked results with scores
- `ApiExplorerNode::handle_get_endpoint_details()` (async private fn) — Fetches full endpoint spec (parameters, request/response schemas, auth); includes `did_you_mean` on miss
- `ApiExplorerNode::handle_build_http_request()` (async private fn) — Validates params against spec and builds ready-to-execute http_request JSON; auth secrets serialized as `${SECURE:...}` placeholders
- `format_spec_error()` (private fn) — Translates all `WebDomainError` variants to LLM-facing structured JSON; panics on non-recoverable variants (InvalidConfig, AdapterInit, SpecTooLarge)
- `Default::default()` (impl fn) — Delegates to `Self::new()`
- `ExecutableNode::execute()` (async fn) — Main dispatch: extracts `__sub_tool` from inputs and routes to handler; returns structured error for unknown sub-tools
- `ExecutableNode::schema()` (fn) — Returns JSON schema of node inputs/outputs/config with documentation strings
- `ExecutableNode::description()` (fn) — Returns one-line description for the LLM tool catalog
- `ToolkitNode::sub_tool_catalog()` (fn) — Returns Vec of 5 `SubToolDefinition`s (load_spec, list_endpoints, search_endpoint, get_endpoint_details, build_http_request)
- `load_spec_sub_tool()` (private fn) — Builds `SubToolDefinition` for load_spec with url (required) and force_reload (optional boolean) parameters
- `list_endpoints_sub_tool()` (private fn) — Builds `SubToolDefinition` for list_endpoints with spec_url (required), tag, limit, offset
- `search_endpoint_sub_tool()` (private fn) — Builds `SubToolDefinition` for search_endpoint with spec_url, query (required), method enum filter, max_results
- `get_endpoint_details_sub_tool()` (private fn) — Builds `SubToolDefinition` for get_endpoint_details with spec_url, operation_id (required)
- `build_http_request_sub_tool()` (private fn) — Builds `SubToolDefinition` for build_http_request with spec_url, operation_id (required), params (optional flat object), auth_secret_ref (optional)

### Tests (cfg test)
- `tests::catalog_has_all_five_sub_tools()` — Verifies sub_tool_catalog() returns exactly 5 tools with correct names
- `tests::load_spec_requires_url()` — Verifies url is in required fields
- `tests::build_http_request_requires_spec_url_and_operation_id()` — Verifies spec_url and operation_id are required; params is optional
- `tests::search_endpoint_exposes_method_enum()` — Verifies method parameter includes all HTTP verbs (GET, POST, PUT, PATCH, DELETE)
- `tests::extract_conversation_id_falls_back_to_default()` — Verifies fallback to "default" when conversation_id not in inputs
- `tests::dispatch_unknown_sub_tool_returns_structured_error()` — Verifies execute() returns error JSON for unrecognized sub_tool name
- `tests::FakePort` (test struct) — Minimal ApiSpecPort implementation tracking call count and returning hand-built ParsedSpec
- `tests::fake_parsed_spec()` (fn) — Factory returning a minimal Petstore-like ParsedSpec with one GET /pets endpoint
- `tests::node_with_fake_port()` (fn) — Helper returning (FakePort, ApiExplorerNode) with default config
- `tests::node_with_fake_port_loose()` (fn) — Helper returning same but with fuzzy_match_threshold=0.05 for search tests on tiny specs
- `tests::load_spec_returns_summary_with_resolved_url()` — Verifies load_spec output includes spec_url_input, resolved_url, title, format, endpoints_count, security_schemes, cached=false
- `tests::load_spec_caches_within_conversation()` — Verifies second load_spec call for same URL within conversation hits cache (port called only once)
- `tests::load_spec_force_reload_bypasses_cache()` — Verifies force_reload=true bypasses cache and increments port call count
- `tests::load_spec_missing_url_returns_invalid_input()` — Verifies missing url parameter returns error with "missing":"url"
- `tests::list_endpoints_returns_paginated_summary()` — Verifies list_endpoints returns total, returned, offset, endpoints array with operation_id/method/path/summary/tags
- `tests::list_endpoints_on_unloaded_spec_returns_spec_not_loaded()` — Verifies calling list_endpoints before load_spec returns "spec_not_loaded" error
- `tests::search_endpoint_ranks_by_fuzzy_score()` — Verifies search_endpoint returns results array with scores and match_reason
- `tests::get_endpoint_details_returns_structured_json()` — Verifies get_endpoint_details returns full endpoint JSON with operation_id, method, path, etc.
- `tests::get_endpoint_details_miss_returns_did_you_mean()` — Verifies endpoint not found returns error with did_you_mean suggestions
- `tests::build_http_request_emits_ready_to_execute_config()` — Verifies build_http_request returns http_request-consumable JSON with method, url, headers (auth as ${SECURE:...})
- `tests::build_http_request_missing_auth_returns_structured_error()` — Verifies missing auth_secret_ref for auth-required endpoint returns "missing_auth" error
- `tests::build_http_request_params_not_object_returns_invalid_input()` — Verifies params must be object or null; array/string/scalar rejected
- `tests::search_endpoint_filters_by_method()` — Verifies method filter excludes non-matching endpoints

## File-level notes

- **Incomplete feature**: The `secure_values: Option<Arc<SecureValueService>>` field (line 36) is marked `#[allow(dead_code)]` with a comment "used in Tasks 14-15 (build_http_request needs secret resolution)". The field is set via `with_secure_values()` builder but never actually used in `handle_build_http_request()` or anywhere else. This is preparatory for a planned feature to decrypt secure values at runtime, not yet implemented.

- **Panic points**: `format_spec_error()` panics on three non-recoverable error variants (InvalidConfig, AdapterInit, SpecTooLarge). These are intentional — they represent configuration/initialization failures that should crash the DAG rather than surface as LLM-recoverable errors. The caller (`execute()` dispatch through handlers) should return `Err` for these before reaching the error formatter.

- **All handlers use LLM-safe error translation**: Every async handler (load_spec through build_http_request) catches application errors via `format_spec_error()` and returns them as `Ok(json!(...))` rather than `Err`, making them recoverable for the LLM. This is intentional for tool-calling workflows.

- **Passive TTL sweep**: The background sweeper (line 52-56) is spawned conditionally on Tokio Handle availability. This allows unit tests without a runtime to construct the node synchronously without panic. The handle is intentionally dropped — the Arc in the registry keeps the sweeper alive for the registry's lifetime.

- **Test coverage**: Comprehensive unit test suite (24 tests) covers all five sub-tools plus parameter validation, error cases, and caching behavior. Uses FakePort stub to avoid real HTTP calls. Two builder helpers (node_with_fake_port, node_with_fake_port_loose) support test construction with controlled fuzzy-match thresholds.

- **No external dependencies leaked**: Node implementation depends only on `ApiSpecUseCase` (application layer), `SessionRegistry<SpecCache>` (domain persistence), and standard library. All port interactions go through the `ApiSpecPort` trait (hexagonal pattern).
