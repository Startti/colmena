# src/libs/colmena/src/dag_engine/infrastructure/nodes/image_generation.rs

**Layer:** infrastructure  
**Purpose:** Implements a DAG node (`ImageGenerationNode`) that generates images from text prompts via OpenAI (DALL-E) or Google Vertex AI (Imagen), persists bytes through a storage repository, and auto-registers generated artifacts in the attachment registry for downstream tool consumption.

## Symbols

### Module-level
- `VERTEX_SCOPE` (const, public) — OAuth 2.0 scope for Google Cloud Platform access

### Structs
- `CachedToken` (struct, private) — Holds a bearer token string and its expiry `Instant`
- `ImageGenerationNode` (struct, public) — Main node impl; holds storage repository, HTTP client, optional secure-value service, optional attachment registry, and cached Vertex token with test-override URL field

### ImageGenerationNode methods (impl)
- `new(storage)` (pub fn) — Creates node with required storage repository; other fields default to None or empty
- `with_secure_values(svc)` (pub fn) — Builder to attach secure-value service for decryption
- `with_attachment_registry(reg)` (pub fn) — Builder to attach attachment registry for auto-registration
- `with_openai_base_url(url)` (fn, test-gated) — Test-only builder override for wiremock/mock OpenAI endpoint
- `resolve_env_var(value)` (fn, private) — Resolves `${VAR}` placeholder strings against `std::env`; passes non-placeholder strings through
- `openai_base_url(&self)` (fn, private) — Returns OpenAI base URL, checking test override first

### ExecutableNode trait impl
- `execute(inputs, config, state, observer)` (async) — Main execution: injects secure values, routes to provider (OpenAI or Google), stores images via repository, auto-registers in attachment registry (fail-soft), returns `{ output: { images: [...], provider, model } }`
- `schema()` (fn) — Returns JSON schema documenting all config fields (provider, model, api_key, prompt, size, quality, n, google_project_id, google_location)
- `description()` (fn) — Returns user-facing description of image generation capability
- `default_input()` (fn) — Returns `"prompt"`
- `default_output()` (fn) — Returns `"output"` (the top-level key in the result value)

### Provider-specific execution
- `openai_generate(api_key, model, prompt, n, size, quality)` (async, private) — Calls OpenAI `/v1/images/generations` endpoint, handles both base64-embedded (`b64_json`) and URL-based responses, decodes base64 to bytes
- `vertex_generate(project, location, model, prompt, n)` (async, private) — Calls Vertex AI `:predict` endpoint via constructed URL, extracts base64 bytes and MIME type from `predictions` array, decodes to bytes
- `get_vertex_token()` (async, private) — Caches Vertex AI bearer token via `yup-oauth2::ApplicationDefaultCredentialsAuthenticator`, refreshes when within 60 seconds of expiry, supports both service-account and runtime metadata credential sources

### Test module
- `stored_ok(key)` (fn) — Test helper returning a mock `StoredOutput`
- Multiple `#[tokio::test]` cases covering: OpenAI happy path, `n=2` multi-image, HTTP error propagation, missing config, inputs-over-config routing, unknown provider, Google without project, session-id forwarding, prompt override, env-var resolution, attachment registry auto-registration, no-registry edge case

## File-level notes

- **Architecture pattern:** Dual-source config resolution (inputs → config → env fallback) enables both DAG chaining and LLM tool use; clear pattern in lines 203–218 and 264–297.
- **Hexagonal pattern:** Storage and attachment registry are injected as trait objects (`Arc<dyn>`), external HTTP client is `reqwest::Client`, credentials resolved via env or config.
- **Fail-soft attachment registration (line 349–381):** Registry errors are logged as warnings but do not fail image generation; `load_attachment` may not see the output if registration fails, but the image bytes still persist and `document_id` is still valid.
- **Google env fallback (line 280–297):** Follows canonical Google Cloud convention (`GOOGLE_CLOUD_PROJECT` / `GOOGLE_LOCATION`) with fallback names, allowing same graph JSON across dev/staging/prod without hard-coded project IDs.
- **OpenAI format handling (line 495–506):** Handles both `b64_json` (gpt-image-1) and `url` (dall-e-3 default, unless explicitly requested as base64); URL responses are fetched and converted to bytes.
- **Vertex token caching (line 587–622):** Uses `tokio::sync::Mutex` with `Instant` for thread-safe, non-blocking refresh; conservative ~50-minute TTL (yup-oauth2 tokens last ~1 hour).
- **Session ID forwarding (line 175–182):** Engine-injected `__colmena_session_id` and `__colmena_agent_session_id` flow through to storage and attachment registry, scoping generated artifacts to conversation/agent context.
- **Prompt preview (line 314):** Truncated to 80 characters for description display; full prompt is sent to provider.
- **MIME type fallback:** OpenAI responses default to `image/png`; Vertex responses read MIME from response, defaulting to `image/png` if missing.
- **Test coverage:** 11 test cases covering happy paths, multi-image, error handling, config sources, session propagation, env-var resolution, and registry auto-registration.
- **No breaking changes to public API:** `ExecutableNode` trait signature unchanged; `ImageGenerationNode::execute` always returns `{ output: { images, provider, model } }` with Plan B output shape (no legacy `attachment_id` or `url` fields).

