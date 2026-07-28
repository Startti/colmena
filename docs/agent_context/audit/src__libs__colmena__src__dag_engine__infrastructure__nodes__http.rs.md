# src/libs/colmena/src/dag_engine/infrastructure/nodes/http.rs

**Layer:** infrastructure  
**Purpose:** Implements HTTP request node (`ExecutableNode`) for DAG execution, supporting GET/POST/PUT/DELETE with headers, query params, OAuth2, multipart form-data, and attachment streaming.

## Symbols

### Types & Structs
- `HttpNode` (struct, pub) — Stateless HTTP request executor; wires optional storage, attachment resolver, and OAuth cache
- `PartSpec` (enum, pub(crate)) — Single resolved multipart form part: Url, Attachment, or Text
- `ResolvedUrlPart` (struct, pub(crate)) — URL resolution result with streaming reader, size, content-type, filename
- `MultipartUrlResolver` (struct, pub(crate)) — Validates and downloads multipart parts from URLs, enforcing size/timeout limits and scheme checks
- `StubResolver` (struct, test-only) — Mock implementation of `AttachmentStreamResolver` for testing

### Constants
- `ATTACHMENT_PLACEHOLDER_PREFIX` (const str) — `"$attachment:"`
- `URL_HTTP_PREFIX` (const str) — `"http://"`
- `URL_HTTPS_PREFIX` (const str) — `"https://"`
- `HttpNode::RESERVED_KEYS` (const array) — 10 keys never sent as query params (base_url, endpoint, method, headers, body, query_params/query_parameters, bearer_token, authorization, secure)
- `HttpNode::DEFAULT_MAX_FILE_SIZE_BYTES` (const) — 104857600 (100 MiB)
- `HttpNode::DEFAULT_MAX_PARTS` (const) — 10
- `HttpNode::DEFAULT_URL_DOWNLOAD_TIMEOUT_SECS` (const) — 30

### Constructors & Builders
- `HttpNode::new()` (fn, pub) — Create HttpNode with all optional fields set to None
- `HttpNode::with_storage()` (fn, pub) — Wire output storage adapter for `$attachment:` resolution in JSON bodies
- `HttpNode::with_attachment_resolver()` (fn, pub) — Wire Plan A attachment resolver (document_id namespace with fallback to storage_key)
- `HttpNode::with_oauth_cache()` (fn, pub) — Wire shared OAuth provider cache for refresh_token grant

### Helpers (Private)
- `HttpNode::is_engine_internal()` (fn, private) — Check if key is engine-injected bookkeeping (`__colmena*` or `__node*`)
- `HttpNode::collect_extra_query_params()` (fn, private) — Filter inputs to extract non-reserved primitives for query string
- `HttpNode::resolve_env_vars()` (fn, private) — Replace `${VAR_NAME}` with `std::env::var`; returns error if var not found
- `HttpNode::resolve_env_vars_in_value()` (fn, private) — Recursively apply env-var resolution to all string leaves in JSON
- `HttpNode::resolve_oauth_provider()` (fn, private async) — Parse `auth` block from config/inputs, validate mutual exclusion with bearer_token/authorization, return provider or error
- `HttpNode::resolve_attachment_placeholders()` (fn, private async) — Recursively replace `$attachment:<id>` strings with base64-encoded data: URIs
- `HttpNode::limit_u64()` (fn, private) — Extract u64 from config.get(key) with fallback to default
- `HttpNode::limit_usize()` (fn, private) — Extract usize from config.get(key) with fallback to default
- `HttpNode::limit_bool()` (fn, private) — Extract bool from config.get(key) with fallback to default
- `HttpNode::is_multipart_mode()` (fn, pub(crate)) — Case-insensitive check for `multipart/` in Content-Type header
- `HttpNode::parse_multipart_body()` (fn, pub(crate)) — Convert JSON body object to flat Vec<PartSpec>; errors on non-object or unrecognized object shapes
- `HttpNode::push_parts_for_value()` (fn, private) — Recursive parser for multipart value: null → skip, string → classify, number/bool → text, array → expand, object → extract url/attachment/value
- `HttpNode::classify_string_part()` (fn, private) — Classify string as `$attachment:` (Attachment), http(s):// (Url), or plain (Text)
- `HttpNode::execute_multipart()` (fn, private async) — Branch path for multipart requests: parse body → resolve URLs/attachments → build form → POST with merged headers
- `HttpNode::add_part_to_form()` (fn, private async) — Stream a single PartSpec into reqwest multipart form (Text direct, Url/Attachment via streaming)

