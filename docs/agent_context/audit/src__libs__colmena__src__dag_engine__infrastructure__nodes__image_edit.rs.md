# src/libs/colmena/src/dag_engine/infrastructure/nodes/image_edit.rs

**Layer:** infrastructure  **Purpose:** Implements the `image_edit` node — edits an existing image via OpenAI's DALL-E API given a text prompt, fetching source and optional mask from URLs (data:, http(s), or storage handles) and returning edited results with uniform attachment registration.

## Symbols

- `ImageEditNode` (struct, pub) — holds storage adapter, HTTP client, secure values service, and attachment registry; manages infrastructure dependencies for image editing

- `ImageEditNode::new` (fn, pub) — constructs a new node with storage adapter and HTTP client; secure values and registry are optional

- `ImageEditNode::with_secure_values` (fn, pub) — builder to inject secure values service for resolving API key placeholders

- `ImageEditNode::with_attachment_registry` (fn, pub) — builder to inject attachment registry for auto-registration of edited images

- `ImageEditNode::with_openai_base_url` (fn, pub cfg(test)) — test-only builder to override OpenAI endpoint URL for wiremock testing

- `ImageEditNode::openai_base_url` (fn, private) — returns OpenAI base URL from config or test override; respects `#[cfg(test)]`

- `ImageEditNode::resolve_env_var` (fn, private) — parses `${ENV_NAME}` syntax and resolves to environment variable value; error if not set

- `ImageEditNode::fetch_image` (async fn, private) — fetches image bytes from `local://` (storage), `chat-attachments/` (storage), `data:` URI, or `http(s)` URL; detects MIME type; returns bytes and MIME

- `ExecutableNode` impl block (impl, pub) — trait implementation for DAG engine node execution

  - `execute` (async fn, pub) — main workflow: extract session IDs; resolve secure values; parse config/inputs (provider/model/api_key/prompt/source_url/mask_url/n/size/quality); fetch source and mask bytes; build multipart form; POST to OpenAI `/v1/images/edits`; decode response (b64_json or url); persist each result via storage adapter; auto-register in attachment registry (fail-soft); return JSON array of `{ document_id, mime_type, size_bytes, description }` per edited image

  - `schema` (fn, pub) — returns JSON schema documenting node config fields (provider, model, api_key, source_url, mask_url, prompt, size, quality, n) and output shape

  - `description` (fn, pub) — returns human-readable description explaining node purpose and output format; mentions chaining with `image_generation` and `$attachment:<document_id>` placeholders

  - `default_input` (fn, pub) — returns `"source_url"` as the default input field for the DAG

  - `default_output` (fn, pub) — returns `"output"` as the default output field for the DAG

- `tests` module (mod, cfg(test)) — integration and unit tests for image editing workflow

  - `stored_ok` (fn, private) — helper that returns a mock `StoredOutput` with minimal fields

  - `base_config` (fn, private) — helper that builds a valid `image_edit` config JSON with provider, model, api_key, source_url, prompt

  - `happy_path_fetches_source_posts_multipart_and_stores` (async test) — verifies happy path: fetch source image via HTTP, call OpenAI API, store result, and emit document_id in output

  - `data_uri_source_is_decoded_locally_without_http` (async test) — verifies data: URI source is decoded without making HTTP requests for the fetch step

  - `missing_source_url_errors` (async test) — verifies error when source_url is omitted from config

  - `missing_prompt_errors` (async test) — verifies error when prompt is omitted from config

  - `unsupported_provider_errors` (async test) — verifies error when provider is not "openai"

  - `openai_error_propagates` (async test) — verifies OpenAI API errors (e.g., 400 bad mask) are propagated to caller with status and body

  - `source_fetch_404_errors` (async test) — verifies 404 response when fetching source URL is surfaced with clear error message

  - `inputs_source_url_overrides_config` (async test) — verifies inputs take precedence over config for prompt and source_url fields; description reflects the inputs value

  - `session_ids_forwarded_to_storage` (async test) — verifies `__colmena_session_id` and `__colmena_agent_session_id` are extracted from inputs and passed to storage.store()

  - `image_edit_auto_registers_artifact_in_registry` (async test) — verifies edited image is auto-registered in attachment registry with document_id, storage_key, generated_by:image_edit origin, and ProviderKind::Generated

  - `no_registry_means_no_registration_but_still_emits_document_id` (async test) — verifies that when no registry is provided, document_id is still emitted in tool result and node does not crash

## File-level notes

- **Repetitive input/config resolution pattern** (lines 184–259): Eight similar blocks extract a value from inputs with fallback to config, applying `.as_type()`, `.map()`, and `.ok_or()` or `.unwrap_or()`. Could be simplified via a generic helper method to reduce boilerplate and improve maintainability.

- **Test fixture duplication**: MockServer setup for OpenAI endpoint appears in multiple tests (lines 497, 562, 676, 715, 760, 846). These could share a common fixture builder, though individual test clarity is still good.

- **Plan B (D8) design milestone** referenced in comments (lines 407, 542, 806) documenting removal of legacy `attachment_id` and `url` fields in tool result; tests verify the contract change.

- **Comprehensive error handling**: All URL schemes (data:, http(s), local://, storage) are validated with clear error messages; OpenAI API errors propagate with status and body detail; empty response bodies are detected.

- **Fail-soft registry registration** (lines 370–404): If attachment registry upsert fails, only a warning is logged and execution continues — appropriate for non-critical side effects.

- **Config field precedence**: Inputs override config, with fallback patterns for optional fields (n, size, quality) and sensible defaults (model → gpt-image-1, n → 1, max n → 10).

- **Test coverage is comprehensive**: happy path, error cases (missing fields, 404, API failure, bad provider), inputs-override-config, session ID forwarding, registry auto-registration, and no-registry fallback.
