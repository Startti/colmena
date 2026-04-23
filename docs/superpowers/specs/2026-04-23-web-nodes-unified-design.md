# Design: Web Toolkit Nodes — Unified Architecture

**Status:** Draft for review
**Date:** 2026-04-23
**Author:** Daniel Garcia (brainstormed with Claude)
**Target version:** 0.4.0
**Sub-specs:** `2026-04-23-web-nodes-a-tavily-client-design.md`, `2026-04-23-web-nodes-c-api-explorer-design.md`, `2026-04-23-web-nodes-b-browser-design.md`

## Summary

Add three new **toolkit nodes** that give LLM nodes first-class internet capabilities: `tavily_client` (web search + URL fetch), `api_explorer` (OpenAPI/Swagger-driven endpoint discovery and HTTP-request construction), and `browser` (self-hosted headless browser for interactive flows with login). All three are exposed to LLM nodes via a new **multi-tool-per-node** runtime extension — one node can expose N sub-tools to the LLM, each with its own schema.

This document specifies the **shared architecture** (ports/adapters layout, runtime extension, session management, error handling). Each node gets its own sub-spec with concrete configuration, tool schemas, and implementation plan.

## Motivation

Today an LLM node in Colmena can call `http_request` as a tool, but this is insufficient for three very common use cases:

1. **Open-ended web research** — the agent needs to search the web and read pages to answer a question whose data is not in any specific API.
2. **Integrating with an API the agent discovers at runtime** — given a Swagger/OpenAPI URL, the agent should find the right endpoint and build a valid `http_request` call deterministically (not by reading HTML docs and guessing).
3. **Interactive web automation** — some portals have no API; the agent needs to log in, fill forms, click, and extract content from real rendered pages.

Solving these three with a single "browser for LLMs" node would bind the cost of a headless browser (memory, startup time, Browserless credits) to every lightweight web search. The right shape is three specialized toolkits, each cost-appropriate for its job, that the LLM composes.

## Goals

- Let LLM nodes do general web search and URL content fetching through Tavily.
- Let LLM nodes load an OpenAPI/Swagger spec from a URL, search its endpoints, and emit a validated HTTP-request configuration suitable for the `http_request` node.
- Let LLM nodes drive a self-hosted headless browser (login, navigate, fill, click, extract, screenshot) with session persistence across conversational follow-ups and HITL suspensions.
- Handle credentials via the existing Secure Values mechanism — passwords never appear in the LLM context.
- Extend the runtime so a single node can expose multiple tools to the LLM (toolkit nodes). Keep the extension backward-compatible and reusable for future nodes (`http_request`, `socketio_request`, `sql_query`).
- Make all timeouts, TTLs, retries, and caps configurable from the graph JSON with sensible defaults.

## Non-goals

- **Self-hosted Chromium in-process** — browsers run in a separate Browserless container; the Rust lib only contains a CDP client. Embedding Chromium is out of scope (complicates PyO3/napi distribution, memory overhead).
- **Hosted browser providers (Browserbase, Hyperbrowser)** — not in v1, but the `BrowserPort` trait is designed to accommodate an adapter for them later without changing the node.
- **Persistent cross-process caches** — all caches are in-memory with TTLs. Redis-backed caches are a future decorator on the ports.
- **Anti-bot / stealth plugins** — Browserless provides baseline stealth; sophisticated evasion is out of scope.
- **Parsing non-OpenAPI API docs** (Postman collections, GraphQL SDL, RAML, raw HTML docs) — `api_explorer` is OpenAPI 3.x and Swagger 2.0 only. For other formats, the agent falls back to `tavily` + manual `http_request` construction.
- **Migrating existing nodes** (`http_request`, `socketio_request`, `sql_query`) to the multi-tool mechanism — the runtime is designed to support it, but the migration itself is a follow-up project.

## Architecture

### Layered structure (hexagonal)

A new top-level module `src/libs/colmena/src/web/`:

