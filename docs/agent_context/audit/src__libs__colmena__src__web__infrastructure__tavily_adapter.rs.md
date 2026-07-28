# src/libs/colmena/src/web/infrastructure/tavily_adapter.rs

**Layer:** infrastructure  **Purpose:** Implements the `SearchPort` trait via REST adapter for the Tavily web search and URL extraction API. Maps HTTP responses to domain errors and parses JSON results into domain types.

## Symbols

- `DEFAULT_BASE_URL` (const) — default Tavily API base URL
- `TavilyAdapter` (struct, pub) — holds reqwest Client, api_key, and base_url
- `TavilyAdapter::new()` (fn, pub) — constructs adapter with timeout, validates api_key non-empty
- `TavilyAdapter::with_base_url()` (fn, pub, cfg test) — test helper to override base URL for wiremock
- `TavilyAdapter::map_error()` (fn, private) — maps HTTP status codes (401/403/429/5xx/etc) to WebDomainError  [FLAG: improvement — lines 75–76 have unreachable branch; 5xx pattern guard produces identical result as fallback `s`, making it dead code]
- `TavilyAdapter::map_transport_error()` (fn, private) — maps reqwest::Error (timeout vs other transport) to WebDomainError
- `SearchPort::search()` (async fn, impl) — executes POST /search with domain filters and time range; parses results array into SearchResponse with title/url/snippet/score/content
- `SearchPort::fetch()` (async fn, impl) — executes POST /extract on single URL; parses first result into FetchResponse with content/title/url; checks failed_results array for extraction errors
- `tests::rejects_empty_api_key()` (test fn) — verifies TavilyAdapter::new rejects empty api_key
- `tests::accepts_nonempty_api_key()` (test fn) — verifies TavilyAdapter::new accepts valid key and sets defaults
- `tests::map_error_*()` (test fns, 5×) — verify status-code error mapping for 429/401/403/502/418
- `tests::fast_adapter()` (helper fn) — constructs adapter with test key and custom base URL
- `tests::search_*()` (integration test fns, 7×) — wiremock-based tests covering happy path, content flag, advanced depth credits, domain filters, time range, and error status codes
- `tests::fetch_*()` (integration test fns, 3×) — wiremock-based tests covering markdown extraction, failed_results handling, and 429 rate limit

## File-level notes

- Clean hexagonal separation: domain errors fully owned by `WebDomainError`, infrastructure layer handles only HTTP status mapping and JSON parsing.
- Comprehensive wiremock integration tests for both endpoints; all request/response paths exercised.
- Defensive JSON parsing: uses `.unwrap_or_default()` / `.unwrap_or("")` throughout for missing fields, appropriate for third-party API fragility.
- One small efficiency issue: the explicit `s if (500..600).contains(&s)` pattern match in `map_error()` (line 75) is unreachable; the fallback arm `s` (line 76) will catch it and produce the same result, making the guard redundant.
- Score field (line 156) cast from f64 to f32 without bounds checking, reasonable for 0–1 range but silently truncates oversized values.
- Test helper `fast_adapter()` properly marked as private test utility; no public surface pollution.
