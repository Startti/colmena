# Design: `api_explorer` Toolkit Node (Spec C)

**Status:** Draft for review
**Date:** 2026-04-23
**Author:** Daniel Garcia (brainstormed with Claude)
**Target version:** 0.4.0
**Depends on:** `2026-04-23-web-nodes-unified-design.md` (runtime multi-tool extension, session registry, Secure Values integration)

## Summary

Introduce a new toolkit node `api_explorer` that lets an LLM agent take an OpenAPI specification URL at runtime (either OpenAPI 3.x or Swagger 2.0) and then deterministically discover endpoints and build valid HTTP-request configurations against it. Five sub-tools are exposed to the LLM: `load_spec`, `list_endpoints`, `search_endpoint`, `get_endpoint_details`, and `build_http_request`. Swagger 2.0 documents are transparently converted to OpenAPI 3.0 inside the adapter so the rest of the system sees a single, modern internal model. Common Git-forge "blob" URLs (GitHub, GitLab, Bitbucket) are automatically rewritten to their raw-content equivalents so the LLM does not need to know the transformation. Parsed specs are cached per conversation (24 h default) with ETag-based revalidation. The output of `build_http_request` is a JSON object shaped exactly as the input of the existing `http_request` node — closing the loop from "an API's docs URL" to "an executing HTTP call" without LLM hallucination.

## Motivation

Agents frequently need to integrate with APIs whose specification is public (Stripe, Amadeus, GitHub, Shopify, Twilio, etc.). The current alternative — `tavily_client` to read HTML docs plus the LLM reasoning its way to an `http_request` config — is error-prone:

- Wrong casing (`customer` vs `customerId`).
- Missing or invented required parameters.
- Stale example snippets in HTML that the LLM treats as authoritative.
- Auth schemes misconstructed (Bearer header name, API-key location).
- Types coerced incorrectly (integer vs string IDs, date formats).

`api_explorer` replaces the LLM's reading of HTML with **deterministic parsing** of a machine-readable spec. The LLM's job narrows to "pick the right endpoint and provide the user-level arguments"; all mechanical translation to HTTP is done by the node.

## Goals

