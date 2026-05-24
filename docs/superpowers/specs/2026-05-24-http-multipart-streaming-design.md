# HTTP Multipart Streaming Support — Design Spec

**Status:** Approved (2026-05-24)
**Author:** daniel@startti.co + Claude
**Component:** `dag_engine/infrastructure/nodes/http.rs` + `storage/domain/output_storage_repository.rs`

## Motivation

The `http_request` node today only serializes the body as JSON (`application/json`) or raw text. This blocks two real use cases:

1. **LLM-driven uploads to ADP** — when an agent needs to push user-uploaded files to ADP endpoints like `POST /knowledge-bases/:id/documents`, which mandates `multipart/form-data` with one or more file parts.
2. **Cross-system artifact transfer** — when an agent generates an artifact via `image_generation` / `tts` (stored in `OutputStorageRepository` and referenced as `$attachment:<key>`) and needs to forward it as a file part to an external API.

The primary caller is the LLM via tool calling. A human configures the tool once in `tool_configurations`; from then on the model decides what to upload based on the user's request.

## Constraints

- **Concurrency at scale.** Colmena workers run on Cloud Run with bounded RAM (typically 1–2GB). Buffering full file payloads in memory (current `read` returns `Vec<u8>`) does not survive thousands of concurrent users uploading 50–100MB files.
- **Backwards compatibility.** Existing graphs with JSON bodies must keep working with no changes.
- **Trait stability.** `OutputStorageRepository` is consumed by 4 other nodes (`llm.rs`, `tts.rs`, `image_generation.rs`, `image_edit.rs`). Any change must be additive — no removed or renamed methods.
- **Security.** Signed URLs from arbitrary sources are downloaded by the worker. Must mitigate SSRF (no plain `http://` by default) and resource exhaustion (size caps, timeouts).

## Decisions

### D1 — Activation: `Content-Type: multipart/*` header

The node enters multipart mode **if and only if** the resolved headers (config + input) contain `Content-Type` with prefix `multipart/`. No new top-level config flag.

The header value the user provides is treated as the MIME type indicator only. Reqwest's `multipart::Form` generates the boundary automatically and the node lets reqwest set the final `Content-Type: multipart/form-data; boundary=<uuid>` header. Any user-supplied boundary is overwritten.

**Rejected alternatives:**

- `multipart: true` flag — adds a new field that can disagree with `Content-Type`; gives two sources of truth.
- Auto-detection by body content — fragile, breaks legitimate JSON bodies that happen to contain URL strings.

### D2 — Body schema in multipart mode

The `body` field (object) is interpreted as a map of `field_name -> value`. Each value is resolved by the following precedence:

| Value shape | Interpretation |
|---|---|
| String starting with `$attachment:<storage_key>` | File part. Bytes streamed from `OutputStorageRepository::read_stream`. Filename and MIME come from `StoredStream`. |
| String matching `^https?://` | File part. HEAD request validates size and resolves MIME/filename, then GET streams the body into the outgoing form. |
| Any other string | Text part (form field with text value). |
| Array | Expands to N parts with the same `field_name`. Each element is resolved by the rules above. |
| Object `{ "url": "...", "filename": "...", "content_type": "..." }` | Explicit file part with URL source and metadata overrides. `filename` and `content_type` keys override what HEAD would derive. |
| Object `{ "attachment": "<storage_key>", "filename": "...", "content_type": "..." }` | Explicit file part with attachment source and metadata overrides. |
| Object `{ "value": "...", "content_type": "..." }` | Explicit text or inline-binary part with content-type override. |
| Number or boolean | Coerced to its JSON string representation and emitted as a text part. |
| `null` | The field is omitted entirely (no part emitted). |

The explicit object forms exist as escape hatches; the string forms are the canonical LLM-facing surface.

**Example — KB upload (typical LLM tool call):**

```json
"body": {
  "files": ["https://storage.googleapis.com/.../signed1", "https://storage.googleapis.com/.../signed2"]
}
```

**Example — overrides:**

```json
"body": {
  "files": [{ "url": "https://...", "filename": "report.pdf", "content_type": "application/pdf" }],
  "metadata": "uploaded by agent"
}
```

### D3 — Streaming for all sources (Option D)

Both URL-sourced parts and `$attachment:`-sourced parts use streaming. No file payload is fully buffered in worker RAM.

For URLs: HEAD pre-flight returns `Content-Length`; then `client.get(url).send().await?.bytes_stream()` is wrapped in `reqwest::multipart::Part::stream_with_length(stream, len)`.

