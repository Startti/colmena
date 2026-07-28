# src/libs/colmena/src/web/application/search_use_case.rs

**Layer:** application  **Purpose:** Orchestrates a SearchPort with LRU caching (keyed by request hash + TTL), per-run rate-limiting (capped calls), and exponential-backoff retry logic for transient upstream/timeout errors. No I/O performed—all operations delegate to the port.

## Symbols

- `hash_search` (fn, private) — computes u64 hash key from SearchRequest fields for cache lookup
- `hash_fetch` (fn, private) — computes u64 hash key from FetchRequest fields for cache lookup
- `CachedResponse` (enum, private) — wrapper holding either SearchResponse or FetchResponse for cache storage
- `CachedEntry` (struct, private) — cache entry pair: response + insertion timestamp for TTL validation
- `is_retryable` (fn, private) — predicate: returns true for 5xx/status-0 upstreams or timeout errors
- `SearchUseCaseConfig` (struct, pub) — configuration struct with cache enable/ttl, rate-limit cap, retry attempts/backoff, timeout
- `SearchUseCaseConfig::default()` (impl fn, pub) — provides defaults: cache on, 1hr ttl, 50 calls/run, 3 attempts, 500ms backoff
- `RateLimitState` (struct, private) — holds HashMap<run_id, call_count> for per-run accounting
- `RateLimitState::try_increment` (fn, private) — increments counter for run_id; returns RateLimit error if at cap
- `RateLimitState::reset` (fn, private) — removes counter entry for run_id
- `SearchUseCase` (struct, pub) — main orchestrator holding Arc<SearchPort>, config, Mutex<RateLimitState>, RwLock<LruCache>
- `SearchUseCase::new` (fn, pub) — constructs with port and config; initializes 1024-entry LRU and default rate-limit state
- `SearchUseCase::config()` (fn, pub) — returns reference to stored config
- `SearchUseCase::reset_run` (fn, pub) — clears per-run rate-limit counter (called by engine at run-end)
- `SearchUseCase::search` (fn, pub async) — search entry point: cache hit check → rate-limit increment → retry loop → cache store on success
- `SearchUseCase::fetch` (fn, pub async) — fetch entry point: cache hit check → rate-limit increment → retry loop → cache store on success
- `SearchUseCase::cache_lookup_search` (fn, private) — retrieves cached SearchResponse by hash key; pops expired entries
- `SearchUseCase::cache_lookup_fetch` (fn, private) — retrieves cached FetchResponse by hash key; pops expired entries
- `SearchUseCase::is_expired` (fn, private) — compares entry age (ms since insertion) against config ttl
- `SearchUseCase::call_with_retry_search` (fn, private async) — retry loop: calls port.search(), backs off exponentially on retryable errors, returns last error after max_attempts
- `SearchUseCase::call_with_retry_fetch` (fn, private async) — retry loop: calls port.fetch(), backs off exponentially on retryable errors, returns last error after max_attempts
- `StubPort` (struct, private test) — mock port with call counters for testing without wiremock
- `StubPort::new` (fn, private) — constructs with zero call counters
- `StubPort::search` (impl fn, async) — increments counter, returns synthetic SearchResponse with one result
- `StubPort::fetch` (impl fn, async) — increments counter, returns synthetic FetchResponse
- `uc()` (fn, private test helper) — returns (Arc<StubPort>, SearchUseCase) with default config
- `FlakyPort` (struct, private test) — mock port that fails N times then succeeds, tracks call count
- `FlakyPort::new` (fn, private) — constructs with fail count and error to return on failures
- `FlakyPort::search` (impl fn, async) — decrements failure counter, returns error until exhausted, then success
- `FlakyPort::fetch` (impl fn, async) — decrements failure counter, returns error until exhausted, then success
- `clone_err` (fn, private test) — converts WebDomainError reference to owned copy (used for FlakyPort error injection)
- 13 test cases covering: delegation to port, rate-limit cap enforcement, separate run counters, reset behavior, shared counter across search/fetch, cache hits/misses, TTL expiry, cache not counting toward limit, retry on 5xx/timeout, retry exhaustion, no retry on RateLimit/AdapterInit

## File-level notes

- **`fail_on_limit` field is unused**: SearchUseCaseConfig carries a `fail_on_limit: bool` field (default false), but it is never consulted in any of SearchUseCase's methods. The rate-limit behavior is identical whether the flag is true or false — always returns a RateLimit error immediately. The field appears to be a partially implemented feature; if `true`, it may have been intended to control whether the usecase should block/retry/suspend vs. fail immediately, but that logic is not present.

- **Duplication in retry logic**: `call_with_retry_search` (lines 235–253) and `call_with_retry_fetch` (lines 255–273) are structurally identical except for the port call (`port.search()` vs. `port.fetch()`). Could be refactored into a single generic retry helper accepting a closure to reduce ~40 lines of boilerplate.

- **Cache and rate-limit integration is correct**: cache lookups return before rate-limit increment, so cache hits do not consume the per-run budget—by design. Tests confirm this behavior.

- **Defensive max(0) in is_expired**: Line 231 uses `.num_milliseconds().max(0)` to guard against negative age on clock skew; reasonable but could benefit from a comment explaining the intent.