- Download and parse a spec from a URL, accepting **OpenAPI 3.x** (JSON or YAML) natively and **Swagger 2.0** (JSON or YAML) via internal conversion to 3.0. Enforce size and timeout limits.
- Auto-rewrite common Git-forge blob URLs (GitHub, GitLab, Bitbucket) to their raw-content URLs before downloading, so the LLM can hand in "the URL the user pasted" without knowing the forge's rendering convention.
- Cache parsed specs per conversation with ETag/Last-Modified revalidation (cheap no-op re-validation when the spec hasn't changed).
- Expose five sub-tools that let the LLM list, search (keyword + fuzzy), and inspect endpoints.
- Provide `build_http_request(operation_id, params, auth_secret_ref?)` that:
  - Validates required parameters and types.
  - Resolves path parameters, query parameters, request body, and headers according to the spec.
  - Emits a JSON object directly usable as input to the `http_request` node (url, method, headers, query_params, body).
  - Applies auth schemes declared in the spec using a user-provided Secure Value reference.
- Surface validation errors (missing required, type mismatch, unknown operation_id, HTML instead of spec) as structured LLM-recoverable results with hints.

## Non-goals

- **AsyncAPI, GraphQL SDL, RAML, Postman collections** — out of scope. Fall back to `tavily_client` + manual `http_request` for these formats.
- **Full Swagger 2.0 fidelity**: the conversion handles the mechanical 1:1 mappings (host/basePath/schemes → servers, definitions → components.schemas, body params → requestBody, consumes/produces → content, securityDefinitions → securitySchemes, etc.). Edge cases that require semantic rework (complex `allOf` polymorphism, vendor extensions, unusual `collectionFormat` values) may result in a conversion error rather than silent data loss — the LLM receives a clear error and can fall back to `tavily_client` for those specific specs.
- **Semantic (embedding-based) endpoint search** — keyword + fuzzy only in v1. The `ApiSpecPort` trait allows plugging a `SemanticEndpointIndex` adapter later.
- **Actually executing the request** — `api_explorer.build_http_request` emits config; the `http_request` node (or a direct tool call) executes it. This keeps `api_explorer` deterministic and side-effect-free.
- **Spec authoring / editing** — read-only.
- **OAuth2 interactive flows** — if the spec declares OAuth2, `build_http_request` emits a Bearer header given a pre-obtained token from a Secure Value. The agent (or another node) is responsible for obtaining the token.
- **Server selection heuristics** — if the spec declares multiple `servers[]`, use the first unless the config overrides. No environment-sniffing.

## API surface

### Node configuration

```json
{
  "type": "api_explorer",
  "config": {
    "enable_cache": true,
    "cache_ttl_seconds": 86400,
    "max_cached_specs": 100,

    "session_idle_ttl_seconds": 900,
    "session_max_lifetime_seconds": 3600,

    "max_spec_size_bytes": 10485760,
    "spec_download_timeout_seconds": 60,

    "default_base_url_override": null,
    "fuzzy_match_threshold": 0.6,

    "retry_policy": {
      "max_attempts": 3,
      "initial_backoff_ms": 500
    }
  }
}
```

All fields are optional; defaults shown above.

### Sub-tools exposed to the LLM

Each sub-tool's description is rich — accuracy hinges on the LLM knowing when to use each.

#### `api_explorer__load_spec`

**Description:**

> Download and parse an OpenAPI 3.x or Swagger 2.0 specification from a URL. Must be called before any other api_explorer tool. The parsed spec is cached for the conversation so subsequent tools are fast. Returns a summary of what the spec contains. You can paste Git-forge URLs (github.com/.../blob/..., gitlab.com/.../-/blob/...) — the node rewrites them to the raw-content URL automatically; use `resolved_url` in the result to see what was actually fetched. Swagger 2.0 documents are converted internally to OpenAPI 3.0 so all subsequent tools behave identically. If the download returns HTML (usually because a Git-forge blob URL could not be normalized), you get a clear error suggesting the raw URL format.

**Parameters:**

| Name | Type | Required | Description |
|---|---|---|---|
| `url` | string | yes | Absolute URL of an OpenAPI 3.x or Swagger 2.0 JSON/YAML file. Git-forge blob URLs are accepted and auto-rewritten to raw. |
| `force_reload` | boolean | no | If true, bypass cache and re-download. Default false. |

**Returns:**

```json
{
  "spec_url_input": "https://github.com/.../blob/main/openapi.yaml",
  "resolved_url": "https://raw.githubusercontent.com/.../main/openapi.yaml",
  "original_format": "swagger-2.0",
  "internal_format": "openapi-3.0.3",
  "title": "Stripe API",
  "version": "2024-06-20",
  "description": "The Stripe API provides...",
  "server_url": "https://api.stripe.com",
  "endpoints_count": 318,
  "tags": ["Accounts", "Balance", "Charges", ...],
  "security_schemes": ["BearerAuth", "BasicAuth"],
  "cached": true
}
```

`original_format` is `"openapi-3.x"` or `"swagger-2.0"`; `internal_format` is always an OpenAPI 3.x string after conversion.

#### `api_explorer__list_endpoints`

**Description:**

> List all endpoints in a previously loaded spec. Prefer `search_endpoint` unless you want to browse by category. Results are paginated.

**Parameters:**

| Name | Type | Required | Description |
|---|---|---|---|
| `spec_url` | string | yes | The URL of the previously-loaded spec. |
| `tag` | string | no | Filter by tag (e.g., "Subscriptions"). |
| `limit` | integer | no | Page size. Default 50, max 200. |
| `offset` | integer | no | Pagination offset. Default 0. |

**Returns:**

```json
{
  "total": 318,
  "returned": 50,
  "offset": 0,
  "endpoints": [
    {
      "operation_id": "PostSubscriptions",
      "method": "POST",
      "path": "/v1/subscriptions",
      "summary": "Create a subscription",
      "tags": ["Subscriptions"]
    }
  ]
}
```

#### `api_explorer__search_endpoint`

**Description:**

> Find endpoints by keyword. Matches against path, summary, description, operation_id, and tags. Uses fuzzy matching so typos and reordered words still work. Returns the best ranked matches with relevance scores.

**Parameters:**

| Name | Type | Required | Description |
|---|---|---|---|
| `spec_url` | string | yes | |
| `query` | string | yes | Free-text query, e.g. "create subscription", "list customers". |
| `method` | string | no | Filter by HTTP method: GET, POST, PUT, PATCH, DELETE. |
| `max_results` | integer | no | Default 10, max 50. |

**Returns:**

```json
{
  "query": "...",
  "results": [
    {
      "operation_id": "PostSubscriptions",
      "method": "POST",
      "path": "/v1/subscriptions",
      "summary": "Create a subscription",
      "score": 0.92,
      "match_reason": "path matches 'subscription'; summary matches 'create'"
    }
  ]
}
```

#### `api_explorer__get_endpoint_details`

**Description:**

> Retrieve the full specification of a single endpoint: parameters (path, query, headers), request body schema, response schemas, and required auth. Call this before `build_http_request` if you need to know what arguments are required.

**Parameters:**

| Name | Type | Required | Description |
|---|---|---|---|
| `spec_url` | string | yes | |
| `operation_id` | string | yes | The operation id from `search_endpoint` or `list_endpoints`. |

**Returns:**

```json
{
  "operation_id": "PostSubscriptions",
  "method": "POST",
  "path": "/v1/subscriptions",
  "summary": "Create a subscription",
  "description": "Long description...",
  "path_parameters": [],
  "query_parameters": [
    { "name": "expand", "type": "array", "required": false, "description": "..." }
  ],
  "header_parameters": [],
  "request_body": {
    "content_type": "application/x-www-form-urlencoded",
    "required": true,
    "schema": { "type": "object", "properties": { "customer": {"type":"string","required":true}, "items": {...} } }
  },
  "responses": {
    "200": { "description": "Success", "schema": { ... } },
    "400": { "description": "Invalid request" }
  },
  "security": [{ "scheme": "BearerAuth" }]
}
```

#### `api_explorer__build_http_request`

**Description:**

> Build a validated HTTP-request configuration for a specific endpoint. The output is a JSON object in the exact shape the `http_request` node accepts — pass it as the input to an `http_request` call to execute. Missing required parameters or wrong types return an error with hints.

**Parameters:**

| Name | Type | Required | Description |
|---|---|---|---|
| `spec_url` | string | yes | |
| `operation_id` | string | yes | |
| `params` | object | yes | A flat map of parameter values. Path params, query params, header params, and body fields are all resolved from the same map. The node routes each to the right location based on the spec. |
| `auth_secret_ref` | string | no | Name of a Secure Value containing the token / API key. Required if the endpoint declares auth. |

**Returns (happy path):**

```json
{
  "url": "https://api.stripe.com/v1/subscriptions",
  "method": "POST",
  "headers": {
    "Authorization": "Bearer ${SECURE:stripe_key}",
    "Content-Type": "application/x-www-form-urlencoded"
  },
  "query_params": {},
  "body": "customer=cus_ABC&items[0][price]=price_XYZ"
}
```

Notes:

- The `Authorization` header uses Colmena's `${SECURE:name}` placeholder, which the downstream `http_request` node resolves through `SecureValueResolver` at execution time. The plaintext secret never enters the LLM context or the returned JSON.
- Body encoding follows the spec's `Content-Type` (form-urlencoded for Stripe, JSON for most others).

**Returns (validation error):**

```json
{
  "error": "missing_required_params",
  "missing": ["customer"],
  "hints": "The 'customer' field is required in the request body. Get a customer id from POST /v1/customers."
}
```

## Architecture

### Domain (`web/domain/`)

```rust
// api_spec_port.rs
#[async_trait]
pub trait ApiSpecPort: Send + Sync {
    async fn fetch_and_parse(&self, url: &str, etag: Option<&str>) -> Result<SpecFetchResult, WebDomainError>;
}

pub enum SpecFetchResult {
    Fresh { spec: ParsedSpec, etag: Option<String> },
    NotModified,
}

pub struct ParsedSpec {
    pub url: String,
    pub title: String,
    pub version: String,
    pub openapi_version: String,
    pub description: Option<String>,
    pub servers: Vec<String>,
    pub endpoints: Vec<Endpoint>,           // flattened by (method, path)
    pub security_schemes: HashMap<String, SecurityScheme>,
    pub tags: Vec<String>,
}

pub struct Endpoint {
    pub operation_id: String,
    pub method: HttpMethod,
    pub path: String,
    pub summary: Option<String>,
    pub description: Option<String>,
    pub tags: Vec<String>,
    pub path_params: Vec<ParameterSpec>,
    pub query_params: Vec<ParameterSpec>,
    pub header_params: Vec<ParameterSpec>,
    pub request_body: Option<RequestBodySpec>,
    pub responses: HashMap<String, ResponseSpec>,
    pub security: Vec<SecurityRequirement>,
}

pub enum SecurityScheme {
    Http { scheme: String, bearer_format: Option<String> },   // bearer, basic
    ApiKey { name: String, location: ApiKeyLocation },         // header | query | cookie
    OAuth2 { flows: ... },                                     // flow metadata; agent provides token
}
```

### Application (`web/application/api_spec_use_case.rs`)

Responsibilities:

- **Spec cache** (LRU by `spec_url`, keyed for revalidation by stored ETag / Last-Modified). Respects `enable_cache`, `cache_ttl_seconds`, `max_cached_specs`. Scoped per `conversation_id` via the shared `SessionRegistry`.
- **Fuzzy search** using `nucleo-matcher` (already tuned for search UIs; Unicode-aware; ~10 LOC to integrate). Scored over concatenated `path + summary + operation_id + tags + description`. `fuzzy_match_threshold` gates which matches are returned.
- **Validation & request construction**:
  - Walk the endpoint's parameter lists.
  - For each spec parameter, look up `params[name]`; if required and missing, collect into `missing`.
  - Coerce types (string → integer for numeric path params, ISO date validation, etc.).
  - Compose URL (substitute path params), query string, headers, body.
  - If `security` is non-empty and `auth_secret_ref` is missing, return `MissingAuth`.
  - Emit the `http_request`-shaped JSON.

```rust
pub struct ApiSpecUseCase {
    port: Arc<dyn ApiSpecPort>,
    registry: Arc<SessionRegistry<SpecCache>>,
    config: ApiSpecUseCaseConfig,
}

struct SpecCache {
    specs: LruCache<String, CachedSpec>,
}

struct CachedSpec {
    parsed: ParsedSpec,
    etag: Option<String>,
    last_modified: Option<String>,
    cached_at: Instant,
}
```

### Infrastructure (`web/infrastructure/openapi_adapter.rs` + `swagger2_to_oas3.rs`)

The adapter is split into a download/normalize stage, a format-detection stage, a (conditional) Swagger-2.0-to-OpenAPI-3.0 conversion stage, and a parse-to-domain stage.

**Download pipeline (in order):**

1. **URL normalization** — before issuing any request, the input URL is matched against known Git-forge patterns and rewritten:
   - `github.com/{owner}/{repo}/blob/{ref}/{path}` → `raw.githubusercontent.com/{owner}/{repo}/{ref}/{path}`
   - `github.com/{owner}/{repo}/tree/{ref}/{path}` → same as above (covers the less-common case)
   - `gitlab.com/{owner}/{repo}/-/blob/{ref}/{path}` → `gitlab.com/{owner}/{repo}/-/raw/{ref}/{path}`
   - `bitbucket.org/{owner}/{repo}/src/{ref}/{path}` → `bitbucket.org/{owner}/{repo}/raw/{ref}/{path}`

   The original and resolved URLs are both tracked and returned.

2. **Fetch** — `reqwest` GET with streaming, honouring `spec_download_timeout_seconds` and `max_spec_size_bytes` (abort mid-stream if the byte counter exceeds the cap).

3. **ETag / Last-Modified revalidation** — if a previous ETag is known, send `If-None-Match`; on `304 Not Modified` return `SpecFetchResult::NotModified`.

4. **HTML-response detection** — if `Content-Type` starts with `text/html`, or if after `Content-Type` absence the first non-whitespace character is `<`, return the `UnexpectedHtmlResponse` error with a hint about using a raw URL.

5. **Format detection (JSON vs YAML)** — by `Content-Type`, file extension, or first non-whitespace character (`{` or `[` → JSON, otherwise YAML).

6. **Root-key detection** — inspect the parsed (but not yet typed) JSON/YAML:
   - If root contains `"swagger": "2.0"` → hand off to the Swagger-2.0 converter.
   - If root contains `"openapi": "3.x"` → skip conversion.
   - Anything else (AsyncAPI, invalid, truncated) → `SpecParseError` with details.

7. **Swagger 2.0 conversion** (`swagger2_to_oas3.rs`, when needed) — pure-Rust JSON-tree transformation; no external deps. Produces an OpenAPI 3.0.3 JSON value. The mapping rules:

   | Swagger 2.0 | OpenAPI 3.0.3 |
   |---|---|
   | `swagger: "2.0"` | `openapi: "3.0.3"` |
   | `host` + `basePath` + `schemes[]` | `servers: [{ url: "{scheme}://{host}{basePath}" }]` (one entry per scheme) |
   | `definitions` | `components.schemas` |
   | `parameters` (global) | `components.parameters` (refs updated) |
   | `responses` (global) | `components.responses` |
   | `securityDefinitions` | `components.securitySchemes` |
   | Per-operation parameter with `in: body` + `schema` | `requestBody: { content: { <consume>: { schema } } }` |
   | Per-operation parameter with `in: formData` | `requestBody` with `multipart/form-data` or `application/x-www-form-urlencoded` schema |
   | `consumes` / `produces` (operation or global) | Folded into the `content` keys of `requestBody` / `responses` |
   | `$ref: "#/definitions/X"` | `$ref: "#/components/schemas/X"` (rewritten everywhere) |
   | `type: file` (formData) | `type: string, format: binary` |
   | `collectionFormat` on array params | OpenAPI 3.0's `style` + `explode` (csv → form/explode=false; multi → form/explode=true; ssv → spaceDelimited; pipes → pipeDelimited; tsv → error, unsupported in 3.0) |

8. **Parse with `oas3`** — the (now guaranteed 3.x) JSON/YAML is handed to `oas3`, then mapped into Colmena's `ParsedSpec` domain value object (isolates the rest of the system from the crate's types).

