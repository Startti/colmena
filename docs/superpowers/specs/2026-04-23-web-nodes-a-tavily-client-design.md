# Design: `tavily_client` Toolkit Node (Spec A)

**Status:** Draft for review
**Date:** 2026-04-23
**Author:** Daniel Garcia (brainstormed with Claude)
**Target version:** 0.4.0
**Depends on:** `2026-04-23-web-nodes-unified-design.md`

## Summary

Introduce a new toolkit node `tavily_client` that gives LLM nodes two capabilities via the Tavily REST API: **`search`** (web search with optional inline content extraction) and **`fetch`** (read cleaned content from a specific URL). Both operations are exposed as LLM sub-tools under a single node instance. The node is stateless across calls but applies cross-run caching and per-run rate limiting to control cost.

This is the first of three web-toolkit nodes and the one that validates the multi-tool-per-node runtime extension end-to-end.

## Motivation

LLM agents in Colmena today cannot do open-ended web research. They can call `http_request` tools pointed at known APIs, but cannot answer questions like "what are the latest changes in the AWS EC2 pricing page?" where the answer lives in unstructured, arbitrary web content.

Tavily is the right provider for this because:
- Flat per-call pricing (~$0.008 per search, ~$0.001 per extract) — predictable cost.
- Response format is LLM-optimized (short relevance-scored snippets, not full HTML dumps).
- `/search` endpoint can optionally include full extracted content of the top-N results in one round-trip — avoiding a search→read→synthesize ping-pong with the LLM.
- First-party content extraction (no separate scraping library needed; no JavaScript rendering needed for standard pages).

## Goals

- Expose two sub-tools to the LLM: `search` and `fetch`.
- Give the LLM per-call control over cost/depth trade-offs via `include_content` and `max_results` parameters in `search`.
- Cache results across DAG runs to avoid paying for repeated queries.
- Enforce a budget cap per run so runaway agents don't burn credits.
- Surface Tavily rate-limit and quota errors to the LLM as structured recoverable results.
- Zero changes to existing nodes.

## Non-goals

- Fallback to a self-hosted search (SearxNG) or alternate provider (Exa, Serper, Brave) — future adapter work, the `SearchPort` trait accommodates it.
- Image / news / places-specific search endpoints (Tavily offers these; out of scope for v1).
- Persisting search history to disk or a database.
- Automatic query rewriting / reformulation — the agent owns query quality.

## API surface

### Node configuration

```json
{
  "type": "tavily_client",
  "config": {
    "api_key": "${TAVILY_API_KEY}",

    "enable_cache": true,
    "cache_ttl_seconds": 3600,

    "max_calls_per_run": 50,
    "max_calls_per_day": null,
    "fail_on_limit": false,

    "retry_policy": {
      "max_attempts": 3,
      "initial_backoff_ms": 500
    },

    "timeout_seconds": 30,

    "search_defaults": {
      "search_depth": "basic",
      "max_results": 5,
      "include_content": false,
      "include_domains": [],
      "exclude_domains": []
    }
  }
}
```

All fields except `api_key` are optional; defaults from the unified design apply.

### Sub-tools exposed to the LLM

#### `tavily_client__search`

**LLM-facing description** (rich, drives accuracy):

> Search the web for up-to-date information on any topic. Returns a ranked list of relevant results with titles, URLs, and snippets. Use this when the user asks about current events, facts you are not confident about, or anything whose answer is not in your training data. Set `include_content=true` only when snippets are insufficient — this is 2-3x more expensive but saves a follow-up `fetch` call. Use `include_domains` to restrict to trusted sources (e.g., official docs). Prefer specific queries over generic ones.

**Parameters:**

| Name | Type | Required | Default | Description |
|---|---|---|---|---|
| `query` | string | yes | — | Natural-language search query. |
| `max_results` | integer | no | 5 | Results to return (1–10). |
| `include_content` | boolean | no | false | If true, includes full extracted text of top results. 2-3x credit cost. |
| `search_depth` | "basic" \| "advanced" | no | "basic" | "advanced" is 2 credits, better ranking for complex queries. |
| `include_domains` | array of strings | no | [] | Restrict results to these domains. |
| `exclude_domains` | array of strings | no | [] | Exclude these domains. |
| `time_range` | "day" \| "week" \| "month" \| "year" | no | null | Restrict by recency. |

**Returns:**

```json
{
  "query": "...",
  "results": [
    {
      "title": "...",
      "url": "...",
      "snippet": "...",
      "score": 0.94,
      "content": "..."             // present only if include_content=true
    }
  ],
  "answer": "...",                 // Tavily's synthesized answer, if provided
  "credits_used": 1
}
```