For attachments: a new `OutputStorageRepository::read_stream` returns a `StoredStream { stream, size_bytes, mime_type, filename }` consumed the same way.

**Why streaming-everywhere (not URL-only):** the user is targeting thousands of concurrent users on shared Cloud Run workers. With URL-only streaming (Option B in the brainstorming), a single concurrent batch of large generated attachments (e.g., long TTS clips, future video artifacts) can still exhaust RAM. Closing that hole now avoids a hidden ceiling.

**Trade-off accepted:** extending the `OutputStorageRepository` trait touches 3 adapter implementations. This blast radius is justified by the scale requirement.

### D4 — `OutputStorageRepository` extension

Add one new method, leave `read` and `store` unchanged. Concrete signature:

```rust
use bytes::Bytes;
use futures::Stream;
use std::pin::Pin;

pub struct StoredStream {
    /// Pinned, boxed async stream of body chunks. Required shape for
    /// `reqwest::multipart::Part::stream_with_length`.
    pub stream: Pin<Box<dyn Stream<Item = Result<Bytes, StorageError>> + Send>>,
    pub size_bytes: u64,
    pub mime_type: String,
    pub filename: String,
}

#[async_trait]
pub trait OutputStorageRepository: Send + Sync {
    async fn store(&self, req: StoreRequest) -> Result<StoredOutput, StorageError>;
    async fn read(&self, storage_key: &str) -> Result<StoredBytes, StorageError>;

    /// Streaming counterpart to [`read`]. Used by `http_request` multipart
    /// mode to avoid buffering full payloads in worker RAM.
    ///
    /// Returns `StorageError::InvalidInput` for unknown `storage_key`.
    /// Returns `StorageError::UpstreamUnavailable` if the underlying source
    /// cannot be reached (relevant for `LocalHttp` / `HttpCallback` adapters).
    async fn read_stream(&self, storage_key: &str) -> Result<StoredStream, StorageError>;
}
```

**Adapter implementations:**

- **`LocalCacheStorageAdapter`** — bytes already in `HashMap<String, Vec<u8>>`. Implementation: `futures::stream::once(async move { Ok(Bytes::from(vec)) })` boxed and pinned. No real RAM reduction (bytes already alive) but provides a uniform API for callers. Size known.
- **`LocalHttpStorageAdapter`** — switches from `client.get(url).bytes().await` to `client.get(url).send().await?` and consumes `response.bytes_stream()`. `size_bytes` from response `Content-Length`. MIME and filename from a HEAD or from the existing metadata path the adapter already has.
- **`HttpCallbackStorageAdapter`** — same pattern as `LocalHttp`, fetching from the callback-resolved signed URL.

**Mocks:** `mockall::automock` regenerates the mock automatically.

### D5 — URL pre-flight (HEAD)

Before downloading the body of any URL part:

1. `HEAD` with timeout `url_download_timeout_secs` (default 30s).
2. Require non-empty `Content-Length` in the response. If absent → fail the part with `UrlValidationFailed { reason: "missing content-length" }`.
3. Compare `Content-Length` against `max_file_size_bytes`. If greater → `FileTooLarge`.
4. Derive `content_type` from response `Content-Type` header; fallback `application/octet-stream`.
5. Derive `filename` from explicit override (object form) → `Content-Disposition: attachment; filename=...` → last URL path segment → `file`.
6. Then issue `GET` to the same URL and pipe `bytes_stream()` into `Part::stream_with_length(stream, len)`.

**Rejected alternative — counting reader without HEAD:** would allow upstream providers that don't support HEAD, but a partial multipart request is already in flight to the downstream when the cap trips, making error semantics messy. We can add `allow_unverified_size: true` later as an opt-in if a real provider demands it.

### D6 — Configurable limits

New optional `config_fields` on `http_request`. All defaults are sized for ADP's KB endpoint and current GCS-backed flows.

| Field | Type | Default | Description |
|---|---|---|---|
| `max_file_size_bytes` | integer | `104857600` (100 MiB) | Hard cap per file part. Validated against `Content-Length` (URL) or `size_bytes` (attachment). |
| `max_parts` | integer | `10` | Maximum total parts (file + text combined) per request. |
| `url_download_timeout_secs` | integer | `30` | Applied to (a) the HEAD pre-flight as a total request timeout and (b) the GET request's connect + response-headers phase. **Not** applied to body-transfer duration (that phase is bounded by `max_file_size_bytes` through the streamed reader). This avoids spurious aborts when transferring large files over slow links. |
| `allow_http_urls` | boolean | `false` | When `false`, plain `http://` URLs are rejected. SSRF / MitM mitigation. |