### Node (`dag_engine/infrastructure/nodes/api_explorer.rs`)

Implements `ToolkitNode`. `sub_tool_catalog()` returns five static `SubToolDefinition`s. `execute()` dispatches on `__sub_tool` — one handler per sub-tool, each validates args, calls the use case, shapes the JSON result.

### Error → LLM mapping

| Domain error | LLM sees |
|---|---|
| Spec not loaded (sub-tool called before `load_spec`) | `{ error: "spec_not_loaded", message: "Call load_spec(url) first." }` |
| `SpecParseError` | `{ error: "spec_parse_failed", details, message: "Spec at <resolved_url> could not be parsed as OpenAPI 3.x or Swagger 2.0." }` |
| `UnexpectedHtmlResponse` | `{ error: "unexpected_html_response", url_given, resolved_url, message: "URL returned HTML. If this is a Git forge 'blob' URL for a lesser-known host, use the raw content URL instead." }` |
| `Swagger2ConversionFailed` | `{ error: "swagger2_conversion_failed", reason, unsupported_feature, message: "This Swagger 2.0 spec uses a feature the converter does not handle. Fall back to reading docs with web__fetch." }` |
| `UnsupportedSpecFormat` (e.g., AsyncAPI, RAML detected) | `{ error: "unsupported_spec_format", detected, message: "api_explorer supports OpenAPI 3.x and Swagger 2.0 only." }` |
| `EndpointNotFound` | `{ error: "endpoint_not_found", searched_for, did_you_mean: [top 3 fuzzy] }` |
| Missing required params in `build_http_request` | `{ error: "missing_required_params", missing: [...], hints }` |
| Type mismatch | `{ error: "invalid_param_type", param, expected_type, got }` |
| Missing `auth_secret_ref` when endpoint requires auth | `{ error: "missing_auth", scheme, message }` |
| Spec too large | `{ error: "spec_too_large", size_bytes, limit_bytes }` |
| `Timeout` / `Upstream` (during fetch) | `{ error: "fetch_failed", url, retryable: true }` |