#### `tavily_client__fetch`

**LLM-facing description:**

> Read the cleaned text content of a specific URL. Use this when you already know the URL (the user gave it to you, or it came from a previous search) and want the full content, not just a snippet. Output is the page text with navigation, ads, and boilerplate removed. Does not execute JavaScript — use the `browser` toolkit for pages that require login or dynamic rendering.

**Parameters:**

| Name | Type | Required | Default | Description |
|---|---|---|---|---|
| `url` | string | yes | — | Absolute URL to fetch. |
| `extract_format` | "markdown" \| "text" | no | "markdown" | Output format. |

**Returns:**

```json
{
  "url": "...",
  "title": "...",
  "content": "...",
  "content_length": 4231,
  "credits_used": 1
}
```

## Architecture

### Domain (`web/domain/`)

```rust
// search_port.rs
#[async_trait]
pub trait SearchPort: Send + Sync {
    async fn search(&self, req: SearchRequest) -> Result<SearchResponse, WebDomainError>;
    async fn fetch(&self, req: FetchRequest) -> Result<FetchResponse, WebDomainError>;
}

pub struct SearchRequest {
    pub query: String,
    pub max_results: u8,
    pub include_content: bool,
    pub search_depth: SearchDepth,
    pub include_domains: Vec<String>,
    pub exclude_domains: Vec<String>,
    pub time_range: Option<TimeRange>,
}

pub struct SearchResponse {
    pub query: String,
    pub results: Vec<SearchResult>,
    pub answer: Option<String>,
    pub credits_used: u32,
}

pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
    pub score: f32,
    pub content: Option<String>,
}

pub struct FetchRequest { pub url: String, pub format: ExtractFormat }
pub struct FetchResponse { pub url: String, pub title: Option<String>, pub content: String }
```

### Application (`web/application/search_use_case.rs`)

Wraps the port with:
- **LRU cache** (`lru` crate) keyed by hash of the full request; respects `enable_cache` and `cache_ttl_seconds`.
- **Rate-limit counter**: per-`dag_run_id` counter for `max_calls_per_run`; a process-local daily bucket for `max_calls_per_day`. Both live in an `Arc<Mutex<RateLimitState>>` owned by the use case. The daily bucket resets at UTC midnight and is **not** shared across engine processes — for multi-process deployments needing a true global cap, a future decorator adapter can back it with Redis.
- **Retry loop** using the configured policy for 5xx / transport errors.

```rust
pub struct SearchUseCase {
    port: Arc<dyn SearchPort>,
    cache: RwLock<LruCache<u64, CachedEntry>>,
    counters: Mutex<RateLimitState>,
    config: SearchUseCaseConfig,
}
```

### Infrastructure (`web/infrastructure/tavily_adapter.rs`)

Thin REST adapter using `reqwest`. Tavily endpoints:

- `POST https://api.tavily.com/search` — body `{api_key, query, search_depth, max_results, include_answer, include_raw_content, include_domains, exclude_domains, ...}`
- `POST https://api.tavily.com/extract` — body `{api_key, urls, extract_depth}`

Mapping error responses to `WebDomainError`:
- HTTP 429 → `RateLimit` (parse `X-RateLimit-*` headers if present)
- HTTP 5xx → `Upstream` (retried internally up to `max_attempts`)
- HTTP 401/403 → `AdapterInit` wrapped (crashes — bad config)
- Transport/timeout → `Timeout` or `Upstream` depending on cause

### Node (`dag_engine/infrastructure/nodes/tavily_client.rs`)

Implements `ToolkitNode`. `sub_tool_catalog()` returns two `SubToolDefinition`s (static). `execute()` dispatches on `__sub_tool`:

```rust
async fn execute(&self, inputs: NodeInputs, ctx: &ExecCtx) -> Result<Value, Error> {
    let sub = inputs.required_str("__sub_tool")?;
    match sub {
        "search" => self.handle_search(inputs, ctx).await,
        "fetch"  => self.handle_fetch(inputs, ctx).await,
        other    => Err(invalid("unknown sub_tool", other)),
    }
}
```

Each handler: validates args → builds domain request → calls use case → formats response as JSON for the LLM.

### Error → LLM mapping

| Domain error | LLM sees |
|---|---|
| `RateLimit` | `{ error: "rate_limit", calls_used, cap, scope: "run" \| "day", message: "..." }` |
| `Timeout` | `{ error: "timeout", ms, message: "..." }` |
| `Upstream {status}` (after retries exhausted) | `{ error: "upstream_error", status, retryable: false, message: "..." }` |
| `InvalidConfig` / `AdapterInit` | DAG crashes (not an LLM-recoverable error) |