Limits are checked in this order: `max_parts` first (cheap, no I/O), then per-part `max_file_size_bytes` after HEAD/metadata.

### D7 — All-or-nothing failure policy

Any single-part failure (HEAD error, oversize, scheme not allowed, unknown `storage_key`, parts limit exceeded) aborts the request **before any multipart body bytes are sent downstream**. The downstream never sees a partial form.

Implementation: resolve all parts first (HEAD + metadata lookups in parallel via `try_join_all`), then construct the `multipart::Form` only after every part is validated. Stream consumption happens during `client.post(...).multipart(form).send().await`.

If a stream errors **mid-flight** (e.g. GCS connection drop during the GET), the downstream request fails with an incomplete body and the node returns `StreamInterrupted { part_field_name, part_index, source_error }`. No retry in v1; the LLM agent loop can retry the whole tool call.

### D8 — Security

- **`https://` by default.** Plain `http://` requires explicit `allow_http_urls: true`. This is the primary SSRF mitigation surface.
- **No localhost / link-local IP blocking in v1.** Workers in Cloud Run don't have meaningful access to private networks beyond what their VPC connector grants. If we later run workers in environments where the network surface matters, add an `ip_allowlist` / `block_private_ips` flag.
- **Content-Length is trusted from HEAD.** If a provider deliberately lies (declares small, streams huge), the downstream will receive `Content-Length`-mismatched parts and reject. The worker is not infinitely vulnerable because the chunked streaming has natural backpressure — reqwest will not buffer the full body even if `Content-Length` is wrong.
- **`secure_values`.** Signed URLs themselves can be sensitive (contain auth in the query string). The existing `secure` flag and never-log-body policy in `http.rs` (lines 348, 358) continue to apply unchanged in multipart mode.

### D9 — Tool calling ergonomics

Canonical `tool_configurations` entry for the KB upload tool:

```json
"tool_configurations": {
  "upload_to_kb": {
    "node_type": "http_request",
    "node_schema": {
      "base_url":      { "fixed": "${ADP_API_BASE_URL}" },
      "endpoint":      { "type": "string", "required": true, "description": "Path including KB id, e.g. /knowledge-bases/<kb_id>/documents" },
      "method":        { "fixed": "POST" },
      "headers":       { "fixed": { "Content-Type": "multipart/form-data" } },
      "authorization": { "fixed": "Bearer ${ADP_SESSION_TOKEN}" },
      "body": {
        "files": {
          "type": "array",
          "items": { "type": "string", "description": "Signed URL or $attachment:<storage_key>" },
          "required": true,
          "description": "Files to upload as multipart parts"
        }
      }
    }
  }
}
```

The LLM sees only `endpoint` and `files`. Everything else (method, auth, content-type) is invisible.

## Architecture

### Module layout

- `src/libs/colmena/src/dag_engine/infrastructure/nodes/http.rs` — multipart branch + body schema parser. Reuses existing `resolve_attachment_placeholders` plumbing for the storage handle.
- `src/libs/colmena/src/storage/domain/output_storage_repository.rs` — new `StoredStream` type + `read_stream` method on the trait.
- `src/libs/colmena/src/storage/infrastructure/local_cache_adapter.rs` — `read_stream` impl wrapping the in-memory `Vec<u8>` as a single-chunk stream.
- `src/libs/colmena/src/storage/infrastructure/local_http_adapter.rs` — `read_stream` impl via `client.get(url).send()` + `bytes_stream()`.
- `src/libs/colmena/src/storage/infrastructure/http_callback_adapter.rs` — `read_stream` impl via callback-resolved URL + `bytes_stream()`.
- `Cargo.toml` — verify `reqwest` has `multipart` and `stream` features enabled; add if missing.

### Data flow

```
LLM tool call
   │   files: ["https://signed-1", "$attachment:abc"]
   ▼
http_request node
   │   detects Content-Type: multipart/*
   ▼
parse body → Vec<PartSpec>
   │   PartSpec::Url("https://signed-1")
   │   PartSpec::Attachment("abc")
   ▼
resolve in parallel (try_join_all)
   │   URL  → HEAD → (size, mime, filename)
   │   Attach → storage.read_stream → StoredStream
   ▼
build reqwest::multipart::Form
   │   each PartSpec → Part::stream_with_length(stream, len)
   ▼
client.post(target).multipart(form).send()
   │   downstream sees: Content-Type: multipart/form-data; boundary=...
   │                    Body: streamed chunks, never fully buffered
   ▼
Response → { status, body }
```

### New error variants