## Data flow (`build_http_request` end-to-end)

```
LLM has already called:
  api_explorer__load_spec(url=".../stripe-openapi.json") → spec cached
  api_explorer__search_endpoint(query="create subscription") → finds PostSubscriptions

LLM now emits tool_call:
  api_explorer__build_http_request({
    spec_url: "...",
    operation_id: "PostSubscriptions",
    params: { customer: "cus_ABC", "items[0][price]": "price_XYZ" },
    auth_secret_ref: "stripe_key"
  })
       │
       ▼
ApiExplorerNode.execute(__sub_tool=build_http_request)
 ├─ validates inputs
 └─ calls ApiSpecUseCase.build_http_request(...)
       │
       ▼
ApiSpecUseCase.build_http_request
 ├─ fetch cached spec from SessionRegistry[conversation_id]
 ├─ find endpoint by operation_id → PostSubscriptions
 ├─ for each declared param, look up in params map
 │    • path params: substitute into path
 │    • query params: add to query_params
 │    • header params: add to headers
 │    • body fields: add to body (encoded per content-type)
 │ ├─ collect missing required → if any, return MissingRequiredParams
 ├─ apply security: emit Authorization: Bearer ${SECURE:stripe_key}
 ├─ compose final HTTP config JSON
 └─ return to LLM
       │
       ▼
LLM now has a ready-to-execute http_request config.
It can pass this to an `http_request` tool invocation next.
```

