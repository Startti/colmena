# src/libs/colmena/src/dag_engine/infrastructure/nodes/tavily_client.rs

**Layer:** infrastructure  
**Purpose:** Implements the `tavily_client` toolkit node, exposing web search and URL-fetch capabilities to LLM agents via Tavily API. Handles per-call config resolution, secure value injection, environment variable substitution, and error formatting.

## Symbols

- `TavilyClientNode` (struct, pub) — Main node type representing the toolkit node; defers adapter construction to first execute() call.
- `TavilyClientNode::new()` (fn, pub) — Creates a new node with no secure values or test doubles.
- `TavilyClientNode::with_secure_values()` (fn, pub) — Builder method to attach a SecureValueService for resolving `<value_N>` placeholders.
- `TavilyClientNode::resolve_env_var()` (fn, pub(crate)) — Resolves `${VAR}` environment variable placeholders; literal strings pass through unchanged.
- `TavilyClientNode::build_use_case()` (fn, async, pub(crate)) — Factory that resolves `api_key` and other config parameters, constructs TavilyAdapter, and returns SearchUseCase with resolved settings.
- `TavilyClientNode::handle_search()` (fn, async) — Private dispatcher for the "search" sub-tool; parses query + optional parameters, applies search_defaults fallback, invokes SearchUseCase.
- `TavilyClientNode::handle_fetch()` (fn, async) — Private dispatcher for the "fetch" sub-tool; validates URL, resolves format (markdown/text), invokes SearchUseCase.
- `Default impl for TavilyClientNode` (impl) — Trivial default() → Self::new().
- `ExecutableNode impl for TavilyClientNode` (impl) — Implements execute() (dispatches __sub_tool to search/fetch), schema() (JSON metadata), description() (LLM-facing help text).
- `ToolkitNode impl for TavilyClientNode` (impl) — Returns vec of two SubToolDefinitions: search and fetch.
- `search_sub_tool()` (fn) — Builds SubToolDefinition for "search" with all parameter properties (query, max_results, include_content, search_depth, include_domains, exclude_domains, time_range).
- `fetch_sub_tool()` (fn) — Builds SubToolDefinition for "fetch" with url and extract_format parameters.
- `merge_string_array()` (fn) — Helper that merges optional input array with optional default array; prefers input if non-empty, falls back to defaults.
- `format_llm_error()` (fn) — Converts WebDomainError to structured JSON for LLM consumption (rate_limit, timeout, upstream_error); respects fail_on_limit flag to pass through or suppress errors.

**Test module:**
- `StubPort` (struct) — Mock SearchPort impl that increments call counters and returns hardcoded results.
- `node_with_stub()` (fn) — Helper fixture that creates a node + stub port + SearchUseCase for testing.
- `catalog_has_search_and_fetch()` (test) — Validates that sub_tool_catalog() returns exactly 2 tools named "search" and "fetch".
- `search_requires_query()` (test) — Validates that "search" tool marks "query" as required.
- `fetch_requires_url()` (test) — Validates that "fetch" tool marks "url" as required.
- `resolve_env_var_passes_literal_through()` (test) — Literal API key (no ${...}) passes through unchanged.
- `resolve_env_var_replaces_placeholder()` (test) — ${VAR} is replaced by env var value.
- `search_dispatches_and_returns_json()` (test) — Async execute() with "search" sub-tool invokes SearchUseCase and returns JSON results.
- `search_merges_search_defaults_from_config()` (test) — Config.search_defaults are merged as fallback for max_results, include_domains, etc.
- `search_rate_limit_returns_structured_error_when_fail_on_limit_false()` (test) — Rate limit error formatted to JSON with "rate_limit" status code (not propagated as DAG error).
- `search_rate_limit_crashes_dag_when_fail_on_limit_true()` (test) — Rate limit error propagates up (DAG run fails) when fail_on_limit=true.
- `search_missing_query_returns_structured_error()` (test) — Missing "query" input returns JSON {"error": "invalid_input", ...}.
- `fetch_dispatches_and_returns_json()` (test) — Async execute() with "fetch" sub-tool invokes SearchUseCase.fetch() and returns JSON.
- `fetch_defaults_to_markdown()` (test) — extract_format defaults to "markdown" when omitted.
- `fetch_missing_url_returns_structured_error()` (test) — Missing "url" input returns JSON {"error": "invalid_input", ...}.
- `unknown_sub_tool_errors()` (test) — Unknown __sub_tool value propagates as DAG error.

## File-level notes

- **Deferred construction:** Adapter and use case are created per execute() call, not at registry time, because api_key is resolved from per-call config (environment variables or secure-value placeholders).
- **Session keying:** Hard-coded `session_id = "default"` for rate-limiting (line 282). The ExecutableNode trait does not thread through dag_run_id, so a stable default is used. This is a documented limitation; future work would thread actual run ID through the trait if per-run rate-limiting is needed.
- **Error handling:** Three error strategies: (1) missing required input → JSON with "invalid_input" error code, returned as success; (2) rate limit with fail_on_limit=false → JSON with "rate_limit" error code, returned as success; (3) all other errors or rate_limit with fail_on_limit=true → Box<dyn Error>, which crashes the DAG.
- **Parameter fallback:** handle_search merges per-call inputs with config.search_defaults for each optional parameter (max_results, include_domains, search_depth, etc.), with input taking precedence.
- **Comprehensive test coverage:** Tests cover happy path (search, fetch), default parameters, error conditions (missing required fields, rate limit behavior, unknown sub-tool), and environment variable resolution.