```
src/libs/colmena/src/
├── web/
│   ├── domain/
│   │   ├── search_port.rs          SearchPort trait
│   │   ├── api_spec_port.rs        ApiSpecPort trait
│   │   ├── browser_port.rs         BrowserPort trait
│   │   ├── session.rs              SessionKey, SessionRegistry
│   │   ├── value_objects.rs        SearchResult, Endpoint, ElementSelector, etc.
│   │   └── errors.rs               WebDomainError (thiserror)
│   ├── application/
│   │   ├── search_use_case.rs      Orchestration over SearchPort + cache + rate limit
│   │   ├── api_spec_use_case.rs    Orchestration + spec cache per conversation
│   │   └── browser_use_case.rs     Orchestration + session registry
│   └── infrastructure/
│       ├── tavily_adapter.rs       SearchPort impl using Tavily REST API
│       ├── openapi_adapter.rs      ApiSpecPort impl using `oas3` crate
│       └── browserless_cdp_adapter.rs  BrowserPort impl using `chromiumoxide`
└── dag_engine/infrastructure/nodes/
    ├── tavily_client.rs            Node wrapping SearchUseCase
    ├── api_explorer.rs             Node wrapping ApiSpecUseCase
    └── browser.rs                  Node wrapping BrowserUseCase
```

Domain has zero infrastructure deps (per existing Colmena convention). Each node is a thin wrapper over its use case; use cases depend only on traits.

### External dependencies

| Concern | Choice | Rationale |
|---|---|---|
| Web search | Tavily REST API | LLM-optimized output, cheap (~$0.008/call), first-party content extraction in `/search` endpoint |
| OpenAPI parsing | `oas3` crate | Supports OpenAPI 3.0 and 3.1; mature; zero-copy; handles both JSON and YAML |
| Browser control | `chromiumoxide` crate | Native async Rust, tokio-friendly, speaks CDP over WebSocket to any Chrome/Browserless instance |
| Browser runtime | Browserless Docker container (user-operated) | Self-hosted, Apache 2.0, gives queue management, session isolation, cleanup, and stealth patches out of the box |
| HTTP (Tavily, OpenAPI downloads) | `reqwest` | Already a project dep |

### Runtime extension: multi-tool per node

Colmena today: one entry in `tool_configurations` = one `ToolDefinition` for the LLM. This spec extends that so a single node can expose N sub-tools.

#### New trait

```rust
pub trait ToolkitNode: ExecutableNode {
    /// Returns the catalogue of sub-tools this node exposes.
    /// - Static toolkits (tavily_client, api_explorer, browser): return a hard-coded Vec,
    ///   ignore `config`.
    /// - Dynamic toolkits (future http_request, sql_query): read `config` to derive the Vec.
    fn sub_tool_catalog(&self, config: &Value) -> Vec<SubToolDefinition>;
}

pub struct SubToolDefinition {
    pub name: &'static str,        // "search", "navigate", "load_spec", ...
    pub description: String,       // rich description for the LLM
    pub parameters: ParametersSchema,
    pub required: Vec<String>,
}
```

Existing nodes keep working — this is additive.

#### New shape of `tool_configurations`

In addition to the current format (kept for backward compatibility), entries may declare toolkits:

```json
"tool_configurations": {
  "browser": {
    "node_type": "browser",
    "node_config": { "browserless_ws_url": "ws://localhost:3000" },
    "expose_sub_tools": "all"
  },
  "web": {
    "node_type": "tavily_client",
    "node_config": { "api_key": "${TAVILY_API_KEY}" },
    "expose_sub_tools": ["search", "fetch"]
  }
}
```

The runtime calls `sub_tool_catalog(&node_config)` on the node, filters by `expose_sub_tools`, and produces one `ToolDefinition` per sub-tool. LLM-visible names are prefixed with the toolkit alias: `browser__navigate`, `web__search`.

#### Dispatch at execution

When the LLM invokes `web__search(query="...")`, the `DagToolExecutor`:

1. Resolves the toolkit alias to the underlying node instance.
2. Injects a reserved input key `__sub_tool: "search"` alongside the LLM's arguments.
3. Calls the node's `execute()`.
4. The node's `execute()` branches on `__sub_tool` and dispatches to the right internal handler.

#### Affected files for the runtime change

| File | Change |
|---|---|
| `dag_engine/domain/tool_configuration.rs` | Add `expose_sub_tools` to parser; add `SubToolDefinition`, `ToolkitNodeRef` |
| `dag_engine/domain/node.rs` | Add `ToolkitNode` trait (extends `ExecutableNode`) |
| `dag_engine/infrastructure/dag_tool_executor.rs` | `generate_tool_definition()` branches on toolkit; `execute()` injects `__sub_tool` |
| `dag_engine/application/dag_run.rs` | Lifecycle hooks fire on `conversation` close, not just `run` completion (for session cleanup) |

Net change: ~300–400 lines, all additive. Zero change required to existing nodes.

### Session management