Added to whatever the `http_request` node returns today (likely surfaced as `serde_json::Value` errors in the output):

- `UrlValidationFailed { url, reason }` — HEAD failed, missing Content-Length, unsupported scheme
- `FileTooLarge { field, declared_size, max }` — pre-flight size check
- `TooManyParts { count, max }`
- `AttachmentNotFound { storage_key }` — passed through from storage error
- `StreamInterrupted { field, part_index, source }`
- `MultipartConfigError { reason }` — body shape doesn't match expected schema (e.g. nested object that isn't one of the explicit forms)

## Testing

### Unit — `http.rs`

- Detection of multipart mode by header (case-insensitive, with and without trailing parameters)
- Body parser: every entry in the value-shape table above (12+ cases)
- Errors: oversize HEAD, missing Content-Length, http:// without opt-in, http:// with opt-in, unknown attachment key, more than `max_parts`, malformed object form
- Limits: defaults, overrides via `config_fields`
- Backwards compat: JSON body unchanged when Content-Type is `application/json` or missing

### Unit — storage adapters

- `LocalCacheStorageAdapter::read_stream` — returns single-chunk stream with correct `size_bytes`, `mime_type`, `filename`
- `LocalHttpStorageAdapter::read_stream` — mock HTTP upstream, verify chunked consumption and `Content-Length` propagation
- `HttpCallbackStorageAdapter::read_stream` — same pattern

### Integration — `tests/`

- New `tests/multipart_http_test.rs` using `wiremock`:
  - Mock upstream serving 3 signed URLs (small + medium + edge-case sized payloads)
  - Mock downstream `POST /upload` capturing multipart boundary and parts
  - Verify: boundary present, exactly N file parts with correct field names, sizes match
  - Verify: oversized upstream → request never reaches downstream
- New `tests/graphs/external/multipart_upload.json` runnable via the DAG engine CLI

### Manual smoke

Once implemented, validate end-to-end against ADP dev:

```bash
source .env
cargo run --bin dag_engine -- run tests/graphs/external/multipart_upload.json \
  --agent-session-id multipart_smoke_001
```

with a real KB id and 2 small files served from a temporary signed URL.

## Documentation

- `docs/node_configurations.json` — update `http_request` node entry:
  - description mentions multipart mode and its activation
  - new `config_fields`: `max_file_size_bytes`, `max_parts`, `url_download_timeout_secs`, `allow_http_urls`
  - new `output_ports` / error structure if applicable
- `docs/developer_guide/25_web_nodes.md` — new section "Multipart uploads" with the body schema table and a worked example
- `docs/agent_context/node_ports_reference.md` — note multipart behavior
- `docs/DEVELOPER_GUIDE.md` index — link to the new section

## Out of scope

These items are intentionally deferred:

- **Retries on stream interruption** — the LLM tool-call loop can retry the whole call. Mid-stream retries would require seekable sources and add complexity.
- **HTTP/2 server push or chunk-size tuning** — defaults from `reqwest` are sufficient.
- **`allow_unverified_size` opt-in** for providers without HEAD support — add when a real case appears.
- **IP allowlists / private network blocking** — Cloud Run + VPC connector already constrains the surface.
- **Per-tool overrides of the global limits via input** — limits are config-only in v1. If the LLM should be able to vary the cap, expose it later via `node_schema`.
- **Streaming for response bodies** — this spec covers only **request** streaming. Response bodies stay parsed as today.

## Risks and mitigations

| Risk | Mitigation |
|---|---|
| `LocalHttp` or `HttpCallback` adapter doesn't preserve `Content-Length` reliably across the proxy chain | Adapter implementation reads `Content-Length` from the HTTP response of the inner GET; if missing, returns `StorageError::UpstreamUnavailable` and the node fails the request cleanly. |
| `reqwest` `stream` feature might already be in use indirectly but not enabled in our `Cargo.toml` | Implementation plan opens with a check of `Cargo.toml` features; enable `["multipart", "stream"]` if missing. |
| The mock generated by `mockall::automock` may not handle the `Pin<Box<dyn Stream + Send>>` return type cleanly | Confirm with a focused test before committing the trait change; if `automock` chokes, drop down to a manual mock just for `read_stream`. |
| Existing graphs that happen to set `Content-Type` to a multipart type but pass a JSON body | Documented breaking change — these graphs were not functional today (server would reject) so this is not a real regression, but call it out in the changelog. |
| Streaming an attachment from `LocalCacheStorageAdapter` doesn't actually reduce RAM | Documented in §D4. Acceptable because the local-cache adapter is for tests/CLI only; production uses `HttpCallback`. |