## Data flow (end-to-end, `search` call)

```
LLM decides to search
       │
       ▼
LLM emits tool_call: tavily_client__search({ query: "...", max_results: 3 })
       │
       ▼
DagToolExecutor.execute()
 ├─ resolves toolkit "tavily_client" → node instance
 ├─ injects __sub_tool="search"
 └─ calls TavilyClientNode.execute(inputs, ctx)
       │
       ▼
TavilyClientNode.handle_search()
 ├─ validates + coerces args (merging with search_defaults from config)
 ├─ calls SearchUseCase.search(req)
 │    │
 │    ├─ cache.lookup(hash(req)) → if hit and not expired, return
 │    ├─ rate_limit.check(dag_run_id) → if exceeded, return RateLimit error
 │    ├─ port.search(req) → TavilyAdapter hits https://api.tavily.com/search
 │    ├─ rate_limit.increment(dag_run_id, credits_used)
 │    ├─ cache.store(hash(req), response, ttl)
 │    └─ return response
 └─ serialize response as JSON tool result
       │
       ▼
LLM receives tool result, continues reasoning
```

## Configuration examples

### Minimal (defaults for everything)

```json
{ "type": "tavily_client", "config": { "api_key": "${TAVILY_API_KEY}" } }
```

### Restricted to official docs with tight budget

```json
{
  "type": "tavily_client",
  "config": {
    "api_key": "${TAVILY_API_KEY}",
    "max_calls_per_run": 5,
    "fail_on_limit": true,
    "search_defaults": {
      "include_domains": ["docs.aws.amazon.com", "kubernetes.io"],
      "search_depth": "advanced",
      "max_results": 3
    }
  }
}
```

### Use from an `llm_call` node

```json
{
  "type": "llm_call",
  "config": {
    "provider": "anthropic",
    "model": "claude-opus-4-7",
    "api_key": "${ANTHROPIC_API_KEY}",
    "tool_configurations": {
      "web": {
        "node_type": "tavily_client",
        "node_config": { "api_key": "${TAVILY_API_KEY}" },
        "expose_sub_tools": ["search", "fetch"]
      }
    },
    "system_message": "You are a research assistant. Use web__search for up-to-date facts, web__fetch to read specific URLs."
  }
}
```

## Testing

### Unit tests

- `search_use_case.rs`: cache hit/miss paths, rate-limit boundary, retry on 5xx → success, retry exhaustion.
- `tavily_adapter.rs` under `wiremock`: each documented HTTP status is mapped to the right domain error; happy paths for `/search` and `/extract`.
- `tavily_client.rs` node: dispatch on `__sub_tool`, param validation, unknown sub_tool error.

### Integration tests

- `tests/web/tavily_live.rs`: runs only when `TAVILY_API_KEY` is set. Covers:
  - Basic search returns ≥1 result.
  - `include_content=true` returns content on top results.
  - `fetch` on a known-stable URL (e.g., `https://example.com`) returns content.
  - Invalid API key → `AdapterInit` (DAG crash).

### Test graphs

- `tests/graphs/web/tavily_search_basic.json` — one `llm_call` with `tavily_client` as tool, simple research query.
- `tests/graphs/web/tavily_fetch_article.json` — user supplies URL, LLM calls `fetch`.

### Python / TS bindings

- Registration smoke test in `python/tests/test_web_nodes.py`.

## Rollout

Implementation delivered in this order (each is its own task in the implementation plan):

1. Runtime: `ToolkitNode` trait + `tool_configurations` parser extension + executor dispatch. (No node uses it yet — land it and test with a stub.)
2. Domain layer: `SearchPort`, value objects, errors.
3. Tavily adapter (with `wiremock` unit tests).
4. `SearchUseCase` (cache + rate limit + retry).
5. `tavily_client` node.
6. Test graphs and integration tests.
7. Python / TS binding exposure.
8. Docs: add to `docs/node_configurations.json`, `docs/agent_context/node_ports_reference.md`, and a new `docs/developer_guide/25_web_nodes.md`.

## Open questions

- **`api_key` as secure value**: should `api_key` only be accepted as `${ENV_VAR}` / secure-value reference, refusing literal strings? Existing nodes accept literals; for consistency we allow both but emit a `tracing` warning when literal. Decision: allow both, warn on literal.
- **Time range parameter enum**: Tavily accepts `"year" | "month" | "week" | "day"`. Adding raw date ranges is future work.