Two of the three nodes are stateful across calls:
- `api_explorer` — a parsed OpenAPI spec persists for reuse across sub-tool calls.
- `browser` — a browser context (cookies, navigation state) persists across a login/navigate/extract flow.

#### Scope key

Sessions are keyed by **`conversation_id`**, not `dag_run_id`. Rationale:

| Scenario | Same `dag_run_id`? | Session outcome |
|---|---|---|
| DAG suspends for HITL, then resumes | Yes | Alive throughout |
| DAG completes, user follows up in same conversation | **No** (new run) | **Alive** if within TTL |
| New conversation from same user | No | Discarded |

The `conversation_id` is the existing external session id that Colmena already threads through the engine (`--session-id` in the CLI, propagated via HITL).

#### Lifecycle

```
[first tool call for this conversation]
    └── CREATE session
         │
         ├── DAG suspends (HITL)  ──► alive
         ├── DAG resumes          ──► alive
         ├── DAG completes        ──► mark IDLE, start TTL countdown
         ├── user follow-up < TTL ──► revive, reuse
         │
         └── Cleanup on: TTL expiry / explicit close_session /
                         new conversation_id / active-session cap eviction
```

#### Defaults (all configurable per-node from JSON)

| Setting | Default | Meaning |
|---|---|---|
| `session_idle_ttl_seconds` | 900 (15 min) | Time of inactivity before cleanup |
| `session_max_lifetime_seconds` | 3600 (1 h) | Hard cap since creation |
| `max_active_sessions` | 50 | Memory cap; LRU-evict idle sessions on overflow |
| `revive_on_follow_up` | true | If false, each new DAG run gets a fresh session |

#### Graceful recovery

When an LLM tool call arrives with a `session_name` whose session was evicted, crashed, or expired, the tool returns a structured error rather than panicking the DAG:

```json
{
  "error": "session_lost",
  "message": "Session 'default' expired. Call browser__new_session() to start fresh.",
  "last_known_url": "https://portal.foo.com/dashboard"
}
```

The LLM can then recover (open a new session, ask the user whether to retry, etc.).

#### Implementation

A generic `SessionRegistry<T>` lives in `web/domain/session.rs`:

```rust
pub struct SessionRegistry<T> {
    inner: Arc<Mutex<HashMap<SessionKey, SessionEntry<T>>>>,
    ttl_config: TtlConfig,
}

pub struct SessionKey {
    pub conversation_id: ConversationId,
    pub session_name: String,   // default "default"
}
```

A background ticker (every 60s, spawned lazily on first use) sweeps for TTL-expired entries and calls their cleanup closure. On conversation close (hook in DAG engine), the registry receives an eager cleanup call for all entries matching the `conversation_id`.

### Secure Values integration

Browser login is the primary use case; `api_explorer.build_http_request` is the secondary.

- Users declare credentials as secure values in the graph (existing feature — see `docs/dds/SECURE_VALUES_DISEÑO.md`).
- The LLM sees only the **name** of the secret (e.g., `"stripe_api_key"`), never the value.
- In `browser`, a dedicated sub-tool `fill_secure(selector, secure_ref)` resolves the secret inside the node and injects it directly into the DOM via CDP. The plaintext password never traverses the LLM context, trace logs, or observability events.
- In `api_explorer`, `build_http_request` accepts `auth_secret_ref: "name"`; if the spec declares a Bearer or API-key auth scheme, the node emits the correct header using the resolved secret.

No code changes to the Secure Values subsystem — the existing `SecureValueResolver` is consumed as-is.

### Caching

| What | Key | Default TTL | Storage |
|---|---|---|---|
| `tavily.search` results | hash(query + opts) | 1 h | In-memory LRU, 1000 entries |
| `tavily.fetch` content | hash(url) | 1 h | In-memory LRU |
| `api_explorer` parsed specs | url + etag/last-modified | 24 h | In-memory LRU, 100 entries |
| `browser` sessions | (no cross-conversation cache) | — | — |

Every cache is per-node-configurable via `enable_cache: bool` and `cache_ttl_seconds`. In-memory only — persistent caches can be added later as port decorators (`RedisCachedSearchPort`) without touching nodes.

### Rate limits

Applies only to `tavily_client` (the sole node with metered external pricing).

```json
{
  "max_calls_per_run": 50,     // per dag_run, default 50, null = off
  "max_calls_per_day": null,   // global, default off
  "fail_on_limit": true         // if false, rate-limit error is returned to LLM as structured result
}
```

Counters are maintained per-node-instance, keyed by `dag_run_id` (not conversation_id — each run gets its own budget).