### HTTP Execution (ExecutableNode impl)
- `ExecutableNode::execute()` (fn impl, pub async) — Main entry point: parse config/inputs (inputs > config priority), resolve env vars, detect multipart vs JSON, handle OAuth/bearer/authorization, execute request, return `{status, body}`
- `ExecutableNode::description()` (fn impl, pub) — "Make HTTP requests to external APIs. Supports GET, POST, PUT, DELETE methods with custom headers and query parameters."
- `ExecutableNode::default_output()` (fn impl, pub) — `"body"` (JSON response body)
- `ExecutableNode::schema()` (fn impl, pub) — Returns JSON schema with config/inputs/outputs structure

### URL Helpers (Private)
- `filename_from_disposition()` (fn, private) — Parse RFC 5987 Content-Disposition header for filename; returns None for absent/malformed; RFC 5987 `filename*=` not yet handled
- `filename_from_url_path()` (fn, private) — Extract last URL path segment (URL-decoded) as fallback filename; falls back to `"file"` if empty

### Test Modules
- `attachment_placeholder_tests` (mod, cfg(test)) — 3 tests: placeholder resolution to data: URI, no placeholder pass-through, error without storage
- `multipart_detection_tests` (mod, cfg(test)) — 6 tests: case-insensitive detection, boundary param, various MIME types, non-multipart rejection
- `multipart_body_parser_tests` (mod, cfg(test)) — 8 tests: string URL/attachment/text classification, arrays, explicit objects with overrides, null handling, malformed rejection
- `multipart_url_resolution_tests` (mod, cfg(test)) — 8 tests: size/MIME derivation, 404 rejection, HTTP rejection (when disabled), scheme validation, no-HEAD assertion (V4-signed URL safety), file-size cap, Content-Disposition parsing
- `multipart_execute_tests` (mod, cfg(test)) — 5 tests: two-URL multipart POST, attachment streaming via storage, resolver-based resolution, resolver without agent_session_id error, too-many-parts error, existing JSON path unaffected
- `oauth_integration_tests` (mod, cfg(test)) — 4 tests: with_oauth_cache builder, successful token fetch + API call, revoked token error, auth + bearer_token mutual exclusion rejection
- `extra_query_params_tests` (mod, cfg(test)) — 4 tests: engine-internal keys filtered, reserved keys filtered, LLM-supplied primitives pass through, non-primitives ignored

## File-level notes

- **Logging via println! (improvement flag)**: Lines 713, 717, 903, 1090, 1099 use `println!` for HTTP call tracing. Production should use a proper logging framework (`tracing`, `log`, etc.) to avoid polluting stderr and support log-level control.

- **OAuth2 v1 limitation (by design)**: Native OAuth (`auth` block) does not yet support multipart requests (line 1033-1037 rejects with clear error). Multipart has its own request path (`execute_multipart`) that doesn't wire OAuth.

- **Plan A / Plan B attachments coexist**: The node implements both direct storage (`OutputStorageRepository::read`) and Plan A resolver (`AttachmentStreamResolver`). Resolver takes priority when wired; fallback to storage for legacy graphs (lines 779-806).

- **Multipart parser is exhaustive**: Handles all value shapes (null skip, scalar text, URL string, `$attachment:` string, explicit objects with url/attachment/value keys, arrays expanded per-field). Error messages distinguish between parse failure and validation failure.

- **Environment variable resolution**: Applied to base_url, endpoint, headers, query_params, bearer_token, authorization, and body strings before sending. Failed resolution errors immediately. Used as primary mechanism for injecting secrets (API keys, tokens) into delivered graphs.

- **Test coverage is comprehensive**: 30+ assertions across 7 test modules cover happy path, error cases, edge cases (oversized files, missing Content-Length, malformed Content-Disposition, too many parts, revoked tokens, conflicting auth methods, leaked engine-internal params).

- **Code style note (minor)**: Line 201-203 has unused binding `let _ = rest;` before extracting substring from `chunk`. Not a logic error, just unconventional; the intent is clear (extract text after "filename=" from original chunk string).

- **Async/await throughout**: All I/O paths (HTTP requests, storage reads, OAuth token mints) are async; `#[async_trait]` on `ExecutableNode` impl allows async `execute()`.