## Configuration examples

### Minimal

```json
{ "type": "api_explorer", "config": {} }
```

### As tool of an `llm_call` together with `http_request`

```json
{
  "type": "llm_call",
  "config": {
    "provider": "anthropic",
    "model": "claude-opus-4-7",
    "api_key": "${ANTHROPIC_API_KEY}",
    "tool_configurations": {
      "apis": {
        "node_type": "api_explorer",
        "expose_sub_tools": "all"
      },
      "http": {
        "node_type": "http_request",
        "node_schema": { /* existing shape */ }
      }
    },
    "system_message": "You are an integration agent. Use apis__load_spec to load an OpenAPI URL, apis__search_endpoint to find what you need, apis__build_http_request to construct a valid call, and then invoke http to execute it."
  }
}
```

## Testing

### Unit tests

- `openapi_adapter.rs`: fixture specs in `tests/fixtures/specs/`:
  - `petstore-3.0.yaml` (canonical OpenAPI 3.0 example)
  - `stripe-excerpt-3.0.json` (form-urlencoded body, `components.schemas` with refs)
  - `github-excerpt-3.0.json` (JSON body, Bearer auth)
  - `amadeus-airlines-2.0.yaml` (real Swagger 2.0 spec — the user's concrete case; round-trip through the converter)
  - `petstore-2.0.yaml` (canonical Swagger 2.0 example)
  - `with-form-data-2.0.yaml` (exercises formData → multipart conversion)
  - `collection-formats-2.0.yaml` (covers csv/multi/ssv/pipes/tsv mappings)

  Covers: YAML and JSON parsing, ETag/304, size limit, HTML-instead-of-spec detection, format detection, unsupported format (AsyncAPI sample).

- `swagger2_to_oas3.rs` (conversion module): tested independently against the 2.0 fixtures. Each mapping rule has a focused test; a golden-file test compares converted output against a hand-verified 3.0 equivalent for `petstore-2.0.yaml`.

- `url_normalizer.rs` (small pure module): table-driven tests for each forge pattern (github.com blob/tree, gitlab, bitbucket, unknown hosts passed through unchanged).
- `api_spec_use_case.rs`: cache hit/miss/expire, fuzzy search scoring, `build_http_request` validation matrix (required missing, type mismatch, auth missing, path params, body encoding).
- `api_explorer.rs` node: dispatch on `__sub_tool`, spec-not-loaded error, JSON shape of every response.

### Integration tests

All offline:
- `tests/web/api_explorer.rs`: end-to-end across all sub-tools on each fixture spec.

### Test graphs

- `tests/graphs/web/api_explorer_petstore.json` — LLM loads petstore spec, lists endpoints, builds a `POST /pet` call.
- `tests/graphs/web/api_explorer_stripe.json` — LLM loads a Stripe-like spec and builds a `POST /v1/subscriptions` call.
- `tests/graphs/web/api_explorer_amadeus_swagger2.json` — LLM loads the Amadeus airline-code-lookup spec via its GitHub blob URL, verifies the URL was rewritten to raw.githubusercontent.com, confirms the internal format became OpenAPI 3.0.3, and builds a valid `GET /v1/reference-data/airlines` call.
- `tests/graphs/web/full_flow_discover_use_api.json` — end-to-end: `tavily_client` finds a spec URL → `api_explorer` loads + searches + builds → `http_request` executes. (Shared with Spec A.)

### Bindings

- Python smoke test: node registers, accepts config, fires a `load_spec` sub-tool via a mock LLM.

## Dependencies added to the crate

- `oas3` (OpenAPI 3.x parser, MIT, active maintenance).
- `nucleo-matcher` (fuzzy matcher, MIT, used by Helix editor — very small, no transitive heavy deps).
- `lru` (already likely present; if not, add).

## Rollout

Implementation order (each is a task in the plan):

1. Domain layer: `ApiSpecPort`, `ParsedSpec` and its sub-types, errors.
2. `url_normalizer` module: pure-function Git-forge URL rewriter with table-driven tests.
3. `swagger2_to_oas3` module: standalone pure-Rust JSON-tree converter with the mapping rules table above, plus its fixture tests (including Amadeus and Petstore 2.0).
4. `openapi_adapter`: fetch + URL normalization + HTML detection + format detection + conversion dispatch + parse via `oas3` + ETag revalidation. Fixture-backed unit tests.
5. `ApiSpecUseCase`: cache (using shared `SessionRegistry`), fuzzy search, validation + request construction.
6. `api_explorer` node: `ToolkitNode` impl with five sub-tools.
7. Test graphs and the cross-node end-to-end test graph.
8. Python / TS bindings smoke.
9. Docs: `docs/node_configurations.json`, `docs/agent_context/node_ports_reference.md`, and update `docs/developer_guide/25_web_nodes.md` (introduced by Spec A) with an `api_explorer` section that documents the Swagger 2.0 conversion and URL normalization behaviors explicitly.

The runtime multi-tool extension is already in place from Spec A; this spec reuses it.

## Open questions

- **Behaviour when the spec's `servers[]` is empty**: reject with a clear error asking the user to set `default_base_url_override` in config. Decision: yes, reject — guessing is worse than asking.
- **Body serialization of deeply nested objects for `application/x-www-form-urlencoded`**: Stripe-style bracket notation is the de facto standard (`items[0][price]`). Implement bracket notation; document clearly; non-Stripe users sending JSON bodies won't hit this path.
- **Maximum endpoints per spec**: do we cap? Some specs (AWS) have thousands. No hard cap, but `list_endpoints` enforces pagination; search returns top-N. Memory cost is ~1 KB per endpoint, acceptable.
- **Swagger 2.0 `collectionFormat: tsv`**: OpenAPI 3.0 has no equivalent `style` for tab-separated arrays. Decision: emit a `Swagger2ConversionFailed` error naming `collectionFormat` as the unsupported feature rather than silently degrading. TSV is rare (<1% of specs) and misrepresentation would cause silent request failures — better to surface clearly.
- **URL normalization for private Git instances** (e.g., `git.internal.company.com`): patterns are only recognized for public hosts (github.com, gitlab.com, bitbucket.org). Self-hosted GitLab/Gitea users must pass the raw URL directly, or provide a normalization pattern via config in a future iteration. Out of scope for v1.