### Observability

All sub-tool invocations emit structured `tracing` events matching the existing engine conventions:

```json
{
  "toolkit": "browser",
  "sub_tool": "navigate",
  "dag_run_id": "...",
  "conversation_id": "...",
  "session_name": "default",
  "duration_ms": 1234,
  "success": true,
  "args_redacted": { "url": "..." }
}
```

Secure-value arguments are replaced with `"***"`. Enable `COLMENA_WEB_TRACE=1` for detailed CDP / HTTP trace logs.

## Error handling

Principle: **errors that the LLM can plausibly recover from are returned as structured tool results; only true configuration/initialization failures crash the DAG.**

### Domain error types (`web/domain/errors.rs`)

```rust
#[derive(Debug, thiserror::Error)]
pub enum WebDomainError {
    // Crash the DAG
    #[error("invalid config: {0}")]       InvalidConfig(String),
    #[error("adapter init failed: {0}")]  AdapterInit(String),

    // Returned to LLM as structured result
    #[error("rate limit exceeded")]       RateLimit { calls_used: u32, cap: u32 },
    #[error("session lost")]              SessionLost { last_known_url: Option<String> },
    #[error("selector not found")]        SelectorNotFound { selector: String, page_url: String, hints: Vec<String> },
    #[error("navigation failed: {0}")]    NavigationFailed(String),
    #[error("timeout after {ms}ms")]      Timeout { ms: u64 },
    #[error("spec parse failed: {0}")]    SpecParseError(String),
    #[error("endpoint not found")]        EndpointNotFound { searched_for: String, did_you_mean: Vec<String> },
    #[error("upstream: {status}")]        Upstream { status: u16, body: String },
}
```

### Retries (transparent to LLM)

| Operation | Attempts | Notes |
|---|---|---|
| Tavily 5xx / network | 3, exp backoff from 500 ms | Idempotent |
| Browserless CDP idempotent ops (navigate, wait_for, extract) | 2 | Safe |
| Browserless CDP side-effect ops (click, fill) | 0 | Don't double-submit |
| OpenAPI spec download | 3 | Idempotent |

Configurable per-node: `retry_policy: { max_attempts, initial_backoff_ms }`.

### Default timeouts (all configurable)

| Operation | Default |
|---|---|
| `tavily.*` | 30 s |
| `api_explorer.load_spec` | 60 s |
| `browser.navigate` | 30 s |
| `browser.click` / `fill` | 10 s |
| `browser.wait_for` | 30 s (cap 120 s) |
| `browser.extract` | 15 s |

## Testing strategy

Per `docs/developer_guide/05_testing.md`.

### Unit (per layer)
- **Domain**: value-object invariants, serialization round-trips.
- **Application**: use cases against mocked ports (`mockall`). Zero network.
- **Adapters**:
  - `tavily_adapter`: `wiremock` for HTTP stubs.
  - `openapi_adapter`: fixture specs in `tests/fixtures/specs/` (petstore, stripe-excerpt).
  - `browserless_cdp_adapter`: gated behind feature flag `integration-browserless`; uses a docker-compose-launched Browserless.

### Integration (`tests/`)
- **Tavily** (`tests/web/tavily_live.rs`): gated on `TAVILY_API_KEY` — skip, don't fail, when absent.
- **api_explorer** (`tests/web/api_explorer.rs`): fully offline, uses fixtures.
- **browser** (`tests/web/browser_live.rs`): gated on `COLMENA_BROWSERLESS_WS`.

### Test graphs (`tests/graphs/web/`)
- `tavily_search_basic.json`, `tavily_fetch_article.json`
- `api_explorer_stripe.json`
- `browser_login_form.json`
- `full_flow_discover_use_api.json` — end-to-end: tavily finds a spec URL → api_explorer loads it → http_request executes.

### Bindings
- Python smoke test in `python/tests/test_web_nodes.py` and TS equivalent — verifies registration and callability.

## Rollout

Delivered as three sub-specs in this order:
1. **Spec A** — `tavily_client` (simplest; validates the toolkit runtime extension end-to-end).
2. **Spec C** — `api_explorer` (adds the per-conversation cache pattern; stateful but lightweight).
3. **Spec B** — `browser` (full session lifecycle; most involved; depends on the runtime extension being production-proven by A and C).

Each sub-spec has its own implementation plan (via `writing-plans`) and implementation cycle.

## Open questions

None at this stage — all architectural decisions were validated during brainstorming. Each sub-spec may surface node-specific questions which are resolved there.
